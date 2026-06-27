# Markdown Code Block UI Design

## Status

Approved design, ready for implementation planning.

## Goal

Redesign fenced code blocks rendered in assistant messages so they look like
polished UI panels instead of plain `` ``` `` markers with an awkward 2-space
indentation.

## Scope

All fenced code blocks produced by `render_markdown()` in
`crates/neo-tui/src/markdown.rs`. This covers assistant transcript messages and
any other callers of `render_markdown()`.

Out of scope:

- Inline `` `code` `` spans (keep current style).
- `plan_box.rs` tool-card box style.
- Copy-to-clipboard functionality (visual hint only, if added later).

## Visual Design

Code blocks become rounded-corner boxes that span the full available message
width. The top border contains a left-aligned language label. Fenced code
markers (`` ``` ``) are removed entirely.

### With language label

```text
● 这是正文段落，正文从这里开始，正文正文正文正文正文正文正文
  正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文

  ╭─ bash ───────────────────────────────────────────────────╮
  │  cargo run -p xtask -- test -p neo-tui plan_box          │
  │                                                          │
  │  # 11 tests run: 11 passed, 492 skipped                  │
  ╰──────────────────────────────────────────────────────────╯

  正文继续，正文正文正文正文正文正文正文正文正文正文正文正文正文
```

### Without language label

```text
● 这是正文段落，正文从这里开始，正文正文正文正文正文正文正文
  正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文

  ╭──────────────────────────────────────────────────────────╮
  │  some plain text block                                   │
  │  second line                                             │
  ╰──────────────────────────────────────────────────────────╯

  正文继续，正文正文正文正文正文正文正文正文正文正文正文正文正文
```

### Narrow terminal

```text
● 正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文

  ╭─ rust ───────────╮
  │  fn main() {     │
  │      println!()  │
  │  }               │
  ╰──────────────────╯

  正文继续正文正文正文正文正文
```

## Layout and Width Math

`MdRenderer::new()` already reserves `max(first_prefix, cont_prefix)` columns,
so `self.width` is the body width *after* the message prefix. The code block
box renders at exactly `self.width`; `finish()` then prepends `cont_prefix`,
aligning the box with the body text.

Constants:

```rust
const CODE_SIDE_PADDING: usize = 2;
const CODE_MIN_BOX_WIDTH: usize = 12;
```

Derived values:

```text
box_width       = self.width
horz_len        = box_width - 2
content_width   = box_width - 2 - 2 * CODE_SIDE_PADDING
```

Line templates:

- Top:    `"╭{title_filled_to_horz_len}╮"`
- Content:`"│{padding}{code_filled_to_content_width}{padding}│"`
- Bottom: `"╰{─ repeated horz_len}╯"`

If `box_width < CODE_MIN_BOX_WIDTH`, fall back to the old plain style
(`  ```{lang}` / `  {line}` / `  ````) without a border. This protects extremely
narrow terminals or deeply nested lists from broken-looking boxes.

### Header title construction

- If a language is provided: `title = format!("─ {lang} ")`.
- If no language is provided: `title = "─"` (minimal corner transition).
- Fit `title` to `horz_len` using `pad_to_width` or `truncate_to_width`.

## Component Changes

Only `crates/neo-tui/src/markdown.rs` changes.

### New constants

```rust
const CODE_SIDE_PADDING: usize = 2;
const CODE_MIN_BOX_WIDTH: usize = 12;
```

### `finish_code_block()` rewrite

Replace the current `` ``` ``-based output with:

1. Compute `box_width`, `horz_len`, and `content_width`.
2. If `box_width < CODE_MIN_BOX_WIDTH`, emit plain indented lines.
3. Emit the top border with the fitted title.
4. For each code line:
   - Truncate or pad to `content_width`.
   - Wrap in `"│  {line}  │"`.
5. Emit the bottom border.
6. Emit one trailing blank line to separate the block from the following
   paragraph.

`diff` language blocks continue to use their existing color logic inside the
new box.

### Unchanged

- `render_markdown()` signature.
- Message prefix handling (`first_prefix`/`cont_prefix`).
- `plan_box.rs` styling.

## Color Scheme

| Element | Color |
|---|---|
| Border characters | `theme.text_muted` |
| Header language label | `theme.brand` |
| Header fill line | `theme.text_muted` |
| Code content | Existing syntect highlight; `theme.text_primary` fallback |
| Diff `+` lines | `theme.diff_added` |
| Diff `-` lines | `theme.diff_removed` |
| Diff `@@` lines | `theme.diff_hunk` |
| Diff context | `theme.diff_context` |

No background color is set; rely on border and padding for visual separation.

## Behavior Details

1. **Empty code blocks** still render the full box with an empty content line.
2. **Long single lines** are hard-truncated to `content_width`; code blocks do
   not soft-wrap.
3. **Spacing**: the box is followed by one blank line. The preceding blank line
   comes naturally from the end of the previous paragraph.
4. **Lists and blockquotes**: because `self.width` already excludes list/quote
   prefixes, the box fills the remaining width correctly inside nested
   structures.
5. **Prefix alignment**: `MdRenderer::new()` reserves
   `max(first_prefix, cont_prefix)` columns, and for assistant messages both
   prefixes are 2 columns wide (`"● "` and `"  "`). Therefore the code block
   box, rendered at `self.width` and then prefixed in `finish()`, aligns with
   the body text. Callers with unequal prefix widths already have the same
   alignment behavior for all rendered lines and are not special-cased here.

## Testing Plan

Add tests to the `#[cfg(test)]` module in `crates/neo-tui/src/markdown.rs`:

| Test | Assertion |
|---|---|
| `code_block_has_rounded_borders` | Top contains `╭` and `╮`, bottom contains `╰` and `╯`, sides contain `│` |
| `code_block_width_equals_input_width` | Every line has `visible_width() == width` |
| `code_block_language_in_header` | Header line contains the language name |
| `code_block_no_fence_backticks` | Output contains no `` ``` `` sequences |
| `code_block_empty_content_renders_box` | Empty block emits top, empty content row, bottom |
| `code_block_honors_min_width` | Narrow widths degrade gracefully without panic |
| `code_block_in_list_renders_within_width` | Block inside a list does not exceed total width |

Run focused tests with:

```bash
cargo run -p xtask -- test -p neo-tui markdown
```

## Trade-offs Considered

- **Rounded vs. sharp corners**: Rounded (`╭╮╰╯`) was chosen to differentiate
  code blocks from the sharp-cornered `plan_box.rs` tool cards.
- **With vs. without "copy" hint**: A "copy" label on the right was omitted to
  avoid implying functionality that is not implemented yet.
- **1-space vs. 2-space inner padding**: 2 spaces on each side gives code
  blocks more panel-like breathing room while still leaving ample width for
  code.
