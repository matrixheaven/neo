# WaitDelegate Card Implementation Plan

**Goal:** Render the approved WaitDelegate card from existing arguments and
result details without changing runtime behavior.

**TDD Route:** Mode off; post-change regression only. One focused TUI test
covers running, completed, timeout, and not-found rendering.

## Scope

- Modify `crates/neo-tui/src/transcript/tool_renderers.rs` for the specialized
  header and body.
- Modify `crates/neo-tui/src/transcript/tool_call.rs` only to pass the existing
  live elapsed time into the header.
- Modify `crates/neo-tui/tests/tool_cards.rs` for focused card assertions.
- Reuse the existing `ToolCallComponent`, result envelope, row limit, and
  expansion behavior. Add no runtime state, event, dependency, or card owner.

## Steps

1. Replace the old result-title-only WaitDelegate header path with outcome-aware
   header spans derived from `arguments` and `details`.
2. Add a WaitDelegate body renderer before the generic raw-result renderer.
3. Keep running targets collapsed by default; render ordered IDs when expanded.
4. Render finalized target title/ID and status rows, using the existing preview
   limit and expansion hint.
5. Run the exact `neo-tui` card test, touched-file rustfmt check, and scoped
   whitespace check.
6. Stage only this task's hunks and commit with a conventional message.

## Compatibility And Retirement

The runtime and persisted transcript contracts are unchanged. The generic
`Using WaitDelegate`/raw-result presentation is retired when structured wait
data is available; generic failure rendering remains the fallback for malformed
or validation-error results.
