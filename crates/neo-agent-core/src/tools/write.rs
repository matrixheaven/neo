use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::diff::{diff_stats, unified_diff};
use super::{Tool, ToolContext, ToolFuture, ToolResult, schema};
use crate::approval::{WriteApprovalChange, WriteApprovalPresentation, WriteApprovalPreview};
use crate::permissions::{FileWriteApprovalOperation, SessionApprovalKey, SessionApprovalScope};
use crate::session::atomic_file::{
    AtomicWriteStatus, create_missing_directories_recording, replace_existing_file_atomic_status,
    validate_safe_directory, write_file_atomic_create_new,
};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteInput {
    #[schemars(
        description = "Ordered list of files to create or completely overwrite. Declaration order is the commit order."
    )]
    files: Vec<WriteFileInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteFileInput {
    #[schemars(
        description = "Path to the file to create or overwrite. Relative paths resolve against the working directory."
    )]
    path: PathBuf,
    #[schemars(
        description = "Complete UTF-8 content for the file. Empty content is valid for a new file."
    )]
    content: String,
}

/// Immutable per-file operation classification, decided once during prepare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteOperation {
    Created,
    Overwritten,
}

impl WriteOperation {
    const fn as_str(self) -> &'static str {
        match self {
            WriteOperation::Created => "created",
            WriteOperation::Overwritten => "overwritten",
        }
    }
}

/// Recheck fingerprint proving a prepared target is unchanged before install.
#[derive(Debug, Clone, PartialEq, Eq)]
enum WriteFingerprint {
    Existing {
        resolved_path: PathBuf,
        sha256: [u8; 32],
    },
    Absent {
        resolved_path: PathBuf,
        nearest_existing_ancestor: PathBuf,
    },
}

/// Runtime-only prepared Write payload. Never serialized or persisted.
#[derive(Debug, Clone)]
pub(crate) struct PreparedWrite {
    files: Vec<PreparedWriteFile>,
    created: usize,
    overwritten: usize,
    added: usize,
    removed: usize,
}

#[derive(Debug, Clone)]
struct PreparedWriteFile {
    requested_path: PathBuf,
    resolved_path: PathBuf,
    operation: WriteOperation,
    fingerprint: WriteFingerprint,
    content: String,
    line_count: usize,
    added: usize,
    removed: usize,
    /// Real unified diff for overwritten targets; empty for created targets,
    /// which render complete content instead.
    diff: String,
}

/// Outcome of installing one prepared Write file, including any directories
/// this installation actually created before an error stopped it.
struct WriteInstallOutcome {
    created_directories: Vec<PathBuf>,
    result: io::Result<AtomicWriteStatus>,
}

pub struct WriteTool;

impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "Write"
    }

    fn description(&self) -> &'static str {
        "Create files or completely overwrite existing UTF-8 files inside the workspace.\n\n\
         Write owns file creation and full-content replacement. Use Edit for targeted \
         replacements inside an existing file. Write never deletes, moves, or follows \
         symlinks / reparse points.\n\n\
         Parameters:\n\
         - files: Non-empty ordered array of file writes. Commit order matches declaration order.\n\
         - files[].path: Path to create or overwrite. Relative paths resolve against the working directory.\n\
         - files[].content: Complete UTF-8 content for the file. Empty content is valid for a new file.\n\n\
         Semantics:\n\
         - One call may mix new-file creation and existing-file overwrite.\n\
         - Existing targets must be ordinary UTF-8 regular files. Directories, symlinks, reparse \
         points, and non-UTF-8 files are rejected before anything is written.\n\
         - Overwriting a file with its current exact contents is a no-op and fails the whole call \
         with zero writes. Creating an empty new file is allowed.\n\
         - The whole call is prepared before any write. Any prepare failure writes nothing and \
         creates no directories.\n\
         - After approval, every target is rechecked; a stale or newly-appeared target fails with \
         zero writes.\n\
         - Missing parent directories are created only during commit.\n\
         - Files commit atomically one-by-one in declaration order. There is no cross-file \
         transaction and no automatic rollback.\n\
         - A partial commit is a failed tool result that reports committed, failed, and \
         not_attempted files plus any directories created.\n\n\
         Guidelines:\n\
         - Read a file before overwriting it so the new content is complete and intended.\n\
         - Provide the entire final content; Write does not merge with existing bytes.\n\
         - Group a coherent set of files into one call.\n\
         - Prefer Edit for surgical changes to a file you are mostly keeping.\n\
         - After any stale or partial failure, re-read affected files and submit a fresh Write \
         call. Never blindly replay the same Write arguments."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<WriteInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_write_allowed()?;
            let prepared = match PreparedWrite::prepare(ctx, &input).await {
                Ok(prepared) => prepared,
                Err(result) => return Ok(result),
            };
            if let Err(result) = prepared.recheck_all(ctx).await {
                return Ok(result);
            }
            let mut on_progress = |_update: ToolResult| {};
            Ok(prepared
                .commit(ctx, &ctx.cancel_token, &mut on_progress)
                .await)
        })
    }
}

