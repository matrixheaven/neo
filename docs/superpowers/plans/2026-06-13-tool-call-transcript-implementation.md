# Tool-Call Transcript UI Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace duplicated orange-yellow "Use XXXX running" lines in Neo's TUI with a single in-place updating kimi-code-style tool-call card.

**Architecture:** Extend `ToolRunTranscript` with status-aware rendering metadata, replace the old `tool_render_rows` header/body builder with a stateful card renderer, and map runtime events to update the card in place. Use existing ratatui frame redraws to overwrite previous content naturally.

**Tech Stack:** Rust 2024, ratatui, crossterm, tokio. Crates: `neo-tui`, `neo-agent`.

---

## File Structure

| File | Responsibility |
|------|---------------|
| `crates/tui/src/app.rs` | Extend `ToolRunTranscript` / `ActiveTool` with `live_output` and helper constructors. |
| `crates/tui/src/components.rs` | New `tool_render_rows` header/body renderer, new helper functions for symbols/verbs/chips/key-args. |
| `crates/neo-agent/src/modes/interactive.rs` | Route `ToolExecutionUpdate` events into `live_output`; flush on `ToolExecutionFinished`. |
| `crates/tui/tests/primitives.rs` | Update existing transcript assertions; add new tests for header/chip/live-output behavior. |

---

## Task 1: Extend ToolRunTranscript with live_output

**Files:**
- Modify: `crates/tui/src/app.rs:2236-2259`
- Test: `crates/tui/tests/primitives.rs`

- [ ] **Step 1: Add `live_output` field to `ToolRunTranscript`**

In `crates/tui/src/app.rs`, change:

```rust
pub struct ToolRunTranscript {
    pub name: String,
    pub arguments: Option<String>,
    pub result: Option<String>,
    pub status: ToolStatusKind,
    pub metadata: ToolRunMetadata,
    pub presentation: ToolPresentationKind,
}
```

to:

```rust
pub struct ToolRunTranscript {
    pub name: String,
    pub arguments: Option<String>,
    pub result: Option<String>,
    pub status: ToolStatusKind,
    pub metadata: ToolRunMetadata,
    pub presentation: ToolPresentationKind,
    pub live_output: Vec<String>,
}
```

- [ ] **Step 2: Update `display_detail` to prefer live output while running**

Replace `display_detail` with:

```rust
impl ToolRunTranscript {
    #[must_use]
    pub fn display_detail(&self) -> String {
        if !self.live_output.is_empty() {
            return self.live_output.join("\n");
        }
        self.result
            .as_ref()
            .filter(|result| !result.is_empty())
            .or_else(|| {
                self.arguments
                    .as_ref()
                    .filter(|arguments| !arguments.is_empty())
            })
            .cloned()
            .unwrap_or_default()
    }
}
```

- [ ] **Step 3: Initialize `live_output` in constructors**

In `TranscriptItem::tool` (around line 2857), add `live_output: Vec::new()`.
In `TranscriptItem::tool_run` (around line 2887-2894), add `live_output: Vec::new()`.

- [ ] **Step 4: Run TUI crate tests**

```bash
cargo test -p neo-tui
```

