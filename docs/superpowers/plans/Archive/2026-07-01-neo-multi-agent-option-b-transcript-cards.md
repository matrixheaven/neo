# Neo Multi-Agent Option B Transcript Cards Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every Neo Multi-Agent transcript card follow the approved Option B design: random agent names are primary identity, roles are badges, and single delegates, delegate groups, and swarms render as living chat-native child-agent transcripts.

**Architecture:** Keep the canonical runtime state from `AgentSnapshot`, `AgentActivityKind`, `AgentToolActivityPhase`, `AgentToolOutputPreview`, `SwarmSnapshot`, and `SwarmProgressEstimator`. Concentrate UI composition in `crates/neo-tui/src/transcript/child_activity.rs`, `delegate_card.rs`, `delegate_group.rs`, and `swarm_card.rs`, with `TranscriptStore` preserving grouping/upsert semantics. Tests live in `crates/neo-tui/tests/multi_agent_transcript.rs` plus narrow runtime regressions in `crates/neo-agent-core/tests/multi_agent_runtime.rs`.

**Tech Stack:** Rust 2024, `cargo nextest`, Neo TUI `Line`/`Span`/`Style`, `TuiTheme`, `neo_agent_core::multi_agent`, existing `TranscriptPane` event application.

---

## Ground Rules

- Do not use subagents that mutate git. Subagents must not run `git add`, `git commit`, `git checkout`, `git reset`, `git stash`, `git clean`, `git rebase`, `git push`, `git merge`, `git rm`, or branch deletion.
- This repository has shared in-progress work. Before editing a task, run `git status --short` and inspect any already-modified target file.
- Commit checkpoints in this plan are written as authorization gates. In Neo, git mutations require explicit user authorization for the exact command before they are executed.
- Use narrow verification only. Do not use bare `cargo test` as evidence.
- The design source of truth is `docs/superpowers/specs/2026-07-01-neo-multi-agent-living-transcript-design.md`.

## File Structure

- Modify: `crates/neo-tui/src/transcript/child_activity.rs`
  - Owns shared child-agent transcript row grammar: tool rows, output previews, thinking rows, streaming body rows, final rows, duplicate suppression.
- Modify: `crates/neo-tui/src/transcript/delegate_card.rs`
  - Renders standalone delegate cards in Option B shape.
- Modify: `crates/neo-tui/src/transcript/delegate_group.rs`
  - Renders same-turn root delegates as one chat-native group card using the shared child activity renderer.
- Modify: `crates/neo-tui/src/transcript/swarm_card.rs`
  - Renders collapsed and expanded `DelegateSwarm` cards with Bayesian progress plus visible agent identities.
- Modify: `crates/neo-tui/src/transcript/entry/copy.rs`
  - Keep copied text aligned with Option B labels when copy output contains delegate or swarm identity text.
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`
  - Main TUI regression suite for Option B.
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`
  - Runtime contract checks for activity phase, output preview, trimming, timing, detach, and terminal reasons.
- Read: `crates/neo-tui/src/transcript/store.rs`
  - Already groups same-turn delegates and merges swarm updates; implementation tasks do not edit it unless a listed test fails specifically because grouping/upsert behavior regressed.

## Task 1: Add Option B TUI Fixtures And Identity Tests

**Files:**
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`
- Test: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add reusable Option B fixtures**

Add these helpers near the existing `running_delegate()` helper. If names collide with existing helpers, update the existing helper bodies to match this content instead of creating duplicate fixture paths.

```rust
fn option_b_delegate(
    id_suffix: &str,
    name: &str,
    role: AgentRole,
    state: AgentLifecycleState,
    title: &str,
) -> AgentSnapshot {
    let display_name = AgentDisplayName::new(name);
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id_suffix),
        display_name: display_name.clone(),
        path: AgentPath::root_child(&display_name),
        role,
        mode: AgentRunMode::Foreground,
        state,
        task: format!("{title}\n\nFull prompt that must not replace the display name."),
        task_title: title.to_owned(),
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
        started_at_ms: matches!(state, AgentLifecycleState::Running).then_some(1_000),
        terminal_at_ms: state.is_terminal().then_some(31_000),
        detached_from_foreground: false,
        terminal_reason: terminal_reason_for_state(state),
        tool_count: 0,
        token_count: 0,
        elapsed: Duration::from_secs(0),
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    }
}

fn option_b_running_delegate() -> AgentSnapshot {
    let mut snapshot = option_b_delegate(
        "nova",
        "Nova",
        AgentRole::Coder,
        AgentLifecycleState::Running,
        "角色对比测试 coder",
    );
    snapshot.tool_count = 3;
    snapshot.token_count = 22_700;
    snapshot.elapsed = Duration::from_secs(21);
    snapshot.activity = vec![
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "read-delegate".to_owned(),
                name: "Read".to_owned(),
                summary: Some("crates/neo-agent-core/src/tools/delegate.rs".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: None,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "bash-nextest".to_owned(),
                name: "Bash".to_owned(),
                summary: Some("cargo nextest run -p neo-agent-core ...".to_owned()),
                phase: AgentToolActivityPhase::Ongoing,
                output: Some(AgentToolOutputPreview {
                    text: "running: cargo nextest run -p neo-agent-core ...\nCompiling neo-agent-core v0.1.0".to_owned(),
                    is_error: false,
                    truncated: true,
                    tail: true,
                }),
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "Let me verify the state mutation path before editing.".to_owned(),
                thinking: true,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "I found the foreground aggregation issue. Next I will make the renderer change.".to_owned(),
                thinking: false,
            },
        },
    ];
    snapshot.latest_text =
        Some("I found the foreground aggregation issue. Next I will make the renderer change.".to_owned());
    snapshot
}
```

- [ ] **Step 2: Add failing single delegate identity test**

Append this test. It should fail before the renderer is changed because current output still resembles `Gibbs Coder Agent Running` / role-first wording.

```rust
#[test]
fn option_b_single_delegate_shows_name_first_and_role_badge() {
    let text = plain(
        DelegateCardComponent::new(option_b_running_delegate())
            .render_with_theme(140, &TuiTheme::default()),
    )
    .join("\n");

    assert!(text.contains("● Nova  [Coder]"), "{text}");
    assert!(text.contains("角色对比测试 coder"), "{text}");
    assert!(text.contains("running"), "{text}");
    assert!(text.contains("21s"), "{text}");
    assert!(text.contains("22.7k"), "{text}");
    assert!(
        !text.contains("Coder Agent Running"),
        "role must be a badge, not the primary visible name: {text}"
    );
}
```

- [ ] **Step 3: Run the new test and verify it fails**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_single_delegate_shows_name_first_and_role_badge
```

