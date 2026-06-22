# Neo Test Suite Deep Analysis

**Generated:** 2026-06-19
**Total test functions found:** ~680+
**Total files with tests:** ~70

---

## Summary of Deletion Recommendations

| Category | Count | Severity |
|----------|-------|----------|
| Near-duplicate infrastructure | 2 files | HIGH |
| Brittle hardcoded values | 3 tests | MEDIUM |
| `thread::sleep` timing-dependent | 6 tests | MEDIUM |
| Overly verbose / low-value | ~8 tests | LOW |
| Near-duplicate assertions | 3 tests | LOW |

---

## Crate 1: `neo-ai`

### Integration Tests (`crates/neo-ai/tests/`)

#### `tool_schema_and_stream.rs` — 10 tests
| Line | Name | Flag |
|------|------|------|
| — | `tool_spec_helpers_collect_arguments` | — |
| — | `tool_spec_collects_arguments_from_schema` | — |
| — | `sse_normalization_*` (4 tests) | — |
| — | serialization tests (3 tests) | — |
**Verdict:** KEEP ALL. Tests real logic, well-structured.

#### `real_provider_adapters.rs` — ~40 tests, 2344 lines
| Line | Name | Flag |
|------|------|------|
| varies | OpenAI Responses streaming (8+ tests) | — |
| varies | Anthropic Messages streaming (8+ tests) | — |
| varies | Google Generative AI streaming (8+ tests) | — |
| varies | retry / reasoning replay (6+ tests) | — |
| varies | image serialization (4+ tests) | — |
**FLAG: FILE-LEVEL** — Massive 2344-line file with duplicated `MockServer` infrastructure (TCP listener + HTTP handler). The same `MockServer` struct appears in `openai_compatible_provider.rs`.
**Recommendation:** Refactor MockServer into a shared `test_utils` module. Do NOT delete tests — they cover real provider adapter logic. Extracting the mock saves ~200 lines of duplication.

#### `openai_compatible_provider.rs` — 4 tests
| Line | Name | Flag |
|------|------|------|
| — | `openai_compatible_normalizes_sse_stream` | — |
| — | `openai_compatible_handles_empty_stream` | — |
| — | `openai_compatible_parses_chat_completion` | — |
| — | `openai_compatible_message_body_serialization` | — |
**FLAG:** Near-duplicate `MockServer` code with `real_provider_adapters.rs`. See above.

#### `fake_provider.rs` — 1 test
| Line | Name | Flag |
|------|------|------|
| — | `fake_model_client_streams_events` | — |
**Verdict:** KEEP. Simple, valid.

#### `provider_resolver.rs` — 9 tests
| Line | Name | Flag |
|------|------|------|
| — | ProviderRegistry credential resolution (9 tests) | — |
**Verdict:** KEEP ALL. Tests real resolution logic.

#### `env_and_options.rs` — 6 tests
| Line | Name | Flag |
|------|------|------|
| — | env API key resolution (2 tests) | — |
| — | RequestOptions (1 test) | — |
| — | ReasoningEffort (1 test) | — |
| — | ReasoningPolicy (1 test) | — |
| — | ReasoningContinuation (1 test) | — |
**Verdict:** KEEP ALL.

#### `model_registry.rs` — 6 tests
| Line | Name | Flag |
|------|------|------|
| — | `model_registry_register_and_get` | — |
| — | `model_registry_list_all` | — |
| — | `model_registry_can_seed_common_builtin_chat_models` | **FLAG** |
| — | other registry tests (3) | — |
**FLAG:** `model_registry_can_seed_common_builtin_chat_models` hardcodes model names like "gpt-5-mini", "gpt-5.4" that will break when model catalog changes. **Recommendation:** Delete or refactor to check count > 0 instead of specific names.

### Inline Modules (`crates/neo-ai/src/`)

#### `types.rs:97-128` — 2 tests
| Line | Name | Flag |
|------|------|------|
| ~100 | `api_type_from_config_string` | — |
| ~115 | `api_type_round_trip` | — |
**Verdict:** KEEP. Round-trip config parsing is worth testing.

#### `catalog.rs:283-381` — 6 tests
| Line | Name | Flag |
|------|------|------|
| ~285 | `infer_api_type_*` (3 tests) | — |
| ~330 | `is_embedding_model` | — |
| ~350 | `catalog_model_capabilities` (2 tests) | — |
**Verdict:** KEEP ALL.

#### `providers/openai_compatible.rs:707-771` — 3 tests
| Line | Name | Flag |
|------|------|------|
| ~710 | `message_body_serialization` | — |
| ~730 | `sse_normalization_edge_cases` (2 tests) | — |
**Verdict:** KEEP ALL.

---

## Crate 2: `neo-tui`

### Integration Tests (`crates/neo-tui/tests/`)

