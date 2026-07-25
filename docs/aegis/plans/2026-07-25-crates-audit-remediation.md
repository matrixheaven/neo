# Neo 2026-07-25 Crates Audit Remediation Implementation Plan

> Executor note: the design is approved. Execute it; do not repeat the full
> audit, reopen approved product decisions, edit `.references/`, preserve old
> internal owners for compatibility, or absorb unrelated worktree changes.

## Goal

Implement every contract in
`docs/aegis/specs/2026-07-25-crates-audit-remediation-design.md` through its
canonical owner, delete the defective/duplicate path, verify each logical task
with the narrowest exact evidence, and commit each task independently.

## Non-Negotiable Boundaries

- Built-in Anthropic supports API keys only. Delete `ANTHROPIC_OAUTH_TOKEN`;
  add no Bearer/OAuth heuristic, adapter, alias, or fallback.
- `ListDelegates` is the sole delegate/swarm discovery owner. `TaskList` is
  metadata-only for bash/question/workflow; only `TaskOutput` reads logs.
- Plan replay uses persisted result details only. Legacy events without details
  render a header and no body; never read live disk to infer history.
- Delegate, DelegateGroup, and DelegateSwarm cards remain byte/layout/behavior
  equivalent. Do not edit their presentation to satisfy these tasks.
- Bash/Terminal admission remains pending while queued. Commands without an
  explicit timeout/cancel remain unbounded. Clipboard deadlines do not apply to
  ShellRuntime.
- No durable event/schema migration, compatibility branch, new hosted service,
  unsafe Neo code, or predictive resource governance.
- Tests follow `AGENTS.md`: one package, one target selector, one precise
  filter. No broad `cargo test` or package-wide `nextest` as evidence.
- Do not modify `.references/`, reset/restore/stash/clean/rebase, or revert
  another agent's work. Push/branch/worktree operations require authorization.

## TDD Route

Mode: `off / skipped`. Implement the minimum canonical repair, then add the
smallest regression that fails on the previous behavior. Pure deletion tasks
do not gain synthetic seams merely to count calls.

## Subagent Execution Model

The root agent must use at least three implementation subagents. The safe
initial wave is five fully disjoint single-task leases: Task 1, Task 5, Task 10,
Task 13, and Task 4. Do not give one subagent a multi-task batch to implement in
one turn.

After each task, its subagent must return to the root. The root reviews, runs
fresh verification, stages exact paths, and commits that task before sending a
follow-up task to the same agent. Suggested follow-up queues are:

- provider agent: Task 1 -> Task 2 -> Task 3;
- workflow/session agent: Task 5 -> Task 6 -> Task 8 -> Task 7;
- coordination agent: Task 10, then wait for Task 7 commit -> Task 9 -> Task 11;
- TUI agent: Task 13 -> Task 14 -> Task 17 -> Task 12;
- agent platform agent: Task 4 -> Task 16, then wait for Task 12 commit -> Task 15.

Task 7 precedes Task 9 because both edit `multi_agent/runtime.rs`. Task 7 also
precedes Task 11 because both may edit `multi_agent_background.rs`. Task 17
precedes Task 12 because both touch transcript pane code. Task 12 precedes Task
15 because both may touch `interactive/tests.rs`. No two live subagents may
edit the same file. Task 18 runs last after all production repairs.

## Task 1: Harden Provider Credential Resolution

**Findings:** F1, F2.
**Commit:** `fix(ai): harden provider credential resolution`

**Files:** `crates/neo-ai/src/registry.rs`,
`crates/neo-ai/tests/provider_resolver.rs`.

1. Add private `collect_utf8_environment` over `std::env::vars_os()`; retain a
   pair only when key and value convert losslessly to UTF-8.
2. Route `ProviderRegistry::resolver` through it; do not mutate global env or
   use lossy conversion.
3. Built-in Anthropic lists only `ANTHROPIC_API_KEY`. Preserve inline key,
   explicit `api_key_env`, precedence, and `x-api-key` behavior.
4. Delete pseudo-OAuth positive tests; add one non-UTF-8 fixture and one
   built-in registry assertion.

