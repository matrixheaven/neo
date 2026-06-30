# Markdown Code Block UI Design

## Status

Implemented in `crates/neo-tui/src/markdown.rs`.

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

`MdRenderer::new()` reserves `max(first_prefix, cont_prefix)` columns, so
`self.width` is the body width *after* the outer message prefix. The code block
box width is derived from the content and the header label, capped at
`self.width` so it never overflows the message body. `finish()` then prepends
the outer prefix (`first_prefix` on line 0, `cont_prefix` on all other lines),
aligning the box with the outer body text. It does not apply any current list
or blockquote marker indent.

Constants:

```rust
const CODE_SIDE_PADDING: usize = 2;
const CODE_MIN_BOX_WIDTH: usize = 12;
```

Derived values:

```text
max_content_width = max visible width of displayed code lines
header_label_width = visible_width("─ {lang} ") or 1 if no language
desired_inner_width = max_content_width + 2 * CODE_SIDE_PADDING
horz_len          = max(desired_inner_width, header_label_width, CODE_MIN_BOX_WIDTH - 2)
                      .min(self.width - 2)
content_width     = horz_len - 2 * CODE_SIDE_PADDING
box_width         = horz_len + 2
```

Line templates:

- Top:    `"╭─ {lang} {─ filled to horz_len}╮"` (border chars and fill are
  `text_muted`; only `{lang}` is `brand`)
- Content:`"│{padding}{code_filled_to content_width}{padding}│"`
- Bottom: `"╰{─ repeated horz_len}╯"`

If `self.width < CODE_MIN_BOX_WIDTH`, fall back to the old plain style
(`  ```{lang}` / `  {line}` / `  ````) without a border. This protects extremely
narrow terminals or deeply nested lists from broken-looking boxes.

### Header title construction

- Compute `horz_len = box_width - 2`.
- If a language is provided:
  - `label = format!("─ {lang} ")`.
  - If `visible_width(label) <= horz_len`:
    `title = label + "─".repeat(horz_len - visible_width(label))`.
  - Otherwise: `title = truncate_to_width(label, horz_len)`.
- If no language is provided:
  `title = "─".repeat(horz_len)`.

The remaining top-border width is filled with `─`, not spaces, so the header
reads as a continuous top edge. The frame characters and the `─` fill use
`text_muted`; only the language label text itself is rendered in `brand`.

## Component Changes

Primary change: `crates/neo-tui/src/markdown.rs`.

The rounded-box output also requires updating integration tests in
`crates/neo-tui/tests/markdown_rendering.rs` that assert on code block
rendering (e.g. replacing backtick-fence expectations with rounded-border
expectations).

### New constants

```rust
const CODE_SIDE_PADDING: usize = 2;
const CODE_MIN_BOX_WIDTH: usize = 12;
```

### `finish_code_block()` rewrite

Replace the current `` ``` ``-based output with:

1. Compute `max_content_width`, `header_label_width`, `horz_len`,
   `content_width`, and `box_width` based on content and the available width.
2. If `self.width < CODE_MIN_BOX_WIDTH`, emit plain indented lines.
3. Emit the top border with the language label in `brand` and the frame/fill
   in `text_muted`.
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
| Border characters, header `─` fill, corners, bottom edge | `theme.text_muted` |
| Header language label | `theme.brand` |
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
4. **Lists and blockquotes**: code blocks use `self.width` directly and are
   prefixed only with the outer `cont_prefix`/`first_prefix` in `finish()`.
   They do not inherit the current list or blockquote marker indent, so inside
   nested structures the box aligns with the outer continuation margin while
   still fitting within the total width.
5. **Prefix alignment**: for top-level assistant messages, `MdRenderer::new()`
   reserves `max(first_prefix, cont_prefix)` columns and both prefixes are 2
   columns wide (`"● "` and `"  "`). Therefore the code block box aligns with
   the body text. Callers with unequal prefix widths already have the same
   alignment behavior for all rendered lines and are not special-cased here.

## Testing Plan

Add tests to the `#[cfg(test)]` module in `crates/neo-tui/src/markdown.rs`:

| Test | Assertion |
|---|---|
| `code_block_has_rounded_borders` | Top contains `╭` and `╮`, bottom contains `╰` and `╯`, sides contain `│` |
| `code_block_width_is_consistent_and_within_bounds` | All lines share the same width and it is `<= input width` |
| `code_block_adapts_to_short_content` | Short content produces a box narrower than the full input width |
| `code_block_language_in_header` | Header line contains the language name |
| `code_block_no_fence_backticks` | Output contains no `` ``` `` sequences |
| `code_block_empty_content_renders_box` | Empty block emits top, empty content row, bottom |
| `code_block_honors_min_width` | Narrow widths degrade gracefully without panic |
| `code_block_in_list_renders_within_width` | Block inside a list does not exceed total width |

Run focused tests with:

```bash
```

## Trade-offs Considered

- **Rounded vs. sharp corners**: Rounded (`╭╮╰╯`) was chosen to differentiate
  code blocks from the sharp-cornered `plan_box.rs` tool cards.
- **With vs. without "copy" hint**: A "copy" label on the right was omitted to
  avoid implying functionality that is not implemented yet.
- **1-space vs. 2-space inner padding**: 2 spaces on each side gives code
  blocks more panel-like breathing room while still leaving ample width for
  code.
