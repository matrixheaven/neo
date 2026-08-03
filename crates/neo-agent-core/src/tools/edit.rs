use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::diff::{diff_stats, unified_diff};
use super::{Tool, ToolContext, ToolFuture, ToolResult, schema};
use crate::approval::{EditApprovalChange, EditApprovalPresentation};
use crate::permissions::{FileWriteApprovalOperation, SessionApprovalKey, SessionApprovalScope};
use crate::session::atomic_file::{
    AtomicWriteStatus, is_reparse_or_symlink, replace_existing_file_atomic_status,
};

const fn default_expected_matches() -> usize {
    1
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditInput {
    #[schemars(
        description = "Path to an existing file. Relative paths resolve against the working directory."
    )]
    path: PathBuf,
    #[schemars(
        description = "Exact non-empty current UTF-8 text to replace. Include enough context to make it unique."
    )]
    old: String,
    #[schemars(description = "Replacement text. Empty removes the matched text.")]
    new: String,
    #[serde(default = "default_expected_matches")]
    #[schemars(
        description = "Optional exact non-overlapping match count for this edit. Defaults to 1."
    )]
    expected_matches: usize,
}

fn prepare_edit(
    context: &ToolContext,
    file_index: usize,
    input: &EditInput,
) -> Result<PreparedEditFile, ToolResult> {
    validate_edit_candidate(context, file_index, &input.path)?;
    let resolved = resolve_edit_target(context, file_index, &input.path)?;
    let bytes = read_edit_bytes(file_index, &input.path, &resolved)?;
    let original = String::from_utf8(bytes.clone()).map_err(|_| {
        prepare_failed(
            Some(file_index),
            None,
            Some(input.path.display().to_string()),
            &format!("file is not valid UTF-8: {}", resolved.display()),
            "Edit only supports existing UTF-8 regular files.",
        )
    })?;
    let staged = apply_edit(input, &original)?;

    if staged == original {
        return Err(prepare_failed(
            Some(file_index),
            None,
            Some(input.path.display().to_string()),
            &format!(
                "file is unchanged after replacements: {}",
                input.path.display()
            ),
            "Remove no-op replacements and submit a fresh Edit call.",
        ));
    }

    let display_path = input.path.to_string_lossy();
    let diff = unified_diff(&display_path, &original, &staged);
    let (added, removed) = diff_stats(&diff);
    let fingerprint = EditFingerprint {
        resolved_path: resolved.clone(),
        file_kind: EditFileKind::Regular,
        sha256: sha256_bytes(&bytes),
    };

    Ok(PreparedEditFile {
        requested_path: input.path.clone(),
        resolved_path: resolved,
        fingerprint,
        original,
        operations: vec![input.clone()],
        staged,
        replacements: 1,
        added,
        removed,
        diff,
    })
}

