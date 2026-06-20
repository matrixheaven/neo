# Conservative Test Slimming Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce Neo test source size and routine test runtime conservatively without weakening behavior coverage.

**Architecture:** Do not rewrite production Rust logic. Only consolidate tests that already assert the same mechanism with different fixtures, move repeated setup into helpers, and convert obvious families into table-driven tests. Keep high-risk runtime semantics covered, especially permissions, cancellation, session replay, provider wire formats, and TUI keybinding behavior.

**Tech Stack:** Rust 2024, Cargo test, Tokio tests, `rtk` command prefix, existing Neo crates (`neo-tui`, `neo-agent`, `neo-agent-core`, `neo-ai`, `xtask`).

---

## Critical Rules For The Worker

Read this before touching files.

- Do not run destructive git commands. Never run `git reset`, `git restore`, `git checkout --`, `git stash`, `git clean`, `git rebase`, or force push.
- Do not delete coverage just because tests look similar. A test can be removed only after its assertions are preserved in a merged/table-driven test.
- Do not change production behavior. This project asks for test simplification only.
- Do not run broad expensive commands first. Use narrow test commands listed in each task.
- Prefer table-driven tests using local arrays or helper structs. Do not add new crates.
- Keep commits small. One task equals one commit.
- If a test is flaky or fails before your edit, stop and report it. Do not “fix” unrelated failures.

## Current Test Footprint Snapshot

Source scan found about 854 Rust test functions:

- `crates/neo-tui`: about 304
- `crates/neo-agent-core`: about 227
- `crates/neo-agent`: about 188
- `crates/neo-ai`: about 80
- `xtask`: about 55

Largest candidates by test count / file size:

- `crates/neo-agent/src/modes/interactive.rs`: about 89 tests
- `xtask/src/main.rs`: 55 tests
- `crates/neo-agent-core/tests/runtime_turn.rs`: 46 tests, 3900+ lines
- `crates/neo-ai/tests/real_provider_adapters.rs`: 40 tests, 2300+ lines
- `crates/neo-agent/tests/cli_commands.rs`: 39 tests, 1700+ lines
- `crates/neo-tui/tests/primitives.rs`: 37 tests
- `crates/neo-tui/tests/app_shell.rs`: 26 tests
- `crates/neo-tui/tests/transcript_pane.rs`: 24 tests

Expected conservative reduction: 60-100 test functions. Do not chase a larger number in one pass.

---

## Task 0: Baseline And Guardrails

**Files:**
- Modify: none
- Test: none

- [ ] **Step 1: Confirm the worktree is clean before starting**

Run:

```bash
rtk git status --short
```

Expected:

```text
ok
```

If files are modified, stop and ask the owner whether to continue. Do not overwrite them.

- [ ] **Step 2: Record current test counts by source scan**

Run:

```bash
node - <<'NODE'
const {execFileSync}=require('child_process');
const out=execFileSync('rg',['-n','#\\[(tokio::)?test\\]','crates','xtask','examples','--glob','*.rs'],{encoding:'utf8'});
const rows=out.trim().split('\n').filter(Boolean).map(l=>l.split(':')[0]);
const byFile=new Map();
const byCrate=new Map();
for (const file of rows) {
  byFile.set(file,(byFile.get(file)||0)+1);
  const m=file.match(/^(crates\/[^\/]+|xtask|examples\/rust)/);
  const crate=m?m[1]:'other';
  byCrate.set(crate,(byCrate.get(crate)||0)+1);
}
console.log(JSON.stringify({
  total: rows.length,
  byCrate: [...byCrate].sort((a,b)=>b[1]-a[1]),
  topFiles: [...byFile].sort((a,b)=>b[1]-a[1]).slice(0,20)
}, null, 2));
NODE
```

Expected:

- Total is close to 854.
- Top files include `interactive.rs`, `xtask/src/main.rs`, `runtime_turn.rs`, `real_provider_adapters.rs`, and `cli_commands.rs`.

- [ ] **Step 3: Create a local notes file for measurements**

Create or update:

`docs/superpowers/plans/2026-06-20-test-slimming-measurements.md`

Initial content:

```markdown
# Test Slimming Measurements

## Baseline

- Source test function count:
- Date:
- Commit:

## Completed Tasks

- None yet.
```

- [ ] **Step 4: Do not commit baseline notes unless the owner wants measurement docs committed**

If the owner wants a record, commit only the measurements file:

```bash
git add docs/superpowers/plans/2026-06-20-test-slimming-measurements.md
git commit -m "docs(test): record test slimming baseline"
```

Otherwise leave the measurements file untracked or delete it with normal file editing, not git clean.

---

## Task 1: Consolidate Xtask Parity Stale-Claim Tests

**Why this first:** Lowest production risk. It touches test helpers in `xtask` only and offers high count reduction.

**Files:**
- Modify: `xtask/src/main.rs`

**Target tests to inspect:**

- `parity_validation_rejects_stale_mcp_adapter_gap_after_adapter_symbol_exists`
- `parity_validation_rejects_stale_mcp_process_gap_after_stdio_adapter_exists`
- `parity_validation_rejects_stale_http_mcp_json_subscribe_gap_after_event_reader_exists`
- `parity_validation_rejects_stale_mcp_event_stream_url_gap_after_symbols_exist`
- `parity_validation_rejects_stale_extension_lifecycle_gap_after_commands_exist`
- `parity_validation_rejects_stale_session_branching_gap_after_fork_exists`
- `parity_validation_rejects_stale_live_session_picker_gap_after_interactive_symbols_exist`
- `parity_validation_rejects_stale_live_model_picker_gap_after_interactive_symbols_exist`
- `parity_validation_rejects_stale_fork_before_continue_gap_after_session_fork_ui_exists`
- `parity_validation_rejects_stale_runtime_hook_queue_gap_after_symbols_exist`
- `parity_validation_rejects_stale_tui_diff_gap_after_renderer_symbols_exist`
- `parity_validation_rejects_stale_tui_paste_buffering_gap_after_input_parser_symbols_exist`
- `parity_validation_rejects_stale_tui_transcript_selection_copy_gap_after_symbols_exist`
- `parity_validation_rejects_stale_terminal_image_protocol_gap_after_symbols_exist`
- `parity_validation_rejects_stale_sixel_gap_after_encoder_symbols_exist`
- `parity_validation_rejects_stale_session_export_json_gap_after_symbols_exist`
- `parity_validation_rejects_stale_reasoning_replay_control_gap_after_symbols_exist`
- `parity_validation_rejects_stale_ai_thinking_gap_after_payload_symbols_exist`
- `parity_validation_rejects_stale_ai_thinking_translation_gap_after_payload_symbols_exist`

- [ ] **Step 1: Locate existing helper names**

Run:

```bash
rg -n "stale_.*gap|parity_validation_rejects_stale|validate.*parity|fixture" xtask/src/main.rs
```

Expected: You see the stale-gap tests and existing fixture helper functions.

- [ ] **Step 2: Write one table-driven test that covers the same stale-gap cases**

In `xtask/src/main.rs`, add this test near the existing stale-gap tests. Adjust helper calls to match the existing helper names in the file.

Use this exact structure, but replace `run_parity_validation_for_fixture` and `assert_parity_rejects` with the actual helper functions already used by the old tests:

```rust
#[test]
fn parity_validation_rejects_stale_gap_claims_after_symbols_exist() {
    struct Case {
        name: &'static str,
        doc: &'static str,
        source_path: &'static str,
        source: &'static str,
        expected: &'static str,
    }

    let cases = [
        Case {
            name: "mcp adapter gap",
            doc: "MCP adapter is missing",
            source_path: "crates/neo-agent-core/src/tools/mcp.rs",
            source: "pub struct McpStdioToolAdapter;",
            expected: "stale",
        },
        Case {
            name: "session fork gap",
            doc: "Session forking is missing",
            source_path: "crates/neo-agent/src/modes/interactive.rs",
            source: "fn fork_selected_session() {}",
            expected: "stale",
        },
        Case {
            name: "terminal image protocol gap",
            doc: "Terminal image protocols are missing",
            source_path: "crates/neo-tui/src/image.rs",
            source: "pub enum ImageProtocol { Kitty, Iterm2, Sixel }",
            expected: "stale",
        },
    ];

    for case in cases {
        let result = run_parity_validation_for_fixture(case.doc, case.source_path, case.source);
        assert_parity_rejects(&result, case.expected, case.name);
    }
}
```

Important: The final case list must include all old stale-gap scenarios before deleting old tests. Start with 3 cases, run red/green, then add the rest.

- [ ] **Step 3: Run the new test only**

Run:

```bash
rtk cargo test -p xtask parity_validation_rejects_stale_gap_claims_after_symbols_exist -- --nocapture
```