Expected: FAIL with the assertion showing the old header shape or missing `[Coder]`.

- [ ] **Step 4: Commit checkpoint only if authorized**

Ask the user for explicit authorization before any git mutation. If authorized, run exactly:

```bash
git add crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "test: capture option b delegate identity"
```

Expected: one test-only commit. If not authorized, leave the file modified and continue.

## Task 2: Implement Shared Option B Child Activity Rows

**Files:**
- Modify: `crates/neo-tui/src/transcript/child_activity.rs`
- Test: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing child activity grammar test**

Append this test to `crates/neo-tui/tests/multi_agent_transcript.rs`.

```rust
#[test]
fn option_b_child_activity_orders_tools_thinking_body_and_final() {
    let mut snapshot = option_b_running_delegate();
    snapshot.state = AgentLifecycleState::Completed;
    snapshot.terminal_at_ms = Some(31_000);
    snapshot.terminal_reason = Some(AgentTerminalReason::Completed);
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "All edits applied. The card now shows agent name first.".to_owned(),
        is_error: false,
    });

    let rows = plain(
        DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()),
    );
    let text = rows.join("\n");

    let used_index = rows.iter().position(|row| row.contains("• Used Read")).expect("used row");
    let using_index = rows.iter().position(|row| row.contains("• Using Bash")).expect("using row");
    let thinking_index = rows.iter().position(|row| row.contains("◌ thinking")).expect("thinking row");
    let body_index = rows.iter().position(|row| row.contains("│ I found")).expect("body row");
    let final_index = rows.iter().position(|row| row.contains("└ All edits applied")).expect("final row");

    assert!(used_index < using_index, "{text}");
    assert!(using_index < thinking_index, "{text}");
    assert!(thinking_index < body_index, "{text}");
    assert!(body_index < final_index, "{text}");
    assert_eq!(final_index, rows.len() - 1, "{text}");
    assert!(text.contains("running: cargo nextest run -p neo-agent-core"), "{text}");
    assert_eq!(text.matches("All edits applied").count(), 1, "{text}");
}
```

- [ ] **Step 2: Run the child activity grammar test and verify it fails**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_child_activity_orders_tools_thinking_body_and_final
```

Expected: FAIL because current thinking/body rows do not use the exact Option B `◌ thinking`, `│`, and final ordering.

- [ ] **Step 3: Update `ChildActivityView` and row renderers**

In `crates/neo-tui/src/transcript/child_activity.rs`, replace `ChildToolRow` and the render helpers with this shape. Keep existing imports and helper functions that still apply.

```rust
pub struct ChildToolRow<'a> {
    pub name: &'a str,
    pub summary: Option<&'a str>,
    pub phase: AgentToolActivityPhase,
    pub output: Option<&'a AgentToolOutputPreview>,
}

pub fn render_child_tool_row(
    row: &ChildToolRow<'_>,
    width: usize,
    indent: &str,
    theme: &TuiTheme,
) -> Vec<Line> {
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
        .map(|value| format!("  {}", one_line(value)))
        .unwrap_or_default();
    let mut lines = vec![
        Line::from_spans(vec![
            Span::raw(indent.to_owned()),
            Span::styled(marker, marker_style),
            Span::raw(format!(" {verb} ")),
            Span::styled(row.name.to_owned(), Style::default().fg(theme.brand)),
            Span::styled(suffix, Style::default().fg(theme.text_muted)),
        ])
        .truncate_to_width(width),
    ];
    if let Some(output) = row.output {
        lines.extend(render_output_preview(output, width, indent, theme));
    }
    lines
}

pub fn render_child_thinking(
    text: &str,
    width: usize,
    indent: &str,
    theme: &TuiTheme,
) -> Vec<Line> {
    let preview = tail_non_empty_lines(text, THINKING_PREVIEW_LINES);
    if preview.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        Line::styled(
            format!("{indent}◌ thinking"),
            Style::default().fg(theme.text_muted),
        )
        .truncate_to_width(width),
    ];
    lines.extend(preview.into_iter().map(|line| {
        Line::styled(
            format!("{indent}  {}", compact_chars(&line, FINAL_TEXT_CHARS)),
            Style::default().fg(theme.text_muted),
        )
        .truncate_to_width(width)
    }));
    lines
}

