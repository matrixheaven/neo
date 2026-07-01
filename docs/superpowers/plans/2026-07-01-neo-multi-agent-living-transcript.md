# Neo Multi-Agent Living Transcript Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close all nine Kimi-parity gaps in Neo's Multi-Agent living delegate and swarm transcript UI.

**Architecture:** First canonicalize the core snapshot model so child tool activity has explicit phases, output previews, timestamps, detach origin, and terminal reason. Then build one shared TUI child-activity view used by single delegates, grouped delegates, and expanded swarm children. Finally add live tick-driven elapsed/progress animation and a stateful swarm progress estimator.

**Tech Stack:** Rust 2024, `neo-agent-core` multi-agent runtime, `neo-tui` transcript components, `Line`/`Span` primitives, `cargo nextest`, `cargo clippy`.

---

## Source Spec

Use `/Users/chenyuanhao/Workspace/neo/docs/superpowers/specs/2026-07-01-neo-multi-agent-living-transcript-design.md`.

This plan implements all nine gaps from the spec:

1. Rich live state model.
2. Sub-tool output preview.
3. Fixed-window thinking preview.
4. Live elapsed tick.
5. Precise Ctrl+B/backgrounded display.
6. Background terminal reason.
7. Same-step delegate group.
8. Stateful animated swarm progress.
9. Shared expanded swarm child rendering.

## Mandatory Constraints

- Start with `icm recall-context "Neo multi-agent living transcript implementation" --limit 5`.
- Use CodeGraph before broad grep/read when locating symbols.
- Do not mutate git unless the user explicitly authorizes that exact git command.
- Do not run `cargo fmt --all` in a dirty shared worktree. Use targeted `rustfmt`
  only for files touched by the current task.
- Do not keep old compatibility fields for agent tool activity. Replace
  `failed: bool` with `AgentToolActivityPhase`.
- Do not move foreground swarm UI out of the transcript.
- Do not show full prompts in delegate/swarm headers after a short title exists.
- Do not widen tests beyond the focused targets listed in each task unless the
  failure output points to that target.

## File Structure

Create:

- `crates/neo-tui/src/transcript/child_activity.rs`
  - Shared child activity view model and renderer for single delegates and
    expanded swarm children.
- `crates/neo-tui/src/transcript/delegate_group.rs`
  - Kimi-style group for 2+ root delegates from the same turn.

Modify:

- `crates/neo-agent-core/src/multi_agent/state.rs`
  - Canonical activity phase, output preview, timestamps, detach origin,
    terminal reason.
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
  - Live event ingestion and child run finalization.
- `crates/neo-agent-core/src/multi_agent/progress.rs`
  - Stateful swarm progress estimator.
- `crates/neo-agent-core/src/events.rs`
  - No new event variant is expected; existing delegate events already carry
    snapshots with `turn`.
- `crates/neo-tui/src/transcript/mod.rs`
  - Export new transcript components/helpers.
- `crates/neo-tui/src/transcript/entry/mod.rs`
  - Add delegate group entry rendering and live tick dispatch.
- `crates/neo-tui/src/transcript/store.rs`
  - Group same-turn delegates and update grouped snapshots.
- `crates/neo-tui/src/transcript/pane.rs`
  - Extend `render_tick()` to tick live delegate/group/swarm entries.
- `crates/neo-tui/src/transcript/delegate_card.rs`
  - Use shared child activity renderer and timestamp-derived elapsed.
- `crates/neo-tui/src/transcript/swarm_card.rs`
  - Use stateful estimator and shared child renderer.
- `crates/neo-tui/tests/multi_agent_transcript.rs`
  - Add all visual regression tests.
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`
  - Add all runtime snapshot regression tests.
- `crates/neo-agent-core/tests/multi_agent_roles.rs`
  - Update fixtures for the canonical activity model.

Do not modify provider clients, model registries, MCP adapters, or unrelated
workflow/Lua files.

## Execution Order

The tasks are mostly sequential:

1. Task 1 changes the core data model and will break compile until fixtures are
   updated.
2. Task 2 makes runtime events populate the new model.
3. Task 3 creates the shared child renderer and fixes single delegate cards.
4. Task 4 adds live tick support and timestamp-derived elapsed.
5. Task 5 handles detach/background terminal display.
6. Task 6 adds grouped root delegates.
7. Task 7 replaces swarm progress estimation.
8. Task 8 ports expanded swarm children to the shared renderer.
9. Task 9 runs focused final verification and review.

Do not run tasks 3-8 before tasks 1-2 are green.

## Task 1: Canonicalize Agent Activity And Timing State

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/state.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_roles.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing core model tests**

Append focused tests to `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
use neo_agent_core::multi_agent::{
    AgentActivityKind, AgentLifecycleState, AgentTerminalReason, AgentToolActivityPhase,
    AgentToolOutputPreview,
};

#[test]
fn agent_tool_activity_uses_explicit_phase_and_output_preview() {
    let activity = AgentActivityKind::Tool {
        id: "call_1".to_owned(),
        name: "Bash".to_owned(),
        summary: Some("cargo nextest run -p neo-tui".to_owned()),
        phase: AgentToolActivityPhase::Ongoing,
        output: Some(AgentToolOutputPreview {
            text: "Compiling neo-tui v0.1.0".to_owned(),
            is_error: false,
            truncated: false,
            tail: true,
        }),
    };

    let serialized = serde_json::to_value(&activity).expect("serialize activity");
    assert_eq!(serialized["phase"], "ongoing");
    assert_eq!(serialized["output"]["tail"], true);
    assert!(
        serialized.get("failed").is_none(),
        "old failed bool must not remain in the canonical schema: {serialized}"
    );
}

