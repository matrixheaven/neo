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

## Task 2 Verification Evidence

- `cargo test -p neo-agent-core --lib messages::tests::thinking_part_id_roundtrips_and_historical_content_defaults -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-agent-core --lib runtime::stream_aggregator::tests::thinking_parts_preserve_provider_ids_and_raw_order -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-agent-core --test runtime_turn runtime_preserves_multiple_thinking_parts_and_text_order -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test transcript_store adjacent_summary_parts_keep_ids_and_compact_visible_projection -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test transcript_store multi_part_unknown_thinking_wraps_as_one_display_stream -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test transcript_store live_and_replayed_redacted_thinking_keep_raw_text_and_render_parity -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test transcript_store summary_projection_deduplicates_unclosed_title_across_parts -- --exact --nocapture`: passed, 1 test.
- `cargo fmt --all -- --check`: passed; `git diff --check`: passed.
- A first unqualified library filter matched zero tests and was discarded as invalid evidence; the subsequent module-qualified exact test passed.

## Review Evidence

- Fresh Task 2 implementer completed the slice without Git lifecycle mutations.
- Spec-compliance reviewer: `PASS` after empty id-bearing replay, pre-render part preservation, and live/replay grouping repairs.
- Code-quality reviewer: `PASS` after runtime expectation, global display wrapping, redaction parity, API cleanup, duplicate-test cleanup, and unclosed-title deduplication repairs.

## Scope Notes

- MessagePhase remains orthogonal to ThinkingKind.
- OpenAI phase mapping uses only explicit output-item `phase` values.
- Task 2 retains provider/event thinking ids and raw ordered parts in canonical `Content` and existing TUI `ThinkingBlock` state; redacted placeholders remain render-time projection.
- No transcript owner, hidden-reasoning path, provider runtime rewrite, card, context-prefix, or session-history change was introduced by this slice.
- Workspace-wide `cargo test --workspace --no-run` remains unclaimed; the known unrelated `NeoChromeState::thinking_enabled()` error at `crates/neo-agent/src/modes/interactive/tests.rs:12959` remains outside scope and untouched.