```bash
rtk cargo nextest run -p neo-ai --lib collect_utf8_environment_skips_non_utf8_pairs
rtk cargo nextest run -p neo-ai --test provider_resolver production_registry_uses_only_anthropic_api_key
rtk rg -n 'std::env::vars\(' crates/neo-ai/src/registry.rs
rtk rg -n 'ANTHROPIC_OAUTH_TOKEN' crates --glob '*.rs'
```

Both scans must be empty. Stop if the repair needs a public credential type,
OAuth/header branch, lossy conversion, or process-env mutation.

## Task 2: Use The Canonical Anthropic Tool Assembler

**Finding:** F3.
**Commit:** `fix(ai): unify anthropic tool call assembly`

**Files:** `crates/neo-ai/src/providers/anthropic.rs`,
`crates/neo-ai/tests/real_provider_adapters.rs`.

1. Replace `tool_args` and `block_tool_ids` with
   `StreamingToolCallAssembler`.
2. Translate content-block start/delta to `ToolCallChunk`; propagate assembly
   errors as protocol errors and finish through `finish_all()`.
3. Missing tool name must produce one protocol error and no args/end event.
4. Reuse the OpenAI-compatible mapping pattern without copying buffer state.

```bash
rtk cargo nextest run -p neo-ai --test real_provider_adapters anthropic_missing_tool_name_is_protocol_error_without_tool_lifecycle_events
rtk cargo nextest run -p neo-ai --test real_provider_adapters anthropic_messages_client_posts_messages_payload_and_streams_events
rtk rg -n 'tool_args|block_tool_ids' crates/neo-ai/src/providers/anthropic.rs
```

The scan must be empty. Stop if the shared assembler contract must change or a
second argument buffer would remain.

## Task 3: Bound And Classify Catalog Responses

**Finding:** F4.
**Commit:** `fix(ai): bound and classify catalog responses`

**Files:** `crates/neo-ai/src/providers/mod.rs`,
`crates/neo-ai/src/catalog.rs`.

1. Re-export the existing `http_status_error` crate-privately; do not duplicate
   status mapping.
2. Non-2xx responses use it and retain best-effort bounded diagnostic bodies.
3. Add one `CATALOG_BODY_LIMIT_BYTES = 16 * 1024 * 1024`; check declared size
   early and count actual chunks regardless of the header.
4. Oversize/JSON decode is Protocol; successful-body chunk failure is
   Transport; known non-2xx status remains authoritative.

```bash
rtk cargo nextest run -p neo-ai --lib catalog_http_errors_use_shared_status_classification
rtk cargo nextest run -p neo-ai --lib oversized_chunked_catalog_response_is_rejected
rtk cargo nextest run -p neo-ai --lib stalled_catalog_response_hits_request_deadline
rtk rg -n 'catalog fetch returned|resp\.json' crates/neo-ai/src/catalog.rs
```

The scan must be empty. Stop for a new dependency, copied classifier, public
API, or evidence that a required catalog legitimately exceeds 16 MiB.

## Task 4: Delete Duplicate Catalog Fetches

**Finding:** F5.
**Commit:** `fix(agent): avoid duplicate catalog fetches`

**File:** `crates/neo-agent/src/modes/interactive/catalog_fetch.rs`.

Delete exactly the two detached `_handle` spawns in `fetch_known_catalog` and
`handle_api_key_submitted`. Keep the one handle stored in
`pending_catalog_fetch`. Add no abstraction, counter, or timeout.

```bash
rtk rg -n 'let _handle = tokio::spawn' crates/neo-agent/src/modes/interactive/catalog_fetch.rs
rtk proxy git diff --check -- crates/neo-agent/src/modes/interactive/catalog_fetch.rs
```

No test is required for this pure deletion. Stop if no unique tracked handle
remains or the detached task is proven to own distinct behavior.

## Task 5: Serialize Session Metadata Mutations

**Finding:** F6.
**Commit:** `fix(core): serialize session metadata mutations`

**File:** `crates/neo-agent-core/src/session/mod.rs` and its unit tests.

1. Add one private `mutate_metadata` using the existing cross-process locking
   primitive on a stable sibling sidecar, never the replaced metadata file.