#[test]
fn agent_snapshot_records_timestamps_detach_origin_and_terminal_reason() {
    let runtime = neo_agent_core::multi_agent::MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("inspect docs");

    assert!(snapshot.created_at_ms > 0);
    assert!(snapshot.updated_at_ms >= snapshot.created_at_ms);
    assert!(snapshot.started_at_ms.is_some());
    assert_eq!(snapshot.terminal_at_ms, None);
    assert!(!snapshot.detached_from_foreground);
    assert_eq!(snapshot.terminal_reason, None);

    let detached = runtime.detach_agent(&snapshot.id).expect("detach running agent");
    assert!(detached.detached_from_foreground);
    assert_eq!(detached.state, AgentLifecycleState::Running);

    let completed = runtime.complete_delegate_for_test(&snapshot.id, "done");
    assert_eq!(completed.state, AgentLifecycleState::Completed);
    assert_eq!(completed.terminal_reason, Some(AgentTerminalReason::Completed));
    assert!(completed.terminal_at_ms.is_some());
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime \
  agent_tool_activity_uses_explicit_phase_and_output_preview \
  agent_snapshot_records_timestamps_detach_origin_and_terminal_reason
```

Expected: FAIL because `AgentToolActivityPhase`, `AgentToolOutputPreview`,
timestamp fields, and terminal reason do not exist yet.

- [ ] **Step 3: Update `state.rs` data model**

In `crates/neo-agent-core/src/multi_agent/state.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolActivityPhase {
    Ongoing,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentToolOutputPreview {
    pub text: String,
    pub is_error: bool,
    pub truncated: bool,
    pub tail: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentTerminalReason {
    Completed,
    Error,
    CancelledByUser,
    TimedOut,
    Killed,
    Lost,
}
```

Replace the `Tool` variant with:

```rust
Tool {
    id: String,
    name: String,
    summary: Option<String>,
    phase: AgentToolActivityPhase,
    output: Option<AgentToolOutputPreview>,
},
```

Extend `AgentSnapshot`:

```rust
pub created_at_ms: u64,
pub updated_at_ms: u64,
pub started_at_ms: Option<u64>,
pub terminal_at_ms: Option<u64>,
pub detached_from_foreground: bool,
pub terminal_reason: Option<AgentTerminalReason>,
```

Keep these fields near `state`/`elapsed` so snapshot construction stays readable.

- [ ] **Step 4: Export the new types**

In `crates/neo-agent-core/src/multi_agent/mod.rs`, extend the `pub use state`
block:

```rust
pub use state::{
    AgentActivityEntry, AgentActivityKind, AgentLifecycleState, AgentRunMode, AgentSnapshot,
    AgentTerminalOutcome, AgentTerminalReason, AgentToolActivityPhase, AgentToolOutputPreview,
    SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
};
```

- [ ] **Step 5: Add runtime timestamp helper**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, add a helper near other
small helpers:

```rust
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
```

When constructing any new `AgentSnapshot`, set:

```rust
let created_at_ms = now_ms();
created_at_ms,
updated_at_ms: created_at_ms,
started_at_ms: matches!(lifecycle_state, AgentLifecycleState::Running).then_some(created_at_ms),
terminal_at_ms: None,
detached_from_foreground: false,
terminal_reason: None,
```

For queued snapshots, `started_at_ms` is `None`.

- [ ] **Step 6: Update snapshot transitions**

In `mark_delegate_running`, set `started_at_ms` if missing and update
`updated_at_ms`:

```rust
let now = now_ms();
snapshot.state = AgentLifecycleState::Running;
snapshot.started_at_ms.get_or_insert(now);
snapshot.updated_at_ms = now;
```

In `detach_agent`, set:

```rust
snapshot.mode = AgentRunMode::Background;
snapshot.detached_from_foreground = true;
snapshot.updated_at_ms = now_ms();
```

In the shared completion/failure/cancel path, set `terminal_at_ms` exactly once:

```rust
let now = now_ms();
snapshot.terminal_at_ms.get_or_insert(now);
snapshot.updated_at_ms = now;
snapshot.terminal_reason = Some(reason);
```

Use these mappings:

```rust
Completed => AgentTerminalReason::Completed
Failed => AgentTerminalReason::Error
Cancelled => AgentTerminalReason::CancelledByUser
TimedOut => AgentTerminalReason::TimedOut
```

Add one explicit background terminal helper so killed/lost status has a
canonical path:

```rust
pub fn mark_background_terminal_reason(
    &self,
    id: &AgentId,
    state: AgentLifecycleState,
    reason: AgentTerminalReason,
    message: Option<String>,
) -> Option<AgentSnapshot> {
    let mut locked = self.state.lock().expect("multi-agent state poisoned");
    let snapshot = locked.agents.get_mut(id.as_str())?;
    if snapshot.state.is_terminal() {
        return Some(snapshot.clone());
    }
    let now = now_ms();
    snapshot.state = state;
    snapshot.terminal_reason = Some(reason);
    snapshot.terminal_at_ms.get_or_insert(now);
    snapshot.updated_at_ms = now;
    if let Some(message) = message.filter(|value| !value.trim().is_empty()) {
        snapshot.latest_text = Some(message.clone());
        snapshot.outcome = Some(AgentTerminalOutcome {
            summary: message,
            is_error: state != AgentLifecycleState::Completed,
        });
    }
    Some(snapshot.clone())
}
```

Use this helper where background delegate reconciliation maps task status:

```rust
completed -> (AgentLifecycleState::Completed, AgentTerminalReason::Completed)
failed -> (AgentLifecycleState::Failed, AgentTerminalReason::Error)
timed_out -> (AgentLifecycleState::TimedOut, AgentTerminalReason::TimedOut)
killed -> (AgentLifecycleState::Failed, AgentTerminalReason::Killed)
lost -> (AgentLifecycleState::Failed, AgentTerminalReason::Lost)
```

- [ ] **Step 7: Update all activity fixtures**

Replace every test fixture shaped like:

```rust
AgentActivityKind::Tool {
    id,
    name,
    summary,
    failed,
}
```

with:

```rust
AgentActivityKind::Tool {
    id,
    name,
    summary,
    phase: if failed {
        AgentToolActivityPhase::Failed
    } else {
        AgentToolActivityPhase::Done
    },
    output: None,
}
```

For running/pending tool fixtures, use `AgentToolActivityPhase::Ongoing`.

- [ ] **Step 8: Run focused model tests**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime \
  agent_tool_activity_uses_explicit_phase_and_output_preview \
  agent_snapshot_records_timestamps_detach_origin_and_terminal_reason
```

Expected: PASS.

Run one compile-focused TUI fixture target to catch stale activity literals:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript delegate_card_renders_kimi_style_running_summary
```

Expected: PASS or a compile failure naming remaining old `failed` fields. If it
fails to compile, update only the stale fixtures and rerun the same command.

## Task 2: Populate Live Tool Phases And Output Previews

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add failing event-ingestion tests**

Append to `crates/neo-agent-core/tests/multi_agent_runtime.rs`:

```rust
use neo_agent_core::{
    AgentEvent, ToolResult,
    multi_agent::{AgentPathKind, DelegateContext},
};
use serde_json::json;
use std::time::Instant;

#[test]
fn child_tool_events_preserve_ongoing_done_and_failed_phase() {
    let runtime = neo_agent_core::multi_agent::MultiAgentRuntime::new();
    let snapshot = runtime.start_delegate(
        "run tests",
        Some("Run tests"),
        neo_agent_core::multi_agent::AgentRole::Coder,
        neo_agent_core::multi_agent::AgentRunMode::Foreground,
        AgentPathKind::Root,
    );
    let started_at = Instant::now();

    let started = runtime
        .apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ToolExecutionStarted {
                turn: 0,
                id: "call_bash".to_owned(),
                name: "Bash".to_owned(),
                arguments: json!({ "command": "cargo nextest run -p neo-tui" }),
            },
        )
        .expect("started update");

    let tool = started.activity.iter().find_map(|entry| match &entry.kind {
        AgentActivityKind::Tool { phase, summary, output, .. } => {
            Some((*phase, summary.clone(), output.clone()))
        }
        AgentActivityKind::Text { .. } => None,
    }).expect("tool row");

    assert_eq!(tool.0, AgentToolActivityPhase::Ongoing);
    assert_eq!(tool.1.as_deref(), Some("cargo nextest run -p neo-tui"));
    assert!(tool.2.is_none());

    let updated = runtime
        .apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ToolExecutionUpdate {
                turn: 0,
                id: "call_bash".to_owned(),
                name: "Bash".to_owned(),
                partial_result: ToolResult::success("Compiling neo-tui v0.1.0"),
            },
        )
        .expect("live output update");
    let output = latest_tool_output(&updated, "call_bash").expect("output preview");
    assert!(output.text.contains("Compiling neo-tui"));
    assert!(output.tail);

    let finished = runtime
        .apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ToolExecutionFinished {
                turn: 0,
                id: "call_bash".to_owned(),
                name: "Bash".to_owned(),
                result: ToolResult::success("Finished test profile"),
            },
        )
        .expect("finished update");
    assert_eq!(latest_tool_phase(&finished, "call_bash"), Some(AgentToolActivityPhase::Done));
    assert_eq!(finished.tool_count, 1);
}

fn latest_tool_phase(
    snapshot: &neo_agent_core::multi_agent::AgentSnapshot,
    id: &str,
) -> Option<AgentToolActivityPhase> {
    snapshot.activity.iter().rev().find_map(|entry| match &entry.kind {
        AgentActivityKind::Tool { id: entry_id, phase, .. } if entry_id == id => Some(*phase),
        _ => None,
    })
}

fn latest_tool_output(
    snapshot: &neo_agent_core::multi_agent::AgentSnapshot,
    id: &str,
) -> Option<AgentToolOutputPreview> {
    snapshot.activity.iter().rev().find_map(|entry| match &entry.kind {
        AgentActivityKind::Tool { id: entry_id, output, .. } if entry_id == id => output.clone(),
        _ => None,
    })
}
```

- [ ] **Step 2: Run the failing ingestion test**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime \
  child_tool_events_preserve_ongoing_done_and_failed_phase
```

Expected: FAIL because runtime still writes the old boolean shape or does not
store output previews.

- [ ] **Step 3: Replace `upsert_tool_activity` signature**

In `runtime.rs`, change `upsert_tool_activity` to:

```rust
fn upsert_tool_activity(
    activity: &mut Vec<AgentActivityEntry>,
    id: &str,
    name: &str,
    summary: Option<String>,
    phase: AgentToolActivityPhase,
    output: Option<AgentToolOutputPreview>,
) {
    for entry in activity.iter_mut().rev() {
        let AgentActivityKind::Tool {
            id: entry_id,
            name: entry_name,
            summary: entry_summary,
            phase: entry_phase,
            output: entry_output,
        } = &mut entry.kind
        else {
            continue;
        };
        if entry_id == id {
            if summary.is_some() {
                *entry_summary = summary;
            }
            *entry_name = name.to_owned();
            *entry_phase = phase;
            if output.is_some() {
                *entry_output = output;
            }
            return;
        }
    }
    activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            summary,
            phase,
            output,
        },
    });
}
```

- [ ] **Step 4: Add output preview helpers**

In `runtime.rs`, add:

```rust
const MAX_AGENT_TOOL_OUTPUT_PREVIEW_BYTES: usize = 50_000;

fn tool_output_preview(name: &str, result: &crate::ToolResult, tail: bool) -> Option<AgentToolOutputPreview> {
    if !should_preview_tool_output(name) || result.content.trim().is_empty() {
        return None;
    }
    let (text, truncated) = cap_preview_text(&result.content, MAX_AGENT_TOOL_OUTPUT_PREVIEW_BYTES);
    Some(AgentToolOutputPreview {
        text,
        is_error: result.is_error,
        truncated,
        tail,
    })
}

fn should_preview_tool_output(name: &str) -> bool {
    matches!(name, "Bash" | "Terminal") || name.starts_with("mcp__") || name.starts_with("extension__")
}

fn cap_preview_text(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_owned(), false);
    }
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    (format!("[...truncated]\n{}", &text[start..]), true)
}
```

- [ ] **Step 5: Update event mappings**

In `apply_child_event`:

```rust
AgentEvent::ToolExecutionStarted { id, name, arguments, .. } => {
    changed = true;
    upsert_tool_activity(
        &mut snapshot.activity,
        id,
        name,
        summarize_tool_arguments(arguments),
        AgentToolActivityPhase::Ongoing,
        None,
    );
}
AgentEvent::ToolExecutionUpdate { id, name, partial_result, .. } => {
    changed = true;
    upsert_tool_activity(
        &mut snapshot.activity,
        id,
        name,
        summarize_tool_result(name, partial_result).or_else(|| last_tool_summary(snapshot.activity.as_slice(), id)),
        AgentToolActivityPhase::Ongoing,
        tool_output_preview(name, partial_result, true),
    );
}
AgentEvent::ToolExecutionFinished { id, name, result, .. } => {
    changed = true;
    snapshot.tool_count = snapshot.tool_count.saturating_add(1);
    let phase = if result.is_error {
        AgentToolActivityPhase::Failed
    } else {
        AgentToolActivityPhase::Done
    };
    let summary = summarize_tool_result(name, result)
        .or_else(|| last_tool_summary(snapshot.activity.as_slice(), id));
    upsert_tool_activity(
        &mut snapshot.activity,
        id,
        name,
        summary,
        phase,
        tool_output_preview(name, result, false),
    );
}
```

Apply the same shape in `summarize_child_activity`.

- [ ] **Step 6: Preserve ongoing tools during trim**

Replace `trim_activity` with a policy that keeps the newest ongoing tool even if
many text deltas arrive:

```rust
fn trim_activity(activity: &mut Vec<AgentActivityEntry>) {
    const MAX_AGENT_ACTIVITY: usize = 24;
    if activity.len() <= MAX_AGENT_ACTIVITY {
        return;
    }
    let mut keep = vec![false; activity.len()];
    for (index, entry) in activity.iter().enumerate().rev() {
        if matches!(
            entry.kind,
            AgentActivityKind::Tool {
                phase: AgentToolActivityPhase::Ongoing,
                ..
            }
        ) {
            keep[index] = true;
        }
    }
    let mut remaining = MAX_AGENT_ACTIVITY.saturating_sub(keep.iter().filter(|value| **value).count());
    for index in (0..activity.len()).rev() {
        if keep[index] {
            continue;
        }
        if remaining > 0 {
            keep[index] = true;
            remaining -= 1;
        }
    }
    let mut index = 0usize;
    activity.retain(|_| {
        let retain = keep[index];
        index += 1;
        retain
    });
}
```

- [ ] **Step 7: Run focused runtime tests**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime \
  child_tool_events_preserve_ongoing_done_and_failed_phase \
  agent_tool_activity_uses_explicit_phase_and_output_preview
```

Expected: PASS.

## Task 3: Add Shared Child Activity Renderer And Update Single Delegate Card

**Files:**

- Create: `crates/neo-tui/src/transcript/child_activity.rs`
- Modify: `crates/neo-tui/src/transcript/mod.rs`
- Modify: `crates/neo-tui/src/transcript/delegate_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing single-card tests**

Append to `crates/neo-tui/tests/multi_agent_transcript.rs`:

```rust
#[test]
fn delegate_card_renders_ongoing_tool_from_explicit_phase_with_output_preview() {
    let mut snapshot = running_delegate();
    snapshot.tool_count = 0;
    snapshot.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "call_bash".to_owned(),
            name: "Bash".to_owned(),
            summary: Some("cargo nextest run -p neo-tui --test multi_agent_transcript".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: Some(AgentToolOutputPreview {
                text: "line 1\nline 2\nline 3\nline 4".to_owned(),
                is_error: false,
                truncated: false,
                tail: true,
            }),
        },
    }];

    let text = plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default())).join("\n");

    assert!(text.contains("• Using Bash"), "{text}");
    assert!(text.contains("line 3"), "{text}");
    assert!(text.contains("line 4"), "{text}");
    assert!(!text.contains("line 1"), "{text}");
}

#[test]
fn delegate_card_fixed_thinking_window_renders_before_single_final_row() {
    let mut snapshot = completed_delegate();
    snapshot.activity = vec![
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "thinking one\nthinking two\nthinking three".to_owned(),
                thinking: true,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "final answer".to_owned(),
                thinking: false,
            },
        },
    ];
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "final answer".to_owned(),
        is_error: false,
    });

    let rows = plain(DelegateCardComponent::new(snapshot).render_with_theme(90, &TuiTheme::default()));
    let text = rows.join("\n");

    assert_eq!(text.matches('◌').count(), 1, "{text}");
    assert_eq!(text.matches('└').count(), 1, "{text}");
    assert!(rows.iter().position(|line| line.contains('◌')).unwrap() < rows.iter().position(|line| line.contains('└')).unwrap());
    assert!(rows.last().unwrap().contains("final answer"), "{text}");
}
```

- [ ] **Step 2: Run the failing TUI tests**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  delegate_card_renders_ongoing_tool_from_explicit_phase_with_output_preview \
  delegate_card_fixed_thinking_window_renders_before_single_final_row
```

Expected: FAIL until the shared renderer exists and delegate card uses explicit
tool phase/output previews.

- [ ] **Step 3: Create `child_activity.rs`**

Create `crates/neo-tui/src/transcript/child_activity.rs`:

```rust
use std::time::Duration;

use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentLifecycleState, AgentProfile, AgentRole,
    AgentRunMode, AgentSnapshot, AgentToolActivityPhase, AgentToolOutputPreview,
};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Span, Style};

pub const MAX_CHILD_TOOL_ROWS: usize = 4;
const THINKING_PREVIEW_LINES: usize = 2;
const TOOL_OUTPUT_PREVIEW_LINES: usize = 2;
const FINAL_TEXT_CHARS: usize = 110;

pub struct ChildActivityView<'a> {
    pub tools: Vec<ChildToolRow<'a>>,
    pub thinking: Option<String>,
    pub final_text: Option<String>,
    pub final_is_error: bool,
}

pub struct ChildToolRow<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub summary: Option<&'a str>,
    pub phase: AgentToolActivityPhase,
    pub output: Option<&'a AgentToolOutputPreview>,
}

#[must_use]
pub fn role_label(role: AgentRole) -> &'static str {
    AgentProfile::for_role(role).display_label
}

