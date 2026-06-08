# neo-tui Gap Summary

Reference: `pi/packages/tui` TypeScript package.
Target: `crates/tui`.

## Implemented high-priority parity

- Prompt editor primitives now support insert, backspace/delete, character movement, word movement, word deletion, and delete-to-line-start/end.
- Keybinding helpers now expose normalized key IDs, default TUI action mappings, user override resolution, and conflict detection.
- Transcript state now has a scrollback-aware viewport model with bottom-follow
  behavior, and the live TUI render path applies that viewport.
- Selection/list primitives now support prefix filtering, wrap-around selection, centered visible windows, scroll indicators, and width-safe line rendering.
- Rendering helpers now expose ANSI/OSC escape-aware terminal visible-width
  measurement, width-safe truncation with optional padding, and wrapping that
  preserves blank lines, splits long tokens, and does not split complete escape
  sequences. Wrapped continuation lines rehydrate active SGR styles so terminal
  color/modifier state survives line boundaries.
- Transcript rendering now classifies unified diff blocks (`---`, `+++`, `@@`,
  context, added, and removed lines), wraps them width-safely, and renders diff
  additions/removals/context/hunks with distinct terminal styles in the
  transcript widget.
- Rendering tests cover prompt, transcript, status, modal, select-list, truncation, wrapping, and keybinding behavior.
- `neo-agent` now owns a crossterm/raw-mode interactive loop slice that renders
  `NeoTuiApp` in a real terminal and uses `neo-tui` input events for text input,
  Enter submit, terminal resize redraws, and Esc/Ctrl-C exit.
- The live interactive loop now dispatches default `KeybindingsManager` actions
  into real prompt and overlay primitives for word movement, word deletion,
  delete-to-line-start/end, submit/newline, approval selection up/down,
  overlay page-up/page-down selection, approval confirm, overlay cancel, exit
  cancel, filesystem-backed prompt Tab completion with literal-tab fallback,
  prompt undo, kill-ring yank, an internal prompt copy buffer, and live
  prompt-text copy to the OS clipboard with internal-buffer fallback when the
  system clipboard is unavailable. In editing mode,
  Up/Down/PageUp/PageDown scroll the transcript viewport.
- The live interactive loop opens a local session picker with `ctrl+r`, using
  real `SessionMetadataStore` records prepared by `neo-agent`, orders parent
  and child sessions as a local tree, and can load a selected JSONL transcript
  into the TUI before continuing in that session.
- With the session picker focused, the live interactive loop can fork the
  selected session with `ctrl+n`, load the child JSONL transcript, and continue
  subsequent prompts in the forked session.
- The live interactive loop opens a model picker with `ctrl+o`, using real
  `ModelRegistry` entries prepared by `neo-agent`, and updates the active model
  label after selection.
- Approval overlays in the live loop now resume pending runtime tool calls:
  Approve/deny choices are sent back through `AgentConfig::with_async_approval_handler`,
  and Ctrl-C cancels the active turn, drains cooperative cancelled barriers,
  and only falls back to abort when the runtime task does not finish.
- Streamed assistant thinking events from `neo-agent-core` are reduced into a
  visible TUI notice while final assistant text remains a separate answer item.
- Agent message image content is preserved in the transcript as stable text
  metadata summaries: URL images render their URL, and base64 images render
  MIME type plus payload size instead of raw image bytes.
- Prompt Tab completion is backed by real local project/runtime data. `neo-tui`
  exposes prompt completion prefix/replacement primitives plus a completion
  picker overlay, and `neo-agent` reads matching files/directories from
  `AppConfig.project_dir` plus slash prompt templates from `.neo/prompts/*.md`,
  and provider/model ids from the resolved `ModelRegistry`. It auto-applies a
  longer common prefix, opens the picker for multiple exact-prefix matches,
  preserves literal tab insertion when no completion exists, and treats an
  exact leading `@provider/model` token as a per-turn model override while
  leaving unknown `@...` prompt text intact.
- The live interactive loop enables terminal bracketed paste mode and routes
  both native crossterm paste events and raw bracketed-paste start/end
  sequences through a buffered `InputParser`. Pasted multiline text is inserted
  into the prompt without treating embedded newlines as submit; a subsequent
  Enter still submits normally.
- Transcript item selection is exposed through live keybindings: Ctrl-Space
  starts selection from the visible transcript item, Shift-Up/Down and
  Shift-PageUp/PageDown extend the item range, Ctrl-C copies the selected
  transcript text to the OS clipboard through `neo-agent`, and the TUI keeps
  the same text in its internal copy buffer. Selected items are highlighted in
  the transcript renderer, and ordinary prompt copy still works when no
  transcript selection is active.

## Remaining lower-priority gaps

- The Rust crate does not implement Kitty/Sixel/OSC image protocols, command
  autocomplete beyond local slash prompt templates, or the full TypeScript
  renderer's advanced diff affordances beyond width-safe unified diff line
  classification, coloring, and item-range transcript selection.
- The Rust crate intentionally contains no provider/runtime configuration or execution logic.
