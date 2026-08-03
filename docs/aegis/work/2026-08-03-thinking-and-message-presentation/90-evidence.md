# Thinking and Message Presentation - Evidence

## Documentation Evidence

- `docs/aegis/specs/2026-08-03-thinking-and-message-presentation-design.md`
  records the complete design and acceptance criteria.
- `docs/aegis/plans/2026-08-03-thinking-and-message-presentation.md` records
  the executable source boundaries, task order, verification matrix, risks,
  compatibility, and retirement conditions.
- `docs/aegis/handoffs/2026-08-03-thinking-and-message-presentation.md`
  records the implementation locks and resume rules.
- The existing repository state confirms `ThinkingKind` is already present;
  this slice does not add another thinking enum or alter that implementation.

## Task 1 Verification Evidence

- `cargo nextest run -p neo-ai --lib response_message_item_phase_maps_explicit_values_without_duplicate_start`: passed, 1 test.
- `cargo nextest run -p neo-ai --lib response_message_item_done_phase_promotes_buffered_reasoning_tool_text_in_order`: passed, 1 test.
- `cargo nextest run -p neo-ai --lib buffered_terminal_events_fall_back_to_unknown_phase`: passed, 1 test.
- `cargo nextest run -p neo-ai --test real_provider_adapters openai_responses_client_buffers_pre_phase_reasoning_and_tool_events_until_message_phase`: passed, 1 test.
- `cargo nextest run -p neo-ai --test real_provider_adapters openai_compatible_client_finishes_tool_call_on_tool_calls_finish_reason_without_done`: passed, 1 test.
- `cargo nextest run -p neo-agent-core --lib message_phase_flows_through_runtime_message_lifecycle`: passed, 1 test.
- `cargo test -p neo-agent-core --test workflow_dispatch --no-run`: passed; `cargo test -p neo-tui --test app_shell --no-run`: passed.
- `cargo fmt --all -- --check`: passed; `git diff --check`: passed.
- Workspace-wide `cargo test --workspace --no-run` was attempted by the implementer and remains blocked only by unrelated `crates/neo-agent/src/modes/interactive/tests.rs:12959` calling missing `NeoChromeState::thinking_enabled()`; no fix was made.

## Review Evidence

- Fresh implementer completed the slice without Git lifecycle mutations.
- Spec-compliance reviewer: `APPROVED` after buffering, late-phase, fallback, idempotence, and historical serde repairs.
- Code-quality reviewer: `APPROVED` after all direct compatibility literals and formatting repairs.

## Scope Notes

- MessagePhase remains orthogonal to ThinkingKind.
- OpenAI phase mapping uses only explicit output-item `phase` values.
- No transcript owner, hidden-reasoning path, provider runtime rewrite, card, context-prefix, or session-history change was introduced by this slice.
