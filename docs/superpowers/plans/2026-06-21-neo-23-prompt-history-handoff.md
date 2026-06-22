# NEO-23 Cross-Session Prompt History Handoff Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist prompt input history across TUI sessions in the same workspace and let Up/Down recall past prompts from an empty composer without leaking input through blocking dialogs.

**Architecture:** Neo already has in-memory `PromptState` history and Up/Down keybindings. Add a small append-only JSONL prompt history store under the workspace session bucket, load it into `PromptState` during controller construction, and append successful user prompts after real submission. Keep history workspace-scoped, ordered by submission time, trim empty entries, and deduplicate consecutive repeats.

**Tech Stack:** Rust 2024, `serde_json`, workspace-scoped session paths, `neo-agent` interactive controller, `neo-tui` prompt state and keybindings, `xtask`/`nextest`/LCOV/CRAP.

---

## Linear Context

- Linear: [NEO-23](https://linear.app/ezc2/issue/NEO-23/neo-23-implement-cross-session-prompt-input-history-with-updown-arrow)
- Priority: Urgent
- Project: TUI & UX Polish
- Summary: Implement global prompt input history across sessions for the current workspace. Up/Down recalls prompts sorted by send time, not by session recency.
- Reference behavior:
  - Kimi Code stores JSONL per CWD and loads it into editor history on startup.
  - Codex stores richer global JSONL records with session id and timestamp, plus history navigation guards and file safety.
  - Neo has in-memory history and Up/Down navigation but no cross-session persistence.

## Non-Negotiable Project Rules

- Before coding:

```bash
rtk icm recall-context "NEO-23 cross-session prompt history" --limit 5
```

- Use `rtk` for shell commands.
- Prefer `cx` for symbol navigation.
- Do not run bare `cargo test`; use `rtk cargo run -p xtask -- test ...`.
- Do not perform git mutations without explicit per-command user authorization.
- Keep scope limited to prompt history. Do not add Ctrl+R search in this issue.
- Store completion memory before final response:

```bash
rtk icm store -t context-neo -c "Completed NEO-23: workspace-scoped prompt-history JSONL loads across TUI sessions, Up/Down recalls from empty composer, consecutive duplicate/blank prompts are skipped, blocking dialogs do not leak into PromptState, and xtask gates pass." -i high -k "NEO-23,prompt-history,tui"
```

## Current Code Map

### PromptState And History

- `crates/neo-tui/src/chrome.rs`
  - `PromptState`
    - Fields: `text`, `cursor`, `history`, `history_index`, `history_draft`, `undo_stack`, `kill_ring`
  - `PromptState::remember_history`
    - Currently skips blank entries but does not trim/deduplicate/persist.
  - `PromptState::recall_previous_history`
    - Currently recalls even when composer is non-empty.
  - `PromptState::recall_next_history`
  - `PromptState::apply_edit`
    - Edits call `stop_history_navigation`.
  - `PromptState::replace_with_history_text`
  - `PromptState::stop_history_navigation`
  - `NeoChromeState::submit_prompt`
    - Calls `self.prompt.remember_history(submitted.clone())` and clears prompt.

### Keybindings And Event Routing

- `crates/neo-tui/src/input.rs`
  - `editor_keybinding_definitions`
    - `EditorCursorUp = up`
    - `EditorCursorDown = down`
  - `picker_keybinding_definitions`
    - `SelectUp = up`
    - `SelectDown = down`

- `crates/neo-agent/src/modes/interactive.rs`
  - `InteractiveController::handle_input_event`
    - Handles approval/rich dialog before prompt editing.
  - `InteractiveController::handle_prompt_history_action`
    - Calls `recall_previous_history` / `recall_next_history`.
  - `InteractiveController::submit_current_prompt`
    - Slash commands return before `NeoChromeState::submit_prompt`; only real prompts enter history.
  - `controller_for_config`
    - Good place to load workspace prompt history into the new controller.

### Existing Tests

- `crates/neo-tui/tests/primitives.rs`
  - `prompt_history_recalls_entries_and_restores_draft`
- `crates/neo-agent/src/modes/interactive.rs`
  - `event_loop_uses_up_down_keys_for_prompt_history`
  - `question_dialog_consumes_keyboard_before_prompt_editing`
  - `question_dialog_prioritizes_real_keybindings_before_prompt_editing`
  - `approval_uses_selection_priority_for_real_keys`
  - `approval_revise_collects_feedback_without_editing_prompt`

### Workspace Paths

- `crates/neo-agent-core/src/session/workspace.rs`
  - `workspace_sessions_dir`
  - `encode_workdir_key`
  - `normalize_workdir`
- `crates/neo-agent/src/config.rs`
  - `neo_home`
  - `workspace_sessions_dir(config)`

Recommended history path:

```text
$NEO_HOME/sessions/wd_<workspace-slug>_<hash12>/prompt-history.jsonl
```

This reuses Neo's workspace bucket and avoids global cross-project history bleed.

## Product Design

### Storage Format

Use JSONL with one prompt per line:

```json
{"created_at":"2026-06-21T12:34:56.789Z","session_id":"01KV...","text":"implement /new"}
```

Fields:

- `created_at`: ISO-8601 or stable millisecond timestamp string. Used for diagnostics; file order remains the primary ordering.
- `session_id`: optional. Include the active session id if known; `null` or omitted for unsaved sessions.
- `text`: trimmed prompt text.

Rules:

- Append only.
- Create parent directory if missing.
- Skip empty/whitespace-only prompts.
- Deduplicate consecutive repeated prompts.
- Keep latest N entries in memory. Recommended default: 500.
- Do not scan old session JSONL files to reconstruct history.
- Do not persist slash commands that do not become user turns.

### Navigation Semantics

Up/Down recall should feel conservative:

- Up from an empty composer recalls the most recent prompt.
- Up while already navigating history moves older.
- Down while navigating moves newer.
- Down past the newest restores the original draft, usually empty.
- Non-empty composer that is not already navigating should not be overwritten by Up.
- Edits during navigation stop history navigation and preserve normal editing behavior.

### Blocking Dialog Semantics

When approval, question, model, provider, session picker, or any focused blocking overlay is active:

- Up/Down should move dialog selection or do dialog-specific behavior.
- Prompt history should not change.
- `PromptState` should not receive text typed into dialogs.

## TUI Design

### Data Flow

```text
User submits prompt
        |
        v
InteractiveController::submit_current_prompt
        |
        +--> NeoChromeState::submit_prompt
        |       |
        |       v
        |   PromptState::remember_history      (in-memory current session)
        |
        +--> PromptHistoryStore::append        (workspace bucket JSONL)
                |
                v
$NEO_HOME/sessions/wd_<workspace>_<hash>/prompt-history.jsonl


New TUI session
        |
        v
controller_for_config
        |
        v
workspace_sessions_dir(config)
        |
        v
PromptHistoryStore::load_recent
        |
        v
PromptState::set_history
        |
        v
Up/Down on empty composer recalls history
```

### Composer States

```text
[manual] [normal] session new · openai/gpt-4.1
>
```

Press Up:

```text
[manual] [normal] session new · openai/gpt-4.1
> implement /new slash command
```

Press Up again:

```text
[manual] [normal] session new · openai/gpt-4.1
> fix approval prompt rendering
```

Type while non-empty, then Up:

```text
[manual] [normal] session new · openai/gpt-4.1
> partial draft
```

Expected: no overwrite on first Up, because the composer is non-empty and not in history navigation.

No extra visible instructional text is needed in-app.

## Implementation Tasks

### Task 1: Tighten `PromptState` In-Memory History Semantics

**Files:**

- Modify: `crates/neo-tui/src/chrome.rs`
- Test: `crates/neo-tui/tests/primitives.rs`

- [ ] Write failing tests:

```rust
#[test]
fn prompt_history_skips_blank_and_consecutive_duplicates() {
    let mut prompt = PromptState::default();
    prompt.remember_history("  first prompt  ");
    prompt.remember_history("first prompt");
    prompt.remember_history("   ");
    prompt.remember_history("second prompt");

    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "second prompt");
    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "first prompt");
    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "first prompt");
}

#[test]
fn prompt_history_does_not_overwrite_non_empty_draft_on_first_up() {
    let mut prompt = PromptState::new("partial").with_cursor(7);
    prompt.remember_history("old prompt");

    assert!(!prompt.recall_previous_history());
    assert_eq!(prompt.text, "partial");
}

#[test]
fn prompt_history_continues_navigation_after_history_entry_is_active() {
    let mut prompt = PromptState::default();
    prompt.remember_history("first");
    prompt.remember_history("second");

    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "second");
    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "first");
}
```

- [ ] Add methods:

```rust
pub fn set_history(&mut self, entries: impl IntoIterator<Item = String>) {
    self.history.clear();
    self.history_index = None;
    self.history_draft = None;
    for entry in entries {
        self.remember_history(entry);
    }
}
```

- [ ] Update `remember_history`:

```rust
pub fn remember_history(&mut self, entry: impl Into<String>) {
    let entry = entry.into().trim().to_owned();
    if entry.is_empty() {
        return;
    }
    if self.history.last().is_some_and(|last| last == &entry) {
        self.stop_history_navigation();
        return;
    }
    self.history.push(entry);
    self.stop_history_navigation();
}
```

- [ ] Update `recall_previous_history` to return `false` when `history_index.is_none()` and `text` is not empty.

- [ ] Run:

```bash
rtk cargo run -p xtask -- test -p neo-tui prompt_history
```

### Task 2: Add Workspace Prompt History Store

**Files:**

- Create: `crates/neo-agent/src/prompt_history.rs`
- Modify: `crates/neo-agent/src/main.rs` or `crates/neo-agent/src/lib.rs` equivalent module root if needed.
- Test: module tests in `prompt_history.rs`

Use `neo-agent` rather than `neo-agent-core` unless other crates need this API. This is a TUI/CLI convenience surface, not core agent runtime behavior.

- [ ] Define record and store:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
struct PromptHistoryRecord {
    created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    text: String,
}

pub(crate) struct PromptHistoryStore {
    path: PathBuf,
    max_entries: usize,
}
```

- [ ] Implement:

```rust
impl PromptHistoryStore {
    pub(crate) fn for_config(config: &AppConfig) -> Self;
    pub(crate) fn load_recent(&self) -> anyhow::Result<Vec<String>>;
    pub(crate) fn append(&self, session_id: Option<&str>, text: &str) -> anyhow::Result<bool>;
}
```

- [ ] Path:

```rust
let dir = crate::config::workspace_sessions_dir(config);
let path = dir.join("prompt-history.jsonl");
```

- [ ] Append behavior:

```rust
let text = text.trim();
if text.is_empty() {
    return Ok(false);
}
if self.load_recent()?.last().is_some_and(|last| last == text) {
    return Ok(false);
}
std::fs::create_dir_all(parent)?;
let file = OpenOptions::new().create(true).append(true).open(&self.path)?;
// On Unix, set 0o600 when creating if practical.
serde_json::to_writer(&file, &record)?;
writeln!(&file)?;
Ok(true)
```

Avoid overengineering locks in this first version unless tests show concurrent writes are likely. `O_APPEND` is enough for normal local TUI use.

- [ ] Load behavior:

Read lines in file order. Ignore malformed lines by skipping them and keeping the rest usable; this prevents one corrupt line from killing the TUI. Trim/dedup consecutive records. Return at most `max_entries`.

- [ ] Tests:

```rust
#[test]
fn prompt_history_store_appends_trims_and_skips_consecutive_duplicates() { ... }

#[test]
fn prompt_history_store_loads_in_file_order_across_sessions() { ... }

#[test]
fn prompt_history_store_uses_distinct_workspace_buckets() { ... }
```

- [ ] Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent prompt_history
```

### Task 3: Load Persistent History Into TUI Controller

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/neo-agent/src/modes/interactive.rs`

- [ ] In `controller_for_config` or `InteractiveController::apply_startup_options`, construct `PromptHistoryStore::for_config(config)`.

- [ ] Load `store.load_recent()` and call `PromptState::set_history(entries)`.

- [ ] Store `PromptHistoryStore` in `InteractiveController` as an optional/private field:

```rust
prompt_history: Option<PromptHistoryStore>,
```

If test configs need no filesystem writes, allow injecting a temp-backed store or make `for_config` deterministic under `NEO_HOME`.

- [ ] On load failure, push a status line such as `Prompt history unavailable: <error>` only in verbose/dev contexts if noisy startup messages are acceptable. Prefer silent failure if history is non-critical and tests cover it.

- [ ] Test:

```rust
#[tokio::test]
async fn controller_loads_workspace_prompt_history_on_startup() {
    // Create temp NEO_HOME/workspace bucket prompt-history.jsonl with two records.
    // Build controller_for_config.
    // Press Up from empty prompt.
    // Assert newest record appears.
}
```

- [ ] Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::controller_loads_workspace_prompt_history_on_startup
```

### Task 4: Append Submitted Prompts To Persistent History

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/neo-agent/src/modes/interactive.rs`

- [ ] Append after a real prompt is accepted, not for slash commands.

Good insertion point: after `NeoChromeState::submit_prompt()` returns `Some(prompt)` and after `PromptSubmission::from_text` resolves the actual user prompt. Persist the prompt text that becomes the user message, not provider/model prefix tokens if those are stripped by `PromptSubmission`.

- [ ] Include `self.active_session_id.as_deref()` when available. For unsaved sessions, pass `None`; after the turn starts and sends a session id, later prompts can include it.

- [ ] Do not fail prompt submission if history append fails. Show at most a status warning, and continue the turn.

- [ ] Tests:

```rust
#[tokio::test]
async fn submitted_prompt_is_persisted_to_workspace_history() {
    // Submit a real prompt.
    // Read prompt-history.jsonl.
    // Assert it contains the prompt text.
}

#[tokio::test]
async fn slash_commands_are_not_persisted_to_prompt_history() {
    // Submit /model or /resume.
    // Assert prompt-history.jsonl is absent or empty.
}
```

- [ ] Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::submitted_prompt_is_persisted_to_workspace_history
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::slash_commands_are_not_persisted_to_prompt_history
```

### Task 5: Cross-Session And Cross-Workspace Regression Tests

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] Test same workspace:

```rust
#[tokio::test]
async fn prompt_history_is_shared_across_sessions_in_same_workspace() {
    // Controller A submits "first from session a".
    // Controller B uses same config/project_dir and starts fresh.
    // Press Up in B.
    // Assert "first from session a" is recalled.
}
```

- [ ] Test different workspace:

```rust
#[tokio::test]
async fn prompt_history_is_isolated_by_workspace_bucket() {
    // Controller A project_dir one submits "workspace one".
    // Controller B project_dir two presses Up.
    // Assert it does not recall "workspace one".
}
```

- [ ] Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::prompt_history_is_shared_across_sessions_in_same_workspace
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::prompt_history_is_isolated_by_workspace_bucket
```

### Task 6: Blocking Dialog Regression Tests

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] Keep existing event routing intact. Add or extend tests proving Up/Down are consumed by focused overlays.

Suggested assertions:

```rust
#[tokio::test]
async fn approval_up_down_does_not_recall_prompt_history() {
    // Seed prompt history with "old prompt".
    // Focus approval overlay.
    // Press Up/Down.
    // Assert prompt text is still empty.
    // Assert approval selection changed as expected.
}

#[tokio::test]
async fn question_up_down_does_not_recall_prompt_history() {
    // Same idea for question dialog.
}
```

- [ ] Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent approval_uses_selection_priority_for_real_keys question_dialog_prioritizes_real_keybindings_before_prompt_editing
```

