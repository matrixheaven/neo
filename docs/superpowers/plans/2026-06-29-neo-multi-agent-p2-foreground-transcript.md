# Neo Multi-Agent P2 Foreground Transcript Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render foreground `Delegate` and `DelegateSwarm` as Kimi-style live inline transcript cards while the parent agent is blocked.

**Architecture:** Keep runtime state in `neo-agent-core`, but make the TUI consume normalized `AgentEvent` variants through `TranscriptPane::apply_agent_event`. Add focused transcript components for single delegates and swarms instead of overloading generic tool body rendering. `ToolCallComponent` remains the normal tool card path, but delegate events upsert child-agent components inside the transcript store.

**Tech Stack:** Rust 2024, `neo-tui` components, `AgentEvent`, `TranscriptPane`, `Line`/`Span`, `TuiTheme`, focused `cargo run -p xtask -- test -p neo-tui <filter>`.

---

## Constraints

- Follow `/Users/chenyuanhao/Workspace/neo/AGENTS.md`.
- Start with `icm recall-context "Neo Multi-Agent P2 foreground transcript Kimi UI" --limit 5`.
- Use CodeGraph before grep/read when locating transcript/event code.
- Do not create a separate agent page or side panel.
- Do not change `/tasks` in this plan.
- Do not implement background detach in this plan; P3 owns it.

## Current Code Touchpoints

- `crates/neo-tui/src/transcript/event_handler.rs`
  - `TranscriptPane::apply_agent_event` routes `AgentEvent` into transcript state.
- `crates/neo-tui/src/transcript/tool_call.rs`
  - Current tool card component with live output and expansion behavior.
- `crates/neo-tui/src/transcript/store.rs`
  - Transcript entries are stored here.
- `crates/neo-tui/src/transcript/mod.rs`
  - Public transcript component exports.
- `crates/neo-tui/tests/`
  - Add focused rendering tests here.
- `crates/neo-agent-core/src/events.rs`
  - P1 adds delegate/swarm events that this plan renders.

## File Structure

Create:

- `crates/neo-tui/src/transcript/delegate_card.rs`
- `crates/neo-tui/src/transcript/swarm_card.rs`
- `crates/neo-tui/tests/multi_agent_transcript.rs`

Modify:

- `crates/neo-tui/src/transcript/mod.rs`
- `crates/neo-tui/src/transcript/store.rs`
- `crates/neo-tui/src/transcript/event_handler.rs`
- `crates/neo-agent-core/src/multi_agent/state.rs` only if the TUI needs a compact row field missing from P1.

## Desired End State

- A running single delegate renders like:

```text
● Gibbs Agent Running (Implement Task 1: PlanBox border fix) · 3 tools · 24s · 25.6k tok
  Press Ctrl+B to run in background
  • Used Read (crates/neo-tui/src/transcript/plan_box.rs)
  ✗ Used Grep (from_spans|pub struct Span|pub struct Line)
  ◌ Let me start by reading the current file to understand its structure.
```

- A foreground swarm renders an `Orchestrating...` phase before children start.
- A foreground swarm renders a `Working...` phase with item progress rows after children start.
- Expanded swarm detail shows child rows and latest child tool/text tail.
- Cards use the same global expansion behavior as thinking/tool output.

## Phase 1: Single Delegate Card

### Task 1.1: Create `DelegateCardComponent`

**Files:**
- Create: `crates/neo-tui/src/transcript/delegate_card.rs`
- Modify: `crates/neo-tui/src/transcript/mod.rs`

- [ ] **Step 1: Implement component**