fn validate_edit_candidate(
    context: &ToolContext,
    file_index: usize,
    path: &Path,
) -> Result<(), ToolResult> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        context.workspace_root().join(path)
    };
    match std::fs::symlink_metadata(&candidate) {
        Ok(metadata) if is_reparse_or_symlink(&metadata) => Err(prepare_failed(
            Some(file_index),
            None,
            Some(path.display().to_string()),
            &format!(
                "refusing symlink or reparse point target: {}",
                path.display()
            ),
            "Edit only supports existing UTF-8 regular files.",
        )),
        Ok(metadata) if !metadata.is_file() => Err(prepare_failed(
            Some(file_index),
            None,
            Some(path.display().to_string()),
            &format!("target is not a regular file: {}", path.display()),
            "Edit only supports existing UTF-8 regular files.",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(prepare_failed(
            Some(file_index),
            None,
            Some(path.display().to_string()),
            &format!("file does not exist: {}", path.display()),
            "Use Write to create new files, or re-read existing targets first.",
        )),
        Err(error) => Err(prepare_failed(
            Some(file_index),
            None,
            Some(path.display().to_string()),
            &format!("failed to read metadata for {}: {error}", path.display()),
            "Re-read the file and submit a fresh Edit call.",
        )),
    }
}

fn resolve_edit_target(
    context: &ToolContext,
    file_index: usize,
    path: &Path,
) -> Result<PathBuf, ToolResult> {
    let resolved = context
        .resolve_parent_for_write(path)
        .map_err(|error| {
            prepare_failed(
                Some(file_index),
                None,
                Some(path.display().to_string()),
                &format!("path resolution failed: {error}"),
                "Re-read the path and submit a fresh Edit call.",
            )
        })?
        .canonicalize()
        .map_err(|error| {
            prepare_failed(
                Some(file_index),
                None,
                Some(path.display().to_string()),
                &format!("failed to resolve existing target: {error}"),
                "Re-read the path and submit a fresh Edit call.",
            )
        })?;
    Ok(resolved)
}

fn read_edit_bytes(file_index: usize, path: &Path, resolved: &Path) -> Result<Vec<u8>, ToolResult> {
    read_regular_file_no_follow(resolved).map_err(|error| {
        prepare_failed(
            Some(file_index), None, Some(path.display().to_string()),
            &format!("failed to read {}: {error}", resolved.display()),
            "Edit only supports existing UTF-8 regular files. Re-read the file and submit a fresh Edit call.",
        )
    })
}

fn apply_edit(input: &EditInput, original: &str) -> Result<String, ToolResult> {
    let actual = count_non_overlapping(original, &input.old);
    if actual != input.expected_matches {
        let lines = match_line_numbers(original, &input.old);
        let line_list = if lines.is_empty() {
            "none".to_owned()
        } else {
            lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let guidance = if actual == 0 {
            "Use Grep on a distinctive fragment or Read the smallest relevant range, then correct old and submit a fresh Edit call.".to_owned()
        } else {
            format!(
                "Read the smallest ranges around the reported match lines to make old more specific, or set expected_matches to {actual} only if every match is intended; then submit a fresh Edit call."
            )
        };
        return Err(prepare_failed(
            None,
            None,
            Some(input.path.display().to_string()),
            &format!(
                "expected {} exact matches · found {actual}; matches at lines {line_list}",
                input.expected_matches
            ),
            &guidance,
        ));
    }
    let staged = replace_non_overlapping(original, &input.old, &input.new);
    if staged == original {
        return Err(prepare_failed(
            Some(0),
            None,
            Some(input.path.display().to_string()),
            &format!(
                "file is unchanged after replacements: {}",
                input.path.display()
            ),
            "Remove no-op replacements and submit a fresh Edit call.",
        ));
    }
    Ok(staged)
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
pub(crate) struct PreparedEdit {
    files: Vec<PreparedEditFile>,
    replacements: usize,
    added: usize,
    removed: usize,
}

#[derive(Debug, Clone)]
struct PreparedEditFile {
    requested_path: PathBuf,
    resolved_path: PathBuf,
    fingerprint: EditFingerprint,
    original: String,
    operations: Vec<EditInput>,
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
        "Apply one exact-text replacement to one existing UTF-8 file.\n\n\
         CRITICAL - READ BEFORE EDIT: Never call Edit from memory. Build old from \
         the latest Read of that exact target in the current turn. If needed, Read only the \
         smallest relevant range and wait for its result before Edit; never batch Read with Edit. \
         Exclude Read line numbers and <system> status text.\n\n\
         Use exactly this input shape:\n\
         {\"path\":\"src/file.rs\",\"old\":\"exact existing text\",\"new\":\"replacement text\"}\n\n\
         Parameters:\n\
         - path: existing file path\n\
         - old: exact current text to replace\n\
         - new: replacement text; empty deletes old\n\
         - expected_matches: optional exact match count, default 1\n\n\
         For the normal single-match case, omit expected_matches and include enough \
         surrounding text in old to make it unique. \
         Set expected_matches only when intentionally replacing an observed exact count \
         greater than 1.\n\n\
         If a match-count error occurs, use Grep or the smallest relevant Read range before \
         constructing a fresh Edit call. Do not replay the failed arguments.\n\n\
         To edit multiple files, emit multiple Edit tool calls in the same response.\n\n\
         Use Write to create files or replace complete file contents."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<EditInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_file_write_allowed()?;
            let prepared = match PreparedEdit::prepare(ctx, &input) {
                Ok(prepared) => prepared,
                Err(result) => return Ok(result),
            };
            if let Err(result) = prepared.recheck_all(ctx) {
                return Ok(result);
            }
            let mut on_progress = |_update: ToolResult| {};
            Ok(prepared.commit(ctx, &ctx.cancel_token, &mut on_progress))
        })
    }
}