#[must_use]
pub fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

#[must_use]
pub fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[must_use]
pub fn can_detach(snapshot: &AgentSnapshot) -> bool {
    snapshot.state == AgentLifecycleState::Running
        && snapshot.mode == AgentRunMode::Foreground
        && !snapshot.detached_from_foreground
}

#[must_use]
pub fn display_elapsed(snapshot: &AgentSnapshot, now_ms: Option<u64>) -> Duration {
    if let (Some(started), None, Some(now)) =
        (snapshot.started_at_ms, snapshot.terminal_at_ms, now_ms)
    {
        return Duration::from_millis(now.saturating_sub(started));
    }
    snapshot.elapsed
}

pub fn child_activity_view(snapshot: &AgentSnapshot, max_tool_rows: usize) -> ChildActivityView<'_> {
    let final_text = snapshot
        .outcome
        .as_ref()
        .map(|outcome| outcome.summary.clone())
        .or_else(|| latest_text_activity(snapshot, false))
        .or_else(|| snapshot.latest_text.clone());
    let thinking = latest_text_activity(snapshot, true);
    let tool_rows = snapshot
        .activity
        .iter()
        .filter_map(tool_row)
        .collect::<Vec<_>>();
    let start = tool_rows.len().saturating_sub(max_tool_rows);
    let tools = tool_rows.into_iter().skip(start).collect::<Vec<_>>();
    ChildActivityView {
        tools,
        thinking,
        final_text: dedupe_final(final_text, snapshot.latest_text.as_deref()),
        final_is_error: snapshot
            .outcome
            .as_ref()
            .is_some_and(|outcome| outcome.is_error)
            || matches!(snapshot.state, AgentLifecycleState::Failed | AgentLifecycleState::TimedOut),
    }
}

pub fn render_child_tool_row(row: &ChildToolRow<'_>, width: usize, indent: &str, theme: &TuiTheme) -> Vec<Line> {
    let marker = match row.phase {
        AgentToolActivityPhase::Failed => "✗",
        AgentToolActivityPhase::Done | AgentToolActivityPhase::Ongoing => "•",
    };
    let marker_style = match row.phase {
        AgentToolActivityPhase::Failed => Style::default().fg(theme.status_error),
        AgentToolActivityPhase::Done => Style::default().fg(theme.status_ok),
        AgentToolActivityPhase::Ongoing => Style::default().fg(theme.text_primary),
    };
    let verb = match row.phase {
        AgentToolActivityPhase::Ongoing => "Using",
        AgentToolActivityPhase::Done | AgentToolActivityPhase::Failed => "Used",
    };
    let suffix = row
        .summary
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!(" ({})", one_line(value)))
        .unwrap_or_default();
    let mut lines = vec![Line::from_spans(vec![
        Span::raw(indent.to_owned()),
        Span::styled(marker, marker_style),
        Span::raw(format!(" {verb} ")),
        Span::styled(row.name.to_owned(), Style::default().fg(theme.brand)),
        Span::styled(suffix, Style::default().fg(theme.text_muted)),
    ]).truncate_to_width(width)];
    if let Some(output) = row.output {
        lines.extend(render_output_preview(output, width, indent, theme));
    }
    lines
}

pub fn render_child_thinking(text: &str, width: usize, indent: &str, theme: &TuiTheme) -> Vec<Line> {
    let preview = tail_non_empty_lines(text, THINKING_PREVIEW_LINES).join(" ");
    if preview.is_empty() {
        return Vec::new();
    }
    vec![Line::styled(
        format!("{indent}◌ {}", compact_chars(&preview, FINAL_TEXT_CHARS)),
        Style::default().fg(theme.text_muted),
    ).truncate_to_width(width)]
}

pub fn render_child_final(text: &str, is_error: bool, width: usize, indent: &str, theme: &TuiTheme) -> Line {
    let color = if is_error { theme.status_error } else { theme.text_primary };
    Line::styled(
        format!("{indent}└ {}", compact_chars(&one_line(text), FINAL_TEXT_CHARS)),
        Style::default().fg(color),
    ).truncate_to_width(width)
}

fn tool_row(entry: &AgentActivityEntry) -> Option<ChildToolRow<'_>> {
    match &entry.kind {
        AgentActivityKind::Tool { id, name, summary, phase, output } => Some(ChildToolRow {
            id,
            name,
            summary: summary.as_deref(),
            phase: *phase,
            output: output.as_ref(),
        }),
        AgentActivityKind::Text { .. } => None,
    }
}

