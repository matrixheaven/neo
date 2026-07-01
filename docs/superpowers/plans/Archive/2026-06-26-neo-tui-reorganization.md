# neo-tui Code Organization Refactoring Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reorganize neo-tui from a flat 16-module layout with a 4508-line monolith into a clean hierarchical structure with clear, self-descriptive snake_case names.

**Architecture:** Five-phase refactoring ordered by blast radius — low-risk renames first, then primitive consolidation, then module splits, then the chrome.rs decomposition, and finally the transcript event handler extraction. Every phase produces a compilable, tested crate. Cross-crate impact is limited to 2 files in neo-agent (`themes.rs`, `modes/interactive.rs`) with ~12 total references.


---

## Naming Decisions

| Current | What it actually does | New name | Rationale |
|---------|----------------------|----------|-----------|
| `terminal/` | Differential frame-to-stdout renderer (output only, NOT terminal emulation) | `screen_output/` | "terminal" implies emulation; this is purely screen output |
| `chrome.rs` | Application shell state: overlays, prompt, theme, approval flow, pickers | `shell/` | "chrome" is jargon; "shell" = the shell of the app |
| `components.rs` | 4 text width/wrapping utility functions | merged into `primitive/text_layout.rs` | NOT components — these are text layout primitives |
| `core/` | Line, Span, Text, Component trait, Container | merged into `primitive/` | "core" is generic; these are rendering primitives |
| `core/terminal.rs` | 9-line dead doc-only redirect | **deleted** | Already a dead redirect to `screen_output::TuiRenderer` |
| `ansi.rs` | Color/Style/Rect types + ANSI escape functions + text layout utils | split into `primitive/color.rs`, `primitive/style.rs`, `primitive/ansi_escape.rs`, `primitive/text_layout.rs` | Mixes 3 concerns: color types, escape sequences, text measurement |
| `image.rs` | 3 terminal image protocol encoders (Kitty/iTerm2/Sixel) | `terminal_image/` | These are terminal image protocol codecs |
| `input.rs` | InputEvent, InputParser, KeyId, KeybindingAction, KeybindingsManager | split into `input/` directory | 4 distinct concerns in one file |
| `neo_tui.rs` | Top-level facade composing chrome + transcript | `app.rs` | Avoids `neo_tui::neo_tui::NeoTui` stutter |
| `tool_diff.rs` | Unified diff parser & data model | `diff_model.rs` | NOT tool-specific — it's a general diff model |
| `question_dialog.rs` | AskUserQuestion state machine (modal dialog) | moved from `widgets/` to `dialogs/` | It IS a dialog, not a widget |

Types that keep their names: `TuiRenderer`, `NeoTui`, `NeoChromeState`, `TuiTheme`, `TranscriptPane`, etc. — only module paths change.

## Final Target Structure

```
src/
├── lib.rs
├── app.rs                      ← was neo_tui.rs
├── diff_model.rs               ← was tool_diff.rs
├── markdown.rs
├── paste.rs
├── searchable_list.rs
├── token_estimate.rs
│
├── primitive/                  ← NEW: merges core/ + ansi.rs + components.rs
│   ├── mod.rs
│   ├── color.rs                ← Color enum (from ansi.rs)
│   ├── style.rs                ← Style, Rect, RESET (from ansi.rs)
│   ├── ansi_escape.rs          ← paint, strip_ansi, fg/bg_to_ansi, next_sequence, update_active_sgr
│   ├── text_layout.rs          ← visible_width, display_width, wrap_*, truncate_*, clip_* (from ansi.rs + components.rs)
│   ├── line.rs                 ← Line, Span (from core/)
│   ├── text.rs                 ← Text (from core/)
│   ├── component.rs            ← Component trait, InputResult, Finalization, Expandable (from core/)
│   └── container.rs            ← Container, GutterContainer (from core/)
│
├── screen_output/              ← was terminal/
│   ├── mod.rs
│   └── frame_differ.rs         ← was renderer.rs (TuiRenderer, CursorPos, CURSOR_MARKER)
│
├── input/                      ← expanded from input.rs
│   ├── mod.rs                  ← InputEvent, InputParser
│   ├── key_id.rs               ← KeyId, KeyIdError
│   ├── keybinding.rs           ← KeybindingAction, KeybindingDefinition, KeybindingConflict, KeybindingsManager
│   └── raw_input.rs            ← unchanged
│
├── terminal_image/             ← was image.rs
│   ├── mod.rs                  ← protocol negotiation, ImageRenderPolicy, TerminalImageCapabilities, etc.
│   ├── kitty.rs                ← encode_kitty_graphics
│   ├── iterm2.rs               ← encode_iterm2_inline_image
│   └── sixel.rs                ← encode_sixel_image
│
├── shell/                      ← was chrome.rs (4508 lines → 14 files)
│   ├── mod.rs                  ← NeoChromeState struct + core methods + re-exports
│   ├── theme.rs                ← TuiTheme, ChromeMode, DevelopmentMode, GoalModeStatus
│   ├── overlay.rs              ← Overlay, OverlayId, OverlayKind, OverlayListSelection
│   ├── prompt.rs               ← PromptState, PromptSnapshot, PromptEdit, PromptCompletionPrefix + helpers
│   ├── approval.rs             ← ApprovalChoice, ApprovalOption, ApprovalModal, ApprovalRequestModal, ApprovalResult
│   ├── pickers.rs              ← PickerItem, PickerState, ModelPickerState, PromptCompletionState
│   ├── command_palette.rs      ← CommandSpec, CommandPaletteState
│   ├── select_list.rs          ← SelectItem, SelectListState, VisibleSelectItem
│   ├── session_picker.rs       ← SessionPickerItem, SessionPickerState, SessionPickerScope + helpers
│   ├── context.rs              ← ContextWindow
│   ├── stream.rs               ← StreamUpdate, ToolStatusKind
│   ├── image_cache.rs          ← InlineImageRenderCache
│   ├── pending_input.rs        ← PendingInputState
│   └── dialog_dispatch.rs      ← DialogInputRef, DialogInputOwned traits + impls + dispatch functions
│
├── transcript/                 ← mostly unchanged
│   ├── mod.rs
│   ├── entry.rs
│   ├── pane.rs                 ← slimmed: rendering only
│   ├── event_handler.rs        ← NEW: apply_agent_event extracted from pane.rs
│   ├── store.rs
│   ├── tool_call.rs
│   ├── tool_group.rs
│   ├── tool_renderers.rs
│   ├── partial_json.rs
│   ├── plan_box.rs
│   └── diff_preview.rs
│
├── dialogs/                    ← question_dialog.rs moves here from widgets/
│   ├── mod.rs
│   ├── question_dialog.rs      ← moved from widgets/
│   └── ... (all existing dialogs unchanged)
│
└── widgets/                    ← question_dialog.rs removed
    ├── mod.rs
    ├── box_draw.rs
    ├── btw_panel.rs
    ├── pending_input_preview.rs
    └── todo_panel.rs
```

## Verification Baseline

Before starting, establish the test baseline:

```bash
```

Expected: All neo-tui tests pass. Record the pass count for later comparison.

---

## Phase 1: Low-Risk Renames