```rust
use neo_agent_core::multi_agent::{AgentLifecycleState, AgentRunMode, AgentSnapshot};

use crate::primitive::{Component, Expandable, Finalization, Line, Span, Style};
use crate::primitive::theme::TuiTheme;

#[derive(Debug, Clone)]
pub struct DelegateCardComponent {
    snapshot: AgentSnapshot,
    expanded: bool,
}

impl DelegateCardComponent {
    #[must_use]
    pub fn new(snapshot: AgentSnapshot) -> Self {
        Self {
            snapshot,
            expanded: false,
        }
    }

    pub fn update(&mut self, snapshot: AgentSnapshot) {
        self.snapshot = snapshot;
    }

    #[must_use]
    pub fn id(&self) -> &str {
        self.snapshot.id.as_str()
    }

    #[must_use]
    pub fn render_with_theme(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        let mut lines = Vec::new();
        lines.push(Line::from_spans(vec![
            Span::styled(status_marker(self.snapshot.state), Style::default().fg(theme.accent)),
            Span::raw(" "),
            Span::styled(self.snapshot.display_name.as_str(), Style::default().fg(theme.accent)),
            Span::raw(format!(
                " Agent {} ({}) · {} tools · {} · {} tok",
                state_label(self.snapshot.state),
                self.snapshot.task,
                self.snapshot.tool_count,
                format_elapsed(self.snapshot.elapsed.as_secs()),
                format_token_count(self.snapshot.token_count)
            )),
        ]).truncate_to_width(width));

        if self.snapshot.state == AgentLifecycleState::Running
            && self.snapshot.mode == AgentRunMode::Foreground
        {
            lines.push(Line::from("  Press Ctrl+B to run in background").dim());
        }

        if let Some(text) = &self.snapshot.latest_text {
            lines.push(Line::from(format!("  ◌ {text}")).truncate_to_width(width));
        }

        if let Some(outcome) = &self.snapshot.outcome {
            lines.push(Line::from(format!("  └ {}", outcome.summary)).truncate_to_width(width));
        }

        lines
    }
}

impl Expandable for DelegateCardComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }
}

impl Component for DelegateCardComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_with_theme(width, &TuiTheme::default())
    }

    fn finalization(&self) -> Finalization {
        match self.snapshot.state {
            AgentLifecycleState::Completed
            | AgentLifecycleState::Failed
            | AgentLifecycleState::Cancelled => Finalization::Finalized,
            AgentLifecycleState::Queued | AgentLifecycleState::Running => Finalization::Live,
        }
    }
}

fn status_marker(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Running => "●",
        AgentLifecycleState::Completed => "✓",
        AgentLifecycleState::Failed => "✗",
        AgentLifecycleState::Queued | AgentLifecycleState::Cancelled => "◌",
    }
}

fn state_label(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Queued => "Queued",
        AgentLifecycleState::Running => "Running",
        AgentLifecycleState::Completed => "Completed",
        AgentLifecycleState::Failed => "Failed",
        AgentLifecycleState::Cancelled => "Cancelled",
    }
}

fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}
```

- [ ] **Step 2: Export component**

Modify `crates/neo-tui/src/transcript/mod.rs`:

```rust
mod delegate_card;
pub use delegate_card::DelegateCardComponent;
```

- [ ] **Step 3: Add rendering test**

Create `crates/neo-tui/tests/multi_agent_transcript.rs`:

```rust
use std::time::Duration;

use neo_agent_core::multi_agent::{
    AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
    AgentSnapshot,
};
use neo_tui::transcript::DelegateCardComponent;
use neo_tui::primitive::Component;

fn running_delegate() -> AgentSnapshot {
    let name = AgentDisplayName::new("Gibbs");
    AgentSnapshot {
        id: AgentId::from_suffix_for_test("test"),
        display_name: name.clone(),
        path: AgentPath::root_child(&name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: AgentLifecycleState::Running,
        task: "Implement Task 1: PlanBox border fix".to_owned(),
        tool_count: 3,
        token_count: 25_600,
        elapsed: Duration::from_secs(24),
        latest_text: Some("Let me start by reading the current file.".to_owned()),
        outcome: None,
    }
}

#[test]
fn delegate_card_renders_kimi_style_running_summary() {
    let mut card = DelegateCardComponent::new(running_delegate());

    let text = card
        .render(120)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("● Gibbs Agent Running"));
    assert!(text.contains("3 tools"));
    assert!(text.contains("24s"));
    assert!(text.contains("25.6k tok"));
    assert!(text.contains("Press Ctrl+B to run in background"));
    assert!(text.contains("Let me start by reading"));
}
```

- [ ] **Step 4: Run test**

Run:

```bash
cargo run -p xtask -- test -p neo-tui delegate_card_renders_kimi_style_running_summary
```

Expected: PASS.

## Phase 2: Swarm Card

### Task 2.1: Create `SwarmCardComponent`

**Files:**
- Create: `crates/neo-tui/src/transcript/swarm_card.rs`
- Modify: `crates/neo-tui/src/transcript/mod.rs`
- Test: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Implement component**