impl PreparedWrite {
    /// Side-effect-free preparation of a complete Write batch.
    pub async fn prepare(
        context: &ToolContext,
        arguments: &serde_json::Value,
    ) -> Result<Arc<Self>, ToolResult> {
        let input: WriteInput = parse_write_input(arguments)?;
        validate_write_input(&input)?;

        let mut prepared_files = Vec::with_capacity(input.files.len());
        let mut seen_targets = HashSet::new();

        for (file_index, file) in input.files.iter().enumerate() {
            let candidate = absolute_candidate(context, &file.path);

            // Classify the model-supplied target without following a link.
            let operation = match std::fs::symlink_metadata(&candidate) {
                Ok(metadata) if is_reparse_or_symlink(&metadata) => {
                    return Err(prepare_failed(
                        Some(file_index),
                        Some(file.path.display().to_string()),
                        format!(
                            "refusing symlink or reparse point target: {}",
                            file.path.display()
                        ),
                        "Write only creates or overwrites ordinary regular files.",
                    ));
                }
                Ok(metadata) if metadata.is_file() => WriteOperation::Overwritten,
                Ok(_) => {
                    return Err(prepare_failed(
                        Some(file_index),
                        Some(file.path.display().to_string()),
                        format!(
                            "target exists and is not a regular file: {}",
                            file.path.display()
                        ),
                        "Write only creates or overwrites ordinary regular files.",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => WriteOperation::Created,
                Err(error) => {
                    return Err(prepare_failed(
                        Some(file_index),
                        Some(file.path.display().to_string()),
                        format!(
                            "failed to read metadata for {}: {error}",
                            file.path.display()
                        ),
                        "Re-read the path and submit a fresh Write call.",
                    ));
                }
            };

            let resolved = context
                .resolve_parent_for_write(&file.path)
                .map_err(|error| {
                    prepare_failed(
                        Some(file_index),
                        Some(file.path.display().to_string()),
                        format!("path resolution failed: {error}"),
                        "Re-read the path and submit a fresh Write call.",
                    )
                })?;
            // Existing targets resolve to a canonical path already; re-canonicalizing
            // is idempotent. Absent targets cannot be canonicalized, so keep the
            // resolved canonical-ancestor-plus-tail path from workspace resolution.
            let resolved = if operation == WriteOperation::Overwritten {
                resolved.canonicalize().map_err(|error| {
                    prepare_failed(
                        Some(file_index),
                        Some(file.path.display().to_string()),
                        format!("failed to resolve existing target: {error}"),
                        "Re-read the path and submit a fresh Write call.",
                    )
                })?
            } else {
                resolved
            };

            let identity = resolved.as_os_str().to_owned();
            if !seen_targets.insert(identity) {
                return Err(prepare_failed(
                    Some(file_index),
                    Some(file.path.display().to_string()),
                    format!("duplicate effective target path: {}", resolved.display()),
                    "Remove duplicate paths and submit a fresh Write call.",
                ));
            }

            let display_path = file.path.to_string_lossy();
            let (fingerprint, added, removed, diff) = match operation {
                WriteOperation::Overwritten => {
                    let bytes = match read_regular_file_no_follow(&resolved) {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            return Err(prepare_failed(
                                Some(file_index),
                                Some(file.path.display().to_string()),
                                format!("failed to read {}: {error}", resolved.display()),
                                "Write only overwrites existing UTF-8 regular files. Re-read the file and submit a fresh Write call.",
                            ));
                        }
                    };
                    let original = match String::from_utf8(bytes.clone()) {
                        Ok(text) => text,
                        Err(_) => {
                            return Err(prepare_failed(
                                Some(file_index),
                                Some(file.path.display().to_string()),
                                format!("file is not valid UTF-8: {}", resolved.display()),
                                "Write only overwrites existing UTF-8 regular files.",
                            ));
                        }
                    };
                    if original == file.content {
                        return Err(prepare_failed(
                            Some(file_index),
                            Some(file.path.display().to_string()),
                            format!(
                                "file already has the requested contents (no-op overwrite): {}",
                                file.path.display()
                            ),
                            "Remove unchanged files and submit a fresh Write call.",
                        ));
                    }
                    let diff = unified_diff(&display_path, &original, &file.content);
                    let (added, removed) = diff_stats(&diff);
                    let fingerprint = WriteFingerprint::Existing {
                        resolved_path: resolved.clone(),
                        sha256: sha256_bytes(&bytes),
                    };
                    (fingerprint, added, removed, diff)
                }
                WriteOperation::Created => {
                    let Some(ancestor) = nearest_existing_ancestor(&resolved) else {
                        return Err(prepare_failed(
                            Some(file_index),
                            Some(file.path.display().to_string()),
                            format!("no existing ancestor directory for {}", resolved.display()),
                            "Provide a path under an existing directory and submit a fresh Write call.",
                        ));
                    };
                    // Statistics use an empty original; created files render complete
                    // content instead of a diff, so the diff string stays empty.
                    let stats_diff = unified_diff(&display_path, "", &file.content);
                    let (added, removed) = diff_stats(&stats_diff);
                    let fingerprint = WriteFingerprint::Absent {
                        resolved_path: resolved.clone(),
                        nearest_existing_ancestor: ancestor,
                    };
                    (fingerprint, added, removed, String::new())
                }
            };

            prepared_files.push(PreparedWriteFile {
                requested_path: file.path.clone(),
                resolved_path: resolved,
                operation,
                fingerprint,
                line_count: file.content.lines().count(),
                content: file.content.clone(),
                added,
                removed,
                diff,
            });
        }

        let created = prepared_files
            .iter()
            .filter(|file| file.operation == WriteOperation::Created)
            .count();
        let overwritten = prepared_files.len() - created;
        let added = prepared_files.iter().map(|file| file.added).sum();
        let removed = prepared_files.iter().map(|file| file.removed).sum();

        Ok(Arc::new(Self {
            files: prepared_files,
            created,
            overwritten,
            added,
            removed,
        }))
    }

    #[must_use]
    pub fn approval_presentation(&self) -> WriteApprovalPresentation {
        WriteApprovalPresentation {
            files: self.files.len(),
            created: self.created,
            overwritten: self.overwritten,
            added: self.added,
            removed: self.removed,
            changes: self
                .files
                .iter()
                .map(|file| WriteApprovalChange {
                    path: file.requested_path.clone(),
                    line_count: file.line_count,
                    added: file.added,
                    removed: file.removed,
                    preview: match file.operation {
                        WriteOperation::Created => WriteApprovalPreview::Created {
                            content: file.content.clone(),
                        },
                        WriteOperation::Overwritten => WriteApprovalPreview::Overwritten {
                            diff: file.diff.clone(),
                        },
                    },
                })
                .collect(),
        }
    }

    /// Multi-key session scope for every workspace-contained prepared target.
    /// Returns `None` when any target cannot participate in a narrow scope.
    #[must_use]
    pub fn session_approval_scope(
        &self,
        workspace: &str,
        workspace_root: &Path,
    ) -> Option<SessionApprovalScope> {
        let mut keys = Vec::with_capacity(self.files.len());
        for file in &self.files {
            if !file.resolved_path.starts_with(workspace_root) {
                return None;
            }
            keys.push(SessionApprovalKey::FileWrite {
                workspace: workspace.to_owned(),
                path: file.resolved_path.display().to_string(),
                operation: FileWriteApprovalOperation::Write,
            });
        }
        if keys.is_empty() {
            return None;
        }
        let n = keys.len();
        Some(SessionApprovalScope {
            keys,
            label: format!("Approve writes to these {n} files for this session"),
            detail: format!("Writes to {n} workspace files"),
        })
    }

    #[must_use]
    pub(crate) fn all_resolved_targets_match(&self, target: &Path) -> bool {
        let Ok(target) = target.canonicalize() else {
            return false;
        };
        !self.files.is_empty() && self.files.iter().all(|file| file.resolved_path == target)
    }

    /// Structured `ToolExecutionUpdate` details for the verified planned projection.
    #[must_use]
    pub fn prepared_update(&self) -> ToolResult {
        ToolResult::ok(format!(
            "prepared Write · {} files · {} created · {} overwritten · +{} -{}",
            self.files.len(),
            self.created,
            self.overwritten,
            self.added,
            self.removed
        ))
        .with_details(json!({
            "kind": "write_prepared",
            "files": self.files.len(),
            "created": self.created,
            "overwritten": self.overwritten,
            "added": self.added,
            "removed": self.removed,
            "changes": self.files.iter().map(planned_change_json).collect::<Vec<_>>(),
        }))
    }

    #[must_use]
    pub(crate) fn cancelled_before_commit_result(&self) -> ToolResult {
        ToolResult::error(
            "Write cancelled before the first commit. Zero writes. Re-read targets before a fresh Write call.",
        )
        .with_details(json!({
            "kind": "write",
            "status": "cancelled",
            "cause": "cancelled",
            "files": self.files.len(),
            "created": self.created,
            "overwritten": self.overwritten,
            "added": 0,
            "removed": 0,
            "created_directories": Vec::<String>::new(),
            "changes": self.files.iter()
                .map(|file| header_change_json(file, "not_attempted"))
                .collect::<Vec<_>>(),
        }))
    }

    /// Whole-batch fingerprint recheck. Zero writes on mismatch.
    pub async fn recheck_all(&self, context: &ToolContext) -> Result<(), ToolResult> {
        for (file_index, file) in self.files.iter().enumerate() {
            self.recheck_file(context, file_index, file).await?;
        }
        Ok(())
    }

    async fn recheck_file(
        &self,
        context: &ToolContext,
        file_index: usize,
        file: &PreparedWriteFile,
    ) -> Result<(), ToolResult> {
        match &file.fingerprint {
            WriteFingerprint::Existing {
                resolved_path,
                sha256,
            } => {
                let resolved = context
                    .resolve_parent_for_write(&file.requested_path)
                    .map_err(|error| {
                        stale_result(
                            Some(file_index),
                            Some(file.requested_path.display().to_string()),
                            format!("path resolution changed after approval: {error}"),
                        )
                    })?
                    .canonicalize()
                    .map_err(|error| {
                        stale_result(
                            Some(file_index),
                            Some(file.requested_path.display().to_string()),
                            format!("path resolution changed after approval: {error}"),
                        )
                    })?;
                if resolved != *resolved_path {
                    return Err(stale_result(
                        Some(file_index),
                        Some(file.requested_path.display().to_string()),
                        format!(
                            "{} resolves to a different target after approval",
                            file.requested_path.display()
                        ),
                    ));
                }
                let current = match read_existing_fingerprint(&resolved) {
                    Ok(sha) => sha,
                    Err(message) => {
                        return Err(stale_result(
                            Some(file_index),
                            Some(file.requested_path.display().to_string()),
                            message,
                        ));
                    }
                };
                if current != *sha256 {
                    return Err(stale_result(
                        Some(file_index),
                        Some(file.requested_path.display().to_string()),
                        format!(
                            "{} changed after approval; planned content no longer matches the current workspace",
                            file.requested_path.display()
                        ),
                    ));
                }
                Ok(())
            }
            WriteFingerprint::Absent {
                resolved_path,
                nearest_existing_ancestor,
            } => {
                let candidate = absolute_candidate(context, &file.requested_path);
                match std::fs::symlink_metadata(&candidate) {
                    Ok(_) => {
                        return Err(stale_result(
                            Some(file_index),
                            Some(file.requested_path.display().to_string()),
                            format!(
                                "{} appeared after approval and will not be overwritten by a create",
                                file.requested_path.display()
                            ),
                        ));
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(stale_result(
                            Some(file_index),
                            Some(file.requested_path.display().to_string()),
                            format!(
                                "failed to recheck {}: {error}",
                                file.requested_path.display()
                            ),
                        ));
                    }
                }
                let resolved = context
                    .resolve_parent_for_write(&file.requested_path)
                    .map_err(|error| {
                        stale_result(
                            Some(file_index),
                            Some(file.requested_path.display().to_string()),
                            format!("path resolution changed after approval: {error}"),
                        )
                    })?;
                if resolved != *resolved_path {
                    return Err(stale_result(
                        Some(file_index),
                        Some(file.requested_path.display().to_string()),
                        format!(
                            "{} resolves to a different target after approval",
                            file.requested_path.display()
                        ),
                    ));
                }
                if validate_safe_directory(nearest_existing_ancestor).is_err() {
                    return Err(stale_result(
                        Some(file_index),
                        Some(file.requested_path.display().to_string()),
                        format!(
                            "ancestor directory {} is no longer a safe directory",
                            nearest_existing_ancestor.display()
                        ),
                    ));
                }
                Ok(())
            }
        }
    }

    /// Commit prepared files in declaration order with per-file atomic installs.
    pub async fn commit(
        &self,
        context: &ToolContext,
        cancel_token: &CancellationToken,
        on_progress: &mut (dyn FnMut(ToolResult) + Send),
    ) -> ToolResult {
        self.commit_with_installer(context, cancel_token, on_progress, |_, file| {
            default_install(file)
        })
        .await
    }

    async fn commit_with_installer(
        &self,
        context: &ToolContext,
        cancel_token: &CancellationToken,
        on_progress: &mut (dyn FnMut(ToolResult) + Send),
        mut install: impl FnMut(usize, &PreparedWriteFile) -> WriteInstallOutcome,
    ) -> ToolResult {
        let total = self.files.len();
        let mut changes = Vec::with_capacity(total);
        let mut committed_count = 0usize;
        let mut cumulative_added = 0usize;
        let mut cumulative_removed = 0usize;
        let mut created_dirs: Vec<PathBuf> = Vec::new();

        for (file_index, file) in self.files.iter().enumerate() {
            if cancel_token.is_cancelled() {
                if committed_count == 0 {
                    return self.cancelled_before_commit_result();
                }
                for remaining in self.files.iter().skip(file_index) {
                    changes.push(header_change_json(remaining, "not_attempted"));
                }
                return ToolResult::error(format!(
                    "Write cancelled after committing {committed_count}/{total} files. Committed content remains. Re-read remaining files before a fresh Write call."
                )).with_details(json!({
                    "kind": "write",
                    "status": "partial_commit",
                    "cause": "cancelled",
                    "files": total,
                    "created": self.created,
                    "overwritten": self.overwritten,
                    "added": cumulative_added,
                    "removed": cumulative_removed,
                    "created_directories": created_directories_json(context, &created_dirs),
                    "changes": changes,
                }));
            }

            if let Err(result) = self.recheck_file(context, file_index, file).await {
                if committed_count == 0 {
                    return result;
                }
                let details = result.details.clone().unwrap_or_else(|| json!({}));
                changes.push(header_change_json(file, "failed"));
                for remaining in self.files.iter().skip(file_index + 1) {
                    changes.push(header_change_json(remaining, "not_attempted"));
                }
                let path = file.requested_path.display().to_string();
                return ToolResult::error(format!(
                    "Write partial commit: {committed_count}/{total} files committed; \
                     {path} became stale before write. Already committed content remains. \
                     Re-read remaining files before a fresh Write call."
                ))
                .with_details(json!({
                    "kind": "write",
                    "status": "partial_commit",
                    "files": total,
                    "created": self.created,
                    "overwritten": self.overwritten,
                    "added": cumulative_added,
                    "removed": cumulative_removed,
                    "failed_path": path,
                    "stale": details,
                    "created_directories": created_directories_json(context, &created_dirs),
                    "changes": changes,
                }));
            }

            let outcome = install(file_index, file);
            for dir in outcome.created_directories {
                if !created_dirs.contains(&dir) {
                    created_dirs.push(dir);
                }
            }
            match outcome.result {
                Ok(AtomicWriteStatus::Durable) => {
                    committed_count += 1;
                    cumulative_added += file.added;
                    cumulative_removed += file.removed;
                    changes.push(installed_change_json(file, "committed"));
                    on_progress(
                        ToolResult::ok(format!(
                            "committed {committed_count}/{total}: {}",
                            file.requested_path.display()
                        ))
                        .with_details(json!({
                            "kind": "write_progress",
                            "committed": committed_count,
                            "total": total,
                            "latest_path": file.requested_path.display().to_string(),
                            "latest_operation": file.operation.as_str(),
                            "added": cumulative_added,
                            "removed": cumulative_removed,
                        })),
                    );
                }
                Ok(AtomicWriteStatus::CommittedUnsynced(error)) => {
                    committed_count += 1;
                    cumulative_added += file.added;
                    cumulative_removed += file.removed;
                    changes.push(installed_change_json(file, "committed_unsynced"));
                    for remaining in self.files.iter().skip(file_index + 1) {
                        changes.push(header_change_json(remaining, "not_attempted"));
                    }
                    return ToolResult::error(format!(
                        "Write durability uncertain after committing {committed_count}/{total} files. \
                         Contents of {} were installed but parent-directory durability is uncertain ({error}). \
                         Re-read files before a fresh Write call; do not blindly replay.",
                        file.requested_path.display()
                    ))
                    .with_details(json!({
                        "kind": "write",
                        "status": "durability_uncertain",
                        "files": total,
                        "created": self.created,
                        "overwritten": self.overwritten,
                        "added": cumulative_added,
                        "removed": cumulative_removed,
                        "created_directories": created_directories_json(context, &created_dirs),
                        "changes": changes,
                    }));
                }
                Err(error) => {
                    changes.push(header_change_json(file, "failed"));
                    for remaining in self.files.iter().skip(file_index + 1) {
                        changes.push(header_change_json(remaining, "not_attempted"));
                    }
                    let zero = committed_count == 0;
                    let status = if zero {
                        "commit_failed"
                    } else {
                        "partial_commit"
                    };
                    let message = if zero {
                        format!(
                            "Write failed before any durable write at {}: {error}. Zero files installed. \
                             Re-read the file and submit a fresh Write call.",
                            file.requested_path.display()
                        )
                    } else {
                        format!(
                            "Write partial commit: {committed_count}/{total} files committed; \
                             {} failed ({error}). Already committed content remains and was not rolled back. \
                             Re-read remaining files before a fresh Write call.",
                            file.requested_path.display()
                        )
                    };
                    return ToolResult::error(message).with_details(json!({
                        "kind": "write",
                        "status": status,
                        "files": total,
                        "created": self.created,
                        "overwritten": self.overwritten,
                        "added": cumulative_added,
                        "removed": cumulative_removed,
                        "created_directories": created_directories_json(context, &created_dirs),
                        "changes": changes,
                    }));
                }
            }
        }

        ToolResult::ok(format!(
            "wrote {} files · {} created · {} overwritten · +{} -{}",
            total, self.created, self.overwritten, self.added, self.removed
        ))
        .with_details(json!({
            "kind": "write",
            "status": "committed",
            "files": total,
            "created": self.created,
            "overwritten": self.overwritten,
            "added": self.added,
            "removed": self.removed,
            "created_directories": created_directories_json(context, &created_dirs),
            "changes": changes,
        }))
    }
}

/// Default per-file installer. Created files create missing parents (recording
/// each directory) then use the create-new atomic helper; overwritten files use
/// the strict existing-file replacement helper.
fn default_install(file: &PreparedWriteFile) -> WriteInstallOutcome {
    match file.operation {
        WriteOperation::Created => {
            let Some(parent) = file.resolved_path.parent() else {
                return WriteInstallOutcome {
                    created_directories: Vec::new(),
                    result: Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!(
                            "path has no parent directory: {}",
                            file.resolved_path.display()
                        ),
                    )),
                };
            };
            let directories = create_missing_directories_recording(parent);
            if let Some(error) = directories.error {
                return WriteInstallOutcome {
                    created_directories: directories.created,
                    result: Err(error),
                };
            }
            let result = write_file_atomic_create_new(&file.resolved_path, file.content.as_bytes());
            WriteInstallOutcome {
                created_directories: directories.created,
                result,
            }
        }
        WriteOperation::Overwritten => WriteInstallOutcome {
            created_directories: Vec::new(),
            result: replace_existing_file_atomic_status(
                &file.resolved_path,
                file.content.as_bytes(),
            ),
        },
    }
}