impl PreparedEdit {
    /// Side-effect-free preparation of one Edit replacement.
    pub fn prepare(
        context: &ToolContext,
        arguments: &serde_json::Value,
    ) -> Result<Arc<Self>, ToolResult> {
        let input: EditInput = parse_edit_input(arguments)?;
        validate_edit_input(&input)?;
        let prepared_files = vec![prepare_edit(context, 0, &input)?];

        let added = prepared_files.iter().map(|file| file.added).sum();
        let removed = prepared_files.iter().map(|file| file.removed).sum();
        if prepared_files.is_empty() || (added == 0 && removed == 0) {
            return Err(prepare_failed(
                None,
                None,
                None,
                "Edit is a no-op",
                "Supply at least one effective replacement and submit a fresh Edit call.",
            ));
        }

        Ok(Arc::new(Self {
            files: prepared_files,
            replacements: 1,
            added,
            removed,
        }))
    }

    pub(crate) fn single_resolved_target(&self) -> Option<&Path> {
        (self.files.len() == 1).then(|| self.files[0].resolved_path.as_path())
    }

    /// Compose consecutive prepared edits for one target without touching disk.
    /// The first prepared fingerprint remains the transaction's external CAS
    /// guard; every operation after the first is applied to the prior staged
    /// content and must still match there.
    pub(crate) fn coalesce_same_file(edits: &[&PreparedEdit]) -> Result<Arc<Self>, ToolResult> {
        let Some(first) = edits.first() else {
            return Err(same_batch_conflict(
                None,
                None,
                "same-file Edit group is empty",
            ));
        };
        let Some(first_file) = first.files.first() else {
            return Err(same_batch_conflict(
                None,
                None,
                "same-file Edit group has no prepared target",
            ));
        };
        if edits.iter().any(|edit| edit.files.len() != 1) {
            return Err(same_batch_conflict(
                None,
                Some(first_file.requested_path.display().to_string()),
                "same-file Edit group contains a multi-file prepared payload",
            ));
        }

        let target = first_file.resolved_path.as_path();
        let base_fingerprint = &first_file.fingerprint;
        let mut staged = first_file.original.clone();
        let mut operations = Vec::new();

        for (edit_index, edit) in edits.iter().enumerate() {
            let file = &edit.files[0];
            if file.resolved_path.as_path() != target
                || file.fingerprint != base_fingerprint.clone()
            {
                return Err(same_batch_conflict(
                    Some(edit_index),
                    Some(file.requested_path.display().to_string()),
                    "same-file Edit calls were prepared from different file versions",
                ));
            }
            for input in &file.operations {
                let next = match apply_edit(input, &staged) {
                    Ok(next) if next != staged => next,
                    Ok(_) => {
                        return Err(same_batch_conflict(
                            Some(edit_index),
                            Some(file.requested_path.display().to_string()),
                            "Edit is a no-op after earlier same-file edits",
                        ));
                    }
                    Err(_) => {
                        return Err(same_batch_conflict(
                            Some(edit_index),
                            Some(file.requested_path.display().to_string()),
                            "Edit.old no longer matches after earlier same-file edits",
                        ));
                    }
                };
                staged = next;
                operations.push(input.clone());
            }
        }

        if operations.is_empty() || staged == first_file.original {
            return Err(same_batch_conflict(
                None,
                Some(first_file.requested_path.display().to_string()),
                "same-file Edit group is a no-op",
            ));
        }

        let display_path = first_file.requested_path.to_string_lossy();
        let diff = unified_diff(&display_path, &first_file.original, &staged);
        let (added, removed) = diff_stats(&diff);
        let replacement_count = operations.len();
        let prepared_file = PreparedEditFile {
            requested_path: first_file.requested_path.clone(),
            resolved_path: first_file.resolved_path.clone(),
            fingerprint: first_file.fingerprint.clone(),
            original: first_file.original.clone(),
            operations,
            staged,
            replacements: replacement_count,
            added,
            removed,
            diff,
        };

        Ok(Arc::new(Self {
            files: vec![prepared_file],
            replacements: replacement_count,
            added,
            removed,
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

    #[must_use]
    pub(crate) fn cancelled_before_commit_result(&self) -> ToolResult {
        ToolResult::error(
            "Edit cancelled before the first commit. Zero writes. Re-read targets before a fresh Edit call.",
        )
        .with_details(json!({
            "kind": "edit",
            "status": "cancelled",
            "cause": "cancelled",
            "files": self.files.len(),
            "replacements": self.replacements,
            "added": 0,
            "removed": 0,
            "changes": self.files.iter().map(|file| file_change_json(
                file,
                "not_attempted",
                file.replacements,
                file.added,
                file.removed,
                Some(&file.diff),
            )).collect::<Vec<_>>(),
        }))
    }

    /// Whole-call fingerprint recheck. Zero writes on mismatch.
    pub fn recheck_all(&self, context: &ToolContext) -> Result<(), ToolResult> {
        for (file_index, file) in self.files.iter().enumerate() {
            Self::recheck_file(context, file_index, file)?;
        }
        Ok(())
    }

    fn recheck_file(
        context: &ToolContext,
        file_index: usize,
        file: &PreparedEditFile,
    ) -> Result<(), ToolResult> {
        let resolved = context
            .resolve_parent_for_write(&file.requested_path)
            .map_err(|error| {
                stale_result(
                    Some(file_index),
                    Some(file.requested_path.display().to_string()),
                    &format!("path resolution changed after approval: {error}"),
                )
            })?
            .canonicalize()
            .map_err(|error| {
                stale_result(
                    Some(file_index),
                    Some(file.requested_path.display().to_string()),
                    &format!("path resolution changed after approval: {error}"),
                )
            })?;
        if resolved != file.resolved_path {
            return Err(stale_result(
                Some(file_index),
                Some(file.requested_path.display().to_string()),
                &format!(
                    "{} resolves to a different target after approval",
                    file.requested_path.display()
                ),
            ));
        }
        let current = match read_fingerprint(&resolved) {
            Ok(fingerprint) => fingerprint,
            Err(message) => {
                return Err(stale_result(
                    Some(file_index),
                    Some(file.requested_path.display().to_string()),
                    &message,
                ));
            }
        };
        if current != file.fingerprint {
            return Err(stale_result(
                Some(file_index),
                Some(file.requested_path.display().to_string()),
                &format!(
                    "{} changed after approval; planned content no longer matches the current workspace",
                    file.requested_path.display()
                ),
            ));
        }
        Ok(())
    }

    /// Commit prepared files in declaration order with per-file atomic writes.
    pub fn commit(
        &self,
        context: &ToolContext,
        cancel_token: &CancellationToken,
        on_progress: &mut (dyn FnMut(ToolResult) + Send),
    ) -> ToolResult {
        self.commit_with_writer(context, cancel_token, on_progress, |_, path, content| {
            replace_existing_file_atomic_status(path, content)
        })
    }

    fn commit_with_writer(
        &self,
        context: &ToolContext,
        cancel_token: &CancellationToken,
        on_progress: &mut (dyn FnMut(ToolResult) + Send),
        mut write_file: impl FnMut(usize, &Path, &[u8]) -> std::io::Result<AtomicWriteStatus>,
    ) -> ToolResult {
        let total = self.files.len();
        let mut changes = Vec::with_capacity(total);
        let mut committed_count = 0usize;
        let mut cumulative_added = 0usize;
        let mut cumulative_removed = 0usize;

        for (file_index, file) in self.files.iter().enumerate() {
            if cancel_token.is_cancelled() {
                if committed_count == 0 {
                    return self.cancelled_before_commit_result();
                }
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
                    "Edit cancelled after committing {committed_count}/{total} files. Committed content remains. Re-read remaining files before a fresh Edit call."
                )).with_details(json!({
                    "kind": "edit",
                    "status": "partial_commit",
                    "cause": "cancelled",
                    "files": total,
                    "replacements": self.replacements,
                    "added": cumulative_added,
                    "removed": cumulative_removed,
                    "changes": changes,
                }));
            }

            if let Err(result) = Self::recheck_file(context, file_index, file) {
                // Retained prepared-file loop reports any already committed content truthfully.
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

            let write_result = write_file(file_index, &file.resolved_path, file.staged.as_bytes());
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
                        "commit_failed"
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
}

fn parse_edit_input(arguments: &serde_json::Value) -> Result<EditInput, ToolResult> {
    match serde_json::from_value::<EditInput>(arguments.clone()) {
        Ok(input) => Ok(input),
        Err(error) => Err(prepare_failed(
            None,
            None,
            None,
            &format!("invalid Edit arguments: {error}"),
            "Submit exactly {\"path\":\"...\",\"old\":\"...\",\"new\":\"...\"}.",
        )),
    }
}

fn validate_edit_input(input: &EditInput) -> Result<(), ToolResult> {
    if input.path.as_os_str().is_empty() {
        return Err(prepare_failed(
            None,
            None,
            None,
            "path must be non-empty",
            "Provide a non-empty path and submit a fresh Edit call.",
        ));
    }
    if input.old.is_empty() {
        return Err(prepare_failed(
            None,
            None,
            Some(input.path.display().to_string()),
            "old must be a non-empty string",
            "Supply exact non-empty old text from a fresh Read.",
        ));
    }
    if input.expected_matches < 1 {
        return Err(prepare_failed(
            None,
            None,
            Some(input.path.display().to_string()),
            "expected_matches must be at least 1",
            "Use the observed exact match count (default 1).",
        ));
    }
    if input.old == input.new {
        return Err(prepare_failed(
            None,
            None,
            Some(input.path.display().to_string()),
            "old and new are identical (no-op replacement)",
            "Remove no-op replacements and submit a fresh Edit call.",
        ));
    }
    Ok(())
}

fn prepare_failed(
    file_index: Option<usize>,
    edit_index: Option<usize>,
    path: Option<String>,
    message: &str,
    guidance: &str,
) -> ToolResult {
    let mut content = String::from("Edit prepare failed · zero writes");
    if let Some(path) = path.as_ref() {
        content.push_str(" · ");
        content.push_str(path);
    }
    if let Some(edit_index) = edit_index {
        let _ = write!(content, " · edit {edit_index}");
    }
    content.push('\n');
    content.push_str(message);
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
    if let Some(edit_index) = edit_index {
        details["edit_index"] = json!(edit_index);
    }
    if let Some(path) = path {
        details["path"] = json!(path);
    }
    ToolResult::error(content).with_details(details)
}

fn stale_result(file_index: Option<usize>, path: Option<String>, message: &str) -> ToolResult {
    let mut content = String::from("Edit failed · stale · zero writes\n");
    if let Some(path) = path.as_ref() {
        content.push_str(path);
        content.push('\n');
    }
    content.push_str(message);
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

fn same_batch_conflict(
    edit_index: Option<usize>,
    path: Option<String>,
    message: &str,
) -> ToolResult {
    let mut content = String::from("Edit failed · same-batch conflict · zero writes\n");
    if let Some(path) = path.as_ref() {
        content.push_str(path);
        content.push('\n');
    }
    content.push_str(message);
    content.push_str(
        "\nNo side effect occurred. Re-read the affected file and submit a fresh Edit call.",
    );
    let mut details = json!({
        "kind": "edit",
        "status": "same_batch_conflict",
        "reason": "same_batch_conflict",
        "side_effect_occurred": false,
        "message": message,
    });
    if let Some(edit_index) = edit_index {
        details["edit_index"] = json!(edit_index);
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

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

fn read_fingerprint(path: &Path) -> Result<EditFingerprint, String> {
    let bytes = read_regular_file_no_follow(path)
        .map_err(|error| format!("failed to recheck {}: {error}", path.display()))?;
    // Content must still be UTF-8; non-UTF-8 is treated as stale for the prepared plan.
    if String::from_utf8(bytes.clone()).is_err() {
        return Err(format!(
            "target is no longer valid UTF-8: {}",
            path.display()
        ));
    }
    Ok(EditFingerprint {
        resolved_path: path.to_path_buf(),
        file_kind: EditFileKind::Regular,
        sha256: sha256_bytes(&bytes),
    })
}

fn read_regular_file_no_follow(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;

    let mut file = open_no_follow(path)?;
    let metadata = file.metadata()?;
    if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("target is not an existing regular file: {}", path.display()),
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(unix)]
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};

    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    Ok(fd.into())
}

#[cfg(windows)]
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use winapi::um::winbase::FILE_FLAG_OPEN_REPARSE_POINT;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(unix, windows)))]
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
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
                json!({ "path": path, "old": "before", "new": "after" }),
            )
            .await
            .expect("tool result");

        assert!(result.is_error);
        let details = result.details.expect("details");
        assert_eq!(details["status"], "prepare_failed");
        assert_eq!(std::fs::read_to_string(path).expect("read file"), "before");
    }