pub fn render_child_body(
    text: &str,
    width: usize,
    indent: &str,
    theme: &TuiTheme,
) -> Option<Line> {
    let text = one_line(text);
    (!text.is_empty()).then(|| {
        Line::styled(
            format!("{indent}│ {}", compact_chars(&text, FINAL_TEXT_CHARS)),
            Style::default().fg(theme.text_primary),
        )
        .truncate_to_width(width)
    })
}
```

In `crates/neo-tui/src/transcript/mod.rs`, add `render_child_body` to the existing `pub(crate) use child_activity::{ ... }` list:

```rust
pub(crate) use child_activity::{
    MAX_CHILD_TOOL_ROWS, can_detach, child_activity_view, compact_chars, display_elapsed,
    format_elapsed, format_token_count, one_line, render_child_body, render_child_final,
    render_child_thinking, render_child_tool_row, role_label,
};
```

Update `tool_row` so it exposes display fields only:

```rust
fn tool_row(entry: &AgentActivityEntry) -> Option<ChildToolRow<'_>> {
    match &entry.kind {
        AgentActivityKind::Tool {
            name,
            summary,
            phase,
            output,
        } => Some(ChildToolRow {
            name,
            summary: summary.as_deref(),
            phase: *phase,
            output: output.as_ref(),
        }),
        AgentActivityKind::Text { .. } => None,
    }
}
```

- [ ] **Step 4: Update final text selection to avoid duplicate body/final rows**

In `child_activity_view`, keep final text from `outcome.summary` when present, and use latest non-thinking text as body text only when it differs from the final text. Add a `body_text: Option<String>` field to `ChildActivityView`:

```rust
pub struct ChildActivityView<'a> {
    pub tools: Vec<ChildToolRow<'a>>,
    pub thinking: Option<String>,
    pub body_text: Option<String>,
    pub final_text: Option<String>,
    pub final_is_error: bool,
}
```

Use this comparison before returning:

```rust
let activity_text = latest_text_activity(activity_window, false);
let final_text = snapshot
    .outcome
    .as_ref()
    .map(|outcome| outcome.summary.clone())
    .or_else(|| {
        if snapshot.state.is_terminal() {
            activity_text.clone().or_else(|| snapshot.latest_text.clone())
        } else {
            None
        }
    });
let body_text = if snapshot.state.is_terminal() {
    activity_text.filter(|text| {
        final_text
            .as_ref()
            .is_none_or(|final_text| one_line(final_text) != one_line(text))
    })
} else {
    activity_text.or_else(|| snapshot.latest_text.clone())
};
```

- [ ] **Step 5: Run the child activity grammar test and verify it passes**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_child_activity_orders_tools_thinking_body_and_final
```

Expected: PASS.

- [ ] **Step 6: Commit checkpoint only if authorized**

Ask for explicit authorization. If granted:

```bash
git add crates/neo-tui/src/transcript/child_activity.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "refactor: standardize child agent activity rows"
```

Expected: one focused commit for shared row grammar.

## Task 3: Render Single Delegate Cards In Option B Shape

**Files:**
- Modify: `crates/neo-tui/src/transcript/delegate_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`
- Test: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing completed/backgrounded single-card tests**

Append:

```rust
#[test]
fn option_b_completed_delegate_uses_name_badge_and_final_row() {
    let mut snapshot = option_b_running_delegate();
    snapshot.state = AgentLifecycleState::Completed;
    snapshot.terminal_at_ms = Some(31_000);
    snapshot.terminal_reason = Some(AgentTerminalReason::Completed);
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "All edits applied. The card now shows agent name first.".to_owned(),
        is_error: false,
    });

    let text = plain(
        DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()),
    )
    .join("\n");

    assert!(text.contains("✓ Nova  [Coder]"), "{text}");
    assert!(text.contains("done"), "{text}");
    assert!(text.contains("4 tools") || text.contains("3 tools"), "{text}");
    assert!(text.contains("└ All edits applied"), "{text}");
    assert!(!text.contains("Agent Completed"), "{text}");
}

#[test]
fn option_b_backgrounded_delegate_uses_backgrounded_label_without_detach_hint() {
    let mut snapshot = option_b_running_delegate();
    snapshot.detached_from_foreground = true;

    let text = plain(
        DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()),
    )
    .join("\n");

    assert!(text.contains("● Nova  [Coder]"), "{text}");
    assert!(text.contains("backgrounded"), "{text}");
    assert!(!text.contains("Ctrl+B"), "{text}");
}
```

- [ ] **Step 2: Run single-card tests and verify they fail**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_single_delegate
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_completed_delegate
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_backgrounded_delegate
```

Expected: FAIL until `DelegateCardComponent` header/body rendering is updated.

- [ ] **Step 3: Implement Option B header helpers**

In `crates/neo-tui/src/transcript/delegate_card.rs`, add these helpers near the existing status helpers:

```rust
fn status_text(phase: DelegateDisplayPhase) -> &'static str {
    match phase {
        DelegateDisplayPhase::Queued => "queued",
        DelegateDisplayPhase::Running => "running",
        DelegateDisplayPhase::Backgrounded => "backgrounded",
        DelegateDisplayPhase::Completed => "done",
        DelegateDisplayPhase::Failed => "failed",
        DelegateDisplayPhase::Cancelled => "cancelled",
        DelegateDisplayPhase::TimedOut => "timed out",
        DelegateDisplayPhase::Lost => "lost",
        DelegateDisplayPhase::Killed => "killed",
    }
}

