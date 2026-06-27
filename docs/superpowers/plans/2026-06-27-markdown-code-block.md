# Markdown Code Block UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace plain `` ``` `` fenced code blocks in `render_markdown()`
with rounded-corner bordered panels that show a language label in the header.

**Architecture:** Keep the change localized to
`crates/neo-tui/src/markdown.rs`. Add two constants for layout, rewrite
`finish_code_block()` to emit top/header, content, and bottom borders, and add
focused unit tests in the same file. Reuse existing helpers (`visible_width`,
`pad_to_width`, `clip_visible_to_width`) for width-aware padding and
truncation.

**Tech Stack:** Rust, `pulldown-cmark`, `syntect`, custom `Line`/`Span`
primitives.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/neo-tui/src/markdown.rs` | Contains `render_markdown()` and `MdRenderer`. The only production file to change. |
| `crates/neo-tui/src/primitive/mod.rs` | Already re-exports `clip_visible_to_width` (crate-visible). No changes expected. |

---

## Task 1: Add Layout Constants and Import `clip_visible_to_width`

**Files:**
- Modify: `crates/neo-tui/src/markdown.rs:1-12`

Add two module-level constants near the top of the file, after the imports.
Also import `clip_visible_to_width` so highlighted code lines can be truncated
without stripping ANSI.

- [ ] **Step 1: Update imports and add constants**

```rust
use crate::primitive::{Color, Style, clip_plain_to_width, clip_visible_to_width, visible_width};
```

Add near the top of `markdown.rs`:

```rust
/// Inner horizontal padding between the side border and code content.
const CODE_SIDE_PADDING: usize = 2;
/// Minimum width for a code block box. Below this we fall back to plain text.
const CODE_MIN_BOX_WIDTH: usize = 12;
```

- [ ] **Step 2: Verify it compiles**

Run:

```bash
cargo check -p neo-tui
```

Expected: success.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-tui/src/markdown.rs
git commit -m "chore(neo-tui): add code block box layout constants"
```

---

## Task 2: Write Failing Tests for the New Code Block Box

**Files:**
- Modify: `crates/neo-tui/src/markdown.rs` (test module at the bottom)

Add tests before the closing brace of the `#[cfg(test)]` module.

- [ ] **Step 1: Add `code_block_has_rounded_borders`**

```rust
#[test]
fn code_block_has_rounded_borders() {
    let text = "```bash\necho hi\n```";
    let lines = render_markdown(text, 40, &TuiTheme::default(), "● ", "  ");
    let top = lines[0].to_ansi();
    assert!(top.contains('╭'), "top must contain ╭");
    assert!(top.contains('╮'), "top must contain ╮");
    let bottom = lines.last().unwrap().to_ansi();
    assert!(bottom.contains('╰'), "bottom must contain ╰");
    assert!(bottom.contains('╯'), "bottom must contain ╯");
    let all_plain: String = lines
        .iter()
        .map(|l| crate::primitive::strip_ansi(&l.to_ansi()))
        .collect();
    assert!(all_plain.contains('│'), "output must contain side borders");
}
```

- [ ] **Step 2: Add `code_block_width_equals_input_width`**

```rust
#[test]
fn code_block_width_equals_input_width() {
    let text = "```rust\nfn main() {\n    println!();\n}\n```";
    for width in [20, 40, 60, 80] {
        let lines = render_markdown(text, width, &TuiTheme::default(), "● ", "  ");
        for line in &lines {
            assert_eq!(
                line.visible_width(),
                width,
                "line must be exactly {width} columns: {:?}",
                line.to_ansi()
            );
        }
    }
}
```

- [ ] **Step 3: Add `code_block_language_in_header`**

```rust
#[test]
fn code_block_language_in_header() {
    let text = "```bash\necho hi\n```";
    let lines = render_markdown(text, 40, &TuiTheme::default(), "● ", "  ");
    let top = lines[0].to_ansi();
    assert!(top.contains("bash"), "header must contain language: {top}");
}
```

- [ ] **Step 4: Add `code_block_no_fence_backticks`**

