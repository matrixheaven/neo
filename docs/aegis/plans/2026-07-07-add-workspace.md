# Add Workspace Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use aegis:subagent-driven-development (recommended) or aegis:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `/add-workspace`, a trusted-project-scoped TUI manager for persistent extra filesystem roots that default to enabled read-only access and feed a shared multi-root policy used by Neo's file tools.

**Architecture:** Implement the core filesystem boundary first in `neo-agent-core`, then route all file tools through that shared policy. Add an agent-side persistent `~/.neo/workspaces.json` store keyed by canonical trusted cwd, then wire a `/mcp`-style TUI manager and confirmation flow that updates both the store and the live runtime policy.

**Tech Stack:** Rust 1.96.1, Cargo workspace, `serde_json`, `tempfile`, existing `neo-agent-core` tool/runtime APIs, existing `neo-tui` overlay/dialog patterns, existing `neo-agent` interactive controller/slash command modules.

---

## Scope Check

The spec spans three layers, but they are not independently useful features: the TUI is unsafe without the core policy, and the policy is not user-controllable without persistence/UI. Keep this as one implementation plan with independently testable tasks ordered from core policy to UI.

Git mutation note: the active Neo project instructions forbid git mutations without explicit per-instance authorization. This plan intentionally omits `git add` and `git commit` steps. When executing, do not commit unless the user explicitly authorizes that specific git mutation.

## File Structure

- Create `crates/neo-agent-core/src/workspace_policy.rs`: pure multi-root path policy, symlink-safe read/write resolution, and focused unit tests.
- Modify `crates/neo-agent-core/src/lib.rs`: export the workspace policy types.
- Modify `crates/neo-agent-core/src/runtime/config.rs`: carry a shared live workspace policy in `AgentConfig`.
- Modify `crates/neo-agent-core/src/runtime/tool_dispatch.rs`: inject the live policy into every `ToolContext`.
- Modify `crates/neo-agent-core/src/tools/mod.rs`: store the policy in `ToolContext`, expose read/write resolvers, and keep plan-mode external write paths separate.
- Modify `crates/neo-agent-core/src/tools/{read,list,find,grep,glob,edit,write}.rs`: route all file paths through the policy.
- Create `crates/neo-agent/src/workspaces.rs`: persistent `~/.neo/workspaces.json` store, validation, mutation helpers, and conversion to core policy roots.
- Modify `crates/neo-agent/src/main.rs`: add the `workspaces` module.
- Modify `crates/neo-agent/src/modes/interactive/mod.rs`: add controller fields for the store and live policy.
- Modify `crates/neo-agent/src/modes/interactive/controller_factory.rs`: initialize the workspace store and load policy for the active cwd.
- Modify `crates/neo-agent/src/modes/interactive/turn.rs`: pass the live workspace policy into turn requests.
- Create `crates/neo-agent/src/modes/interactive/workspace_manager.rs`: controller glue for opening `/add-workspace`, validating paths, handling actions, showing confirmations, saving mutations, and refreshing policy.
- Modify `crates/neo-agent/src/modes/interactive/slash_commands.rs`: route `/add-workspace`.
- Modify `crates/neo-agent/src/modes/interactive/command_palette.rs`: add command palette entry and dispatch.
- Modify `crates/neo-agent/src/modes/interactive/prompt_completion.rs`: add slash completion item.
- Create `crates/neo-tui/src/dialogs/workspace_manager.rs`: workspace manager dialog state, rows, actions, rendering, and input handling.
- Create `crates/neo-tui/src/dialogs/confirm_dialog.rs`: reusable warning confirmation dialog for workspace mutations.
- Modify `crates/neo-tui/src/dialogs/mod.rs`: export workspace manager and confirm dialog types.
- Modify `crates/neo-tui/src/shell/overlay.rs`: add overlay variants and rendering.
- Modify `crates/neo-tui/src/shell/dialog_factory.rs`: add `open_workspace_manager` and `open_confirm_dialog`.
- Modify `crates/neo-tui/src/shell/dialog_dispatch.rs`: route input to the new dialogs.
- Modify `crates/neo-tui/src/shell/input_dispatch.rs`: expose workspace manager and confirm dialog actions/results.
- Modify `crates/neo-agent/src/modes/interactive/dialog_results.rs`: process workspace manager, text input, and confirmation results in the correct order.
- Modify `crates/neo-agent/src/modes/interactive/tests.rs`: add slash/controller integration tests.
- Test existing or new TUI dialog unit tests in `crates/neo-tui/src/dialogs/workspace_manager.rs` and `crates/neo-tui/src/dialogs/confirm_dialog.rs`.

## Task 1: Core Workspace Policy

**Files:**
- Create: `crates/neo-agent-core/src/workspace_policy.rs`
- Modify: `crates/neo-agent-core/src/lib.rs`

- [ ] **Step 1: Write failing policy tests**

Create `crates/neo-agent-core/src/workspace_policy.rs` with the tests first. Use this exact test module and empty public type stubs above it so the file compiles far enough to fail behaviorally:

```rust
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAccessRootKind {
    Primary,
    Added,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAccessRoot {
    pub path: PathBuf,
    pub kind: WorkspaceAccessRootKind,
    pub read: bool,
    pub write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAccessPolicy {
    roots: Vec<WorkspaceAccessRoot>,
}

#[derive(Debug, Error)]
pub enum WorkspaceAccessError {
    #[error("path is outside workspace: {path}")]
    PathOutsideWorkspace { path: PathBuf },
    #[error("path is not readable: {path}")]
    ReadDenied { path: PathBuf },
    #[error("path is not writable: {path}")]
    WriteDenied { path: PathBuf },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl WorkspaceAccessPolicy {
    pub fn new(primary_root: impl AsRef<Path>) -> Result<Self, WorkspaceAccessError> {
        let primary = primary_root.as_ref().canonicalize()?;
        Ok(Self {
            roots: vec![WorkspaceAccessRoot {
                path: primary,
                kind: WorkspaceAccessRootKind::Primary,
                read: true,
                write: true,
            }],
        })
    }

    pub fn with_roots(
        primary_root: impl AsRef<Path>,
        roots: impl IntoIterator<Item = WorkspaceAccessRoot>,
    ) -> Result<Self, WorkspaceAccessError> {
        let mut policy = Self::new(primary_root)?;
        policy.roots.extend(roots);
        Ok(policy)
    }

    pub fn roots(&self) -> &[WorkspaceAccessRoot] {
        &self.roots
    }

    pub fn resolve_read_path(&self, path: &Path) -> Result<PathBuf, WorkspaceAccessError> {
        let _ = path;
        Err(WorkspaceAccessError::PathOutsideWorkspace {
            path: PathBuf::new(),
        })
    }

    pub fn resolve_write_path(&self, path: &Path) -> Result<PathBuf, WorkspaceAccessError> {
        let _ = path;
        Err(WorkspaceAccessError::PathOutsideWorkspace {
            path: PathBuf::new(),
        })
    }

    pub fn display_path(&self, path: &Path) -> String {
        path.display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added_root(path: PathBuf, read: bool, write: bool) -> WorkspaceAccessRoot {
        WorkspaceAccessRoot {
            path,
            kind: WorkspaceAccessRootKind::Added,
            read,
            write,
        }
    }

    #[test]
    fn read_allows_primary_relative_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("src.txt");
        std::fs::write(&file, "hello").expect("write");
        let policy = WorkspaceAccessPolicy::new(dir.path()).expect("policy");

        let resolved = policy
            .resolve_read_path(Path::new("src.txt"))
            .expect("resolve");

        assert_eq!(resolved, file.canonicalize().expect("canonical file"));
    }

    #[test]
    fn read_allows_absolute_path_inside_added_read_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let file = added.path().join("lib.rs");
        std::fs::write(&file, "mod lib;").expect("write");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(
                added.path().canonicalize().expect("canonical added"),
                true,
                false,
            )],
        )
        .expect("policy");

        let resolved = policy.resolve_read_path(&file).expect("resolve");

        assert_eq!(resolved, file.canonicalize().expect("canonical file"));
    }

    #[test]
    fn read_denies_absolute_path_inside_read_disabled_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let file = added.path().join("lib.rs");
        std::fs::write(&file, "mod lib;").expect("write");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(
                added.path().canonicalize().expect("canonical added"),
                false,
                false,
            )],
        )
        .expect("policy");

        let err = policy.resolve_read_path(&file).expect_err("denied");

        assert!(matches!(err, WorkspaceAccessError::ReadDenied { .. }));
    }

    #[test]
    fn write_allows_new_file_inside_added_write_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let path = added.path().join("new.txt");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(
                added.path().canonicalize().expect("canonical added"),
                true,
                true,
            )],
        )
        .expect("policy");

        let resolved = policy.resolve_write_path(&path).expect("resolve");

        assert_eq!(resolved, path);
    }

    #[test]
    fn write_denies_new_file_inside_read_only_added_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let path = added.path().join("new.txt");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(
                added.path().canonicalize().expect("canonical added"),
                true,
                false,
            )],
        )
        .expect("policy");

        let err = policy.resolve_write_path(&path).expect_err("denied");

        assert!(matches!(err, WorkspaceAccessError::WriteDenied { .. }));
    }

    #[test]
    fn read_rejects_symlink_escape() {
        let primary = tempfile::tempdir().expect("primary");
        let outside = tempfile::tempdir().expect("outside");
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "secret").expect("write");
        let link = primary.path().join("link.txt");
        create_symlink(&outside_file, &link);
        let policy = WorkspaceAccessPolicy::new(primary.path()).expect("policy");

        let err = policy.resolve_read_path(&link).expect_err("escape denied");

        assert!(matches!(
            err,
            WorkspaceAccessError::PathOutsideWorkspace { .. }
        ));
    }

    #[cfg(unix)]
    fn create_symlink(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).expect("symlink");
    }

    #[cfg(windows)]
    fn create_symlink(target: &Path, link: &Path) {
        std::os::windows::fs::symlink_file(target, link).expect("symlink");
    }
}
```