### Task 1: Delete `core/terminal.rs` dead redirect

**Files:**
- Delete: `crates/neo-tui/src/core/terminal.rs`
- Modify: `crates/neo-tui/src/core/mod.rs`

- [ ] **Step 1: Delete the file**

```bash
rm crates/neo-tui/src/core/terminal.rs
```

- [ ] **Step 2: Remove the module declaration from core/mod.rs**

In `crates/neo-tui/src/core/mod.rs`, remove the line `pub mod terminal;`.

The file should become:

```rust
pub mod component;
pub mod container;
pub mod line;
pub mod text;

pub use component::{Component, Expandable, Finalization, InputResult};
pub use container::{Container, GutterContainer};
pub use line::{Line, Span};
pub use text::Text;
```

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p neo-tui`
Expected: Zero errors

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "refactor(neo-tui): delete dead core/terminal.rs redirect"
```

---

### Task 2: Rename `tool_diff.rs` → `diff_model.rs`

**Files:**
- Rename: `crates/neo-tui/src/tool_diff.rs` → `crates/neo-tui/src/diff_model.rs`
- Modify: `crates/neo-tui/src/lib.rs`
- Modify: `crates/neo-tui/src/transcript/tool_renderers.rs`

Import sites: 1 (only `transcript/tool_renderers.rs:9`).

- [ ] **Step 1: Rename the file**

```bash
mv crates/neo-tui/src/tool_diff.rs crates/neo-tui/src/diff_model.rs
```

- [ ] **Step 2: Update lib.rs**

In `crates/neo-tui/src/lib.rs`, change `pub mod tool_diff;` to `pub mod diff_model;`.

- [ ] **Step 3: Update the one import site**

In `crates/neo-tui/src/transcript/tool_renderers.rs` line 9, change:

```rust
use crate::tool_diff::{DiffModel, DiffRenderLine, DiffRenderLineKind, DiffRenderState};
```
to:
```rust
use crate::diff_model::{DiffModel, DiffRenderLine, DiffRenderLineKind, DiffRenderState};
```

- [ ] **Step 4: Verify**

Expected: Zero errors, all tests pass.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor(neo-tui): rename tool_diff → diff_model (not tool-specific)"
```

---

### Task 3: Rename `terminal/` → `screen_output/`

**Files:**
- Rename: `crates/neo-tui/src/terminal/` → `crates/neo-tui/src/screen_output/`
- Rename: `crates/neo-tui/src/screen_output/renderer.rs` → `crates/neo-tui/src/screen_output/frame_differ.rs`
- Modify: `crates/neo-tui/src/screen_output/mod.rs`
- Modify: `crates/neo-tui/src/lib.rs`
- Modify all import sites (4 intra-crate, 0 cross-crate via this path).

Intra-crate import sites for `crate::terminal::`:
- `ansi.rs:590,609` — `crate::terminal::CURSOR_MARKER` (test code)
- `neo_tui.rs:2` — `use crate::terminal::CursorPos;`
- `transcript/pane.rs:19` — `use crate::terminal::{CURSOR_MARKER, CursorPos};`
- `transcript/pane.rs:963` — doc comment reference

- [ ] **Step 1: Rename the directory and file**

```bash
mv crates/neo-tui/src/terminal crates/neo-tui/src/screen_output
mv crates/neo-tui/src/screen_output/renderer.rs crates/neo-tui/src/screen_output/frame_differ.rs
```

- [ ] **Step 2: Update screen_output/mod.rs**

Replace the entire file content:

```rust
//! Screen output rendering — differential frame-to-stdout renderer.
//!
//! This module contains the single-buffer differential renderer that takes
//! complete frames (`Vec<String>` with embedded ANSI codes) and writes only
//! the changed lines to stdout. It is NOT a terminal emulator — it never
//! reads stdin or parses user input. Input handling lives in `crate::input`.

pub mod frame_differ;

pub use frame_differ::{CURSOR_MARKER, CursorPos, TuiRenderer};
```

- [ ] **Step 3: Update lib.rs**

In `crates/neo-tui/src/lib.rs`, change `pub mod terminal;` to `pub mod screen_output;`.

- [ ] **Step 4: Update all import sites**

In `crates/neo-tui/src/ansi.rs` test code (around lines 590, 609), replace `crate::terminal::CURSOR_MARKER` with `crate::screen_output::CURSOR_MARKER`.

In `crates/neo-tui/src/neo_tui.rs` line 2, change:
```rust
use crate::terminal::CursorPos;
```
to:
```rust
use crate::screen_output::CursorPos;
```

In `crates/neo-tui/src/transcript/pane.rs` line 19, change:
```rust
use crate::terminal::{CURSOR_MARKER, CursorPos};
```
to:
```rust
use crate::screen_output::{CURSOR_MARKER, CursorPos};
```

In `crates/neo-tui/src/transcript/pane.rs` around line 963 (doc comment), change `crate::terminal::TuiRenderer` to `crate::screen_output::TuiRenderer`.

- [ ] **Step 5: Verify**

Expected: Zero errors, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(neo-tui): rename terminal/ → screen_output/ (not a terminal emulator)"
```

---

### Task 4: Rename `neo_tui.rs` → `app.rs`

**Files:**
- Rename: `crates/neo-tui/src/neo_tui.rs` → `crates/neo-tui/src/app.rs`
- Modify: `crates/neo-tui/src/lib.rs`

The `NeoTui` struct name stays the same — only the module path changes. Import sites:
- `lib.rs:18` — `pub use neo_tui::NeoTui;`
- Tests: `trust_dialog.rs:107`, `todo_question.rs:79,121`, `app_shell.rs:590,623,661` — all use `neo_tui::NeoTui`

- [ ] **Step 1: Rename the file**

```bash
mv crates/neo-tui/src/neo_tui.rs crates/neo-tui/src/app.rs
```

- [ ] **Step 2: Update lib.rs**

Change `pub mod neo_tui;` to `pub mod app;` and `pub use neo_tui::NeoTui;` to `pub use app::NeoTui;`.

- [ ] **Step 3: Update internal references in app.rs**

In `crates/neo-tui/src/app.rs`, update any `use crate::terminal::` to `use crate::screen_output::` (from Task 3) if not already done. Also update `use crate::chrome::` stays the same for now.

- [ ] **Step 4: Update test files**

In `crates/neo-tui/tests/trust_dialog.rs`, `todo_question.rs`, `app_shell.rs`:
Replace all occurrences of `neo_tui::NeoTui` with `app::NeoTui`.

```bash
# Verify what needs changing:
grep -rn 'neo_tui::NeoTui\|neo_tui::transcript' crates/neo-tui/tests/
# Apply the rename:
find crates/neo-tui/tests -name '*.rs' -exec sed -i '' 's/neo_tui::NeoTui/app::NeoTui/g; s/neo_tui::transcript/app::transcript/g' {} +
```

Note: Some tests may reference `neo_tui::` as the crate name (e.g., in `use neo_tui::*`). Those are crate-level imports and do NOT change. Only the module path `neo_tui::NeoTui` (which becomes `app::NeoTui`) changes.

- [ ] **Step 5: Verify**