### Task 7: Documentation

**Files:**

- Modify: `docs/config.md` if slash/key behavior is documented there.
- Modify: `docs/sessions.md` if workspace-scoped history belongs with session storage.
- Modify: `docs/quickstart.md` only if useful.

- [ ] Document:

- Up/Down recall prompt history from an empty composer.
- History is workspace-scoped under the session bucket.
- Slash commands are not persisted.
- History is append-only JSONL and local-only.

- [ ] Run:

```bash
rtk cargo run -p xtask -- parity
```

## Verification Plan

Focused:

```bash
rtk cargo run -p xtask -- test -p neo-tui prompt_history
rtk cargo run -p xtask -- test -p neo-agent prompt_history
rtk cargo run -p xtask -- test -p neo-agent interactive::tests::event_loop_uses_up_down_keys_for_prompt_history
rtk cargo run -p xtask -- test -p neo-agent approval_uses_selection_priority_for_real_keys question_dialog_prioritizes_real_keybindings_before_prompt_editing
```

Before claiming completion:

```bash
rtk cargo run -p xtask -- test --workspace --all-features
rtk cargo run -p xtask -- coverage
rtk cargo run -p xtask -- crap
rtk cargo run -p xtask -- ci
```

Artifacts:

- `target/llvm-cov/lcov.info`
- `target/crap/crap-crates.md`
- `target/crap/crap-crates.json`
- `target/crap/crap-workspace.md`