fn latest_text_activity(snapshot: &AgentSnapshot, thinking: bool) -> Option<String> {
    let text = snapshot.activity.iter().filter_map(|entry| match &entry.kind {
        AgentActivityKind::Text { text, thinking: entry_thinking } if *entry_thinking == thinking => Some(text.trim()),
        _ => None,
    }).filter(|text| !text.is_empty()).collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

fn dedupe_final(final_text: Option<String>, latest_text: Option<&str>) -> Option<String> {
    let final_text = final_text?;
    if latest_text.is_some_and(|latest| latest.trim() == final_text.trim()) {
        return Some(final_text);
    }
    Some(final_text)
}

fn render_output_preview(output: &AgentToolOutputPreview, width: usize, indent: &str, theme: &TuiTheme) -> Vec<Line> {
    let preview_indent = format!("{indent}    ");
    tail_non_empty_lines(&output.text, TOOL_OUTPUT_PREVIEW_LINES)
        .into_iter()
        .map(|line| {
            let color = if output.is_error { theme.status_error } else { theme.text_muted };
            Line::styled(format!("{preview_indent}{line}"), Style::default().fg(color)).truncate_to_width(width)
        })
        .collect()
}

fn tail_non_empty_lines(text: &str, limit: usize) -> Vec<String> {
    let mut lines = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let start = lines.len().saturating_sub(limit);
    lines.drain(0..start);
    lines
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compact_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    format!("{}...", text.chars().take(max_chars.saturating_sub(3)).collect::<String>())
}
```

- [ ] **Step 4: Export the helper module**

In `crates/neo-tui/src/transcript/mod.rs`:

```rust
mod child_activity;
pub(crate) use child_activity::{
    child_activity_view, render_child_final, render_child_thinking, render_child_tool_row,
    can_detach, display_elapsed, format_elapsed, format_token_count, role_label, MAX_CHILD_TOOL_ROWS,
};
```

- [ ] **Step 5: Update `DelegateCardComponent`**

In `delegate_card.rs`, remove local `DelegateActivityView`, `ToolActivityRow`,
`delegate_activity_view`, `pending_tool_ids`, `recent_tool_activity`,
`render_tool_activity`, and duplicate compact helpers.

Use:

```rust
let activity = child_activity_view(&self.snapshot, MAX_CHILD_TOOL_ROWS);
for tool in &activity.tools {
    lines.extend(render_child_tool_row(tool, width, "  ", theme));
}
if let Some(thinking) = activity.thinking.as_deref() {
    lines.extend(render_child_thinking(thinking, width, "  ", theme));
}
if let Some(final_text) = activity.final_text.as_deref() {
    lines.push(render_child_final(final_text, activity.final_is_error, width, "  ", theme));
}
```

- [ ] **Step 6: Run the single-card tests**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  delegate_card_renders_ongoing_tool_from_explicit_phase_with_output_preview \
  delegate_card_fixed_thinking_window_renders_before_single_final_row \
  completed_delegate_card_does_not_duplicate_identical_latest_text_and_summary
```

Expected: PASS.

## Task 4: Drive Live Elapsed And Animation From Transcript Ticks

**Files:**

- Modify: `crates/neo-tui/src/transcript/entry/mod.rs`
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Modify: `crates/neo-tui/src/transcript/delegate_card.rs`
- Modify: `crates/neo-tui/src/transcript/swarm_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing render tick test**

Append to `crates/neo-tui/tests/multi_agent_transcript.rs`:

```rust
#[test]
fn render_tick_marks_transcript_dirty_for_live_delegate_elapsed() {
    let mut pane = TranscriptPane::new();
    let mut snapshot = running_delegate();
    snapshot.elapsed = Duration::from_secs(0);
    snapshot.started_at_ms = Some(1);
    snapshot.terminal_at_ms = None;
    pane.apply_agent_event(neo_agent_core::AgentEvent::DelegateStarted {
        turn: 7,
        agent: snapshot,
    });

    let _ = pane.render_frame(120, 30);
    assert!(!pane.is_dirty_for_test());

    pane.render_tick_at_ms_for_test(61_000);
    assert!(pane.is_dirty_for_test());
    let frame = pane.render_frame(120, 30).join("\n");
    assert!(frame.contains("1m 0s") || frame.contains("1m"), "{frame}");
}
```

If `is_dirty_for_test` or `render_tick_at_ms_for_test` do not exist, add
`#[cfg(test)]` methods in `TranscriptPane`:

```rust
#[cfg(test)]
pub fn is_dirty_for_test(&self) -> bool {
    self.dirty
}

#[cfg(test)]
pub fn render_tick_at_ms_for_test(&mut self, now_ms: u64) {
    self.activity_frame = self.activity_frame.wrapping_add(1);
    if self.transcript.tick_live_entries(now_ms) || self.has_streaming_thinking() {
        self.mark_dirty();
    }
    let _ = self.render_frame(self.width, self.height);
}
```

- [ ] **Step 2: Run the failing render tick test**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  render_tick_marks_transcript_dirty_for_live_delegate_elapsed
```

Expected: FAIL because `render_tick()` only marks dirty for streaming thinking.

- [ ] **Step 3: Add live tick dispatch on transcript entries**

In `crates/neo-tui/src/transcript/entry/mod.rs`, add:

```rust
impl TranscriptEntry {
    pub fn on_render_tick(&mut self, now_ms: u64) -> bool {
        match self {
            Self::Delegate { component } => component.on_render_tick(now_ms),
            Self::DelegateSwarm { component } => component.on_render_tick(now_ms),
            Self::DelegateGroup { component } => component.on_render_tick(now_ms),
            _ => false,
        }
    }
}
```

`DelegateGroup` is created in Task 6. Until then, either omit that arm or add it
when Task 6 introduces the variant.

In `store.rs`, add:

```rust
pub fn tick_live_entries(&mut self, now_ms: u64) -> bool {
    self.entries.iter_mut().any(|entry| entry.on_render_tick(now_ms))
}
```

- [ ] **Step 4: Update `TranscriptPane::render_tick`**

In `pane.rs`:

```rust
pub fn render_tick(&mut self) {
    self.activity_frame = self.activity_frame.wrapping_add(1);
    let now_ms = current_time_ms();
    if self.transcript.tick_live_entries(now_ms) || self.has_streaming_thinking() {
        self.mark_dirty();
    }
    let _ = self.render_frame(self.width, self.height);
}
```

Add local helper:

```rust
fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
```

- [ ] **Step 5: Add tick state to delegate and swarm cards**

In `delegate_card.rs`, add a field:

```rust
now_ms: Option<u64>,
```

Initialize it to `None`. Add:

```rust
pub fn on_render_tick(&mut self, now_ms: u64) -> bool {
    if self.finalization() == Finalization::Finalized {
        return false;
    }
    if self.now_ms == Some(now_ms) {
        return false;
    }
    self.now_ms = Some(now_ms);
    true
}
```

When rendering elapsed, use:

```rust
fn display_elapsed(snapshot: &AgentSnapshot, now_ms: Option<u64>) -> Duration {
    if let (Some(started), None, Some(now)) = (snapshot.started_at_ms, snapshot.terminal_at_ms, now_ms) {
        return Duration::from_millis(now.saturating_sub(started));
    }
    snapshot.elapsed
}
```

Apply the same pattern to `SwarmCardComponent`.

- [ ] **Step 6: Run render tick tests**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  render_tick_marks_transcript_dirty_for_live_delegate_elapsed \
  delegate_card_renders_kimi_style_running_summary
```

Expected: PASS.

## Task 5: Render Precise Ctrl+B, Backgrounded, And Terminal Background Reasons

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-tui/src/transcript/delegate_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add failing backgrounded display test**

Append to `crates/neo-tui/tests/multi_agent_transcript.rs`:

```rust
#[test]
fn detached_foreground_delegate_renders_backgrounded_without_ctrl_b_hint() {
    let mut snapshot = running_delegate();
    snapshot.mode = AgentRunMode::Background;
    snapshot.detached_from_foreground = true;
    snapshot.state = AgentLifecycleState::Running;

    let text = plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default())).join("\n");

    assert!(text.contains("Backgrounded"), "{text}");
    assert!(!text.contains("Press Ctrl+B"), "{text}");
    assert!(!text.contains("Completed"), "{text}");
}

#[test]
fn lost_background_delegate_renders_failed_reason_not_completed() {
    let mut snapshot = completed_delegate();
    snapshot.state = AgentLifecycleState::Failed;
    snapshot.mode = AgentRunMode::Background;
    snapshot.terminal_reason = Some(AgentTerminalReason::Lost);
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "Background agent lost (session restarted before completion)".to_owned(),
        is_error: true,
    });

    let text = plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default())).join("\n");

    assert!(text.contains("Lost") || text.contains("Failed"), "{text}");
    assert!(text.contains("Background agent lost"), "{text}");
    assert!(!text.contains("Completed"), "{text}");
}
```

- [ ] **Step 2: Run the failing tests**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  detached_foreground_delegate_renders_backgrounded_without_ctrl_b_hint \
  lost_background_delegate_renders_failed_reason_not_completed
```

Expected: FAIL until display phase derives from detach/terminal reason.

- [ ] **Step 3: Add display phase helpers in delegate card**

In `delegate_card.rs`, add:

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
enum DelegateDisplayPhase {
    Queued,
    Running,
    Backgrounded,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Lost,
    Killed,
}

fn display_phase(snapshot: &AgentSnapshot) -> DelegateDisplayPhase {
    if snapshot.detached_from_foreground && snapshot.state == AgentLifecycleState::Running {
        return DelegateDisplayPhase::Backgrounded;
    }
    match snapshot.terminal_reason {
        Some(AgentTerminalReason::Lost) => DelegateDisplayPhase::Lost,
        Some(AgentTerminalReason::Killed) => DelegateDisplayPhase::Killed,
        _ => match snapshot.state {
            AgentLifecycleState::Queued => DelegateDisplayPhase::Queued,
            AgentLifecycleState::Running => DelegateDisplayPhase::Running,
            AgentLifecycleState::Completed => DelegateDisplayPhase::Completed,
            AgentLifecycleState::Failed => DelegateDisplayPhase::Failed,
            AgentLifecycleState::Cancelled => DelegateDisplayPhase::Cancelled,
            AgentLifecycleState::TimedOut => DelegateDisplayPhase::TimedOut,
        },
    }
}
```

Update `status_marker`, `status_color`, and `state_label` to consume
`DelegateDisplayPhase` instead of raw `AgentLifecycleState`.

Labels:

```rust
Queued, Running, Backgrounded, Completed, Failed, Cancelled, Timed Out, Lost, Killed
```

- [ ] **Step 4: Tighten detach hint condition**

Replace the current hint condition with:

```rust
fn can_detach(snapshot: &AgentSnapshot) -> bool {
    snapshot.state == AgentLifecycleState::Running
        && snapshot.mode == AgentRunMode::Foreground
        && !snapshot.detached_from_foreground
}
```

Use `can_detach(&self.snapshot)`.

- [ ] **Step 5: Run background display tests**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  detached_foreground_delegate_renders_backgrounded_without_ctrl_b_hint \
  lost_background_delegate_renders_failed_reason_not_completed
```

Expected: PASS.

## Task 6: Add Kimi-Style Delegate Group For Same-Turn Root Delegates

**Files:**

- Create: `crates/neo-tui/src/transcript/delegate_group.rs`
- Modify: `crates/neo-tui/src/transcript/mod.rs`
- Modify: `crates/neo-tui/src/transcript/entry/mod.rs`
- Modify: `crates/neo-tui/src/transcript/store.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing group test**

Append to `crates/neo-tui/tests/multi_agent_transcript.rs`:

```rust
#[test]
fn same_turn_root_delegates_render_as_one_live_group() {
    let mut pane = TranscriptPane::new();
    let mut first = running_delegate();
    first.id = AgentId::from_suffix_for_test("first");
    first.display_name = AgentDisplayName::new("Gibbs");
    first.task_title = "PlanBox border fix".to_owned();

    let mut second = running_delegate();
    second.id = AgentId::from_suffix_for_test("second");
    second.display_name = AgentDisplayName::new("Ada");
    second.role = AgentRole::Explorer;
    second.task_title = "Trace markdown width".to_owned();
    second.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "read_1".to_owned(),
            name: "Read".to_owned(),
            summary: Some("crates/neo-tui/src/markdown.rs".to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
        },
    }];
    second.tool_count = 1;

    pane.apply_agent_event(neo_agent_core::AgentEvent::DelegateStarted { turn: 9, agent: first });
    pane.apply_agent_event(neo_agent_core::AgentEvent::DelegateStarted { turn: 9, agent: second });

    let frame = pane.render_frame(140, 40).join("\n");

    assert!(frame.contains("Running 2 agents"), "{frame}");
    assert!(frame.contains("Coder · PlanBox border fix"), "{frame}");
    assert!(frame.contains("Explorer · Trace markdown width"), "{frame}");
    assert!(frame.contains("Used Read"), "{frame}");
    assert_eq!(frame.matches("Agent Running").count(), 0, "{frame}");
}
```

Use the existing `AgentId::from_suffix_for_test` helper. Do not add another
test-only constructor for the same purpose.

- [ ] **Step 2: Run the failing group test**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  same_turn_root_delegates_render_as_one_live_group
```

Expected: FAIL because delegates are still separate transcript entries.

- [ ] **Step 3: Create `delegate_group.rs`**

Create `crates/neo-tui/src/transcript/delegate_group.rs`:

```rust
use std::time::Duration;

use neo_agent_core::multi_agent::{
    AgentLifecycleState, AgentRunMode, AgentSnapshot, AgentToolActivityPhase,
};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Component, Finalization, Line, Span, Style};
use crate::transcript::{
    can_detach, child_activity_view, display_elapsed, format_elapsed, format_token_count,
    role_label,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegateGroupComponent {
    turn: u32,
    agents: Vec<AgentSnapshot>,
    now_ms: Option<u64>,
}

impl DelegateGroupComponent {
    pub fn new(turn: u32, agents: Vec<AgentSnapshot>) -> Self {
        Self { turn, agents, now_ms: None }
    }

    pub const fn turn(&self) -> u32 {
        self.turn
    }

    pub fn contains(&self, id: &str) -> bool {
        self.agents.iter().any(|agent| agent.id.as_str() == id)
    }

    pub fn upsert(&mut self, snapshot: AgentSnapshot) {
        if let Some(existing) = self.agents.iter_mut().find(|agent| agent.id == snapshot.id) {
            *existing = snapshot;
        } else {
            self.agents.push(snapshot);
        }
    }

    pub fn on_render_tick(&mut self, now_ms: u64) -> bool {
        if self.finalization() == Finalization::Finalized {
            return false;
        }
        if self.now_ms == Some(now_ms) {
            return false;
        }
        self.now_ms = Some(now_ms);
        true
    }

    pub fn render_with_theme(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let mut lines = vec![self.header(width, theme)];
        for (index, agent) in self.agents.iter().enumerate() {
            let last = index + 1 == self.agents.len();
            lines.extend(self.render_agent(agent, last, width, theme));
        }
        if self.agents.iter().any(can_detach) {
            lines.push(Line::styled("  Press Ctrl+B to run in background", Style::default().fg(theme.text_muted)));
        }
        lines
    }

    fn header(&self, width: usize, theme: &TuiTheme) -> Line {
        let all_terminal = self.agents.iter().all(|agent| agent.state.is_terminal());
        let marker = if all_terminal { "•" } else { "●" };
        let marker_color = if all_terminal { theme.status_ok } else { theme.text_primary };
        let total = self.agents.len();
        let elapsed = self.max_elapsed();
        let label = if all_terminal {
            format!("{total} agents finished")
        } else {
            let running = self
                .agents
                .iter()
                .filter(|agent| {
                    agent.state == AgentLifecycleState::Running
                        && !(agent.detached_from_foreground && agent.mode == AgentRunMode::Background)
                })
                .count();
            let waiting = self
                .agents
                .iter()
                .filter(|agent| agent.state == AgentLifecycleState::Queued)
                .count();
            let backgrounded = self
                .agents
                .iter()
                .filter(|agent| {
                    agent.detached_from_foreground && agent.state == AgentLifecycleState::Running
                })
                .count();
            let mut parts = Vec::new();
            if running > 0 {
                parts.push(format!("{running} running"));
            }
            if waiting > 0 {
                parts.push(format!("{waiting} waiting"));
            }
            if backgrounded > 0 {
                parts.push(format!("{backgrounded} backgrounded"));
            }
            if parts.is_empty() {
                format!("Running {total} agents")
            } else {
                format!("Running {total} agents ({})", parts.join(", "))
            }
        };
        let tools = self.agents.iter().map(|agent| agent.tool_count).sum::<usize>();
        let tokens = self.agents.iter().map(|agent| agent.token_count).sum::<usize>();
        let tail = if all_terminal {
            format!(
                " · {tools} tools · {} · {} tok",
                format_elapsed(elapsed.as_secs()),
                format_token_count(tokens)
            )
        } else {
            format!(" · {}", format_elapsed(elapsed.as_secs()))
        };
        Line::from_spans(vec![
            Span::styled(marker, Style::default().fg(marker_color)),
            Span::raw(" "),
            Span::styled(label, Style::default().fg(theme.brand)),
            Span::styled(tail, Style::default().fg(theme.text_muted)),
        ])
        .truncate_to_width(width)
    }

    fn render_agent(
        &self,
        agent: &AgentSnapshot,
        is_last: bool,
        width: usize,
        theme: &TuiTheme,
    ) -> Vec<Line> {
        let branch = if is_last { "└─" } else { "├─" };
        let continuation = if is_last { "   " } else { "│  " };
        let activity = latest_activity(agent).unwrap_or_else(|| fallback_activity(agent));
        let mut lines = vec![
            Line::from_spans(vec![
                Span::raw(format!("  {branch} ")),
                Span::styled(role_label(agent.role), Style::default().fg(theme.brand)),
                Span::styled(
                    format!(" · {}", agent.display_title()),
                    Style::default().fg(theme.text_primary),
                ),
                Span::styled(format_stats(agent, self.now_ms), Style::default().fg(theme.text_muted)),
            ])
            .truncate_to_width(width),
        ];
        if !agent.state.is_terminal() {
            lines.push(
                Line::styled(
                    format!("  {continuation}    {activity}"),
                    Style::default().fg(theme.text_muted),
                )
                .truncate_to_width(width),
            );
        } else if agent.state == AgentLifecycleState::Failed
            || agent.state == AgentLifecycleState::TimedOut
        {
            lines.push(
                Line::styled(
                    format!("  {continuation}    Error: {activity}"),
                    Style::default().fg(theme.status_error),
                )
                .truncate_to_width(width),
            );
        }
        lines
    }

    fn max_elapsed(&self) -> Duration {
        self.agents
            .iter()
            .map(|agent| display_elapsed(agent, self.now_ms))
            .max()
            .unwrap_or_default()
    }
}

impl Component for DelegateGroupComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_with_theme(width, &TuiTheme::default())
    }

    fn finalization(&self) -> Finalization {
        if self.agents.iter().all(|agent| agent.state.is_terminal()) {
            Finalization::Finalized
        } else {
            Finalization::Live
        }
    }
}
```

Add these helper functions at the bottom of `delegate_group.rs`:

```rust
fn format_stats(agent: &AgentSnapshot, now_ms: Option<u64>) -> String {
    let elapsed = display_elapsed(agent, now_ms);
    let mut parts = Vec::new();
    if agent.tool_count > 0 {
        parts.push(format!("{} tools", agent.tool_count));
    }
    if !elapsed.is_zero() {
        parts.push(format_elapsed(elapsed.as_secs()));
    }
    if agent.token_count > 0 {
        parts.push(format!("{} tok", format_token_count(agent.token_count)));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    }
}