Expected: Zero errors, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(neo-tui): rename neo_tui.rs → app.rs (avoid module name stutter)"
```

---

### Task 5: Move `question_dialog.rs` from `widgets/` to `dialogs/`

**Files:**
- Move: `crates/neo-tui/src/widgets/question_dialog.rs` → `crates/neo-tui/src/dialogs/question_dialog.rs`
- Modify: `crates/neo-tui/src/widgets/mod.rs`
- Modify: `crates/neo-tui/src/dialogs/mod.rs`

No internal references to `crate::widgets::question_dialog` exist (verified). The module is surfaced via `widgets/mod.rs` glob re-export. The dialog types (`QuestionStateMachine`, etc.) are used by `chrome.rs`.

- [ ] **Step 1: Move the file**

```bash
mv crates/neo-tui/src/widgets/question_dialog.rs crates/neo-tui/src/dialogs/question_dialog.rs
```

- [ ] **Step 2: Update widgets/mod.rs**

Remove from `crates/neo-tui/src/widgets/mod.rs`:
- `pub mod question_dialog;`
- The `pub use question_dialog::{...}` block

The file should become:

```rust
pub mod box_draw;
pub mod btw_panel;
pub mod pending_input_preview;
pub mod todo_panel;

pub use box_draw::*;
pub use btw_panel::*;
pub use pending_input_preview::PendingInputPreview;
pub use todo_panel::{TodoDisplayItem, TodoDisplayStatus, TodoPanel, select_visible_todos};
```

- [ ] **Step 3: Update dialogs/mod.rs**

Add to `crates/neo-tui/src/dialogs/mod.rs`:

```rust
pub mod question_dialog;

pub use question_dialog::{
    QuestionDialogAction, QuestionDisplayData, QuestionDisplayOption,
    QuestionOptionState, QuestionResult, QuestionState, QuestionStateMachine,
};
```

- [ ] **Step 4: Update import sites**

In `crates/neo-tui/src/chrome.rs` (or wherever it is by this point), update:

```rust
use crate::widgets::{QuestionDialogAction, QuestionDisplayData, QuestionDisplayOption,
    QuestionResult, QuestionStateMachine, TodoDisplayItem, TodoDisplayStatus};
```
to:
```rust
use crate::widgets::{TodoDisplayItem, TodoDisplayStatus};
use crate::dialogs::{
    QuestionDialogAction, QuestionDisplayData, QuestionDisplayOption,
    QuestionResult, QuestionStateMachine,
};
```

Search for any other `crate::widgets::Question*` references:
```bash
grep -rn 'crate::widgets::Question' crates/neo-tui/src/
```
Update any matches to `crate::dialogs::Question*`.

- [ ] **Step 5: Verify**

Expected: Zero errors, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(neo-tui): move question_dialog from widgets to dialogs (it IS a dialog)"
```

---

## Phase 2: Primitive Consolidation

### Task 6: Create `primitive/` by merging `core/` + `ansi.rs` + `components.rs`

This is the highest blast-radius change in terms of import count (~180 sites). The approach: create the new directory, re-export everything from `primitive/mod.rs`, then bulk-replace all old import paths.

**Files:**
- Create: `crates/neo-tui/src/primitive/mod.rs`
- Create: `crates/neo-tui/src/primitive/color.rs` (Color enum from ansi.rs)
- Create: `crates/neo-tui/src/primitive/style.rs` (Style, Rect, RESET from ansi.rs)
- Create: `crates/neo-tui/src/primitive/ansi_escape.rs` (ANSI escape functions from ansi.rs)
- Create: `crates/neo-tui/src/primitive/text_layout.rs` (width/wrap/truncate from ansi.rs + components.rs)
- Move: `crates/neo-tui/src/core/line.rs` → `crates/neo-tui/src/primitive/line.rs`
- Move: `crates/neo-tui/src/core/text.rs` → `crates/neo-tui/src/primitive/text.rs`
- Move: `crates/neo-tui/src/core/component.rs` → `crates/neo-tui/src/primitive/component.rs`
- Move: `crates/neo-tui/src/core/container.rs` → `crates/neo-tui/src/primitive/container.rs`
- Delete: `crates/neo-tui/src/ansi.rs` (contents split into primitive/)
- Delete: `crates/neo-tui/src/components.rs` (contents merged into primitive/text_layout.rs)
- Delete: `crates/neo-tui/src/core/mod.rs` (replaced by primitive/mod.rs)
- Modify: `crates/neo-tui/src/lib.rs`
- Modify: ALL files in neo-tui that import `crate::ansi::`, `crate::components::`, or `crate::core::`
- Modify: `crates/neo-agent/src/themes.rs` (2 cross-crate imports)
- Modify: `crates/neo-agent/src/modes/interactive.rs` (8 cross-crate imports)

**Split plan for `ansi.rs` contents:**

| Current location in ansi.rs | Destination |
|-----------------------------|-------------|
| `Color` enum + impls | `primitive/color.rs` |
| `Style` struct + impls, `Rect` struct, `RESET` const | `primitive/style.rs` |
| `fg_to_ansi`, `bg_to_ansi`, `style_to_ansi`, `paint`, `strip_ansi`, `next_sequence`, `update_active_sgr` | `primitive/ansi_escape.rs` |
| `visible_width`, `display_width`, `wrap_text`, `pad_to_width`, `truncate_to_width`, `clip_plain_to_width`, `clip_visible_to_width` | `primitive/text_layout.rs` |

**Split plan for `components.rs` contents:**

| Current location in components.rs | Destination |
|-----------------------------------|-------------|
| `visible_width`, `truncate_width`, `wrap_width`, `wrap_width_with_indices` | `primitive/text_layout.rs` |

**IMPORTANT — conflict resolution:** Both `ansi.rs` and `components.rs` export a function named `visible_width`. Check if they are the same function or different. If `components::visible_width` delegates to `ansi::visible_width`, keep one copy. If they differ, rename one (e.g., `visible_width_plain` vs `visible_width_styled`). The implementer must verify by reading both functions.

- [ ] **Step 1: Read ansi.rs and components.rs fully**

Read both files to understand exact function signatures and any name conflicts.

```bash
wc -l crates/neo-tui/src/ansi.rs crates/neo-tui/src/components.rs
```

- [ ] **Step 2: Create primitive/ directory and move core/ files**

```bash
mkdir -p crates/neo-tui/src/primitive
cp crates/neo-tui/src/core/line.rs crates/neo-tui/src/primitive/line.rs
cp crates/neo-tui/src/core/text.rs crates/neo-tui/src/primitive/text.rs
cp crates/neo-tui/src/core/component.rs crates/neo-tui/src/primitive/component.rs
cp crates/neo-tui/src/core/container.rs crates/neo-tui/src/primitive/container.rs
```

In each copied file, update internal imports from `use crate::ansi::` to `use crate::primitive::` (for Style, Color, etc.) and `use crate::components::` to `use crate::primitive::text_layout::`.