#### `app_shell.rs` — 35 tests
| Line | Name | Flag |
|------|------|------|
| varies | NeoChromeState rendering (8 tests) | — |
| varies | context window / git status (5 tests) | — |
| varies | permission modes / plan mode / goal mode (6 tests) | — |
| varies | tool lifecycle (5 tests) | — |
| varies | session picker / command palette / model picker (6 tests) | — |
| 62 | `cwd_label_uses_shell_home_slash_format` | **FLAG** |
| varies | other shell tests (4 tests) | — |
**FLAG:** `cwd_label_uses_shell_home_slash_format` depends on `$HOME` env var being set. **Recommendation:** Not a deletion candidate, but should be made robust with a mocked HOME.

#### `primitives.rs` — 23 tests
| Line | Name | Flag |
|------|------|------|
| 1-150 | InputEvent mapping (5 tests) | — |
| 154-421 | bracketed paste tests (5 tests) | **FLAG** |
| 422+ | keybindings (3 tests) | — |
| varies | TranscriptViewport (3 tests) | — |
| varies | PromptState editing/undo/history/completion (5 tests) | — |
| varies | ANSI width / wrap_width / truncate_width (2 tests) | — |
**FLAG:** Bracketed paste tests (lines 154-421) are **overly verbose** — each test manually constructs 14-20 `KeyEvent` objects to simulate escape sequences. **Recommendation:** Refactor into a `make_paste_sequence()` helper. Not a deletion candidate, but ~200 lines could become ~40.

#### `tool_grouping.rs` — 10 tests
**Verdict:** KEEP ALL. Tests grouping logic, no issues.

#### `tool_cards.rs` — 16 tests
| Line | Name | Flag |
|------|------|------|
| 385 | `tool_card_lines_do_not_exceed_terminal_width_after_gutter` | **FLAG** |
| 461 | `ask_user_question_header_does_not_exceed_terminal_width_after_gutter` | **FLAG** |
| 517 | `grouped_read_lines_do_not_exceed_terminal_width_after_gutter` | **FLAG** |
| varies | other tool card tests (13 tests) | — |
**FLAG:** Three near-duplicate regression tests all asserting the same gutter-width invariant. **Recommendation:** Consolidate into one parameterized test with a table of component variants. Delete 2 of 3.

#### `todo_question.rs` — 13 tests
**Verdict:** KEEP ALL. Tests QuestionDialog state machine and TodoPanel.

#### `image_protocols.rs` — 12 tests
**Verdict:** KEEP ALL. Tests Kitty/iTerm2/Sixel encoding.

#### `core_components.rs` — 5 tests
**Verdict:** KEEP ALL. Tests Line, Container, GutterContainer, Text.

#### `markdown_rendering.rs` — 18 tests
**Verdict:** KEEP ALL. Good coverage of heading, bold/italic, code, lists, tables, CJK, emoji.

#### `diff_model.rs` — 7 tests
**Verdict:** KEEP ALL. Tests parsing, navigation, folding, copy, line numbers, wrapping.

#### `transcript_store.rs` — 4 tests
**Verdict:** KEEP ALL.

#### `thinking_blocks.rs` — 5 tests
| Line | Name | Flag |
|------|------|------|
| 21 | `live_thinking_shows_spinner_and_tail_window` | — |
| 47 | `live_thinking_spinner_advances_on_render_tick` | — |
| 65 | `completed_thinking_shows_head_window_and_collapse_hint` | — |
| 90 | `completed_short_thinking_shows_all_without_hint` | — |
| 107 | `ctrl_o_toggle_expands_completed_thinking` | — |
**Verdict:** KEEP ALL.

#### `transcript_pane.rs` — 24 tests
| Line | Name | Flag |
|------|------|------|
| 23 | `unchanged_theme_and_size_do_not_schedule_body_rerender` | — |
| 38 | `transcript_pane_renders_transcript_entries_in_one_ordered_frame` | — |
| varies | approval prompt rendering (5 tests) | — |
| varies | streaming / tool card lifecycle (12 tests) | — |
| varies | replay / message mapping (4 tests) | — |
**Verdict:** KEEP ALL. Good coverage of complex rendering pipeline.

### Inline Modules (`crates/neo-tui/src/`)

#### `ansi.rs:507+` — 24 tests
| Line | Name | Flag |
|------|------|------|
| 512 | `rgb_foreground` | — |
| 517 | `named_colors` | — |
| 523 | `foreground_and_background_named_colors_use_matching_ansi_slots` | — |
| 551 | `dynamic_colors_use_foreground_and_background_prefixes` | — |
| 559 | `style_to_ansi_combines` | — |
| 567 | `empty_style_produces_nothing` | — |
| 572 | `paint_wraps_with_reset` | — |
| 579 | `visible_width_strips_ansi` | — |
| 584 | `visible_width_treats_emoji_presentation_as_one_display_unit` | — |
| 592 | `wrap_text_basic` | — |
| 597 | `wrap_text_preserves_empty_lines` | — |
| 602 | `pad_to_width_adds_spaces` | — |
| 607 | `truncate_adds_ellipsis` | — |
| 612 | `strip_ansi_removes_cursor_marker` | — |
| 617 | `strip_ansi_removes_dcs_pm_sos_apc_with_st` | — |
| 625 | `strip_ansi_string_sequences_cancel_on_can_sub` | — |
| 631 | `visible_width_ignores_cursor_marker` | — |
| 637 | `visible_width_ignores_dcs_with_st` | — |
| 642 | `strip_ansi_empty_string` | **FLAG** |
| 647 | `strip_ansi_no_ansi_preserved` | **FLAG** |
| 652 | `strip_ansi_trailing_esc_removed` | — |
| 657 | `strip_ansi_unknown_two_char_sequence_removed` | — |
| 662 | `strip_ansi_multibyte_after_esc_does_not_panic` | — |
| 669 | `strip_ansi_osc_terminated_by_bel` | — |
**FLAG:** `strip_ansi_empty_string` and `strip_ansi_no_ansi_preserved` are trivial edge-case tests. They test that empty input returns empty and non-ANSI text passes through unchanged — both are trivially true. **Recommendation:** Delete these 2. Keep the remaining 22 which test real ANSI parsing edge cases.