2. Lock, reread, mutate, and atomic-replace in that order.
3. Route rename, summary, activity, title, and all other mutators through it.
4. Hold the same lock across fork ID allocation, directory copy/publication,
   metadata commit, and rollback.
5. On any failure preserve the prior metadata and clean unpublished child data.

```bash
rtk cargo nextest run -p neo-agent-core --lib metadata_mutation_sidecar_serializes_and_preserves_previous_file
rtk rg -n 'self\.read_metadata|self\.write_metadata|next_child_id|record_fork_metadata' crates/neo-agent-core/src/session/mod.rs
```

Manually confirm remaining hits are inside the canonical transaction/read-only
paths. Stop if this needs unsafe code/new dependency or cannot preserve fork
atomicity on Windows, Linux, and macOS.

## Task 6: Supervise Workflow Worker Panics

**Finding:** F7 workflow half.
**Commit:** `fix(core): supervise workflow worker panics`

**Files:** `crates/neo-agent-core/src/workflow/runtime.rs`,
`crates/neo-agent-core/tests/workflow_runtime.rs`.

1. Retain and supervise the runner `JoinHandle` in `WorkflowRuntime`.
2. On panic, first append an interrupted terminal outcome for any open
   invocation with `details.reason = "worker_panicked"`.
3. Clear `current_invocation`, then transition the workflow to Failed.
4. Persistence failure follows existing recovery-failure behavior. Never retry
   the effect and do not add a journal shape/enum variant.

```bash
rtk cargo nextest run -p neo-agent-core --test workflow_runtime workflow_worker_panic_finishes_invocation_before_failed_state
rtk rg -n 'worker_panicked|current_invocation|finish_worker' crates/neo-agent-core/src/workflow/runtime.rs
```

Stop unless the invocation outcome is durable before workflow terminalization.

## Task 7: Terminalize Panicked Delegate Workers

**Finding:** F7 delegate/swarm half.
**Commit:** `fix(core): terminalize panicked delegate workers`

**Files:** `crates/neo-agent-core/src/tools/delegate.rs`,
`crates/neo-agent-core/src/multi_agent/runtime.rs`,
`crates/neo-agent-core/tests/multi_agent_background.rs`.

1. `MultiAgentRuntime` owns panic terminalization; BackgroundTaskManager only
   mirrors the resulting state.
2. Delegate panic produces canonical Failed/`worker_panicked` state.
3. Swarm panic terminalizes every queued/running child before the final swarm
   snapshot is registered.
4. Preserve WaitDelegate, TaskOutput, and all Delegate-family UI contracts.

```bash
rtk cargo nextest run -p neo-agent-core --test multi_agent_background background_worker_panics_terminalize_delegate_and_swarm
rtk rg -n 'worker_panicked|finish_delegate|finish_delegate_swarm' crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent-core/src/multi_agent/runtime.rs
```

Stop if any child remains Running, terminal state exists only in the background
manager, or presentation code would need modification.

## Task 8: Isolate Workflow Recovery Failures Per Run

**Finding:** F8.
**Commit:** `fix(core): isolate workflow rehydration failures`

**Files:** `crates/neo-agent-core/src/workflow/runtime.rs`,
`crates/neo-agent-core/tests/workflow_runtime.rs`.

Contain metadata, journal parse/open, resolver, and recovery-append errors in
`rehydrate_run_entry`: insert an inspectable failed handle and continue ordered
sibling recovery. Only workflows-root enumeration or registry invariants may
fail the whole rehydrate. Never skip/delete a bad run or resume its effect.

```bash
rtk cargo nextest run -p neo-agent-core --test workflow_runtime rehydrate_isolates_recovery_append_failure
rtk rg -n 'rehydrate_run_entry\(entry.*await\?|JournalWriter::open.*\?|bound_recovery_resolver\(\)\?' crates/neo-agent-core/src/workflow/runtime.rs
```

The scan must show no session-wide propagation for run-local failures.

## Task 9: Make Delegate Message Delivery Atomic

**Finding:** F9.
**Commit:** `fix(core): make delegate message delivery atomic`

**Files:** `crates/neo-agent-core/src/multi_agent/runtime.rs`,
`crates/neo-agent-core/src/tools/delegate_controls.rs`,
`crates/neo-agent-core/src/runtime/queue.rs`, and local unit tests.