- [ ] **Step 2: Run policy test and verify failure**

Run:

```bash
cargo nextest run -p neo-agent-core --lib workspace_policy::tests::read_allows_primary_relative_path
```

Expected: FAIL because `resolve_read_path` returns `PathOutsideWorkspace`.

- [ ] **Step 3: Implement policy resolution**

Replace the stub methods in `crates/neo-agent-core/src/workspace_policy.rs` with this implementation. Keep the tests from Step 1.

```rust
impl WorkspaceAccessPolicy {
    pub fn new(primary_root: impl AsRef<Path>) -> Result<Self, WorkspaceAccessError> {
        let primary = primary_root.as_ref().canonicalize()?;
        Ok(Self {
            roots: vec![WorkspaceAccessRoot {
                path: primary,
                kind: WorkspaceAccessRootKind::Primary,
                read: true,
                write: true,
            }],
        })
    }

    pub fn with_roots(
        primary_root: impl AsRef<Path>,
        roots: impl IntoIterator<Item = WorkspaceAccessRoot>,
    ) -> Result<Self, WorkspaceAccessError> {
        let mut policy = Self::new(primary_root)?;
        policy.roots.extend(roots.into_iter().filter_map(|root| {
            let path = root.path.canonicalize().ok()?;
            path.is_dir().then_some(WorkspaceAccessRoot {
                path,
                kind: root.kind,
                read: root.read,
                write: root.read && root.write,
            })
        }));
        Ok(policy)
    }

    pub fn roots(&self) -> &[WorkspaceAccessRoot] {
        &self.roots
    }

    pub fn primary_root(&self) -> Option<&Path> {
        self.roots
            .iter()
            .find(|root| root.kind == WorkspaceAccessRootKind::Primary)
            .map(|root| root.path.as_path())
    }

    pub fn resolve_read_path(&self, path: &Path) -> Result<PathBuf, WorkspaceAccessError> {
        let candidate = self.absolute_candidate(path);
        let canonical = candidate.canonicalize()?;
        let Some(root) = self.containing_root(&canonical) else {
            return Err(WorkspaceAccessError::PathOutsideWorkspace { path: canonical });
        };
        if root.read {
            Ok(canonical)
        } else {
            Err(WorkspaceAccessError::ReadDenied { path: canonical })
        }
    }

    pub fn resolve_write_path(&self, path: &Path) -> Result<PathBuf, WorkspaceAccessError> {
        let candidate = self.absolute_candidate(path);
        let parent = candidate
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.primary_root().unwrap_or(Path::new(".")).to_path_buf());
        let resolved_parent = parent.canonicalize()?;
        let file_name =
            candidate
                .file_name()
                .ok_or_else(|| WorkspaceAccessError::PathOutsideWorkspace {
                    path: candidate.clone(),
                })?;
        let Some(root) = self.containing_root(&resolved_parent) else {
            return Err(WorkspaceAccessError::PathOutsideWorkspace {
                path: resolved_parent,
            });
        };
        if !root.write {
            return Err(WorkspaceAccessError::WriteDenied {
                path: resolved_parent.join(file_name),
            });
        }
        Ok(resolved_parent.join(file_name))
    }

    pub fn display_path(&self, path: &Path) -> String {
        let normalized = normalize_path(path);
        if let Some(primary) = self.primary_root()
            && let Ok(relative) = normalized.strip_prefix(primary)
            && !relative.as_os_str().is_empty()
        {
            return relative.display().to_string();
        }
        normalized.display().to_string()
    }

    fn absolute_candidate(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.primary_root()
                .unwrap_or(Path::new("."))
                .join(path)
        }
    }

    fn containing_root(&self, canonical_path: &Path) -> Option<&WorkspaceAccessRoot> {
        self.roots
            .iter()
            .filter(|root| canonical_path.starts_with(&root.path))
            .max_by_key(|root| root.path.components().count())
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::Normal(_) => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
        }
    }
    normalized
}
```

- [ ] **Step 4: Export policy types**

In `crates/neo-agent-core/src/lib.rs`, add:

```rust
pub mod workspace_policy;
pub use workspace_policy::{
    WorkspaceAccessError, WorkspaceAccessPolicy, WorkspaceAccessRoot, WorkspaceAccessRootKind,
};
```

Place `pub mod workspace_policy;` with the other module declarations, and place the `pub use` near existing public re-exports.

- [ ] **Step 5: Run policy tests**

Run:

```bash
cargo nextest run -p neo-agent-core --lib workspace_policy::tests::
```

Expected: PASS for all `workspace_policy::tests::*`.

## Task 2: Inject Policy Into ToolContext

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
- Modify: `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`

- [ ] **Step 1: Add a failing ToolContext read policy test**

In the existing test module at the bottom of `crates/neo-agent-core/src/tools/mod.rs`, add:

```rust
#[test]
fn tool_context_resolve_workspace_path_uses_added_read_root() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let file = added.path().join("lib.rs");
    std::fs::write(&file, "pub fn lib() {}").expect("write");
    let policy = crate::WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [crate::WorkspaceAccessRoot {
            path: added.path().canonicalize().expect("canonical added"),
            kind: crate::WorkspaceAccessRootKind::Added,
            read: true,
            write: false,
        }],
    )
    .expect("policy");
    let ctx = ToolContext::new(primary.path())
        .expect("context")
        .with_workspace_policy(policy);

    let resolved = ctx.resolve_workspace_path(&file).expect("resolve");

    assert_eq!(resolved, file.canonicalize().expect("canonical file"));
}
```

- [ ] **Step 2: Run the ToolContext test and verify failure**

Run:

```bash
cargo nextest run -p neo-agent-core --lib tools::tests::tool_context_resolve_workspace_path_uses_added_read_root
```

Expected: FAIL because `with_workspace_policy` does not exist.

- [ ] **Step 3: Add workspace policy fields to AgentConfig**

In `crates/neo-agent-core/src/runtime/config.rs`, add imports:

```rust
use crate::workspace_policy::WorkspaceAccessPolicy;
```

Add this field to `AgentConfig` near `workspace_root`:

```rust
#[serde(skip)]
#[schemars(skip)]
pub workspace_policy: Arc<RwLock<Option<WorkspaceAccessPolicy>>>,
```

In `impl Default for AgentConfig` or the constructor currently initializing fields, initialize it with:

```rust
workspace_policy: Arc::new(RwLock::new(None)),
```

Add a builder method near `with_plan_mode`:

```rust
#[must_use]
pub fn with_workspace_policy(
    mut self,
    workspace_policy: Arc<RwLock<Option<WorkspaceAccessPolicy>>>,
) -> Self {
    self.workspace_policy = workspace_policy;
    self
}
```

- [ ] **Step 4: Add policy support to ToolContext**

In `crates/neo-agent-core/src/tools/mod.rs`, import the policy:

```rust
use crate::{AgentEvent, WorkspaceAccessError, WorkspaceAccessPolicy};
```

Add this field to `ToolContext`:

```rust
workspace_policy: WorkspaceAccessPolicy,
```

In `ToolContext::new`, build the policy after canonicalizing `cwd`:

```rust
let workspace_policy = WorkspaceAccessPolicy::new(&cwd)?;
```

and add it to the returned struct:

```rust
workspace_policy,
```

Add this method:

```rust
#[must_use]
pub fn with_workspace_policy(mut self, workspace_policy: WorkspaceAccessPolicy) -> Self {
    self.workspace_policy = workspace_policy;
    self
}
```

Update `Debug` to include:

```rust
.field("workspace_policy_roots", &self.workspace_policy.roots())
```

Add this helper near the existing workspace resolvers:

```rust
fn map_workspace_error(error: WorkspaceAccessError) -> ToolError {
    match error {
        WorkspaceAccessError::PathOutsideWorkspace { path }
        | WorkspaceAccessError::ReadDenied { path }
        | WorkspaceAccessError::WriteDenied { path } => ToolError::PathOutsideWorkspace { path },
        WorkspaceAccessError::Io(source) => ToolError::Io(source),
    }
}
```

Replace `resolve_workspace_path` with:

```rust
pub fn resolve_workspace_path(&self, path: &Path) -> Result<PathBuf, ToolError> {
    if self.is_allowed_external_write_path(path) {
        return Ok(normalize_path(path));
    }
    self.workspace_policy
        .resolve_read_path(path)
        .map_err(map_workspace_error)
}
```

Replace `resolve_parent_for_write` with:

```rust
pub fn resolve_parent_for_write(&self, path: &Path) -> Result<PathBuf, ToolError> {
    if self.is_allowed_external_write_path(path) {
        return Ok(normalize_path(path));
    }
    self.workspace_policy
        .resolve_write_path(path)
        .map_err(map_workspace_error)
}
```

Leave `allowed_external_write_paths` intact for the plan-file exception.

- [ ] **Step 5: Inject policy in runtime tool dispatch**

In `crates/neo-agent-core/src/runtime/tool_dispatch.rs`, update `default_tool_context` after `ToolContext::new(workspace_root)` is created:

```rust
let configured_policy = config
    .workspace_policy
    .read()
    .ok()
    .and_then(|policy| policy.clone());
```

Inside the first `.map(|context| {` block in `default_tool_context`, before the existing `.with_access(ToolAccess::none())` chain, apply:

```rust
let context = configured_policy
    .clone()
    .map_or(context, |policy| context.with_workspace_policy(policy));
```

Then continue with the existing chained `with_access`, `with_cancel_token`, and child runtime setup.

- [ ] **Step 6: Run ToolContext test**

Run:

```bash
cargo nextest run -p neo-agent-core --lib tools::tests::tool_context_resolve_workspace_path_uses_added_read_root
```

Expected: PASS.

## Task 3: Route File Tools Through Policy