#### `input.rs:1098+` — 10 tests
| Line | Name | Flag |
|------|------|------|
| 1107 | `esc_then_enter_becomes_newline` | — |
| 1121 | `esc_alone_is_buffered_and_flushed_after_timeout` | **FLAG** |
| 1133 | `esc_then_letter_does_not_swallow_letter` | — |
| 1147 | `esc_bracket_z_becomes_shift_tab_keybinding` | — |
| 1166 | `bracketed_paste_still_works` | — |
| 1203 | `shift_enter_produces_newline` | — |
| 1212 | `alt_enter_produces_newline` | — |
| 1221 | `ctrl_j_produces_newline` | — |
| 1230 | `keybinding_action_ids_round_trip` | — |
| 1287 | `key_base_names_special_keys_and_characters` | — |
**FLAG:** `esc_alone_is_buffered_and_flushed_after_timeout` (line 1128) uses `thread::sleep(ESC_ENTER_NEWLINE_WINDOW + Duration::from_millis(20))` — timing-dependent. **Recommendation:** This test validates real timing behavior of the ESC key disambiguation. Keep it but note it may be flaky under CI load.

#### `widgets/box_draw.rs:84+` — 8 tests
**Verdict:** KEEP ALL. Box drawing has enough edge cases to justify.

#### `widgets/question_dialog.rs:865+` — 10 tests
**Verdict:** KEEP ALL. Tests state machine logic.

#### `widgets/todo_panel.rs:201+` — 8 tests
**Verdict:** KEEP ALL.

#### `searchable_list.rs:301+` — 7 tests
**Verdict:** KEEP ALL.

#### `dialogs/provider_manager.rs:476+` — 16 tests
**Verdict:** KEEP ALL. Tests dialog state management.

#### `dialogs/model_selector.rs:360+` — 6 tests
**Verdict:** KEEP ALL.

#### `dialogs/choice_picker.rs:345+` — 6 tests
**Verdict:** KEEP ALL.

#### `dialogs/tabbed_model_selector.rs:238+` — 5 tests
**Verdict:** KEEP ALL.

#### `dialogs/api_key_input.rs:137+` — 5 tests
**Verdict:** KEEP ALL.

#### `dialogs/custom_registry_import.rs:241+` — 8 tests
**Verdict:** KEEP ALL.

#### `transcript/entry.rs:1120+` — 7 tests
**Verdict:** KEEP ALL. Tests rendering of transcript entry variants.

#### `transcript/pane.rs:1793+` — 2 tests
| Line | Name | Flag |
|------|------|------|
| 1799 | `prompt_box_lines_are_exact_width` | — |
| 1834 | `completion_dropdown_is_below_prompt` | — |
**Verdict:** KEEP.

#### `transcript/plan_box.rs:137+` — 4 tests
**Verdict:** KEEP ALL.

#### `terminal/renderer.rs:1245+` — 15 tests
**Verdict:** KEEP ALL. Tests diff rendering, Kitty image handling, cursor management — real rendering logic.

---

## Crate 3: `neo-agent`

### Integration Tests (`crates/neo-agent/tests/`)