fn role_badge(snapshot: &AgentSnapshot) -> String {
    format!("[{}]", role_label(snapshot.role))
}
```

Replace the header construction in `render_with_theme` with this structure:

```rust
lines.push(
    Line::from_spans(vec![
        Span::styled(status_marker(phase), accent),
        Span::raw(" "),
        Span::styled(self.snapshot.display_name.as_str(), accent),
        Span::raw("  "),
        Span::styled(role_badge(&self.snapshot), muted),
        Span::styled(
            format!(
                "  {} · {} · {} tools · {} · {} tok",
                self.snapshot.task_title,
                status_text(phase),
                self.snapshot.tool_count,
                format_elapsed(elapsed.as_secs()),
                format_token_count(self.snapshot.token_count),
            ),
            primary,
        ),
    ])
    .truncate_to_width(width),
);
```

Keep `status_color`, `status_marker`, and `display_phase`, but remove role-first wording such as `"{} Agent {}"`.

- [ ] **Step 4: Render body text between thinking and final**

In `render_with_theme`, after thinking and before final, add:

```rust
if let Some(body_text) = activity.body_text.as_deref()
    && let Some(line) = crate::transcript::render_child_body(body_text, width, "  ", theme)
{
    lines.push(line);
}
```

Keep final rendering as the last child row:

```rust
if let Some(final_text) = activity.final_text.as_deref() {
    lines.push(render_child_final(
        final_text,
        activity.final_is_error,
        width,
        "  ",
        theme,
    ));
}
```

- [ ] **Step 5: Run single delegate tests and verify they pass**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_single_delegate
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_completed_delegate
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_backgrounded_delegate
```

Expected: PASS.

- [ ] **Step 6: Update stale old-header tests**

In `crates/neo-tui/tests/multi_agent_transcript.rs`, replace assertions like:

```rust
assert!(text.contains("Gibbs Coder Agent Running"), "{text}");
```

with:

```rust
assert!(text.contains("Gibbs  [Coder]"), "{text}");
assert!(text.contains("running"), "{text}");
```

For Explorer:

```rust
assert!(text.contains("Gibbs  [Explorer]"), "{text}");
```

- [ ] **Step 7: Run the focused old-header compatibility filters**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript delegate_card_renders_kimi_style_running_summary
cargo nextest run -p neo-tui --test multi_agent_transcript delegate_card_header_uses_role_display_label
cargo nextest run -p neo-tui --test multi_agent_transcript transcript_pane_upserts_delegate_card_from_events
```

Expected: PASS with Option B wording.

- [ ] **Step 8: Commit checkpoint only if authorized**

Ask for explicit authorization. If granted:

```bash
git add crates/neo-tui/src/transcript/delegate_card.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "feat: render delegate cards as option b transcripts"
```

Expected: one focused delegate card commit.

## Task 4: Render Delegate Groups As Child-Agent Transcript Cards

**Files:**
- Modify: `crates/neo-tui/src/transcript/delegate_group.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`
- Test: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing delegate group Option B test**

Append:

```rust
#[test]
fn option_b_delegate_group_keeps_agent_names_primary() {
    let mut pane = TranscriptPane::new(160, 30);
    let nova = option_b_running_delegate();
    let mut vega = option_b_delegate(
        "vega",
        "Vega",
        AgentRole::Explorer,
        AgentLifecycleState::Queued,
        "搜索历史卡片回归点",
    );
    vega.path = AgentPath::root_child(&vega.display_name);

    pane.apply_agent_event(AgentEvent::DelegateStarted { turn: 7, agent: nova });
    pane.apply_agent_event(AgentEvent::DelegateStarted { turn: 7, agent: vega });
    let _ = pane.render_frame(160, 30);

    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Running 2 agents"), "{text}");
    assert!(text.contains("├─ Nova  [Coder]"), "{text}");
    assert!(text.contains("└─ Vega  [Explorer]"), "{text}");
    assert!(text.contains("• Used Read"), "{text}");
    assert!(text.contains("◌ thinking"), "{text}");
    assert!(text.contains("Waiting for scheduler slot"), "{text}");
    assert!(!text.contains("Coder · 角色对比测试"), "{text}");
}
```

- [ ] **Step 2: Run the delegate group test and verify it fails**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_delegate_group_keeps_agent_names_primary
```

Expected: FAIL because existing group rows use role-first formatting or omit full activity.

- [ ] **Step 3: Update `DelegateGroupComponent::render_agent`**

In `crates/neo-tui/src/transcript/delegate_group.rs`, import the shared body renderer:

```rust
use crate::transcript::{
    can_detach, child_activity_view, display_elapsed, format_elapsed, format_token_count,
    render_child_body, render_child_final, render_child_thinking, render_child_tool_row, role_label,
};
```

Replace the first row spans in `render_agent` with:

```rust
Line::from_spans(vec![
    Span::raw(format!("  {branch} ")),
    Span::styled(agent.display_name.as_str(), state_style),
    Span::raw("  "),
    Span::styled(format!("[{}]", role_label(agent.role)), muted),
    Span::styled(
        format!("  {}{}", agent.display_title(), format_stats(agent, self.now_ms)),
        primary,
    ),
])
.truncate_to_width(width)
```

Then render shared activity under that row:

```rust
let view = child_activity_view(agent, MAX_GROUP_TOOL_ROWS);
for tool in &view.tools {
    lines.extend(render_child_tool_row(tool, width, &format!("  {continuation} "), theme));
}
if let Some(thinking) = view.thinking.as_deref() {
    lines.extend(render_child_thinking(thinking, width, &format!("  {continuation} "), theme));
}
if let Some(body_text) = view.body_text.as_deref()
    && let Some(line) = render_child_body(body_text, width, &format!("  {continuation} "), theme)
{
    lines.push(line);
}
if let Some(final_text) = view.final_text.as_deref() {
    lines.push(render_child_final(
        final_text,
        view.final_is_error,
        width,
        &format!("  {continuation} "),
        theme,
    ));
}
if view.tools.is_empty()
    && view.thinking.is_none()
    && view.body_text.is_none()
    && view.final_text.is_none()
{
    lines.push(
        Line::styled(
            format!("  {continuation} ◌ {}", fallback_activity(agent)),
            Style::default().fg(theme.text_muted),
        )
        .truncate_to_width(width),
    );
}
```

Use `const MAX_GROUP_TOOL_ROWS: usize = 2;` near the top of the file.