Specifically:
- `primitive/line.rs`: change `use crate::ansi::{Style, clip_plain_to_width, paint, strip_ansi, visible_width}` to `use crate::primitive::style::Style; use crate::primitive::ansi_escape::{paint, strip_ansi}; use crate::primitive::text_layout::{clip_plain_to_width, visible_width};`
- `primitive/text.rs`: change `use crate::ansi::{Style, display_width}` to `use crate::primitive::style::Style; use crate::primitive::text_layout::display_width;`
- `primitive/component.rs`: change `use crate::input::InputEvent;` — this stays the same (input module path unchanged).

- [ ] **Step 3: Create primitive/color.rs**

Extract the `Color` enum and all its impl blocks from `ansi.rs`. Include `use crate::primitive::style::Style;` if Color methods reference Style.

- [ ] **Step 4: Create primitive/style.rs**

Extract `Style` struct + impls, `Rect` struct, and `RESET` const from `ansi.rs`.

- [ ] **Step 5: Create primitive/ansi_escape.rs**

Extract these functions from `ansi.rs`:
- `fg_to_ansi`, `bg_to_ansi`, `style_to_ansi`
- `paint`
- `strip_ansi`
- `next_sequence`
- `update_active_sgr`

Add necessary imports: `use crate::primitive::color::Color; use crate::primitive::style::{Style, RESET};`

- [ ] **Step 6: Create primitive/text_layout.rs**

Merge text layout functions from both `ansi.rs` and `components.rs`:

From `ansi.rs`: `visible_width`, `display_width`, `wrap_text`, `pad_to_width`, `truncate_to_width`, `clip_plain_to_width`, `clip_visible_to_width`

From `components.rs`: `truncate_width`, `wrap_width`, `wrap_width_with_indices`

Resolve the `visible_width` name conflict (both files export it — determine if identical or rename).

Add necessary imports from within `primitive/`.

- [ ] **Step 7: Create primitive/mod.rs**

```rust
//! Rendering primitives: color, style, text layout, and component traits.
//!
//! This module consolidates the foundational types and functions used across
//! the entire TUI: Color and Style value types, ANSI escape sequence builders,
//! text measurement/wrapping utilities, and the Component trait hierarchy.

pub mod ansi_escape;
pub mod color;
pub mod component;
pub mod container;
pub mod line;
pub mod style;
pub mod text;
pub mod text_layout;

// Flat re-exports for ergonomic imports
pub use ansi_escape::{
    bg_to_ansi, fg_to_ansi, next_sequence, paint, strip_ansi, style_to_ansi,
    update_active_sgr,
};
pub use color::Color;
pub use component::{Component, Expandable, Finalization, InputResult};
pub use container::{Container, GutterContainer};
pub use line::{Line, Span};
pub use style::{Rect, RESET, Style};
pub use text::Text;
pub use text_layout::{
    clip_plain_to_width, clip_visible_to_width, display_width, pad_to_width,
    truncate_to_width, truncate_width, visible_width, wrap_text, wrap_width,
    wrap_width_with_indices,
};
```

- [ ] **Step 8: Update lib.rs**

In `crates/neo-tui/src/lib.rs`:
- Add `pub mod primitive;`
- Remove `pub mod ansi;`
- Remove `pub mod components;`
- Remove `pub mod core;`

- [ ] **Step 9: Bulk-update all intra-crate imports**

Replace all three old paths with the new unified path:

```bash
# Replace crate::core:: → crate::primitive::
find crates/neo-tui/src -name '*.rs' -exec sed -i '' 's/crate::core::/crate::primitive::/g' {} +

# Replace crate::components:: → crate::primitive::
find crates/neo-tui/src -name '*.rs' -exec sed -i '' 's/crate::components::/crate::primitive::/g' {} +

# Replace crate::ansi:: → crate::primitive::
find crates/neo-tui/src -name '*.rs' -exec sed -i '' 's/crate::ansi::/crate::primitive::/g' {} +
```

Note: `sed` on macOS requires `-i ''` (empty string for in-place without backup).

- [ ] **Step 10: Update cross-crate imports in neo-agent**

In `crates/neo-agent/src/themes.rs`:
```rust
# Line 10: change
use neo_tui::ansi::Color;
# to:
use neo_tui::primitive::Color;

# Line 11: change
use neo_tui::chrome::TuiTheme;
# This stays the same until Task 9 (chrome → shell rename)
```

In `crates/neo-agent/src/modes/interactive.rs`:
```bash
# Replace all neo_tui::ansi:: with neo_tui::primitive::
sed -i '' 's/neo_tui::ansi::/neo_tui::primitive::/g' crates/neo-agent/src/modes/interactive.rs
```

- [ ] **Step 11: Delete old files**

```bash
rm crates/neo-tui/src/ansi.rs
rm crates/neo-tui/src/components.rs
rm crates/neo-tui/src/core/mod.rs
# core/line.rs, core/text.rs, core/component.rs, core/container.rs already copied to primitive/
rm -rf crates/neo-tui/src/core/
```

- [ ] **Step 12: Fix any compilation errors**

Run: `cargo check -p neo-tui 2>&1 | head -50`

Common issues:
- Name conflicts in text_layout.rs (duplicate `visible_width`) — resolve by keeping the more complete version.
- Missing `use` statements in extracted files — add as needed.
- Test code referencing old paths — the sed should have caught them, but verify.

Fix errors iteratively until `cargo check -p neo-tui` passes.

- [ ] **Step 13: Full verification**

Expected: Zero errors, all tests pass, formatting clean.

- [ ] **Step 14: Commit**

```bash
git add -A && git commit -m "refactor(neo-tui): consolidate core/ansi/components into primitive/

Merges three scattered primitive modules into one unified primitive/ directory:
- core/ (Line, Span, Text, Component trait, Container)
- ansi.rs (Color, Style, ANSI escape functions, text layout)
- components.rs (text width/wrap utilities)

Resolves the visible_width name conflict and eliminates the misleading
'components' name (they are text layout functions, not UI components)."
```

---

## Phase 3: Module Splits

### Task 7: Split `input.rs` into `input/` directory

`input.rs` (1186 lines) contains 4 distinct concerns. Convert from file module to directory module.

**Files:**
- Convert: `crates/neo-tui/src/input.rs` → `crates/neo-tui/src/input/mod.rs` (slimmed)
- Create: `crates/neo-tui/src/input/key_id.rs` — KeyId, KeyIdError
- Create: `crates/neo-tui/src/input/keybinding.rs` — KeybindingAction (174-line enum), KeybindingDefinition, KeybindingConflict, KeybindingsManager
- Keep: `crates/neo-tui/src/input/raw_input.rs` — unchanged

Import sites: 14 intra-crate, 0 cross-crate. All import `crate::input::{InputEvent, KeybindingAction, KeyId}`.

- [ ] **Step 1: Convert input.rs to directory**

```bash
mkdir -p crates/neo-tui/src/input_tmp
mv crates/neo-tui/src/input.rs crates/neo-tui/src/input_tmp/mod.rs
# raw_input.rs is already at crates/neo-tui/src/input/raw_input.rs
# Move the temp mod.rs to the right place
mv crates/neo-tui/src/input_tmp/mod.rs crates/neo-tui/src/input/mod.rs
rmdir crates/neo-tui/src/input_tmp
```

- [ ] **Step 2: Create input/key_id.rs**