#### `cli_commands.rs` — 38 tests
| Line | Name | Flag |
|------|------|------|
| 119 | `root_command_reports_interactive_entrypoint_without_placeholders` | — |
| 134 | `root_command_renders_configured_tui_session_state` | — |
| 158 | `root_verbose_flag_renders_real_startup_details` | — |
| 189 | `project_theme_auto_discovery_loads_theme_for_verbose_startup` | — |
| 219 | `root_resume_flag_opens_real_local_session_picker` | — |
| 241 | `root_resume_flag_rejects_subcommands_instead_of_being_ignored` | — |
| 256 | `root_resume_flag_rejects_options_that_conflict_with_the_picker` | — |
| 275 | `run_text_command_without_credentials_fails_without_local_response` | — |
| 294 | `run_command_without_credentials_fails_without_local_response` | — |
| 315 | `sessions_list_uses_workspace_session_bucket` | — |
| 330 | `sessions_rename_and_fork_surface_flat_metadata_without_tree_command` | — |
| 379 | `run_text_with_missing_credentials_does_not_persist_assistant_response` | — |
| 404 | `sessions_show_and_resume_read_jsonl_transcripts` | — |
| 434 | `sessions_accept_exact_workspace_bucket_ids` | — |
| 465 | `sessions_reject_invalid_session_ids` | — |
| 520 | `sessions_compact_stores_algorithmic_summary_and_resume_replays_kept_context` | — |
| 567 | `sessions_export_html_renders_replayed_messages` | — |
| 593 | `sessions_export_json_returns_sanitized_replayed_session_artifact` | — |
| 660 | `extensions_list_discovers_manifests` | — |
| 689 | `extensions_install_update_and_list_sources_from_local_directory` | — |
| 734 | `extensions_defaults_use_project_config_directory_when_invoked_from_another_cwd` | — |
| 806 | `extensions_uninstall_removes_install_dir_and_source_entry` | — |
| 839 | `extensions_call_round_trips_json_rpc` | — |
| 890 | `extensions_lifecycle_commands_persist_status_and_gate_call` | — |
| 970 | `removed_remote_cli_surfaces_fail_parser` | — |
| 1007 | `config_model_scope_selects_first_matching_model_for_interactive_start` | — |
| 1030 | `mcp_list_reports_empty_configuration_without_placeholder_language` | — |
| 1040 | `mcp_list_reads_project_config_servers` | — |
| 1067 | `mcp_list_displays_remote_servers` | — |
| 1098 | `mcp_add_enable_disable_del_persists_project_config_without_printing_secrets` | — |
| 1163 | `mcp_add_remote_http_probes_and_reports_success` | — |
| 1209 | `mcp_add_remote_http_reports_failure_without_abort` | — |
| 1234 | `mcp_add_with_disable_creates_enabled_false` | — |
| 1258 | `mcp_add_studio_parses_command_string_and_cwd` | — |
| 1290 | `mcp_add_with_enabled_tools_filters_tool_list` | — |
| 1370 | `run_text_registers_enabled_stdio_mcp_tools_from_project_config` | — |
| 1423 | `run_text_registers_enabled_http_mcp_tools_from_project_config` | — |
| 1509 | `run_text_rejects_remote_mcp_server_missing_url` | — |
**Verdict:** KEEP ALL. Comprehensive CLI integration tests covering sessions, extensions, MCP, and config.

#### `mock_provider_e2e.rs` — 20 tests
| Line | Name | Flag |
|------|------|------|
| 388 | `run_text_uses_production_openai_responses_adapter_against_mock_provider` | — |
| 435 | `run_emits_jsonl_events_from_mock_provider_without_fake_output` | — |
| 456 | `run_output_json_emits_stable_typed_events_from_mock_provider` | — |
| 483 | `run_output_json_emits_thinking_content_events_from_mock_provider` | — |
| 546 | `run_continue_flag_uses_latest_session_in_stable_json_output` | **FLAG** |
| 590 | `run_text_no_session_flag_runs_without_creating_session_files` | — |
| 608 | `run_text_models_scope_selects_first_matching_runtime_model` | — |
| 649 | `run_text_applies_project_runtime_generation_options_to_provider_request` | — |
| 677 | `run_text_continue_flag_replays_latest_session_and_appends_turn` | **FLAG** |
| 712 | `run_merges_piped_stdin_with_cli_prompt` | — |
| 731 | `run_text_merges_piped_stdin_with_cli_prompt` | **FLAG** |
| 750 | `root_run_text_flag_expands_workspace_relative_file_prompt_args` | — |
| 779 | `run_expands_project_prompt_template_before_json_output` | — |
| 813 | `run_text_expands_project_prompt_template_with_arguments` | — |
| 841 | `run_text_includes_project_system_prompt_file_before_user_message` | — |
| 872 | `run_text_loads_project_context_after_persisted_trust` | — |
| 897 | `run_text_yolo_skips_project_context_even_after_persisted_trust` | — |
| 928 | `run_text_rejects_prompt_file_args_outside_workspace` | — |
| 953 | `run_text_registers_enabled_extension_tool_in_model_request` | — |
| 982 | `run_text_registers_enabled_stdio_mcp_tools_from_project_config` | — |
**FLAG:** Lines 567 and 692 use `thread::sleep(Duration::from_millis(200))`. These are in `run_continue_flag_uses_latest_session_in_stable_json_output` and `run_text_continue_flag_replays_latest_session_and_appends_turn`. The sleep waits for session file writes to be visible. **FLAG:** `run_merges_piped_stdin_with_cli_prompt` (line 712) and `run_text_merges_piped_stdin_with_cli_prompt` (line 731) appear to be near-duplicates testing the same stdin-pipe merge behavior in `run` vs `run text` subcommands. **Recommendation:** Keep one of the stdin-merge pair. The `thread::sleep` tests should be kept but refactored to use file-system polling instead of fixed sleeps.