Expected: PASS.

If it fails because helper names are wrong, fix only the helper calls. Do not edit production parity logic.

- [ ] **Step 4: Move the remaining old stale-gap fixture data into the table**

For every old stale-gap test listed above, copy its input doc/source/expected string into the `cases` array.

Keep each case named clearly:

```rust
Case {
    name: "live model picker",
    doc: "...exact old doc fixture...",
    source_path: "...exact old source path...",
    source: "...exact old source fixture...",
    expected: "...exact old expected error substring...",
},
```

- [ ] **Step 5: Delete only old tests now covered by the table**

Remove the individual stale-gap test functions listed in this task. Do not remove helper functions unless they become unused and the compiler confirms it.

- [ ] **Step 6: Verify xtask**

Run:

```bash
rtk cargo test -p xtask -- --nocapture
```

Expected: all xtask tests pass.

- [ ] **Step 7: Format and inspect count reduction**

Run:

```bash
rtk cargo fmt --all --check
node - <<'NODE'
const {execFileSync}=require('child_process');
const out=execFileSync('rg',['-n','#\\[(tokio::)?test\\]','xtask/src/main.rs'],{encoding:'utf8'});
console.log(out.trim().split('\n').filter(Boolean).length);
NODE
```

Expected: `xtask/src/main.rs` test count is lower by at least 10.

- [ ] **Step 8: Commit**

Run:

```bash
git add xtask/src/main.rs
git commit -m "test(xtask): consolidate stale gap parity cases"
```

---

## Task 2: Consolidate Xtask Auth Token Leak Tests

**Files:**
- Modify: `xtask/src/main.rs`

**Target tests to inspect:**

- `parity_validation_rejects_auth_token_leaks_in_docs`
- `parity_validation_allows_auth_token_placeholders_in_docs`
- `parity_validation_does_not_treat_source_identifiers_as_auth_token_leaks`
- `parity_validation_rejects_private_package_signature_fixture_material`

- [ ] **Step 1: Read the four tests**

Run:

```bash
rg -n "auth_token|token_leaks|private_package_signature|source_identifiers" xtask/src/main.rs
```

- [ ] **Step 2: Create a table-driven test**

Replace the four tests with one test like this, using existing helper names from the file:

```rust
#[test]
fn parity_validation_checks_secret_like_doc_content() {
    struct Case {
        name: &'static str,
        doc: &'static str,
        should_pass: bool,
        expected: &'static str,
    }

    let cases = [
        Case {
            name: "rejects real token",
            doc: "api_key = \"sk-real-looking-secret-value\"",
            should_pass: false,
            expected: "token",
        },
        Case {
            name: "allows placeholder token",
            doc: "api_key = \"${OPENAI_API_KEY}\"",
            should_pass: true,
            expected: "",
        },
        Case {
            name: "allows source identifier",
            doc: "The auth_token field is redacted by config show.",
            should_pass: true,
            expected: "",
        },
        Case {
            name: "rejects private package signature fixture material",
            doc: "-----BEGIN PRIVATE KEY-----\nexample\n-----END PRIVATE KEY-----",
            should_pass: false,
            expected: "private",
        },
    ];

    for case in cases {
        let result = run_docs_parity_validation(case.doc);
        if case.should_pass {
            assert!(result.is_ok(), "{} should pass: {result:?}", case.name);
        } else {
            let error = result.expect_err(case.name);
            assert!(
                error.to_string().contains(case.expected),
                "{} expected {:?}, got {error}",
                case.name,
                case.expected
            );
        }
    }
}
```

Again, adjust `run_docs_parity_validation` to the real helper.

- [ ] **Step 3: Run only the new test**

```bash
rtk cargo test -p xtask parity_validation_checks_secret_like_doc_content -- --nocapture
```

Expected: PASS.

- [ ] **Step 4: Run xtask tests**

```bash
rtk cargo test -p xtask -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add xtask/src/main.rs
git commit -m "test(xtask): table drive secret parity cases"
```

---

## Task 3: Consolidate Question Dialog TUI Tests

**Why this is safe:** These are mostly state-machine tests and simple overlay behavior. Keep one render-width test and one full-flow test.

**Files:**
- Modify: `crates/neo-tui/tests/todo_question.rs`
- Modify only if needed: `crates/neo-tui/src/widgets/question_dialog.rs`

**Target tests:**

