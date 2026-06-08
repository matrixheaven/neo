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
- Rendering tests cover prompt, transcript, status, modal, select-list, truncation, wrapping, and keybinding behavior.
- `neo-agent` now owns a crossterm/raw-mode interactive loop slice that renders
  `NeoTuiApp` in a real terminal and uses `neo-tui` input events for text input,
  Enter submit, terminal resize redraws, and Esc/Ctrl-C exit.
- The live interactive loop now dispatches default `KeybindingsManager` actions
  into real prompt and overlay primitives for word movement, word deletion,
  delete-to-line-start/end, submit/newline, approval selection up/down,
  overlay page-up/page-down selection, approval confirm, overlay cancel, exit
  cancel, filesystem-backed prompt Tab completion with literal-tab fallback,
  prompt undo, kill-ring yank, and an internal prompt copy buffer. In editing
  mode, Up/Down/PageUp/PageDown scroll the transcript viewport.
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
  and Ctrl-C aborts the active turn instead of inventing a decision.
- Prompt Tab completion is backed by the real project filesystem. `neo-tui`
  exposes prompt completion prefix/replacement primitives plus a completion
  picker overlay, and `neo-agent` reads matching files/directories from
  `AppConfig.project_dir`, auto-applies a longer common prefix, opens the
  picker for multiple exact-prefix matches, and preserves literal tab insertion
  when no filesystem completion exists.

## Remaining lower-priority gaps

- The Rust crate does not implement the full TypeScript terminal diff renderer,
  image protocols, richer provider or command autocomplete, stdin buffering, or
  OS/terminal clipboard integration.
- The Rust crate intentionally contains no provider/runtime configuration or execution logic.
