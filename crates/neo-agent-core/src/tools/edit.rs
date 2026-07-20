use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::diff::unified_diff;
use super::{Tool, ToolContext, ToolFuture, ToolResult, schema};
use crate::approval::{EditApprovalChange, EditApprovalPresentation};
use crate::permissions::{FileWriteApprovalOperation, SessionApprovalKey, SessionApprovalScope};
use crate::session::atomic_file::{AtomicWriteStatus, write_file_atomic_status};

const fn default_expected_matches() -> usize {
    1
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditInput {
    #[schemars(
        description = "Ordered list of existing UTF-8 files to edit. Declaration order is the commit order."
    )]
    files: Vec<EditFileInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditFileInput {
    #[schemars(
        description = "Path to an existing file to edit. Relative paths resolve against the working directory."
    )]
    path: PathBuf,
    #[schemars(
        description = "Ordered exact replacements applied to staged content for this file. Declaration order is meaningful."
    )]
    replacements: Vec<EditReplacementInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditReplacementInput {
    #[schemars(
        description = "Exact existing UTF-8 text to replace. Matching is character-for-character with no normalization."
    )]
    old: String,
    #[schemars(
        description = "Replacement text. Empty string removes the matched text."
    )]
    new: String,
    #[serde(default = "default_expected_matches")]
    #[schemars(
        description = "Exact number of non-overlapping matches that must be present in the current staged content. Defaults to 1."
    )]
    expected_matches: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditFileKind {
    Regular,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditFingerprint {
    resolved_path: PathBuf,
    file_kind: EditFileKind,
    sha256: [u8; 32],
}

/// Runtime-only prepared Edit payload. Never serialized or persisted.
#[derive(Debug, Clone)]
pub struct PreparedEdit {
    files: Vec<PreparedEditFile>,
    replacements: usize,
    added: usize,
    removed: usize,
    /// Test-only seam: fail the commit writer before replacement at this file index.
    fail_commit_at: Option<usize>,
}

#[derive(Debug, Clone)]
struct PreparedEditFile {
    requested_path: PathBuf,
    resolved_path: PathBuf,
    fingerprint: EditFingerprint,
    #[allow(dead_code)] // retained for recheck equality diagnostics and future audit
    original: String,
    staged: String,
    replacements: usize,
    added: usize,
    removed: usize,
    diff: String,
}

pub struct EditTool;

impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "Edit"
    }

    fn description(&self) -> &'static str {
        "Apply ordered exact replacements to existing UTF-8 regular files inside the workspace.\n\n\
         Edit is the only exact-replacement tool. Use Write to create files or fully overwrite them. \
         Edit never creates, deletes, moves, or follows symlinks / reparse points.\n\n\
         Parameters:\n\
         - files: Non-empty ordered array of file edits. Commit order matches declaration order.\n\
         - files[].path: Existing file path. Relative paths resolve against the working directory.\n\
         - files[].replacements: Non-empty ordered array of exact replacements for that file.\n\
         - replacements[].old: Non-empty exact UTF-8 substring to match. No whitespace, newline, or \
         Unicode normalization is applied.\n\
         - replacements[].new: Replacement text (may be empty to delete the match).\n\
         - replacements[].expected_matches: Exact non-overlapping match count required in the current \
         staged content for that file. Defaults to 1. Actual count must equal this value.\n\n\
         Semantics:\n\
         - Replacements inside one file run in declaration order against staged content.\n\
         - The whole call is prepared before any write. Any prepare failure writes nothing.\n\
         - After approval, every target is rechecked; stale content fails with zero writes.\n\
         - Files commit atomically one-by-one in declaration order. There is no cross-file transaction \
         and no automatic rollback.\n\
         - Partial commit is a failed tool result that reports committed, failed, and not_attempted files.\n\n\
         Guidelines:\n\
         - Always Read target files first and supply the observed exact match counts.\n\
         - Group all replacements for one file into that file's replacements array.\n\
         - Prefer Write for new files or full-file rewrites.\n\
         - After any failure, re-read affected files and issue a fresh Edit call. Never blindly replay \
         the same Edit arguments."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<EditInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_write_allowed()?;
            let prepared = match PreparedEdit::prepare(ctx, &input).await {
                Ok(prepared) => prepared,
                Err(result) => return Ok(result),
            };
            if let Err(result) = prepared.recheck_all().await {
                return Ok(result);
            }
            let mut on_progress = |_update: ToolResult| {};
            Ok(prepared.commit(&ctx.cancel_token, &mut on_progress).await)
        })
    }
}