```rust
#[test]
fn code_block_no_fence_backticks() {
    let text = "```bash\necho hi\n```";
    let all = render_markdown(text, 40, &TuiTheme::default(), "● ", "  ")
        .into_iter()
        .map(|l| crate::primitive::strip_ansi(&l.to_ansi()))
        .collect::<String>();
    assert!(!all.contains("```"), "output must not contain fence backticks");
}
```

- [ ] **Step 5: Add `code_block_empty_content_renders_box`**

```rust
#[test]
fn code_block_empty_content_renders_box() {
    let text = "```bash\n```";
    let lines = render_markdown(text, 30, &TuiTheme::default(), "● ", "  ");
    let top = lines[0].to_ansi();
    let bottom = lines.last().unwrap().to_ansi();
    assert!(top.contains('╭') && top.contains('╮'));
    assert!(bottom.contains('╰') && bottom.contains('╯'));
}
```

- [ ] **Step 6: Add `code_block_honors_min_width`**

```rust
#[test]
fn code_block_honors_min_width() {
    let text = "```bash\necho hi\n```";
    // Width is too small for a real box; just ensure no panic and lines fit.
    let lines = render_markdown(text, 4, &TuiTheme::default(), "● ", "  ");
    for line in &lines {
        assert!(line.visible_width() <= 4);
    }
}
```

- [ ] **Step 7: Add `code_block_in_list_renders_within_width`**

```rust
#[test]
fn code_block_in_list_renders_within_width() {
    let text = "- item\n\n  ```bash\n  echo hi\n  ```\n";
    let width = 40;
    let lines = render_markdown(text, width, &TuiTheme::default(), "● ", "  ");
    for line in &lines {
        assert!(
            line.visible_width() <= width,
            "line width {} should be <= {width}: {:?}",
            line.visible_width(),
            line.to_ansi()
        );
    }
}
```

- [ ] **Step 8: Run the tests to confirm they fail**

Run:

```bash
cargo run -p xtask -- test -p neo-tui code_block
```

Expected: tests fail because the new behavior is not implemented yet.

- [ ] **Step 9: Commit**

```bash
git add crates/neo-tui/src/markdown.rs
git commit -m "test(neo-tui): add code block box rendering tests"
```

---

## Task 3: Implement the Rounded Code Block Box

**Files:**
- Modify: `crates/neo-tui/src/markdown.rs` (replace `finish_code_block()`)

- [ ] **Step 1: Replace `finish_code_block()`**

Find the existing `fn finish_code_block(&mut self)` and replace it with:

```rust
fn finish_code_block(&mut self) {
    let lang = self.code_lang.take().unwrap_or_default();
    let code = std::mem::take(&mut self.code_buffer);
    self.buffering_code = false;

    if self.width < CODE_MIN_BOX_WIDTH {
        self.emit_plain_code_block(&lang, &code);
        return;
    }

    let box_width = self.width;
    let horz_len = box_width - 2;
    let content_width = box_width.saturating_sub(2 + 2 * CODE_SIDE_PADDING).max(1);
    let border_style = Style::default().fg(self.theme.text_muted);
    let brand_style = Style::default().fg(self.theme.brand);

    // Top border with language label.
    let title = if lang.is_empty() {
        "─".to_owned()
    } else {
        format!("─ {lang} ")
    };
    let title_fitted = if visible_width(&title) <= horz_len {
        pad_to_width(&title, horz_len)
    } else {
        truncate_to_width(&title, horz_len)
    };
    self.out.push(Line::from_spans(vec![
        Span::styled("╭", border_style),
        Span::styled(title_fitted, brand_style),
        Span::styled("╮", border_style),
    ]));

    // Content lines.
    let raw_lines: Vec<&str> = code.trim_end_matches('\n').lines().collect();
    if raw_lines.is_empty() {
        self.emit_code_content_line("", content_width, border_style);
    } else if lang.eq_ignore_ascii_case("diff") {
        for line in raw_lines {
            self.emit_diff_box_line(line, content_width, border_style);
        }
    } else {
        let highlighted = highlight_code(&code, &lang, self.theme);
        for line in highlighted {
            let fitted = fit_ansi_line_to_width(&line, content_width);
            self.emit_code_content_line(&fitted, content_width, border_style);
        }
    }

    // Bottom border.
    let bottom_inner = "─".repeat(horz_len);
    self.out.push(Line::from_spans(vec![
        Span::styled("╰", border_style),
        Span::styled(bottom_inner, border_style),
        Span::styled("╯", border_style),
    ]));

    // Trailing blank line.
    self.out.push(Line::raw(""));
}
```

- [ ] **Step 2: Add helper methods**

Add the following helper methods inside `impl<'a> MdRenderer<'a> { ... }`:

```rust
fn emit_code_content_line(&mut self, text: &str, content_width: usize, border_style: Style) {
    let fitted = pad_to_width(text, content_width);
    self.out.push(Line::from_spans(vec![
        Span::styled("│", border_style),
        Span::raw(" ".repeat(CODE_SIDE_PADDING)),
        Span::raw(fitted),
        Span::raw(crate::primitive::RESET.to_string()),
        Span::raw(" ".repeat(CODE_SIDE_PADDING)),
        Span::styled("│", border_style),
    ]));
}