Extract from `input/mod.rs` lines 216–262:
- `pub struct KeyId(String)` and its impl block
- `pub struct KeyIdError` and its impls (Display, Error)
- `impl fmt::Display for KeyId`

Move to `crates/neo-tui/src/input/key_id.rs`:

```rust
use std::fmt;

pub struct KeyId(String);

impl KeyId {
    pub fn new(value: impl Into<String>) -> Result<Self, KeyIdError> {
        // ... existing implementation
    }
    pub fn as_str(&self) -> &str { ... }
    pub fn is_text_insertion_key(&self) -> bool { ... }
}

impl fmt::Display for KeyId { ... }

pub struct KeyIdError { ... }
impl fmt::Display for KeyIdError { ... }
impl std::error::Error for KeyIdError {}
```

- [ ] **Step 3: Create input/keybinding.rs**

Extract from `input/mod.rs` lines 265–end:
- `pub enum KeybindingAction` (174-line enum) + its impl
- `pub struct KeybindingDefinition`
- `pub struct KeybindingConflict`
- `pub struct KeybindingsManager` + `impl Default` + `impl KeybindingsManager`

Move to `crates/neo-tui/src/input/keybinding.rs`. Add `use super::key_id::KeyId;` if needed.

- [ ] **Step 4: Slim down input/mod.rs**

`crates/neo-tui/src/input/mod.rs` should contain only:
- `pub enum InputEvent` (lines 15–34)
- `pub struct InputParser` + impl (lines 36–94)
- Module declarations and re-exports:

```rust
pub mod key_id;
pub mod keybinding;
pub mod raw_input;

pub use key_id::{KeyId, KeyIdError};
pub use keybinding::{
    KeybindingAction, KeybindingConflict, KeybindingDefinition, KeybindingsManager,
};
pub use raw_input::{
    RawEvent, RawInputParser, decode_printable_key, is_key_release, is_key_repeat,
    is_kitty_protocol_active, matches_key, parse_key, set_kitty_protocol_active,
};
```

The InputParser may reference KeybindingAction and KeybindingsManager — update those references from local to `use super::keybinding::...`.

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p neo-tui`
Expected: Zero errors. All existing `crate::input::InputEvent`, `crate::input::KeybindingAction`, `crate::input::KeyId` imports still work because `mod.rs` re-exports them.

- [ ] **Step 6: Full verification**

Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "refactor(neo-tui): split input.rs into input/ directory

Extracts KeyId/KeyIdError into key_id.rs and KeybindingAction/KeybindingsManager
into keybinding.rs, leaving InputEvent/InputParser in mod.rs."
```

---

### Task 8: Split `image.rs` → `terminal_image/` directory

`image.rs` (849 lines) contains 3 independent terminal image protocol encoders plus negotiation logic.

**Files:**
- Convert: `crates/neo-tui/src/image.rs` → `crates/neo-tui/src/terminal_image/mod.rs` (negotiation + types)
- Create: `crates/neo-tui/src/terminal_image/kitty.rs` — Kitty Graphics protocol
- Create: `crates/neo-tui/src/terminal_image/iterm2.rs` — iTerm2 inline image protocol
- Create: `crates/neo-tui/src/terminal_image/sixel.rs` — Sixel protocol
- Modify: `crates/neo-tui/src/lib.rs`
- Modify: 2 import sites

Import sites (intra-crate only):
- `transcript/entry.rs:5` — `use crate::image::{ImageRenderPolicy, ImageSource, InlineImage, TerminalImageCapabilities};`
- `transcript/pane.rs:18` — same import

These will become `crate::terminal_image::` after the sed replacement.

- [ ] **Step 1: Create directory and move main file**

```bash
mkdir -p crates/neo-tui/src/terminal_image
mv crates/neo-tui/src/image.rs crates/neo-tui/src/terminal_image/mod.rs
```

- [ ] **Step 2: Identify the three encoder functions and their helpers**

Read `crates/neo-tui/src/terminal_image/mod.rs` and identify:
- `encode_kitty_graphics` and its helper types (KittyGraphicsOptions, KittyImageFormat)
- `encode_iterm2_inline_image` and its helper types (Iterm2InlineImageOptions, Iterm2Dimension)
- `encode_sixel_image` and its helper types (SixelPaletteColor, SixelImageOptions)

Keep in `mod.rs`: `ImageProtocolPreference`, `NegotiatedImageProtocol`, `ImageSource`, `ImageProtocolError`, `TerminalImageCapabilities`, `ImageRenderPolicy`, `RenderedInlineImage`, `InlineImage`.

- [ ] **Step 3: Create terminal_image/kitty.rs**

Cut `encode_kitty_graphics` and all Kitty-specific types from `mod.rs`. Paste into `crates/neo-tui/src/terminal_image/kitty.rs`. Add necessary `use super::*;` or specific imports.

- [ ] **Step 4: Create terminal_image/iterm2.rs**

Cut `encode_iterm2_inline_image` and iTerm2-specific types. Paste into `crates/neo-tui/src/terminal_image/iterm2.rs`.

- [ ] **Step 5: Create terminal_image/sixel.rs**

Cut `encode_sixel_image` and Sixel-specific types. Paste into `crates/neo-tui/src/terminal_image/sixel.rs`.

- [ ] **Step 6: Add module declarations to terminal_image/mod.rs**

At the top of `crates/neo-tui/src/terminal_image/mod.rs`, add:

```rust
pub mod kitty;
pub mod iterm2;
pub mod sixel;

pub use kitty::encode_kitty_graphics;
pub use iterm2::encode_iterm2_inline_image;
pub use sixel::encode_sixel_image;
```

- [ ] **Step 7: Update lib.rs**

In `crates/neo-tui/src/lib.rs`, change `pub mod image;` to `pub mod terminal_image;`.

- [ ] **Step 8: Update import sites**

```bash
find crates/neo-tui/src -name '*.rs' -exec sed -i '' 's/crate::image::/crate::terminal_image::/g' {} +
```

- [ ] **Step 9: Verify**

Expected: Zero errors, all tests pass.

- [ ] **Step 10: Commit**

```bash
git add -A && git commit -m "refactor(neo-tui): split image.rs → terminal_image/ directory

Extracts Kitty, iTerm2, and Sixel protocol encoders into separate files.
Renames module from 'image' to 'terminal_image' for clarity."
```

---

## Phase 4: Chrome Decomposition

### Task 9: Split `chrome.rs` (4508 lines) → `shell/` directory

This is the largest task. `chrome.rs` contains at least 10 independent concerns. The approach: create the directory, extract each concern into its own file, keep `NeoChromeState` in `mod.rs` with its full impl.