#### `rpc_mode.rs` — 15 tests
| Line | Name | Flag |
|------|------|------|
| 118 | `rpc_get_state_reports_project_runtime_state` | — |
| 157 | `config_mode_rpc_uses_the_real_rpc_loop_without_subcommand` | — |
| 188 | `rpc_get_messages_replays_session_jsonl_messages` | — |
| 230 | `rpc_get_messages_returns_empty_replay_for_empty_session` | — |
| 258 | `rpc_session_methods_reject_invalid_or_missing_ids` | — |
| 334 | `rpc_get_messages_accepts_in_directory_jsonl_path` | — |
| 363 | `rpc_sessions_list_returns_local_session_metadata` | — |
| 426 | `rpc_sessions_tree_method_is_not_exposed` | — |
| 447 | `rpc_sessions_get_returns_local_session_metadata_and_messages` | — |
| 523 | `rpc_sessions_export_html_returns_rendered_local_session` | — |
| 562 | `rpc_sessions_export_json_returns_sanitized_replayed_session_artifact` | — |
| 639 | `rpc_set_session_name_updates_local_session_metadata` | — |
| 676 | `rpc_get_commands_returns_local_prompt_template_commands` | — |
| 781 | `rpc_get_commands_omits_excluded_auto_discovered_prompt_template` | — |
| 818 | `rpc_prompt_streams_agent_events_and_returns_assistant_text` | — |
**Verdict:** KEEP ALL. Good RPC integration coverage.

### Inline Modules (`crates/neo-agent/src/`)

#### `modes/interactive.rs:4255+` — ~96 tests (10,000+ line file)
**CRITICAL FLAG: FILE-LEVEL.** This is the largest single source file in the project at 10,000+ lines. The test module starting at line ~4876 contains 96 tests covering:
- Git status badge (8 tests)
- Exit message / transcript pane (2 tests)
- Question handling (3 tests)
- TUI draw composition (2 tests)
- Controller snapshot / submit (2 tests)
- Event loop: types, errors, paste, resize, keybindings, clipboard, ctrl-c/d/z, escape, tabs, slash commands, completions (40+ tests)
- Image protocol detection (3 tests)
- Prompt completions (4 tests)
- Autocomplete source (2 tests)
- Approval flow: choice, shortcut, question dialog, revise, cancel, interrupt (12 tests)
- Session: rebuild transcript, load, fork, resume, cross-cwd (6 tests)
- Model picker / config (4 tests)
- Permission modes: slash plan/auto/yolo, shift-tab, plan mode (8 tests)
- Development mode (2 tests)
- Composed frame width (1 test)

**FLAG:** Many tests use `poll_input_event(Duration::from_millis(0))` and `Duration::from_millis(250)` timeouts. While not `thread::sleep`, these tight polling loops may be flaky under CI load.

**FLAG:** Several `#[allow(clippy::too_many_lines)]` annotations at lines 7815, 7978, 8812, 9183, 9504 indicate excessively long individual test functions.

**Recommendation:**
1. Do NOT delete tests — they cover critical interactive behavior.
2. Split this file. The 10,000-line file is a maintenance burden. Split tests into:
   - `tests/interactive_event_loop.rs` (event loop tests)
   - `tests/interactive_approval.rs` (approval flow tests)
   - `tests/interactive_session.rs` (session management tests)
   - `tests/interactive_completions.rs` (slash/tab completion tests)
3. Extract shared test fixtures (mock controllers, fake event streams) into a `test_helpers` module.

#### `modes/run.rs` — 12 tests
| Line | Name | Flag |
|------|------|------|
| 2021 | `agent_config_for_app_applies_runtime_config` | — |
| 2094 | `create_session_path_uses_named_uuid_session_ids` | — |
| 2134 | `stable_json_maps_compaction_lifecycle_events` | — |
| 2184 | `agent_config_for_app_scales_default_compaction_to_model_context_window` | — |
| 2244 | `agent_config_for_app_keeps_explicit_custom_compaction_threshold` | — |
| 2304 | `agent_config_for_app_async_approval_channel_waits_for_ui_decision` | — |
| 2368 | `run_prompt_with_runtime_appends_continuation_to_existing_session_context` | — |
| 2449 | `streaming_event_effects_skip_duplicate_user_message` | — |
| 2462 | `streaming_event_effects_persist_assistant_text` | — |
| 2479 | `streaming_event_effects_persist_non_message_events_without_text` | — |
| 2490 | `list_configured_models_formats_text_entries` | — |
| 2536 | `list_configured_models_formats_json_entries` | — |
**Verdict:** KEEP ALL.

#### `config.rs:1046+` — 5 tests
**Verdict:** KEEP ALL. Tests permission mode config.

#### `config_ops.rs:354+` — 3 tests
**Verdict:** KEEP ALL. Tests provider CRUD operations.

#### `main.rs:689+` — 2 tests
**Verdict:** KEEP. Tests catalog provider listing.

#### `resources.rs:276+` — 4 tests
**Verdict:** KEEP ALL. Tests system prompt composition.

#### `rpc_types.rs:77+` — 4 tests
**Verdict:** KEEP ALL. Tests stable JSON shape of RPC types.

