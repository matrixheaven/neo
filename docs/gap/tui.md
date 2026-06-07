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
  Enter submit, and Esc/Ctrl-C exit.

## Remaining lower-priority gaps

- The Rust crate does not implement the full TypeScript terminal diff renderer,
  overlay stack, image protocols, ANSI-preserving wrapping, autocomplete, stdin
  buffering, undo stack, or kill ring.
- The Rust crate intentionally contains no provider/runtime configuration or execution logic.