- [ ] **Step 4: Run the delegate group test and verify it passes**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_delegate_group_keeps_agent_names_primary
```

Expected: PASS.

- [ ] **Step 5: Run grouping/upsert regression**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript transcript_pane_upserts_delegate_card_from_events
```

Expected: PASS. This ensures single-card event insertion still works after group renderer changes.

- [ ] **Step 6: Commit checkpoint only if authorized**

Ask for explicit authorization. If granted:

```bash
git add crates/neo-tui/src/transcript/delegate_group.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "feat: render delegate groups as option b transcripts"
```

Expected: one focused group card commit.

## Task 5: Render Collapsed DelegateSwarm With Option B Identity And Progress

**Files:**
- Modify: `crates/neo-tui/src/transcript/swarm_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`
- Test: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing collapsed swarm Option B test**

Append:

```rust
#[test]
fn option_b_collapsed_swarm_shows_names_badges_and_bayes_progress() {
    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "coder item".to_owned(),
            agent: option_b_running_delegate(),
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "planner item".to_owned(),
            agent: AgentSnapshot {
                id: AgentId::from_suffix_for_test("iris"),
                display_name: AgentDisplayName::new("Iris"),
                role: AgentRole::Planner,
                state: AgentLifecycleState::Completed,
                tool_count: 3,
                token_count: 8_200,
                elapsed: Duration::from_secs(12),
                terminal_at_ms: Some(12_000),
                terminal_reason: Some(AgentTerminalReason::Completed),
                outcome: Some(AgentTerminalOutcome {
                    summary: "Plan is ready".to_owned(),
                    is_error: false,
                }),
                ..option_b_running_delegate()
            },
        },
        SwarmChildSnapshot {
            item_index: 2,
            item: "explorer item".to_owned(),
            agent: option_b_delegate(
                "vega",
                "Vega",
                AgentRole::Explorer,
                AgentLifecycleState::Running,
                "搜索历史卡片回归点",
            ),
        },
        SwarmChildSnapshot {
            item_index: 3,
            item: "queued item".to_owned(),
            agent: option_b_delegate(
                "rune",
                "Rune",
                AgentRole::Coder,
                AgentLifecycleState::Queued,
                "queued renderer task",
            ),
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "option-b-swarm".to_owned(),
        description: "角色对比测试".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 2,
        aggregate,
        children,
    };

    let text = plain(SwarmCardComponent::new(snapshot).render_with_theme(160, &TuiTheme::default()))
        .join("\n");

    assert!(text.contains("Swarm: 角色对比测试"), "{text}");
    assert!(text.contains("progress ["), "{text}");
    assert!(text.contains("bayes estimate"), "{text}");
    assert!(text.contains("Nova  [Coder]"), "{text}");
    assert!(text.contains("Iris  [Planner]"), "{text}");
    assert!(text.contains("Vega  [Explorer]"), "{text}");
    assert!(text.contains("Rune  [Coder]"), "{text}");
    assert!(text.contains("Using Bash"), "{text}");
    assert!(text.contains("queued"), "{text}");
    assert!(!text.contains("001 "), "index numbers are not the primary visual language: {text}");
}
```