- `app_pushes_question_overlay`
- `app_confirm_question_returns_answers`
- `app_cancel_question_returns_id`
- `app_closes_question_overlay_by_question_id`
- `question_dialog_esc_cancels`
- `question_submit_page_number_two_cancels`
- `question_dialog_tab_navigation_through_keys`
- `question_dialog_number_key_selection`
- `question_dialog_down_moves_one_option_at_a_time`

- [ ] **Step 1: Run existing tests before editing**

```bash
rtk cargo test -p neo-tui --test todo_question -- --nocapture
```

Expected: 16 tests pass.

- [ ] **Step 2: Add one table-driven key behavior test**

In `crates/neo-tui/tests/todo_question.rs`, add:

```rust
#[test]
fn question_dialog_key_behaviors() {
    struct Case {
        name: &'static str,
        keys: &'static [KeyCode],
        assert_state: fn(&NeoChromeState),
    }

    let cases = [
        Case {
            name: "enter selects first option and reaches submit",
            keys: &[KeyCode::Enter],
            assert_state: |app| {
                let state = app.question_dialog_state().expect("focused");
                assert!(state.on_submit_tab(), "expected submit tab");
                assert!(state.questions[0].selected[0], "first option selected");
            },
        },
        Case {
            name: "number selects matching option",
            keys: &[KeyCode::Char('2')],
            assert_state: |app| {
                let state = app.question_dialog_state().expect("focused");
                assert!(state.questions[0].selected[1], "second option selected");
                assert!(!state.questions[0].selected[0], "first option unselected");
            },
        },
        Case {
            name: "down moves one row",
            keys: &[KeyCode::Down],
            assert_state: |app| {
                let state = app.question_dialog_state().expect("focused");
                assert_eq!(state.cursor, 1);
            },
        },
        Case {
            name: "right reaches submit and left returns",
            keys: &[KeyCode::Enter, KeyCode::Right, KeyCode::Left],
            assert_state: |app| {
                let state = app.question_dialog_state().expect("focused");
                assert_eq!(state.active_tab, 0);
            },
        },
    ];

    for case in cases {
        let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
        app.push_question_overlay("q-1", make_single_question());
        for key in case.keys {
            let _ = app.handle_question_dialog_key(KeyEvent::new(*key, KeyModifiers::NONE));
        }
        (case.assert_state)(&app);
    }
}
```

- [ ] **Step 3: Run the new test**

```bash
rtk cargo test -p neo-tui --test todo_question question_dialog_key_behaviors -- --nocapture
```

Expected: PASS.

- [ ] **Step 4: Delete covered small tests**

Delete these individual tests only after the table-driven test passes:

- `question_dialog_tab_navigation_through_keys`
- `question_dialog_number_key_selection`
- `question_dialog_down_moves_one_option_at_a_time`

Keep these tests:

- `question_overlay_renders_in_live_tui_frame`
- `question_overlay_lines_fit_terminal_width`
- `focused_dialog_input_drives_question_dialog_other_text`
- `question_dialog_full_flow_two_questions`
- `state_machine_multi_select_other_option`
- `state_machine_scroll_sync`
- `select_visible_prioritises_in_progress_and_latest_done`

- [ ] **Step 5: Merge cancel tests**

Replace `app_cancel_question_returns_id`, `app_closes_question_overlay_by_question_id`, and `question_dialog_esc_cancels` with:

```rust
#[test]
fn question_dialog_cancel_paths_close_overlay() {
    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-cancel", make_single_question());
    assert_eq!(app.cancel_question(), Some("q-cancel".to_owned()));
    assert!(!app.question_dialog_is_focused());

    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-close", make_single_question());
    assert!(app.close_question_overlay("q-close").is_some());
    assert!(!app.question_dialog_is_focused());

    let mut app = NeoChromeState::new("neo", "s1", "m1", "/tmp/ws");
    app.push_question_overlay("q-esc", make_single_question());
    let action = app
        .handle_question_dialog_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .expect("question handles esc");
    assert_eq!(action, QuestionDialogAction::Cancel);
    assert!(!app.question_dialog_is_focused());
}
```

- [ ] **Step 6: Run todo_question tests**

```bash
rtk cargo test -p neo-tui --test todo_question -- --nocapture
```

Expected: PASS. Test count should be lower by about 4-5.

- [ ] **Step 7: Commit**

```bash
git add crates/neo-tui/tests/todo_question.rs
git commit -m "test(tui): consolidate question dialog cases"
```