fn parse_write_input(arguments: &serde_json::Value) -> Result<WriteInput, ToolResult> {
    match serde_json::from_value::<WriteInput>(arguments.clone()) {
        Ok(input) => Ok(input),
        Err(error) => Err(prepare_failed(
            None,
            None,
            format!("invalid Write arguments: {error}"),
            "Submit a fresh Write call using the files[] contract.",
        )),
    }
}

fn validate_write_input(input: &WriteInput) -> Result<(), ToolResult> {
    if input.files.is_empty() {
        return Err(prepare_failed(
            None,
            None,
            "files must be a non-empty array".to_owned(),
            "Group at least one file into files[] and submit a fresh Write call.",
        ));
    }
    for (file_index, file) in input.files.iter().enumerate() {
        if file.path.as_os_str().is_empty() {
            return Err(prepare_failed(
                Some(file_index),
                None,
                "path must be non-empty".to_owned(),
                "Provide a non-empty path and submit a fresh Write call.",
            ));
        }
    }
    Ok(())
}

fn prepare_failed(
    file_index: Option<usize>,
    path: Option<String>,
    message: String,
    guidance: &str,
) -> ToolResult {
    let mut content = String::from("Write prepare failed · zero writes");
    if let Some(path) = path.as_ref() {
        content.push_str(" · ");
        content.push_str(path);
    }
    content.push('\n');
    content.push_str(&message);
    content.push('\n');
    content.push_str(guidance);

    let mut details = json!({
        "kind": "write",
        "status": "prepare_failed",
        "message": message,
    });
    if let Some(file_index) = file_index {
        details["file_index"] = json!(file_index);
    }
    if let Some(path) = path {
        details["path"] = json!(path);
    }
    ToolResult::error(content).with_details(details)
}

