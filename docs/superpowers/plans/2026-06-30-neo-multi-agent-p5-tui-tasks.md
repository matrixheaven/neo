# Neo Multi-Agent P5 TUI And Tasks UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Polish multi-agent transcript and `/tasks` UX so delegates and swarms look and behave like live Kimi-style cards instead of static prompt snapshots.

**Architecture:** Keep all rendering inside the chat transcript. Delegate cards render short titles, role labels, bounded activity tails, and useful final summaries. Swarm cards render first-class swarm state, aggregate counts, smoothed progress, and latest child activity/result instead of permanently showing original prompts. `/tasks` uses the same output semantics as `TaskOutput` and the canonical `cancelled` vocabulary.

**Tech Stack:** Rust 2024, `neo-tui` primitive `Line`/`Span`, transcript components, task browser, `TuiTheme`, `cargo nextest run`.

---

## Source Spec

Use `/Users/chenyuanhao/Workspace/neo/docs/superpowers/specs/2026-06-30-neo-multi-agent-hardening-design.md`.

This plan covers:

- Section 19 TUI and Transcript.
- Section 13 TaskOutput UI implications.
- Section 14 TaskStop vocabulary in task browser.
- Acceptance criteria under TUI.

P1 and P2 must be complete first. P4 is recommended before this plan so role display labels come from `AgentProfile`.

## Constraints

- Start implementation with `icm recall-context "Neo multi-agent P5 TUI tasks transcript" --limit 5`.
- Use CodeGraph before grep/read for symbol discovery in this repo.
- Do not run bare `cargo test`; use `cargo nextest run ...`.
- Do not mutate git unless the user explicitly authorizes that exact command.
- Do not create a separate swarm page. Transcript cards stay in the chat transcript.
- Do not show the full child prompt forever in swarm rows.
- Do not use the word `stopped` for delegate/swarm states.
- Do not let long titles hide `tools`, elapsed time, or token count from delegate headers.

## Kimi Reference To Check Before Coding

Read these reference paths before implementation:

- `docs/kimi-code/packages/agent-tui/src/components/tool-call/agent.ts`
- `docs/kimi-code/packages/agent-tui/src/components/tool-call/agent-swarm.ts`
- `docs/kimi-code/packages/agent-tui/src/components/tool-call/__tests__/agent.test.ts`
- `docs/kimi-code/packages/agent-tui/src/components/tool-call/__tests__/agent-swarm.test.ts`

Purpose: match the interaction shape, max-height behavior, and live swarm progress feel. Do not port TypeScript directly.

## Current Code Touchpoints

- `crates/neo-tui/src/transcript/delegate_card.rs`
  - Already renders a delegate header and recent activity.
- `crates/neo-tui/src/transcript/swarm_card.rs`
  - Already renders a swarm card and progress.
- `crates/neo-tui/src/transcript/store.rs`
  - Upserts delegate/swarm snapshots from events.
- `crates/neo-agent-core/src/multi_agent/state.rs`
  - Snapshot fields drive TUI.
- `crates/neo-agent/src/modes/task_browser.rs`
  - `/tasks` browser item mapping and details.
- `crates/neo-tui/tests/multi_agent_transcript.rs`
- `crates/neo-agent/src/modes/task_browser.rs` module tests.

## File Structure

Modify:

- `crates/neo-agent-core/src/multi_agent/state.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-tui/src/transcript/delegate_card.rs`
- `crates/neo-tui/src/transcript/swarm_card.rs`
- `crates/neo-tui/src/transcript/store.rs`
- `crates/neo-agent/src/modes/task_browser.rs`
- `crates/neo-tui/tests/multi_agent_transcript.rs`
- `crates/neo-agent/src/modes/task_browser.rs` tests

Do not modify provider/model code.

## Desired End State

- Delegate card header shape:

```text
● Gibbs Agent Running (Implement Task 1: PlanBox border fix) · 3 tools · 24s · 25.6k tok
```

- Header uses role display label and `task_title`, not full prompt.
- Activity area has stable max height and older rows scroll out.
- Completed delegate cards avoid duplicating parent tool result text.
- Swarm card starts near zero progress for queued work and updates through intermediate states.
- Swarm rows show child latest activity or final result, not full prompt forever.
- `/tasks` shows background agents and swarms with useful output and `cancelled` vocabulary.

## Task 1: Add Short Task Titles To Agent Snapshot

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/state.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing title derivation test**

Append to `crates/neo-tui/tests/multi_agent_transcript.rs`:

```rust
#[test]
fn delegate_card_header_uses_task_title_not_full_prompt() {
    let mut snapshot = running_delegate();
    snapshot.task = "Read crates/neo-agent-core/src/lib.rs, count the public modules, then explain every module in detail with exact line references".to_owned();
    snapshot.task_title = "Count public modules".to_owned();

    let text = plain(DelegateCardComponent::new(snapshot).render_with_theme(80, &TuiTheme::default()));

    assert!(text.contains("(Count public modules)"), "{text}");
    assert!(!text.contains("explain every module in detail"), "{text}");
    assert!(text.contains("tools"), "{text}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL because `task_title` does not exist or card uses `task`.

- [ ] **Step 3: Add `task_title` to `AgentSnapshot`**

In `state.rs`:

```rust
pub struct AgentSnapshot {
    pub id: AgentId,
    pub display_name: AgentDisplayName,
    pub path: AgentPath,
    pub role: AgentRole,
    pub mode: AgentRunMode,
    pub state: AgentLifecycleState,
    pub task_title: String,
    pub task: String,
    pub tool_count: usize,
    pub token_count: usize,
    pub elapsed: Duration,
    pub latest_text: Option<String>,
    #[serde(default)]
    pub activity: Vec<AgentActivityEntry>,
    pub outcome: Option<AgentTerminalOutcome>,
}
```

Update every fixture and constructor. In test fixtures, set `task_title` to a short literal such as `"Map auth module"`, `"Completed task"`, or `"Swarm child"`.

- [ ] **Step 4: Add deterministic title helper in runtime**

In `runtime.rs`:

```rust
fn derive_task_title(task: &str, explicit: Option<&str>) -> String {
    if let Some(title) = explicit.map(str::trim).filter(|title| !title.is_empty()) {
        return truncate_title(title);
    }
    let first_line = task.lines().next().unwrap_or(task).trim();
    truncate_title(first_line)
}

fn truncate_title(title: &str) -> String {
    const MAX_CHARS: usize = 64;
    let mut chars = title.chars();
    let short = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        format!("{short}...")
    } else {
        short
    }
}
```

Use `derive_task_title(&request.task, request.title.as_deref())` when creating or resuming an agent.

- [ ] **Step 5: Update delegate card header**

In `delegate_card.rs`, replace `short_task_title(&self.snapshot.task)` with `self.snapshot.task_title.as_str()`.

- [ ] **Step 6: Run title test**

Run:

```bash
```

Expected: PASS.

## Task 2: Bound Delegate Activity Tail And Avoid Duplicate Final Output

**Files:**

- Modify: `crates/neo-tui/src/transcript/delegate_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing activity max-height test**

Append:

```rust
#[test]
fn delegate_card_keeps_only_recent_activity_rows_when_collapsed() {
    let mut snapshot = running_delegate();
    snapshot.activity = (0..8)
        .map(|index| AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: format!("activity row {index}"),
                thinking: index % 2 == 0,
            },
        })
        .collect();

    let text = plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()));

    assert!(!text.contains("activity row 0"), "{text}");
    assert!(!text.contains("activity row 1"), "{text}");
    assert!(text.contains("activity row 7"), "{text}");
    assert!(text.lines().count() <= 7, "{text}");
}
```

- [ ] **Step 2: Add failing duplicate-output test**

Append:

```rust
#[test]
fn completed_delegate_card_does_not_duplicate_identical_latest_text_and_summary() {
    let mut snapshot = completed_delegate();
    snapshot.latest_text = Some("34 lines".to_owned());
    snapshot.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: "34 lines".to_owned(),
            thinking: false,
        },
    });
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "34 lines".to_owned(),
        is_error: false,
    });

    let text = plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()));

    assert_eq!(text.matches("34 lines").count(), 1, "{text}");
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run:

```bash
```

Expected: FAIL if duplicate output or too many rows render.

- [ ] **Step 4: Make recent activity bounded and summary-aware**

In `delegate_card.rs`, replace `recent_activity` with:

```rust
fn recent_activity<'a>(
    activity: &'a [AgentActivityEntry],
    outcome: Option<&AgentTerminalOutcome>,
) -> Vec<&'a AgentActivityEntry> {
    let duplicate_summary = outcome.map(|outcome| outcome.summary.trim());
    let filtered = activity
        .iter()
        .filter(|entry| match &entry.kind {
            AgentActivityKind::Text { text, .. } => {
                Some(text.trim()) != duplicate_summary
            }
            AgentActivityKind::Tool { .. } => true,
        })
        .collect::<Vec<_>>();
    let start = filtered.len().saturating_sub(MAX_SINGLE_AGENT_ACTIVITY_ROWS);
    filtered[start..].to_vec()
}
```

Call it as:

```rust
for activity in recent_activity(&self.snapshot.activity, self.snapshot.outcome.as_ref()) {
    lines.push(render_activity(activity, width, theme));
}
```

When rendering outcome, skip it if it is identical to the last rendered text. Use:

```rust
let rendered_summary = lines
    .iter()
    .any(|line| line.to_plain_string().contains(outcome.summary.trim()));