**Files:**
- Modify: `crates/neo-agent-core/src/tools/read.rs`
- Modify: `crates/neo-agent-core/src/tools/list.rs`
- Modify: `crates/neo-agent-core/src/tools/find.rs`
- Modify: `crates/neo-agent-core/src/tools/grep.rs`
- Modify: `crates/neo-agent-core/src/tools/glob.rs`
- Modify: `crates/neo-agent-core/src/tools/edit.rs`
- Modify: `crates/neo-agent-core/src/tools/write.rs`

- [ ] **Step 1: Add failing ReadTool policy tests**

In `crates/neo-agent-core/src/tools/read.rs`, add these tests to the existing test module or create one at the bottom:

```rust
#[cfg(test)]
mod workspace_policy_tests {
    use super::*;
    use crate::{ToolAccess, ToolContext, WorkspaceAccessPolicy, WorkspaceAccessRoot, WorkspaceAccessRootKind};
    use serde_json::json;

    #[tokio::test]
    async fn read_allows_added_read_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let file = added.path().join("lib.rs");
        std::fs::write(&file, "pub fn lib() {}\n").expect("write");
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

        let result = ReadTool
            .execute(&ctx, json!({ "path": file }))
            .await
            .expect("read");

        assert!(!result.is_error, "unexpected read error: {}", result.content);
        assert!(result.content.contains("pub fn lib()"));
    }

    #[tokio::test]
    async fn read_denies_path_outside_all_roots() {
        let primary = tempfile::tempdir().expect("primary");
        let outside = tempfile::tempdir().expect("outside");
        let file = outside.path().join("secret.txt");
        std::fs::write(&file, "secret\n").expect("write");
        let ctx = ToolContext::new(primary.path())
            .expect("context")
            .with_access(ToolAccess::all());

        let err = ReadTool
            .execute(&ctx, json!({ "path": file }))
            .await
            .expect_err("outside denied");

        assert!(matches!(err, crate::tools::ToolError::PathOutsideWorkspace { .. }));
    }
}
```

- [ ] **Step 2: Run ReadTool outside-root test and verify failure**

Run:

```bash
cargo nextest run -p neo-agent-core --lib tools::read::workspace_policy_tests::read_denies_path_outside_all_roots
```

Expected before implementation: FAIL because `ReadTool` currently bypasses the workspace policy for absolute paths.

- [ ] **Step 3: Update ReadTool path resolution**

In `crates/neo-agent-core/src/tools/read.rs`, remove `resolve_read_path` and update execution to:

```rust
let path = ctx.resolve_workspace_path(&input.path)?;
```

Keep the rest of `run_read` unchanged.

- [ ] **Step 4: Update WriteTool to use write resolver consistently**

In `crates/neo-agent-core/src/tools/write.rs`, keep:

```rust
let path = ctx.resolve_parent_for_write(&input.path)?;
```

No functional edit is needed if it already matches, but add a test in the file:

```rust
#[cfg(test)]
mod workspace_policy_tests {
    use super::*;
    use crate::{ToolAccess, ToolContext, WorkspaceAccessPolicy, WorkspaceAccessRoot, WorkspaceAccessRootKind};
    use serde_json::json;

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

        let err = WriteTool
            .execute(&ctx, json!({ "path": path, "content": "hello" }))
            .await
            .expect_err("write denied");

        assert!(matches!(err, crate::tools::ToolError::PathOutsideWorkspace { .. }));
    }
}
```

- [ ] **Step 5: Audit other file tools**

Confirm these lines exist:

```rust
// crates/neo-agent-core/src/tools/list.rs
let path = ctx.resolve_workspace_path(&input.path)?;

// crates/neo-agent-core/src/tools/find.rs
let root = ctx.resolve_workspace_path(&input.path)?;

// crates/neo-agent-core/src/tools/grep.rs
let root = ctx.resolve_workspace_path(&input.path)?;

// crates/neo-agent-core/src/tools/glob.rs
let walk_root = ctx.resolve_workspace_path(&input.path)?;

// crates/neo-agent-core/src/tools/edit.rs
let path = ctx.resolve_workspace_path(&input.path)?;
```

Use `rg -n "canonicalize|join\\(&input\\.path\\)|resolve_workspace_path|resolve_parent_for_write" crates/neo-agent-core/src/tools/{list,find,grep,glob,edit,write}.rs` to inspect these six tools. For every file in the list above, the resolver line shown above must be the only path-boundary decision before file walking, reading, or writing.

- [ ] **Step 6: Run narrow file-tool tests**

Run:

```bash
cargo nextest run -p neo-agent-core --lib tools::read::workspace_policy_tests::
cargo nextest run -p neo-agent-core --lib tools::write::workspace_policy_tests::
cargo nextest run -p neo-agent-core --lib workspace_policy::tests::
```

Expected: PASS for all listed filters.

## Task 4: Persistent Workspace Store

**Files:**
- Create: `crates/neo-agent/src/workspaces.rs`
- Modify: `crates/neo-agent/src/main.rs`

- [ ] **Step 1: Write store tests and type skeleton**

Create `crates/neo-agent/src/workspaces.rs` with this skeleton and tests:

```rust
use std::{
    collections::BTreeMap,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, bail};
use neo_agent_core::{WorkspaceAccessRoot, WorkspaceAccessRootKind};
use serde::{Deserialize, Serialize};

const WORKSPACES_FILE: &str = "workspaces.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkspaceStoreData {
    pub schema_version: u32,
    #[serde(default)]
    pub projects: BTreeMap<String, WorkspaceProject>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct WorkspaceProject {
    #[serde(default)]
    pub entries: Vec<WorkspaceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkspaceEntry {
    pub path: PathBuf,
    pub enabled: bool,
    pub read: bool,
    pub write: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceStore {
    path: PathBuf,
}

impl Default for WorkspaceStoreData {
    fn default() -> Self {
        Self {
            schema_version: 1,
            projects: BTreeMap::new(),
        }
    }
}

impl WorkspaceEntry {
    pub(crate) fn read_only(path: PathBuf) -> Self {
        Self {
            path,
            enabled: true,
            read: true,
            write: false,
        }
    }
}

impl WorkspaceStore {
    pub(crate) fn from_home() -> anyhow::Result<Self> {
        let home = crate::config::neo_home()
            .context("NEO_HOME or platform home directory is required to resolve workspace store")?;
        Ok(Self {
            path: home.join(WORKSPACES_FILE),
        })
    }

    #[cfg(test)]
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn read_project(&self, project_dir: &Path) -> anyhow::Result<WorkspaceProject> {
        let data = self.read()?;
        Ok(data
            .projects
            .get(&project_key(project_dir)?)
            .cloned()
            .unwrap_or_default())
    }

    pub(crate) fn write_project(
        &self,
        project_dir: &Path,
        project: WorkspaceProject,
    ) -> anyhow::Result<()> {
        let mut data = self.read()?;
        data.projects.insert(project_key(project_dir)?, project);
        self.write(&data)
    }

    fn read(&self) -> anyhow::Result<WorkspaceStoreData> {
        if !self.path.exists() {
            return Ok(WorkspaceStoreData::default());
        }
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read workspace store {}", self.path.display()))?;
        if content.trim().is_empty() {
            return Ok(WorkspaceStoreData::default());
        }
        match serde_json::from_str::<WorkspaceStoreData>(&content) {
            Ok(data) => Ok(data),
            Err(err) => {
                let backup = self.path.with_extension("json.bak");
                if backup.exists() {
                    fs::remove_file(&backup).with_context(|| {
                        format!("failed to remove old workspace store backup {}", backup.display())
                    })?;
                }
                fs::rename(&self.path, &backup).with_context(|| {
                    format!(
                        "failed to back up corrupted workspace store to {}",
                        backup.display()
                    )
                })?;
                tracing::warn!(
                    "workspace store {} was corrupted ({err}); backed up to {}. Starting fresh.",
                    self.path.display(),
                    backup.display()
                );
                Ok(WorkspaceStoreData::default())
            }
        }
    }

    fn write(&self, data: &WorkspaceStoreData) -> anyhow::Result<()> {
        let parent = self.path.parent().context("workspace store has no parent")?;
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create workspace store directory {}",
                parent.display()
            )
        })?;
        let content = serde_json::to_string_pretty(data).context("serialize workspace store")?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)
            .context("create temporary workspace store file")?;
        temp.write_all(content.as_bytes())
            .context("write temporary workspace store file")?;
        temp.persist(&self.path)
            .map_err(|err| anyhow::anyhow!("failed to persist workspace store: {err}"))?;
        Ok(())
    }
}

pub(crate) fn validate_new_workspace_entry(
    project_dir: &Path,
    project: &WorkspaceProject,
    path: &Path,
) -> anyhow::Result<WorkspaceEntry> {
    let canonical_project = project_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project directory {}",
            project_dir.display()
        )
    })?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize workspace path {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("Path is not a directory");
    }
    if canonical == canonical_project || canonical.starts_with(&canonical_project) {
        bail!("Directory is already inside the primary workspace");
    }
    if project.entries.iter().any(|entry| entry.path == canonical) {
        bail!("Directory is already configured");
    }
    if project
        .entries
        .iter()
        .any(|entry| entry.path.starts_with(&canonical) || canonical.starts_with(&entry.path))
    {
        bail!("Directory overlaps another added workspace");
    }
    Ok(WorkspaceEntry::read_only(canonical))
}

pub(crate) fn access_roots_from_project(project: &WorkspaceProject) -> Vec<WorkspaceAccessRoot> {
    project
        .entries
        .iter()
        .filter(|entry| entry.enabled && entry.path.is_dir() && (entry.read || entry.write))
        .map(|entry| WorkspaceAccessRoot {
            path: entry.path.clone(),
            kind: WorkspaceAccessRootKind::Added,
            read: entry.read,
            write: entry.write && entry.read,
        })
        .collect()
}

fn project_key(project_dir: &Path) -> anyhow::Result<String> {
    let canonical = project_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project dir {}",
            project_dir.display()
        )
    })?;
    let Some(key) = canonical.to_str() else {
        bail!("project dir is not valid UTF-8: {}", canonical.display());
    };
    Ok(key.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_writes_project_entries_under_canonical_key() {
        let root = tempfile::tempdir().expect("root");
        let project = root.path().join("project");
        let added = root.path().join("added");
        fs::create_dir_all(&project).expect("project");
        fs::create_dir_all(&added).expect("added");
        let store = WorkspaceStore::new(root.path().join("workspaces.json"));
        let entry = WorkspaceEntry::read_only(added.canonicalize().expect("canonical added"));

        store
            .write_project(
                &project,
                WorkspaceProject {
                    entries: vec![entry.clone()],
                },
            )
            .expect("write project");

        let loaded = store.read_project(&project).expect("read project");
        assert_eq!(loaded.entries, vec![entry]);
    }

    #[test]
    fn new_entry_defaults_to_enabled_read_only() {
        let root = tempfile::tempdir().expect("root");
        let project = root.path().join("project");
        let added = root.path().join("added");
        fs::create_dir_all(&project).expect("project");
        fs::create_dir_all(&added).expect("added");

        let entry = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &added)
            .expect("entry");

        assert!(entry.enabled);
        assert!(entry.read);
        assert!(!entry.write);
    }

    #[test]
    fn validation_rejects_directory_inside_primary_workspace() {
        let root = tempfile::tempdir().expect("root");
        let project = root.path().join("project");
        let nested = project.join("nested");
        fs::create_dir_all(&nested).expect("nested");

        let err = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &nested)
            .expect_err("reject nested");

        assert!(err.to_string().contains("primary workspace"));
    }

    #[test]
    fn access_roots_skip_disabled_entries() {
        let root = tempfile::tempdir().expect("root");
        let added = root.path().join("added");
        fs::create_dir_all(&added).expect("added");
        let mut entry = WorkspaceEntry::read_only(added.canonicalize().expect("canonical added"));
        entry.enabled = false;

        let roots = access_roots_from_project(&WorkspaceProject {
            entries: vec![entry],
        });

        assert!(roots.is_empty());
    }

    #[test]
    fn corrupted_store_is_backed_up_and_treated_as_empty() {
        let root = tempfile::tempdir().expect("root");
        let path = root.path().join("workspaces.json");
        fs::write(&path, "not json").expect("write corrupted");
        let project = root.path().join("project");
        fs::create_dir_all(&project).expect("project");
        let store = WorkspaceStore::new(path.clone());

        let loaded = store.read_project(&project).expect("read after corruption");

        assert!(loaded.entries.is_empty());
        assert!(path.with_extension("json.bak").exists());
    }
}
```