1. Validate generation and synchronously enqueue in one live-registry
   operation. Do not hold a global state lock across `.await`.
2. Return typed `Delivered | NotRunning | Unknown`; enqueue failure cannot be
   reported as Delivered.
3. Delete tool-layer snapshot prechecks; swarm broadcast calls the same
   primitive per child. Do not add an offline mailbox.

```bash
rtk cargo nextest run -p neo-agent-core --lib live_delivery_unregister_race_never_reports_false_delivered
rtk rg -n 'agent_snapshot\(&input\.id\)|deliver_live_message.*bool|status: delivered' crates/neo-agent-core/src/tools/delegate_controls.rs crates/neo-agent-core/src/multi_agent/runtime.rs
```

Stop if queue acceptance is unobservable or a caller fallback is required.

## Task 10: Classify MCP Authentication Structurally

**Finding:** F10.
**Commit:** `fix(core): classify MCP authentication structurally`

**Files:** `crates/neo-agent-core/src/tools/mcp/http.rs`,
`crates/neo-agent-core/src/tools/mcp_manager.rs` and local unit tests.

Map typed rmcp HTTP/OAuth auth-required sources to
`McpErrorKind::NeedsAuth`. Manager status, hint, and reconnect suppression read
only the kind. Delete Display matching for `401`, `Unauthorized`, and similar
phrases; text remains presentation-only.

```bash
rtk cargo nextest run -p neo-agent-core --lib typed_http_auth_error_maps_to_needs_auth_without_text_matching
rtk cargo nextest run -p neo-agent-core --lib ordinary_error_text_containing_401_remains_protocol
rtk rg -n 'contains\("(401|Unauthorized|Auth required|auth_required)|diagnostic_hint\(message' crates/neo-agent-core/src/tools/mcp/http.rs crates/neo-agent-core/src/tools/mcp_manager.rs
```

Only negative-test strings may remain. Stop if rmcp exposes no typed source and
the only possible implementation is text matching or a dependency upgrade.

## Task 11: Separate Task And Delegate Discovery

**Finding:** F11.
**Commit:** `refactor(core): separate task and delegate discovery`

**Files:** `crates/neo-agent-core/src/tools/background_tasks.rs`,
`crates/neo-agent-core/tests/multi_agent_background.rs` as needed.

1. Add manager-owned metadata-only enumeration; listing never hydrates logs.
2. `TaskList` filters manager delegate/swarm records before sort/limit and
   lists only bash/question/workflow metadata.
3. Delete runtime delegate synthesis, dedupe state, three synthesis-positive
   tests, and delegate/swarm claims in the model-visible TaskList description.
4. Keep TaskOutput/TaskStop adapters; `ListDelegates` is the only discovery API.

```bash
rtk cargo nextest run -p neo-agent-core --lib task_list_uses_metadata_only_enumeration_and_excludes_delegates
rtk cargo nextest run -p neo-agent-core --test multi_agent_background list_delegates_reports_background_delegate
rtk rg -n 'runtime_delegate_task_snapshots|existing_ids|task_list_tool_(includes_active_runtime|deduplicates_delegate)' crates/neo-agent-core/src/tools/background_tasks.rs
```

The scan must be empty. Stop if listing still reads `.log` or another delegate
discovery path remains.

## Task 12: Replay Plans Only From Persisted Details

**Finding:** F12.
**Commit:** `fix(tui): replay plans from persisted result details`

**Files:** `crates/neo-tui/src/transcript/pane.rs`,
`crates/neo-agent/src/modes/interactive/tests.rs`,
`crates/neo-tui/tests/tool_cards.rs` for obsolete-test removal.

Delete `ReplayPlanSnapshot`, Write/Edit argument inference, batch-first-edit
logic, and replay-time filesystem reads. Render persisted result details; if
absent, render only the header. Do not change event/result schema.

```bash
rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::replay_exit_plan_mode_uses_only_persisted_snapshot_details --exact --nocapture --include-ignored
rtk rg -n 'ReplayPlanSnapshot|replay_plan_snapshot|remember_replay_plan_snapshot|replay_tool_result_details|looks_like_plan_file_path' crates/neo-tui crates/neo-agent
```