- [ ] **Step 2: Run the collapsed swarm test and verify it fails**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_collapsed_swarm_shows_names_badges_and_bayes_progress
```

Expected: FAIL because current collapsed swarm uses `Agent Swarm`/numbered rows and role is not a badge.

- [ ] **Step 3: Implement collapsed swarm header**

In `crates/neo-tui/src/transcript/swarm_card.rs`, replace the first two header lines in `render_with_theme` with:

```rust
lines.push(
    Line::from_spans(vec![
        Span::styled("● ", swarm_status_style(&self.snapshot, theme)),
        Span::styled("Swarm: ", brand),
        Span::styled(self.snapshot.description.as_str(), primary),
        Span::styled(
            format!(
                " · {} agents · {} run · {} done · {} wait",
                self.snapshot.aggregate.total,
                self.snapshot.aggregate.running,
                self.snapshot.aggregate.completed,
                self.snapshot.aggregate.queued,
            ),
            muted,
        ),
    ])
    .truncate_to_width(width),
);
lines.push(
    Line::from_spans(vec![
        Span::raw("  progress ["),
        progress_meter(progress, theme),
        Span::raw("] "),
        Span::styled(format!("{:.0}% · bayes estimate · max {}", progress * 100.0, self.snapshot.max_concurrency), muted),
    ])
    .truncate_to_width(width),
);
```

If `progress_meter` currently returns a long decorative span, add a compact helper:

```rust
fn compact_progress_meter(progress: f32, width: usize) -> String {
    let width = width.max(1);
    let filled = ((progress.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    format!("{}{}", "■".repeat(filled), "·".repeat(width - filled))
}
```

Then use `compact_progress_meter(progress, 10)` in the header line if `progress_meter` cannot be embedded inside brackets.

- [ ] **Step 4: Implement collapsed child rows**

Replace the numbered child row with:

```rust
let branch = if index + 1 == self.snapshot.children.len() { "└─" } else { "├─" };
lines.push(
    Line::from_spans(vec![
        Span::raw(format!("  {branch} ")),
        Span::styled(child.agent.display_name.as_str(), state_style),
        Span::raw("  "),
        Span::styled(format!("[{}]", role_label(child.agent.role)), muted),
        Span::raw(" "),
        Span::styled(marker(child.agent.state), state_style),
        Span::raw(" ["),
        progress_bar_line(progress, child.agent.state, theme),
        Span::raw("] "),
        Span::styled(
            format!(
                "{:.0}% · {}",
                progress * 100.0,
                child_activity_summary(&child.agent, &child.item),
            ),
            primary,
        ),
    ])
    .truncate_to_width(width),
);
```

Remove primary reliance on `001`, `002`, `003` numbering in the visible row. The child order remains `item_index` order.

- [ ] **Step 5: Run the collapsed swarm test and verify it passes**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_collapsed_swarm_shows_names_badges_and_bayes_progress
```

Expected: PASS.

- [ ] **Step 6: Run existing swarm summary regressions**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript swarm_card_renders_orchestrating_before_children_run
cargo nextest run -p neo-tui --test multi_agent_transcript swarm_card_renders_working_after_child_runs
cargo nextest run -p neo-tui --test multi_agent_transcript swarm_card_prefers_child_activity_over_original_item_text
```

Expected: PASS after old wording assertions are migrated to Option B wording. Keep semantic expectations: orchestrating/working status, progress percent, and activity-over-prompt behavior.

- [ ] **Step 7: Commit checkpoint only if authorized**

Ask for explicit authorization. If granted:

```bash
git add crates/neo-tui/src/transcript/swarm_card.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "feat: render collapsed swarms as option b transcripts"
```

Expected: one focused collapsed swarm commit.

## Task 6: Render Expanded DelegateSwarm With Shared Child Activity

**Files:**
- Modify: `crates/neo-tui/src/transcript/swarm_card.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`
- Test: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add failing expanded swarm Option B test**

Append:

```rust
#[test]
fn option_b_expanded_swarm_preserves_full_child_transcripts() {
    let mut nova = option_b_running_delegate();
    nova.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: "All edits applied. Now let me verify the paths.".to_owned(),
            thinking: false,
        },
    });
    let mut iris = option_b_delegate(
        "iris-expanded",
        "Iris",
        AgentRole::Planner,
        AgentLifecycleState::Completed,
        "Plan renderer work",
    );
    iris.tool_count = 2;
    iris.token_count = 8_200;
    iris.elapsed = Duration::from_secs(12);
    iris.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "read-plan".to_owned(),
            name: "Read".to_owned(),
            summary: Some("docs/superpowers/plans/...".to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
        },
    }];
    iris.outcome = Some(AgentTerminalOutcome {
        summary: "The implementation should stay inside transcript cards.".to_owned(),
        is_error: false,
    });

    let children = vec![
        SwarmChildSnapshot { item_index: 0, item: "nova".to_owned(), agent: nova },
        SwarmChildSnapshot { item_index: 1, item: "iris".to_owned(), agent: iris },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "option-b-expanded".to_owned(),
        description: "角色对比测试".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 2,
        aggregate,
        children,
    };
    let mut card = SwarmCardComponent::new(snapshot);
    card.set_expanded(true);

    let rows = plain(card.render_with_theme(160, &TuiTheme::default()));
    let text = rows.join("\n");

    assert!(text.contains("├─ Nova  [Coder]"), "{text}");
    assert!(text.contains("└─ Iris  [Planner]"), "{text}");
    assert!(text.contains("• Used Read"), "{text}");
    assert!(text.contains("• Using Bash"), "{text}");
    assert!(text.contains("◌ thinking"), "{text}");
    assert!(text.contains("│ All edits applied"), "{text}");
    assert!(text.contains("└ The implementation should stay inside transcript cards."), "{text}");
}
```

- [ ] **Step 2: Run the expanded swarm test and verify it fails**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_expanded_swarm_preserves_full_child_transcripts
```

Expected: FAIL until expanded rows use name badges and shared body rows.

- [ ] **Step 3: Update expanded child rendering**

In `SwarmCardComponent::render_with_theme`, inside `if self.expanded`, replace the child header spans with the same name-first badge grammar:

```rust
let branch = if index + 1 == self.snapshot.children.len() { "└─" } else { "├─" };
let continuation = if index + 1 == self.snapshot.children.len() { "   " } else { "│  " };
lines.push(
    Line::from_spans(vec![
        Span::raw(format!("  {branch} ")),
        Span::styled(child.agent.display_name.as_str(), state_style),
        Span::raw("  "),
        Span::styled(format!("[{}]", role_label(child.agent.role)), muted),
        Span::styled(
            format!(
                "  {} · {} · {} tools · {} tok",
                state_label(child.agent.state),
                format_elapsed(elapsed.as_secs()),
                child.agent.tool_count,
                format_token_count(child.agent.token_count),
            ),
            primary,
        ),
    ])
    .truncate_to_width(width),
);
```

Then render activity using:

```rust
let indent = format!("  {continuation} ");
let view = child_activity_view(&child.agent, MAX_CHILD_TOOL_ROWS);
for tool in &view.tools {
    lines.extend(render_child_tool_row(tool, width, &indent, theme));
}
if let Some(thinking) = view.thinking.as_deref() {
    lines.extend(render_child_thinking(thinking, width, &indent, theme));
}
if let Some(body_text) = view.body_text.as_deref()
    && let Some(line) = render_child_body(body_text, width, &indent, theme)
{
    lines.push(line);
}
if let Some(final_text) = view.final_text.as_deref() {
    lines.push(render_child_final(final_text, view.final_is_error, width, &indent, theme));
}
```

- [ ] **Step 4: Run expanded swarm tests and verify they pass**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_expanded_swarm_preserves_full_child_transcripts
cargo nextest run -p neo-tui --test multi_agent_transcript swarm_card_uses_theme_styles_and_expanded_child_details
```

Expected: PASS after updating old assertions to name-badge wording.

- [ ] **Step 5: Commit checkpoint only if authorized**

Ask for explicit authorization. If granted:

```bash
git add crates/neo-tui/src/transcript/swarm_card.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "feat: render expanded swarms as option b transcripts"
```

Expected: one focused expanded swarm commit.

## Task 7: Width And Theme Regression Coverage

**Files:**
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`
- Modify: `crates/neo-tui/src/transcript/delegate_card.rs`
- Modify: `crates/neo-tui/src/transcript/delegate_group.rs`
- Modify: `crates/neo-tui/src/transcript/swarm_card.rs`