    #[tokio::test]
    async fn cancellation_before_first_commit_writes_nothing() {
        let workspace = tempfile::tempdir().expect("workspace");
        let path = workspace.path().join("existing.txt");
        std::fs::write(&path, "before\n").expect("seed file");
        let context = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());
        let prepared = PreparedEdit::prepare(
            &context,
            &json!({ "path": "existing.txt", "old": "before", "new": "after" }),
        )
        .expect("prepare");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut on_progress = |_update| {};

        let result = prepared.commit_with_writer(&context, &cancel, &mut on_progress, |_, _, _| {
            panic!("writer must not run after cancellation")
        });

        assert!(result.is_error);
        let details = result.details.expect("details");
        assert_eq!(details["status"], "cancelled");
        assert_eq!(details["cause"], "cancelled");
        assert_eq!(details["changes"][0]["status"], "not_attempted");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read file"),
            "before\n"
        );
    }

    #[tokio::test]
    async fn writer_failure_reports_zero_writes() {
        let workspace = tempfile::tempdir().expect("workspace");
        let context = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());
        std::fs::write(workspace.path().join("a.txt"), "aaa\n").expect("seed file");
        let prepared = PreparedEdit::prepare(
            &context,
            &json!({ "path": "a.txt", "old": "aaa", "new": "AAA" }),
        )
        .expect("prepare");
        let mut on_progress = |_update| {};

        let first_failure = prepared.commit_with_writer(
            &context,
            &CancellationToken::new(),
            &mut on_progress,
            |_, _, _| Err(std::io::Error::other("first writer failure")),
        );

        assert!(first_failure.is_error);
        let details = first_failure.details.expect("first failure details");
        assert_eq!(details["status"], "commit_failed");
        assert_eq!(details["changes"][0]["status"], "failed");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("a.txt")).expect("a"),
            "aaa\n"
        );
    }

    #[tokio::test]
    async fn prepare_rejects_old_array_unknown_and_non_utf8_inputs() {
        let workspace = tempfile::tempdir().expect("workspace");
        let context = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());
        std::fs::write(workspace.path().join("file.txt"), "before\n").expect("text file");

        let unknown = PreparedEdit::prepare(
            &context,
            &json!({ "path": "file.txt", "old": "before", "new": "after", "extra": true }),
        )
        .expect_err("unknown field on edit item");
        assert_eq!(
            unknown.details.expect("unknown details")["status"],
            "prepare_failed"
        );

        let old_array = PreparedEdit::prepare(
            &context,
            &json!({
                "edits": [{ "path": "file.txt", "old": "before", "new": "after" }]
            }),
        )
        .expect_err("old array contract");
        assert_eq!(
            old_array.details.expect("old array details")["status"],
            "prepare_failed"
        );

        std::fs::write(workspace.path().join("binary.bin"), [0xff, 0xfe]).expect("binary file");
        let non_utf8 = PreparedEdit::prepare(
            &context,
            &json!({ "path": "binary.bin", "old": "before", "new": "after" }),
        )
        .expect_err("non-UTF-8 target");
        assert_eq!(
            non_utf8.details.expect("non-UTF-8 details")["status"],
            "prepare_failed"
        );
    }

    #[tokio::test]
    async fn prepared_edit_coalesces_disjoint_same_file_operations() {
        let workspace = tempfile::tempdir().expect("workspace");
        let path = workspace.path().join("file.txt");
        std::fs::write(&path, "one\ntwo\n").expect("seed file");
        let context = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());
        let first = PreparedEdit::prepare(
            &context,
            &json!({ "path": "file.txt", "old": "one", "new": "ONE" }),
        )
        .expect("first prepare");
        let second = PreparedEdit::prepare(
            &context,
            &json!({ "path": "file.txt", "old": "two", "new": "TWO" }),
        )
        .expect("second prepare");

        let coalesced =
            PreparedEdit::coalesce_same_file(&[first.as_ref(), second.as_ref()]).expect("coalesce");
        assert_eq!(coalesced.files[0].staged, "ONE\nTWO\n");
        assert_eq!(coalesced.replacements, 2);

        let mut writes = 0;
        let mut on_progress = |_update| {};
        let result = coalesced.commit_with_writer(
            &context,
            &CancellationToken::new(),
            &mut on_progress,
            |_, _, content| {
                writes += 1;
                assert_eq!(content, b"ONE\nTWO\n");
                std::fs::write(&path, content)?;
                Ok(AtomicWriteStatus::Durable)
            },
        );

        assert!(!result.is_error);
        assert_eq!(writes, 1);
        assert_eq!(
            std::fs::read_to_string(path).expect("read result"),
            "ONE\nTWO\n"
        );
    }

    #[tokio::test]
    async fn prepared_edit_coalescing_rejects_conflicting_sequence() {
        let workspace = tempfile::tempdir().expect("workspace");
        let path = workspace.path().join("file.txt");
        std::fs::write(&path, "before\n").expect("seed file");
        let context = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());
        let first = PreparedEdit::prepare(
            &context,
            &json!({ "path": "file.txt", "old": "before", "new": "after" }),
        )
        .expect("first prepare");
        let second = PreparedEdit::prepare(
            &context,
            &json!({ "path": "file.txt", "old": "before", "new": "again" }),
        )
        .expect("second prepare");

        let result = PreparedEdit::coalesce_same_file(&[first.as_ref(), second.as_ref()])
            .expect_err("conflicting same-file edits");
        let details = result.details.expect("conflict details");
        assert_eq!(details["status"], "same_batch_conflict");
        assert_eq!(details["side_effect_occurred"], false);
        assert_eq!(
            std::fs::read_to_string(path).expect("read file"),
            "before\n"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn fifo_swap_returns_stale_without_blocking() {
        use rustix::fs::{CWD, Mode, mkfifoat};

        let workspace = tempfile::tempdir().expect("workspace");
        let path = workspace.path().join("target.txt");
        std::fs::write(&path, "before\n").expect("text file");
        let context = ToolContext::new(workspace.path())
            .expect("context")
            .with_access(ToolAccess::all());
        let prepared = PreparedEdit::prepare(
            &context,
            &json!({ "path": "target.txt", "old": "before", "new": "after" }),
        )
        .expect("prepare");
        std::fs::remove_file(&path).expect("remove target");
        mkfifoat(CWD, &path, Mode::RUSR | Mode::WUSR).expect("create fifo");

        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            sender
                .send(prepared.recheck_all(&context))
                .expect("send result");
        });
        let result = receiver
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("FIFO recheck must not block")
            .expect_err("FIFO must be stale");
        worker.join().expect("recheck worker");

        assert_eq!(result.details.expect("details")["status"], "stale");
    }
}
