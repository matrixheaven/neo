# NEO-28 `/btw` Sidecar Dialog Handoff Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `/btw` as an ephemeral sidecar conversation for temporary questions that can see the current projected context without polluting the main session transcript, JSONL history, or active main turn.

**Architecture:** Prefer the Kimi Code design: create a same-session, in-memory side agent from a stable snapshot of the parent projected history, keep parent tool definitions for prompt-cache prefix stability, deny all side-agent tool calls, and render the side conversation in a bounded TUI panel between the transcript and composer. Do not create a persistent session or append sidecar messages to the main JSONL file.

**Tech Stack:** Rust 2024, `tokio`, `crossterm`, `neo-agent-core` runtime/context/types, `neo-agent` interactive mode, `neo-tui` transcript/chrome rendering, Kimi Code `/btw` reference implementation, /`nextest`/`llvm-cov`/CRAP gates.

---

## Linear Context

- Linear: [NEO-28](https://linear.app/neo-agent/issue/NEO-28/neo-28-implement-btw-sidecar-dialog-for-temporary-questions-without)
- Title: Implement `/btw` sidecar dialog for temporary questions without polluting main context
- Priority: High
- Project: CLI Commands
- Related issue: NEO-20 Multi-Agent system. Implement NEO-28 as a deliberately small same-session sidecar. If NEO-20 is already complete by implementation time, reuse only its low-level in-memory agent primitives and keep the `/btw` product behavior defined here.
- User-facing promise: ask a quick side question, get an answer using current context, close with Esc, and leave the main session untouched.

## Mandatory References

Read before implementation:

- `AGENTS.md`
- `~/.codex/RTK.md`
- `~/.codex/CX.md`
- `docs/kimi-code/apps/kimi-code/src/tui/controllers/btw-panel.ts`
- `docs/kimi-code/apps/kimi-code/src/tui/components/panes/btw-panel.ts`
- `docs/kimi-code/apps/kimi-code/src/tui/kimi-tui.ts`
- `docs/kimi-code/packages/agent-core/test/session/init.test.ts`
- `docs/kimi-code/apps/kimi-web/src/composables/useKimiWebClient.ts`
- `docs/kimi-code/apps/kimi-web/src/components/SideChatPanel.vue`
- `docs/kimi-code/apps/kimi-web/test/side-chat.test.ts`
- `docs/kimi-code/apps/kimi-web/test/side-chat-panel.test.ts`

Important Kimi behaviors to preserve in Neo:

- `/btw <question>` opens a temporary side panel and immediately asks the side agent.
- Bare `/btw` opens an empty panel waiting for input.
- The side agent inherits a projected parent history snapshot.
- The side agent is in-memory only.
- Side messages are filtered out of the main transcript.
- Side tool calls are denied even though tool definitions remain present for prompt-cache reasons.
- The panel height is capped to roughly one third of terminal rows.
- Esc closes the panel; if it is running or empty, close also cancels the side agent.
- While the side panel is running, another side prompt shows a busy notice rather than starting a second concurrent side turn.

## Product Decisions

### Recommended Context Strategy

Use the Kimi-style same-session side agent.

Why:

- It best preserves provider prompt-cache prefix stability because the same model, system prompt, and tool definitions remain in the request shape.
- It avoids a persistent side session appearing in `neo sessions list`.
- It allows `/btw` to be visually and semantically a temporary panel, not a forked workspace.
- It avoids main JSONL pollution.

Do not start with the Codex-style persistent fork-session approach. It is useful as a conceptual comparison, but it creates a new session identity and risks losing the same prompt-cache-key behavior that NEO-28 explicitly calls out as the sensitive design point.

### Tool Policy

Sidecar agents must not use tools.

Keep tool definitions in the side request for prompt-cache stability, but intercept every tool call before execution and return an error tool result:

```text
Tool calls are disabled for side questions. Answer with text only.
```

This matches the Kimi `DenyAllPermissionPolicy` behavior and keeps the model request shape stable without letting the temporary side chat mutate workspace state, ask user questions, run shell commands, or read files.

### Persistence Policy

The sidecar conversation is in-memory only:

- no JSONL session event writes;
- no session metadata writes;
- no `sessions.metadata.json` entry;
- no `session_index.jsonl` entry;
- no main transcript replay contribution;
- no prompt history persistence unless the user explicitly submits `/btw ...` as a slash command history item and the prompt-history implementation intentionally stores slash commands, which NEO-27 should not do.

When the user closes the panel, discard sidecar state.

### Main Turn Isolation

`/btw` is not message queueing and not steering:

- It does not enqueue a follow-up for NEO-24.
- It does not steer the active main turn.
- It may run while the main turn is streaming if implementation supports a separate side runtime task.
- It reads from a parent context snapshot at creation time; later main-turn deltas do not silently mutate the sidecar context.

## Current Neo Code Map

### Interactive Slash Commands

- `crates/neo-agent/src/modes/interactive.rs`
  - `InteractiveController::handle_slash_command`
  - `InteractiveController::handle_simple_slash_command`
  - `InteractiveController::submit_current_prompt`
  - `InteractiveController::start_turn_with_prompt`
  - `InteractiveController::drain_active_turn`
  - `session_completion_items`
  - `command_specs`

Add `/btw` handling here. Unlike normal prompts, `/btw` must not call the main `start_turn_with_prompt` path.

### Runtime And Context

- `crates/neo-agent-core/src/runtime.rs`
  - `AgentRuntime`
  - `AgentContext`
  - `AgentConfig`
  - `ContextTransform`
  - `BeforeToolCallHook`
  - `AsyncBeforeToolCallHook`
  - `drop_incomplete_trailing_tool_turn` currently exists privately and is relevant to sidecar projection.
- `crates/neo-agent-core/src/messages.rs`
  - `AgentMessage`
  - `Content`
- `crates/neo-agent-core/src/session/mod.rs`
  - `replay_messages`
  - replay/summarization helpers

The sidecar needs a projected context snapshot, not the live mutable parent context.

### Run Helpers

- `crates/neo-agent/src/modes/run.rs`
  - `agent_config_for_app`
  - `run_prompt_streaming`
  - `run_prompt_in_session_streaming`
  - `finish_prompt_turn_streaming`
  - `append_streaming_event`

Do not reuse the JSONL-writing helpers for sidecar output. Reuse configuration/model construction where possible, but keep sidecar events in memory.

### TUI Rendering

- `crates/neo-tui/src/chrome.rs`
  - `PromptState`
  - overlay/prompt blocking helpers
- `crates/neo-tui/src/components.rs`
  - layout heights
- `crates/neo-tui/src/transcript/pane.rs`
  - existing transcript patterns for streaming text, thinking, tool cards, approval prompts
- `crates/neo-tui/src/transcript/entry.rs`
  - rendering style references

Kimi renders BTW between transcript and editor. Neo should do the same with a compact panel, not a separate page.

## UX Design

### Slash Command Behavior

```text
/btw
/btw why is the runtime replay dropping this tool result?
```

Rules:

- Bare `/btw` opens an empty sidecar panel.
- `/btw <question>` opens the panel and submits `<question>` immediately.
- If a sidecar panel already exists and is idle, `/btw <question>` replaces it with a fresh sidecar snapshot before asking.
- If a sidecar panel is running, a new `/btw <question>` cancels the old sidecar and opens a new one.
- Bare `/btw` toggles the panel open/closed when idle.
- Bare `/btw` while the sidecar is running keeps the panel open and shows a busy notice; Esc is the only close/cancel gesture for a running sidecar.

### Composer Routing While Panel Is Open

Recommended Kimi-compatible behavior:

- While the BTW panel is open, the main composer is connected to the sidecar panel.
- Pressing Enter with text sends that text as a sidecar follow-up if the sidecar is idle.
- If the sidecar is running, preserve the typed text in the composer and show a busy notice in the BTW panel.
- Press Esc to close the sidecar and return the composer to the main conversation.

This keeps the sidecar implementation small: no separate nested input widget is needed.

### Panel Placement

Render order:

```text
main transcript
activity / todo / queue panels if present
BTW panel
composer
footer
```

The BTW panel must not be a full-screen modal and must not live in a separate page or side tab.

### Empty Panel

```text
╭─ BTW ─ Esc close ──────────────────────────────────────────────────────────╮
│ Ready for a side question...                                               │
╰────────────────────────────────────────────────────────────────────────────╯

[manual] [normal] session 20260622-1430 · gpt-4.1
> why does the replay path skip the final tool result?
```

### Running With Thinking Preview

```text
╭─ BTW ─ Esc close · ↑↓ scroll ──────────────────────────────────────────────╮
│ Q: why does the replay path skip the final tool result?                    │
│ Thinking about the replay reducer and open tool-call boundary...           │
│ Waiting for answer...                                                      │
╰────────────────────────────────────────────────────────────────────────────╯

[manual] [normal] session 20260622-1430 · gpt-4.1
>
```

### Answered

```text
╭─ BTW ─ Esc close ──────────────────────────────────────────────────────────╮
│ Q: why does the replay path skip the final tool result?                    │
│ The likely issue is that the replay code trims an incomplete assistant      │
│ tool-call turn before all matching ToolResult events have been observed.    │
│ Check the reducer that pairs assistant tool_calls with tool results.        │
╰────────────────────────────────────────────────────────────────────────────╯
```

### Follow-Up

```text
╭─ BTW ─ Esc close · ↑↓ scroll ──────────────────────────────────────────────╮
│ Q: why does the replay path skip the final tool result?                    │
│ The likely issue is that the replay code trims an incomplete assistant...   │
│                                                                            │
│ Q: what test should I add first?                                           │
│ Add a replay fixture with assistant tool_calls followed by matching         │
│ ToolResult events, then assert the tool result remains in context.          │
╰────────────────────────────────────────────────────────────────────────────╯
```

### Busy Notice

```text
╭─ BTW ─ Esc close · ↑↓ scroll ──────────────────────────────────────────────╮
│ Q: explain the trust flow                                                  │
│ Thinking through startup config and project context loading...             │
│                                                                            │
│ Wait for /btw to finish before sending another question.                   │
╰────────────────────────────────────────────────────────────────────────────╯
```

### Tool Denied

If the side model tries to call a tool:

```text
╭─ BTW ─ Esc close ──────────────────────────────────────────────────────────╮
│ Q: inspect src/runtime.rs                                                  │
│ Tool calls are disabled for side questions. Answer with text only.         │
│ The answer should be based on inherited context rather than live tools.     │
╰────────────────────────────────────────────────────────────────────────────╯
```

Do not show an approval prompt for sidecar tool calls. Deny them automatically.

### Error / Cancelled

```text
╭─ BTW ─ Esc close ──────────────────────────────────────────────────────────╮
│ Q: summarize current goal state                                            │
│ Interrupted by user                                                        │
╰────────────────────────────────────────────────────────────────────────────╯
```

Esc behavior:

- running panel: cancel sidecar turn and close;
- completed panel: close without cancellation;
- empty panel: close and discard;
- main active turn must continue unaffected.

## Core Design

### Sidecar State Types

Add small, explicit types rather than overloading main session state:

```rust
pub struct BtwSidecar {
    pub id: BtwSidecarId,
    pub parent_session_id: Option<String>,
    pub turns: Vec<BtwTurn>,
    pub phase: BtwPhase,
}

pub struct BtwTurn {
    pub prompt: String,
    pub answer: String,
    pub thinking: String,
    pub error: Option<String>,
    pub phase: BtwPhase,
}

pub enum BtwPhase {
    Idle,
    Running,
    Done,
    Failed,
    Cancelled,
}
```

These can live in `neo-agent` if they are purely controller/TUI state. Put reusable projection and denial helpers in `neo-agent-core`.

### Parent Context Projection

Create a helper that produces the inherited sidecar context:

```rust
pub fn sidecar_projected_messages(parent: &[AgentMessage]) -> Vec<AgentMessage> {
    let mut messages = trim_incomplete_trailing_tool_turn(parent);
    messages.push(AgentMessage::system_text(SIDE_QUESTION_SYSTEM_REMINDER));
    messages
}
```

Reminder text:

```text
This is a side-channel conversation with the user.
You are a lightweight temporary instance answering a side question.
Do not modify the main conversation, queue, goal, plan, files, or workspace.
Tool definitions may be present only for prompt-cache stability.
All tool calls are disabled and will be rejected.
Answer with text only.
```

Important details:

- Use projected or compacted parent history when available.
- Trim incomplete trailing tool exchanges so the sidecar does not inherit an open tool call without its tool result.
- Do not append sidecar messages to the parent `AgentContext`.
- The sidecar can maintain its own in-memory context for follow-up questions until closed.

### Deny-All Tool Hook

Use a before-tool-call hook so tools remain declared but never execute:

```rust
fn deny_sidecar_tool_call(_call: &AgentToolCall) -> Option<ToolResult> {
    Some(ToolResult::error(
        "Tool calls are disabled for side questions. Answer with text only.",
    ))
}
```

Keep this path independent from user permission modes. Manual/auto/yolo must not affect sidecar tool denial.

### Prompt Cache Shape

The sidecar should reuse:

- same selected model;
- same system prompt;
- same tool definitions;
- same provider options where practical;
- same projected parent history prefix.

It should not reuse:

- parent mutable `AgentContext`;
- parent JSONL writer;
- parent approval handler;
- parent goal/plan authoring side effects;
- parent tool execution access.

## Implementation Tasks

### Task 1: Add Core Sidecar Context Tests

**Files:**

- Modify: `crates/neo-agent-core/src/runtime.rs` or create `crates/neo-agent-core/src/sidecar.rs`
- Test: `crates/neo-agent-core/tests/btw_sidecar.rs` or runtime unit tests

- [ ] Add a test proving projected sidecar messages inherit parent user context.
- [ ] Add a test proving incomplete trailing tool calls are removed.
- [ ] Add a test proving the side reminder is appended after inherited history.
- [ ] Add a test proving parent context is not mutated.

Candidate tests:

```rust
#[test]
fn btw_sidecar_inherits_projected_parent_history_without_mutating_parent() {}

#[test]
fn btw_sidecar_trims_incomplete_trailing_tool_exchange() {}

#[test]
fn btw_sidecar_appends_side_question_system_reminder() {}
```

Run:

```bash
```

Expected before implementation: tests fail because no sidecar projection API exists.

### Task 2: Implement Sidecar Projection And Deny-All Hook

**Files:**

- Create: `crates/neo-agent-core/src/sidecar.rs`
- Modify: `crates/neo-agent-core/src/lib.rs`
- Modify: `crates/neo-agent-core/src/runtime.rs` only if private helper extraction is needed

- [ ] Add `SIDE_QUESTION_SYSTEM_REMINDER`.
- [ ] Add `sidecar_projected_messages`.
- [ ] Extract or expose a narrowly named helper for trimming incomplete trailing tool exchanges.
- [ ] Add `deny_sidecar_tool_call`.
- [ ] Export only the minimal public API needed by `neo-agent`.

Do not expose broad mutable internals from `AgentContext`.

### Task 3: Add In-Memory BTW Runtime Runner

**Files:**

- Create: `crates/neo-agent/src/modes/btw.rs`
- Modify: `crates/neo-agent/src/modes/mod.rs`
- Modify: `crates/neo-agent/src/modes/run.rs` if config-building helpers need to be shared

- [ ] Build a `BtwRunner` that receives a model/client, app config, inherited messages, and prompt text.
- [ ] Use the same `agent_config_for_app` defaults where practical.
- [ ] Install the deny-all before-tool hook.
- [ ] Do not pass a JSONL writer.
- [ ] Stream `TextDelta`, `ThinkingDelta`, `MessageEnd`, and errors into an in-memory channel.
- [ ] Keep sidecar cancellation separate from main turn cancellation.

Suggested event type:

```rust
pub enum BtwEvent {
    Started { sidecar_id: String, prompt: String },
    ThinkingDelta(String),
    TextDelta(String),
    ToolDenied { message: String },
    Finished,
    Cancelled,
    Failed(String),
}
```

### Task 4: Add TUI BTW Panel State And Rendering

**Files:**

- Create: `crates/neo-tui/src/widgets/btw_panel.rs`
- Modify: `crates/neo-tui/src/widgets/mod.rs`
- Modify: `crates/neo-tui/src/components.rs`
- Modify: `crates/neo-tui/src/chrome.rs` if panel state belongs in chrome

- [ ] Render a bordered `BTW` panel.
- [ ] Cap total panel height to `max(3, terminal_rows / 3)`.
- [ ] Render each turn as `Q: ...` plus thinking preview, answer markdown/text, error, or waiting state.
- [ ] Support scroll up/down when composer is empty.
- [ ] Preserve stable height while content grows to avoid layout jump.
- [ ] Add snapshot tests for empty, running, answered, busy, long content, and narrow width.

Candidate tests:

```rust
#[test]
fn btw_panel_renders_empty_state_with_esc_hint() {}

#[test]
fn btw_panel_caps_height_to_one_third_terminal_rows() {}

#[test]
fn btw_panel_truncates_long_lines_without_overlapping_border() {}
```

### Task 5: Wire `/btw` Into Interactive Mode

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-agent/src/modes/btw.rs`

- [ ] Add slash parsing for `/btw` and `/btw <question>`.
- [ ] Add slash completion and command palette entry.
- [ ] Bare `/btw` opens the sidecar panel.
- [ ] `/btw <question>` opens and submits immediately.
- [ ] If an existing panel is running, cancel it before replacing it with a new sidecar.
- [ ] Do not call the main prompt submit path.
- [ ] Do not queue or steer the main active turn.
- [ ] Use a parent projected context snapshot from the current session/transcript state.

Candidate tests:

```rust
#[tokio::test]
async fn slash_btw_opens_empty_sidecar_panel_without_starting_main_turn() {}

#[tokio::test]
async fn slash_btw_question_starts_in_memory_sidecar_only() {}

#[tokio::test]
async fn slash_btw_while_main_turn_running_does_not_steer_or_queue_main_turn() {}
```

### Task 6: Route Composer, Esc, And Scroll Input

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-tui/src/chrome.rs`
- Modify: `crates/neo-tui/src/input.rs` only if existing actions are insufficient

- [ ] While BTW panel is open and idle, Enter sends composer text to the sidecar.
- [ ] While BTW panel is running, Enter preserves composer text and shows busy notice.
- [ ] Esc closes completed/empty panel.
- [ ] Esc cancels running sidecar and closes the panel.
- [ ] Up/Down scroll panel when the composer is empty and the panel has overflow.
- [ ] Closing BTW must not cancel the main active turn.

Candidate tests:

```rust
#[tokio::test]
async fn btw_open_routes_composer_submit_to_sidecar() {}

#[tokio::test]
async fn btw_running_preserves_composer_text_and_shows_busy_notice() {}

#[tokio::test]
async fn escape_closes_btw_without_touching_main_turn() {}
```

### Task 7: Keep Main Transcript And JSONL Clean

**Files:**

- Modify: `crates/neo-agent/src/modes/btw.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/neo-agent/tests/interactive*.rs`

- [ ] Assert sidecar user prompts never appear as main `AgentEvent::UserMessage`.
- [ ] Assert sidecar assistant text never appears in the main transcript store.
- [ ] Assert no sidecar events are written to the session JSONL file.
- [ ] Assert sidecar state disappears after close and is absent after session resume.

Candidate test:

```rust
#[tokio::test]
async fn btw_conversation_is_not_written_to_main_session_jsonl() {}
```

### Task 8: Documentation

**Files:**

- Modify: `docs/quickstart.md`
- Modify: slash-command docs if present
- Modify: `AGENTS.md` only if the project guide needs the new slash command listed

Document:

- `/btw <question>` asks a side question.
- Bare `/btw` opens the side panel.
- Esc closes and may cancel the sidecar.
- The sidecar cannot use tools.
- The sidecar does not persist in session history.
- It does not steer or queue the main turn.

## Verification Plan

Focused tests:

```bash
```

Adjust filters to actual names.

Required repository gates before completion:

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

- Do not persist sidecar turns to JSONL.
- Do not append sidecar messages to the parent `AgentContext`.
- Do not create a visible Neo session for BTW.
- Do not let sidecar tools run in yolo mode.
- Do not remove tool definitions from the side request if doing so changes the prompt-cache prefix.
- Do not inherit an open/incomplete trailing tool exchange.
- Do not let `/btw` become NEO-24 queue or steer behavior.
- Do not cancel the main turn when closing/cancelling the sidecar.
- Do not let panel height exceed one third of the terminal.
- Do not create a separate page or route for the panel.
- Do not use direct git mutation commands; this repository forbids them without explicit per-command authorization.

## Self-Review Checklist

- [ ] `/btw` opens an empty sidecar panel.
- [ ] `/btw <question>` opens and asks immediately.
- [ ] Parent projected context is inherited.
- [ ] Side reminder is appended.
- [ ] Tool calls are denied automatically.
- [ ] Tool definitions remain present for cache stability.
- [ ] Side messages do not appear in main transcript replay.
- [ ] Side messages do not write to JSONL.
- [ ] Esc close/cancel behavior is correct.
- [ ] Busy notice preserves the user's typed text.
- [ ] Panel render is stable at narrow and short terminal sizes.
- [ ] Tests cover core context, TUI rendering, interactive routing, and persistence isolation.
- [ ] Verification commands ran with direct cargo commands.

## Suggested ICM Store On Completion

```bash
rtk icm store -t context-neo -c "Implemented NEO-28 /btw sidecar dialog: same-session in-memory side agent inherits projected parent context, denies all tools while preserving tool definitions for prompt-cache stability, renders a bounded TUI panel, and keeps main transcript/JSONL clean." -i high -k "NEO-28,btw,sidecar,tui,prompt-cache"
```