#### `trust.rs:111+` — 3 tests
**Verdict:** KEEP ALL. Tests trust store persistence.

#### `themes.rs:439+` — 2 tests
**Verdict:** KEEP. Tests theme JSON key validation.

#### `prompt_templates.rs:659+` — 2 tests
**Verdict:** KEEP. Tests argument parsing and substitution.

---

## Crate 4: `neo-agent-core`

### Integration Tests (`crates/neo-agent-core/tests/`)

#### `runtime_turn.rs` — 65 tests, ~4579 lines
**CRITICAL FLAG: FILE-LEVEL.** The largest single test file in the project. Contains 65 `#[tokio::test]` functions covering the full runtime turn lifecycle:
- Basic streaming / context / token usage (6 tests)
- Goal mode (1 test)
- Model event yielding (1 test)
- Cancellation (5 tests)
- Tool execution and recording (2 tests)
- Model capability rejection (3 tests)
- Reasoning effort (2 tests)
- Thinking content (4 tests)
- Compaction (4 tests)
- Todo updates (2 tests)
- Error handling (2 tests)
- Tool hooks (1 test)
- Approval flow (8 tests)
- Shell / terminal lifecycle (4 tests)
- Parallel tool execution (3 tests)
- Permission modes: yolo, auto, manual, plan (12 tests)
- Exit plan/goal mode (4 tests)

**FLAG:** Uses `timeout(Duration::from_millis(250), ...)` **~30+ times** and `DelayedStep::Delay(Duration::from_secs(5))` at lines 291, 372. The 250ms timeouts are used as "wait for stream event" polling, which is generally acceptable for async tests but could be flaky under heavy CI load. The 5-second `DelayedStep::Delay` values are used to test "no more events expected" — they verify that a stream produces `None` within the timeout.

**Recommendation:** Do NOT delete tests — they are the core runtime correctness suite. Consider:
1. Extracting `timeout(Duration::from_millis(250), stream.next())` into a `next_event(&mut stream)` helper to reduce boilerplate.
2. The `DelayedStep::Delay(Duration::from_secs(5))` pattern could use a shorter delay (1s) since these test mock providers that respond instantly.

#### `tool_bash.rs` — 20 tests
| Line | Name | Flag |
|------|------|------|
| 6 | `bash_default_timeout_allows_long_workspace_commands` | — |
| 18 | `bash_model_schema_matches_kimi_style_shape` | — |
| 56 | `builtin_tool_names_use_model_facing_kimi_style_casing` | — |
| 87 | `bash_foreground_output_is_raw_terminal_text_with_structured_details` | — |
| 131 | `task_list_defaults_to_active_background_tasks` | — |
| 189 | `bash_background_run_returns_task_id_and_task_output_finishes` | — |
| 234 | `bash_background_requires_description` | — |
| 254 | `task_output_block_times_out_while_task_is_running` | — |
| 295 | `task_stop_is_safe_for_finished_task` | — |
| 337 | `bash_cwd_runs_command_from_workspace_subdirectory` | — |
| 361 | `bash_cwd_rejects_paths_outside_workspace` | — |
| 377 | `bash_foreground_returns_after_shell_exits_with_inherited_background_output` | **FLAG** |
| 403 | `bash_foreground_reports_missing_cd_promptly` | **FLAG** |
| 436 | `bash_foreground_details_do_not_leak_output_past_max_output_bytes` | **FLAG** |
| 465 | `task_output_details_do_not_leak_output_past_max_output_bytes` | — |
| 509 | `bash_foreground_kills_child_when_cancel_token_is_cancelled` | — |
| 563 | `bash_foreground_sigpipe_*` (unix-only) | — |
| 607 | `bash_foreground_handles_large_output_*` (unix-only) | — |
| 708 | `bash_background_start_includes_task_id_and_next_steps` | — |
**FLAG:** Lines 415 and 447 use `thread::sleep(Duration::from_millis(20))` in `bash_foreground_returns_after_shell_exits_with_inherited_background_output` and `bash_foreground_reports_missing_cd_promptly`. These wait for shell process startup. **Recommendation:** Keep but consider polling for output availability instead of fixed sleep.