```rust
use neo_agent_core::multi_agent::{AgentLifecycleState, SwarmSnapshot};

use crate::primitive::{Component, Expandable, Finalization, Line};

#[derive(Debug, Clone)]
pub struct SwarmCardComponent {
    snapshot: SwarmSnapshot,
    expanded: bool,
}

impl SwarmCardComponent {
    #[must_use]
    pub fn new(snapshot: SwarmSnapshot) -> Self {
        Self {
            snapshot,
            expanded: false,
        }
    }

    pub fn update(&mut self, snapshot: SwarmSnapshot) {
        self.snapshot = snapshot;
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<Line> {
        let mut lines = vec![
            Line::from(format!(
                "─ Agent Swarm ─ {} ─",
                self.snapshot.description
            ))
            .truncate_to_width(width),
            Line::from(""),
        ];

        for child in &self.snapshot.children {
            lines.push(
                Line::from(format!(
                    "{:03} [{}] {}",
                    child.item_index + 1,
                    progress_bar(child.agent.state),
                    child.item
                ))
                .truncate_to_width(width),
            );
        }

        if self.snapshot.children.iter().all(|child| {
            matches!(child.agent.state, AgentLifecycleState::Queued)
        }) {
            lines.push(Line::from(""));
            lines.push(Line::from("🟡 Orchestrating..."));
        } else {
            lines.push(Line::from(""));
            lines.push(Line::from("🟡 Working...  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"));
        }

        if self.expanded {
            for child in &self.snapshot.children {
                lines.push(
                    Line::from(format!(
                        "  {} {}   {}   {}",
                        marker(child.agent.state),
                        child.agent.display_name.as_str(),
                        child.item,
                        state_label(child.agent.state)
                    ))
                    .truncate_to_width(width),
                );
            }
        }

        lines
    }
}

impl Expandable for SwarmCardComponent {
    fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }
}

impl Component for SwarmCardComponent {
    fn render(&mut self, width: usize) -> Vec<Line> {
        self.render_lines(width)
    }

    fn finalization(&self) -> Finalization {
        if self.snapshot.children.iter().all(|child| {
            matches!(
                child.agent.state,
                AgentLifecycleState::Completed
                    | AgentLifecycleState::Failed
                    | AgentLifecycleState::Cancelled
            )
        }) {
            Finalization::Finalized
        } else {
            Finalization::Live
        }
    }
}

fn progress_bar(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Queued => "..........",
        AgentLifecycleState::Running => "###.......",
        AgentLifecycleState::Completed => "##########",
        AgentLifecycleState::Failed | AgentLifecycleState::Cancelled => "xxx.......",
    }
}

fn marker(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Running => "●",
        AgentLifecycleState::Completed => "✓",
        AgentLifecycleState::Failed => "✗",
        AgentLifecycleState::Queued | AgentLifecycleState::Cancelled => "◌",
    }
}

fn state_label(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Queued => "queued",
        AgentLifecycleState::Running => "running",
        AgentLifecycleState::Completed => "done",
        AgentLifecycleState::Failed => "failed",
        AgentLifecycleState::Cancelled => "cancelled",
    }
}
```

- [ ] **Step 2: Export component**

Modify `crates/neo-tui/src/transcript/mod.rs`:

```rust
mod swarm_card;
pub use swarm_card::SwarmCardComponent;
```

- [ ] **Step 3: Add tests**

Append to `crates/neo-tui/tests/multi_agent_transcript.rs`:

```rust
use neo_agent_core::multi_agent::{SwarmChildSnapshot, SwarmSnapshot};
use neo_tui::transcript::SwarmCardComponent;

#[test]
fn swarm_card_renders_orchestrating_before_children_run() {
    let child = running_delegate();
    let child = AgentSnapshot {
        state: AgentLifecycleState::Queued,
        ..child
    };
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-1".to_owned(),
        description: "Audit and fix Neo tool schemas".to_owned(),
        mode: AgentRunMode::Foreground,
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "Search tools: Grep, Find".to_owned(),
            agent: child,
        }],
    };
    let mut card = SwarmCardComponent::new(snapshot);

    let text = card
        .render(120)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Agent Swarm"));
    assert!(text.contains("001"));
    assert!(text.contains("Orchestrating"));
}

#[test]
fn swarm_card_renders_working_after_child_runs() {
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-1".to_owned(),
        description: "Audit and fix Neo tool schemas".to_owned(),
        mode: AgentRunMode::Foreground,
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "Search tools: Grep, Find".to_owned(),
            agent: running_delegate(),
        }],
    };
    let mut card = SwarmCardComponent::new(snapshot);

    let text = card
        .render(120)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Working"));
    assert!(text.contains("###......."));
}
```

- [ ] **Step 4: Run tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui swarm_card_renders
```

Expected: PASS.

## Phase 3: Event Routing

### Task 3.1: Route delegate events into transcript entries

**Files:**
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`
- Modify: `crates/neo-tui/src/transcript/store.rs`

- [ ] **Step 1: Add transcript store variants**

In `TranscriptEntry`, add variants shaped like:

```rust
Delegate(DelegateCardComponent),
DelegateSwarm(SwarmCardComponent),
```

Add upsert helpers to the transcript store:

