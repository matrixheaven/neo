# neo-tui Gap Summary

Reference: `pi/packages/tui` TypeScript package.
Target: `crates/tui`.

## Implemented high-priority parity

- Prompt editor primitives now support insert, backspace/delete, character movement, word movement, word deletion, and delete-to-line-start/end.
- Keybinding helpers now expose normalized key IDs, default TUI action mappings, user override resolution, and conflict detection.
- Transcript state now has a scrollback-aware viewport model with bottom-follow behavior.
- Selection/list primitives now support prefix filtering, wrap-around selection, centered visible windows, scroll indicators, and width-safe line rendering.
- Rendering helpers now expose terminal visible-width measurement, width-safe truncation with optional padding, and wrapping that preserves blank lines and splits long tokens.
- Rendering tests cover prompt, transcript, status, modal, select-list, truncation, wrapping, and keybinding behavior.
- `neo-agent` now owns a crossterm/raw-mode interactive loop slice that renders
  `NeoTuiApp` in a real terminal and uses `neo-tui` input events for text input,
  Enter submit, terminal resize redraws, and Esc/Ctrl-C exit.
- The live interactive loop now dispatches default `KeybindingsManager` actions
  into real prompt and overlay primitives for word movement, word deletion,
  delete-to-line-start/end, submit/newline, approval selection up/down,
  overlay page-up/page-down selection, approval confirm, overlay cancel, exit
  cancel, tab insertion, prompt undo, kill-ring yank, and an internal prompt
  copy buffer.
- The live interactive loop opens a local session picker with `ctrl+r`, using
  real `SessionMetadataStore` records prepared by `neo-agent`, and can load a
  selected JSONL transcript into the TUI before continuing in that session.
- The live interactive loop opens a model picker with `ctrl+o`, using real
  `ModelRegistry` entries prepared by `neo-agent`, and updates the active model
  label after selection.

## Remaining lower-priority gaps

- The Rust crate does not implement the full TypeScript terminal diff renderer,
  image protocols, ANSI-preserving wrapping, autocomplete, stdin buffering,
  OS/terminal clipboard integration, or tab completion.
- The Rust crate intentionally contains no provider/runtime configuration or execution logic.