#### `tool_mcp.rs` — 17 tests
| Line | Name | Flag |
|------|------|------|
| 295 | `mcp_stdio_adapter_discovers_and_calls_json_rpc_tools` | — |
| 357 | `mcp_stdio_adapter_reuses_initialized_session_across_operations` | — |
| 410 | `mcp_stdio_adapter_subscribes_and_receives_resource_updates` | — |
| 454 | `mcp_stdio_adapter_reconnects_after_cached_session_closes` | — |
| 507 | `mcp_stdio_adapter_registers_session_with_process_supervisor_for_cleanup` | — |
| 545 | `mcp_http_adapter_subscribes_to_sse_resource_updates` | — |
| 586 | `mcp_http_adapter_reads_resource_updates_from_event_channel_after_json_subscribe_ack` | — |
| 642 | `mcp_http_adapter_uses_event_stream_url_from_json_subscribe_ack` | — |
| 700 | `mcp_http_adapter_requires_sse_event_channel_after_json_subscribe_ack` | — |
| 743 | `mcp_http_adapter_rejects_non_http_event_stream_url_from_json_subscribe_ack` | — |
| 787 | `mcp_http_adapter_reports_sse_stream_end_after_subscribe_response` | — |
| 815 | `mcp_http_adapter_discovers_and_calls_json_rpc_tools` | — |
| 887 | `mcp_http_adapter_accepts_sse_json_rpc_responses` | — |
| 921 | `mcp_http_adapter_lists_and_reads_resources` | — |
| 979 | `mcp_provider_discovers_namespaced_tool_specs` | — |
| 1020 | `mcp_tool_execution_delegates_to_async_adapter` | — |
| 1059 | `mcp_tool_execution_surfaces_adapter_errors` | — |
**FLAG:** Lines 1364, 1374 use `thread::sleep(Duration::from_millis(20))`. These are in helper functions, not in the tests directly. **Recommendation:** Keep. The sleep is in test helpers for process startup.

#### `tool_permissions.rs` — 7 tests
**Verdict:** KEEP ALL. Tests permission enforcement.

#### `tool_terminal.rs` — 7 tests
| Line | Name | Flag |
|------|------|------|
| 46 | `terminal_read_waits_briefly_for_fresh_running_output` | **FLAG** |
| 96 | `terminal_write_then_read_observes_interactive_shell_output` | — |
| 153 | `terminal_tool_start_write_read_resize_and_stop_uses_real_pty` | — |
| 261 | `process_supervisor_cleanup_stops_terminal_handles` | — |
| 299 | `terminal_read_details_do_not_leak_output_past_max_output_bytes` | — |
| 355 | `terminal_rejects_missing_mode` | — |
| 371 | `terminal_rejects_unknown_handle` | — |
**FLAG:** Lines 415, 447 use `thread::sleep(Duration::from_millis(20))` waiting for PTY process startup. **Recommendation:** Keep — PTY tests inherently need process startup time.

#### `tool_files.rs` — 3 tests
**Verdict:** KEEP ALL. Tests file read/search/write/edit.

#### `session_jsonl.rs` — 12 tests
**Verdict:** KEEP ALL. Tests JSONL session persistence, replay, compaction.

#### `session_tree.rs` — 5 tests
**Verdict:** KEEP ALL. Tests session metadata and hierarchy.

#### `rpc_jsonl.rs` — 5 tests
**Verdict:** KEEP ALL. Tests JSONL codec.

#### `goals.rs` — 5 tests
**Verdict:** KEEP ALL. Tests goal lifecycle.

#### `skills.rs` — 14 tests
**Verdict:** KEEP ALL. Tests skill loading, expansion, invocation, discovery.

#### `tool_names.rs` — 2 tests
**Verdict:** KEEP. Tests name safety.

#### `extension_runner.rs` — 5 tests
**Verdict:** KEEP ALL. Tests stdio runner JSONL RPC.

### Inline Modules (`crates/neo-agent-core/src/`)

#### `runtime.rs:2986+` — 5 tests
| Line | Name | Flag |
|------|------|------|
| 3027 | `execute_invoke_skill_expands_named_string_arguments` | — |
| 3050 | `execute_invoke_skill_tolerates_undeclared_string_argument` | — |
| 3088 | `execute_invoke_skill_rejects_disabled_missing_and_flow_cases` | — |
| 3137 | `skill_tool_request_converts_only_string_named_arguments` | — |
| 3163 | `skill_is_manual_only_tracks_flow_manifest_type` | — |
**Verdict:** KEEP ALL.

#### `events.rs:326+` — 9 tests
**Verdict:** KEEP ALL. Tests event serialization.

#### `session/workspace.rs:80+` — 7 tests
**Verdict:** KEEP ALL. Tests slugify and workspace key encoding.

#### `session/index.rs:228+` — 8 tests
**Verdict:** KEEP ALL. Tests session index append/find/list.

#### `session/export.rs:285+` — 3 tests
**Verdict:** KEEP ALL. Tests URL sanitization in markdown export.

#### `mode/plan_mode_guard.rs:80+` — 6 tests
**Verdict:** KEEP ALL. Tests plan mode write restrictions.

#### `mode/plan.rs:189+` — 8 tests
**Verdict:** KEEP ALL. Tests plan lifecycle.

#### `tools/todo.rs:250+` — 20 tests
**Verdict:** KEEP ALL. Tests todo tool schema, execution, formatting, state management. Large but thorough.

#### `tools/ask_user.rs:317+` — 14 tests
| Line | Name | Flag |
|------|------|------|
| varies | ask_user lifecycle tests (10 tests) | — |
| 631 | `ask_user_background_stopped_question_ignores_late_answer` | **FLAG** |
| 672 | (related background test) | **FLAG** |
**FLAG:** Lines 631, 672 use `tokio::time::sleep(Duration::from_millis(10))` for background task timing. **Recommendation:** Keep — the 10ms sleep is minimal and tests real async behavior.

