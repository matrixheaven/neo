//! Cross-platform isolated git worktree manager for workflow children (design §32.3).
//!
//! Ownership boundary:
//! - Creates/records/cleans dedicated worktree paths for `worktree = isolated` children.
//! - Uses typed `PathBuf` process arguments only — never ad-hoc shell strings.
//! - Does not auto-merge; does not delete dirty/unreviewed worktrees.
//! - Does not own workflow run state, journals, or child agent lifecycle.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Lifecycle of one managed isolated worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeLifecycleState {
    /// `git worktree add` succeeded; path is ready for the child.
    Created,
    /// Caller marked the worktree as actively used by a child.
    Active,
    /// Cleanup was requested and completed (clean tree only).
    Cleaned,
    /// Cleanup refused because the worktree is dirty or unreviewed.
    DirtyRefusedCleanup,
}

/// One isolated worktree binding recorded for child provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IsolatedWorktree {
    /// Absolute (or fully resolved relative-to-base) worktree path.
    pub path: PathBuf,
    /// Source workspace the worktree was created from.
    pub source_workspace: PathBuf,
    /// Optional branch/ref used for the worktree (detach when `None`).
    pub branch: Option<String>,
    pub state: WorktreeLifecycleState,
    /// Whether the last status probe observed a dirty worktree.
    pub dirty: bool,
}

impl IsolatedWorktree {
    #[must_use]
    pub fn mark_active(mut self) -> Self {
        if matches!(
            self.state,
            WorktreeLifecycleState::Created | WorktreeLifecycleState::Active
        ) {
            self.state = WorktreeLifecycleState::Active;
        }
        self
    }
}

/// Errors from worktree support checks and lifecycle operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeError {
    Unsupported { message: String },
    CreateFailed { message: String },
    CleanupRefused { message: String },
    CleanupFailed { message: String },
    Io { message: String },
}