---

## Task 4: Consolidate Primitive ANSI/Wrap Tests

**Files:**
- Modify: `crates/neo-tui/tests/primitives.rs`
- Do not modify production code.

**Target groups:**

- `wrap_width_preserves_display_width_for_wide_text`
- `visible_width_ignores_ansi_csi_and_osc_sequences`
- `wrap_width_preserves_ansi_sequences_without_counting_them`
- `wrap_width_rehydrates_active_ansi_style_on_continuation_lines`
- `wrap_width_rehydrates_multiple_active_ansi_styles_on_continuation_lines`
- `wrap_width_rehydrates_sgr_sequences_that_reset_then_set_style`
- `wrap_width_stops_rehydrating_style_after_reset`
- `truncate_width_does_not_split_ansi_or_osc_sequences`
- `truncate_width_is_display_width_safe_and_can_pad`

- [ ] **Step 1: Run current primitives tests**

```bash
rtk cargo test -p neo-tui --test primitives -- --nocapture
```

Expected: PASS.

- [ ] **Step 2: Add table-driven visible width test**

Create one test:

```rust
#[test]
fn ansi_width_cases_are_display_width_safe() {
    struct Case {
        name: &'static str,
        input: &'static str,
        width: usize,
        expected_width: usize,
    }

    let cases = [
        Case {
            name: "plain ascii",
            input: "hello",
            width: 10,
            expected_width: 5,
        },
        Case {
            name: "ansi sgr ignored",
            input: "\x1b[31mred\x1b[0m",
            width: 10,
            expected_width: 3,
        },
        Case {
            name: "osc ignored",
            input: "\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\",
            width: 10,
            expected_width: 4,
        },
        Case {
            name: "wide cjk",
            input: "你好",
            width: 10,
            expected_width: 4,
        },
    ];

    for case in cases {
        assert_eq!(
            neo_tui::ansi::visible_width(case.input),
            case.expected_width,
            "{}",
            case.name
        );
        for line in neo_tui::components::wrap_width(case.input, case.width) {
            assert!(
                neo_tui::ansi::visible_width(&line) <= case.width,
                "{} overflowed: {line:?}",
                case.name
            );
        }
    }
}
```

If function paths differ, use the imports already present in `primitives.rs`.

- [ ] **Step 3: Delete only cases fully covered by the new table**

Delete small tests that only repeat visible-width assertions now covered by the table.

Do not delete rehydration tests unless the new table asserts exact ANSI style continuation. Rehydration is easy to regress.

- [ ] **Step 4: Run primitives tests**

```bash
rtk cargo test -p neo-tui --test primitives -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-tui/tests/primitives.rs
git commit -m "test(tui): consolidate primitive width cases"
```

---

## Task 5: Consolidate CLI Session ID Rejection Tests

**Files:**
- Modify: `crates/neo-agent/tests/cli_commands.rs`
- Do not modify CLI production code.

**Target tests:**

- `sessions_reject_incomplete_session_ids_without_guessing`
- `sessions_reject_path_traversal_ids`
- `sessions_accept_exact_workspace_bucket_ids`
- Similar RPC tests must be left for Task 6.

- [ ] **Step 1: Run current CLI tests for sessions**

```bash
rtk cargo test -p neo-agent --test cli_commands sessions_ -- --nocapture
```

Expected: session-related CLI tests pass.

- [ ] **Step 2: Add a helper for running session command failures**

In `crates/neo-agent/tests/cli_commands.rs`, find existing helpers for invoking `neo`. Add a local helper near session tests:

```rust
fn assert_session_command_rejects(args: &[&str], expected: &str) {
    let output = run_neo(args);
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded: {args:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(expected),
        "expected {expected:?} in stderr for {args:?}, got {stderr}"
    );
}
```

Use the actual command helper name from the file instead of `run_neo`.

- [ ] **Step 3: Replace repeated rejection tests with one table**

```rust
#[test]
fn sessions_reject_invalid_session_ids() {
    let cases = [
        (["sessions", "show", "abc"].as_slice(), "incomplete"),
        (["sessions", "show", "../secret"].as_slice(), "invalid"),
        (["sessions", "resume", "../secret"].as_slice(), "invalid"),
    ];

    for (args, expected) in cases {
        assert_session_command_rejects(args, expected);
    }
}
```

Adjust command names and expected strings to match current tests.

- [ ] **Step 4: Keep the exact-ID acceptance test**

