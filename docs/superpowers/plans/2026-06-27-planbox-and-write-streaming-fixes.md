# PlanBox Rendering + Write Streaming UX Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three rendering issues: (1) PlanBox border characters are broken and mis-colored, (2) PlanBox content is plain text instead of rendered markdown, (3) Write streaming preview has a jarring format switch and bloated progress line.

**Architecture:** Task 1 rewrites `PlanBoxComponent::render` to use `Line::from_spans` for correct border coloring and adds right-side corners. Task 2 makes PlanBox call the existing `render_markdown` function for `.md` content. Task 3 unifies Write streaming and final previews by reusing `render_write_preview` during streaming and moving the progress indicator to the header chip.

**Tech Stack:** Rust, neo-tui primitives (`Line`, `Span`, `Style`), `pulldown_cmark` via `render_markdown`, syntect highlighting.

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `crates/neo-tui/src/transcript/plan_box.rs` | PlanBox border + content rendering | 1, 2 |
| `crates/neo-tui/src/transcript/tool_renderers.rs` | Write streaming preview, progress line | 3 |
| `crates/neo-tui/src/transcript/tool_call.rs` | Streaming dispatch, header chip | 3 |
| `crates/neo-tui/src/markdown.rs` | `render_markdown` (read-only, consumed by Task 2) | — |
| `crates/neo-tui/tests/tool_cards.rs` | Tests for PlanBox and Write tool | 1, 2, 3 |

---

## Task 1: Fix PlanBox border rendering

**Problem:** The top border has no right-corner `┐`, the bottom border has no right-corner `┘`, and the left/right vertical bars `│` use `content_color` instead of `border_color`.

**Files:**
- Modify: `crates/neo-tui/src/transcript/plan_box.rs:31-83` (`render` method)
- Test: `crates/neo-tui/src/transcript/plan_box.rs` (unit tests in `#[cfg(test)]` module)

- [ ] **Step 1: Write failing tests for border structure**