**Files:**
- Convert: `crates/neo-tui/src/chrome.rs` → `crates/neo-tui/src/shell/mod.rs`
- Create: `crates/neo-tui/src/shell/theme.rs`
- Create: `crates/neo-tui/src/shell/overlay.rs`
- Create: `crates/neo-tui/src/shell/prompt.rs`
- Create: `crates/neo-tui/src/shell/approval.rs`
- Create: `crates/neo-tui/src/shell/pickers.rs`
- Create: `crates/neo-tui/src/shell/command_palette.rs`
- Create: `crates/neo-tui/src/shell/select_list.rs`
- Create: `crates/neo-tui/src/shell/session_picker.rs`
- Create: `crates/neo-tui/src/shell/context.rs`
- Create: `crates/neo-tui/src/shell/stream.rs`
- Create: `crates/neo-tui/src/shell/image_cache.rs`
- Create: `crates/neo-tui/src/shell/pending_input.rs`
- Create: `crates/neo-tui/src/shell/dialog_dispatch.rs`
- Modify: `crates/neo-tui/src/lib.rs`
- Modify: ALL files importing `crate::chrome::` (23 files intra-crate)
- Modify: `crates/neo-agent/src/themes.rs` (1 cross-crate import)
- Modify: `crates/neo-agent/src/modes/interactive.rs` (2 cross-crate imports)

**Extraction map (line ranges from chrome.rs):**

| File | Types/functions | Source lines |
|------|----------------|-------------|
| `theme.rs` | `TuiTheme` + Default + impl, `ChromeMode`, `DevelopmentMode`, `GoalModeStatus`, free fns `format_token_count`, `review_title`, `plan_review_options` | 138–456 |
| `context.rs` | `ContextWindow` + impl | 388–456 |
| `stream.rs` | `StreamUpdate` enum, `ToolStatusKind` + impl | 1947–1995, 3414–3432 |
| `overlay.rs` | `OverlayId`, `Overlay` + impl, `OverlayKind` + impl, `OverlayListSelection` + impl | 1998–2252 |
| `dialog_dispatch.rs` | `DialogInputRef` trait, `DialogInputOwned` trait, all impls, `handle_dialog_selection` and friends, `handle_input_ref`, `handle_input_owned` | 2254–2384 |
| `session_picker.rs` | `SessionPickerScope`, `SessionPickerItem` + impl, `SessionPickerState` + impl, private helpers (`format_relative_time`, `single_line`, `home_alias`, `truncate_plain_to_width`, `truncate_styled_to_width`, `truncate_left`) | 2370–2789, 2791–2892 |
| `pickers.rs` | `PromptCompletionPrefix`, `PromptCompletionState` + impl, `PickerItem` + impl, `From<PickerItem> for SelectItem`, `PickerState` + impl, `ModelPickerState` alias | 2903–3060, 4008–4012 |
| `command_palette.rs` | `CommandSpec` + impl, `CommandPaletteState` + impl, `picker_from_select_item`, `select_from_command`, `command_from_select_item` | 3061–3148 |
| `approval.rs` | `ApprovalChoice`, `ApprovalOption` + impl, `ApprovalModal` + impl, `ApprovalRequestModal` + impl, `ApprovalResult`, free fn `approval_number` | 3149–3317, 4097–4157, 1936 |
| `image_cache.rs` | `InlineImageRenderCache` + impl | 3318–3347 |
| `pending_input.rs` | `PendingInputState` + impl | 3350–3410 |
| `select_list.rs` | `SelectItem` + impl, `SelectListState` + impl, `VisibleSelectItem`, `render_select_item`, `select_item_matches` | 4160–4359 |
| `prompt.rs` | `PromptState` + impl, `PromptSnapshot`, `DeleteDirection`, `PromptEdit`, free fns (`prompt_grapheme_width`, `wrap_prompt_lines`, `char_index_at_visual_col`, `visual_col_at_char_index`, `find_word_backward`, `find_word_forward`, `is_word_like`) | 3435–4007, 4015–4094, 33–124 |
| `mod.rs` | `NeoChromeState` struct + impl (the god-object core), module re-exports | 459–1935, module decls |

- [ ] **Step 1: Create shell/ directory**

```bash
mkdir -p crates/neo-tui/src/shell
```

- [ ] **Step 2: Extract theme.rs**

Create `crates/neo-tui/src/shell/theme.rs` with:
- `TuiTheme` struct (lines 138–175)
- `impl Default for TuiTheme` (lines 177–220)
- `impl TuiTheme` (lines 222–360)
- `pub enum ChromeMode` (lines 363–368)
- `pub enum DevelopmentMode` (lines 371–376)
- `pub enum GoalModeStatus` (lines 379–385)
- Free functions: `format_token_count` (417), `review_title` (427), `plan_review_options` (439)
- `use crate::primitive::Color;` (was `use crate::ansi::Color;`, already updated by Task 6)

- [ ] **Step 3: Extract context.rs**

Create `crates/neo-tui/src/shell/context.rs` with:
- `pub struct ContextWindow` (lines 388–391)
- `impl ContextWindow` (lines 393–415)

- [ ] **Step 4: Extract stream.rs**

Create `crates/neo-tui/src/shell/stream.rs` with:
- `pub enum StreamUpdate` (lines 1947–1995) — includes all 15 variants
- `pub enum ToolStatusKind` (lines 3414–3432) + impl (label, marker)
- Any imports needed (AgentEvent types, etc.)

- [ ] **Step 5: Extract select_list.rs**

Create `crates/neo-tui/src/shell/select_list.rs` with:
- `pub struct SelectItem` (4160) + impl
- `pub struct SelectListState` (4182) + impl (4195–4313)
- `pub struct VisibleSelectItem<'a>` (4190)
- Free functions: `render_select_item` (4314), `select_item_matches` (4347)
- `use crate::primitive::{Color, Style, paint, visible_width};` as needed

- [ ] **Step 6: Extract pickers.rs**

Create `crates/neo-tui/src/shell/pickers.rs` with:
- `pub struct PromptCompletionPrefix` (4008)
- `pub struct PromptCompletionState` (2903) + impl (2908–2955)
- `pub struct PickerItem` (2958) + impl
- `impl From<PickerItem> for SelectItem` (2979)
- `pub struct PickerState` (2986) + impl (2990–3058)
- `pub type ModelPickerState = PickerState;` (2367)
- `use super::select_list::{SelectItem, SelectListState, VisibleSelectItem};`

- [ ] **Step 7: Extract command_palette.rs**

Create `crates/neo-tui/src/shell/command_palette.rs` with:
- `pub struct CommandSpec` (3061) + impl
- `pub struct CommandPaletteState` (3083) + impl (3087–3146)
- Free functions: `picker_from_select_item` (3052), `select_from_command` (3136), `command_from_select_item` (3140)
- `use super::select_list::{SelectItem, SelectListState};`
- `use super::pickers::PickerItem;`

- [ ] **Step 8: Extract approval.rs**

Create `crates/neo-tui/src/shell/approval.rs` with:
- `pub enum ApprovalChoice` (4097)
- `pub struct ApprovalOption` (4107) + impl
- `pub struct ApprovalModal` (4123) + impl
- `pub struct ApprovalRequestModal` (3149) + impl (3160–3298)
- `pub struct ApprovalResult` (3301)
- Free function: `approval_number` (1936)
- `use super::theme::TuiTheme;`

- [ ] **Step 9: Extract session_picker.rs**

Create `crates/neo-tui/src/shell/session_picker.rs` with:
- `pub enum SessionPickerScope` (2370)
- `pub struct SessionPickerItem` (2376) + impl (2385)
- `pub struct SessionPickerState` (2407) + impl (2417–2789)
- Private helpers: `format_relative_time` (2791), `single_line` (2813), `home_alias` (2821), `truncate_plain_to_width` (2835), `truncate_styled_to_width` (2854), `truncate_left` (2873)
- `use crate::primitive::{Color, Style, paint, strip_ansi, visible_width};`