```rust
pub fn upsert_delegate(&mut self, snapshot: AgentSnapshot) {
    let id = snapshot.id.as_str().to_owned();
    if let Some(entry) = self.entries.iter_mut().find(|entry| entry.delegate_id() == Some(&id)) {
        entry.update_delegate(snapshot);
        return;
    }
    self.entries.push(TranscriptEntry::Delegate(DelegateCardComponent::new(snapshot)));
}

pub fn upsert_delegate_swarm(&mut self, snapshot: SwarmSnapshot) {
    let id = snapshot.swarm_id.clone();
    if let Some(entry) = self.entries.iter_mut().find(|entry| entry.swarm_id() == Some(&id)) {
        entry.update_delegate_swarm(snapshot);
        return;
    }
    self.entries.push(TranscriptEntry::DelegateSwarm(SwarmCardComponent::new(snapshot)));
}
```

Use the existing transcript store's current entry vector and helper naming. The required behavior is: find an existing delegate/swarm entry by ID and update it; otherwise append a new entry.

- [ ] **Step 2: Add event routing**

In `TranscriptPane::apply_agent_event`, insert a new handler before generic tool events:

```rust
if self.apply_delegate_event(event) {
    return;
}
```

Add:

```rust
fn apply_delegate_event(&mut self, event: &AgentEvent) -> bool {
    match event {
        AgentEvent::DelegateStarted { agent, .. }
        | AgentEvent::DelegateUpdated { agent, .. }
        | AgentEvent::DelegateFinished { agent, .. } => {
            self.upsert_delegate(agent.clone());
            true
        }
        AgentEvent::DelegateSwarmStarted { swarm, .. }
        | AgentEvent::DelegateSwarmUpdated { swarm, .. }
        | AgentEvent::DelegateSwarmFinished { swarm, .. } => {
            self.upsert_delegate_swarm(swarm.clone());
            true
        }
        _ => false,
    }
}
```

- [ ] **Step 3: Add event-handler test**

Append to `crates/neo-tui/tests/multi_agent_transcript.rs`:

```rust
use neo_agent_core::AgentEvent;
use neo_tui::transcript::TranscriptPane;

#[test]
fn transcript_pane_upserts_delegate_card_from_events() {
    let mut pane = TranscriptPane::default();
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 1,
        agent: running_delegate(),
    });

    let rendered = pane.render_to_plain_text_for_test(120);

    assert!(rendered.contains("Gibbs Agent Running"));
}
```

Add a test-only helper named `render_to_plain_text_for_test` on `TranscriptPane` if it does not exist. It must render entries to plain text and stay behind `#[cfg(test)]` or a test-only module export.

- [ ] **Step 4: Run test**

Run:

```bash
cargo run -p xtask -- test -p neo-tui transcript_pane_upserts_delegate_card_from_events
```

Expected: PASS.

## Phase 4: Foreground Runtime Event Emission

### Task 4.1: Emit delegate events from `Delegate` and `DelegateSwarm`

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`

- [ ] **Step 1: Add event callback to `ToolContext`**

Add a narrowly scoped callback to `ToolContext` instead of overloading `ToolUpdateCallback`:

```rust
pub type ToolEventCallback = Arc<dyn Fn(AgentEvent) + Send + Sync>;
```

Add this field:

```rust
pub tool_event: Option<ToolEventCallback>,
```

Initialize it as `None` in `ToolContext::new`. In runtime tool dispatch, set it to a closure that emits through the current `EventSink`.

`DelegateTool` and `DelegateSwarmTool` must emit:

```rust
AgentEvent::DelegateStarted { turn, agent }
AgentEvent::DelegateUpdated { turn, agent }
AgentEvent::DelegateFinished { turn, agent }
```

Do not make tools directly depend on TUI types.

- [ ] **Step 2: Add focused runtime test**

In `crates/neo-agent-core/tests/multi_agent_runtime.rs`, add a fake turn test that calls `Delegate` and asserts the event stream contains `DelegateStarted` and `DelegateFinished`.

Use the existing runtime-turn test harness style. The assertion should look like:

```rust
assert!(events.iter().any(|event| matches!(event, AgentEvent::DelegateStarted { .. })));
assert!(events.iter().any(|event| matches!(event, AgentEvent::DelegateFinished { .. })));
```

- [ ] **Step 3: Run focused runtime test**

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core delegate_emits_foreground_events
```

Expected: PASS.

## Phase 5: Verification

### Task 5.1: Run P2 focused verification

- [ ] Run:

```bash
cargo run -p xtask -- test -p neo-tui multi_agent_transcript
```

Expected: PASS.

- [ ] Run:

```bash
cargo run -p xtask -- test -p neo-agent-core delegate_emits_foreground_events
```

Expected: PASS.

- [ ] Run:

```bash
cargo run -p xtask -- check
```

Expected: PASS unless unrelated dirty-worktree work breaks the global check. Report unrelated breakage without reverting files.

## Handoff Notes For P3

- P3 owns background mode, `Ctrl+B` detach, `BackgroundTaskManager` integration, `/tasks` rows, and mailbox delivery.
- Keep transcript cards inline in the main chat.
- Do not add an agent page or panel.