- [ ] **Step 2: Add module declaration**

In `crates/neo-agent/src/main.rs`, add:

```rust
mod workspaces;
```

Place it near `mod trust;`.

- [ ] **Step 3: Run workspace store tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- workspaces::tests::new_entry_defaults_to_enabled_read_only --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- workspaces::tests::store_writes_project_entries_under_canonical_key --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- workspaces::tests::corrupted_store_is_backed_up_and_treated_as_empty --exact --nocapture --include-ignored
```

Expected: PASS for each exact test.

## Task 5: TUI Workspace Manager Dialog

**Files:**
- Create: `crates/neo-tui/src/dialogs/workspace_manager.rs`
- Modify: `crates/neo-tui/src/dialogs/mod.rs`
- Modify: `crates/neo-tui/src/shell/overlay.rs`
- Modify: `crates/neo-tui/src/shell/dialog_factory.rs`
- Modify: `crates/neo-tui/src/shell/dialog_dispatch.rs`
- Modify: `crates/neo-tui/src/shell/input_dispatch.rs`

- [ ] **Step 1: Create dialog state with tests**

Create `crates/neo-tui/src/dialogs/workspace_manager.rs` with:

```rust
use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{InputResult, Style, paint, truncate_width};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub path: String,
    pub enabled: bool,
    pub read: bool,
    pub write: bool,
    pub missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceManagerOptions {
    pub trusted: bool,
    pub rows: Vec<WorkspaceRow>,
    pub theme: TuiTheme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceManagerAction {
    Add,
    ToggleEnabled(String),
    ToggleRead(String),
    ToggleWrite(String),
    Delete(String),
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Workspace(WorkspaceRow),
    Add,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceManagerState {
    trusted: bool,
    rows: Vec<Row>,
    selected_index: usize,
    theme: TuiTheme,
    action: Option<WorkspaceManagerAction>,
}

impl WorkspaceManagerState {
    #[must_use]
    pub fn new(opts: &WorkspaceManagerOptions) -> Self {
        Self {
            trusted: opts.trusted,
            rows: build_rows(opts),
            selected_index: 0,
            theme: opts.theme,
            action: None,
        }
    }

    pub fn set_options(&mut self, opts: &WorkspaceManagerOptions) {
        let previous_path = self.selected_path();
        self.trusted = opts.trusted;
        self.rows = build_rows(opts);
        self.theme = opts.theme;
        self.action = None;
        self.selected_index = previous_path
            .and_then(|path| {
                self.rows
                    .iter()
                    .position(|row| row.path().as_deref() == Some(path.as_str()))
            })
            .unwrap_or_else(|| self.selected_index.min(self.rows.len().saturating_sub(1)));
    }

    #[must_use]
    pub fn action(&self) -> Option<&WorkspaceManagerAction> {
        self.action.as_ref()
    }

    pub fn take_action(&mut self) -> Option<WorkspaceManagerAction> {
        self.action.take()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        if width < 4 {
            return Vec::new();
        }
        let inner = width.saturating_sub(2).max(1);
        let border = Style::default().fg(self.theme.overlay_border);
        let title = Style::default().fg(self.theme.text_primary).bold();
        let hint = Style::default().fg(self.theme.text_muted);
        let mut lines = vec![
            paint(&format!("┌{}┐", "─".repeat(inner)), border),
            box_line(" Workspace Access", inner, title, border),
        ];
        if !self.trusted {
            lines.push(box_line(" Esc close", inner, hint, border));
            lines.push(box_line("", inner, Style::default(), border));
            lines.push(box_line("  This project is not trusted.", inner, hint, border));
            lines.push(box_line("", inner, Style::default(), border));
            lines.push(box_line(
                "  Additional workspace directories can expose files outside this cwd.",
                inner,
                hint,
                border,
            ));
            lines.push(box_line(
                "  Trust this workspace before managing extra filesystem access.",
                inner,
                hint,
                border,
            ));
            lines.push(box_line("", inner, Style::default(), border));
            lines.push(paint(&format!("└{}┘", "─".repeat(inner)), border));
            return lines;
        }

        let hint_text = if inner >= 76 {
            " ↑↓ · A add · E on/off · R read on/off · W write on/off · D delete · Esc"
        } else {
            " ↑↓ · A add · E on/off · R read · W write · D delete · Esc"
        };
        lines.push(box_line(hint_text, inner, hint, border));
        lines.push(box_line("", inner, Style::default(), border));
        if self.rows.iter().all(|row| matches!(row, Row::Add)) {
            lines.push(box_line(
                "  No additional workspaces configured.",
                inner,
                hint,
                border,
            ));
            lines.push(box_line(
                "  Added directories become available to file tools for this trusted cwd.",
                inner,
                hint,
                border,
            ));
            lines.push(box_line("", inner, Style::default(), border));
        }
        for (index, row) in self.rows.iter().enumerate() {
            let selected = index == self.selected_index;
            for rendered in render_row(row, selected, inner) {
                lines.push(box_line(&rendered, inner, Style::default(), border));
            }
        }
        lines.push(box_line("", inner, Style::default(), border));
        lines.push(paint(&format!("└{}┘", "─".repeat(inner)), border));
        lines
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.action.is_some() {
            return InputResult::Handled;
        }
        match input {
            InputEvent::Action(KeybindingAction::SelectUp) => {
                self.move_up();
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectDown) => {
                self.move_down();
                InputResult::Handled
            }
            InputEvent::Submit => self.add_or_ignore(),
            InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => {
                self.action = Some(WorkspaceManagerAction::Close);
                InputResult::Cancelled
            }
            InputEvent::Insert(ch) => self.handle_key(*ch),
            _ => InputResult::Ignored,
        }
    }

    fn handle_key(&mut self, ch: char) -> InputResult {
        match ch.to_ascii_lowercase() {
            'a' => {
                self.action = Some(WorkspaceManagerAction::Add);
                InputResult::Submitted
            }
            'e' => self.action_for_selected(WorkspaceManagerAction::ToggleEnabled),
            'r' => self.action_for_selected(WorkspaceManagerAction::ToggleRead),
            'w' => self.action_for_selected(WorkspaceManagerAction::ToggleWrite),
            'd' => self.action_for_selected(WorkspaceManagerAction::Delete),
            _ => InputResult::Ignored,
        }
    }

    fn add_or_ignore(&mut self) -> InputResult {
        if matches!(self.rows.get(self.selected_index), Some(Row::Add)) {
            self.action = Some(WorkspaceManagerAction::Add);
            InputResult::Submitted
        } else {
            InputResult::Handled
        }
    }

    fn action_for_selected(
        &mut self,
        build: impl FnOnce(String) -> WorkspaceManagerAction,
    ) -> InputResult {
        let Some(path) = self.selected_path() else {
            return InputResult::Handled;
        };
        self.action = Some(build(path));
        InputResult::Submitted
    }

    fn selected_path(&self) -> Option<String> {
        self.rows
            .get(self.selected_index)
            .and_then(Row::path)
            .map(str::to_owned)
    }

    fn move_up(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected_index = if self.selected_index == 0 {
            self.rows.len() - 1
        } else {
            self.selected_index - 1
        };
    }

    fn move_down(&mut self) {
        if !self.rows.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.rows.len();
        }
    }
}

impl Row {
    fn path(&self) -> Option<&str> {
        match self {
            Self::Workspace(row) => Some(&row.path),
            Self::Add => None,
        }
    }
}

fn build_rows(opts: &WorkspaceManagerOptions) -> Vec<Row> {
    if !opts.trusted {
        return Vec::new();
    }
    let mut rows: Vec<Row> = opts.rows.iter().cloned().map(Row::Workspace).collect();
    rows.push(Row::Add);
    rows
}

fn render_row(row: &Row, selected: bool, inner: usize) -> Vec<String> {
    let marker = if selected { "▸" } else { " " };
    match row {
        Row::Add => vec![truncate_width(
            &format!("{marker} + Add workspace directory"),
            inner,
            "",
            false,
        )],
        Row::Workspace(row) => {
            let enabled = if row.enabled { "[on ]" } else { "[off]" };
            let read = if row.read { "[R ]" } else { "[R-]" };
            let write = if row.write { "[W ]" } else { "[W-]" };
            let summary = if row.missing {
                "missing directory · ignored".to_owned()
            } else if !row.enabled {
                "disabled".to_owned()
            } else if row.write {
                "read/write · active".to_owned()
            } else if row.read {
                "read-only · active".to_owned()
            } else {
                "no file access · inactive".to_owned()
            };
            vec![
                truncate_width(
                    &format!("{marker} {enabled} {read} {write} {}", row.path),
                    inner,
                    "",
                    false,
                ),
                truncate_width(&format!("      {summary}"), inner, "", false),
            ]
        }
    }
}

fn box_line(text: &str, inner: usize, style: Style, border_style: Style) -> String {
    let content = truncate_width(text, inner, "", false);
    let padding = inner.saturating_sub(crate::primitive::visible_width(&content));
    format!(
        "{}{}{}{}",
        paint("│", border_style),
        paint(&content, style),
        " ".repeat(padding),
        paint("│", border_style)
    )
}
```

Add tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> TuiTheme {
        TuiTheme::default()
    }

    fn row(path: &str, enabled: bool, read: bool, write: bool, missing: bool) -> WorkspaceRow {
        WorkspaceRow {
            path: path.to_owned(),
            enabled,
            read,
            write,
            missing,
        }
    }

    fn manager(rows: Vec<WorkspaceRow>) -> WorkspaceManagerState {
        WorkspaceManagerState::new(&WorkspaceManagerOptions {
            trusted: true,
            rows,
            theme: theme(),
        })
    }

    #[test]
    fn renders_empty_state() {
        let state = manager(Vec::new());
        let rendered = state.render_lines(88).join("\n");
        assert!(rendered.contains("No additional workspaces configured."));
        assert!(rendered.contains("+ Add workspace directory"));
    }

    #[test]
    fn renders_list_state_with_access_flags() {
        let state = manager(vec![row("/tmp/shared", true, true, false, false)]);
        let rendered = state.render_lines(88).join("\n");
        assert!(rendered.contains("[on ] [R ] [W-] /tmp/shared"));
        assert!(rendered.contains("read-only"));
    }

    #[test]
    fn renders_untrusted_warning() {
        let state = WorkspaceManagerState::new(&WorkspaceManagerOptions {
            trusted: false,
            rows: Vec::new(),
            theme: theme(),
        });
        let rendered = state.render_lines(88).join("\n");
        assert!(rendered.contains("This project is not trusted."));
        assert!(!rendered.contains("+ Add workspace directory"));
    }

    #[test]
    fn key_e_toggles_enabled_for_selected_row() {
        let mut state = manager(vec![row("/tmp/shared", true, true, false, false)]);
        let result = state.handle_input(&InputEvent::Insert('E'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(
            state.action(),
            Some(WorkspaceManagerAction::ToggleEnabled(path)) if path == "/tmp/shared"
        ));
    }

    #[test]
    fn key_r_toggles_read_for_selected_row() {
        let mut state = manager(vec![row("/tmp/shared", true, true, false, false)]);
        let result = state.handle_input(&InputEvent::Insert('R'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(
            state.action(),
            Some(WorkspaceManagerAction::ToggleRead(path)) if path == "/tmp/shared"
        ));
    }

    #[test]
    fn key_w_toggles_write_for_selected_row() {
        let mut state = manager(vec![row("/tmp/shared", true, true, false, false)]);
        let result = state.handle_input(&InputEvent::Insert('W'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(
            state.action(),
            Some(WorkspaceManagerAction::ToggleWrite(path)) if path == "/tmp/shared"
        ));
    }
}
```

- [ ] **Step 2: Export dialog types**

In `crates/neo-tui/src/dialogs/mod.rs`, add:

```rust
pub mod workspace_manager;
pub use workspace_manager::{
    WorkspaceManagerAction, WorkspaceManagerOptions, WorkspaceManagerState, WorkspaceRow,
};
```

- [ ] **Step 3: Wire overlay variant and rendering**

In `crates/neo-tui/src/shell/overlay.rs`, import `WorkspaceManagerState`, add an enum variant:

```rust
WorkspaceManager(WorkspaceManagerState),
```

Update `rich_dialog_lines`:

```rust
Self::WorkspaceManager(state) => Some(state.render_lines(width)),
```

Update any dialog matching lists that include `McpManager` so `WorkspaceManager` is treated as a rich dialog.

- [ ] **Step 4: Add shell factory and input accessors**

In `crates/neo-tui/src/shell/dialog_factory.rs`, add:

```rust
pub fn open_workspace_manager(
    &mut self,
    opts: &crate::dialogs::WorkspaceManagerOptions,
) -> OverlayId {
    let existing_id =
        self.find_overlay_by_kind(|kind| matches!(kind, OverlayKind::WorkspaceManager(_)));
    if let Some(id) = existing_id {
        if let Some(overlay) = self.overlays.iter_mut().find(|overlay| overlay.id == id)
            && let OverlayKind::WorkspaceManager(state) = &mut overlay.kind
        {
            state.set_options(opts);
        }
        self.focus_overlay(id);
        return id;
    }
    let state = crate::dialogs::WorkspaceManagerState::new(opts);
    self.push_overlay(Overlay::new(
        "workspace-access",
        OverlayKind::WorkspaceManager(state),
    ))
}
```

In `crates/neo-tui/src/shell/dialog_dispatch.rs`, route input:

```rust
OverlayKind::WorkspaceManager(state) => handle_input_ref(state, input),
```

and implement:

```rust
impl DialogInputRef for WorkspaceManagerState {
    fn handle(&mut self, input: &InputEvent) -> InputResult {
        self.handle_input(input)
    }
}
```

In `crates/neo-tui/src/shell/input_dispatch.rs`, add:

```rust
pub fn workspace_manager_action(&self) -> Option<crate::dialogs::WorkspaceManagerAction> {
    let OverlayKind::WorkspaceManager(state) = &self.focused_overlay()?.kind else {
        return None;
    };
    state.action().cloned()
}

pub fn take_workspace_manager_action(&mut self) -> Option<crate::dialogs::WorkspaceManagerAction> {
    let OverlayKind::WorkspaceManager(state) = &mut self.focused_overlay_mut()?.kind else {
        return None;
    };
    state.take_action()
}
```

- [ ] **Step 5: Run TUI manager tests**

Run:

```bash
cargo nextest run -p neo-tui --lib dialogs::workspace_manager::tests::
```

Expected: PASS for all workspace manager dialog tests.

## Task 6: Reusable Confirmation Dialog

**Files:**
- Create: `crates/neo-tui/src/dialogs/confirm_dialog.rs`
- Modify: `crates/neo-tui/src/dialogs/mod.rs`
- Modify: `crates/neo-tui/src/shell/overlay.rs`
- Modify: `crates/neo-tui/src/shell/dialog_factory.rs`
- Modify: `crates/neo-tui/src/shell/dialog_dispatch.rs`
- Modify: `crates/neo-tui/src/shell/input_dispatch.rs`

- [ ] **Step 1: Create confirm dialog with tests**

Create `crates/neo-tui/src/dialogs/confirm_dialog.rs`:

```rust
use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{InputResult, Style, paint, truncate_width, visible_width};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmDialogOptions {
    pub id: String,
    pub title: String,
    pub hint: String,
    pub lines: Vec<String>,
    pub theme: TuiTheme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmDialogResult {
    Approved { id: String },
    Cancelled { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmDialogState {
    id: String,
    title: String,
    hint: String,
    lines: Vec<String>,
    theme: TuiTheme,
    result: Option<ConfirmDialogResult>,
}

impl ConfirmDialogState {
    #[must_use]
    pub fn new(opts: ConfirmDialogOptions) -> Self {
        Self {
            id: opts.id,
            title: opts.title,
            hint: opts.hint,
            lines: opts.lines,
            theme: opts.theme,
            result: None,
        }
    }

    #[must_use]
    pub const fn result(&self) -> Option<&ConfirmDialogResult> {
        self.result.as_ref()
    }

    pub fn take_result(&mut self) -> Option<ConfirmDialogResult> {
        self.result.take()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        if width < 4 {
            return Vec::new();
        }
        let inner = width.saturating_sub(2).max(1);
        let border = Style::default().fg(self.theme.overlay_border);
        let title = Style::default().fg(self.theme.text_primary).bold();
        let hint = Style::default().fg(self.theme.text_muted);
        let mut lines = vec![
            paint(&format!("┌{}┐", "─".repeat(inner)), border),
            box_line(&format!(" {}", self.title), inner, title, border),
            box_line(&format!(" {}", self.hint), inner, hint, border),
            box_line("", inner, Style::default(), border),
        ];
        for line in &self.lines {
            lines.push(box_line(line, inner, Style::default(), border));
        }
        lines.push(box_line("", inner, Style::default(), border));
        lines.push(paint(&format!("└{}┘", "─".repeat(inner)), border));
        lines
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Handled;
        }
        match input {
            InputEvent::Insert(ch) if matches!(ch.to_ascii_lowercase(), 'y') => {
                self.result = Some(ConfirmDialogResult::Approved {
                    id: self.id.clone(),
                });
                InputResult::Submitted
            }
            InputEvent::Insert(ch) if matches!(ch.to_ascii_lowercase(), 'n') => {
                self.result = Some(ConfirmDialogResult::Cancelled {
                    id: self.id.clone(),
                });
                InputResult::Cancelled
            }
            InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => {
                self.result = Some(ConfirmDialogResult::Cancelled {
                    id: self.id.clone(),
                });
                InputResult::Cancelled
            }
            _ => InputResult::Ignored,
        }
    }
}

fn box_line(text: &str, inner: usize, style: Style, border_style: Style) -> String {
    let content = truncate_width(text, inner, "", false);
    let padding = inner.saturating_sub(visible_width(&content));
    format!(
        "{}{}{}{}",
        paint("│", border_style),
        paint(&content, style),
        " ".repeat(padding),
        paint("│", border_style)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ConfirmDialogState {
        ConfirmDialogState::new(ConfirmDialogOptions {
            id: "toggle-write:/tmp/shared".to_owned(),
            title: "Confirm Write Access".to_owned(),
            hint: "Y approve · N cancel · Esc cancel".to_owned(),
            lines: vec![
                " Enable write access for this directory?".to_owned(),
                " /tmp/shared".to_owned(),
            ],
            theme: TuiTheme::default(),
        })
    }

    #[test]
    fn renders_title_hint_and_body() {
        let rendered = state().render_lines(80).join("\n");
        assert!(rendered.contains("Confirm Write Access"));
        assert!(rendered.contains("Y approve"));
        assert!(rendered.contains("/tmp/shared"));
    }

    #[test]
    fn y_approves() {
        let mut state = state();
        let result = state.handle_input(&InputEvent::Insert('Y'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(
            state.result(),
            Some(ConfirmDialogResult::Approved { id }) if id == "toggle-write:/tmp/shared"
        ));
    }

    #[test]
    fn n_cancels() {
        let mut state = state();
        let result = state.handle_input(&InputEvent::Insert('N'));
        assert!(matches!(result, InputResult::Cancelled));
        assert!(matches!(
            state.result(),
            Some(ConfirmDialogResult::Cancelled { id }) if id == "toggle-write:/tmp/shared"
        ));
    }
}
```

- [ ] **Step 2: Export and wire overlay**

In `crates/neo-tui/src/dialogs/mod.rs`, add:

```rust
pub mod confirm_dialog;
pub use confirm_dialog::{ConfirmDialogOptions, ConfirmDialogResult, ConfirmDialogState};
```

In `crates/neo-tui/src/shell/overlay.rs`, add `ConfirmDialogState` to imports and `OverlayKind::ConfirmDialog(ConfirmDialogState)`. Render it in `input_dialog_lines`:

```rust
Self::ConfirmDialog(state) => Some(state.render_lines(width)),
```

In `crates/neo-tui/src/shell/dialog_factory.rs`, add:

```rust
pub fn open_confirm_dialog(&mut self, opts: crate::dialogs::ConfirmDialogOptions) -> OverlayId {
    let state = crate::dialogs::ConfirmDialogState::new(opts);
    self.push_overlay(Overlay::new("confirm", OverlayKind::ConfirmDialog(state)))
}
```

In `dialog_dispatch.rs`, route `OverlayKind::ConfirmDialog(state)` through `handle_input_ref`.

In `input_dispatch.rs`, add:

```rust
pub fn confirm_dialog_result(&self) -> Option<&crate::dialogs::ConfirmDialogResult> {
    let OverlayKind::ConfirmDialog(state) = &self.focused_overlay()?.kind else {
        return None;
    };
    state.result()
}

pub fn take_confirm_dialog_result(&mut self) -> Option<crate::dialogs::ConfirmDialogResult> {
    let OverlayKind::ConfirmDialog(state) = &mut self.focused_overlay_mut()?.kind else {
        return None;
    };
    state.take_result()
}
```

- [ ] **Step 3: Run confirm dialog tests**

Run:

```bash
cargo nextest run -p neo-tui --lib dialogs::confirm_dialog::tests::
```

Expected: PASS for all confirm dialog tests.

## Task 7: Interactive Controller Workspace Manager

**Files:**
- Create: `crates/neo-agent/src/modes/interactive/workspace_manager.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/controller_factory.rs`
- Modify: `crates/neo-agent/src/modes/interactive/turn.rs`
- Modify: `crates/neo-agent/src/modes/interactive/dialog_results.rs`

- [ ] **Step 1: Add controller fields and module declaration**

In `crates/neo-agent/src/modes/interactive/mod.rs`, add:

```rust
mod workspace_manager;
```

Add fields to `InteractiveController`:

```rust
workspace_store: Option<crate::workspaces::WorkspaceStore>,
workspace_policy: Arc<RwLock<Option<neo_agent_core::WorkspaceAccessPolicy>>>,
pending_workspace_mutation: Option<workspace_manager::PendingWorkspaceMutation>,
```

Initialize them in controller constructors:

```rust
workspace_store: None,
workspace_policy: Arc::new(RwLock::new(None)),
pending_workspace_mutation: None,
```

- [ ] **Step 2: Pass policy into turns**

In `crates/neo-agent/src/modes/interactive/turn.rs`, when building the turn request, add:

```rust
request.workspace_policy = Arc::clone(&self.workspace_policy);
```

If `TurnRequest` does not have that field yet, add it to the local request struct and apply it to `AgentConfig` with:

```rust
config.workspace_policy = Arc::clone(&request.workspace_policy);
```

- [ ] **Step 3: Initialize store in controller factory**

In `crates/neo-agent/src/modes/interactive/controller_factory.rs`, find the existing assignment to `controller.trust_store`. Immediately after that assignment, add:

```rust
controller.workspace_store = crate::workspaces::WorkspaceStore::from_home().ok();
controller.refresh_workspace_policy_from_store();
```

- [ ] **Step 4: Create controller workspace manager module**

Create `crates/neo-agent/src/modes/interactive/workspace_manager.rs`:

```rust
use std::path::{Path, PathBuf};

use neo_agent_core::WorkspaceAccessPolicy;
use neo_tui::dialogs::{
    ConfirmDialogOptions, ConfirmDialogResult, TextInputOptions, TextInputResult,
    WorkspaceManagerAction, WorkspaceManagerOptions, WorkspaceRow,
};

use crate::trust::ProjectTrustState;

use super::InteractiveController;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PendingWorkspaceMutation {
    Add { path: PathBuf },
    ToggleEnabled { path: PathBuf },
    ToggleRead { path: PathBuf },
    ToggleWrite { path: PathBuf },
    Delete { path: PathBuf },
}

impl InteractiveController {
    pub(super) fn open_workspace_manager(&mut self) {
        let theme = self.tui.chrome().theme();
        let trusted = self.workspace_access_trusted();
        let rows = if trusted {
            self.workspace_rows()
        } else {
            Vec::new()
        };
        self.tui
            .chrome_mut()
            .open_workspace_manager(&WorkspaceManagerOptions {
                trusted,
                rows,
                theme,
            });
    }

    pub(super) fn handle_workspace_manager_action(&mut self) {
        let Some(action) = self.tui.chrome_mut().take_workspace_manager_action() else {
            return;
        };
        match action {
            WorkspaceManagerAction::Close => self.tui.chrome_mut().close_focused_overlay(),
            WorkspaceManagerAction::Add => {
                self.tui.chrome_mut().open_text_input(TextInputOptions {
                    title: "Add Workspace Directory".to_owned(),
                    prompt: "Path".to_owned(),
                    submit_label: "Enter continue".to_owned(),
                });
            }
            WorkspaceManagerAction::ToggleEnabled(path) => {
                self.confirm_workspace_mutation(PendingWorkspaceMutation::ToggleEnabled {
                    path: PathBuf::from(path),
                });
            }
            WorkspaceManagerAction::ToggleRead(path) => {
                self.confirm_workspace_mutation(PendingWorkspaceMutation::ToggleRead {
                    path: PathBuf::from(path),
                });
            }
            WorkspaceManagerAction::ToggleWrite(path) => {
                self.confirm_workspace_mutation(PendingWorkspaceMutation::ToggleWrite {
                    path: PathBuf::from(path),
                });
            }
            WorkspaceManagerAction::Delete(path) => {
                self.confirm_workspace_mutation(PendingWorkspaceMutation::Delete {
                    path: PathBuf::from(path),
                });
            }
        }
    }

    pub(super) fn handle_workspace_text_input_result(&mut self, result: TextInputResult) -> bool {
        match result {
            TextInputResult::Submitted(value) => {
                self.tui.chrome_mut().close_focused_overlay();
                let path = expand_workspace_path(&self.workspace_root, value.trim());
                match self.validate_workspace_path(&path) {
                    Ok(entry) => {
                        self.confirm_workspace_mutation(PendingWorkspaceMutation::Add {
                            path: entry.path,
                        });
                    }
                    Err(error) => {
                        self.push_status(format!("Workspace path error: {error}"));
                        self.open_workspace_manager();
                    }
                }
                true
            }
            TextInputResult::Cancelled => {
                self.tui.chrome_mut().close_focused_overlay();
                self.open_workspace_manager();
                true
            }
        }
    }

    pub(super) fn handle_workspace_confirm_result(&mut self, result: ConfirmDialogResult) -> bool {
        let approved = matches!(result, ConfirmDialogResult::Approved { .. });
        self.tui.chrome_mut().close_focused_overlay();
        let Some(mutation) = self.pending_workspace_mutation.take() else {
            self.open_workspace_manager();
            return true;
        };
        if approved {
            if let Err(error) = self.apply_workspace_mutation(mutation) {
                self.push_status(format!("Failed to update workspace access: {error}"));
            }
        }
        self.refresh_workspace_policy_from_store();
        self.open_workspace_manager();
        true
    }

    pub(super) fn refresh_workspace_policy_from_store(&mut self) {
        let Ok(primary_policy) = WorkspaceAccessPolicy::new(&self.workspace_root) else {
            return;
        };
        let policy = if self.workspace_access_trusted() {
            self.workspace_store
                .as_ref()
                .and_then(|store| store.read_project(&self.workspace_root).ok())
                .and_then(|project| {
                    WorkspaceAccessPolicy::with_roots(
                        &self.workspace_root,
                        crate::workspaces::access_roots_from_project(&project),
                    )
                    .ok()
                })
                .unwrap_or(primary_policy)
        } else {
            primary_policy
        };
        if let Ok(mut guard) = self.workspace_policy.write() {
            *guard = Some(policy);
        }
    }

    fn workspace_access_trusted(&self) -> bool {
        matches!(
            self.local_config.as_ref().map(|config| &config.project_trust),
            Some(ProjectTrustState::Trusted { .. } | ProjectTrustState::NotRequired)
        )
    }

    fn workspace_rows(&self) -> Vec<WorkspaceRow> {
        let Some(store) = self.workspace_store.as_ref() else {
            return Vec::new();
        };
        store
            .read_project(&self.workspace_root)
            .map(|project| {
                project
                    .entries
                    .into_iter()
                    .map(|entry| WorkspaceRow {
                        path: entry.path.display().to_string(),
                        enabled: entry.enabled,
                        read: entry.read,
                        write: entry.write,
                        missing: !entry.path.is_dir(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn validate_workspace_path(
        &self,
        path: &Path,
    ) -> anyhow::Result<crate::workspaces::WorkspaceEntry> {
        let project = self
            .workspace_store
            .as_ref()
            .map(|store| store.read_project(&self.workspace_root))
            .transpose()?
            .unwrap_or_default();
        crate::workspaces::validate_new_workspace_entry(&self.workspace_root, &project, path)
    }

    fn confirm_workspace_mutation(&mut self, mutation: PendingWorkspaceMutation) {
        let opts = confirm_options_for_mutation(&mutation, self.tui.chrome().theme());
        self.pending_workspace_mutation = Some(mutation);
        self.tui.chrome_mut().open_confirm_dialog(opts);
    }

    fn apply_workspace_mutation(&mut self, mutation: PendingWorkspaceMutation) -> anyhow::Result<()> {
        let Some(store) = self.workspace_store.as_ref() else {
            anyhow::bail!("workspace store unavailable");
        };
        let mut project = store.read_project(&self.workspace_root)?;
        match mutation {
            PendingWorkspaceMutation::Add { path } => {
                let entry = crate::workspaces::validate_new_workspace_entry(
                    &self.workspace_root,
                    &project,
                    &path,
                )?;
                project.entries.push(entry);
            }
            PendingWorkspaceMutation::ToggleEnabled { path } => {
                if let Some(entry) = project.entries.iter_mut().find(|entry| entry.path == path) {
                    entry.enabled = !entry.enabled;
                }
            }
            PendingWorkspaceMutation::ToggleRead { path } => {
                if let Some(entry) = project.entries.iter_mut().find(|entry| entry.path == path) {
                    entry.read = !entry.read;
                    if !entry.read {
                        entry.write = false;
                    }
                }
            }
            PendingWorkspaceMutation::ToggleWrite { path } => {
                if let Some(entry) = project.entries.iter_mut().find(|entry| entry.path == path) {
                    entry.write = !entry.write;
                    if entry.write {
                        entry.read = true;
                    }
                }
            }
            PendingWorkspaceMutation::Delete { path } => {
                project.entries.retain(|entry| entry.path != path);
            }
        }
        store.write_project(&self.workspace_root, project)
    }
}

fn expand_workspace_path(workspace_root: &Path, raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

fn confirm_options_for_mutation(
    mutation: &PendingWorkspaceMutation,
    theme: neo_tui::primitive::theme::TuiTheme,
) -> ConfirmDialogOptions {
    match mutation {
        PendingWorkspaceMutation::Add { path } => ConfirmDialogOptions {
            id: format!("add:{}", path.display()),
            title: "Confirm Workspace Access".to_owned(),
            hint: "Y approve · N cancel · Esc cancel".to_owned(),
            lines: vec![
                " Add this directory to the current trusted project?".to_owned(),
                String::new(),
                " Directory".to_owned(),
                format!("   {}", path.display()),
                String::new(),
                " Access".to_owned(),
                "   enabled, read-only".to_owned(),
                String::new(),
                " Neo file tools will be able to read files under this directory.".to_owned(),
            ],
            theme,
        },
        PendingWorkspaceMutation::ToggleWrite { path } => ConfirmDialogOptions {
            id: format!("toggle-write:{}", path.display()),
            title: "Confirm Write Access".to_owned(),
            hint: "Y approve · N cancel · Esc cancel".to_owned(),
            lines: vec![
                " Enable or disable write access for this directory?".to_owned(),
                String::new(),
                format!(" {}", path.display()),
                String::new(),
                " Neo file tools may be able to edit and create files under this root.".to_owned(),
                " Tool permission mode still applies.".to_owned(),
            ],
            theme,
        },
        PendingWorkspaceMutation::Delete { path } => ConfirmDialogOptions {
            id: format!("delete:{}", path.display()),
            title: "Remove Workspace Directory".to_owned(),
            hint: "Y remove · N cancel · Esc cancel".to_owned(),
            lines: vec![
                " Remove this workspace access entry?".to_owned(),
                String::new(),
                format!(" {}", path.display()),
                String::new(),
                " Files on disk are not deleted. Only Neo's persisted access entry changes."
                    .to_owned(),
            ],
            theme,
        },
        PendingWorkspaceMutation::ToggleEnabled { path }
        | PendingWorkspaceMutation::ToggleRead { path } => ConfirmDialogOptions {
            id: format!("toggle:{}", path.display()),
            title: "Confirm Workspace Access Change".to_owned(),
            hint: "Y approve · N cancel · Esc cancel".to_owned(),
            lines: vec![
                " Change access for this workspace directory?".to_owned(),
                String::new(),
                format!(" {}", path.display()),
            ],
            theme,
        },
    }
}
```

- [ ] **Step 5: Wire dialog result processing**

In `crates/neo-agent/src/modes/interactive/dialog_results.rs`, before MCP handling, add:

```rust
if self.tui.chrome_mut().workspace_manager_action().is_some() {
    self.handle_workspace_manager_action();
    return true;
}
```

When processing text input results, route workspace add form titles first. If the existing `handle_text_input_result` consumes all text input, update that handler to return `true` when `handle_workspace_text_input_result(result)` handles a `TextInputResult` opened by workspace manager.

Add confirmation result handling:

```rust
if let Some(result) = self.tui.chrome_mut().take_confirm_dialog_result() {
    self.handle_workspace_confirm_result(result);
    return true;
}
```

- [ ] **Step 6: Run a narrow build check**

Run:

```bash
cargo check -p neo-agent --bin neo
```

Expected: PASS. If this exposes signature drift in existing constructors, update every constructor to initialize the three new workspace fields.

## Task 8: Slash, Palette, Help, And Completion

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- Modify: `crates/neo-agent/src/modes/interactive/command_palette.rs`
- Modify: `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add failing slash completion test**

In `crates/neo-agent/src/modes/interactive/tests.rs`, add near the slash completion tests:

```rust
#[test]
fn slash_completions_include_add_workspace_command() {
    let completions = prompt_completions(&test_workspace_root(), "/", &[], None, true)
        .expect("completions");
    let values: Vec<_> = completions.into_iter().map(|item| item.value).collect();
    assert!(
        values.iter().any(|value| value == "/add-workspace"),
        "missing /add-workspace in {values:?}"
    );
}
```

- [ ] **Step 2: Add failing slash opens overlay test**

In `crates/neo-agent/src/modes/interactive/tests.rs`, add:

```rust
#[tokio::test]
async fn slash_add_workspace_opens_workspace_manager_overlay() {
    let mut controller = test_controller(
        test_config(tempfile::tempdir().unwrap().path(), test_workspace_root()),
        test_workspace_root(),
    )
    .await;

    controller.type_text("/add-workspace");
    controller.submit_prompt().await.expect("submit slash");

    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("/add-workspace should open an overlay");
    assert!(
        matches!(overlay.kind, neo_tui::shell::OverlayKind::WorkspaceManager(_)),
        "/add-workspace should open the workspace manager overlay, got {:?}",
        overlay.kind
    );
}
```

If `OverlayKind` is not public from tests, match using the existing helper pattern used by `/mcp` tests in the same file.

- [ ] **Step 3: Run slash tests and verify failure**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completions_include_add_workspace_command --exact --nocapture --include-ignored
```

Expected: FAIL because `/add-workspace` is not in completion yet.

- [ ] **Step 4: Add slash completion command**

In `crates/neo-agent/src/modes/interactive/prompt_completion.rs`, add to `STATIC_SLASH_COMMANDS`:

```rust
("/add-workspace", "Manage additional workspace directories"),
```

- [ ] **Step 5: Add slash handler**

In `crates/neo-agent/src/modes/interactive/slash_commands.rs`, add to `handle_simple_slash_command`:

```rust
"/add-workspace" => self.open_workspace_manager(),
```

This branch must call `self.clear_submitted_prompt()` through the existing function tail, just like `/mcp`.

- [ ] **Step 6: Add command palette entry**

In `crates/neo-agent/src/modes/interactive/command_palette.rs`, add to `command_specs`:

```rust
CommandSpec::new(
    "add-workspace",
    "Open workspace access",
    Some("Manage additional workspace directories"),
),
```

Add to `run_open_picker_command`:

```rust
"add-workspace" => self.open_workspace_manager(),
```

If the function is async only because other branches are async, keep this branch synchronous inside the match.

- [ ] **Step 7: Run slash tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completions_include_add_workspace_command --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_add_workspace_opens_workspace_manager_overlay --exact --nocapture --include-ignored
```

Expected: PASS for both tests.

## Task 9: End-To-End Workspace Mutation Tests

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add test for approved add defaulting read-only**

Add:

```rust
#[tokio::test]
async fn add_workspace_approved_persists_enabled_read_only_entry() {
    let temp = tempfile::tempdir().expect("temp");
    let project = temp.path().join("project");
    let added = temp.path().join("added");
    std::fs::create_dir_all(&project).expect("project");
    std::fs::create_dir_all(&added).expect("added");
    let trust_path = temp.path().join("trust.json");
    let workspace_path = temp.path().join("workspaces.json");
    let mut config = test_config(temp.path(), project.clone());
    config.project_trust = crate::trust::ProjectTrustState::Trusted {
        target: project.canonicalize().expect("canonical project"),
    };
    let mut controller = test_controller(config, project.clone()).await;
    controller.set_trust_store(crate::trust::ProjectTrustStore::new(trust_path));
    controller.set_workspace_store(crate::workspaces::WorkspaceStore::new(workspace_path.clone()));

    controller.handle_slash_command("/add-workspace").await;
    controller
        .handle_input_event(neo_tui::input::InputEvent::Insert('A'))
        .await
        .expect("open add input");
    controller
        .handle_input_event(neo_tui::input::InputEvent::Paste(
            added.display().to_string(),
        ))
        .await
        .expect("paste path");
    controller
        .handle_input_event(neo_tui::input::InputEvent::Submit)
        .await
        .expect("submit path");
    controller
        .handle_input_event(neo_tui::input::InputEvent::Insert('Y'))
        .await
        .expect("approve");

    let store = crate::workspaces::WorkspaceStore::new(workspace_path);
    let project_state = store.read_project(&project).expect("read project");
    assert_eq!(project_state.entries.len(), 1);
    let entry = &project_state.entries[0];
    assert!(entry.enabled);
    assert!(entry.read);
    assert!(!entry.write);
}
```

If `set_workspace_store` does not exist yet, add it in `InteractiveController` test-only impl next to `set_trust_store`:

```rust
#[cfg(test)]
fn set_workspace_store(&mut self, store: crate::workspaces::WorkspaceStore) {
    self.workspace_store = Some(store);
    self.refresh_workspace_policy_from_store();
}
```

- [ ] **Step 2: Add test for write toggle keeping read on**

Add:

```rust
#[tokio::test]
async fn workspace_write_toggle_keeps_read_enabled() {
    let temp = tempfile::tempdir().expect("temp");
    let project = temp.path().join("project");
    let added = temp.path().join("added");
    std::fs::create_dir_all(&project).expect("project");
    std::fs::create_dir_all(&added).expect("added");
    let workspace_path = temp.path().join("workspaces.json");
    let store = crate::workspaces::WorkspaceStore::new(workspace_path.clone());
    store
        .write_project(
            &project,
            crate::workspaces::WorkspaceProject {
                entries: vec![crate::workspaces::WorkspaceEntry::read_only(
                    added.canonicalize().expect("canonical added"),
                )],
            },
        )
        .expect("seed");
    let mut config = test_config(temp.path(), project.clone());
    config.project_trust = crate::trust::ProjectTrustState::Trusted {
        target: project.canonicalize().expect("canonical project"),
    };
    let mut controller = test_controller(config, project.clone()).await;
    controller.set_workspace_store(store);

    controller.handle_slash_command("/add-workspace").await;
    controller
        .handle_input_event(neo_tui::input::InputEvent::Insert('W'))
        .await
        .expect("write toggle");
    controller
        .handle_input_event(neo_tui::input::InputEvent::Insert('Y'))
        .await
        .expect("approve");

    let loaded = crate::workspaces::WorkspaceStore::new(workspace_path)
        .read_project(&project)
        .expect("read project");
    let entry = &loaded.entries[0];
    assert!(entry.read);
    assert!(entry.write);
}
```

- [ ] **Step 3: Add test for read toggle turning write off**

Add:

```rust
#[tokio::test]
async fn workspace_read_toggle_off_turns_write_off() {
    let temp = tempfile::tempdir().expect("temp");
    let project = temp.path().join("project");
    let added = temp.path().join("added");
    std::fs::create_dir_all(&project).expect("project");
    std::fs::create_dir_all(&added).expect("added");
    let workspace_path = temp.path().join("workspaces.json");
    let mut entry = crate::workspaces::WorkspaceEntry::read_only(
        added.canonicalize().expect("canonical added"),
    );
    entry.write = true;
    let store = crate::workspaces::WorkspaceStore::new(workspace_path.clone());
    store
        .write_project(
            &project,
            crate::workspaces::WorkspaceProject {
                entries: vec![entry],
            },
        )
        .expect("seed");
    let mut config = test_config(temp.path(), project.clone());
    config.project_trust = crate::trust::ProjectTrustState::Trusted {
        target: project.canonicalize().expect("canonical project"),
    };
    let mut controller = test_controller(config, project.clone()).await;
    controller.set_workspace_store(store);

    controller.handle_slash_command("/add-workspace").await;
    controller
        .handle_input_event(neo_tui::input::InputEvent::Insert('R'))
        .await
        .expect("read toggle");
    controller
        .handle_input_event(neo_tui::input::InputEvent::Insert('Y'))
        .await
        .expect("approve");

    let loaded = crate::workspaces::WorkspaceStore::new(workspace_path)
        .read_project(&project)
        .expect("read project");
    let entry = &loaded.entries[0];
    assert!(!entry.read);
    assert!(!entry.write);
}
```

- [ ] **Step 4: Run exact integration tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::add_workspace_approved_persists_enabled_read_only_entry --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::workspace_write_toggle_keeps_read_enabled --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::workspace_read_toggle_off_turns_write_off --exact --nocapture --include-ignored
```

Expected: PASS for all three tests.

## Task 10: Plan Mode And Final Verification

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/permission.rs`

- [ ] **Step 1: Add plan-mode guard regression test**

In `crates/neo-agent-core/src/runtime/permission.rs`, add this test to the existing `#[cfg(test)]` module:

```rust
#[tokio::test]
async fn plan_mode_denies_write_to_added_write_root() {
    let primary = tempfile::tempdir().expect("primary");
    let added = tempfile::tempdir().expect("added");
    let policy = WorkspaceAccessPolicy::with_roots(
        primary.path(),
        [WorkspaceAccessRoot {
            path: added.path().canonicalize().expect("canonical added"),
            kind: WorkspaceAccessRootKind::Added,
            read: true,
            write: true,
        }],
    )
    .expect("policy");
    let plan_mode = std::sync::Arc::new(std::sync::RwLock::new(PlanMode::default()));
    {
        let mut guard = plan_mode.write().expect("plan lock");
        guard.enter_in_memory();
    }
    let config = AgentConfig::default()
        .with_workspace_root(primary.path())
        .with_plan_mode(std::sync::Arc::clone(&plan_mode))
        .with_workspace_policy(std::sync::Arc::new(std::sync::RwLock::new(Some(policy))));

    let call = AgentToolCall {
        id: "write-1".to_owned(),
        name: "Write".to_owned(),
        arguments: serde_json::json!({
            "path": added.path().join("blocked.txt"),
            "content": "blocked"
        }),
    };

    let preparation = super::permission_preparation_for_mode(
        &config,
        &call,
        &call.arguments,
        PermissionMode::Ask,
    );

    assert!(matches!(preparation, PermissionPreparation::Deny(_)));
}
```

If `permission_preparation_for_mode` is private to a module, place the test inside that module's existing `#[cfg(test)]` test module and use local names directly.

- [ ] **Step 2: Run plan-mode regression test**

Run:

```bash
cargo nextest run -p neo-agent-core --lib runtime::permission::tests::plan_mode_denies_write_to_added_write_root
```

Expected: PASS. If it fails, fix the order so plan-mode guard runs before workspace policy or permission approval.

- [ ] **Step 3: Run final narrow verification set**

Run:

```bash
cargo nextest run -p neo-agent-core --lib workspace_policy::tests::
cargo nextest run -p neo-agent-core --lib tools::read::workspace_policy_tests::
cargo nextest run -p neo-agent-core --lib tools::write::workspace_policy_tests::
cargo nextest run -p neo-tui --lib dialogs::workspace_manager::tests::
cargo nextest run -p neo-tui --lib dialogs::confirm_dialog::tests::
cargo test --package neo-agent --bin neo -- workspaces::tests::new_entry_defaults_to_enabled_read_only --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_add_workspace_opens_workspace_manager_overlay --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::add_workspace_approved_persists_enabled_read_only_entry --exact --nocapture --include-ignored
```

Expected: PASS for every command.

- [ ] **Step 4: Run formatting check**

Run:

```bash
cargo fmt --all --check
```

Expected: PASS. If it fails, run `cargo fmt --all`, then rerun `cargo fmt --all --check`.

## Self-Review

- Spec coverage: The plan covers slash command, palette/completion/help surfaces, `/mcp`-style manager UI, add path flow, confirmations, `~/.neo/workspaces.json`, trusted cwd gating, default enabled read-only entries, enable/disable/read/write/delete mutations, live runtime policy refresh, shared file-tool policy, plan-mode precedence, and focused tests.
- Placeholder scan: No `TBD`, `TODO`, `implement later`, or unspecified "add tests" steps remain. Every test step includes concrete test code or exact command filters.
- Type consistency: `WorkspaceAccessPolicy`, `WorkspaceAccessRoot`, `WorkspaceEntry`, `WorkspaceStore`, `WorkspaceManagerState`, `WorkspaceManagerAction`, `ConfirmDialogState`, and `PendingWorkspaceMutation` names are used consistently across tasks.
- Execution note: If an existing test helper has a different signature from a snippet in this plan, keep the asserted behavior exactly the same and adjust only the helper call in the named test file.