fn stale_result(file_index: Option<usize>, path: Option<String>, message: String) -> ToolResult {
    let mut content = String::from("Write failed · stale · zero writes\n");
    if let Some(path) = path.as_ref() {
        content.push_str(path);
        content.push('\n');
    }
    content.push_str(&message);
    content.push_str("\nRe-read affected files and submit a new Write call.");
    let mut details = json!({
        "kind": "write",
        "status": "stale",
        "message": message,
    });
    if let Some(file_index) = file_index {
        details["file_index"] = json!(file_index);
    }
    if let Some(path) = path {
        details["path"] = json!(path);
    }
    ToolResult::error(content).with_details(details)
}

/// Header-only change entry for planned, failed, and not-attempted files.
fn header_change_json(file: &PreparedWriteFile, status: &str) -> serde_json::Value {
    json!({
        "path": file.requested_path.display().to_string(),
        "operation": file.operation.as_str(),
        "status": status,
    })
}

/// Full change entry for installed files, including content or diff body.
fn installed_change_json(file: &PreparedWriteFile, status: &str) -> serde_json::Value {
    let mut value = json!({
        "path": file.requested_path.display().to_string(),
        "operation": file.operation.as_str(),
        "status": status,
        "line_count": file.line_count,
        "added": file.added,
        "removed": file.removed,
    });
    match file.operation {
        WriteOperation::Created => value["content"] = json!(file.content),
        WriteOperation::Overwritten => value["diff"] = json!(file.diff),
    }
    value
}