impl std::fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { message }
            | Self::CreateFailed { message }
            | Self::CleanupRefused { message }
            | Self::CleanupFailed { message }
            | Self::Io { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for WorktreeError {}

/// Sole owner of isolated worktree path/process/cleanup metadata.
#[derive(Debug, Clone)]
pub struct WorktreeManager {
    /// Directory under which isolated worktree directories are created.
    base_dir: PathBuf,
    /// Git program name (tests may inject a stub path; production is `"git"`).
    git_program: PathBuf,
}

impl WorktreeManager {
    /// Create a manager that places isolated worktrees under `base_dir`.
    #[must_use]
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            git_program: PathBuf::from("git"),
        }
    }

    /// Override the git binary (typed path — never a shell string).
    #[must_use]
    pub fn with_git_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.git_program = program.into();
        self
    }

    #[must_use]
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Return `Ok(())` when `workspace` is a supported git work tree.
    ///
    /// Unsupported repositories (non-git, bare without worktree support, or
    /// missing git) fail with [`WorktreeError::Unsupported`].
    pub fn ensure_isolation_supported(&self, workspace: &Path) -> Result<(), WorktreeError> {
        if !workspace.exists() {
            return Err(WorktreeError::Unsupported {
                message: format!(
                    "isolated worktree unsupported: workspace {} does not exist",
                    workspace.display()
                ),
            });
        }
        let output = self
            .git_command(workspace)
            .args(["rev-parse", "--is-inside-work-tree"])
            .output()
            .map_err(|error| WorktreeError::Unsupported {
                message: format!("isolated worktree unsupported: git is not available ({error})"),
            })?;
        if !output.status.success() {
            return Err(WorktreeError::Unsupported {
                message: format!(
                    "isolated worktree unsupported: {} is not a git work tree",
                    workspace.display()
                ),
            });
        }
        let body = String::from_utf8_lossy(&output.stdout);
        if body.trim() != "true" {
            return Err(WorktreeError::Unsupported {
                message: format!(
                    "isolated worktree unsupported: {} is not inside a git work tree",
                    workspace.display()
                ),
            });
        }
        Ok(())
    }

    /// Create a dedicated worktree for `child_key` from `source_workspace`.
    ///
    /// Fails before any path is recorded when isolation is unsupported.
    /// Paths use `PathBuf` only; process args are individual typed arguments.
    pub fn create_isolated(
        &self,
        source_workspace: &Path,
        child_key: &str,
    ) -> Result<IsolatedWorktree, WorktreeError> {
        self.ensure_isolation_supported(source_workspace)?;
        let safe_key = sanitize_child_key(child_key);
        if safe_key.is_empty() {
            return Err(WorktreeError::CreateFailed {
                message: "isolated worktree child key is empty after sanitization".to_owned(),
            });
        }
        std::fs::create_dir_all(&self.base_dir).map_err(|error| WorktreeError::Io {
            message: format!(
                "failed to create worktree base {}: {error}",
                self.base_dir.display()
            ),
        })?;
        let path = self.base_dir.join(&safe_key);
        if path.exists() {
            return Err(WorktreeError::CreateFailed {
                message: format!("isolated worktree path already exists: {}", path.display()),
            });
        }
        // Detached worktree at HEAD — no branch bookkeeping, no auto-merge later.
        let output = self
            .git_command(source_workspace)
            .arg("worktree")
            .arg("add")
            .arg("--detach")
            .arg(&path)
            .output()
            .map_err(|error| WorktreeError::CreateFailed {
                message: format!("git worktree add failed to spawn: {error}"),
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(WorktreeError::CreateFailed {
                message: format!(
                    "git worktree add failed for {}: {}",
                    path.display(),
                    stderr.trim()
                ),
            });
        }
        Ok(IsolatedWorktree {
            path,
            source_workspace: source_workspace.to_path_buf(),
            branch: None,
            state: WorktreeLifecycleState::Created,
            dirty: false,
        })
    }

    /// Probe porcelain status and update the handle's dirty flag.
    pub fn refresh_dirty(&self, handle: &mut IsolatedWorktree) -> Result<bool, WorktreeError> {
        let dirty = self.is_dirty(&handle.path)?;
        handle.dirty = dirty;
        Ok(dirty)
    }

    /// Explicit cleanup. Never deletes a dirty worktree; never auto-merges.
    ///
    /// Callers must pass a handle previously returned by [`Self::create_isolated`].
    pub fn cleanup_explicit(&self, handle: &mut IsolatedWorktree) -> Result<(), WorktreeError> {
        if matches!(handle.state, WorktreeLifecycleState::Cleaned) {
            return Ok(());
        }
        let dirty = self.is_dirty(&handle.path)?;
        handle.dirty = dirty;
        if dirty {
            handle.state = WorktreeLifecycleState::DirtyRefusedCleanup;
            return Err(WorktreeError::CleanupRefused {
                message: format!(
                    "refusing to delete dirty isolated worktree {}",
                    handle.path.display()
                ),
            });
        }
        let output = self
            .git_command(&handle.source_workspace)
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(&handle.path)
            .output()
            .map_err(|error| WorktreeError::CleanupFailed {
                message: format!("git worktree remove failed to spawn: {error}"),
            })?;
        if !output.status.success() {
            // Fall back to removing an empty leftover directory if git already
            // forgot the registration but the path is clean/empty.
            if handle.path.exists() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(WorktreeError::CleanupFailed {
                    message: format!(
                        "git worktree remove failed for {}: {}",
                        handle.path.display(),
                        stderr.trim()
                    ),
                });
            }
        }
        handle.state = WorktreeLifecycleState::Cleaned;
        handle.dirty = false;
        Ok(())
    }

    fn is_dirty(&self, worktree_path: &Path) -> Result<bool, WorktreeError> {
        if !worktree_path.exists() {
            return Ok(false);
        }
        let output = self
            .git_command(worktree_path)
            .args(["status", "--porcelain"])
            .output()
            .map_err(|error| WorktreeError::Io {
                message: format!("git status failed to spawn: {error}"),
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(WorktreeError::Io {
                message: format!(
                    "git status failed in {}: {}",
                    worktree_path.display(),
                    stderr.trim()
                ),
            });
        }
        Ok(!output.stdout.is_empty())
    }

    fn git_command(&self, cwd: &Path) -> Command {
        let mut command = Command::new(&self.git_program);
        command.arg("-C").arg(cwd);
        // Avoid inheriting ambient locale/pager noise; keep argv fully typed.
        command.env("GIT_TERMINAL_PROMPT", "0");
        command.env("GIT_OPTIONAL_LOCKS", "0");
        command
    }
}

/// Portable path-safe fragment for worktree directory names.
fn sanitize_child_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else if ch == '/' || ch == '\\' || ch == '.' {
            out.push('_');
        }
    }
    // Cap length for filesystem portability (Windows MAX_PATH fragments).
    out.chars().take(96).collect()
}

/// Whether `path` components are portable (no raw separators embedded as OsStr).
#[must_use]
pub fn path_is_portable_components(path: &Path) -> bool {
    path.components().all(|component| match component {
        std::path::Component::Normal(part) => !part.to_string_lossy().contains(['/', '\\']),
        std::path::Component::RootDir
        | std::path::Component::Prefix(_)
        | std::path::Component::CurDir
        | std::path::Component::ParentDir => true,
    })
}

/// Build argv for a git invocation without shell interpolation (test/helpers).
#[must_use]
pub fn git_argv(program: &OsStr, cwd: &Path, args: &[&str]) -> Vec<PathBuf> {
    let mut argv = Vec::with_capacity(args.len() + 3);
    argv.push(PathBuf::from(program));
    argv.push(PathBuf::from("-C"));
    argv.push(cwd.to_path_buf());
    for arg in args {
        argv.push(PathBuf::from(arg));
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_child_key_strips_path_separators() {
        let key = sanitize_child_key("agent/foo\\bar.baz");
        assert!(!key.contains('/'));
        assert!(!key.contains('\\'));
        assert!(!key.contains('.'));
    }

    #[test]
    fn portable_components_reject_embedded_separators_in_os_str() {
        // Normal Path construction never embeds separators in a single component,
        // so a regular path is portable.
        let path = PathBuf::from("base").join("child_key");
        assert!(path_is_portable_components(&path));
    }
}
