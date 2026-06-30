# NEO-24 Message Queue And Ctrl+S Steer Handoff Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish Neo's active-turn input UX so users can continue typing while a model turn is running. Enter during an active turn enqueues the composer text as a FIFO follow-up. Ctrl+S steers immediately. Ctrl+S source priority is strict: composer text first, then queued items in FIFO order.

**Architecture:** Neo already has runtime-level `steering_queue` and `follow_up_queue` support in `neo-agent-core`; this task should complete the product surface around that foundation. Keep the runtime append-only, route active-turn TUI input into the running runtime through an explicit side channel, render queue state in the TUI, and preserve JSONL replay semantics.

**Tech Stack:** Rust 2024, `tokio`, `crossterm`, `neo-agent-core` runtime events, `neo-tui` transcript/chrome rendering, JSONL session replay, /`nextest`/`llvm-cov`/CRAP gates.

---

## Linear Context

- Linear: [NEO-24](https://linear.app/neo-agent/issue/NEO-24/implement-message-queue-and-message-steer-for-long-running-agent)
- Priority: Urgent
- Project: Mode System
- Summary: Implement message queue and message steer for long-running agent workflows.
- Recommended order: implement before NEO-26 and NEO-27.
- Why first: active-turn queue and steer affect the runtime, JSONL event replay, transcript state, keybindings, and live TUI input flow.

## Mandatory References

Before coding, read:

- `AGENTS.md`
- `~/.codex/RTK.md`
- `~/.codex/CX.md`
- `docs/kimi-code/apps/kimi-web/test/steer.test.ts`
- `docs/kimi-code/apps/kimi-web/src/composables/useKimiWebClient.ts`
- `docs/kimi-code/apps/kimi-web/src/components/QueuePane.vue`
- `docs/kimi-code/apps/kimi-web/src/components/ComposerBar.vue`
- `docs/kimi-code/apps/kimi-web/src/i18n/index.ts`

Use the Kimi files as UX precedent, not as code to copy blindly. The useful precedent is:

- normal send while a turn is running becomes a queue operation;
- Ctrl+S is the steer shortcut;
- queued prompts are shown as an ordered list;
- steering injects work into the active turn instead of waiting for the turn to finish.

Neo-specific override:

- Ctrl+S must choose composer content before queued content.
- If the composer is empty, Ctrl+S chooses the oldest queued item.
- Do not add new queue or steer slash-command surfaces.

## Non-Negotiable Project Rules

- Before coding, run:

```bash
rtk icm recall-context "NEO-24 message queue ctrl-s steer implementation" --limit 5
```

- Use `rtk` for shell commands.
- Use `cx` before broad file reads when navigating symbols.
- Do not run bare `cargo test`; use `cargo nextest run ...` through `rtk`.
- Do not perform git mutations unless the user gives explicit per-command authorization. This includes `git add`, `git commit`, `git push`, `git switch`, `git checkout`, `git reset`, `git stash`, `git clean`, `git rm`, `git merge`, and `git rebase`.
- Do not preserve obsolete compatibility branches or duplicate models. Make one clean model.
- Stay inside NEO-24 scope. Do not fix unrelated failures.
- If a meaningful error is resolved, store it with ICM before final response.
- When the task is complete, store a significant-task memory before final response:

```bash
rtk icm store -t context-neo -c "Completed NEO-24: active-turn queue and Ctrl+S steer UX, including Enter FIFO follow-up queueing, composer-first steer priority, TUI queue rendering, JSONL replay, tests, coverage, CRAP, and CI evidence." -i high -k "NEO-24,message-queue,steer,ctrl-s,tui"
```

## Current Code Map

### Runtime Queue Foundation

- `crates/neo-agent-core/src/runtime.rs`
  - `QueueMode`
  - `AgentConfig::steering_queue_mode`
  - `AgentConfig::follow_up_queue_mode`
  - `AgentConfig::with_queue_modes`
  - `AgentContext::queue_steering_message`
  - `AgentContext::queue_follow_up_message`
  - `AgentContext::pending_steering_len`
  - `AgentContext::pending_follow_up_len`
  - `AgentContext::from_replay`
  - `AgentContext::apply_replay_queue_event`
  - `run_agent_turn`
  - `drain_steering_queue`
  - `drain_follow_up_queue`
  - `drain_next_pending_queue`
  - `append_queued_messages`

- `crates/neo-agent-core/src/events.rs`
  - `AgentEvent::SteeringQueued`
  - `AgentEvent::FollowUpQueued`
  - `AgentEvent::QueueDrained`
  - `QueueKind::{Steering, FollowUp}`

- Existing focused tests:
  - `crates/neo-agent-core/tests/runtime_turn.rs`
    - `runtime_drains_queued_steering_before_followups`
  - `crates/neo-agent-core/tests/session_jsonl.rs`
    - queue replay coverage around `SteeringQueued`, `FollowUpQueued`, and `QueueDrained`

### TUI Submit And Active Turn Flow

- `crates/neo-agent/src/modes/interactive.rs`
  - `InteractiveController::submit_current_prompt`
    - Current behavior: if `active_turn.is_some()`, it pushes a "turn already running" message and returns.
    - Replace this rejection with active-turn queue behavior.
  - `InteractiveController::start_turn_with_prompt`
    - Creates `RunningTurn` with event, approval, session id, question, and cancel channels.
  - `RunningTurn`
    - Needs an input side channel so active-turn submissions reach the running runtime instead of becoming controller-only state.
  - `InteractiveController::drain_active_turn`
    - Reduces runtime events into transcript/chrome state.
  - `InteractiveController::apply_turn_event`
    - Applies each `AgentEvent` to transcript and chrome.

### TUI Rendering And Input

- `crates/neo-tui/src/input.rs`
  - Add a default Ctrl+S prompt action, for example `PromptSteer`.
- `crates/neo-tui/src/chrome.rs`
  - `NeoChromeState::apply_agent_event`
  - `NeoChromeState::working_label`
  - `NeoChromeState::set_custom_working_label`
  - `PromptState`
- `crates/neo-tui/src/transcript/pane.rs`
  - `TranscriptPane::apply_agent_event`
  - `TranscriptPane::apply_queue_event`
    - Currently pushes plain status lines such as `Steering queued: ...`, `Follow-up queued: ...`, and `... queue drained`.
  - `TranscriptPane::push_status`
  - `TranscriptPane::push_user_message`
- `crates/neo-tui/src/transcript/entry.rs`
  - Add a structured queue/steer card variant only if the existing status/card model cannot render the queue clearly.

### Streaming Persistence

- `crates/neo-agent/src/modes/run.rs`
  - `run_prompt_streaming`
  - `run_prompt_in_session_streaming`
  - `finish_prompt_turn_streaming`
  - `append_streaming_event`
  - Runtime-emitted queue events flow through this path into JSONL. Do not write queue events directly from TUI code as a shortcut.

## Product Behavior

### Idle Turn

When no model turn is running:

- Enter submits composer text as a normal user prompt.
- Ctrl+S with composer text behaves like a normal send. This matches Kimi's "steer degrades to send while idle" behavior.
- Ctrl+S with an empty composer and an empty queue is a no-op, ideally with a tiny status message.
- If a queue exists while idle because of a race or restore edge case, drain it FIFO using the same path as regular queued follow-ups.

### Active Turn: Enter

When a model turn is running:

- Enter with non-empty composer text enqueues that text as a follow-up.
- The composer clears immediately.
- The queued item appears in the queue strip/list.
- The active model stream continues uninterrupted.
- Queued follow-ups run FIFO after the active turn completes, unless the user steers one earlier with Ctrl+S.

### Active Turn: Ctrl+S

When a model turn is running, Ctrl+S chooses exactly one source:

1. If the composer has non-empty text, steer that composer text immediately and clear the composer.
2. Else, if the queue has one or more items, pop the oldest queued item and steer it immediately.
3. Else, no-op with a small status message.

This ordering is non-negotiable:

```text
composer text > queue[1] > queue[2] > queue[3] > ...
```

If composer text is steered while queued follow-ups exist, leave the queue intact. Do not merge composer text with queued text, because the requested priority model treats the composer as the immediate steer payload and the queue as a FIFO backlog.

### Active Turn: Queue Drain After Completion

After a running turn completes:

- If the follow-up queue still has items, drain them FIFO.
- Starting the next queued turn should use the same normal turn path as user-submitted prompts.
- Preserve transcript ordering so the user can understand which input was queued, which was steered, and which ran after completion.

### Blocking Dialogs

When a blocking dialog is focused:

- Enter, paste, arrows, delete, escape, and Ctrl+S must route to the dialog first.
- Composer input must not leak into `PromptState`.
- Ctrl+S must not steer from approval dialogs, model/session/provider pickers, plan review, goal review, or Ask User dialogs.

This follows the blocking dialog contract in `AGENTS.md`.

## Architecture Plan

### Active Turn Side Channel

Running-turn input must reach the runtime that owns `AgentContext`.

Recommended shape:

```rust
pub enum ActiveTurnInput {
    EnqueueFollowUp { id: String, message: AgentMessage },
    SteerNow { id: String, message: AgentMessage },
    SteerOldestQueued,
}
```

Add an `mpsc::UnboundedSender<ActiveTurnInput>` to `RunningTurn`/`TurnChannels`, then pass the receiver through `run_prompt_streaming` and `run_prompt_in_session_streaming` into `AgentRuntime`. Runtime should poll this side channel at natural boundaries and emit append-only queue events.

The exact type can differ, but keep these semantics explicit:

- Enter sends `EnqueueFollowUp`.
- Ctrl+S with composer text sends `SteerNow`.
- Ctrl+S without composer text asks the active queue controller to pop the oldest queued item and steer it.

### Queue State Model

Use one state model across runtime and UI. Avoid separate compatibility models.

Recommended item fields:

```rust
pub struct QueuedInput {
    pub id: String,
    pub kind: QueueKind,
    pub message: AgentMessage,
    pub state: QueueItemState,
    pub created_at_ms: u64,
}

pub enum QueueItemState {
    Queued,
    Injected,
    Processed,
    Cancelled,
}
```

If adding this full type creates too much churn, keep the stored runtime queue as `Vec<AgentMessage>` for this task and add stateful UI bookkeeping in `neo-agent` only. However, do not invent two names for the same concept. The preferred end state is one public queue item model in `neo-agent-core`.

### Runtime Boundary Semantics

Natural injection boundaries:

- Before the next model request at the beginning of `run_agent_turn`.
- After assistant completes without tool calls, before deciding whether to end the run.
- After a tool batch finishes and tool results have been appended.
- After an `EnterPlanMode` continuation when the runtime already loops.

Do not inject steer messages in the middle of:

- shell/terminal tool execution;
- approval dialogs;
- Ask User blocking dialogs;
- partial model text deltas;
- unfinished tool call arguments.

The existing `run_agent_turn` flow already drains steering before follow-ups and around tool batches. The implementation should add missing state/events/UI, not bypass this logic.

### Runtime As Source Of Truth

The runtime should own durable queue events and replay state:

- Queue events should be appended through the existing JSONL writer path.
- Replay should reconstruct pending queue state.
- Queue drained events should be emitted when items move into an active prompt or active steer payload.
- TUI state should be derived from runtime events where possible.

Avoid a UI-only queue that disappears on redraw, resume, or streaming race.

### Race Handling

There are two important races:

- User presses Enter or Ctrl+S exactly as the active turn ends.
- A queued item is popped for steer while the runtime is transitioning to the next turn.

Acceptable behavior:

- If still active, steer into the active turn.
- If already idle, submit the selected text as the next normal turn.
- Never drop text.
- Never duplicate text.

## TUI Design

Keep this utilitarian and close to existing Neo transcript styling. Do not make a new route or command mode. The queue should feel like part of the active conversation surface.

### Footer While Active With Empty Queue

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ [manual] [normal]  working  Esc interrupt  Enter queue  Ctrl+S steer       │
└────────────────────────────────────────────────────────────────────────────┘
```

### Footer While Active With Queue

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ [manual] [normal]  working  queue 3  Enter queue  Ctrl+S steer next        │
└────────────────────────────────────────────────────────────────────────────┘
```

### Compact Queue Strip

Place this near the bottom of the transcript or above the composer, depending on the existing chrome layout. It should not steal focus from the composer.

```text
╭─ queue · 3 waiting ────────────────────────────────────────────────────────╮
│ 1  Add a regression for active turn Enter queueing                         │
│ 2  Then update the transcript queue card rendering                         │
│ 3  Finally run focused runtime and TUI tests                               │
╰──────────────────────────────────────────── Enter adds · Ctrl+S steers next╯
```

Rules:

- Single-line preview per queued item.
- Truncate with ellipsis according to terminal width.
- Stable row heights so queue updates do not jump the layout.
- Show ordinal numbers based on FIFO order.
- Do not show hints for slash commands.

### Queue Overflow

For more than four visible items:

```text
╭─ queue · 8 waiting ────────────────────────────────────────────────────────╮
│ 1  Add a regression for active turn Enter queueing                         │
│ 2  Then update the transcript queue card rendering                         │
│ 3  Finally run focused runtime and TUI tests                               │
│ 4  Update docs for Ctrl+S steer semantics                                  │
│    +4 more                                                                 │
╰──────────────────────────────────────────── Enter adds · Ctrl+S steers next╯
```

### Steering From Composer

When Ctrl+S steers composer text:

```text
╭─ steer · composer ─────────────────────────────────────────────────────────╮
│ Please also inspect the replay path before changing the UI only.           │
╰────────────────────────────────────────────────────────────────────────────╯
```

### Steering From Queue

When Ctrl+S steers the oldest queued item:

```text
╭─ steer · queue #1 ─────────────────────────────────────────────────────────╮
│ Add a regression for active turn Enter queueing.                           │
╰────────────────────────────────────────────────────────────────────────────╯
```

After the card appears, renumber the remaining queue:

```text
╭─ queue · 2 waiting ────────────────────────────────────────────────────────╮
│ 1  Then update the transcript queue card rendering                         │
│ 2  Finally run focused runtime and TUI tests                               │
╰──────────────────────────────────────────── Enter adds · Ctrl+S steers next╯
```

### Empty Ctrl+S Status

```text
╭─ steer ────────────────────────────────────────────────────────────────────╮
│ Nothing to steer. Type a message or queue one first.                       │
╰────────────────────────────────────────────────────────────────────────────╯
```

Use an existing transient status mechanism if Neo has one. Avoid adding a persistent transcript entry for harmless no-ops unless the codebase already treats no-ops that way.

## Implementation Tasks

### Task 1: Write Runtime Tests First

- [ ] Add focused tests before implementation.
- [ ] Cover active turn Enter queueing instead of rejection.
- [ ] Cover queued follow-ups draining FIFO after the active turn.
- [ ] Cover Ctrl+S with composer text steering composer text before queued content.
- [ ] Cover Ctrl+S with empty composer steering the oldest queued item.
- [ ] Cover remaining queued items keeping FIFO order after one item is steered.
- [ ] Cover turn-end race without dropped or duplicated text.
- [ ] Cover JSONL replay reconstruction of queue state and drained queue state.

Likely homes:

- `crates/neo-agent-core/src/runtime.rs` unit tests for queue state and replay.
- `crates/neo-agent/tests/interactive*.rs` or nearby integration tests for interactive behavior.
- `crates/neo-tui/tests/*` for prompt/keybinding/chrome behavior if the existing harness supports it.

Candidate test names:

```rust
#[tokio::test]
async fn active_turn_enter_enqueues_followup_fifo() {}

#[tokio::test]
async fn ctrl_s_with_composer_text_steers_text_before_queue() {}

#[tokio::test]
async fn ctrl_s_with_empty_composer_steers_oldest_queued_prompt_fifo() {}

#[tokio::test]
async fn ctrl_s_when_idle_submits_normally() {}

#[tokio::test]
async fn blocking_dialog_ctrl_s_does_not_steer() {}

#[tokio::test]
async fn queued_prompts_flush_fifo_after_turn_finishes() {}
```

### Task 2: Add Ctrl+S Keybinding

- [ ] Add a dedicated prompt action such as `PromptSteer`.
- [ ] Bind it to Ctrl+S by default.
- [ ] Route it through the existing keybinding customization system.
- [ ] Ensure focused blocking dialogs consume or override it before prompt handling.
- [ ] Add tests for default binding and dialog precedence.

Search targets:

- `crates/neo-tui/src/input.rs`
- `crates/neo-tui/src/chrome.rs`
- `crates/neo-agent/src/modes/interactive.rs`

Watch for terminal flow-control behavior on some shells. If raw mode does not reliably deliver Ctrl+S, document the terminal limitation and keep the binding configurable.

### Task 3: Change Active-Turn Submit Routing

- [ ] In `submit_current_prompt`, preserve normal submit behavior when no turn is active.
- [ ] If a turn is active and Enter is pressed, enqueue the prompt instead of rejecting it.
- [ ] Clear the composer after successful enqueue.
- [ ] Append queue events through the active session writer.
- [ ] Update chrome/transcript state from emitted events.
- [ ] Keep attachments behavior explicit. If attachments are not supported for queued follow-ups yet, reject with a clear message instead of silently losing them.

### Task 4: Implement Ctrl+S Routing

- [ ] Add a handler similar to `submit_current_prompt`, but with steer semantics.
- [ ] If composer has non-empty text, send text as immediate steering input, clear composer, and keep existing queue untouched.
- [ ] If composer is empty and queue has items, pop the oldest queued item and steer that item.
- [ ] Emit queue drained/update events when an item is removed from the queue for steer.
- [ ] If composer and queue are both empty, show a no-op status.
- [ ] If the runtime is idle by the time the handler fires, submit selected text as a normal prompt.

Prefer a single internal helper for "selected text should become next model input" so the idle race path and normal submit path do not diverge.

### Task 5: Render Queue State

- [ ] Replace status-only queue feedback with a compact queue strip/list.
- [ ] Show queue count.
- [ ] Show FIFO ordinals.
- [ ] Show truncated text preview.
- [ ] Update after enqueue, steer, drain, and replay.
- [ ] Avoid focus changes.
- [ ] Avoid layout jumps.

Likely edit targets:

- `crates/neo-tui/src/transcript/pane.rs`
- `crates/neo-tui/src/chrome.rs`
- `crates/neo-agent/src/modes/interactive.rs`

If there is already a generalized status card or tool card component, use it. Do not add a separate full-screen queue manager.

### Task 6: Persist And Replay Queue Events

- [ ] Ensure JSONL session replay knows what was queued.
- [ ] Ensure replay knows what was steered from composer.
- [ ] Ensure replay knows what was steered from the queue.
- [ ] Ensure replay knows what remained pending after a turn.
- [ ] Ensure replay knows what drained after a turn.

The exact event taxonomy can reuse existing `AgentEvent` variants if sufficient. If not, add minimal metadata rather than duplicate event concepts.

Suggested metadata:

- queue kind: follow-up or steering;
- source: composer or queued item;
- item id or sequence number;
- preview text if event bodies are already persisted elsewhere.

### Task 7: Documentation

- [ ] Document that active-turn Enter queues follow-ups.
- [ ] Document that Ctrl+S steers.
- [ ] Document Ctrl+S priority: composer text, then queued items FIFO.
- [ ] Document that queued follow-ups run after the active turn if not steered.
- [ ] Document that blocking dialogs keep focus and do not steer.
- [ ] Do not document any new slash commands for this feature.

Suggested files:

- `docs/quickstart.md`
- `docs/config.md` if keybindings are documented there.
- `docs/tui.md` or the closest TUI interaction doc if one exists.

## Verification Plan

Required focused checks:

```bash
```

Adjust filters to actual test names after adding tests. Do not use bare `cargo test` as completion evidence.

Required repository gates before final completion:

```bash
rtk cargo llvm-cov nextest --workspace --all-features
rtk cargo crap
rtk cargo nextest run --workspace --all-features
```

Artifacts to inspect:

- `target/llvm-cov/lcov.info`
- `target/crap/crap-crates.md`
- `target/crap/crap-crates.json`

## Easy-To-Miss Pitfalls

- Do not add queue or steer slash commands. The product direction is Enter and Ctrl+S.
- Do not leave the existing "turn already running" rejection path for Enter during active turns.
- Do not merge composer text with queued prompts on Ctrl+S. Composer text wins and queues remain untouched.
- Do not silently drop queued attachments. Either support them or reject clearly.
- Do not update only the TUI. Runtime/session replay must understand the queue.
- Do not let Ctrl+S leak out of focused blocking dialogs.
- Do not make queue order depend on render order. Use a monotonic sequence or stable deque.
- Do not let terminal width changes cause row height churn or overlapping footer/composer text.
- Do not add compatibility branches that preserve obsolete active-turn rejection behavior.
- Do not use direct git mutation commands. The repository forbids them without explicit per-command authorization.

## Self-Review Checklist

- [ ] Enter during active turn enqueues and clears composer.
- [ ] Ctrl+S with composer text steers that text before any queued item.
- [ ] Ctrl+S with empty composer steers the oldest queued item.
- [ ] Queued items remain FIFO after one item is steered.
- [ ] Idle Ctrl+S with text behaves like normal send.
- [ ] Blocking dialogs keep all input, including Ctrl+S, away from prompt routing.
- [ ] Queue state survives JSONL replay.
- [ ] Transcript/chrome rendering shows count and FIFO order clearly.
- [ ] Focused tests cover runtime, interactive routing, and rendering.
- [ ] Verification commands ran with direct cargo commands.

## Suggested ICM Store On Completion

After completing and verifying implementation, store:

```bash
rtk icm store -t context-neo -c "Implemented NEO-24 active-turn queue and Ctrl+S steer UX: Enter enqueues FIFO follow-ups, Ctrl+S steers composer text first or oldest queued item, queue state renders in TUI and persists through JSONL replay." -i high -k "NEO-24,queue,steer,ctrl-s,tui,runtime"
```