The scan must be empty. Stop if persisted details are not present in the real
session event path; do not add another snapshot event or fallback.

## Task 13: Keep Notification Failures Observable And Retryable

**Finding:** F13.
**Commit:** `fix(tui): keep notification failures observable and retryable`

**Files:** `crates/neo-tui/src/notify.rs`, `crates/neo-tui/Cargo.toml`.

1. Add existing workspace `tracing` as a direct dependency.
2. Replace diagnostic `eprintln!` with once-only `tracing::warn!`; keep Bell's
   intentional BEL write unchanged.
3. Unsupported/permanent spawn errors sticky-disable. Nonzero exit, wait error,
   or waiter-thread failure clears `in_flight` and permits retry.
4. Use only the smallest private completion transition needed for testing.

```bash
rtk cargo test --package neo-tui --lib -- notify::tests::notification_child_failure_is_retryable --exact --nocapture
rtk cargo test --package neo-tui --lib -- notify::tests::permanent_spawn_errors_disable_future_notifications --exact --nocapture
rtk cargo test --package neo-tui --lib -- notify::tests::desktop_notification_command_uses_platform_binary_without_shell --exact --nocapture
rtk rg -n 'eprintln!\("Neo notification|finish_desktop_notification\(diagnostic\.is_some\(\)\)' crates/neo-tui/src/notify.rs
```

Run the exact tests and existing platform-command test natively on macOS,
Linux, and Windows. Stop if observability requires frame writes,
`AgentEvent::Error`, or changing Bell.

## Task 14: Fail Windows Entry When Input Mode Is Unknown

**Finding:** F14.
**Commit:** `fix(tui): fail Windows entry when input mode is unknown`

**File:** `crates/neo-tui/src/screen_output/terminal_modes.rs`.

Propagate console mode-query failure. Add only a private function/closure seam
for deterministic query/set failure tests; preserve guard rollback and Unix
code. Run both exact tests in native Windows Terminal/ConPTY.

```bash
rtk cargo test --package neo-tui --lib -- screen_output::terminal_modes::windows_input_mode::tests::query_failure_aborts_entry --exact --nocapture
rtk cargo test --package neo-tui --lib -- screen_output::terminal_modes::windows_input_mode::tests::enable_and_restore_round_trip --exact --nocapture
rtk rg -n 'let Ok\(mode\) = winapi_util::console::mode' crates/neo-tui/src/screen_output/terminal_modes.rs
```

The scan must be empty. Check host memory before booting only the Windows VM;
shut it down afterward. Stop if no interactive console is available rather
than weakening failure into silent inactive mode.

## Task 15: Bound Clipboard Helper Execution

**Finding:** F15.
**Commit:** `fix(agent): bound clipboard helper execution`

**Files:** `crates/neo-agent/src/modes/interactive/clipboard.rs`, `mod.rs`,
`prompt_edit.rs`, `sessions.rs`, and `tests.rs`.

1. Convert the injected writer contract to one async future-returning path.
2. Use `tokio::process::Command`, async stdin, `kill_on_drop(true)`, and one
   private fixed deadline covering stdin plus child exit.
3. Controller owns one pending task, cancels it on replacement/exit, polls it
   through the existing pending-task chain, and reports completion status.
4. Prompt, transcript, and resume-command copy all use this path. Delete the
   synchronous `ClipboardWriter` implementation and callers.
5. Update the internal buffer immediately. Keep `pbcopy`, `clip.exe`, and
   `wl-copy -> xclip` selection. Tests inject a helper; they never require a
   desktop clipboard service.

```bash
rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::event_loop_clipboard_timeout_does_not_block_input --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::new_clipboard_copy_cancels_previous_write --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --bin neo -- modes::interactive::clipboard::tests::clipboard_command_timeout_kills_child --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --bin neo -- modes::interactive::clipboard::tests::clipboard_command_spec_uses_native_helper_without_shell --exact --nocapture --include-ignored
rtk rg -n 'std::process|wait_with_output|write_clipboard_stdin|wait_clipboard_command' crates/neo-agent/src/modes/interactive/clipboard.rs
```