impl PreparedEdit {
    /// Side-effect-free preparation of a complete Edit batch.
    pub async fn prepare(
        context: &ToolContext,
        arguments: &serde_json::Value,
    ) -> Result<Arc<Self>, ToolResult> {
        let input: EditInput = parse_edit_input(arguments)?;
        validate_edit_input(&input)?;

        let mut prepared_files = Vec::with_capacity(input.files.len());
        let mut seen_targets = HashSet::new();
        let mut total_replacements = 0usize;

        for (file_index, file) in input.files.iter().enumerate() {
            // Check the model-supplied path before workspace resolution follows links.
            let absolute_candidate = if file.path.is_absolute() {
                file.path.clone()
            } else {
                context.workspace_root().join(&file.path)
            };
            match std::fs::symlink_metadata(&absolute_candidate) {
                Ok(metadata) if is_reparse_or_symlink(&metadata) => {
                    return Err(prepare_failed(
                        Some(file_index),
                        None,
                        Some(file.path.display().to_string()),
                        format!(
                            "refusing symlink or reparse point target: {}",
                            file.path.display()
                        ),
                        "Edit only supports existing UTF-8 regular files.",
                    ));
                }
                Ok(metadata) if !metadata.is_file() => {
                    return Err(prepare_failed(
                        Some(file_index),
                        None,
                        Some(file.path.display().to_string()),
                        format!("target is not a regular file: {}", file.path.display()),
                        "Edit only supports existing UTF-8 regular files.",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(prepare_failed(
                        Some(file_index),
                        None,
                        Some(file.path.display().to_string()),
                        format!("file does not exist: {}", file.path.display()),
                        "Use Write to create new files, or re-read existing targets first.",
                    ));
                }
                Err(error) => {
                    return Err(prepare_failed(
                        Some(file_index),
                        None,
                        Some(file.path.display().to_string()),
                        format!(
                            "failed to read metadata for {}: {error}",
                            file.path.display()
                        ),
                        "Re-read the file and submit a fresh Edit call.",
                    ));
                }
            }

            let resolved = context
                .resolve_parent_for_write(&file.path)
                .map_err(|error| {
                    prepare_failed(
                        Some(file_index),
                        None,
                        Some(file.path.display().to_string()),
                        format!("path resolution failed: {error}"),
                        "Re-read the path and submit a fresh Edit call.",
                    )
                })?;

            let identity = resolved.as_os_str().to_owned();
            if !seen_targets.insert(identity) {
                return Err(prepare_failed(
                    Some(file_index),
                    None,
                    Some(file.path.display().to_string()),
                    format!(
                        "duplicate effective target path: {}",
                        resolved.display()
                    ),
                    "Remove duplicate paths and submit a fresh Edit call.",
                ));
            }

            // Re-check resolved path (non-followed candidate already checked above).
            let metadata = match std::fs::symlink_metadata(&resolved) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(prepare_failed(
                        Some(file_index),
                        None,
                        Some(file.path.display().to_string()),
                        format!("file does not exist: {}", resolved.display()),
                        "Use Write to create new files, or re-read existing targets first.",
                    ));
                }
                Err(error) => {
                    return Err(prepare_failed(
                        Some(file_index),
                        None,
                        Some(file.path.display().to_string()),
                        format!("failed to read metadata for {}: {error}", resolved.display()),
                        "Re-read the file and submit a fresh Edit call.",
                    ));
                }
            };

            if is_reparse_or_symlink(&metadata) {
                return Err(prepare_failed(
                    Some(file_index),
                    None,
                    Some(file.path.display().to_string()),
                    format!(
                        "refusing symlink or reparse point target: {}",
                        resolved.display()
                    ),
                    "Edit only supports existing UTF-8 regular files.",
                ));
            }
            if !metadata.is_file() {
                return Err(prepare_failed(
                    Some(file_index),
                    None,
                    Some(file.path.display().to_string()),
                    format!("target is not a regular file: {}", resolved.display()),
                    "Edit only supports existing UTF-8 regular files.",
                ));
            }

            let bytes = match tokio::fs::read(&resolved).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    return Err(prepare_failed(
                        Some(file_index),
                        None,
                        Some(file.path.display().to_string()),
                        format!("failed to read {}: {error}", resolved.display()),
                        "Re-read the file and submit a fresh Edit call.",
                    ));
                }
            };
            let original = match String::from_utf8(bytes.clone()) {
                Ok(text) => text,
                Err(_) => {
                    return Err(prepare_failed(
                        Some(file_index),
                        None,
                        Some(file.path.display().to_string()),
                        format!("file is not valid UTF-8: {}", resolved.display()),
                        "Edit only supports existing UTF-8 regular files.",
                    ));
                }
            };