## Easy-To-Miss Failure Modes

- Overwriting a non-empty draft on first Up.
- Persisting slash commands such as `/model`, `/resume`, or `/new`.
- Deduplicating globally instead of only consecutive duplicate prompts; repeated useful prompts separated by other prompts should remain.
- Reading sessions in session-id order instead of using prompt-history file order.
- Storing under a global path and leaking prompts across projects.
- Failing the user's prompt submission because history append failed.
- Letting approval/question/model/session picker Up/Down leak into `PromptState`.
- Loading unbounded history into memory.
- Rewriting the entire JSONL file on every prompt without a reason.
- Using direct `cargo test` as evidence.

## Self-Review Checklist For Implementer

- [ ] History path is workspace-scoped.
- [ ] Prompt history loads into a fresh controller.
- [ ] Successful prompts append to JSONL.
- [ ] Slash commands do not append to JSONL.
- [ ] Blank prompts are skipped.
- [ ] Consecutive duplicate prompts are skipped.
- [ ] Up from empty recalls newest prompt.
- [ ] First Up from non-empty draft does not overwrite it.
- [ ] Down restores the draft after navigating past newest.
- [ ] Blocking dialogs consume Up/Down before prompt history.
- [ ] Focused tests pass through `xtask`.
- [ ] Workspace tests, LCOV, CRAP, and CI were run through `xtask`.
- [ ] ICM store was called before final handoff.
