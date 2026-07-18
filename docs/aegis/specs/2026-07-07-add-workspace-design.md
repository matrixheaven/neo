# `/add-workspace` Slash Command Design

## Goal

Add a `/add-workspace` slash command that opens an interactive TUI panel for managing additional filesystem workspace roots for the current trusted project. Added roots are persistent across sessions, scoped to the trusted canonical cwd, enabled by default, and read-only by default. Any mutation to this workspace access list must show a warning confirmation dialog before it is persisted or applied to the live runtime.

## Background

Neo's file tools currently treat the process workspace root as the primary boundary. Tools such as `List`, `Grep`, `Glob`, `Find`, `Edit`, and `Write` resolve paths through `ToolContext` workspace helpers. `Read` is looser today because it resolves absolute paths directly. Plan mode has a separate narrow exception for writing the active plan file outside the workspace.

Users sometimes need Neo to inspect or edit sibling repositories, shared libraries, generated assets, or local reference directories while staying in one conversation. Requiring a new session rooted at each directory is too heavy. The new command adds explicit, persistent, user-managed workspace roots while keeping trust and write access visible.

This feature is intentionally tied to project trust. Only a trusted cwd can manage extra workspaces. The workspace list is not session state and must survive `/new`, `/resume`, and app restarts.

## Scope

In scope:

- Add `/add-workspace` to slash handling, slash completion, command palette, and help.
- Add a `/mcp`-style workspace manager overlay.
- Add a path-entry flow for adding a directory.
- Add confirmation dialogs for every workspace access mutation.
- Persist extra workspace roots in `~/.neo/workspaces.json`, keyed by the same canonical cwd style used by trust.
- Default new entries to `enabled = true`, `read = true`, `write = false`.
- Allow enabling, disabling, deleting, and changing read/write access for entries.
- Apply changed access immediately to the current runtime without restarting Neo.
- Route all file tools through one shared multi-root workspace policy.
- Show a trusted-project warning instead of the manager when the current cwd is not trusted.

Out of scope:

- Shell/Bash cwd expansion or shell command allowlisting.
- Session-scoped temporary workspace roots.
- Project-local config files for workspace access.
- Remote filesystems or hosted collaboration.
- Automatic trust of added directories.
- Editing files in disabled roots.
- Compatibility branches for old path resolution once the unified policy is introduced.

## Product Semantics

The primary cwd remains the main workspace root. `/add-workspace` manages extra roots attached to that cwd's trust key.

An extra root contributes access only when:

- The current project is trusted.
- The entry exists and is a directory.
- The entry is enabled.
- The requested operation is allowed by its read/write flags.

Read access permits `Read`, `List`, `Grep`, `Glob`, and `Find`.

Write access permits `Edit` and `Write`. Write access requires read access. If the user turns write on, read stays on. If the user turns read off, write is turned off too after confirmation.

New entries are read-only by default:

```text
enabled = true
read    = true
write   = false
```

Plan mode remains stricter than workspace access. While plan mode is active, writes are still limited to the active plan file unless the model exits plan mode through the existing approval flow.

## Storage

Add `~/.neo/workspaces.json`.

```json
{
  "schema_version": 1,
  "projects": {
    "/Users/me/project": {
      "entries": [
        {
          "path": "/Users/me/shared-lib",
          "enabled": true,
          "read": true,
          "write": false,
          "created_at": "2026-07-07T10:00:00Z",
          "updated_at": "2026-07-07T10:00:00Z"
        }
      ]
    }
  }
}
```

Rules:

- Keys are canonical project cwd paths.
- Entry paths are canonical absolute directory paths.
- Writes are atomic, following the `trust.json` pattern.
- Corrupt JSON is backed up to `workspaces.json.bak` and treated as empty.
- Missing entries are not deleted automatically. The UI marks them as missing and runtime policy ignores them.
- The store lives under `NEO_HOME`, not under the project.

## Workspace Policy

Introduce a shared policy model in `neo-agent-core` so file tools do not each invent path semantics.

```rust
pub struct WorkspaceAccessRoot {
    pub path: PathBuf,
    pub kind: WorkspaceAccessRootKind,
    pub read: bool,
    pub write: bool,
}

pub enum WorkspaceAccessRootKind {
    Primary,
    Added,
}
```

The policy exposes:

- `resolve_read_path(path: &Path) -> Result<PathBuf, ToolError>`
- `resolve_write_path(path: &Path) -> Result<PathBuf, ToolError>`
- `display_path(path: &Path) -> String`
- `roots() -> &[WorkspaceAccessRoot]`

Path behavior:

- Relative paths resolve against the primary cwd.
- Absolute paths may resolve inside any active root with the required permission.
- Existing read paths are canonicalized before policy checks.
- New write paths canonicalize the parent directory first, then join the target file name.
- Symlinks that escape an allowed root are rejected.
- Path checks use `Path` and `PathBuf`, never string prefix matching.

`ToolContext` should hold this policy. The current `allowed_external_write_paths` remains only for plan-mode plan-file writes and is not reused for persistent workspaces.

## Runtime Flow

```text
startup / config refresh
  -> resolve current project trust state
  -> read ~/.neo/workspaces.json
  -> select entries for canonical cwd only if trusted
  -> build WorkspaceAccessPolicy
  -> store policy in AgentConfig shared state
  -> default_tool_context injects policy into ToolContext
  -> file tools resolve through policy
```

When the dialog commits a mutation:

```text
user action
  -> confirmation dialog
  -> approved
  -> atomic write to ~/.neo/workspaces.json
  -> rebuild active WorkspaceAccessPolicy
  -> refresh workspace manager overlay
  -> push status line
```

If the user cancels the confirmation, no persistent or live state changes.

## TUI Design

The manager should match the `/mcp` overlay style: bordered panel, selectable rows, keyboard hints, in-place refresh, and lightweight status markers.

### Trusted Empty State

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Workspace Access                                                          │
│ ↑↓ navigate · A add · Esc close                                           │
│                                                                            │
│  No additional workspaces configured.                                     │
│  Added directories become available to file tools for this trusted cwd.    │
│                                                                            │
│  + Add workspace directory                                                 │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

### Trusted List State

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Workspace Access                                                          │
│ ↑↓ · A add · E on/off · R read on/off · W write on/off · D delete · Esc  │
│                                                                            │
│ ▸ [on ] [R ] [W-] /Users/me/shared-lib                                    │
│       read-only · active                                                   │
│   [off] [R ] [W-] /Users/me/reference                                     │
│       disabled                                                             │
│   [on ] [R ] [W ] /Users/me/local-plugin                                  │
│       read/write · active                                                  │
│   [on ] [R ] [W-] /Users/me/missing-dir                                   │
│       missing directory · ignored                                          │
│                                                                            │
│  + Add workspace directory                                                 │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

Legend:

- `[on ]` means enabled.
- `[off]` means disabled.
- `[R ]` means read allowed.
- `[R-]` means read denied.
- `[W ]` means write allowed.
- `[W-]` means write denied.

### Untrusted State

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Workspace Access                                                          │
│ Esc close                                                                 │
│                                                                            │
│  This project is not trusted.                                              │
│                                                                            │
│  Additional workspace directories can expose files outside this cwd.       │
│  Trust this workspace before managing extra filesystem access.             │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

The untrusted panel does not offer a trust action. Trust remains owned by the existing startup trust flow and trust commands.

### Add Directory Form

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Add Workspace Directory                                                    │
│ Enter path · Tab fields · Enter continue · Esc cancel                      │
│                                                                            │
│ Path                                                                       │
│ /Users/me/shared-lib█                                                      │
│                                                                            │
│ Initial access                                                             │
│   enabled: yes                                                             │
│   read:    yes                                                             │
│   write:   no                                                              │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

The form accepts paste input and `~` expansion. Submission validates that the canonical target exists and is a directory before opening the warning confirmation.

### Add Confirmation

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Confirm Workspace Access                                                   │
│ Y approve · N cancel · Esc cancel                                          │
│                                                                            │
│  Add this directory to the current trusted project?                        │
│                                                                            │
│  Project                                                                  │
│    /Users/me/project                                                       │
│                                                                            │
│  Directory                                                                │
│    /Users/me/shared-lib                                                    │
│                                                                            │
│  Access                                                                   │
│    enabled, read-only                                                      │
│                                                                            │
│  Neo file tools will be able to read files under this directory.           │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

### Enable Write Confirmation

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Confirm Write Access                                                       │
│ Y approve · N cancel · Esc cancel                                          │
│                                                                            │
│  Enable write access for this directory?                                   │
│                                                                            │
│  /Users/me/local-plugin                                                    │
│                                                                            │
│  Neo file tools will be able to edit and create files under this root.     │
│  Tool permission mode still applies, but this expands the filesystem       │
│  boundary for the trusted project.                                         │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

