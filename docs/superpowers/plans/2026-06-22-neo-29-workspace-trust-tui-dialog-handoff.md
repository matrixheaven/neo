# NEO-29 Workspace Trust TUI Dialog Handoff Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show a blocking Workspace Trust dialog the first time Neo opens a directory with project context inputs, so project-local instructions and configuration are not loaded before the user explicitly trusts or rejects the workspace.

**Architecture:** Treat trust as a startup security gate, not a normal preferences dialog. Resolve project trust as a three-state decision (`trusted`, `untrusted`, `unknown`), show the TUI dialog only for `unknown`, persist both trust and untrusted decisions, and keep project context disabled in restricted mode. Borrow Codex's trust model for unknown/untrusted project handling and VS Code's workspace-trust UX; Kimi Code is a negative reference because it loads project instructions unconditionally.

**Tech Stack:** Rust 2024, `clap`, `crossterm`, `neo-agent` config/trust startup flow, `neo-tui` blocking overlays, JSON trust store at `~/.neo/trust.json`, Codex workspace trust references, `xtask`/`nextest`/`llvm-cov`/CRAP gates.

---

## Linear Context

- Linear: [NEO-25](https://linear.app/ezc2/issue/NEO-25/实现新目录进入-neo-时的-workspace-trust-tui-对话框)
- Title: 实现新目录进入 neo 时的 Workspace Trust TUI 对话框
- Priority: Medium
- Project: TUI & UX Polish
- Security risk: untrusted project files such as `AGENTS.md`, `CLAUDE.md`, `.neo/`, or `.agents/skills/` can prompt-inject Neo before any tool approval happens. The dialog must happen before loading project context.

## Mandatory References

Read before implementation:

- `AGENTS.md`
- `~/.codex/RTK.md`
- `~/.codex/CX.md`
- `crates/neo-agent/src/trust.rs`
- `crates/neo-agent/src/config.rs`
- `crates/neo-agent/src/resources.rs`
- `crates/neo-agent/src/modes/interactive.rs`
- `crates/neo-agent/src/cli.rs`
- `crates/neo-agent/src/main.rs`
- `crates/neo-tui/src/chrome.rs`
- `crates/neo-tui/src/components.rs`
- `docs/codex/codex-rs/tui/src/onboarding/trust_directory.rs`
- `docs/codex/codex-rs/tui/src/onboarding/onboarding_screen.rs`
- `docs/codex/codex-rs/config/src/loader/mod.rs`
- `docs/codex/codex-rs/core/src/config/config_loader_tests.rs`
- `docs/kimi-code/packages/agent-core/src/profile/context.ts`
- `docs/kimi-code/packages/agent-core/src/mcp/config-loader.ts`

Reference conclusions:

- Kimi Code does not have a workspace trust gate; do not copy that security model.
- Codex disables project-local layers when trust is unknown or explicitly untrusted.
- Codex shows a TUI trust screen only when trust is unknown.
- Codex does not repeatedly prompt once a project is explicitly marked untrusted.
- Codex resolves a trust target at the repository root when the user is in a nested directory.
- VS Code's Workspace Trust is the UX baseline: trust this workspace, trust parent, or continue restricted.

## Current Neo State

### Trust Store Exists But Cannot Be Written In Production

- `crates/neo-agent/src/trust.rs`
  - `ProjectTrustStore`
  - `ProjectTrustStore::from_home`
  - `ProjectTrustStore::get`
  - `ProjectTrustStore::set` exists but is `#[cfg(test)]`.
  - `resolve_project_trust(project_dir, yolo) -> bool` returns only trusted/untrusted, losing `unknown`.
  - `has_project_trust_inputs(project_dir)` detects ancestor `AGENTS.md`/`CLAUDE.md` and `.agents/skills`, but not `.neo/`.

### Project Context Loading Is Already Gated

- `crates/neo-agent/src/resources.rs`
  - `load_context_files(project_dir, project_trusted)`
  - project-level context files are loaded only when `project_trusted == true`.

Keep this gate. The main bug is that unknown trust silently becomes untrusted and there is no TUI/CLI path to persist a decision.

### Config Load Collapses Unknown To False

- `crates/neo-agent/src/config.rs`
  - `AppConfig::load` calls `project_trusted_from_yolo`.
  - `project_trusted_from_yolo` calls `trust::resolve_project_trust`.
  - `AppConfig` stores only `project_trusted: bool`.

For NEO-25, add enough trust metadata to know whether startup should show a dialog.

### CLI Docs Are Ahead Of Code

- `AGENTS.md` and `docs/quickstart.md` mention:

```bash
neo trust status
neo trust approve
neo trust deny
neo trust clear
```

- `crates/neo-agent/src/cli.rs` currently has no `Command::Trust`.

Restoring the CLI command is useful and keeps docs honest.

### Blocking Overlay Infrastructure Exists

- `crates/neo-tui/src/chrome.rs`
  - `OverlayKind`
  - `ChoicePicker`
  - `focused_overlay_blocks_prompt`
  - input routing for picker/approval/question overlays
- `crates/neo-tui/src/components.rs`
  - overlay heights and prompt hiding behavior

Use the existing blocking dialog contract: main prompt height must be zero, and all relevant input must route to the trust dialog.

## Product Decisions

### Trust States

Use three states:

```rust
pub enum ProjectTrustDecision {
    Trusted { source: TrustSource },
    Untrusted { source: TrustSource },
    Unknown { inputs: ProjectTrustInputs },
}
```

`Unknown` means Neo detected trust-sensitive project inputs but no stored decision applies.

`Untrusted` is not an error. It is an explicit restricted-mode decision and must not repeatedly prompt.

### Startup Behavior

| Inputs present? | Stored decision | TUI behavior | Project context |
| --- | --- | --- | --- |
| no | none | no dialog | no project context to load |
| yes | trusted current/ancestor | no dialog | load project context |
| yes | untrusted current/ancestor | no dialog | do not load project context |
| yes | unknown | blocking dialog | wait for user choice |

### Dialog Choices

The TUI dialog must offer:

1. Trust this directory
2. Trust parent directory
3. Continue untrusted

Default highlighted option: Continue untrusted.

Reason: NEO-25 is a security prompt. The safest selection should be default.

### Trust Parent Semantics

When the user chooses Trust parent directory:

- scan ancestors from current directory upward;
- include only ancestors with project trust inputs;
- prefer the nearest meaningful project root;
- if multiple candidates exist, show a second-step list;
- persist trust for the selected ancestor;
- current directory inherits trust through ancestor lookup.

Borrow Codex's root-target idea:

- if inside a git repository, the trust parent option should clearly show the repository root;
- if not inside git, show nearest ancestor with context inputs.

Do not run mutating git commands. Read-only root detection is fine; a pure filesystem `.git` walk is better if sufficient.

### Restricted Mode

Continue untrusted should:

- persist `Some(false)` for the selected current trust target;
- set `config.project_trusted = false`;
- continue startup;
- never load project `AGENTS.md`, `CLAUDE.md`, `.neo/SYSTEM.md`, `.neo/APPEND_SYSTEM.md`, project prompt templates, project skills, or project MCP config if those are introduced later;
- show a small startup status like `Workspace untrusted: project context disabled`.

Neo currently has only user-global config, but the plan should explicitly guard future project-local config and MCP loading. Codex treats project-local config/hooks/exec policies as disabled when untrusted or unknown; Neo should follow that principle.

### Yolo Behavior

Linear says existing yolo behavior skips project context. Keep current behavior unless product direction changes:

- yolo should not silently load untrusted project context;
- yolo should not show the trust dialog;
- yolo should run with `project_trusted = false` unless a future explicit flag says otherwise.

This may sound counterintuitive, but it matches the current Neo code and avoids letting a broad tool permission mode become implicit trust in project instructions.

## UX Design

### Initial Dialog

```text
╭─ Workspace Trust ──────────────────────────────────────────────────────────╮
│ Neo found project context files before loading this workspace.             │
│                                                                            │
│ Directory                                                                  │
│   /Users/me/src/acme/tools/cli                                             │
│                                                                            │
│ Detected                                                                   │
│   AGENTS.md                                                                │
│   .neo/                                                                    │
│   .agents/skills/                                                          │
│                                                                            │
│ Project instructions can change model behavior. Trust only workspaces      │
│ whose contents you understand.                                             │
│                                                                            │
│   1. Trust this directory                                                  │
│   2. Trust parent directory                                                │
│ ▶ 3. Continue untrusted                                                    │
│                                                                            │
│ ↑/↓ select · 1/2/3 choose · Enter confirm                                  │
╰────────────────────────────────────────────────────────────────────────────╯
```

### Parent Directory Second Step

If multiple ancestors are candidates:

```text
╭─ Trust Parent Directory ───────────────────────────────────────────────────╮
│ Select the ancestor to trust. Trusting a parent also trusts child folders. │
│                                                                            │
│   1. /Users/me/src/acme/tools                                              │
│ ▶ 2. /Users/me/src/acme                                                    │
│   3. /Users/me/src                                                        │
│                                                                            │
│ ↑/↓ select · Enter trust selected · Esc back                               │
╰────────────────────────────────────────────────────────────────────────────╯
```

If only one parent candidate exists, the first dialog can show it inline:

```text
│   2. Trust parent directory                                                │
│      /Users/me/src/acme                                                    │
```

### Trusted Result

After the user trusts:

```text
Workspace trusted: /Users/me/src/acme
Loaded project context from AGENTS.md
```

Use normal status transcript styling. Do not display project file contents in the trust dialog.

### Untrusted Result

After the user continues untrusted:

```text
Workspace untrusted: project context disabled
```

Footer can continue to show normal permission/development modes. Do not add a noisy permanent badge unless there is already a status area for workspace trust.

### Blocking Contract

While the trust dialog is open:

- hide composer (`prompt_height = 0`);
- all insert, paste, delete, arrows, Enter, Escape go to the dialog;
- no slash commands run;
- no model turn starts;
- no project context is loaded.

This dialog is more security-sensitive than approval prompts because it decides which instructions Neo itself may read.

## Implementation Tasks

### Task 1: Add Trust Decision Model Tests

**Files:**

- Modify: `crates/neo-agent/src/trust.rs`

- [ ] Add tests for `Trusted`, `Untrusted`, and `Unknown`.
- [ ] Add tests for ancestor trust inheritance.
- [ ] Add tests for explicit untrusted inheritance.
- [ ] Add tests detecting `.neo/` and `.agents/skills/`.
- [ ] Add tests for parent candidate scanning.

Candidate tests:

```rust
#[test]
fn trust_decision_is_unknown_when_inputs_exist_without_store_entry() {}

#[test]
fn trust_decision_inherits_trusted_ancestor() {}

#[test]
fn trust_decision_inherits_untrusted_ancestor() {}

#[test]
fn trust_inputs_detect_neo_directory_and_agents_skills() {}

#[test]
fn trust_parent_candidates_include_ancestors_with_inputs() {}
```

Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent trust
```

Expected before implementation: tests fail because the trust API collapses unknown to false and does not expose parent candidates.

### Task 2: Refactor Trust Store API

**Files:**

- Modify: `crates/neo-agent/src/trust.rs`

- [ ] Make `ProjectTrustStore::set(&self, project_dir, Option<bool>)` available outside tests.
- [ ] Keep JSON sorted and canonicalized.
- [ ] Add public or `pub(crate)` `ProjectTrustDecision`.
- [ ] Add `resolve_project_trust_decision(project_dir, yolo) -> Result<ProjectTrustDecision>`.
- [ ] Migrate call sites away from the old boolean-only trust resolver and remove that resolver in the same change.
- [ ] Add `ProjectTrustInputs` containing detected paths and candidate parent directories.
- [ ] Detect:
  - `AGENTS.md`
  - `AGENTS.MD`
  - `CLAUDE.md`
  - `CLAUDE.MD`
  - `.neo/`
  - `.agents/skills/`

Do not read the contents of project files while deciding whether to show the dialog. Existence checks are enough.

### Task 3: Carry Trust Metadata Through Config

**Files:**

- Modify: `crates/neo-agent/src/config.rs`

- [ ] Add a config field for the resolved trust decision or startup trust prompt data.
- [ ] Preserve existing `project_trusted: bool` for resource loading if it keeps changes small.
- [ ] Ensure `AppConfig::load` can identify unknown trust without loading project context.
- [ ] Ensure yolo keeps `project_trusted = false` and does not request the dialog.
- [ ] Add config tests for trusted, untrusted, unknown, and no-input directories.

Suggested shape:

```rust
pub struct AppConfig {
    pub project_trusted: bool,
    pub project_trust: ProjectTrustState,
    // existing fields...
}

pub enum ProjectTrustState {
    Trusted { target: PathBuf },
    Untrusted { target: PathBuf },
    Unknown { inputs: ProjectTrustInputs },
    NotRequired,
}
```

Keep this simple. The goal is not a new global policy system.

### Task 4: Build Trust Dialog State And Renderer

**Files:**

- Create: `crates/neo-tui/src/dialogs/trust.rs` or equivalent existing dialogs module path
- Modify: `crates/neo-tui/src/dialogs/mod.rs`
- Modify: `crates/neo-tui/src/chrome.rs`
- Modify: `crates/neo-tui/src/components.rs`

- [ ] Add `TrustDialogState`.
- [ ] Add selection enum:

```rust
pub enum TrustDialogChoice {
    TrustCurrent,
    TrustParent,
    ContinueUntrusted,
}
```

- [ ] Default selection is `ContinueUntrusted`.
- [ ] Render current directory.
- [ ] Render detected context file/directory list.
- [ ] Render parent candidate path when available.
- [ ] Render second-step parent list when multiple candidates exist.
- [ ] Add `OverlayKind::TrustDialog`.
- [ ] Add it to `focused_overlay_blocks_prompt`.
- [ ] Set dialog height around 16 rows, with truncation for long paths.

Snapshot tests:

```rust
#[test]
fn trust_dialog_defaults_to_continue_untrusted() {}

#[test]
fn trust_dialog_renders_detected_inputs_without_file_contents() {}

#[test]
fn trust_dialog_renders_parent_selection_step() {}

#[test]
fn trust_dialog_hides_prompt_when_focused() {}
```

### Task 5: Integrate Dialog Into Interactive Startup

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-agent/src/config.rs`
- Modify: `crates/neo-agent/src/trust.rs`

- [ ] During interactive startup, if trust state is `Unknown`, open the trust dialog before accepting prompt input.
- [ ] On Trust this directory, persist `Some(true)` for current directory and set runtime config/chrome state to trusted.
- [ ] On Trust parent directory, persist `Some(true)` for selected ancestor.
- [ ] On Continue untrusted, persist `Some(false)` for current trust target and keep `project_trusted = false`.
- [ ] After a trust/untrusted decision, rebuild or reload only the resources affected by trust.
- [ ] If trusted, project context can be loaded for subsequent turns.
- [ ] If untrusted, project context remains disabled.
- [ ] Do not start a model turn until the dialog is resolved.

Important architecture decision:

`AppConfig::load` currently computes `project_trusted` before `resources::load_system_prompt` is called later in run execution. That is good. The implementation can update `InteractiveController`'s config/state before the first turn. If `AppConfig` is immutable in the controller, add a small controller-local trust override and pass the updated trust bool into run requests.

### Task 6: Restore CLI Trust Commands

**Files:**

- Modify: `crates/neo-agent/src/cli.rs`
- Modify: `crates/neo-agent/src/main.rs`
- Test: `crates/neo-agent/tests/cli_commands.rs`

Add:

```text
neo trust status
neo trust approve
neo trust deny
neo trust clear
```

Behavior:

- `status` prints current directory, trust target, detected inputs, and effective decision.
- `approve` writes `Some(true)` for current directory unless `--parent <path>` or `--parent` selection support is added.
- `deny` writes `Some(false)`.
- `clear` removes the current directory decision.

Keep scope small; TUI parent selection is the main UX. CLI can be current-directory first unless Linear acceptance explicitly requires parent CLI support.

Candidate tests:

```rust
#[test]
fn trust_status_reports_unknown_for_context_directory_without_decision() {}

#[test]
fn trust_approve_persists_trusted_decision() {}

#[test]
fn trust_deny_persists_untrusted_decision() {}

#[test]
fn trust_clear_removes_decision() {}
```

### Task 7: Guard Project-Local Resources Beyond AGENTS.md

**Files:**

- Modify: `crates/neo-agent/src/resources.rs`
- Modify: skill/prompt/config loaders if project-local variants exist
- Test: relevant module tests

- [ ] Confirm project `AGENTS.md`/`CLAUDE.md` stay gated.
- [ ] Confirm project `.neo/SYSTEM.md` and `.neo/APPEND_SYSTEM.md`, if currently loaded, are not loaded while untrusted.
- [ ] Confirm project skills under `.agents/skills/` are not loaded while untrusted.
- [ ] Confirm project prompt templates under `.neo/prompts/` are not loaded while untrusted if they exist.
- [ ] Confirm MCP/project config cannot be loaded from an untrusted project if project-local config returns in the future.

Codex reference: unknown/untrusted project layers are loaded only as disabled layers for diagnostics, not applied to effective config.

### Task 8: Documentation

**Files:**

- Modify: `docs/quickstart.md`
- Modify: `docs/tools.md` if trust affects tool behavior docs
- Modify: `AGENTS.md` only if project guide needs updated details

Document:

- when the trust dialog appears;
- what Trust this directory does;
- what Trust parent directory does;
- what Continue untrusted does;
- where trust decisions are stored;
- how CLI trust commands work;
- yolo does not imply project instruction trust.

## Verification Plan

Focused tests:

```bash
rtk cargo run -p xtask -- test -p neo-agent trust
rtk cargo run -p xtask -- test -p neo-tui trust
rtk cargo run -p xtask -- test -p neo-agent interactive_trust
rtk cargo run -p xtask -- test -p neo-agent cli_commands::trust
```

Adjust filters to real test names.

Required repository gates before completion:

```bash
rtk cargo run -p xtask -- coverage
rtk cargo run -p xtask -- crap
rtk cargo run -p xtask -- ci
```

Artifacts to inspect:

- `target/llvm-cov/lcov.info`
- `target/crap/crap-crates.md`
- `target/crap/crap-crates.json`

## Easy-To-Miss Pitfalls

- Do not load `AGENTS.md` and then ask whether to trust it. The dialog must happen before project context is injected.
- Do not collapse unknown and explicit untrusted. Unknown prompts; explicit untrusted does not prompt again.
- Do not make Trust this directory the default highlighted option. The safe default is Continue untrusted.
- Do not read or display project context file contents inside the trust dialog.
- Do not let yolo imply project context trust.
- Do not let blocked trust dialog input leak into the composer.
- Do not silently ignore `.neo/` or `.agents/skills/` when deciding whether a trust gate is needed.
- Do not store trust decisions in the project directory.
- Do not use relative path keys that break after `cd` from a symlink. Canonicalize and normalize.
- Do not use git mutation commands. Root detection must be read-only.
- Do not add old compatibility layers that keep both trust models alive. Migrate to the three-state model.

## Self-Review Checklist

- [ ] Unknown trust with detected inputs opens a blocking TUI dialog.
- [ ] Explicit untrusted does not reopen the dialog.
- [ ] Trusted current directory loads project context.
- [ ] Trusted parent directory is inherited by nested children.
- [ ] Continue untrusted persists `false` and disables project context.
- [ ] Default dialog selection is Continue untrusted.
- [ ] Dialog lists paths but not file contents.
- [ ] Composer is hidden while dialog is focused.
- [ ] CLI trust status/approve/deny/clear work.
- [ ] Project resource loading is gated by trust.
- [ ] Tests cover trust store, config, TUI render/input, interactive startup, and CLI.
- [ ] Verification commands ran through `xtask`.

## Suggested ICM Store On Completion

```bash
rtk icm store -t context-neo -c "Implemented NEO-25 Workspace Trust TUI dialog: Neo now resolves project trust as trusted/untrusted/unknown, prompts before loading project context, persists trust and untrusted decisions, supports trust parent selection, restores CLI trust commands, and keeps untrusted workspaces in restricted mode." -i high -k "NEO-25,workspace-trust,tui,security,codex"
```