/// Planned change entry (approval / prepared projection) with the full body for
/// every file regardless of eventual status.
fn planned_change_json(file: &PreparedWriteFile) -> serde_json::Value {
    let mut value = json!({
        "path": file.requested_path.display().to_string(),
        "operation": file.operation.as_str(),
        "line_count": file.line_count,
        "added": file.added,
        "removed": file.removed,
    });
    match file.operation {
        WriteOperation::Created => value["content"] = json!(file.content),
        WriteOperation::Overwritten => value["diff"] = json!(file.diff),
    }
    value
}

fn created_directories_json(context: &ToolContext, dirs: &[PathBuf]) -> Vec<String> {
    let root = context
        .workspace_root()
        .canonicalize()
        .unwrap_or_else(|_| context.workspace_root().to_path_buf());
    dirs.iter()
        .map(|dir| match dir.strip_prefix(&root) {
            Ok(relative) if !relative.as_os_str().is_empty() => relative.display().to_string(),
            _ => dir.display().to_string(),
        })
        .collect()
}

fn absolute_candidate(context: &ToolContext, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        context.workspace_root().join(path)
    }
}

/// Nearest existing ancestor directory of a (possibly absent) target path.
fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path.parent();
    while let Some(dir) = current {
        match std::fs::symlink_metadata(dir) {
            Ok(_) => return Some(dir.to_path_buf()),
            Err(_) => current = dir.parent(),
        }
    }
    None
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn read_existing_fingerprint(path: &Path) -> Result<[u8; 32], String> {
    let bytes = read_regular_file_no_follow(path)
        .map_err(|error| format!("failed to recheck {}: {error}", path.display()))?;
    if String::from_utf8(bytes.clone()).is_err() {
        return Err(format!(
            "target is no longer valid UTF-8: {}",
            path.display()
        ));
    }
    Ok(sha256_bytes(&bytes))
}

