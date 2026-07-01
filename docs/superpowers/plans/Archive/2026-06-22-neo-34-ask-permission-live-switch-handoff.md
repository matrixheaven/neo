# NEO-34 Ask Permission Mode and Live Switching Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rename Neo's `manual` permission mode to `ask` and make `/ask`, `/auto`, `/yolo` update the active agent loop's permission behavior without cancelling or waiting for the current turn.

**Architecture:** Replace `PermissionMode::Manual` with `PermissionMode::Ask` across core, TUI, config, docs, and tests. Add shared live permission state to `AgentConfig` / `TurnRequest` so permission preparation reads the current mode at each tool call instead of only using the value copied at turn start. Reorder TUI slash-command handling so permission commands are dispatchable while `active_turn` is running.

**Tech Stack:** Rust 2024, `Arc<RwLock<PermissionMode>>` shared runtime state, Neo `AgentRuntime`, TUI slash command routing, `crossterm` overlays, `serde` config parsing, focused `nextest` through `cargo nextest run`.

---

## Linear Context

- Linear: [NEO-34](https://linear.app/neo-agent/issue/NEO-34/rename-manual-permission-mode-to-ask-and-allow-live-permission-changes)
- Title: Rename manual permission mode to ask and allow live permission changes during active turns
- Priority: High
- Project: Mode System
- Team: Neo
- Labels: Feature, Bug

## User Request

The user wants:

- `manual` permission mode renamed completely to `ask`, matching the existing `/ask` slash command.
- While the Agent Loop is running, `/ask`, `/auto`, and `/yolo` should dynamically change permission mode without interrupting the current turn.
- `/permissions` may also open its selector and change permission mode during an active turn if the current rendering/input-routing model allows it.
- The implementation should inspect current Neo code and borrow from `docs/kimi-code`.

This is both a naming cleanup and a runtime behavior fix.

## Current Neo Behavior

Current user-visible behavior:

- `/ask` exists but sets `PermissionMode::Manual`.
- Footer badge shows `[manual]`.
- Config docs recommend `permission_mode = "manual"`.
- Command palette says `permission.manual`.
- Tests assert `PermissionMode::Manual` and `[manual]`.

Current active-turn behavior:

- `submit_current_prompt` checks `active_turn.is_some()` before slash command dispatch for everything except `/new` and `/clear`.
- If a turn is running, typing `/ask`, `/auto`, or `/yolo` produces `A turn is already running`.
- `start_turn_with_prompt` copies `self.permission_mode` into `TurnRequest`.
- `controller_for_config` copies `request.permission_mode` into `effective_config.permission_mode`.
- `AgentRuntime` / `permission_preparation_for_mode` reads `config.permission_mode`, which is a static copy for that turn.

Therefore, even if UI state were changed during a turn, later tool calls in that same turn would not reliably use the updated mode.

## Kimi Code Reference

Read these files:

- `docs/kimi-code/apps/kimi-code/src/tui/commands/registry.ts`
- `docs/kimi-code/apps/kimi-code/src/tui/commands/config.ts`
- `docs/kimi-code/apps/kimi-code/src/tui/components/dialogs/permission-selector.ts`
- `docs/kimi-code/apps/kimi-code/src/tui/kimi-tui.ts`
- `docs/kimi-code/packages/node-sdk/src/session.ts`
- `docs/kimi-code/packages/node-sdk/src/rpc.ts`
- `docs/kimi-code/packages/agent-core/src/session/rpc.ts`
- `docs/kimi-code/packages/agent-core/src/agent/permission/index.ts`
- `docs/kimi-code/packages/agent-core/src/agent/injection/permission-mode.ts`

Important patterns to borrow:

- Slash commands for permission are `availability: 'always'`.
- TUI command handlers call `session.setPermission(mode)` even for existing sessions.
- SDK/RPC routes permission changes into the active agent/session.
- `PermissionManager` stores mutable mode override.
- Permission policies read `agent.permission.mode` at evaluation time.
- Status/replay emits `permission_updated`.
- UI app state updates immediately after successful set.

Neo does not need to copy Kimi's RPC architecture; it should borrow the state model: permission mode must be live runtime state, not a turn-start snapshot.

## Mandatory References

Read these before coding:

- `AGENTS.md`
- `~/.codex/RTK.md`
- `~/.codex/CX.md`
- `crates/neo-agent-core/src/permissions.rs`
- `crates/neo-agent-core/src/runtime.rs`
- `crates/neo-agent-core/tests/runtime_turn.rs`
- `crates/neo-agent/src/config.rs`
- `crates/neo-agent/src/modes/interactive.rs`
- `crates/neo-tui/src/chrome.rs`
- `crates/neo-tui/tests/app_shell.rs`
- `docs/config.md`
- `docs/tools.md`
- `docs/goals.md`

Run recall first:

```bash
rtk icm recall-context "Neo permission mode manual ask auto yolo live switching active turn slash command" --limit 5
```

## Non-Negotiable Project Rules

- Use `rtk` for shell commands.
- Prefer `cx` for symbol navigation before broad reads.
- Do not run bare `cargo test`; use `rtk cargo nextest run ...`.
- Do not perform git mutations unless the user gives explicit per-command authorization.
- Preserve unrelated worktree changes.
- Do not keep a long-term dual naming model. `ask` is the new name. `manual` may only exist as a documented migration read alias if needed.

## Product Decisions

- Canonical user-facing mode names are exactly: `ask`, `auto`, `yolo`.
- Rust enum variants should be exactly: `PermissionMode::Ask`, `PermissionMode::Auto`, `PermissionMode::Yolo`.
- Default mode is `Ask`.
- Footer badge is `[ask]`.
- Picker option label should be `Ask`, not `Manual`.
- Slash command `/ask` stays the direct command for Ask mode.
- Do not add `/manual`.
- Do not keep `permission.manual` command palette id. Replace it with `permission.ask`.
- New config should use `permission_mode = "ask"`.
- Existing `permission_mode = "manual"` may be accepted as a transitional read alias only if changing serde would otherwise break existing user config. It must not be emitted, documented as preferred, or shown in UI.
- Active-turn `/ask`, `/auto`, `/yolo` must not cancel, interrupt, enqueue, or send a model message.
- Active-turn `/permissions` should open the picker if rich overlays can be focused safely during an active turn. If implementation risk is high, first patch may block `/permissions` during active turns with a clear status, but `/ask` `/auto` `/yolo` are mandatory.

## Target UX

Idle footer:

```text
[ask] openai/gpt-4.1  cwd ~/Workspace/neo
```

After `/auto` while a turn is running:

```text
Permission Mode: auto
[auto] working  Esc interrupt  ...
```

After `/ask` while a turn is running:

```text
Permission Mode: ask
[ask] working  Esc interrupt  ...
```

Permission picker:

```text
╭─ Select permission mode ─────────────────────────╮
│ ▸ Ask   Ask before commands, edits, risky tools   │
│   Auto  Run non-interactively; skip questions     │
│   YOLO  Auto-approve tools; allow questions       │
╰──────────────────────────────────────────────────╯
```

## File Structure

Modify:

- `crates/neo-agent-core/src/permissions.rs`
  - Rename enum variant and label.
  - Add optional serde alias for reading legacy `manual`.

- `crates/neo-agent-core/src/runtime.rs`
  - Add shared live permission state.
  - Make permission preparation read live mode at each tool call.
  - Keep builder APIs ergonomic.

- `crates/neo-agent-core/tests/runtime_turn.rs`
  - Update `Manual` to `Ask`.
  - Add live-switch tests.

- `crates/neo-agent/src/config.rs`
  - Update defaults and tests.
  - Ensure `permission_mode = "ask"` loads.
  - Ensure legacy `manual` loads only if alias is intentionally supported.

- `crates/neo-agent/src/modes/interactive.rs`
  - Update slash routing.
  - Update picker items.
  - Update command palette ids.
  - Add active-turn dynamic switching tests.
  - Pass live permission state into turn requests.

- `crates/neo-tui/src/chrome.rs`
  - Update footer badge from manual to ask.

- `crates/neo-tui/tests/app_shell.rs`
  - Update footer tests.

- `docs/config.md`, `docs/tools.md`, `docs/goals.md`
  - Replace user-facing manual mode with ask mode.

Possibly modify:

- Any generated schema snapshots if present.
- Other tests found by `rtk rg "PermissionMode::Manual|\\[manual\\]|permission:manual|manual permission|permission_mode = \"manual\""`.

## Task 1: Rename `PermissionMode::Manual` to `PermissionMode::Ask`

**Files:**

- Modify: `crates/neo-agent-core/src/permissions.rs`

- [ ] Replace:

```rust
pub enum PermissionMode {
    Manual,
    Auto,
    Yolo,
}
```

with:

```rust
pub enum PermissionMode {
    #[serde(alias = "manual")]
    Ask,
    Auto,
    Yolo,
}
```

The serde alias is allowed only as a migration read path. Do not document `manual` as a supported new value.

- [ ] Replace label implementation:

```rust
Self::Manual => "manual",
```

with:

```rust
Self::Ask => "ask",
```

- [ ] Replace default:

```rust
Self::Manual
```

with:

```rust
Self::Ask
```

- [ ] Run a search and replace in code, not docs yet:

```bash
rtk rg -n "PermissionMode::Manual" crates
```

Replace every occurrence with `PermissionMode::Ask`.

- [ ] Run focused compile/tests after code rename:

```bash
```

If the test filter is too narrow, use:

```bash
```

Expected after updates: no compile errors from `PermissionMode::Manual`.

## Task 2: Update TUI Labels, Picker, Slash, and Command Palette

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-tui/src/chrome.rs`
- Modify: `crates/neo-tui/tests/app_shell.rs`

- [ ] Update `permission_mode_items`:

```rust
neo_tui::dialogs::ChoiceItem::new("permission:ask", "Ask")
    .with_description("Ask before commands, edits, and other risky actions. Read/search tools run directly; session approval rules are respected."),
```

- [ ] Update `slash_permission_mode`:

```rust
"/ask" => Some(PermissionMode::Ask),
```

- [ ] Update command handlers:

```rust
"permission.ask" => self.set_permission_mode(PermissionMode::Ask),
```

Remove `permission.manual`.

- [ ] Update `handle_builtin_choice_item` permission handling:

```rust
"permission:ask" => self.set_permission_mode(PermissionMode::Ask),
```

Remove `permission:manual`.

- [ ] Update command palette specs:

```rust
CommandSpec::new(
    "permission.ask",
    "Ask permission mode",
    Some("Ask before risky actions"),
),
```

- [ ] Update slash completion description for `/ask`:

```rust
Some("ask permission mode")
```

- [ ] Update `NeoChromeState::permission_badge`:

```rust
PermissionMode::Ask => ("ask", self.theme().footer_permission_ask),
```

- [ ] Update app shell tests:

```rust
app.set_permission_mode(neo_agent_core::PermissionMode::Ask);
assert!(lines.iter().any(|line| line.contains("[ask]")));
```

- [ ] Update interactive tests:
  - `slash_ask_sets_manual_permission_mode` -> `slash_ask_sets_ask_permission_mode`
  - Expected mode `PermissionMode::Ask`
  - Expected status `Permission Mode: ask`
  - Expected badge `[ask]`
  - Permission picker comment: "Move from Ask (index 0) to Auto (index 1)"

- [ ] Run focused TUI tests:

```bash
```

Expected: PASS.

## Task 3: Add Shared Live Permission State

**Files:**

- Modify: `crates/neo-agent-core/src/runtime.rs`

Current `AgentConfig` has:

```rust
pub permission_mode: PermissionMode,
```

This is a static value. Add live state:

```rust
#[serde(skip)]
#[schemars(skip)]
pub live_permission_mode: Arc<RwLock<PermissionMode>>,
```

- [ ] Initialize in `AgentConfig::for_model`:

```rust
permission_mode: PermissionMode::default(),
live_permission_mode: Arc::new(RwLock::new(PermissionMode::default())),
```

- [ ] Update `with_permission_mode` so static and live values stay in sync:

```rust
#[must_use]
pub fn with_permission_mode(mut self, mode: PermissionMode) -> Self {
    self.permission_mode = mode;
    self.live_permission_mode = Arc::new(RwLock::new(mode));
    self
}
```

This can no longer be `const fn`.

- [ ] Add builder for externally shared state:

```rust
#[must_use]
pub fn with_live_permission_mode(mut self, live_permission_mode: Arc<RwLock<PermissionMode>>) -> Self {
    let mode = live_permission_mode
        .read()
        .map(|guard| *guard)
        .unwrap_or(self.permission_mode);
    self.permission_mode = mode;
    self.live_permission_mode = live_permission_mode;
    self
}
```

- [ ] Add helper:

```rust
fn current_permission_mode(config: &AgentConfig) -> PermissionMode {
    config
        .live_permission_mode
        .read()
        .map(|guard| *guard)
        .unwrap_or(config.permission_mode)
}
```

- [ ] Replace every permission mode decision inside runtime with `current_permission_mode(config)`:

```rust
let mode = current_permission_mode(config);
if tool_call.name == "AskUserQuestion" && mode == PermissionMode::Auto { ... }
if mode == PermissionMode::Auto { ... }
if mode == PermissionMode::Yolo { ... }
```

Do not repeatedly lock for every branch; read once near the start of `permission_preparation_for_mode` after plan-mode guard.

- [ ] Keep `config.permission_mode` for serialization/snapshots and initial state, but permission evaluation must use live mode.

- [ ] Add unit/runtime test skeleton in `crates/neo-agent-core/tests/runtime_turn.rs`:

```rust
#[tokio::test]
async fn active_turn_permission_preparation_reads_live_permission_mode() {
    let live_mode = Arc::new(RwLock::new(PermissionMode::Ask));
    let mut config = AgentConfig::for_model(harness.model())
        .with_permission_mode(PermissionMode::Ask)
        .with_live_permission_mode(Arc::clone(&live_mode));

    // Arrange a model response that performs two risky tool calls with a pause
    // or approval hook between them. Change live_mode to Auto before the second
    // tool is prepared. The second tool must not request approval.
}
```

Use existing fake harness patterns in `runtime_turn.rs`; do not invent a new harness.

## Task 4: Carry Live State From TUI Into Running Turns

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] Add field to `InteractiveController`:

```rust
live_permission_mode: Arc<RwLock<PermissionMode>>,
```

Use `tokio::sync::RwLock` only if the surrounding code already prefers async locks. Otherwise use `std::sync::RwLock` to match `AgentConfig::plan_mode`.

- [ ] Initialize it from config/default:

```rust
let live_permission_mode = Arc::new(RwLock::new(PermissionMode::default()));
```

When config is applied:

```rust
self.permission_mode = config.permission_mode;
if let Ok(mut mode) = self.live_permission_mode.write() {
    *mode = config.permission_mode;
}
self.tui.chrome_mut().set_permission_mode(config.permission_mode);
```

- [ ] Update `set_permission_mode`:

```rust
fn set_permission_mode(&mut self, mode: PermissionMode) {
    self.permission_mode = mode;
    if let Ok(mut live) = self.live_permission_mode.write() {
        *live = mode;
    }
    self.tui.chrome_mut().set_permission_mode(mode);
    self.push_status(format!("Permission Mode: {}", mode.label()));
}
```

- [ ] Update `TurnRequest`:

```rust
pub live_permission_mode: Arc<RwLock<PermissionMode>>,
```

Set default in `TurnRequest::new` to `Arc::new(RwLock::new(PermissionMode::default()))`.

- [ ] In `start_turn_with_prompt`, set:

```rust
request.permission_mode = self.permission_mode;
request.live_permission_mode = Arc::clone(&self.live_permission_mode);
```

- [ ] In `controller_for_config`, when building `effective_config`, attach live state:

```rust
effective_config.permission_mode = request.permission_mode;
effective_config = effective_config.with_live_permission_mode(Arc::clone(&request.live_permission_mode));
```

If moving `effective_config` makes this awkward, set fields directly:

```rust
effective_config.permission_mode = request.permission_mode;
effective_config.live_permission_mode = Arc::clone(&request.live_permission_mode);
```

- [ ] Add a small helper to avoid lock boilerplate:

```rust
fn set_live_permission_mode(live: &Arc<RwLock<PermissionMode>>, mode: PermissionMode) {
    if let Ok(mut current) = live.write() {
        *current = mode;
    }
}
```

## Task 5: Let Permission Slash Commands Run During Active Turns

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`

Current `submit_current_prompt` blocks active turns before most slash commands.

- [ ] Add helper:

```rust
fn is_live_permission_slash(prompt: &str) -> bool {
    slash_permission_mode(prompt.trim()).is_some()
        || matches!(prompt.trim(), "/permissions" | "/permission")
}
```

- [ ] Reorder `submit_current_prompt` so permission commands are handled before the active-turn guard:

```rust
if matches!(prompt.as_str(), "/new" | "/clear") {
    self.handle_simple_slash_command(&prompt);
    return Ok(());
}

if is_live_permission_slash(&prompt) {
    self.handle_permission_slash_command(&prompt);
    return Ok(());
}

if self.active_turn.is_some() {
    self.push_status("A turn is already running");
    return Ok(());
}
```

- [ ] Ensure `handle_permission_slash_command` clears prompt and never submits a turn.

- [ ] Decide `/permissions` active-turn behavior:

Recommended first implementation:

- Allow opening the picker during active turn because it is a focused overlay and does not interact with the model.
- The active turn continues draining events while the overlay is open.
- Choosing a mode updates live permission state.

If tests show overlay blocks approval/question handling, temporarily degrade:

```rust
if self.active_turn.is_some() && matches!(prompt, "/permissions" | "/permission") {
    self.clear_submitted_prompt();
    self.push_status("Use /ask, /auto, or /yolo while a turn is running");
    return true;
}
```

But `/ask`, `/auto`, `/yolo` must always work during active turns.

- [ ] Add tests:

```rust
#[tokio::test]
async fn slash_auto_updates_permission_mode_while_turn_is_running() {
    let mut controller = running_turn_controller().await;
    assert!(controller.active_turn.is_some());

    controller.type_text("/auto");
    controller.handle_input_event(InputEvent::Submit).await.unwrap();

    assert!(controller.active_turn.is_some(), "turn should keep running");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
    assert!(transcript_has_status(&controller, "Permission Mode: auto"));
    assert!(!transcript_has_status(&controller, "A turn is already running"));
}
```

Add equivalent tests for `/ask` and `/yolo`.

## Task 6: Verify Active Turn Uses Updated Mode for Later Tool Calls

**Files:**

- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`
- Possibly modify: `crates/neo-agent/src/modes/interactive.rs`

This is the behavior that prevents a superficial UI-only fix.

- [ ] Build a runtime test with two risky tool calls.

Use existing fake model/harness patterns. The shape should be:

1. Start with Ask mode.
2. First risky tool call asks for approval.
3. Approval handler changes live permission mode to Auto and returns allow once.
4. Second risky tool call should run without a second approval.

Pseudo-code:

```rust
#[tokio::test]
async fn live_permission_switch_to_auto_affects_later_tool_calls_in_same_turn() {
    let live_mode = Arc::new(RwLock::new(PermissionMode::Ask));
    let approvals = Arc::new(Mutex::new(0usize));
    let approvals_for_handler = Arc::clone(&approvals);
    let live_for_handler = Arc::clone(&live_mode);

    let config = AgentConfig::for_model(harness.model())
        .with_permission_mode(PermissionMode::Ask)
        .with_live_permission_mode(Arc::clone(&live_mode))
        .with_async_approval_handler(move |_request| {
            let approvals = Arc::clone(&approvals_for_handler);
            let live = Arc::clone(&live_for_handler);
            async move {
                *approvals.lock().expect("approvals") += 1;
                *live.write().expect("live mode") = PermissionMode::Auto;
                PermissionApprovalDecision::AllowOnce
            }
        });

    // Run fake turn with two Bash/Edit calls.
    // Assert approvals == 1.
}
```

Adapt to exact fake harness APIs.

- [ ] Add inverse test:

1. Start Auto.
2. Before second risky tool call, switch live mode to Ask.
3. Second risky tool call should request approval.

If difficult to time inside a model stream, a unit-level test for `permission_preparation_for_mode` may be acceptable if that helper can be tested without making it public. Prefer behavior-level runtime test.

## Task 7: Update Config and Docs

**Files:**

- Modify: `crates/neo-agent/src/config.rs`
- Modify: `docs/config.md`
- Modify: `docs/tools.md`
- Modify: `docs/goals.md`

- [ ] Update config tests:

```rust
assert_eq!(config.permission_mode, PermissionMode::Ask);
```

- [ ] Add test for canonical config:

```rust
permission_mode = "ask"
```

Expected: loads `PermissionMode::Ask`.

- [ ] If using serde alias, add test for migration read:

```rust
permission_mode = "manual"
```

Expected: loads `PermissionMode::Ask`.

Do not add docs that present `manual` as preferred.

- [ ] Update docs/config.md:

```toml
permission_mode = "ask"
```

Allowed values:

- `"ask"` — Ask before commands, edits, and other risky actions.
- `"auto"` ...
- `"yolo"` ...

Slash commands:

- `/ask` — Switch to ask permission mode.

- [ ] Update docs/tools.md:
  - Replace `manual` with `ask` in runtime boundary text.
  - Update explanations: "In ask mode, the runtime emits ApprovalRequested..."

- [ ] Update docs/goals.md:
  - Footer examples should show `[ask]`, not `[manual]`.

- [ ] Search docs:

```bash
rtk rg -n "\\bmanual\\b|\\[manual\\]|PermissionMode::Manual|permission:manual|permission\\.manual" docs crates
```

Only unrelated uses of "manual" should remain, such as "manual invocation" in skills docs.

## Task 8: Focused Verification

Run focused tests:

```bash
rtk cargo fmt --all --check
```

If filters are too narrow:

```bash
```

Do not run full workspace CI unless this turns into a broader runtime refactor.

## Edge Cases and Pitfalls

- Do not only change labels. The active turn must read live permission state during later tool calls.
- Do not leave `PermissionMode::Manual` as an alias enum variant. The canonical Rust variant should be `Ask`.
- Do not leave footer as `[manual]`.
- Do not keep command palette id `permission.manual`.
- Do not block `/ask`, `/auto`, `/yolo` behind `active_turn.is_some()`.
- Do not cancel or interrupt the current turn when changing permission mode.
- Do not enqueue `/ask` as a user message or pending prompt.
- Do not persist new config as `manual`.
- Do not let `manual` appear in generated docs as a preferred value.
- Do not forget plan mode hard guards: `ask/auto/yolo` are permission posture; plan mode restrictions still supersede them.
- Be careful with active approval prompts. If a prompt is already displayed, changing to `auto` should affect future approvals, not necessarily auto-resolve an approval already waiting. Document/test that boundary.
- If `/permissions` overlay during active turn conflicts with approval/question overlays, keep direct `/ask` `/auto` `/yolo` live and defer active-turn picker support with a clear status.

## Self Review Checklist

Before final handoff/PR, verify:

- [ ] No `PermissionMode::Manual` remains in code.
- [ ] Default permission mode is `Ask`.
- [ ] `PermissionMode::Ask.label()` returns `ask`.
- [ ] Footer shows `[ask]`.
- [ ] `/ask` status says `Permission Mode: ask`.
- [ ] Picker row says `Ask`.
- [ ] Command palette id is `permission.ask`.
- [ ] `permission_mode = "ask"` loads.
- [ ] Legacy `manual` config behavior is intentionally decided and tested.
- [ ] `/ask`, `/auto`, `/yolo` work while `active_turn` is running.
- [ ] These commands do not cancel/intercept the running turn.
- [ ] Active turn tool permission evaluation reads live state.
- [ ] Switching ask -> auto affects later tool calls in same turn.
- [ ] Switching auto -> ask affects later tool calls in same turn.
- [ ] Docs use ask/auto/yolo.
- [ ] Focused tests ran with direct cargo commands.

## Suggested Implementation Order

1. Rename enum variant and update compile failures.
2. Update TUI labels/picker/command ids/tests.
3. Add live permission state to runtime config.
4. Thread live permission state from interactive controller into turn config.
5. Reorder slash handling so permission commands bypass active-turn guard.
6. Add active-turn slash tests.
7. Add live-mode runtime tests.
8. Update docs.
9. Run focused verification.

## Implementation Notes for the Next AI

The hard part is not the rename. The hard part is preventing a UI-only fix that still leaves running turns on the old permission mode. Keep asking this question while coding: "When a risky tool is prepared five seconds after the user typed `/auto`, which state does `permission_preparation_for_mode` read?" If the answer is a copied `TurnRequest.permission_mode`, the implementation is incomplete.