            let mut staged = original.clone();
            for (replacement_index, replacement) in file.replacements.iter().enumerate() {
                let actual = count_non_overlapping(&staged, &replacement.old);
                if actual != replacement.expected_matches {
                    let lines = match_line_numbers(&staged, &replacement.old);
                    let line_list = if lines.is_empty() {
                        "none".to_owned()
                    } else {
                        lines
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    return Err(prepare_failed(
                        Some(file_index),
                        Some(replacement_index),
                        Some(file.path.display().to_string()),
                        format!(
                            "expected {} exact matches · found {actual}; matches at lines {line_list}",
                            replacement.expected_matches
                        ),
                        "Re-read the file and submit a new Edit call.",
                    ));
                }
                staged = replace_non_overlapping(&staged, &replacement.old, &replacement.new);
            }

            if staged == original {
                return Err(prepare_failed(
                    Some(file_index),
                    None,
                    Some(file.path.display().to_string()),
                    format!(
                        "file is unchanged after replacements: {}",
                        file.path.display()
                    ),
                    "Remove no-op replacements and submit a fresh Edit call.",
                ));
            }

            let display_path = file.path.to_string_lossy();
            let diff = unified_diff(&display_path, &original, &staged);
            let (added, removed) = diff_stats(&diff);
            let fingerprint = EditFingerprint {
                resolved_path: resolved.clone(),
                file_kind: EditFileKind::Regular,
                sha256: sha256_bytes(&bytes),
            };

            total_replacements += file.replacements.len();
            prepared_files.push(PreparedEditFile {
                requested_path: file.path.clone(),
                resolved_path: resolved,
                fingerprint,
                original,
                staged,
                replacements: file.replacements.len(),
                added,
                removed,
                diff,
            });
        }

        let added = prepared_files.iter().map(|file| file.added).sum();
        let removed = prepared_files.iter().map(|file| file.removed).sum();
        if prepared_files.is_empty() || (added == 0 && removed == 0) {
            return Err(prepare_failed(
                None,
                None,
                None,
                "batch is a no-op".to_owned(),
                "Supply at least one effective replacement and submit a fresh Edit call.",
            ));
        }