### Delete Confirmation

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ Remove Workspace Directory                                                 │
│ Y remove · N cancel · Esc cancel                                           │
│                                                                            │
│  Remove this workspace access entry?                                       │
│                                                                            │
│  /Users/me/shared-lib                                                      │
│                                                                            │
│  Files on disk are not deleted. Only Neo's persisted access entry changes. │
│                                                                            │
└────────────────────────────────────────────────────────────────────────────┘
```

## Slash And Palette Behavior

Add the command in all user-visible command surfaces:

- Slash submit: `/add-workspace`
- Slash completion: `/add-workspace`
- Help panel: `/add-workspace`
- Command palette id: `add-workspace`

Completion description:

```text
Manage additional workspace directories
```

The command consumes the prompt and opens the overlay. It does not submit a model turn.

## Validation Rules

Adding a path:

- Expands `~`.
- Resolves relative paths against the primary cwd.
- Canonicalizes the path.
- Requires an existing directory.
- Rejects the primary cwd itself.
- Rejects paths already covered by the primary cwd.
- Rejects duplicate canonical entries.
- Rejects a parent/child relationship with an existing added entry for the first version.

Toggling permissions:

- `E` toggles the selected entry between enabled and disabled.
- `R` toggles read access for the selected entry.
- `W` toggles write access for the selected entry.
- Turning write on also keeps read on.
- Turning read off also turns write off.
- Turning write on uses the stronger write confirmation copy.
- Turning read off uses a warning that file tools will no longer be able to inspect the root.

Runtime invalidation:

- Missing directories remain visible but inactive.
- Disabled entries remain visible but inactive.
- Entries with read disabled and write disabled remain visible but inactive.

## Error Handling

User-facing validation failures stay inside the dialog as status text:

- `Path does not exist`
- `Path is not a directory`
- `Directory is already inside the primary workspace`
- `Directory is already configured`
- `Directory overlaps another added workspace`
- `Failed to save workspace access: <error>`

Store read failures that are not corrupt JSON surface as status lines and keep the manager closed, because the UI cannot safely display or mutate unknown state.

Corrupt store handling follows trust store behavior: back up, warn through tracing/status, and continue with an empty store.

## Security Considerations

This feature expands model-visible filesystem boundaries, so the secure defaults are:

- Only trusted cwd can manage entries.
- New entries are read-only.
- Write access requires explicit confirmation.
- Every mutation requires confirmation.
- Plan mode remains a hard write guard.
- Shell access is not expanded.
- Symlink escapes are rejected.
- The model is told about active roots, but disabled or missing roots are not included in runtime access.

## Tests

Focused tests should avoid broad package-wide runs.

`neo-agent` tests:

- `/add-workspace` consumes the prompt and opens the workspace manager overlay.
- Untrusted project opens the untrusted warning panel.
- Trusted project opens list state with persisted entries.
- Add form validates a directory and opens confirmation.
- Approved add persists read-only enabled entry and refreshes live policy.
- Cancelled mutation does not change store or policy.
- Enable write requires confirmation and persists `write = true`.
- Disable read turns write off.

`neo-tui` tests:

- Manager renders empty, list, untrusted, and missing-directory states.
- Keyboard actions emit add, enable/disable toggle, read on/off toggle, write on/off toggle, delete, and close actions.
- Confirmation dialogs render the correct warning copy.
- Narrow widths truncate long paths without overlapping text.

`neo-agent-core` tests:

- Policy allows reads inside primary cwd.
- Policy allows reads inside enabled read roots.
- Policy denies reads inside disabled or read-disabled roots.
- Policy allows writes only inside enabled write roots.
- Policy rejects symlink escapes.
- `Read`, `List`, `Grep`, `Glob`, `Find`, `Edit`, and `Write` all use the shared policy.
- Plan mode still denies writes to added write roots.

Example narrow verification commands:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::<exact_test_name> --exact --nocapture --include-ignored
cargo nextest run -p neo-tui --test <target> <workspace_manager_filter>
cargo nextest run -p neo-agent-core --lib <workspace_policy_filter>
```

## Migration

There is no migration from existing state because the feature adds a new store. Existing sessions continue to use only the primary cwd until the user adds entries.

The implementation should remove or replace any old per-tool path logic that conflicts with the shared workspace policy. In particular, `Read` must stop bypassing the workspace policy for absolute paths.

## Non-Goals

This is not a general permission editor, a project config system, or a shell sandbox. It only manages persistent filesystem roots for Neo's built-in file tools under a trusted cwd.

## Self-Review

- Placeholder scan: no placeholders remain.
- Consistency check: default added roots are consistently enabled and read-only, and write access always requires explicit confirmation.
- Scope check: the spec is focused on persistent extra workspace roots, TUI management, runtime file-tool policy, and tests.
- Ambiguity check: shell behavior, plan mode precedence, untrusted-project behavior, storage scope, and overlapping path rules are explicit.