fn emit_diff_box_line(&mut self, line: &str, content_width: usize, border_style: Style) {
    let (color, text) = if let Some(t) = line.strip_prefix('+') {
        (self.theme.diff_added, t)
    } else if let Some(t) = line.strip_prefix('-') {
        (self.theme.diff_removed, t)
    } else if line.starts_with("@@") {
        (self.theme.diff_hunk, line)
    } else {
        (self.theme.diff_context, line)
    };
    let fitted = pad_to_width(&truncate_to_width(text, content_width), content_width);
    self.out.push(Line::from_spans(vec![
        Span::styled("│", border_style),
        Span::raw(" ".repeat(CODE_SIDE_PADDING)),
        Span::styled(fitted, Style::default().fg(color)),
        Span::raw(crate::primitive::RESET.to_string()),
        Span::raw(" ".repeat(CODE_SIDE_PADDING)),
        Span::styled("│", border_style),
    ]));
}

fn emit_plain_code_block(&mut self, lang: &str, code: &str) {
    let border = if lang.is_empty() {
        "```".to_owned()
    } else {
        format!("```{lang}")
    };
    self.out.push(Line::styled(
        format!("  {border}"),
        Style::default().fg(self.theme.text_muted),
    ));
    for raw_line in code.trim_end_matches('\n').lines() {
        self.out.push(Line::raw(format!("  {raw_line}")));
    }
    self.out.push(Line::styled(
        "  ```".to_owned(),
        Style::default().fg(self.theme.text_muted),
    ));
    self.out.push(Line::raw(""));
}
```

- [ ] **Step 3: Add free helper `fit_ansi_line_to_width`**

Add this free function near the other helpers in `markdown.rs`:

```rust
/// Pad or hard-truncate an ANSI-styled line to exactly `width` visible columns.
fn fit_ansi_line_to_width(line: &str, width: usize) -> String {
    let vis = visible_width(line);
    if vis > width {
        clip_visible_to_width(line, width)
    } else {
        let mut result = line.to_owned();
        result.push_str(&" ".repeat(width - vis));
        result
    }
}
```

- [ ] **Step 4: Run the new tests**

Run:

```bash
cargo run -p xtask -- test -p neo-tui code_block
```

Expected: tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-tui/src/markdown.rs
git commit -m "feat(neo-tui): render markdown code blocks as rounded boxes"
```

---

## Task 4: Run the Full Markdown Test Suite and Fix Regressions

**Files:**
- Modify: `crates/neo-tui/src/markdown.rs` (if any existing tests fail)

- [ ] **Step 1: Run all markdown tests**

```bash
cargo run -p xtask -- test -p neo-tui markdown
```

Expected: all tests pass. If existing tests asserted on `` ``` `` markers, update
those assertions to match the new box style.

- [ ] **Step 2: Run the neo-tui check**

```bash
cargo run -p xtask -- check --workspace
```

Expected: fmt, clippy, and nextest all pass. Fix any formatting or clippy
warnings in the changed file.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-tui/src/markdown.rs
git commit -m "test(neo-tui): adjust markdown tests for new code block boxes"
```

---

## Task 5: Verify the Design Doc is Current

**Files:**
- Read: `docs/superpowers/specs/2026-06-27-markdown-code-block-design.md`

- [ ] **Step 1: Compare implementation against spec**

Skim the spec sections (Visual Design, Layout, Component Changes, Colors,
Behavior, Testing). Confirm the implementation matches.

If anything diverged, update the spec or the code to match. The source of truth
after implementation should be the code, with the spec updated to reflect it.

- [ ] **Step 2: Commit any spec updates**

```bash
git add docs/superpowers/specs/2026-06-27-markdown-code-block-design.md
git commit -m "docs: update code block design spec after implementation"
```

---

## Self-Review

- **Spec coverage:**
  - Rounded box with language header → Task 3.
  - Full message width alignment → Task 3 uses `self.width` and existing prefix
    reservation.
  - No `` ``` `` fences → Task 3 removes them and Task 2 test verifies.
  - Empty blocks, narrow fallback, diff blocks → Task 3 helpers.
  - Color scheme → Task 3 uses `text_muted` and `brand`.
  - Tests → Task 2 and Task 4.
- **Placeholder scan:** No TBD/TODO; every step has concrete code or commands.
- **Type consistency:** `render_markdown()` signature unchanged. `MdRenderer`
  helpers use `usize` widths and `Style` consistently.

## Execution Handoff

Plan complete and saved to
`docs/superpowers/plans/2026-06-27-markdown-code-block.md`.

Two execution options:

1. **Subagent-Driven (recommended)** — Dispatch a fresh subagent per task,
   review between tasks, fast iteration.
2. **Inline Execution** — Execute tasks in this session using
   `executing-plans`, batch execution with checkpoints.

Which approach?