- [ ] **Step 1: Add failing narrow-width identity preservation test**

Append:

```rust
#[test]
fn option_b_narrow_width_preserves_name_and_role_badge() {
    let text = plain(
        DelegateCardComponent::new(option_b_running_delegate())
            .render_with_theme(48, &TuiTheme::default()),
    )
    .join("\n");

    assert!(text.contains("Nova"), "{text}");
    assert!(text.contains("[Coder]"), "{text}");
    assert!(
        !text.contains("Full prompt that must not replace"),
        "narrow header must drop prompt/title before identity: {text}"
    );
}
```

- [ ] **Step 2: Add failing theme marker test**

Append:

```rust
#[test]
fn option_b_state_markers_do_not_depend_on_color_only() {
    let completed = AgentSnapshot {
        state: AgentLifecycleState::Completed,
        terminal_reason: Some(AgentTerminalReason::Completed),
        outcome: Some(AgentTerminalOutcome {
            summary: "Done".to_owned(),
            is_error: false,
        }),
        ..option_b_running_delegate()
    };
    let failed = AgentSnapshot {
        state: AgentLifecycleState::Failed,
        terminal_reason: Some(AgentTerminalReason::Error),
        outcome: Some(AgentTerminalOutcome {
            summary: "Failed".to_owned(),
            is_error: true,
        }),
        ..option_b_running_delegate()
    };

    let completed_text = plain(
        DelegateCardComponent::new(completed).render_with_theme(120, &TuiTheme::default()),
    )
    .join("\n");
    let failed_text = plain(
        DelegateCardComponent::new(failed).render_with_theme(120, &TuiTheme::default()),
    )
    .join("\n");

    assert!(completed_text.contains("✓ Nova  [Coder]"), "{completed_text}");
    assert!(completed_text.contains("done"), "{completed_text}");
    assert!(failed_text.contains("✗ Nova  [Coder]"), "{failed_text}");
    assert!(failed_text.contains("failed"), "{failed_text}");
}
```

- [ ] **Step 3: Run the width/theme tests and verify they fail if implementation is incomplete**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_narrow_width_preserves_name_and_role_badge
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_state_markers_do_not_depend_on_color_only
```

Expected: PASS if Tasks 3-6 already handled width and markers. If either fails, continue to Step 4.

- [ ] **Step 4: Make identity prefix compact before truncation**

In each card header, build the identity spans before appending task/stats. Use this pattern in delegate, group, and swarm child rows:

```rust
Span::styled(snapshot.display_name.as_str(), state_style),
Span::raw("  "),
Span::styled(format!("[{}]", role_label(snapshot.role)), muted),
Span::styled(format!("  {}", status_text(phase)), primary),
```

Avoid composing `task_title` before `display_name`.

- [ ] **Step 5: Run the width/theme tests and verify they pass**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_narrow_width_preserves_name_and_role_badge
cargo nextest run -p neo-tui --test multi_agent_transcript option_b_state_markers_do_not_depend_on_color_only
```

Expected: PASS.

- [ ] **Step 6: Commit checkpoint only if authorized**

Ask for explicit authorization. If granted:

```bash
git add crates/neo-tui/tests/multi_agent_transcript.rs crates/neo-tui/src/transcript/delegate_card.rs crates/neo-tui/src/transcript/delegate_group.rs crates/neo-tui/src/transcript/swarm_card.rs
git commit -m "test: lock option b width and state markers"
```

Expected: one focused visual-regression commit.

## Task 8: Runtime Contract Regression Sweep

**Files:**
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/state.rs`

- [ ] **Step 1: Add or update runtime test for ongoing tool preservation**

In `crates/neo-agent-core/tests/multi_agent_runtime.rs`, add this test if no equivalent exists:

```rust
#[test]
fn child_activity_trim_preserves_visible_ongoing_tool_and_latest_text() {
    let runtime = MultiAgentRuntime::new();
    let snapshot = runtime.start_foreground_delegate_for_test("long running bash");
    let started_at = std::time::Instant::now();

    let _ = runtime.apply_child_event(
        &snapshot.id,
        started_at,
        &AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "bash-live".to_owned(),
            name: "Bash".to_owned(),
            arguments: json!({"cmd": "cargo nextest run -p neo-tui --test multi_agent_transcript"}),
        },
    );
    for index in 0..32 {
        let _ = runtime.apply_child_event(
            &snapshot.id,
            started_at,
            &AgentEvent::ThinkingDelta {
                turn: 1,
                text: format!("thinking chunk {index}"),
            },
        );
    }

    let updated = runtime
        .snapshot(&snapshot.id)
        .expect("snapshot remains present");
    let text = serde_json::to_string(&updated.activity).expect("serialize activity");
    assert!(text.contains("bash-live"), "{text}");
    assert!(text.contains("thinking chunk 31"), "{text}");
}
```

- [ ] **Step 2: Run the runtime trim test and verify behavior**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime child_activity_trim_preserves_visible_ongoing_tool_and_latest_text
```

Expected: PASS if current trim policy already preserves the ongoing tool. If it fails, continue to Step 3.

- [ ] **Step 3: Update trim policy only if the test fails**

In `crates/neo-agent-core/src/multi_agent/runtime.rs`, update `trim_activity` to preserve:

```rust
fn trim_activity(activity: &mut Vec<AgentActivityEntry>) {
    const MAX_ACTIVITY: usize = 24;
    if activity.len() <= MAX_ACTIVITY {
        return;
    }

    let newest_ongoing_tool = activity.iter().rposition(|entry| {
        matches!(
            entry.kind,
            AgentActivityKind::Tool {
                phase: AgentToolActivityPhase::Ongoing,
                ..
            }
        )
    });
    let newest_thinking = activity.iter().rposition(|entry| {
        matches!(entry.kind, AgentActivityKind::Text { thinking: true, .. })
    });
    let newest_body = activity.iter().rposition(|entry| {
        matches!(entry.kind, AgentActivityKind::Text { thinking: false, .. })
    });

    let keep_start = activity.len().saturating_sub(MAX_ACTIVITY);
    let mut kept = Vec::with_capacity(MAX_ACTIVITY);
    for (index, entry) in activity.drain(..).enumerate() {
        let preserve = Some(index) == newest_ongoing_tool
            || Some(index) == newest_thinking
            || Some(index) == newest_body
            || index >= keep_start;
        if preserve {
            kept.push(entry);
        }
    }
    if kept.len() > MAX_ACTIVITY {
        let overflow = kept.len() - MAX_ACTIVITY;
        kept.drain(0..overflow);
    }
    *activity = kept;
}
```

- [ ] **Step 4: Run runtime contract tests**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime agent_tool_activity_uses_explicit_phase_and_output_preview
cargo nextest run -p neo-agent-core --test multi_agent_runtime agent_snapshot_records_timestamps_detach_origin_and_terminal_reason
cargo nextest run -p neo-agent-core --test multi_agent_runtime background_terminal_reason_records_lost_without_claiming_completion
cargo nextest run -p neo-agent-core --test multi_agent_runtime child_activity_trim_preserves_visible_ongoing_tool_and_latest_text
```

Expected: PASS.

- [ ] **Step 5: Commit checkpoint only if authorized**

Ask for explicit authorization. If granted:

```bash
git add crates/neo-agent-core/tests/multi_agent_runtime.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/src/multi_agent/state.rs
git commit -m "test: protect multi-agent activity state contract"
```

Expected: one focused runtime contract commit.

## Task 9: Copy Output And Final Verification

**Files:**
- Modify: `crates/neo-tui/src/transcript/entry/copy.rs`
- Test: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Inspect copy output for old wording**

Run:

```bash
rg -n "Agent Running|Agent Completed|Coder ·|Explorer ·|Planner ·|Agent Swarm|001 \\[" crates/neo-tui/src/transcript crates/neo-tui/tests/multi_agent_transcript.rs
```

Expected: No production renderer still emits old role-first or numbered-primary wording. Test names may mention old behavior only if assertions have been migrated.

- [ ] **Step 2: Update copy rendering if it uses old labels**

If `copy.rs` formats delegate copy output with role-first wording, replace it with:

```rust
format!(
    "{} [{}] {}",
    snapshot.display_name.as_str(),
    role_label(snapshot.role),
    snapshot.display_title(),
)
```

For swarm copy output, ensure each child uses:

```rust
format!(
    "{} [{}] {}",
    child.agent.display_name.as_str(),
    role_label(child.agent.role),
    child.item,
)
```

- [ ] **Step 3: Run all focused TUI tests for this feature**

Run:

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript option_b
cargo nextest run -p neo-tui --test multi_agent_transcript delegate_card
cargo nextest run -p neo-tui --test multi_agent_transcript swarm_card
```

Expected: PASS. If a pre-existing test has old wording, update its assertion to the Option B contract without weakening semantic coverage.

- [ ] **Step 4: Run focused clippy for touched tests**

Run:

```bash
cargo clippy -p neo-tui --test multi_agent_transcript -- -D warnings -A clippy::pedantic
```

Expected: PASS.

- [ ] **Step 5: Run focused runtime verification**

Run:

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime agent_tool_activity_uses_explicit_phase_and_output_preview
cargo nextest run -p neo-agent-core --test multi_agent_runtime child_activity_trim_preserves_visible_ongoing_tool_and_latest_text
cargo clippy -p neo-agent-core --test multi_agent_runtime -- -D warnings -A clippy::pedantic
```

Expected: PASS.

- [ ] **Step 6: Commit checkpoint only if authorized**

Ask for explicit authorization. If granted:

```bash
git add crates/neo-tui/src/transcript/entry/copy.rs crates/neo-tui/src/transcript crates/neo-tui/tests/multi_agent_transcript.rs crates/neo-agent-core/src/multi_agent crates/neo-agent-core/tests/multi_agent_runtime.rs
git commit -m "feat: complete option b multi-agent transcript cards"
```

Expected: one final cleanup commit if previous checkpoints were not committed, or no commit if all changes were already committed in earlier authorized checkpoints.

## Self-Review

- Spec coverage: Tasks 1, 3, 4, 5, 6, and 7 implement Option B visual identity, single delegate, delegate group, collapsed swarm, expanded swarm, status/theme, and width rules. Task 2 implements shared child activity grammar. Task 8 protects runtime state semantics. Task 9 covers copy output and focused verification.
- Placeholder scan: The plan contains concrete file paths, test names, code snippets, commands, and expected outcomes. It contains no `TBD`, no `TODO`, and no unspecified edge-case instruction.
- Type consistency: The plan consistently uses existing types `AgentSnapshot`, `AgentActivityEntry`, `AgentActivityKind`, `AgentToolActivityPhase`, `AgentToolOutputPreview`, `SwarmSnapshot`, `SwarmChildSnapshot`, `DelegateCardComponent`, `DelegateGroupComponent`, `SwarmCardComponent`, and `TranscriptPane`.
- Git policy: Every commit step is an explicit authorization gate, matching Neo's stricter worktree safety boundary.