The first two tests prove controller progress/replacement. The third must call
the production command runner with a controllable blocking child and prove the
child is gone after timeout/drop; merely observing an aborted future is
insufficient. Run the native command-spec test on macOS, Linux, and Windows.
The scan must be empty. Stop if cancellation cannot kill the child, a blocking
process path remains, or Bash/Terminal behavior would change.

## Task 16: Complete Native Windows Path Prefixes

**Finding:** F16.
**Commit:** `fix(agent): complete native Windows path prefixes`

**File:** `crates/neo-agent/src/modes/interactive/prompt_completion.rs`.

Use `std::path::is_separator` for filesystem prefixes and `@` query segments.
Preserve the user's separator style in inserted paths. Windows accepts `/` and
`\\`; Unix accepts only `/` and keeps backslash as a filename character. Do not
change canonical display values or expand into shell normalization.

```bash
rtk cargo test --package neo-agent --bin neo -- modes::interactive::prompt_completion::tests::completion_path_uses_native_separators --exact --nocapture --include-ignored
rtk rg -n "query\.rsplit\('/')|prefix\.rsplit_once\('/')" crates/neo-agent/src/modes/interactive/prompt_completion.rs
```

Run natively on Windows, Linux, and macOS. The stale scan must be empty.

## Task 17: Remove The Dead Inline-Image Side Channel

**Finding:** C1.
**Commit:** `refactor(tui): remove unused inline image side channel`

**Files:** delete `crates/neo-tui/src/shell/image_cache.rs`; edit
`shell/mod.rs`, `transcript/pane.rs`, `transcript/entry/mod.rs`,
`transcript/mod.rs`, `tests/image_protocols.rs`, and `tests/app_shell.rs`.

Delete `InlineImageRenderCache`, `InlineImageRender`, sequence/render accessors,
exports, and cache-only tests. Preserve the real terminal-image rendering path.

```bash
rtk cargo test --package neo-tui --test app_shell -- transcript_user_images_render_thumbnail_inside_normal_frame --exact --nocapture
rtk rg -n 'InlineImageRenderCache|InlineImageRender|inline_image_renders|inline_image_sequences|inline_image_render\(' crates/neo-tui
```

The scan must be empty. Stop if a non-test production consumer exists.

## Task 18: Collapse The Approved Duplicate Helpers

**Finding:** C2.
**Commit:** `refactor: collapse audited duplicate owners`

Run last. File-disjoint sub-batches may use subagents, but root integration,
formatting, tests, scans, and commit are serial.

1. `neo-tui/src/shell/dialog_dispatch.rs`: call concrete state `handle_input`
   directly; delete forwarding traits/helpers/impls.
2. `neo-tui/src/dialogs/trust.rs`: reuse choice-picker SGR foreground/background
   helpers; delete local tables/layer.
3. TUI box/token wrappers: call `content_line` and canonical token formatter;
   delete wrappers and only the three approved low-value tests.
4. Core paths: expose `workspace_policy::normalize_path` crate-privately;
   migrate plan/tool callers and delete duplicate normalizers.
5. Workflow dispatch: migrate `.replace` callers/tests to `.refresh`, rename the
   misleading test, then delete `replace` without alias.
6. XML: add one crate-private `xml_escape::{escape_text, escape_attribute}`;
   migrate four implementations while preserving text-vs-attribute semantics.
7. Agent wrappers: direct-call prompt-tree loader and canonical default-model
   comparison; delete wrappers.

```bash
rtk cargo nextest run -p neo-tui --lib shell::tests::confirm_dialog_result_is_available_after_approval_input
rtk cargo nextest run -p neo-tui --lib dialogs::trust::tests::renders_detected_inputs_without_file_contents
rtk cargo nextest run -p neo-tui --lib primitive::ansi_escape::tests::dynamic_colors_use_foreground_and_background_prefixes
rtk cargo nextest run -p neo-agent-core --lib mode::plan_mode_guard::tests::active_denies_write_and_edit
rtk cargo nextest run -p neo-agent-core --lib workspace_policy::tests::read_allows_primary_relative_path
rtk cargo nextest run -p neo-agent-core --test workflow_dispatch each_run_one_resolves_current_live_registry
rtk cargo nextest run -p neo-agent-core --lib skills::context::tests::render_skill_context_escapes_xml_special_chars_in_name_and_path
rtk cargo nextest run -p neo-agent-core --test shell_messages shell_command_message_escapes_xml_text_without_escaping_quotes
rtk cargo nextest run -p neo-agent --bin neo modes::interactive::tests::idle_model_and_provider_refreshes_bound_workflow_dispatch_client
rtk cargo nextest run -p neo-agent --bin neo prompt::templates::tests::project_prompt_loader_ignores_only_missing_directories
```