Replace the test module in `plan_box.rs` (lines 137-171) with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_basic_box() {
        let comp = PlanBoxComponent::new("# Plan\n- Step 1", Some("/tmp/abc.md".to_string()));
        let lines = comp.render(40, &TuiTheme::default());
        assert!(lines.len() >= 3); // top border + content lines + bottom border
        let top = lines[0].to_ansi();
        assert!(top.contains("plan: abc.md"));
    }

    #[test]
    fn top_border_has_right_corner() {
        let comp = PlanBoxComponent::new("hello", None);
        let lines = comp.render(40, &TuiTheme::default());
        let top = lines[0].to_ansi();
        assert!(top.contains('\u{2510}'), "top border must end with ┐, got: {top}");
    }

    #[test]
    fn bottom_border_has_right_corner() {
        let comp = PlanBoxComponent::new("hello", None);
        let lines = comp.render(40, &TuiTheme::default());
        let bottom = lines.last().unwrap().to_ansi();
        assert!(bottom.contains('\u{2519}'), "bottom border must end with ┘, got: {bottom}");
    }

    #[test]
    fn render_empty_content() {
        let comp = PlanBoxComponent::new("", None);
        let lines = comp.render(20, &TuiTheme::default());
        assert!(lines.len() >= 3);
    }

    #[test]
    fn wrap_text_long_line() {
        let wrapped = PlanBoxComponent::wrap_text("aaaa bbbb cccc dddd", 10);
        assert!(wrapped.len() > 1);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-tui plan_box`
Expected: FAIL — `top_border_has_right_corner` and `bottom_border_has_right_corner` fail.

- [ ] **Step 3: Rewrite the `render` method**

Replace the entire `render` method (lines 33-83) and the `titled_border` function (lines 85-92) with:

```rust
    /// Render the plan box as styled lines, fitting within `width` columns.
    #[must_use]
    pub fn render(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        if width < 10 {
            return vec![];
        }

        let border_style = Style::default().fg(theme.status_ok);
        let content_style = Style::default().fg(theme.text_primary);
        let muted_style = Style::default().fg(theme.text_muted);

        let inner_width = width.saturating_sub(4).max(1); // │ + space + content + space + │

        // Title
        let basename = self
            .path
            .as_ref()
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or("plan");
        let title = if let Some(status) = &self.status {
            format!(" plan: {basename} · {status} ")
        } else {
            format!(" plan: {basename} ")
        };

        let mut lines = vec![Self::titled_border(&title, width, border_style)];

        // Content lines
        for raw_line in self.content.lines() {
            for chunk in Self::wrap_text(raw_line, inner_width) {
                let padded = Self::pad_to(&chunk, inner_width);
                lines.push(Line::from_spans(vec![
                    Span::styled(" \u{2502} ", border_style),
                    Span::styled(padded, content_style),
                    Span::styled(" \u{2502}", border_style),
                ]));
            }
        }

        // Empty row if no content
        if self.content.trim().is_empty() {
            let padded = " ".repeat(inner_width);
            lines.push(Line::from_spans(vec![
                Span::styled(" \u{2502} ", border_style),
                Span::styled(padded, muted_style),
                Span::styled(" \u{2502}", border_style),
            ]));
        }

        // Bottom border: └─── ... ───┘
        let bottom_inner = "\u{2500}".repeat(width.saturating_sub(2));
        lines.push(Line::from_spans(vec![
            Span::styled(format!("\u{2514}{bottom_inner}"), border_style),
            Span::styled("\u{2519}", border_style),
        ]));

        lines
    }

    fn titled_border(title: &str, width: usize, border_style: Style) -> Line {
        // ┌ title ─── ... ───┐
        let title_display: String = title.chars().take(width.saturating_sub(2)).collect();
        let remaining = width
            .saturating_sub(2)
            .saturating_sub(title_display.chars().count());
        Line::from_spans(vec![
            Span::styled(
                format!("\u{250c}{title_display}{}", "\u{2500}".repeat(remaining)),
                border_style,
            ),
            Span::styled("\u{2510}", border_style),
        ])
    }
```

Key changes:
- Top border now ends with `┐` (`\u{2510}`) — added as a separate span
- Bottom border now ends with `┘` (`\u{2519}`) — added as a separate span
- Left/right vertical bars `│` now use `border_style` (green) instead of `content_style`
- Content text gets its own span with `content_style` (or `muted_style` for empty)
- `titled_border` now takes `Style` instead of `Color` and returns a `Line` with two spans

- [ ] **Step 4: Add the `Span` import**

At the top of `plan_box.rs`, update the import line to include `Span`:

```rust
use crate::primitive::Line;
use crate::primitive::{Color, Style, Span};
use crate::shell::TuiTheme;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo run -p xtask -- test -p neo-tui plan_box`
Expected: PASS — all 5 tests including the two new corner tests.

- [ ] **Step 6: Build check**

Run: `cargo build -p neo-tui`
Expected: Clean build.

---

## Task 2: Render PlanBox content as markdown

**Problem:** PlanBox content is rendered as plain text. Plan files are markdown (`.md`), and kimi-code renders them with headings, lists, code formatting etc.

**Files:**
- Modify: `crates/neo-tui/src/transcript/plan_box.rs` — `render` method, content lines section

- [ ] **Step 1: Add `render_markdown` import**

At the top of `plan_box.rs`, add:

```rust
use crate::markdown::render_markdown;
```

- [ ] **Step 2: Replace the content rendering loop**

In the `render` method, replace the plain-text content loop (the `for raw_line in self.content.lines()` block) with markdown rendering. The markdown-rendered lines need to be padded and wrapped with border characters.

Replace the current content rendering block:

```rust
        // Content lines
        for raw_line in self.content.lines() {
            for chunk in Self::wrap_text(raw_line, inner_width) {
                let padded = Self::pad_to(&chunk, inner_width);
                lines.push(Line::from_spans(vec![
                    Span::styled(" \u{2502} ", border_style),
                    Span::styled(padded, content_style),
                    Span::styled(" \u{2502}", border_style),
                ]));
            }
        }
```

With:

```rust
        // Content lines — render as markdown if the file is .md, plain text otherwise
        let is_markdown = self
            .path
            .as_ref()
            .and_then(|p| p.rsplit('.').nth(1))
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        let content_lines = if is_markdown {
            render_markdown(&self.content, inner_width, theme, "", "")
        } else {
            self.content
                .lines()
                .flat_map(|l| Self::wrap_text(l, inner_width))
                .map(|text| Line::styled(text, content_style))
                .collect::<Vec<_>>()
        };
        for line in content_lines {
            // Pad the rendered line's visible text to inner_width, then wrap
            // with border characters. The markdown renderer already applies
            // its own styling via spans, so we preserve those.
            let visible = line.visible_text();
            let padded = Self::pad_to(&visible, inner_width);
            let mut spans = vec![
                Span::styled(" \u{2502} ", border_style),
            ];
            spans.extend(line.spans);
            // Replace the content portion with padded version if shorter
            spans.push(Span::styled(
                " ".repeat(inner_width.saturating_sub(visible.chars().count())),
                Style::default(),
            ));
            spans.push(Span::styled(" \u{2502}", border_style));
            lines.push(Line::from_spans(spans));
        }
```

Wait — this approach has a problem. The `render_markdown` output has its own styling that we want to preserve, but padding is tricky because we can't easily measure the visible width of styled spans. Let me use a simpler approach: convert markdown lines to ANSI strings, then pad the plain text.

Actually, let me reconsider. The `Line` type has a `to_ansi()` method but we need `Line` output. Let me check if there's a `visible_text()` or similar method.

Let me use a cleaner approach: render markdown to lines, then for each line, extract its spans, calculate visible width, pad, and wrap with borders.

Replace with:

```rust
        // Content lines — render as markdown if the file is .md, plain text otherwise
        let is_markdown = self
            .path
            .as_ref()
            .and_then(|p| p.rsplit('.').nth(1))
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
        if is_markdown {
            let md_lines = render_markdown(&self.content, inner_width, theme, "", "");
            for md_line in md_lines {
                let visible_len = md_line.visible_len();
                let padding = " ".repeat(inner_width.saturating_sub(visible_len));
                let mut spans = vec![Span::styled(" \u{2502} ", border_style)];
                spans.extend(md_line.into_spans());
                spans.push(Span::styled(padding, Style::default()));
                spans.push(Span::styled(" \u{2502}", border_style));
                lines.push(Line::from_spans(spans));
            }
        } else {
            for raw_line in self.content.lines() {
                for chunk in Self::wrap_text(raw_line, inner_width) {
                    let padded = Self::pad_to(&chunk, inner_width);
                    lines.push(Line::from_spans(vec![
                        Span::styled(" \u{2502} ", border_style),
                        Span::styled(padded, content_style),
                        Span::styled(" \u{2502}", border_style),
                    ]));
                }
            }
        }
```

Note: This requires `Line` to have `visible_len()` and `into_spans()` methods. Check if they exist; if not, add them to the `Line` type in `primitive/line.rs`.

- [ ] **Step 3: Add `visible_len` and `into_spans` to `Line` if missing**

Check `crates/neo-tui/src/primitive/line.rs` for these methods. If `visible_len` doesn't exist, add:

```rust
    /// Return the total visible (non-ANSI) character count of all spans.
    #[must_use]
    pub fn visible_len(&self) -> usize {
        self.spans.iter().map(|s| s.content.chars().count()).sum()
    }

    /// Consume the line and return its spans.
    #[must_use]
    pub fn into_spans(self) -> Vec<Span> {
        self.spans
    }
```

- [ ] **Step 4: Write a test for markdown rendering**

Add to the test module in `plan_box.rs`:

```rust
    #[test]
    fn markdown_content_renders_headings() {
        let comp = PlanBoxComponent::new(
            "# Title\n\nSome text",
            Some("/tmp/plan.md".to_string()),
        );
        let lines = comp.render(60, &TuiTheme::default());
        // Should have top border + at least 2 content lines + bottom border
        assert!(lines.len() >= 4);
        // First content line should contain "Title" text
        let content = lines.iter().skip(1).take(3).map(|l| l.to_ansi()).collect::<String>();
        assert!(content.contains("Title"));
    }

    #[test]
    fn non_markdown_uses_plain_text() {
        let comp = PlanBoxComponent::new("plain text", Some("/tmp/plan.txt".to_string()));
        let lines = comp.render(40, &TuiTheme::default());
        assert!(lines.len() >= 3);
        let content = lines[1].to_ansi();
        assert!(content.contains("plain text"));
    }
```

- [ ] **Step 5: Run tests**

Run: `cargo run -p xtask -- test -p neo-tui plan_box`
Expected: PASS.

---

## Task 3: Unify Write streaming preview with final preview

**Problem:** During Write streaming, the tool card shows a progress line ("Preparing changes for ... ~N tok · Mm elapsed") followed by a tail-window of content without a header line. When Write completes, the format switches to a path+line-count header followed by a top-down numbered preview. This creates a jarring format jump.

**Goal:** During streaming, reuse `render_write_preview` (the same final renderer) so the format is consistent from start to finish. Move the progress indicator (token count, elapsed time) into a chip appended to the tool header instead of occupying a body line.

**Files:**
- Modify: `crates/neo-tui/src/transcript/tool_renderers.rs` — `render_streaming_preview` function (~line 608)
- Modify: `crates/neo-tui/src/transcript/tool_call.rs` — header rendering (~line 207)

- [ ] **Step 1: Rewrite `render_streaming_preview` to reuse `render_write_preview` for Write**

In `crates/neo-tui/src/transcript/tool_renderers.rs`, replace the `render_streaming_preview` function (the entire function body) with:

```rust
/// Render a live preview while a tool's arguments are still streaming from the
/// model. For Write, reuses the final `render_write_preview` format (with line
/// numbers and syntax highlighting) so there is no format switch on completion.
/// For Edit, shows only a progress line.
#[must_use]
pub fn render_streaming_preview(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
    _started_at: Option<std::time::Instant>,
) -> Vec<Line> {
    let args = state.arguments.as_deref().unwrap_or("");

    if state.name == "Write" {
        let path = extract_partial_string_field(args, "file_path")
            .or_else(|| extract_partial_string_field(args, "path"))
            .unwrap_or_default();
        let content = extract_partial_string_field(args, "content").unwrap_or_default();
        if content.is_empty() {
            // Content hasn't started streaming yet — show a minimal hint.
            return vec![Line::styled(
                "  Waiting for content...",
                Style::default().fg(theme.text_muted),
            )];
        }
        // Reuse the final preview renderer for format consistency.
        let palette = ToolBodyPalette::themed(theme);
        return render_write_preview(&path, &content, expanded, palette);
    }

    if state.name == "Edit" {
        let path = extract_partial_string_field(args, "file_path")
            .or_else(|| extract_partial_string_field(args, "path"))
            .unwrap_or_default();
        let tokens = estimate_tokens(args);
        let content = extract_partial_string_field(args, "content").unwrap_or_default();
        if content.is_empty() {
            return vec![Line::styled(
                format!("  Preparing edit for {path}... ~{} tok", format_token_count(tokens)),
                Style::default().fg(theme.text_muted),
            )];
        }
        // Edit streaming shows a brief progress line — no format unification
        // needed since Edit final view uses diff, not content preview.
        return vec![Line::styled(
            format!(
                "  Editing {path}... ~{} tok",
                format_token_count(tokens),
            ),
            Style::default().fg(theme.text_muted),
        )];
    }

    Vec::new()
}
```

Key changes:
- Write branch now calls `render_write_preview` directly (same function used after completion)
- No more `format_progress_line` or `render_write_streaming_content` calls for Write
- Progress info (tokens, elapsed) is removed from the body — it will move to the header chip (Step 3)
- Edit branch is simplified to a single progress line

- [ ] **Step 2: Add streaming token/elapsed chip to the Write tool header**

In `crates/neo-tui/src/transcript/tool_call.rs`, find `render_with_theme` (line ~207). After the header spans are built, add a streaming chip for Write/Edit when the tool is still running. Find the section that builds the header:

```rust
        let header_spans = if self.state.name == "ExitPlanMode" {
            crate::transcript::tool_renderers::exit_plan_mode_header_spans(&self.state, theme)
        } else {
            tool_header_spans(&self.state, theme, self.workspace_dir.as_deref())
        };
        let header_width = width.saturating_sub(2).max(1);
```

Replace with:

```rust
        let mut header_spans = if self.state.name == "ExitPlanMode" {
            crate::transcript::tool_renderers::exit_plan_mode_header_spans(&self.state, theme)
        } else {
            tool_header_spans(&self.state, theme, self.workspace_dir.as_deref())
        };
        // While Write/Edit is streaming, show a token count chip in the header.
        if is_pending_or_running(self.state.status)
            && is_file_write_tool(&self.state.name)
            && let Some(started_at) = self.streaming_started_at
        {
            let tokens = estimate_tool_tokens(self.state.arguments.as_deref().unwrap_or(""));
            let elapsed = started_at.elapsed().as_secs();
            let chip = format!(" · ~{} tok · {}m", format_tool_token_count(tokens), elapsed);
            header_spans.push(Span::styled(
                chip,
                Style::default().fg(theme.text_muted),
            ));
        }
        let header_width = width.saturating_sub(2).max(1);
```

This requires importing `estimate_tool_tokens` and `format_tool_token_count` — or making them accessible. To keep the change minimal, expose lightweight wrappers from `tool_renderers.rs`:

In `tool_renderers.rs`, add `pub` wrappers near `estimate_tokens`:

```rust
/// Public wrapper for token estimation (used by tool header chip).
#[must_use]
pub fn estimate_tool_tokens(args: &str) -> usize {
    estimate_tokens(args)
}

/// Public wrapper for token count formatting (used by tool header chip).
#[must_use]
pub fn format_tool_token_count(tokens: usize) -> String {
    format_token_count(tokens)
}
```

Then in `tool_call.rs`, add to the imports from `tool_renderers`:

```rust
use crate::transcript::tool_renderers::{estimate_tool_tokens, format_tool_token_count};
```

Or use the full path inline:
```rust
let tokens = crate::transcript::tool_renderers::estimate_tool_tokens(...);
```

- [ ] **Step 3: Verify `format_progress_line` and `render_write_streaming_content` are no longer called**

After the rewrite, `format_progress_line` and `render_write_streaming_content` may become unused. Check for callers:

Run: `rg "format_progress_line|render_write_streaming_content" crates/neo-tui/src/`
Expected: No remaining callers except possibly tests. If unused, mark them with `#[allow(dead_code)]` or remove them.

- [ ] **Step 4: Run tests**

Run: `cargo run -p xtask -- test -p neo-tui tool_cards`
Expected: PASS. If any test asserts the old progress line format, update it.

Run: `cargo run -p xtask -- test -p neo-tui`
Expected: All tests pass except pre-existing `scope_less_tool_approval_omits_approve_for_session_option`.

- [ ] **Step 5: Build check**

Run: `cargo build -p neo-tui`
Expected: Clean build, no warnings about unused functions (remove them if they are dead code).

---

## Self-Review Notes

1. **Spec coverage:**
   - PlanBox border corners + colors → Task 1 ✅
   - PlanBox markdown rendering → Task 2 ✅
   - Write streaming format unification → Task 3 ✅
   - Write progress line removal → Task 3 Step 1+2 ✅

2. **Type consistency:**
   - `titled_border` signature changes from `(title, width, Color)` to `(title, width, Style)` — Task 1 updates the only caller
   - `render_markdown` is called with `(content, inner_width, theme, "", "")` — matches its public signature
   - `Line::visible_len()` and `Line::into_spans()` may need to be added in Task 2 Step 3

3. **Potential issues:**
   - Markdown-rendered lines may contain ANSI codes that affect padding. The `visible_len()` method must count only visible characters, not escape sequences. Since `Span.content` stores raw text (not ANSI), `content.chars().count()` is correct.
   - The streaming chip in the header adds width to the header line. The existing `truncate_to_width(header_width)` handles this.