Do not delete `sessions_accept_exact_workspace_bucket_ids` unless its assertion is copied into another positive-path session test.

- [ ] **Step 5: Run CLI session tests**

```bash
rtk cargo test -p neo-agent --test cli_commands sessions_ -- --nocapture
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/neo-agent/tests/cli_commands.rs
git commit -m "test(agent): consolidate invalid session id CLI cases"
```

---

## Task 6: Consolidate RPC Session ID Rejection Tests

**Files:**
- Modify: `crates/neo-agent/tests/rpc_mode.rs`

**Target tests:**

- `rpc_get_messages_rejects_incomplete_session_id`
- `rpc_get_messages_reports_missing_session_as_invalid_params`
- `rpc_sessions_get_rejects_incomplete_session_id`
- `rpc_sessions_get_reports_missing_session_as_invalid_params`

- [ ] **Step 1: Run current RPC session tests**

```bash
rtk cargo test -p neo-agent --test rpc_mode rpc_ -- --nocapture
```

Expected: PASS.

- [ ] **Step 2: Add a table-driven RPC invalid session test**

Create:

```rust
#[test]
fn rpc_session_methods_reject_invalid_or_missing_ids() {
    struct Case {
        method: &'static str,
        session_id: &'static str,
        expected: &'static str,
    }

    let cases = [
        Case {
            method: "get_messages",
            session_id: "abc",
            expected: "incomplete",
        },
        Case {
            method: "get_messages",
            session_id: "0000000000000",
            expected: "not found",
        },
        Case {
            method: "sessions.get",
            session_id: "abc",
            expected: "incomplete",
        },
        Case {
            method: "sessions.get",
            session_id: "0000000000000",
            expected: "not found",
        },
    ];

    for case in cases {
        let response = call_rpc_with_session_id(case.method, case.session_id);
        assert!(
            response_contains_error(&response, case.expected),
            "{} {} expected {:?}, got {response:?}",
            case.method,
            case.session_id,
            case.expected
        );
    }
}
```

Replace helper names with existing helpers in `rpc_mode.rs`.

- [ ] **Step 3: Delete old covered tests**

Delete the four target tests after the new one passes.

- [ ] **Step 4: Run RPC tests**

```bash
rtk cargo test -p neo-agent --test rpc_mode -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent/tests/rpc_mode.rs
git commit -m "test(agent): consolidate RPC session error cases"
```

---

## Task 7: Consolidate Provider Retry Tests Conservatively

**Files:**
- Modify: `crates/neo-ai/tests/real_provider_adapters.rs`

**Target tests:**

- `openai_responses_client_retries_retryable_http_responses`
- `anthropic_messages_client_retries_retryable_http_responses`
- `google_generative_ai_client_retries_retryable_http_responses`

- [ ] **Step 1: Run retry tests before editing**

```bash
rtk cargo test -p neo-ai retries_retryable_http_responses -- --nocapture
```

Expected: 3 retry tests pass.

- [ ] **Step 2: Extract shared assertion helper only**

Do not merge provider-specific payload tests. Only extract duplicate retry server setup if it is identical.

Add helper:

```rust
async fn assert_retry_count<F, Fut>(expected_attempts: usize, run: F)
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server = start_retry_server(attempts.clone()).await;
    run(server.url()).await;
    assert_eq!(
        attempts.load(std::sync::atomic::Ordering::SeqCst),
        expected_attempts
    );
}
```

Use existing server helper names if present. If no shared server helper exists, stop this task and skip provider retry consolidation.

- [ ] **Step 3: Rewrite each retry test to use the helper**

Keep three separate test functions. This is intentionally conservative.

Example shape:

```rust
#[tokio::test]
async fn openai_responses_client_retries_retryable_http_responses() {
    assert_retry_count(2, |base_url| async move {
        let client = openai_responses_client(base_url);
        let result = client.chat(test_request()).await;
        assert!(result.is_ok());
    })
    .await;
}
```

- [ ] **Step 4: Run provider tests**

```bash
rtk cargo test -p neo-ai --test real_provider_adapters retries_retryable_http_responses -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-ai/tests/real_provider_adapters.rs
git commit -m "test(ai): share provider retry assertions"
```

---

## Task 8: Consolidate Runtime Capability Rejection Tests

**Files:**
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

**Target tests:**