fn read_regular_file_no_follow(path: &Path) -> io::Result<Vec<u8>> {
    use std::io::Read as _;

    let mut file = open_no_follow(path)?;
    let metadata = file.metadata()?;
    if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("target is not an existing regular file: {}", path.display()),
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};

    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    Ok(fd.into())
}

#[cfg(windows)]
fn open_no_follow(path: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use winapi::um::winbase::FILE_FLAG_OPEN_REPARSE_POINT;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_no_follow(path: &Path) -> io::Result<std::fs::File> {
    std::fs::File::open(path)
}

fn is_reparse_or_symlink(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink() || platform_reparse_point(metadata)
}

#[cfg(windows)]
fn platform_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn platform_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ToolAccess, ToolContext, WorkspaceAccessPolicy, WorkspaceAccessRoot,
        WorkspaceAccessRootKind,
    };
    use serde_json::json;

    fn context(root: &Path) -> ToolContext {
        ToolContext::new(root)
            .expect("context")
            .with_access(ToolAccess::all())
    }

    #[tokio::test]
    async fn write_denies_read_only_added_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [WorkspaceAccessRoot {
                path: added.path().canonicalize().expect("canonical added"),
                kind: WorkspaceAccessRootKind::Added,
                read: true,
                write: false,
            }],
        )
        .expect("policy");
        let ctx = ToolContext::new(primary.path())
            .expect("context")
            .with_workspace_policy(policy)
            .with_access(ToolAccess::all());
        let path = added.path().join("new.txt");

        let result = WriteTool
            .execute(
                &ctx,
                json!({ "files": [{ "path": path, "content": "hello" }] }),
            )
            .await
            .expect("tool result");

        assert!(result.is_error);
        assert_eq!(result.details.expect("details")["status"], "prepare_failed");
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn write_batch_prepare_rejections_leave_files_and_directories_untouched() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ctx = context(workspace.path());

        // Duplicate effective target.
        std::fs::write(workspace.path().join("dup.txt"), "same\n").expect("seed dup");
        let duplicate = PreparedWrite::prepare(
            &ctx,
            &json!({
                "files": [
                    { "path": "dup.txt", "content": "first\n" },
                    { "path": "./dup.txt", "content": "second\n" }
                ]
            }),
        )
        .await
        .expect_err("duplicate rejected");
        assert_eq!(
            duplicate.details.expect("dup details")["status"],
            "prepare_failed"
        );

        // Existing directory is not a regular file.
        std::fs::create_dir(workspace.path().join("adir")).expect("seed dir");
        let directory = PreparedWrite::prepare(
            &ctx,
            &json!({ "files": [{ "path": "adir", "content": "x" }] }),
        )
        .await
        .expect_err("directory rejected");
        assert_eq!(
            directory.details.expect("dir details")["status"],
            "prepare_failed"
        );
        assert!(workspace.path().join("adir").is_dir());

        // Non-UTF-8 existing file cannot be overwritten.
        std::fs::write(workspace.path().join("binary.bin"), [0xff, 0xfe]).expect("seed binary");
        let non_utf8 = PreparedWrite::prepare(
            &ctx,
            &json!({ "files": [{ "path": "binary.bin", "content": "text\n" }] }),
        )
        .await
        .expect_err("non-utf8 rejected");
        assert_eq!(
            non_utf8.details.expect("binary details")["status"],
            "prepare_failed"
        );
        assert_eq!(
            std::fs::read(workspace.path().join("binary.bin")).expect("binary bytes"),
            [0xff, 0xfe]
        );

        // No-op overwrite of an existing file.
        std::fs::write(workspace.path().join("noop.txt"), "same\n").expect("seed noop");
        let noop = PreparedWrite::prepare(
            &ctx,
            &json!({ "files": [{ "path": "noop.txt", "content": "same\n" }] }),
        )
        .await
        .expect_err("no-op rejected");
        assert_eq!(
            noop.details.expect("noop details")["status"],
            "prepare_failed"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("noop.txt")).expect("noop bytes"),
            "same\n"
        );

        // Prepare created nothing new.
        assert!(!workspace.path().join("first.txt").exists());
        assert!(!workspace.path().join("second.txt").exists());
    }

    #[tokio::test]
    async fn write_batch_failure_after_first_commit_reports_partial_without_rollback() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ctx = context(workspace.path());
        let prepared = PreparedWrite::prepare(
            &ctx,
            &json!({
                "files": [
                    { "path": "a.txt", "content": "AAA\n" },
                    { "path": "b.txt", "content": "BBB\n" },
                    { "path": "c.txt", "content": "CCC\n" }
                ]
            }),
        )
        .await
        .expect("prepare");
        let mut on_progress = |_update| {};

        // Failure at the very first file installs nothing.
        let zero = prepared
            .commit_with_installer(&ctx, &CancellationToken::new(), &mut on_progress, |_, _| {
                WriteInstallOutcome {
                    created_directories: Vec::new(),
                    result: Err(io::Error::other("first install failure")),
                }
            })
            .await;
        assert!(zero.is_error);
        let details = zero.details.expect("zero details");
        assert_eq!(details["status"], "commit_failed");
        assert_eq!(details["changes"][0]["status"], "failed");
        assert_eq!(details["changes"][1]["status"], "not_attempted");
        assert_eq!(details["changes"][2]["status"], "not_attempted");
        assert!(!workspace.path().join("a.txt").exists());

        // Failure at the second file keeps the first committed, no rollback.
        let partial = prepared
            .commit_with_installer(
                &ctx,
                &CancellationToken::new(),
                &mut on_progress,
                |index, file| {
                    if index == 1 {
                        WriteInstallOutcome {
                            created_directories: Vec::new(),
                            result: Err(io::Error::other("injected install failure")),
                        }
                    } else {
                        default_install(file)
                    }
                },
            )
            .await;
        assert!(partial.is_error);
        let details = partial.details.expect("partial details");
        assert_eq!(details["status"], "partial_commit");
        assert_eq!(details["changes"][0]["status"], "committed");
        assert_eq!(details["changes"][1]["status"], "failed");
        assert_eq!(details["changes"][2]["status"], "not_attempted");
        assert_eq!(details["added"], 1);
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("a.txt")).expect("a"),
            "AAA\n"
        );
        assert!(!workspace.path().join("b.txt").exists());
        assert!(!workspace.path().join("c.txt").exists());
    }

    #[tokio::test]
    async fn write_batch_reports_directories_created_before_install_failure() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ctx = context(workspace.path());
        let prepared = PreparedWrite::prepare(
            &ctx,
            &json!({ "files": [{ "path": "deep/nested/dir/file.txt", "content": "x\n" }] }),
        )
        .await
        .expect("prepare");
        let mut on_progress = |_update| {};

        let result = prepared
            .commit_with_installer(
                &ctx,
                &CancellationToken::new(),
                &mut on_progress,
                |_, file| {
                    let parent = file.resolved_path.parent().expect("parent");
                    let directories = create_missing_directories_recording(parent);
                    WriteInstallOutcome {
                        created_directories: directories.created,
                        result: Err(io::Error::other("disk full after directories")),
                    }
                },
            )
            .await;

        assert!(result.is_error);
        let details = result.details.expect("details");
        assert_eq!(details["status"], "commit_failed");
        let created: Vec<String> = details["created_directories"]
            .as_array()
            .expect("created_directories array")
            .iter()
            .map(|value| value.as_str().expect("dir string").to_owned())
            .collect();
        assert!(created.iter().any(|dir| dir.contains("deep")));
        // Directories remain; no unsafe cleanup, and the file was not installed.
        assert!(workspace.path().join("deep/nested/dir").is_dir());
        assert!(!workspace.path().join("deep/nested/dir/file.txt").exists());
    }

    #[tokio::test]
    async fn write_batch_cancellation_before_and_after_first_commit_is_truthful() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ctx = context(workspace.path());
        let prepared = PreparedWrite::prepare(
            &ctx,
            &json!({
                "files": [
                    { "path": "one.txt", "content": "one\n" },
                    { "path": "two.txt", "content": "two\n" }
                ]
            }),
        )
        .await
        .expect("prepare");
        let mut on_progress = |_update| {};

        let cancel = CancellationToken::new();
        cancel.cancel();
        let before = prepared
            .commit_with_installer(&ctx, &cancel, &mut on_progress, |_, _| {
                panic!("installer must not run after cancellation")
            })
            .await;
        assert!(before.is_error);
        let details = before.details.expect("before details");
        assert_eq!(details["status"], "cancelled");
        assert_eq!(details["cause"], "cancelled");
        assert_eq!(details["changes"][0]["status"], "not_attempted");
        assert_eq!(details["changes"][1]["status"], "not_attempted");
        assert!(!workspace.path().join("one.txt").exists());
        assert!(!workspace.path().join("two.txt").exists());

        let partial_cancel = CancellationToken::new();
        let cancel_after_write = partial_cancel.clone();
        let after = prepared
            .commit_with_installer(&ctx, &partial_cancel, &mut on_progress, move |_, file| {
                let outcome = default_install(file);
                cancel_after_write.cancel();
                outcome
            })
            .await;
        assert!(after.is_error);
        let details = after.details.expect("after details");
        assert_eq!(details["status"], "partial_commit");
        assert_eq!(details["cause"], "cancelled");
        assert_eq!(details["changes"][0]["status"], "committed");
        assert_eq!(details["changes"][1]["status"], "not_attempted");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("one.txt")).expect("one"),
            "one\n"
        );
        assert!(!workspace.path().join("two.txt").exists());
    }

    #[tokio::test]
    async fn write_schema_rejects_legacy_and_unknown_fields() {
        let workspace = tempfile::tempdir().expect("workspace");
        let ctx = context(workspace.path());

        for arguments in [
            json!({ "path": "legacy.txt", "content": "legacy" }),
            json!({ "files": [{ "path": "x.txt", "content": "x" }], "extra": true }),
            json!({ "files": [{ "path": "x.txt", "content": "x", "mode": 1 }] }),
            json!({ "files": [{ "path": "x.txt" }] }),
        ] {
            let rejected = PreparedWrite::prepare(&ctx, &arguments)
                .await
                .expect_err("schema rejected");
            assert_eq!(
                rejected.details.expect("schema details")["status"],
                "prepare_failed"
            );
        }
        assert!(!workspace.path().join("legacy.txt").exists());
        assert!(!workspace.path().join("x.txt").exists());
    }
}