if !rendered_summary {
    lines.push(...);
}
```

Use the existing line-to-string helper if present; if not present, add a private `line_plain_text(&Line) -> String` helper in this file.

- [ ] **Step 5: Run activity tests**

Run:

```bash
```

Expected: PASS.

## Task 3: Render Role Labels From Profiles

**Files:**

- Modify: `crates/neo-tui/src/transcript/delegate_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing role-label test**

Append:

```rust
#[test]
fn delegate_card_header_uses_role_display_label() {
    let mut snapshot = running_delegate();
    snapshot.display_name = AgentDisplayName::new("Hypatia");
    snapshot.role = AgentRole::Explorer;
    snapshot.task_title = "Map auth module".to_owned();

    let text = plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()));

    assert!(text.contains("Hypatia Explorer Agent Running"), "{text}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL if card always prints `Agent`.

- [ ] **Step 3: Add display label function**

In `delegate_card.rs`:

```rust
fn role_label(role: AgentRole) -> &'static str {
    neo_agent_core::multi_agent::AgentProfile::for_role(role).display_label
}
```

Update header format to:

```rust
format!(
    " {} Agent {} ({}) · {} tools · {} · {} tok",
    role_label(self.snapshot.role),
    state_label(self.snapshot.state),
    self.snapshot.task_title,
    self.snapshot.tool_count,
    format_elapsed(self.snapshot.elapsed.as_secs()),
    format_token_count(self.snapshot.token_count)
)
```

Keep display name as the colored prefix before this string.

- [ ] **Step 4: Run role-label test**

Run:

```bash
```

Expected: PASS.

## Task 4: Improve Swarm Progress And Row Content

**Files:**

- Modify: `crates/neo-tui/src/transcript/swarm_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing queued-progress test**

Append:

```rust
#[test]
fn swarm_card_progress_starts_near_zero_when_all_children_queued() {
    let snapshot = swarm_with_child_states(vec![
        AgentLifecycleState::Queued,
        AgentLifecycleState::Queued,
        AgentLifecycleState::Queued,
    ]);

    let text = plain(SwarmCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()));

    assert!(text.contains("Running"), "{text}");
    assert!(
        text.contains("0%") || text.contains("1%") || text.contains("2%"),
        "{text}"
    );
    assert!(!text.contains("100%"), "{text}");
}
```

- [ ] **Step 2: Add failing latest-activity row test**

Append:

```rust
#[test]
fn swarm_card_child_row_prefers_latest_activity_over_full_prompt() {
    let mut snapshot = swarm_with_child_states(vec![AgentLifecycleState::Running]);
    snapshot.children[0].agent.task = "Run a very long investigation prompt that should not remain visible after activity arrives".to_owned();
    snapshot.children[0].agent.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "call_1".to_owned(),
            name: "Read".to_owned(),
            summary: Some("crates/neo-agent-core/src/lib.rs".to_owned()),
            failed: false,
        },
    });

    let text = plain(SwarmCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()));

    assert!(text.contains("Used Read"), "{text}");
    assert!(!text.contains("very long investigation prompt"), "{text}");
}
```

Add this helper in `crates/neo-tui/tests/multi_agent_transcript.rs` before the new swarm tests:

```rust
fn swarm_with_child_states(states: Vec<AgentLifecycleState>) -> SwarmSnapshot {
    let aggregate = SwarmAggregate::from_states(states.iter().copied());
    SwarmSnapshot {
        swarm_id: "swarm_test".to_owned(),
        description: "Test swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: states.len().max(1),
        aggregate,
        children: states
            .into_iter()
            .enumerate()
            .map(|(index, state)| {
                let name = AgentDisplayName::new(format!("Agent{index}"));
                SwarmChildSnapshot {
                    item_index: index + 1,
                    item: format!("item-{index}"),
                    agent: AgentSnapshot {
                        id: AgentId::from_suffix_for_test(&format!("swarm_child_{index}")),
                        display_name: name.clone(),
                        path: AgentPath::swarm_child("swarm_test", &name),
                        role: AgentRole::Coder,
                        mode: AgentRunMode::Foreground,
                        state,
                        task_title: format!("Child {index}"),
                        task: format!("Child prompt {index}"),
                        tool_count: 0,
                        token_count: 0,
                        elapsed: Duration::from_secs(0),
                        latest_text: None,
                        activity: Vec::new(),
                        outcome: None,
                    },
                }
            })
            .collect(),
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run:

```bash
```

Expected: FAIL if progress jumps too high or row content is still prompt-first.

- [ ] **Step 4: Add progress estimator**

In `swarm_card.rs`:

```rust
fn estimated_swarm_progress(snapshot: &SwarmSnapshot) -> u8 {
    if snapshot.aggregate.total == 0 {
        return 0;
    }
    let total = snapshot.aggregate.total as f32;
    let terminal = (snapshot.aggregate.completed
        + snapshot.aggregate.failed
        + snapshot.aggregate.cancelled
        + snapshot.aggregate.timed_out) as f32;
    let running_credit = snapshot
        .children
        .iter()
        .filter(|child| child.agent.state == AgentLifecycleState::Running)
        .map(|child| {
            let tool_credit = (child.agent.tool_count as f32 * 0.04).min(0.35);
            0.10 + tool_credit
        })
        .sum::<f32>();
    let queued_floor = if terminal == 0.0 && running_credit == 0.0 {
        0.0
    } else {
        0.02
    };
    (((terminal + running_credit) / total).max(queued_floor) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8
}
```

Use this for header percent and bar fill. Completed all-terminal swarms naturally reach 100%.

- [ ] **Step 5: Add latest row text helper**

In `swarm_card.rs`:

```rust
fn child_row_tail(child: &SwarmChildSnapshot) -> String {
    if let Some(outcome) = &child.agent.outcome {
        return outcome.summary.clone();
    }
    if let Some(activity) = child.agent.activity.last() {
        return match &activity.kind {
            AgentActivityKind::Tool { name, summary, failed, .. } => {
                let verb = if *failed { "Failed" } else { "Used" };
                match summary {
                    Some(summary) => format!("{verb} {name} ({summary})"),
                    None => format!("{verb} {name}"),
                }
            }
            AgentActivityKind::Text { text, .. } => text.clone(),
        };
    }
    child.agent.task_title.clone()
}
```

Use `child_row_tail(child)` in each child row instead of `child.agent.task`.

- [ ] **Step 6: Run swarm card tests**

Run:

```bash
```

Expected: PASS.

## Task 5: Align `/tasks` With Delegate And Swarm Output

**Files:**

- Modify: `crates/neo-agent/src/modes/task_browser.rs`
- Modify: tests inside `crates/neo-agent/src/modes/task_browser.rs`

- [ ] **Step 1: Add failing task browser status test**

In the task browser tests module, add:

```rust
#[test]
fn task_browser_uses_cancelled_vocabulary_for_interrupted_tasks() {
    let cancelled = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Cancelled));

    assert_eq!(cancelled.status, TaskBrowserStatus::Cancelled);
    assert_eq!(cancelled.status.label(), "cancelled");
    assert!(cancelled.status.is_interrupted());
}
```

- [ ] **Step 2: Add failing swarm detail test**

Add:

```rust
#[test]
fn task_browser_swarm_details_include_aggregate_and_child_results() {
    let item = snapshot_to_item(&delegate_swarm_snapshot_with_completed_children());
    let details = item.detail_lines.join("\n");

    assert!(details.contains("aggregate:"), "{details}");
    assert!(details.contains("completed"), "{details}");
    assert!(details.contains("agent_"), "{details}");
}
```

Use existing swarm fixture helpers if present. If not, add a local `delegate_swarm_snapshot_with_completed_children()` helper that builds a `BackgroundTaskSnapshot` with a completed `SwarmSnapshot`.

- [ ] **Step 3: Run tests to verify they fail**

Run:

```bash
```

Expected: FAIL until status enum and detail mapping are updated.

- [ ] **Step 4: Rename task browser status**

In `task_browser.rs`, replace `TaskBrowserStatus::Stopped` with:

```rust
TaskBrowserStatus::Cancelled
```

Return label:

```rust
Self::Cancelled => "cancelled",
```

Map `BackgroundTaskStatus::Cancelled` to `TaskBrowserStatus::Cancelled`.

- [ ] **Step 5: Add delegate/swarm details**

In `snapshot_to_item`, for delegate snapshots include:

```rust
detail_lines.push(format!("agent_id: {}", agent.id.as_str()));
detail_lines.push(format!("status: {}", agent.state.as_str()));
if let Some(outcome) = &agent.outcome {
    detail_lines.push(format!("summary: {}", outcome.summary));
}
for activity in agent.activity.iter().rev().take(4).rev() {
    detail_lines.push(format!("activity: {}", format_agent_activity(activity)));
}
```

For swarm snapshots include:

```rust
detail_lines.push(format!("swarm_id: {}", swarm.swarm_id));
detail_lines.push(format!("status: {}", swarm.state.as_str()));
detail_lines.push(format!(
    "aggregate: total={} queued={} running={} completed={} failed={} cancelled={} timed_out={}",
    swarm.aggregate.total,
    swarm.aggregate.queued,
    swarm.aggregate.running,
    swarm.aggregate.completed,
    swarm.aggregate.failed,
    swarm.aggregate.cancelled,
    swarm.aggregate.timed_out,
));
for child in &swarm.children {
    detail_lines.push(format!(
        "{} {} {} {}",
        child.item_index,
        child.agent.id.as_str(),
        child.agent.state.as_str(),
        child.agent
            .outcome
            .as_ref()
            .map(|outcome| outcome.summary.as_str())
            .unwrap_or(child.agent.task_title.as_str())
    ));
}
```

Add `format_agent_activity` beside task browser formatting helpers:

```rust
fn format_agent_activity(activity: &AgentActivityEntry) -> String {
    match &activity.kind {
        AgentActivityKind::Tool { name, summary, failed, .. } => {
            let verb = if *failed { "Failed" } else { "Used" };
            match summary {
                Some(summary) => format!("{verb} {name} ({summary})"),
                None => format!("{verb} {name}"),
            }
        }
        AgentActivityKind::Text { text, .. } => text.clone(),
    }
}
```

- [ ] **Step 6: Run task browser tests**

Run:

```bash
```

Expected: PASS.

## Task 6: Verify Color And Theme Usage For Swarm/Workflow Cards

**Files:**

- Modify: `crates/neo-tui/src/transcript/swarm_card.rs`
- Modify: `crates/neo-tui/src/transcript/workflow_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing ANSI color test for swarm status**