- `runtime_rejects_tools_when_model_lacks_tools_before_request`
- `runtime_rejects_image_content_when_model_lacks_images_before_request`
- `runtime_rejects_reasoning_effort_when_model_lacks_reasoning_before_request`

**Warning:** This is higher risk than xtask/TUI tests. Do not edit runtime production code.

- [ ] **Step 1: Run current target tests**

```bash
rtk cargo test -p neo-agent-core --test runtime_turn runtime_rejects_ -- --nocapture
```

Expected: target rejection tests pass.

- [ ] **Step 2: Add a helper that runs one rejection scenario**

Find the repeated setup in the three target tests. Extract only the setup that is identical.

Helper shape:

```rust
async fn assert_runtime_rejects_request(
    request: TurnRequest,
    expected_message: &str,
) {
    let result = run_runtime_turn(request).await;
    let error = result.expect_err("request should be rejected");
    assert!(
        error.to_string().contains(expected_message),
        "expected {expected_message:?}, got {error}"
    );
}
```

Use the real request/runtime helper types from `runtime_turn.rs`.

- [ ] **Step 3: Keep three separate tests**

Do not merge these into one test. The names are useful, and capability regressions are important. Only reduce repeated setup lines.

- [ ] **Step 4: Run runtime rejection tests**

```bash
rtk cargo test -p neo-agent-core --test runtime_turn runtime_rejects_ -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent-core/tests/runtime_turn.rs
git commit -m "test(runtime): share capability rejection setup"
```

---

## Task 9: Consolidate Runtime Approval Decision Mirror Tests

**Files:**
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

**Target tests:**

- `runtime_executes_ask_permission_tool_after_approval_hook_allows_it`
- `runtime_skips_ask_permission_tool_after_approval_hook_denies_it`
- `runtime_executes_ask_permission_tool_after_async_approval_wait_allows_it`
- `runtime_skips_ask_permission_tool_after_async_approval_wait_denies_it`

**Warning:** Keep sync and async approval paths separate unless the helper is extremely obvious.

- [ ] **Step 1: Run target approval tests**

```bash
rtk cargo test -p neo-agent-core --test runtime_turn approval -- --nocapture
```

Expected: approval-related runtime tests pass.

- [ ] **Step 2: Extract assertion helper**

Add:

```rust
fn assert_tool_executed_or_skipped(events: &[AgentEvent], should_execute: bool) {
    let executed = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionStarted { .. } | AgentEvent::ToolExecutionCompleted { .. }
        )
    });
    assert_eq!(executed, should_execute);
}
```

Adjust event variant names to the actual variants in `runtime_turn.rs`.

- [ ] **Step 3: Replace duplicated assertions only**

Do not merge the four tests unless the entire body becomes tiny and still readable.

- [ ] **Step 4: Run approval tests**

```bash
rtk cargo test -p neo-agent-core --test runtime_turn approval -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent-core/tests/runtime_turn.rs
git commit -m "test(runtime): share approval decision assertions"
```

---

## Task 10: Interactive Controller Test Triage Only

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive.rs`

**Goal:** Do not do a broad rewrite. Only consolidate very small mirror tests.

**Safe target groups:**

- `slash_model_does_not_enter_streaming_mode`
- `slash_provider_does_not_enter_streaming_mode`
- `event_loop_escape_closes_slash_completion_without_exiting`
- `event_loop_escape_is_noop_when_idle`
- `event_loop_ctrl_c_clears_prompt_before_confirming_exit`
- `event_loop_ctrl_c_requires_second_press_to_exit_when_prompt_is_empty`

**Do not touch in this pass:**

- approval tests
- question dialog tests
- session fork/resume tests
- interrupt/cancellation drain tests
- command palette export tests

- [ ] **Step 1: Run selected tests before editing**

```bash
rtk cargo test -p neo-agent interactive::tests::slash_model_does_not_enter_streaming_mode -- --nocapture
rtk cargo test -p neo-agent interactive::tests::slash_provider_does_not_enter_streaming_mode -- --nocapture
rtk cargo test -p neo-agent interactive::tests::event_loop_escape_is_noop_when_idle -- --nocapture
```

Expected: all pass.

- [ ] **Step 2: Merge slash no-streaming tests**

Replace the two slash tests with:

```rust
#[tokio::test]
async fn slash_picker_commands_do_not_enter_streaming_mode() {
    for command in ["/model", "/provider"] {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text(command);
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("slash command handled");
        assert!(
            controller.active_turn.is_none(),
            "{command} should not start a model turn"
        );
    }
}
```

Adjust private field access if needed. If `active_turn` is not accessible in the test module, use the existing assertion from the old tests.

- [ ] **Step 3: Run merged test**

```bash
rtk cargo test -p neo-agent interactive::tests::slash_picker_commands_do_not_enter_streaming_mode -- --nocapture
```

Expected: PASS.

- [ ] **Step 4: Stop after this small merge**

Do not keep editing `interactive.rs` in this task. It is too easy to over-collapse useful behavior tests.

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent/src/modes/interactive.rs
git commit -m "test(agent): merge slash picker no-streaming cases"
```

