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

## Task 3 Verification Evidence

- `cargo test -p neo-tui --test thinking_blocks summary_thinking_preserves_body_after_title -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test thinking_blocks summary_thinking_keeps_inline_bold_body -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test thinking_blocks summary_thinking_collapses_adjacent_duplicate_titles -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test thinking_blocks summary_thinking_omits_placeholder_and_collapses_titles_across_it -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test thinking_blocks summary_thinking_keeps_title_across_empty_active_part -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test thinking_blocks summary_thinking_without_leading_title_uses_generic_spinner -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test thinking_blocks summary_thinking_preserves_indented_body_after_title -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test thinking_blocks full_thinking_renders_bounded_preview -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test thinking_blocks unknown_thinking_does_not_extract_title -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test transcript_store summary_projection_keeps_body_after_leading_title_across_ordered_parts -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test transcript_store summary_projection_keeps_unclosed_bold_body_across_parts -- --exact --nocapture`: passed, 1 test.
- `cargo test -p neo-tui --test transcript_store summary_projection_keeps_body_without_leading_title_across_parts -- --exact --nocapture`: passed, 1 test.
- `cargo check -p neo-tui --tests`: passed; `cargo fmt --all -- --check`: passed; `git diff --check`: passed.

## Task 4 Verification Evidence

- `cargo test --package neo-tui --test transcript_store commentary_and_final_answer_render_as_separate_entries -- --exact --nocapture`: passed, 1 test. This covers explicit phase propagation, separate ordered assistant entries, `▸` Commentary presentation, `●` FinalAnswer presentation, live/history separation, and canonical text order.
- `cargo test --package neo-tui --test transcript_store unknown_message_phase_preserves_legacy_rendering -- --exact --nocapture`: passed, 1 test. This covers the Unknown phase's historical `●` marker and non-commentary route.
- The Task4 ordered test also exercises `TranscriptStore::render_rows` through direct `plain_rows` assertions, proving the compatibility projection uses the same phase metadata and markers as the primary presentation path.
- `cargo check -p neo-tui --tests`: passed.
- `cargo fmt --all -- --check`: passed; `git diff --check`: passed.
- Task4 source scope is limited to the existing transcript event/store/pane/presentation owners and `transcript_store` tests; no card, provider, runtime, persistence, context, or session-history path is in the task-owned diff.
- The independent advisory review identified an Important gap in the alternate `TranscriptStore::render_rows` projection. The coordinator repaired that path in the canonical store owner and added direct regression assertions; no second owner or compatibility fallback was introduced.
- Workspace-wide `cargo test --workspace --no-run` remains unclaimed for Task4 because the known unrelated `NeoChromeState::thinking_enabled()` error at `crates/neo-agent/src/modes/interactive/tests.rs:12959` remains outside scope and untouched.

## Review Evidence

- Fresh Task 2 implementer completed the previous slice without Git lifecycle mutations.
- Task 2 spec-compliance reviewer: `PASS` after empty id-bearing replay, pre-render part preservation, and live/replay grouping repairs.
- Task 2 code-quality reviewer: `PASS` after runtime expectation, global display wrapping, redaction parity, API cleanup, duplicate-test cleanup, and unclosed-title deduplication repairs.
- Fresh Task 3 implementer completed the renderer slice without Git lifecycle mutations.
- Task 3 spec-compliance reviewer: `PASS` after placeholder continuity, body-only streaming, and ordered title/body repairs.
- Task 3 code-quality reviewer: `PASS` after active-title, body-indentation, renderer-comment, and checkpoint-state repairs.
- Task 4 advisory reviewer assessed the working-tree routing slice against the approved spec, owner boundary, compatibility behavior, focused evidence, and retirement rules; it identified the alternate `render_rows` route as an Important evidence/behavior gap.
- The coordinator repaired the identified path in `TranscriptStore::render_rows`, reran both exact Task4 regressions, `cargo check -p neo-tui --tests`, formatting, and `git diff --check`; the repaired slice is ready for task-only commit review.
- No ADR is needed for this slice: the implementation keeps the existing `TranscriptStore`/`TranscriptPresentation` owners and adds no new durable architecture contract.

## Scope Notes

- MessagePhase remains orthogonal to ThinkingKind.
- OpenAI phase mapping uses only explicit output-item `phase` values.
- Task 2 retains provider/event thinking ids and raw ordered parts in canonical `Content` and existing TUI `ThinkingBlock` state; redacted placeholders remain render-time projection.
- Task 4 routes explicit phase metadata through the existing assistant entry state; Commentary is lower-emphasis normal assistant output, FinalAnswer is normal Markdown, and Unknown remains legacy-compatible.
- No second transcript owner, hidden-reasoning path, provider runtime rewrite, card, context-prefix, or session-history change was introduced by this slice.
- Workspace-wide `cargo test --workspace --no-run` remains unclaimed; the known unrelated `NeoChromeState::thinking_enabled()` error at `crates/neo-agent/src/modes/interactive/tests.rs:12959` remains outside scope and untouched.