Append:

```rust
#[test]
fn swarm_card_uses_theme_colors_for_status_and_progress() {
    let theme = TuiTheme::default();
    let snapshot = swarm_with_child_states(vec![AgentLifecycleState::Running]);
    let rows = SwarmCardComponent::new(snapshot).render_with_theme(140, &theme);
    let ansi = ansi(&rows);

    assert_ansi_contains_color(&ansi, theme.status_info);
    assert_ansi_contains_color(&ansi, theme.accent);
}
```

- [ ] **Step 2: Run test to verify it fails if style default is used**

Run:

```bash
```

Expected: PASS if current colors are already wired; FAIL if `Style::default()` erases theme color.

- [ ] **Step 3: Replace default styles with theme styles**

In `swarm_card.rs`, ensure:

```rust
let accent = Style::default().fg(theme.accent);
let muted = Style::default().fg(theme.text_muted);
let primary = Style::default().fg(theme.text_primary);
let status = Style::default().fg(status_color(snapshot.state, theme));
```

Use `accent` for progress fill and `status` for status markers. Do not use uncolored `Style::default()` for header/status/progress.

In `workflow_card.rs`, perform the same theme-style replacement for header and state markers.

- [ ] **Step 4: Run color test**

Run:

```bash
```

Expected: PASS.

## Task 7: P5 Verification And Commit Boundary

**Files:**

- Verify all files changed by this plan.

- [ ] **Step 1: Run TUI multi-agent transcript tests**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 2: Run task browser tests**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 3: Scan for stale prompt-first rendering and stopped vocabulary**

Run:

```bash
rg -n "Stopped|stopped|snapshot\\.task\\)|child\\.agent\\.task" crates/neo-tui/src/transcript crates/neo-agent/src/modes/task_browser.rs crates/neo-tui/tests crates/neo-agent/src/modes/task_browser.rs
```

Expected:

- No `Stopped` or `stopped` in delegate/swarm task vocabulary.
- No swarm row rendering that uses `child.agent.task` directly.
- Delegate card rendering must use `snapshot.task_title`; direct `snapshot.task` use in `delegate_card.rs` is a failure.

- [ ] **Step 4: Commit if authorized**

Only if the user has explicitly authorized git mutation in this session:

```bash
git add crates/neo-agent-core/src/multi_agent/state.rs \
  crates/neo-agent-core/src/multi_agent/runtime.rs \
  crates/neo-tui/src/transcript/delegate_card.rs \
  crates/neo-tui/src/transcript/swarm_card.rs \
  crates/neo-tui/src/transcript/store.rs \
  crates/neo-agent/src/modes/task_browser.rs \
  crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "feat: polish multi-agent transcript and tasks ux"
```

Expected: one logical commit for P5.