fn latest_activity(agent: &AgentSnapshot) -> Option<String> {
    let view = child_activity_view(agent, 1);
    if let Some(tool) = view.tools.last() {
        let verb = if tool.phase == AgentToolActivityPhase::Ongoing {
            "Using"
        } else {
            "Used"
        };
        return Some(match tool.summary {
            Some(summary) if !summary.trim().is_empty() => {
                format!("{verb} {} ({})", tool.name, one_line(summary))
            }
            _ => format!("{verb} {}", tool.name),
        });
    }
    view.final_text.map(|text| one_line(&text))
}

fn fallback_activity(agent: &AgentSnapshot) -> String {
    match agent.state {
        AgentLifecycleState::Queued => "Waiting...".to_owned(),
        AgentLifecycleState::Running => "Running...".to_owned(),
        AgentLifecycleState::Completed => "Completed".to_owned(),
        AgentLifecycleState::Failed => "Failed".to_owned(),
        AgentLifecycleState::Cancelled => "Cancelled".to_owned(),
        AgentLifecycleState::TimedOut => "Timed out".to_owned(),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
```

- [ ] **Step 4: Add entry variant and export**

In `mod.rs`:

```rust
mod delegate_group;
pub use delegate_group::DelegateGroupComponent;
```

In `entry/mod.rs` add:

```rust
DelegateGroup {
    component: DelegateGroupComponent,
},
```

Render arm:

```rust
Self::DelegateGroup { component } => component.render_with_theme(inner_width, theme),
```

Finalization arm:

```rust
Self::DelegateGroup { component } => component.finalization(),
```

- [ ] **Step 5: Group delegates in store**

Change `TranscriptStore::upsert_delegate` to accept `turn`:

```rust
pub fn upsert_delegate(&mut self, turn: u32, snapshot: AgentSnapshot) {
    ...
}
```

Update `event_handler.rs` to pass `turn`.

Store grouping algorithm:

1. If an existing `DelegateGroup` contains the agent id, update it.
2. If an existing standalone `Delegate` has the same `turn` and both agents are
   root delegates, replace that standalone entry with `DelegateGroup`.
3. If a group for the turn already exists, insert/update inside it.
4. Otherwise append standalone `Delegate`.

Root delegate check:

```rust
fn is_root_delegate(snapshot: &AgentSnapshot) -> bool {
    snapshot.path.is_root_child()
}
```

If `AgentPath` lacks `is_root_child`, add a method that returns true for paths
not under a swarm id. Do not infer root status from display name.

- [ ] **Step 6: Run group test**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  same_turn_root_delegates_render_as_one_live_group
```

Expected: PASS.

## Task 7: Replace Static Swarm Progress With Stateful Animated Estimator

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/progress.rs`
- Modify: `crates/neo-tui/src/transcript/swarm_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing estimator tests**

Append to `crates/neo-tui/tests/multi_agent_transcript.rs`:

```rust
#[test]
fn swarm_progress_starts_at_zero_then_moves_after_running_activity() {
    let mut card = SwarmCardComponent::new(swarm_with_child_states(vec![
        AgentLifecycleState::Queued,
        AgentLifecycleState::Queued,
    ]));

    let queued = plain(card.render_with_theme(140, &TuiTheme::default())).join("\n");
    assert!(queued.contains("0%") || queued.contains("1%"), "{queued}");
    assert!(!queued.contains("100%"), "{queued}");

    let mut running = card.snapshot().clone();
    running.children[0].agent.state = AgentLifecycleState::Running;
    running.children[0].agent.started_at_ms = Some(1_000);
    running.children[0].agent.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "call_1".to_owned(),
            name: "Read".to_owned(),
            summary: Some("README.md".to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
        },
    });
    card.update(running);
    card.on_render_tick(2_000);

    let frame = plain(card.render_with_theme(140, &TuiTheme::default())).join("\n");
    assert!(frame.contains("Working"), "{frame}");
    assert!(!frame.contains("100%"), "{frame}");
    assert!(frame.contains("Used Read"), "{frame}");
}
```

- [ ] **Step 2: Run the failing progress test**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  swarm_progress_starts_at_zero_then_moves_after_running_activity
```

Expected: FAIL because the current card has no stateful estimator/tick behavior.

- [ ] **Step 3: Implement `SwarmProgressEstimator`**

In `progress.rs`, keep `SwarmProgressInput` only if other callers still need it,
but make the canonical API stateful:

```rust
#[derive(Debug, Clone, Default)]
pub struct SwarmProgressEstimator {
    members: std::collections::BTreeMap<String, MemberProgressState>,
    completed_samples: Vec<CompletedSample>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwarmProgressEstimate {
    pub raw_ticks: f32,
    pub display_ticks: f32,
    pub progress: f32,
    pub boosted: bool,
}
```

Implement:

```rust
pub fn ensure_member(&mut self, member_id: &str, now_ms: u64)
pub fn mark_started(&mut self, member_id: &str, now_ms: u64)
pub fn record_tool_call(&mut self, member_id: &str, tool_call_id: &str, now_ms: u64)
pub fn mark_completed(&mut self, member_id: &str, now_ms: u64)
pub fn mark_failed(&mut self, member_id: &str, now_ms: u64)
pub fn mark_cancelled(&mut self, member_id: &str, now_ms: u64)
pub fn estimate(&mut self, member_id: &str, phase: SwarmEstimatorPhase, capacity_ticks: f32, now_ms: u64) -> SwarmProgressEstimate
pub fn has_pending_catchup(&self) -> bool
```

Use these constants:

```rust
const DEFAULT_UNFINISHED_PROGRESS_CAP: f32 = 0.85;
const DEFAULT_CATCHUP_TIME_MS: u64 = 1_500;
const DEFAULT_WORKLOAD_SPREAD_FACTOR: f32 = 1.5;
```

The estimator must never return `progress >= 1.0` for a non-terminal phase.

- [ ] **Step 4: Store estimator in `SwarmCardComponent`**

Add fields:

```rust
estimator: SwarmProgressEstimator,
now_ms: Option<u64>,
```

In `new`, call `sync_estimator_from_snapshot` once.

In `update`, assign the snapshot and call `sync_estimator_from_snapshot`.

`sync_estimator_from_snapshot`:

- ensure every child member exists;
- call `mark_started` for running children with `started_at_ms`;
- call `record_tool_call` for every tool activity id in every child;
- call terminal methods for terminal child states.

- [ ] **Step 5: Render braille-like child bars**

Replace `progress_bar_text(state)` with a function that accepts an estimate:

```rust
fn progress_bar_text(estimate: f32, state: AgentLifecycleState) -> String {
    const WIDTH: usize = 8;
    let progress = if state.is_terminal() { 1.0 } else { estimate.clamp(0.0, 0.95) };
    let filled = (progress * WIDTH as f32).floor() as usize;
    format!("{}{}", "■".repeat(filled), "·".repeat(WIDTH.saturating_sub(filled)))
}
```

Use the square/dot bar in this pass. Do not introduce braille glyphs until a
separate width-focused test proves Neo's renderer handles them consistently.

- [ ] **Step 6: Drive swarm tick**

Implement:

```rust
pub fn on_render_tick(&mut self, now_ms: u64) -> bool {
    if self.finalization() == Finalization::Finalized && !self.estimator.has_pending_catchup() {
        return false;
    }
    self.now_ms = Some(now_ms);
    self.estimator.has_pending_catchup() || self.snapshot.children.iter().any(|child| !child.agent.state.is_terminal())
}
```

- [ ] **Step 7: Run progress tests**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  swarm_progress_starts_at_zero_then_moves_after_running_activity \
  swarm_card_progress_starts_near_zero_when_all_children_queued \
  swarm_card_child_row_prefers_latest_activity_over_full_prompt
```

Expected: PASS.

## Task 8: Make Expanded Swarm Children Use The Shared Child Renderer

**Files:**

- Modify: `crates/neo-tui/src/transcript/swarm_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing expanded swarm parity test**

Append to `crates/neo-tui/tests/multi_agent_transcript.rs`:

```rust
#[test]
fn expanded_swarm_child_uses_delegate_activity_rules() {
    let mut snapshot = swarm_with_child_states(vec![AgentLifecycleState::Completed]);
    snapshot.children[0].agent.activity = vec![
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "bash_1".to_owned(),
                name: "Bash".to_owned(),
                summary: Some("printf 2".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: Some(AgentToolOutputPreview {
                    text: "1\n2\n3".to_owned(),
                    is_error: false,
                    truncated: false,
                    tail: false,
                }),
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "thinking one\nthinking two".to_owned(),
                thinking: true,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "final child summary".to_owned(),
                thinking: false,
            },
        },
    ];
    snapshot.children[0].agent.outcome = Some(AgentTerminalOutcome {
        summary: "final child summary".to_owned(),
        is_error: false,
    });

    let mut card = SwarmCardComponent::new(snapshot);
    card.set_expanded(true);
    let rows = card.render_with_theme(120, &TuiTheme::default());
    let text = plain(rows.clone()).join("\n");

    assert_eq!(text.matches('◌').count(), 1, "{text}");
    assert_eq!(text.matches('└').count(), 1, "{text}");
    assert!(text.contains("Used Bash"), "{text}");
    assert!(text.contains("final child summary"), "{text}");
}
```

- [ ] **Step 2: Run the failing parity test**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  expanded_swarm_child_uses_delegate_activity_rules
```

Expected: FAIL because expanded swarm still renders old activity rules.

- [ ] **Step 3: Replace expanded child activity rendering**

In `swarm_card.rs`, remove `render_child_activity` and
`should_render_latest_text` if they are no longer used.

Inside the `if self.expanded` block:

```rust
let view = child_activity_view(&child.agent, MAX_CHILD_TOOL_ROWS);
for tool in &view.tools {
    lines.extend(render_child_tool_row(tool, width, "    ", theme));
}
if let Some(thinking) = view.thinking.as_deref() {
    lines.extend(render_child_thinking(thinking, width, "    ", theme));
}
if let Some(final_text) = view.final_text.as_deref() {
    lines.push(render_child_final(final_text, view.final_is_error, width, "    ", theme));
}
```

For collapsed child row summary, prefer:

1. latest ongoing tool;
2. latest finished tool;
3. final outcome summary;
4. latest text;
5. task title;
6. item fallback.

Use a helper:

```rust
fn child_activity_summary(agent: &AgentSnapshot, fallback_item: &str) -> String {
    let view = child_activity_view(agent, 1);
    if let Some(tool) = view.tools.last() {
        let verb = if tool.phase == AgentToolActivityPhase::Ongoing { "Using" } else { "Used" };
        return compact_to_chars(&format_tool_summary(verb, tool), 96);
    }
    if let Some(final_text) = view.final_text {
        return compact_to_chars(&one_line(&final_text), 96);
    }
    if let Some(text) = &agent.latest_text {
        return compact_to_chars(&one_line(text), 96);
    }
    if !agent.task_title.is_empty() {
        return compact_to_chars(&one_line(&agent.task_title), 96);
    }
    compact_to_chars(&one_line(fallback_item), 96)
}
```

- [ ] **Step 4: Run expanded swarm tests**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript \
  expanded_swarm_child_uses_delegate_activity_rules \
  swarm_card_child_row_prefers_latest_activity_over_full_prompt
```

Expected: PASS.

## Task 9: Focused Final Verification And Self-Review

**Files:**

- Review all files modified in Tasks 1-8.

- [ ] **Step 1: Run targeted formatting on touched files only**

Run:

```bash
rustfmt --edition 2024 \
  crates/neo-agent-core/src/multi_agent/state.rs \
  crates/neo-agent-core/src/multi_agent/runtime.rs \
  crates/neo-agent-core/src/multi_agent/progress.rs \
  crates/neo-tui/src/transcript/child_activity.rs \
  crates/neo-tui/src/transcript/delegate_card.rs \
  crates/neo-tui/src/transcript/delegate_group.rs \
  crates/neo-tui/src/transcript/swarm_card.rs \
  crates/neo-tui/src/transcript/mod.rs \
  crates/neo-tui/src/transcript/entry/mod.rs \
  crates/neo-tui/src/transcript/store.rs \
  crates/neo-tui/src/transcript/pane.rs \
  crates/neo-agent-core/tests/multi_agent_runtime.rs \
  crates/neo-agent-core/tests/multi_agent_roles.rs \
  crates/neo-tui/tests/multi_agent_transcript.rs
```

Expected: exit 0.

- [ ] **Step 2: Run full focused Multi-Agent runtime target**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime
```

Expected: all tests in this target pass.

- [ ] **Step 3: Run role fixture target**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_roles
```

Expected: all tests in this target pass.

- [ ] **Step 4: Run full focused TUI transcript target**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript
```

Expected: all tests in this target pass.

- [ ] **Step 5: Run clippy on touched test targets**

Run:

```bash
cargo clippy -p neo-agent-core --test multi_agent_runtime -- -D warnings -A clippy::pedantic
cargo clippy -p neo-agent-core --test multi_agent_roles -- -D warnings -A clippy::pedantic
cargo clippy -p neo-tui --test multi_agent_transcript -- -D warnings -A clippy::pedantic
```

Expected: all commands exit 0.

- [ ] **Step 6: Manual transcript smoke test**

Run Neo manually with a prompt that forces one foreground delegate with Bash:

```text
Use Delegate in foreground. Ask a coder subagent to run `printf 'a\nb\nc\n'` with Bash and summarize the last line.
```

Expected visual shape:

```text
● <Name> Coder Agent Running (...) · 1 tools · <elapsed> · <tok> tok
  Press Ctrl+B to run in background
  • Using Bash (printf 'a\nb\nc\n')
      b
      c
  ◌ <single thinking preview>
  └ <single final line>
```

Run another prompt that forces two foreground delegates in one model step.

Expected: one `Running 2 agents` group, not two standalone full cards.

Run a foreground `DelegateSwarm` with three items and `max_concurrency = 1`.

Expected: first child running, later children queued at zero progress, overall
progress below 100%, latest activity replacing full prompt after child activity
arrives.

- [ ] **Step 7: Self-review against the nine gaps**

Check this table and record the result in the final implementation summary:

| Gap | Evidence required |
| --- | --- |
| Rich live state | `AgentToolActivityPhase` is canonical and no `failed: bool` activity field remains |
| Output preview | TUI test shows Bash output preview under delegate tool row |
| Fixed thinking | TUI test counts one `◌` and one final `└` |
| Live elapsed | Render tick test shows dirty state and elapsed change without child output |
| Ctrl+B/backgrounded | TUI test shows `Backgrounded` and no hint after detach |
| Terminal background reason | TUI/core test distinguishes `Lost`/`Killed` from completed |
| AgentGroup | TUI test shows `Running 2 agents` for same-turn delegates |
| Swarm animation | Swarm progress test starts near zero and moves after activity |
| Expanded swarm parity | Expanded swarm test uses same thinking/final/tool rules |

- [ ] **Step 8: Check diff scope**

Run:

```bash
git diff --name-only
```

Expected: only files listed in this plan plus user/other-agent pre-existing dirty
files. Do not revert unrelated dirty files. Do not commit unless the user gives
explicit per-command authorization.

## Plan Self-Review

- Spec coverage: every requirement in the source spec maps to Tasks 1-9.
- Placeholder scan: no task contains placeholder tokens or an unspecified test
  command.
- Type consistency: all activity rows use `AgentToolActivityPhase` and
  `AgentToolOutputPreview`; no task introduces a second tool phase type.
- Scope check: the plan touches only Multi-Agent state/runtime/progress and TUI
  transcript files.
- Git policy check: generic writing-plans commit steps are intentionally omitted
  because Neo's local policy forbids git mutations without explicit
  per-command authorization.