#### `tools/glob.rs:201+` — 13 tests
**Verdict:** KEEP ALL.

#### `tools/grep.rs:591+` — 13 tests
**Verdict:** KEEP ALL.

#### `tools/list.rs:183+` — 6 tests
**Verdict:** KEEP ALL.

#### `tools/find.rs:149+` — 5 tests
**Verdict:** KEEP ALL.

#### `tools/read.rs:303+` — 10 tests
**Verdict:** KEEP ALL.

#### `tools/background_tasks.rs:858+` — 10 tests
**Verdict:** KEEP ALL.

#### `tools/plan_mode.rs:197+` — 11 tests
**Verdict:** KEEP ALL.

#### `tools/skills_manager.rs:387+` — 5 tests
**Verdict:** KEEP ALL.

#### `tools/mcp.rs:1374+` — 1 test
| Line | Name | Flag |
|------|------|------|
| 1379 | `sse_event_byte_buffer_waits_for_complete_utf8_event` | — |
**Verdict:** KEEP. Tests UTF-8 boundary handling.

#### `tools/sessions.rs:242+` — 4 tests
**Verdict:** KEEP ALL.

#### `tools/extensions/bridge.rs:269+` — 2 tests
**Verdict:** KEEP.

#### `tools/goal.rs:338+` — 6 tests
**Verdict:** KEEP ALL.

---

## Actionable Deletion / Refactoring List

### DELETE (low risk, clear benefit)

| # | File | Test(s) | Reason |
|---|------|---------|--------|
| 1 | `neo-ai/tests/model_registry.rs` | `model_registry_can_seed_common_builtin_chat_models` | Hardcodes "gpt-5-mini", "gpt-5.4" — brittle to catalog changes |
| 2 | `neo-tui/src/ansi.rs:642` | `strip_ansi_empty_string` | Trivial: empty in → empty out |
| 3 | `neo-tui/src/ansi.rs:647` | `strip_ansi_no_ansi_preserved` | Trivial: no-ANSI text passes through unchanged |
| 4 | `neo-tui/tests/tool_cards.rs:461` | `ask_user_question_header_does_not_exceed_terminal_width_after_gutter` | Near-duplicate of line 385 test |
| 5 | `neo-tui/tests/tool_cards.rs:517` | `grouped_read_lines_do_not_exceed_terminal_width_after_gutter` | Near-duplicate of line 385 test |
| 6 | `neo-agent/tests/mock_provider_e2e.rs:731` | `run_text_merges_piped_stdin_with_cli_prompt` | Near-duplicate of line 712 `run_merges_piped_stdin_with_cli_prompt` |

**Total lines freed:** ~120 lines of test code + maintenance burden

### REFACTOR (medium effort, high benefit)

| # | What | Into What | Benefit |
|---|------|-----------|---------|
| 1 | `real_provider_adapters.rs` + `openai_compatible_provider.rs` MockServer | Shared `test_utils::MockServer` module | Eliminates ~200 lines of duplicated TCP/HTTP mock infrastructure |
| 2 | `neo-tui/tests/primitives.rs` bracketed paste tests (lines 154-421) | `make_paste_sequence()` helper + table-driven tests | Reduces ~270 lines to ~40 lines |
| 3 | `neo-agent/src/modes/interactive.rs` (10,000 lines, 96 tests) | Split into 4-5 focused test files | Massive readability/maintenance improvement |
| 4 | `neo-agent-core/tests/runtime_turn.rs` (4579 lines, 65 tests) | Extract `next_event()` helper, split into 3 files | Reduces boilerplate, ~30 identical timeout patterns |
| 5 | `neo-tui/src/input.rs:1121` `esc_alone_is_buffered_and_flushed_after_timeout` | Replace `thread::sleep` with async timer | Eliminates timing flakiness |

### KEEP BUT NOTE (acceptable risk)

| # | File | Issue | Why Keep |
|---|------|-------|----------|
| 1 | `runtime_turn.rs` 250ms timeouts (~30x) | Timeout-based polling | Tests real async behavior; 250ms is generous |
| 2 | `tool_bash.rs` 20ms sleeps | Process startup | Shell tests need real process time |
| 3 | `tool_mcp.rs` 20ms sleeps | Process startup | MCP server tests need real process time |
| 4 | `tool_terminal.rs` 20ms sleeps | PTY startup | PTY tests inherently need process time |
| 5 | `app_shell.rs:62` `$HOME` dependency | Env var | Should mock HOME but not a deletion candidate |

---

## Statistics Summary

| Crate | Integration Tests | Inline Tests | Total Tests | Flagged |
|-------|------------------|-------------|-------------|---------|
| neo-ai | 76 | 11 | 87 | 1 delete |
| neo-tui | 177 | 152 | 329 | 4 delete, 2 refactor |
| neo-agent | 73 | 37 | 110 | 1 delete, 1 refactor |
| neo-agent-core | 168 | 131 | 299 | 0 delete, 2 refactor |
| **TOTAL** | **494** | **331** | **825** | **6 delete, 5 refactor** |