Retirement scans, all expected empty:

```bash
rtk rg -n 'DialogInputRef|DialogInputOwned|handle_input_ref|handle_input_owned' crates/neo-tui/src
rtk rg -n 'fn dialog_sgr|enum DialogSgrLayer' crates/neo-tui/src/dialogs/trust.rs
rtk rg -n 'fn normalize(_path)?\(' crates/neo-agent-core/src/mode/plan_mode_guard.rs crates/neo-agent-core/src/tools/mod.rs
rtk rg -n 'pub fn replace\(' crates/neo-agent-core/src/runtime/workflow_dispatch.rs
rtk rg -n 'fn escape_xml|fn escape_attribute|fn escape_xml_text|fn escape_xml_attr' crates/neo-agent-core/src/messages.rs crates/neo-agent-core/src/skills crates/neo-agent-core/src/instructions/resolver.rs
rtk rg -n 'side_bordered_line|format_tool_token_count|load_user_prompt_templates|configured_model_is_default' crates
```

Do not delete `WorkflowSnapshot` or `WorkflowStepRecord`. Stop on behavior drift
or if direct calls cannot replace a wrapper without a second owner.

## Per-Task Completion Gate

For each task, before commit:

1. Inspect `rtk git status --short` and separate unrelated changes.
2. Run the named exact regression(s) and retirement scans.
3. Run file-scoped `rtk rustfmt --check --edition 2024 <touched .rs files>`.
4. Run `rtk proxy git diff --check -- <exact task files>`.
5. Review the complete diff against the approved spec.
6. Stage only exact task files and commit with the prescribed message.
7. Record commit SHA and evidence before releasing an overlapping task.

Never revert another change to make verification pass. If baseline interference
prevents a task's exact proof, report the exact blocker and leave the task
uncommitted rather than widening scope.

## Platform Verification Gate

After all implementation commits:

- macOS: rerun the exact platform-neutral/provider/runtime tests affected by
  the final integration plus macOS notification/clipboard/completion tests.
- Linux: run exact notification, clipboard, completion, metadata locking, and
  recovery tests on a native Linux target.
- Windows: run exact env, metadata locking, notification, clipboard,
  completion, and VT input tests in Windows Terminal/ConPTY.
- VM policy: check host memory, boot only one Parallels VM at a time, and stop
  it when proof is complete. Keep any VM already running required work intact.

Native platform failures caused by the implementation are fixed in the owning
task. Pre-existing unrelated failures are documented, never repaired here.

## Final Integration Gate

1. Run every retirement scan from Tasks 1-18 again at final HEAD.
2. Confirm no Delegate-family card files changed and no ShellRuntime timeout or
   admission code changed.
3. Confirm `.references/` and unrelated user files are untouched.
4. Run `rtk cargo fmt --all --check` only as final formatting evidence; do not run a
   broad test suite.
5. Run `rtk proxy git diff --check` and inspect `rtk git status --short`.
6. Produce a task/commit/evidence/platform matrix and list residual risks.
7. Do not push without explicit user authorization.

## Stop Conditions

Stop the affected task and ask for direction when implementation would require:

- real Anthropic OAuth or any credential heuristic;
- a durable schema migration or historical live-disk fallback;
- a second lifecycle/state/persistence owner;
- unsafe Neo code, a new third-party dependency, or an rmcp upgrade;
- changing Delegate-family presentation or ShellRuntime waiting semantics;
- lossy native paths or silent Windows terminal degradation;
- destructive Git/worktree action or overwriting unrelated user changes.