        Ok(Arc::new(Self {
            files: prepared_files,
            replacements: total_replacements,
            added,
            removed,
            fail_commit_at: None,
        }))
    }

    #[must_use]
    pub fn approval_presentation(&self) -> EditApprovalPresentation {
        EditApprovalPresentation {
            files: self.files.len(),
            replacements: self.replacements,
            added: self.added,
            removed: self.removed,
            changes: self
                .files
                .iter()
                .map(|file| EditApprovalChange {
                    path: file.requested_path.clone(),
                    replacements: file.replacements,
                    added: file.added,
                    removed: file.removed,
                    diff: file.diff.clone(),
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
                operation: FileWriteApprovalOperation::Edit,
            });
        }
        if keys.is_empty() {
            return None;
        }
        let n = keys.len();
        Some(SessionApprovalScope {
            keys,
            label: format!("Approve edits to these {n} files for this session"),
            detail: format!("Edits to {n} workspace files"),
        })
    }

    /// Structured `ToolExecutionUpdate` details for the verified planned projection.
    #[must_use]
    pub fn prepared_update(&self) -> ToolResult {
        ToolResult::ok(format!(
            "prepared Edit · {} files · {} replacements · +{} -{}",
            self.files.len(),
            self.replacements,
            self.added,
            self.removed
        ))
        .with_details(json!({
            "kind": "edit_prepared",
            "files": self.files.len(),
            "replacements": self.replacements,
            "added": self.added,
            "removed": self.removed,
            "changes": self.files.iter().map(|file| json!({
                "path": file.requested_path.display().to_string(),
                "replacements": file.replacements,
                "added": file.added,
                "removed": file.removed,
                "diff": file.diff,
            })).collect::<Vec<_>>(),
        }))
    }

    /// Whole-batch fingerprint recheck. Zero writes on mismatch.
    pub async fn recheck_all(&self) -> Result<(), ToolResult> {
        for (file_index, file) in self.files.iter().enumerate() {
            self.recheck_file(file_index, file).await?;
        }
        Ok(())
    }

    async fn recheck_file(
        &self,
        file_index: usize,
        file: &PreparedEditFile,
    ) -> Result<(), ToolResult> {
        let current = match read_fingerprint(&file.resolved_path).await {
            Ok(fingerprint) => fingerprint,
            Err(message) => {
                return Err(stale_result(
                    Some(file_index),
                    Some(file.requested_path.display().to_string()),
                    message,
                ));
            }
        };
        if current != file.fingerprint {
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

    /// Commit prepared files in declaration order with per-file atomic writes.
    pub async fn commit(
        &self,
        cancel_token: &CancellationToken,
        on_progress: &mut (dyn FnMut(ToolResult) + Send),
    ) -> ToolResult {
        let total = self.files.len();
        let mut changes = Vec::with_capacity(total);
        let mut committed_count = 0usize;
        let mut cumulative_added = 0usize;
        let mut cumulative_removed = 0usize;

        for (file_index, file) in self.files.iter().enumerate() {
            if cancel_token.is_cancelled() {
                // Already committed files stay committed; remaining are not_attempted.
                for remaining in self.files.iter().skip(file_index) {
                    changes.push(file_change_json(
                        remaining,
                        "not_attempted",
                        remaining.replacements,
                        remaining.added,
                        remaining.removed,
                        Some(&remaining.diff),
                    ));
                }
                return ToolResult::error(format!(
                    "Edit cancelled after committing {committed_count}/{total} files. \
                     Committed content remains. Re-read remaining files before a fresh Edit call."
                ))
                .with_details(json!({
                    "kind": "edit",
                    "status": "partial_commit",
                    "files": total,
                    "replacements": self.replacements,
                    "added": cumulative_added,
                    "removed": cumulative_removed,
                    "changes": changes,
                }));
            }

            if let Err(result) = self.recheck_file(file_index, file).await {
                // Convert whole-batch stale into partial when earlier files committed.
                if committed_count == 0 {
                    return result;
                }
                let details = result.details.clone().unwrap_or_else(|| json!({}));
                changes.push(file_change_json(
                    file,
                    "failed",
                    file.replacements,
                    file.added,
                    file.removed,
                    Some(&file.diff),
                ));
                for remaining in self.files.iter().skip(file_index + 1) {
                    changes.push(file_change_json(
                        remaining,
                        "not_attempted",
                        remaining.replacements,
                        remaining.added,
                        remaining.removed,
                        Some(&remaining.diff),
                    ));
                }
                let path = file.requested_path.display().to_string();
                return ToolResult::error(format!(
                    "Edit partial commit: {committed_count}/{total} files committed; \
                     {path} became stale before write. Already committed content remains. \
                     Re-read remaining files before a fresh Edit call."
                ))
                .with_details(json!({
                    "kind": "edit",
                    "status": "partial_commit",
                    "files": total,
                    "replacements": self.replacements,
                    "added": cumulative_added,
                    "removed": cumulative_removed,
                    "failed_path": path,
                    "stale": details,
                    "changes": changes,
                }));
            }

            let write_result = self.write_file(file_index, &file.resolved_path, file.staged.as_bytes());
            match write_result {
                Ok(AtomicWriteStatus::Durable) => {
                    committed_count += 1;
                    cumulative_added += file.added;
                    cumulative_removed += file.removed;
                    changes.push(file_change_json(
                        file,
                        "committed",
                        file.replacements,
                        file.added,
                        file.removed,
                        Some(&file.diff),
                    ));
                    on_progress(
                        ToolResult::ok(format!(
                            "committed {committed_count}/{total}: {}",
                            file.requested_path.display()
                        ))
                        .with_details(json!({
                            "kind": "edit_progress",
                            "committed": committed_count,
                            "total": total,
                            "latest_path": file.requested_path.display().to_string(),
                            "added": cumulative_added,
                            "removed": cumulative_removed,
                        })),
                    );
                }
                Ok(AtomicWriteStatus::CommittedUnsynced(error)) => {
                    committed_count += 1;
                    cumulative_added += file.added;
                    cumulative_removed += file.removed;
                    changes.push(file_change_json(
                        file,
                        "committed_unsynced",
                        file.replacements,
                        file.added,
                        file.removed,
                        Some(&file.diff),
                    ));
                    for remaining in self.files.iter().skip(file_index + 1) {
                        changes.push(file_change_json(
                            remaining,
                            "not_attempted",
                            remaining.replacements,
                            remaining.added,
                            remaining.removed,
                            Some(&remaining.diff),
                        ));
                    }
                    return ToolResult::error(format!(
                        "Edit durability uncertain after committing {committed_count}/{total} files. \
                         Contents of {} were installed but parent-directory durability is uncertain ({error}). \
                         Re-read files before a fresh Edit call; do not blindly replay.",
                        file.requested_path.display()
                    ))
                    .with_details(json!({
                        "kind": "edit",
                        "status": "durability_uncertain",
                        "files": total,
                        "replacements": self.replacements,
                        "added": cumulative_added,
                        "removed": cumulative_removed,
                        "changes": changes,
                    }));
                }
                Err(error) => {
                    changes.push(file_change_json(
                        file,
                        "failed",
                        file.replacements,
                        file.added,
                        file.removed,
                        Some(&file.diff),
                    ));
                    for remaining in self.files.iter().skip(file_index + 1) {
                        changes.push(file_change_json(
                            remaining,
                            "not_attempted",
                            remaining.replacements,
                            remaining.added,
                            remaining.removed,
                            Some(&remaining.diff),
                        ));
                    }
                    let status = if committed_count == 0 {
                        "partial_commit"
                    } else {
                        "partial_commit"
                    };
                    let zero = committed_count == 0;
                    let message = if zero {
                        format!(
                            "Edit failed before any durable write at {}: {error}. Zero writes. \
                             Re-read the file and submit a fresh Edit call.",
                            file.requested_path.display()
                        )
                    } else {
                        format!(
                            "Edit partial commit: {committed_count}/{total} files committed; \
                             {} failed ({error}). Already committed content remains and was not rolled back. \
                             Re-read remaining files before a fresh Edit call.",
                            file.requested_path.display()
                        )
                    };
                    return ToolResult::error(message).with_details(json!({
                        "kind": "edit",
                        "status": status,
                        "files": total,
                        "replacements": self.replacements,
                        "added": cumulative_added,
                        "removed": cumulative_removed,
                        "changes": changes,
                    }));
                }
            }
        }

        ToolResult::ok(format!(
            "edited {} files · {} replacements · +{} -{}",
            total, self.replacements, self.added, self.removed
        ))
        .with_details(json!({
            "kind": "edit",
            "status": "committed",
            "files": total,
            "replacements": self.replacements,
            "added": self.added,
            "removed": self.removed,
            "changes": changes,
        }))
    }

    fn write_file(
        &self,
        file_index: usize,
        path: &Path,
        content: &[u8],
    ) -> std::io::Result<AtomicWriteStatus> {
        if self.fail_commit_at == Some(file_index) {
            return Err(std::io::Error::other(format!(
                "injected commit failure at file index {file_index}"
            )));
        }
        write_file_atomic_status(path, content)
    }

    /// Test support: fail the per-file commit writer before replacement at `index`.
    /// Production runtime never calls this.
    #[must_use]
    pub fn with_injected_commit_failure(mut self: Arc<Self>, index: usize) -> Arc<Self> {
        let prepared = Arc::make_mut(&mut self);
        prepared.fail_commit_at = Some(index);
        self
    }

    #[must_use]
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    #[must_use]
    pub fn replacement_count(&self) -> usize {
        self.replacements
    }

    #[must_use]
    pub fn added(&self) -> usize {
        self.added
    }

    #[must_use]
    pub fn removed(&self) -> usize {
        self.removed
    }

    #[must_use]
    pub fn requested_paths(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .map(|file| file.requested_path.clone())
            .collect()
    }
}

fn parse_edit_input(arguments: &serde_json::Value) -> Result<EditInput, ToolResult> {
    match serde_json::from_value::<EditInput>(arguments.clone()) {
        Ok(input) => Ok(input),
        Err(error) => Err(prepare_failed(
            None,
            None,
            None,
            format!("invalid Edit arguments: {error}"),
            "Submit a fresh Edit call using the files[] contract.",
        )),
    }
}

fn validate_edit_input(input: &EditInput) -> Result<(), ToolResult> {
    if input.files.is_empty() {
        return Err(prepare_failed(
            None,
            None,
            None,
            "files must be a non-empty array".to_owned(),
            "Group at least one existing file into files[] and submit a fresh Edit call.",
        ));
    }
    for (file_index, file) in input.files.iter().enumerate() {
        if file.path.as_os_str().is_empty() {
            return Err(prepare_failed(
                Some(file_index),
                None,
                None,
                "path must be non-empty".to_owned(),
                "Provide a non-empty path and submit a fresh Edit call.",
            ));
        }
        if file.replacements.is_empty() {
            return Err(prepare_failed(
                Some(file_index),
                None,
                Some(file.path.display().to_string()),
                "replacements must be a non-empty array".to_owned(),
                "Add at least one replacement for this file.",
            ));
        }
        for (replacement_index, replacement) in file.replacements.iter().enumerate() {
            if replacement.old.is_empty() {
                return Err(prepare_failed(
                    Some(file_index),
                    Some(replacement_index),
                    Some(file.path.display().to_string()),
                    "old must be a non-empty string".to_owned(),
                    "Supply exact non-empty old text from a fresh Read.",
                ));
            }
            if replacement.expected_matches < 1 {
                return Err(prepare_failed(
                    Some(file_index),
                    Some(replacement_index),
                    Some(file.path.display().to_string()),
                    "expected_matches must be at least 1".to_owned(),
                    "Use the observed exact match count (default 1).",
                ));
            }
            if replacement.old == replacement.new {
                return Err(prepare_failed(
                    Some(file_index),
                    Some(replacement_index),
                    Some(file.path.display().to_string()),
                    "old and new are identical (no-op replacement)".to_owned(),
                    "Remove no-op replacements and submit a fresh Edit call.",
                ));
            }
        }
    }
    Ok(())
}

fn prepare_failed(
    file_index: Option<usize>,
    replacement_index: Option<usize>,
    path: Option<String>,
    message: String,
    guidance: &str,
) -> ToolResult {
    let mut content = String::from("Edit prepare failed · zero writes");
    if let Some(path) = path.as_ref() {
        content.push_str(" · ");
        content.push_str(path);
    }
    if let Some(replacement_index) = replacement_index {
        content.push_str(&format!(" · replacement {replacement_index}"));
    }
    content.push('\n');
    content.push_str(&message);
    content.push('\n');
    content.push_str(guidance);

    let mut details = json!({
        "kind": "edit",
        "status": "prepare_failed",
        "message": message,
    });
    if let Some(file_index) = file_index {
        details["file_index"] = json!(file_index);
    }
    if let Some(replacement_index) = replacement_index {
        details["replacement_index"] = json!(replacement_index);
    }
    if let Some(path) = path {
        details["path"] = json!(path);
    }
    ToolResult::error(content).with_details(details)
}

fn stale_result(file_index: Option<usize>, path: Option<String>, message: String) -> ToolResult {
    let mut content = String::from("Edit failed · stale · zero writes\n");
    if let Some(path) = path.as_ref() {
        content.push_str(path);
        content.push('\n');
    }
    content.push_str(&message);
    content.push_str("\nRe-read affected files and submit a new Edit call.");
    let mut details = json!({
        "kind": "edit",
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

fn file_change_json(
    file: &PreparedEditFile,
    status: &str,
    replacements: usize,
    added: usize,
    removed: usize,
    diff: Option<&str>,
) -> serde_json::Value {
    let mut value = json!({
        "path": file.requested_path.display().to_string(),
        "status": status,
        "replacements": replacements,
        "added": added,
        "removed": removed,
    });
    if let Some(diff) = diff {
        value["diff"] = json!(diff);
    }
    value
}

fn count_non_overlapping(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.matches(needle).count()
}

fn replace_non_overlapping(haystack: &str, old: &str, new: &str) -> String {
    haystack.replace(old, new)
}

fn match_line_numbers(haystack: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut search_from = 0usize;
    while let Some(rel) = haystack[search_from..].find(needle) {
        let abs = search_from + rel;
        let line = haystack[..abs].bytes().filter(|b| *b == b'\n').count() + 1;
        lines.push(line);
        search_from = abs + needle.len().max(1);
        if search_from >= haystack.len() {
            break;
        }
    }
    lines
}

fn diff_stats(diff: &str) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

async fn read_fingerprint(path: &Path) -> Result<EditFingerprint, String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!("failed to recheck metadata for {}: {error}", path.display())
    })?;
    if is_reparse_or_symlink(&metadata) {
        return Err(format!(
            "target became a symlink or reparse point: {}",
            path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!("target is no longer a regular file: {}", path.display()));
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| format!("failed to recheck {}: {error}", path.display()))?;
    // Content must still be UTF-8; non-UTF-8 is treated as stale for the prepared plan.
    if String::from_utf8(bytes.clone()).is_err() {
        return Err(format!("target is no longer valid UTF-8: {}", path.display()));
    }
    Ok(EditFingerprint {
        resolved_path: path.to_path_buf(),
        file_kind: EditFileKind::Regular,
        sha256: sha256_bytes(&bytes),
    })
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
mod workspace_policy_tests {
    use super::*;
    use crate::{
        ToolAccess, ToolContext, WorkspaceAccessPolicy, WorkspaceAccessRoot,
        WorkspaceAccessRootKind,
    };
    use serde_json::json;

    #[tokio::test]
    async fn edit_denies_read_only_added_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let path = added.path().join("existing.txt");
        std::fs::write(&path, "before").expect("seed file");
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

        let result = EditTool
            .execute(
                &ctx,
                json!({
                    "files": [{
                        "path": path,
                        "replacements": [{ "old": "before", "new": "after" }]
                    }]
                }),
            )
            .await
            .expect("tool result");

        assert!(result.is_error);
        let details = result.details.expect("details");
        assert_eq!(details["status"], "prepare_failed");
        assert_eq!(std::fs::read_to_string(path).expect("read file"), "before");
    }
}