Expected: compiles, existing tests may fail on header text (expected at this stage).

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/app.rs
git commit -m "feat(tui): add live_output field to ToolRunTranscript"
```

---

## Task 2: Add helper functions for header text, symbols, and chips

**Files:**
- Modify: `crates/tui/src/components.rs`
- Test: `crates/tui/tests/primitives.rs`

- [ ] **Step 1: Replace `tool_status_symbol` to return static symbols**

In `crates/tui/src/components.rs`, replace `tool_status_symbol` with:

```rust
fn tool_status_symbol(status: ToolStatusKind) -> &'static str {
    match status {
        ToolStatusKind::Pending | ToolStatusKind::Running => "●",
        ToolStatusKind::Succeeded => "✓",
        ToolStatusKind::Failed => "✗",
        ToolStatusKind::Cancelled => "⊘",
    }
}
```

Remove the `activity_frame` parameter from `tool_status_symbol` and from all call sites.

- [ ] **Step 2: Replace `tool_status_suffix` with `tool_status_verb`**

Remove `tool_status_suffix`. Add:

```rust
fn tool_status_verb(status: ToolStatusKind) -> &'static str {
    match status {
        ToolStatusKind::Pending | ToolStatusKind::Running => "Using",
        ToolStatusKind::Succeeded => "Used",
        ToolStatusKind::Failed => "Failed",
        ToolStatusKind::Cancelled => "Cancelled",
    }
}
```

- [ ] **Step 3: Add `tool_key_argument` extractor**

Add after `tool_call_label`:

```rust
fn tool_key_argument(tool: &ToolRunTranscript) -> String {
    let arguments = tool.arguments.as_deref().unwrap_or_default().trim();
    if arguments.is_empty() {
        return String::new();
    }

    // Try to extract a single meaningful value from JSON args.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) {
        let candidates = ["path", "command", "pattern", "glob", "query"];
        for key in candidates {
            if let Some(v) = value.get(key).and_then(|v| v.as_str()) {
                return one_line(v);
            }
        }
    }

    one_line(arguments)
}
```

- [ ] **Step 4: Add `tool_result_chip` generator**

Add:

```rust
fn tool_result_chip(tool: &ToolRunTranscript) -> String {
    let result = match tool.result.as_deref() {
        Some(r) if !r.is_empty() => r,
        _ => return String::new(),
    };

    let lower = tool.name.to_lowercase();
    if lower == "read" || lower == "write" || lower == "edit" {
        let lines = result.lines().count();
        return format!(" · {lines} lines");
    }
    if lower == "grep" {
        let matches = result.lines().count();
        return format!(" · {matches} matches");
    }
    if lower == "find" || lower == "glob" || lower == "list" {
        let files = result.lines().count();
        return format!(" · {files} files");
    }
    if lower == "bash" || lower == "shell" {
        if let Some(code) = tool.metadata.exit_code {
            if code != 0 {
                return format!(" · exit {code}");
            }
        }
        let bytes = result.len();
        return format!(" · {bytes} bytes");
    }

    let bytes = result.len();
    format!(" · {bytes} bytes")
}
```

- [ ] **Step 5: Update `status_style` to map Running to accent blue**

In `status_style`, change the `Running` arm from `theme.running` to `theme.accent`:

```rust
fn status_style(kind: ToolStatusKind, theme: TuiTheme) -> Style {
    match kind {
        ToolStatusKind::Pending => Style::default().fg(theme.pending),
        ToolStatusKind::Running => Style::default().fg(theme.accent),
        ToolStatusKind::Succeeded => Style::default().fg(theme.succeeded),
        ToolStatusKind::Failed => Style::default().fg(theme.failed),
        ToolStatusKind::Cancelled => Style::default().fg(theme.cancelled),
    }
}
```

- [ ] **Step 6: Run cargo check**

```bash
cargo check -p neo-tui
```

Expected: compiles (helpers unused is OK at this stage).

- [ ] **Step 7: Commit**

```bash
git add crates/tui/src/components.rs
git commit -m "feat(tui): add tool card helpers for symbols, verbs, chips, and key args"
```

---

## Task 3: Rewrite tool_render_rows to build the new card

**Files:**
- Modify: `crates/tui/src/components.rs:598-650`
- Test: `crates/tui/tests/primitives.rs`

- [ ] **Step 1: Update `tool_render_rows` signature and header**

Change the function signature to drop `activity_frame`:

```rust
fn tool_render_rows(
    tool: &ToolRunTranscript,
    expanded: bool,
    selected: bool,
    theme: TuiTheme,
    text_width: usize,
) -> Vec<TranscriptRenderRow> {
```

Update the body to build the new header:

```rust
fn tool_render_rows(
    tool: &ToolRunTranscript,
    expanded: bool,
    selected: bool,
    theme: TuiTheme,
    text_width: usize,
) -> Vec<TranscriptRenderRow> {
    if let Some(diff) = tool.result.as_deref().and_then(DiffModel::parse_unified) {
        return diff_tool_render_rows(tool, &diff, expanded, selected, theme, text_width);
    }

    let key_arg = tool_key_argument(tool);
    let chip = tool_result_chip(tool);
    let verb = tool_status_verb(tool.status);
    let symbol = tool_status_symbol(tool.status);

    let header = if key_arg.is_empty() {
        format!("{symbol} {verb} {}{chip}", tool.name)
    } else {
        format!("{symbol} {verb} {} ({key_arg}){chip}", tool.name)
    };

    let header_fg = match tool.status {
        ToolStatusKind::Pending => theme.pending,
        ToolStatusKind::Running => theme.accent,
        ToolStatusKind::Succeeded => theme.succeeded,
        ToolStatusKind::Failed => theme.failed,
        ToolStatusKind::Cancelled => theme.cancelled,
    };
    let header_style = selected_style(
        Style::default()
            .fg(header_fg)
            .add_modifier(Modifier::BOLD),
        selected,
        theme,
    );
    let body_style = selected_style(Style::default().fg(theme.notice), selected, theme);
    let error_style = selected_style(Style::default().fg(theme.failed), selected, theme);

    let mut rows = vec![TranscriptRenderRow::new(header, header_style, None)];

    let detail = tool.display_detail();
    if !detail.is_empty() {
        let detail_lines = detail.lines().collect::<Vec<_>>();
        let visible_count = if expanded {
            detail_lines.len()
        } else {
            detail_lines.len().min(TOOL_PREVIEW_LINES)
        };
        for line in detail_lines.iter().take(visible_count) {
            for wrapped in wrap_width(line, text_width.saturating_sub(4).max(1)) {
                let style = if tool.status == ToolStatusKind::Failed {
                    error_style
                } else {
                    body_style
                };
                rows.push(TranscriptRenderRow::new(
                    format!("  {wrapped}"),
                    style,
                    None,
                ));
            }
        }
        if !expanded && detail_lines.len() > visible_count {
            rows.push(TranscriptRenderRow::new(
                format!(
                    "  ... ({} more lines, ctrl+o to expand)",
                    detail_lines.len() - visible_count
                ),
                body_style,
                None,
            ));
        }
    }

    rows
}
```

- [ ] **Step 2: Find and update the caller of `tool_render_rows`**

Search for the call site:

```bash
grep -n "tool_render_rows" crates/tui/src/components.rs
```

Remove the `activity_frame` argument from the call.

- [ ] **Step 3: Run TUI tests**

```bash
cargo test -p neo-tui
```

Expected: tests around "● Use read(" and "running" fail; fix them in Task 4.

- [ ] **Step 4: Commit**

```bash
git add crates/tui/src/components.rs
git commit -m "feat(tui): render tool calls as in-place updating cards"
```

---

## Task 4: Update tests for the new transcript format

**Files:**
- Modify: `crates/tui/tests/primitives.rs`

- [ ] **Step 1: Update assertions that look for old header text**

Search for old patterns:

```bash
grep -n '"● Use read(' crates/tui/tests/primitives.rs
```

Change lines like:

```rust
assert!(lines.iter().any(|line| line.contains("● Use read(")));
```

to:

```rust
assert!(lines.iter().any(|line| line.contains("Using Read")));
```

- [ ] **Step 2: Rewrite `transcript_widget_animates_running_tool_marker_in_place`**

Replace the existing test around line 1562 with:

```rust
#[test]
fn transcript_widget_renders_running_tool_card_in_place() {
    let transcript = ChatTranscript::from_items([TranscriptItem::tool_run(
        "list",
        Some(r#"{"path":"crates/tui/src"}"#.to_owned()),
        None,
        ToolStatusKind::Running,
        neo_tui::ToolRunMetadata::default(),
        neo_tui::ToolPresentationKind::Text,
    )]);

    let frame = render_widget(
        80,
        4,
        TranscriptWidget::new(&transcript),
    );

    assert!(frame.iter().any(|line| line.contains("● Using list")));
    assert!(frame.iter().any(|line| line.contains("(crates/tui/src)")));
    assert!(!frame.iter().any(|line| line.contains("running")));
}
```

Note: if `tool_run` signature gained a `live_output` parameter in Task 1, pass `None` or `Vec::new()` accordingly.

- [ ] **Step 3: Add chip assertion test**

Add after the running test:

```rust
#[test]
fn transcript_widget_shows_result_chip_when_tool_succeeds() {
    let result = "line1\nline2\nline3\nline4".to_owned();
    let transcript = ChatTranscript::from_items([TranscriptItem::tool_run(
        "Read",
        Some(r#"{"path":"src/lib.rs"}"#.to_owned()),
        Some(result),
        ToolStatusKind::Succeeded,
        neo_tui::ToolRunMetadata::default(),
        neo_tui::ToolPresentationKind::Text,
    )]);

    let frame = render_widget(80, 8, TranscriptWidget::new(&transcript));

    assert!(frame.iter().any(|line| line.contains("✓ Used Read")));
    assert!(frame.iter().any(|line| line.contains("· 4 lines")));
    assert!(frame.iter().any(|line| line.contains("... (1 more lines, ctrl+o to expand)")));
}
```

- [ ] **Step 4: Add failed-tool red output test**

```rust
#[test]
fn transcript_widget_tints_failed_tool_output_red() {
    let result = "error: something went wrong".to_owned();
    let transcript = ChatTranscript::from_items([TranscriptItem::tool_run(
        "Bash",
        Some(r#"{"command":"cargo test"}"#.to_owned()),
        Some(result),
        ToolStatusKind::Failed,
        neo_tui::ToolRunMetadata {
            exit_code: Some(101),
            ..Default::default()
        },
        neo_tui::ToolPresentationKind::Text,
    )]);

    let buffer = render_widget_buffer(80, 4, TranscriptWidget::new(&transcript));
    let has_red = buffer.content.chunks(80).any(|line| {
        line.iter().any(|cell| cell.fg == Color::Rgb(248, 81, 73))
    });
    assert!(has_red);
}
```

- [ ] **Step 5: Run TUI tests**

```bash
cargo test -p neo-tui
```

Expected: all TUI tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/tui/tests/primitives.rs
git commit -m "test(tui): update assertions for new tool-call transcript cards"
```

---

## Task 5: Route live Bash output into the card

**Files:**
- Modify: `crates/tui/src/app.rs`

- [ ] **Step 1: Modify `StreamUpdate::ToolUpdated` to append shell live output**

In `crates/tui/src/app.rs`, change the `ToolUpdated` arm from:

```rust
StreamUpdate::ToolUpdated { id, detail } => {
    if let Some(tool) = self.active_tools.iter_mut().find(|tool| tool.id == id) {
        tool.result = Some(detail);
        self.transcript.update_tool_run(
            tool.transcript_index,
            tool.clone().into_transcript_item(),
        );
    }
}
```

to:

```rust
StreamUpdate::ToolUpdated { id, detail } => {
    if let Some(tool) = self.active_tools.iter_mut().find(|tool| tool.id == id) {
        let is_shell = tool.name.eq_ignore_ascii_case("bash")
            || tool.name.eq_ignore_ascii_case("shell")
            || tool.name.eq_ignore_ascii_case("terminal");
        if is_shell && tool.status == ToolStatusKind::Running {
            if let Some(TranscriptItem::Tool { tool_run, .. }) =
                self.transcript.items.get_mut(tool.transcript_index)
            {
                tool_run.live_output.extend(detail.lines().map(ToOwned::to_owned));
                while tool_run.live_output.len() > 3 {
                    tool_run.live_output.remove(0);
                }
            }
        } else {
            tool.result = Some(detail);
            self.transcript.update_tool_run(
                tool.transcript_index,
                tool.clone().into_transcript_item(),
            );
        }
    }
}
```

- [ ] **Step 2: Clear `live_output` when tool finishes**

In `NeoTuiApp::apply_stream_update` where `StreamUpdate::ToolFinished` is handled, after `self.active_tools.remove(index)`, add:

```rust
if let Some(TranscriptItem::Tool { tool_run, .. }) =
    self.transcript.items.get_mut(tool.transcript_index)
{
    tool_run.live_output.clear();
}
```

- [ ] **Step 3: Run neo-agent and neo-tui tests**

```bash
cargo test -p neo-tui
cargo test -p neo-agent
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/tui/src/app.rs
git commit -m "feat(tui): stream live bash output into running tool cards"
```

---

## Task 6: Clean up unused theme fields and dead code

**Files:**
- Modify: `crates/tui/src/app.rs`
- Modify: `crates/tui/src/components.rs`
- Modify: `crates/neo-agent/src/themes.rs`

- [ ] **Step 1: Remove `tool_status_suffix` if still present**

It was replaced in Task 2; ensure it is deleted.

- [ ] **Step 2: Remove `activity_frame` from `TranscriptWidget` if no longer needed**

Search:

```bash
grep -n "activity_frame" crates/tui/src/components.rs crates/tui/src/app.rs
```

If `TranscriptWidget` only used `activity_frame` for the spinner, remove the field and `with_activity_frame` method. If other components still need it (e.g. thinking spinner), keep it.

- [ ] **Step 3: Optionally deprecate `running` theme field**

For now, keep `TuiTheme.running` to avoid breaking user themes. Add a doc comment noting it is unused by the new tool card header.

- [ ] **Step 4: Run full workspace check**

```bash
cargo run -p xtask -- check
```

Expected: xtask self-checks pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore(tui): clean up spinner/frame code after tool card redesign"
```

---

## Task 7: Final verification

- [ ] **Step 1: Run full TUI and agent test suites**

```bash
cargo test -p neo-tui --all-features
cargo test -p neo-agent --all-features
```

Expected: all pass.

- [ ] **Step 2: Run clippy**

```bash
cargo clippy -p neo-tui -p neo-agent --all-targets --all-features -- -D warnings
```

Expected: no warnings.

- [ ] **Step 3: Manual smoke test**

```bash
cargo build -p neo-agent
# In a separate terminal or temp project:
# cargo run -p neo-agent -- print "list the source files"
```

Verify that running tools show a single `● Using ...` line that flips to `✓ Used ...` with a chip, and no duplicate "running" lines accumulate.

- [ ] **Step 4: Final commit**

```bash
git commit -m "feat(tui): kimi-code-style in-place tool-call transcript cards" --allow-empty
```

---

## Self-Review Checklist

- [ ] **Spec coverage:** Every design decision in `2026-06-13-tool-call-transcript-design.md` has a corresponding task.
- [ ] **Caveat-word scan:** No unresolved marker strings or vague steps remain.
- [ ] **Type consistency:** `ToolRunTranscript.live_output` is `Vec<String>` everywhere; `tool_status_symbol` no longer takes `activity_frame`.
- [ ] **Test coverage:** Header text, chip, live output clearing, and failure tinting are all tested.

## Gap Resolution

If `cargo check` reveals `ToolRunTranscript` is constructed in more places than listed, add `live_output: Vec::new()` at each site. If `AgentEvent::ToolExecutionUpdate` is not yet emitted by `AgentRuntime`, this task becomes a no-op and live output is deferred to a follow-up plan.