---

## Task 11: Final Measurement And Verification

**Files:**
- Modify: `docs/superpowers/plans/2026-06-20-test-slimming-measurements.md` if it was created in Task 0.

- [ ] **Step 1: Re-run source test count scan**

```bash
node - <<'NODE'
const {execFileSync}=require('child_process');
const out=execFileSync('rg',['-n','#\\[(tokio::)?test\\]','crates','xtask','examples','--glob','*.rs'],{encoding:'utf8'});
const rows=out.trim().split('\n').filter(Boolean).map(l=>l.split(':')[0]);
const byFile=new Map();
const byCrate=new Map();
for (const file of rows) {
  byFile.set(file,(byFile.get(file)||0)+1);
  const m=file.match(/^(crates\/[^\/]+|xtask|examples\/rust)/);
  const crate=m?m[1]:'other';
  byCrate.set(crate,(byCrate.get(crate)||0)+1);
}
console.log(JSON.stringify({
  total: rows.length,
  byCrate: [...byCrate].sort((a,b)=>b[1]-a[1]),
  topFiles: [...byFile].sort((a,b)=>b[1]-a[1]).slice(0,20)
}, null, 2));
NODE
```

Expected:

- Total count lower than baseline.
- Most reduction should come from `xtask/src/main.rs`, `todo_question.rs`, `cli_commands.rs`, or `rpc_mode.rs`.

- [ ] **Step 2: Run focused suites touched by this plan**

```bash
rtk cargo test -p xtask -- --nocapture
rtk cargo test -p neo-tui --test todo_question -- --nocapture
rtk cargo test -p neo-tui --test primitives -- --nocapture
rtk cargo test -p neo-agent --test cli_commands sessions_ -- --nocapture
rtk cargo test -p neo-agent --test rpc_mode -- --nocapture
rtk cargo test -p neo-ai --test real_provider_adapters retries_retryable_http_responses -- --nocapture
rtk cargo test -p neo-agent-core --test runtime_turn runtime_rejects_ -- --nocapture
rtk cargo test -p neo-agent-core --test runtime_turn approval -- --nocapture
```

Expected: all pass. If one command is slow, wait. If one fails, stop and fix only the related task.

- [ ] **Step 3: Run formatting and diff checks**

```bash
rtk cargo fmt --all --check
rtk git diff --check
```

Expected: both pass.

- [ ] **Step 4: Commit measurements if used**

If the measurements file was maintained:

```bash
git add docs/superpowers/plans/2026-06-20-test-slimming-measurements.md
git commit -m "docs(test): record test slimming results"
```

- [ ] **Step 5: Report final result**

Report:

- Starting test function count
- Ending test function count
- Net removed/merged test functions
- Files touched
- Verification commands and results
- Any skipped task and why

---

## What Not To Do

Do not do these in the conservative pass:

- Do not delete `runtime_turn.rs` approval/cancellation tests outright.
- Do not merge provider wire-format tests across OpenAI/Anthropic/Google if payload assertions differ.
- Do not remove CLI tests that exercise actual file/session side effects unless the same side effect is asserted elsewhere.
- Do not change `xtask` parity production logic to make table tests easier.
- Do not chase doctest or cargo binary count accuracy before reducing obvious duplicate source tests.
- Do not add snapshot testing or new dependencies.

## Suggested Conservative Stop Point

Stop after Tasks 1-6 if the net reduction is already 40+ test functions. That is enough for one safe pass. Tasks 7-10 are more valuable for line reduction than raw test-count reduction and should be done only when the earlier tasks are clean.

## Self-Review Checklist

- Every deleted test has its assertion preserved in another test.
- Every commit touches one domain only.
- No production logic changed unless it was a trivial helper visibility/import needed by tests.
- All commands listed in the task passed after the edit.
- The worker did not run prohibited git mutation commands.