- [ ] **Step 10: Extract image_cache.rs**

Create `crates/neo-tui/src/shell/image_cache.rs` with:
- `pub struct InlineImageRenderCache` (3318) + impl (3322–3347)
- `use std::collections::BTreeMap;`
- `use crate::transcript::InlineImageRender;`

- [ ] **Step 11: Extract pending_input.rs**

Create `crates/neo-tui/src/shell/pending_input.rs` with:
- `pub struct PendingInputState` (3350) + impl (3357–3410)
- `use std::collections::VecDeque;`

- [ ] **Step 12: Extract prompt.rs**

Create `crates/neo-tui/src/shell/prompt.rs` with:
- Free functions: `prompt_grapheme_width` (33), `wrap_prompt_lines` (44), `char_index_at_visual_col` (104), `visual_col_at_char_index` (124), `find_word_backward` (4046), `find_word_forward` (4069), `is_word_like` (4092)
- `struct PromptSnapshot` (3450) — private
- `pub struct PromptState` (3435) + impl (3455–4007)
- `enum DeleteDirection` (4015) — private
- `pub enum PromptEdit<'a>` (4021)
- `pub struct PromptCompletionPrefix` (4008) — if not already in pickers.rs (decide: put in prompt.rs since it's prompt-related, then pickers.rs imports it)
- `use crate::primitive::{Color, Style, paint, visible_width, wrap_width};`

Note: If `PromptCompletionPrefix` goes to `prompt.rs`, update `pickers.rs` to import it: `use super::prompt::PromptCompletionPrefix;`.

- [ ] **Step 13: Extract overlay.rs**

Create `crates/neo-tui/src/shell/overlay.rs` with:
- `pub struct OverlayId(u64)` (1998) + impl (2000–2005)
- `pub struct Overlay` (2008) + impl (2014–2087)
- `pub enum OverlayKind` (2091) + impl (2112–2207)
- `enum OverlayListSelection<'a>` (2209) + impl (2216–2252)
- Imports from sibling modules:
```rust
use super::approval::ApprovalRequestModal;
use super::command_palette::CommandPaletteState;
use super::pickers::{PickerState, PromptCompletionState};
use super::session_picker::SessionPickerState;
use super::select_list::VisibleSelectItem;
use crate::dialogs::{...};
use crate::widgets::QuestionStateMachine;
```

- [ ] **Step 14: Extract dialog_dispatch.rs**

Create `crates/neo-tui/src/shell/dialog_dispatch.rs` with:
- `trait DialogInputRef` (2305) — private trait
- `trait DialogInputOwned` (2308) — private trait
- All impl blocks (2313–2384)
- Free functions: `handle_dialog_selection` (2254), `handle_selector_dialog_selection`, `handle_model_dialog_selection`, `handle_provider_choice_dialog_selection`, `handle_input_dialog_selection`, `handle_input_ref` (2297), `handle_input_owned` (2300)
- `use super::overlay::OverlayKind;`
- `use crate::primitive::InputResult;`
- `use crate::input::{InputEvent, KeybindingAction, KeyId};`

- [ ] **Step 15: Create shell/mod.rs with NeoChromeState**

The remaining content from `chrome.rs` (lines 459–1935, plus module-level `use` statements and the `MAX_PROMPT_VISIBLE_LINES` const) goes into `crates/neo-tui/src/shell/mod.rs`.

```rust
//! Application shell state: prompt editing, overlay management, approval flow,
//! theme, session picker, command palette, and all interactive UI state.

mod approval;
mod command_palette;
mod context;
mod dialog_dispatch;
mod image_cache;
mod overlay;
mod pending_input;
mod pickers;
mod prompt;
mod select_list;
mod session_picker;
mod stream;
mod theme;

// Public re-exports
pub use approval::{ApprovalChoice, ApprovalModal, ApprovalOption, ApprovalRequestModal, ApprovalResult};
pub use command_palette::{CommandPaletteState, CommandSpec};
pub use context::ContextWindow;
pub use image_cache::InlineImageRenderCache;
pub use overlay::{Overlay, OverlayId, OverlayKind};
pub use pending_input::PendingInputState;
pub use pickers::{ModelPickerState, PickerItem, PickerState, PromptCompletionState, PromptCompletionPrefix};
pub use prompt::{PromptEdit, PromptState};
pub use select_list::{SelectItem, SelectListState, VisibleSelectItem};
pub use session_picker::{SessionPickerItem, SessionPickerScope, SessionPickerState};
pub use stream::{StreamUpdate, ToolStatusKind};
pub use theme::{ChromeMode, DevelopmentMode, GoalModeStatus, TuiTheme};

// NeoChromeState struct + impl (the central application state)
// ... (lines 459–1935 from original chrome.rs, with updated imports)
```

The `mod.rs` will be approximately 1500 lines (the NeoChromeState impl). This is the integration hub and cannot easily be split further without fracturing the impl into extension-style blocks (possible future work).

- [ ] **Step 16: Update NeoChromeState imports in mod.rs**

The new `shell/mod.rs` needs updated imports pointing to sibling modules:

```rust
use std::collections::{BTreeMap, VecDeque};
use std::ops::Range;
use std::path::{Path, PathBuf};

use neo_agent_core::{AgentEvent, PermissionMode, PermissionOperation};
use unicode_segmentation::UnicodeSegmentation;

use crate::primitive::{Color, InputResult, truncate_width, visible_width};
use crate::dialogs::{
    ApiKeyInputState, ChoicePickerState, CustomRegistryImportState,
    McpAddFormState, McpManagerState, ModelSelectorState, ProviderManagerState,
    TabbedModelSelectorState, TextInputState,
};
use crate::terminal_image::{ImageRenderPolicy, TerminalImageCapabilities};
use crate::input::{InputEvent, KeybindingAction};
use crate::widgets::{TodoDisplayItem, TodoDisplayStatus};
// Question types now come from dialogs after Task 5:
use crate::dialogs::{
    QuestionDialogAction, QuestionDisplayData, QuestionDisplayOption,
    QuestionResult, QuestionStateMachine,
};

use self::approval::*;
use self::dialog_dispatch::*;
use self::overlay::*;
use self::pickers::*;
use self::prompt::*;
use self::session_picker::*;
use self::stream::*;
```

- [ ] **Step 17: Update lib.rs**

In `crates/neo-tui/src/lib.rs`, change `pub mod chrome;` to `pub mod shell;`.

- [ ] **Step 18: Bulk-update intra-crate imports**

```bash
find crates/neo-tui/src -name '*.rs' -exec sed -i '' 's/crate::chrome::/crate::shell::/g' {} +
```

This updates all 23 files that import `crate::chrome::{TuiTheme}`, `crate::chrome::{NeoChromeState}`, etc.

- [ ] **Step 19: Update cross-crate imports in neo-agent**

In `crates/neo-agent/src/themes.rs` line 11:
```rust
# Change:
use neo_tui::chrome::TuiTheme;
# To:
use neo_tui::shell::TuiTheme;
```

In `crates/neo-agent/src/modes/interactive.rs`:
```bash
sed -i '' 's/neo_tui::chrome::/neo_tui::shell::/g' crates/neo-agent/src/modes/interactive.rs
```

This updates `ChromeMode::Streaming` and `PickerState` references.

- [ ] **Step 20: Delete old chrome.rs**

```bash
rm crates/neo-tui/src/chrome.rs
```

- [ ] **Step 21: Fix compilation errors**

Run: `cargo check -p neo-tui 2>&1 | head -80`

Common issues:
- Private items referenced across sibling modules — make them `pub(crate)` or `pub(super)`.
- `PromptCompletionPrefix` placement — if in `prompt.rs`, ensure `pickers.rs` imports it.
- Test module at bottom of old chrome.rs — move tests to `shell/mod.rs` or to respective extracted files.
- Missing trait implementations — `DialogInputRef`/`DialogInputOwned` are private traits in `dialog_dispatch.rs`. If `NeoChromeState` methods call dispatch functions, ensure they're `pub(super)`.

Fix iteratively.

- [ ] **Step 22: Full verification**

Expected: Zero errors, all tests pass.

- [ ] **Step 23: Commit**

```bash
git add -A && git commit -m "refactor(neo-tui): decompose chrome.rs (4508 lines) → shell/ directory

Splits the monolithic chrome.rs into 14 focused files:
- theme.rs: TuiTheme, ChromeMode, DevelopmentMode
- overlay.rs: Overlay, OverlayId, OverlayKind
- prompt.rs: PromptState, PromptEdit (657-line impl)
- approval.rs: ApprovalRequestModal, ApprovalModal
- pickers.rs: PickerState, PromptCompletionState
- command_palette.rs: CommandSpec, CommandPaletteState
- select_list.rs: SelectListState (foundation widget)
- session_picker.rs: SessionPickerState (491-line impl)
- context.rs: ContextWindow
- stream.rs: StreamUpdate, ToolStatusKind
- image_cache.rs: InlineImageRenderCache
- pending_input.rs: PendingInputState
- dialog_dispatch.rs: DialogInputRef/Owned traits + dispatch fns
- mod.rs: NeoChromeState struct + impl (integration hub)

Renames module from 'chrome' (jargon) to 'shell' (application shell state)."
```

---

## Phase 5: Transcript Event Handler Extraction

### Task 10: Extract `apply_agent_event` from `transcript/pane.rs`

`TranscriptPane::apply_agent_event` (lines 501–937) is a 436-line event router that converts `AgentEvent` values into transcript entries. This is event routing logic, not rendering logic.

**Files:**
- Modify: `crates/neo-tui/src/transcript/pane.rs` — remove `apply_agent_event` method
- Create: `crates/neo-tui/src/transcript/event_handler.rs` — standalone function or extension impl
- Modify: `crates/neo-tui/src/transcript/mod.rs`

The method is called as `pane.apply_agent_event(event)` from `neo-agent/src/modes/interactive.rs`. To maintain the same call site API, use an extension trait or keep the method on `TranscriptPane` but move the implementation into a separate file via Rust's multi-impl capability.

- [ ] **Step 1: Read the method signature and body**

Read `crates/neo-tui/src/transcript/pane.rs` lines 501–937 to understand:
- The generic parameter `<E>` and its bounds
- All transcript entry types it constructs
- Any private fields/methods of `TranscriptPane` it accesses

- [ ] **Step 2: Create event_handler.rs with extension impl**

Create `crates/neo-tui/src/transcript/event_handler.rs`:

```rust
use neo_agent_core::AgentEvent;
// ... other imports

use super::pane::TranscriptPane;
// ... transcript entry imports

impl TranscriptPane {
    pub fn apply_agent_event<E>(&mut self, event: E)
    where
        E: Into<AgentEvent>,
    {
        // ... the full 436-line body, unchanged
    }
}
```

Note: Rust allows `impl` blocks for a type in any module within the same crate. The method stays `pub` and callable as `pane.apply_agent_event(event)` — no API change.

Move the private helper methods that are only used by `apply_agent_event` (if any) into this file as well. Methods used by both `apply_agent_event` and other pane methods stay in `pane.rs`.

- [ ] **Step 3: Add module declaration**

In `crates/neo-tui/src/transcript/mod.rs`, add:

```rust
mod event_handler;
```

(Make it private — the method is exposed through `TranscriptPane`'s public API.)

- [ ] **Step 4: Remove the method from pane.rs**

Delete lines 501–937 (the `apply_agent_event` method) from `crates/neo-tui/src/transcript/pane.rs`. Also remove any now-unused imports.

- [ ] **Step 5: Verify**

Expected: Zero errors, all tests pass. No API changes.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(neo-tui): extract apply_agent_event from pane.rs (436 lines)

Moves the event routing logic out of the rendering file into
transcript/event_handler.rs. TranscriptPane::apply_agent_event stays
public with the same signature — only the impl location changes."
```

---

## Post-Refactoring Verification

- [ ] **Final verification: full workspace check**

```bash
cargo check --workspace --all-features
cargo check -p neo-agent
cargo fmt --all --check
```

- [ ] **Verify no old module paths remain**

```bash
# These should all return zero matches:
grep -rn 'crate::chrome::' crates/neo-tui/src/
grep -rn 'crate::ansi::' crates/neo-tui/src/
grep -rn 'crate::components::' crates/neo-tui/src/
grep -rn 'crate::core::' crates/neo-tui/src/
grep -rn 'crate::terminal::' crates/neo-tui/src/
grep -rn 'crate::image::' crates/neo-tui/src/
grep -rn 'crate::tool_diff::' crates/neo-tui/src/
grep -rn 'crate::neo_tui::' crates/neo-tui/src/
grep -rn 'neo_tui::chrome::' crates/neo-agent/src/
grep -rn 'neo_tui::ansi::' crates/neo-agent/src/
```

- [ ] **Verify file sizes are reasonable**

```bash
find crates/neo-tui/src -name '*.rs' -exec wc -l {} \; | sort -rn | head -20
```

The largest file should be `shell/mod.rs` (NeoChromeState impl, ~1500 lines) or `transcript/pane.rs` (reduced from 2174 to ~1700 lines). No file should exceed 2000 lines.

- [ ] **Final commit**

```bash
git add -A && git commit -m "refactor(neo-tui): complete code organization restructure

Before: 16 flat modules with a 4508-line monolith (chrome.rs)
After:  clean hierarchical structure with self-descriptive names

Key changes:
- terminal/ → screen_output/ (NOT a terminal emulator)
- chrome.rs → shell/ (14 focused files, was 4508-line monolith)
- core/ + ansi.rs + components.rs → primitive/ (unified rendering primitives)
- image.rs → terminal_image/ (3 protocol encoders separated)
- input.rs → input/ directory (4 concerns separated)
- neo_tui.rs → app.rs (avoid name stutter)
- tool_diff.rs → diff_model.rs (not tool-specific)
- question_dialog moved from widgets/ to dialogs/
- apply_agent_event extracted from transcript/pane.rs"
```
