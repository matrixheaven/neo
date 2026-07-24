# Neo 2026-07-24 Crates Audit Remediation Implementation Plan

> Executor note: implement the approved design task by task. Do not repeat a
> full-repository audit, redesign the product, edit `.references`, retain a
> duplicate owner for compatibility, or absorb unrelated worktree changes.

## Goal

Remediate all fourteen high-confidence findings and the associated cleanup
findings from the 2026-07-24 crates audit. Each repair must land in the existing
canonical owner, retire the defective path, preserve durable user data, and be
proved through focused tests on macOS, Linux, and Windows where applicable.

## Architecture

```text
provider config -> ProviderSpec -> CredentialResolver -> provider client
RPC stdin -> request loop -> live JSONL writer -> RPC stdout
turn spawn -> AgentEventStream -> completion or cancel-on-drop
DelegateSwarm request -> one MultiAgentRuntime validation/commit transaction
HTTP chunks -> shared bounded SSE framer -> provider-specific event parser
tool deltas -> shared live-output tail + canonical ANSI parser -> cards
workflow/session writes -> session::atomic_file -> platform durability
native paths -> private lossless index wire / safe Windows native-path wrapper
shell guards -> Windows barrier with path in environment data
git -z bytes -> native PathBuf -> bounded regular-file line counter
```

No adapter layer is added beside an old implementation. Repair the owner,
migrate its direct consumers, and delete the superseded path in the same task.

## Tech Stack

- Rust 2024, minimum Rust 1.96.1, `unsafe_code = "forbid"`
- Tokio, `tokio-util::sync::CancellationToken`, serde/serde_json, url, base64
- existing `winsafe`, rustix, session, transcript, provider, and runtime owners
- standard-library `Path`, `PathBuf`, `OsString`, `VecDeque`, buffered I/O
- `cargo test`/`cargo nextest`, file-scoped rustfmt, `rg`, `git diff --check`
- native macOS host plus one-at-a-time Parallels Linux/Windows verification

## Baseline And Authority Refs

- `AGENTS.md`
- `docs/aegis/specs/2026-07-24-crates-audit-remediation-design.md`
- `docs/aegis/baseline/2026-07-23-runworkflow-runtime-contract.md`
- `docs/aegis/specs/2026-07-20-bash-terminal-tool-card-brief.md`
- current source and direct callers/tests named in each task

The design spec is approved. It is the requirement authority; this plan is the
execution map. Reference repositories are not implementation targets.

## Compatibility Boundary

- Delete the top-level/global provider credential path. Only
  `[providers.<id>]` supplies persistent provider credentials.
- Existing session-index Unicode records remain readable. New records use one
  versioned lossless native-path wire. Do not rewrite existing files.
- RPC method names, request/response payload meanings, and session persistence
  remain stable; delivery becomes incremental and request errors become local.
- RunWorkflow journal order, append-only durability, actual-usage-only cost
  semantics, and explicit human controls remain unchanged.
- Bash/Terminal admission waits remain pending. Commands without an explicit
  timeout/cancel remain unbounded.
- Delegate, DelegateGroup, and DelegateSwarm card layout, content, ordering,
  expansion, and transcript semantics remain exactly unchanged.
- No alias, fallback parser, feature flag, second writer, second sanitizer,
  second framer, or remove-then-rename path is authorized.

## TDD Route

- Mode: off.
- Decision: skipped.
- Strict authority: not applicable; strict test-first TDD was not requested.
- Test posture: implement the minimum owner repair, then add one focused
  regression for each broken behavior and retain stronger existing tests.
- Verification: every Rust test command names one package, one target selector,
  and a precise test filter. No broad workspace or package-wide test run is
  completion evidence.

## Execution Readiness View

- Intent Lock: repair the audited owner boundaries without product redesign.
- Scope Fence: only files named below and direct mechanical call sites.
- Owner Lock: the owner column in the design spec is authoritative.
- Compatibility Lock: only existing session-index read compatibility survives.
- Retirement Lock: old internal owners disappear in the same task as migration.
- Platform Lock: platform-only behavior requires native-platform evidence.
- Review Gates: one conventional commit per task; review and focused tests
  before beginning the next task.
- Stop Conditions: persistent-data migration, external compatibility evidence,
  need for unsafe Neo code, inability to find a safe native-path Windows API,
  or a conflict with unrelated dirty worktree changes.
- Drift Rule: return to the approved spec; do not invent a fallback to keep
  moving.

## Task 1: Make Provider Credentials Provider-Scoped

**Findings:** F1, F14.
**Commit:** `fix: make provider credentials provider-scoped`

**Files:**

- `crates/neo-agent/src/config/mod.rs`
- `crates/neo-agent/src/config/types.rs`
- `crates/neo-agent/src/config/loader.rs`
- `crates/neo-agent/src/config/mutations.rs`
- `crates/neo-agent/src/modes/run/runtime/model.rs`
- direct `AppConfig` construction sites reported by a focused `rg`
- `crates/neo-ai/src/auth.rs`
- `crates/neo-ai/src/registry.rs`
- `crates/neo-ai/src/lib.rs`
- delete `crates/neo-ai/src/env_api_keys.rs`
- `crates/neo-ai/tests/provider_resolver.rs`
- `crates/neo-ai/tests/env_and_options.rs`
- `docs/en/configuration/config-files.md`
- `docs/zh/configuration/config-files.md`
- `docs/en/configuration/providers.md`
- `docs/zh/configuration/providers.md`

**Repair track:**

1. Delete top-level `AppConfig::api_key_env` from config/runtime state and its
   loader fallback. Provider config continues to populate `ProviderSpec`; the
   `neo provider add --api-key-env` flag remains because it writes that
   provider-scoped field.
2. Make `ProviderResolver` call `CredentialResolver` directly. Preserve inline
   key precedence, Google env order, Windows ASCII case-insensitive env lookup,
   Unix exact lookup, and secret-redacted errors.
3. Selecting a model chooses that model's provider spec without copying or
   synthesizing credentials.
4. Update English and Chinese current docs to show provider-scoped config only.

**Retirement track:** delete `RESOLVED_API_KEY_ENV`,
`provider_with_invocation_overrides`, agent-side credential resolution, public
`env_api_keys` exports/module/tests, and all top-level config examples. Do not
retain aliases for renamed internal credential source labels.

**Focused evidence:**

```bash
cargo test --package neo-agent --bin neo -- modes::run::runtime::model::tests::selected_provider_never_inherits_another_provider_credentials --exact --nocapture --include-ignored
cargo nextest run -p neo-ai --test provider_resolver credential_resolver_prefers_inline_env_then_auth_file_without_leaking_values
cargo nextest run -p neo-ai --test provider_resolver credential_resolver_matches_environment_names_case_insensitively_on_windows
cargo nextest run -p neo-ai --test provider_resolver credential_resolver_matches_environment_names_exactly_on_unix
rg -n 'env_api_keys|env_api_key_from|find_env_keys_from|__NEO_RESOLVED_API_KEY|provider_with_invocation_overrides' crates docs/en docs/zh
```

Expected: each platform-appropriate test reports one pass; the lingering scan
has no old-owner hit. Provider-scoped `api_key_env` hits are expected.

## Task 2: Stream JSONL RPC Incrementally

**Finding:** F2.
**Commit:** `fix: stream rpc responses incrementally`

**Files:**

- `crates/neo-agent/src/rpc/server.rs`
- `crates/neo-agent/src/main.rs`
- `crates/neo-agent/tests/rpc_mode.rs` or the existing real-process RPC target

**Repair track:**

1. Change `execute` to own locked stdin/stdout and return `Result<()>`; use a
   private generic `BufRead`/`Write` entry only if it materially simplifies
   deterministic tests.
2. Encode each `RpcMessage` as one complete JSONL line, `write_all`, and flush
   immediately while stdin remains open.
3. Stream prompt events from the existing run event channel while the turn is
   running. Preserve final `assistant_text` and `event_count` meanings.
4. Map request-local execution errors to `RpcResponse::failure` with the
   original request ID and continue the input loop. Only stdin/stdout or
   encoding failures terminate the server.

**Retirement track:** delete the accumulated `String`, completion-time event
replay, `execute -> Result<String>`, and the final RPC `print!` path in main.
Do not create a second prompt runtime or approval protocol.

**Focused evidence:**

```bash
cargo nextest run -p neo-agent --test rpc_mode rpc_responds_before_stdin_eof_and_accepts_next_request
cargo nextest run -p neo-agent --test rpc_mode rpc_prompt_failure_is_correlated_and_server_continues
```

Expected: the first response is observable before closing child stdin; the
second request succeeds after a correlated prompt failure.

## Task 3: Cancel Abandoned Agent Event Streams

**Finding:** F3.
**Commit:** `fix: cancel spawned turns when event streams are abandoned`

**Files:**

- `crates/neo-agent-core/src/runtime/agent.rs`
- `crates/neo-agent-core/tests/runtime_turn.rs`

**Repair track:** add the spawned turn's `CancellationToken` and a completion
flag to `AgentEventStream`. Mark normal completion only on `Poll::Ready(None)`;
`Drop` cancels only an incomplete stream. Apply the same constructor contract
to `run_turn`, `run_turn_with_cancel`, and manual compaction paths.

**Retirement track:** do not add caller-side drop guards in RPC, TUI, or
multi-agent code. The stream is the single owner.

**Focused evidence:**

```bash
cargo nextest run -p neo-agent-core --test runtime_turn agent_event_stream_cancels_only_when_abandoned
```

Expected: the abandoned branch observes cancellation before any later effect;
the fully drained branch leaves its external token uncancelled.

## Task 4: Prepare Delegate Swarms Atomically

**Finding:** F4.
**Commit:** `fix: prepare delegate swarms atomically`

**Files:**

- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/src/tools/delegate.rs`
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`

**Repair track:** move full-batch preparation into one
`MultiAgentRuntime` state-lock transaction. Validate every new/resumed child,
capacity, ID, lifecycle, and collision before mutation. On success, create new
children, transition resumes, and register the initial snapshot in deterministic
item order. Reuse small locked helpers from single-delegate entry points.

**Retirement track:** delete the tool-level per-child mutation loop and any
rollback/compensation path. No event or worker starts before commit. Do not edit
Delegate-family TUI card files.

**Focused evidence:**

```bash
cargo nextest run -p neo-agent-core --test multi_agent_runtime delegate_swarm_invalid_late_resume_is_atomic
cargo nextest run -p neo-agent-core --test multi_agent_runtime delegate_swarm_resume_agent_ids_restarts_existing_children
```

Expected: the failure test leaves agents, order, counts, history, status, and
swarm state unchanged; the existing success path remains deterministic.

## Task 5: Share And Bound SSE Framing

**Findings:** F5, F13.
**Commit:** `fix: bound shared sse framing and validate provider urls`

**Files:**

- `crates/neo-ai/src/providers/common/sse.rs`
- `crates/neo-ai/src/providers/common/http.rs`
- `crates/neo-ai/src/providers/anthropic.rs`
- `crates/neo-ai/src/providers/google.rs`
- `crates/neo-ai/src/providers/openai/responses.rs`
- `crates/neo-ai/src/providers/openai/compatible.rs`

**Repair track:**

1. Add one private/shared `SseFramer` that owns pending bytes, split delimiter
   handling, and a fixed `MAX_SSE_FRAME_BYTES = 8 * 1024 * 1024` machine-safety
   bound. Use a consumed cursor and occasional compaction, not per-frame prefix
   draining.
2. Preserve `\n\n`, `\r\n\r\n`, multi-`data:` lines, split delimiters, and
   each provider's existing terminal semantics.
3. Return one non-retryable protocol error on an oversized frame and stop.
4. Make Google reuse `common::http::request_url` before adding `alt=sse`.

**Retirement track:** delete provider-local pending-byte framing and Google's
direct URL validation path. Provider-specific JSON/event parse state remains
separate; do not force unrelated protocol states into a generic parser.

**Focused evidence:**

```bash
cargo nextest run -p neo-ai --lib providers::common::sse::tests::framer_rejects_oversized_incomplete_frame
cargo nextest run -p neo-ai --lib providers::common::sse::tests::framer_accepts_each_delimiter_split_at_every_byte
cargo nextest run -p neo-ai --lib providers::anthropic::tests::oversized_sse_frame_is_rejected
cargo nextest run -p neo-ai --lib providers::google::tests::oversized_sse_frame_is_rejected
cargo nextest run -p neo-ai --lib providers::openai::responses::tests::oversized_sse_frame_is_rejected
cargo nextest run -p neo-ai --lib providers::openai::compatible::tests::oversized_sse_frame_is_rejected
cargo nextest run -p neo-ai --lib providers::google::tests::request_url_rejects_non_http_schemes_without_retry
cargo nextest run -p neo-ai --lib providers::google::tests::content_free_stop_emits_balanced_message
cargo nextest run -p neo-ai --lib providers::openai::compatible::tests::normalize_openai_chat_sse_accepts_finish_reason_without_done_marker
rg -n 'buffer\.extend_from_slice|buffer\.drain\(' crates/neo-ai/src/providers/anthropic.rs crates/neo-ai/src/providers/google.rs crates/neo-ai/src/providers/openai/responses.rs crates/neo-ai/src/providers/openai/compatible.rs
```

Expected: exhaustive delimiter splitting passes, every provider rejects the
same oversized frame through the shared owner, the lingering scan is empty,
and existing terminal behavior is unchanged. The byte bound is not
configurable cost governance.

## Task 6A: Retire The Duplicate TUI ANSI Sanitizer

**Finding:** F7.
**Commit:** `refactor(tui): unify shell ansi sanitization`

**Files:**

- `crates/neo-tui/src/transcript/shell_run.rs`
- `crates/neo-tui/src/primitive/ansi_escape.rs`
- delete `crates/neo-tui/src/utils/shell_output.rs`
- delete `crates/neo-tui/tests/shell_output.rs`
- remove the empty utils module/export only if it has no other consumer

**Repair track:** make the existing ANSI state machine incrementally consume
split CSI, OSC, DCS, APC, PM, SOS, C1, and ordinary text. Finalize/reset clears
pending control state. Migrate ShellRun sanitization to it.

**Retirement track:** delete the weak sanitizer and its tests. Do not touch
`delegate_card.rs`, `delegate_group.rs`, `swarm_card.rs`, or
`child_activity.rs`.

**Focused evidence:**

```bash
cargo nextest run -p neo-tui --test tool_cards shell_run_sanitizes_split_control_strings_with_canonical_ansi_state
rg -n 'sanitize_shell_output|utils::shell_output' crates/neo-tui
```

Expected: split control strings sanitize correctly and the duplicate owner has
no hits.

## Task 6B: Frame TUI Streaming Output Across Chunks

**Finding:** F6.
**Commit:** `fix(tui): frame streaming output across chunks`

**Files:**

- add `crates/neo-tui/src/transcript/live_output.rs`
- `crates/neo-tui/src/transcript/mod.rs`
- `crates/neo-tui/src/transcript/tool_call.rs`
- `crates/neo-tui/src/transcript/shell_run.rs`
- focused cases in `crates/neo-tui/tests/tool_cards.rs`

**Repair track:** add one private live-output owner containing
`VecDeque<String>`, the current incomplete line, canonical ANSI state, current
char count, and dropped counters. Pass existing bounds from callers: ToolCall 6
lines/50,000 chars; ShellRun 12 lines/256 KiB. Show the partial tail without
treating chunk boundaries as newlines; finalize it exactly once.

**Retirement track:** delete both `Vec::remove(0)` loops and duplicated
partial-line state. Do not touch the four frozen Delegate-family files.

**Focused evidence:**

```bash
cargo nextest run -p neo-tui --test tool_cards tool_call_live_output_reassembles_split_lines_and_ansi
cargo nextest run -p neo-tui --test tool_cards shell_run_live_output_reassembles_split_control_sequences
cargo nextest run -p neo-tui --test tool_cards shell_live_output_bounds_eviction_without_losing_partial_tail
rg -n 'live_output\.remove\(0\)' crates/neo-tui
```

Expected: exact logical text is visible across arbitrary chunk splits and the
old sanitizer/front-removal owners have no hits.

## Task 7: Consolidate Atomic Writes And Safe Directory Creation

**Findings:** F8, F9 and path-helper portion of C1.
**Commit:** `fix: centralize durable atomic file creation`

**Files:**

- `crates/neo-agent-core/src/session/atomic_file.rs`
- `crates/neo-agent-core/src/session/mod.rs`
- `crates/neo-agent-core/src/workflow/journal.rs`
- `crates/neo-agent-core/src/workspace_policy.rs`
- `crates/neo-agent-core/src/tools/edit.rs`
- `crates/neo-agent-core/src/tools/write.rs`
- `crates/neo-agent-core/src/tools/skills_manager.rs`
- `crates/neo-agent-core/tests/workflow_journal.rs`

**Repair track:**

1. Make `ensure_safe_directory_tree` reuse the existing component-wise
   creation/recording helper. `symlink_metadata` validates every existing
   ancestor before descendants are created; reject Unix symlinks, Windows
   reparse/junctions, and non-directories.
2. Make `write_run_metadata` call `write_file_atomic_create_new` and map its
   outcome without reimplementing temp publication or directory sync.
3. Expose the minimum crate-private directory-sync and reparse/symlink
   predicates from `session::atomic_file`; migrate the listed direct duplicate
   owners only where their semantics are identical.
4. Preserve the append-only workflow journal sync order exactly. Journal file
   creation may call the shared directory-sync helper, but must not route
   append records through an atomic replacement function.

**Retirement track:** delete workflow-local temp/hard-link mechanics and its
duplicate directory-sync implementation after the journal caller migrates;
delete leaf-only `create_dir_all` and semantically identical platform
predicates. Stop rather than merging a predicate with a different trust or
error contract.

**Focused evidence on macOS and Linux:**

```bash
cargo test --package neo-agent-core --test workflow_journal -- run_metadata_creation_does_not_overwrite_existing_metadata --exact --nocapture
cargo test --package neo-agent-core --lib -- session::atomic_file::tests::safe_directory_tree_rejects_symlinked_ancestor --exact --nocapture
```

**Focused evidence on Windows:**

```powershell
cargo test --package neo-agent-core --lib -- session::atomic_file::tests::directory_sync_accepts_directory_handles --exact --nocapture
cargo test --package neo-agent-core --lib -- session::atomic_file::tests::safe_directory_tree_rejects_junction_ancestor --exact --nocapture
cargo test --package neo-agent-core --test workflow_journal -- run_metadata_creation_does_not_overwrite_existing_metadata --exact --nocapture
```

The junction fixture may call `cmd.exe /d /c mklink /J` with structured args in
test code; it must fail loudly if the environment cannot create the fixture.

## Task 8: Add One Lossless Session-Index Path Wire

**Finding:** session-index portion of F11.
**Commit:** `fix: preserve native paths in the session index`

**Files:**

- `crates/neo-agent-core/src/session/index.rs`
- focused session-index unit tests in the same module

**Repair track:** keep public `SessionIndexEntry` fields as `PathBuf`. Add a
private versioned/tagged serde wire for both path fields: Unix bytes and Windows
UTF-16 code units encoded with the existing base64 dependency. The reader
accepts old Unicode string records and the new tagged records, deterministically
rejecting foreign/invalid encodings. New writes emit only the new canonical
record shape; no existing file is rewritten.

**Retirement track:** delete any `to_str`, lossy conversion, or parallel public
path representation. Read compatibility is confined to the private wire.

**Focused evidence:**

```bash
cargo test --package neo-agent-core --lib -- session::index::tests::index_round_trips_native_non_unicode_paths --exact --nocapture
cargo test --package neo-agent-core --lib -- session::index::tests::index_reads_existing_unicode_record_without_rewrite --exact --nocapture
```

Run the native non-Unicode fixture on Unix and the unpaired-surrogate fixture on
Windows. Expected: `session_dir` and `workdir` round-trip losslessly.

## Task 9: Make The Windows Launch Barrier Treat Paths As Data

**Finding:** F10.
**Commit:** `fix: pass windows launch barrier paths as data`

**Files:**

- `crates/neo-agent-core/src/tools/shell_guard/process_tree.rs`
- `crates/neo-agent-core/src/tools/shell_guard/guardian.rs`
- `crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs`
- `crates/neo-agent/tests/process_guard_windows.rs`

**Repair track:** give the barrier a fixed private environment key. Guardian
and terminal command builders attach the native `PathBuf` as an environment
value; command text references only the fixed name with command extensions
disabled. Keep one shared barrier path for both tools.

**Retirement track:** delete `Path::display()` interpolation and character-by-
character quoting branches. Do not use PowerShell, a helper executable, or a
second synchronization scheme.

**Focused Windows evidence:**

```powershell
cargo test --package neo-agent --test process_guard_windows -- windows_launch_barrier_treats_status_path_as_data --exact --nocapture
```

The fixture path must contain `%PATH% ! & O'Reilly space` and exercise a real
spawn/release boundary. Expected: the intended child starts and exits without
accessing an expanded or truncated path.

## Task 10: Preserve Git Paths And Bound Footer I/O

**Finding:** F12.
**Commit:** `fix: keep git footer file inspection nonblocking`

**Files:**

- `crates/neo-agent/src/modes/interactive/git_status.rs`
- `crates/neo-agent/src/modes/interactive/tests.rs`
- `crates/neo-agent/Cargo.toml` only if the existing Unix rustix features need
  enabling; do not add a new dependency

**Repair track:**

1. Parse Unix NUL paths with `OsStringExt::from_vec`. On Windows, represent an
   undecodable entry as one uninspectable item rather than using lossy text.
2. Validate containment and `symlink_metadata`; accept only regular,
   non-symlink/reparse files.
3. On Unix open with nonblocking/no-follow flags, then recheck file metadata to
   close the FIFO swap race. Read at most 1 MiB; over-limit means unknown.
4. Count special, invalid, and oversized paths as `?1` entries and keep the
   background refresh alive. Preserve the restored line-count feature.

**Retirement track:** delete path-byte `from_utf8_lossy`, unbounded reads, and
the lstat-only trust decision. Do not simplify the footer back to `±`.

**Focused Unix evidence:**

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::git_status_untracked_fifo_returns_without_blocking --exact --nocapture
cargo test --package neo-agent --bin neo -- modes::interactive::tests::git_status_untracked_non_utf8_path_is_counted_losslessly --exact --nocapture
cargo test --package neo-agent --bin neo -- modes::interactive::tests::git_status_untracked_file_over_limit_counts_as_unknown --exact --nocapture
```

Run the oversized and a Unicode-special-filename case on Windows. Expected:
bounded completion and a truthful unknown count for uninspectable entries.

```powershell
cargo test --package neo-agent --bin neo -- modes::interactive::tests::git_status_untracked_windows_unicode_path_is_counted_losslessly --exact --nocapture
cargo test --package neo-agent --bin neo -- modes::interactive::tests::git_status_untracked_file_over_limit_counts_as_unknown --exact --nocapture
```

Windows does not need to synthesize impossible Git byte sequences: prove a
native Unicode path and the bounded unknown fallback. Unix owns the raw invalid
UTF-8 fixture.

## Task 11: Resolve Windows Native-Path Atomic Replacement

**Finding:** replacement portion of F11.
**Commit:** `fix: replace windows files through native paths`

**Files:**

- `crates/neo-agent-core/src/session/atomic_file.rs`
- `crates/neo-agent-core/Cargo.toml`
- workspace `Cargo.lock` only through Cargo dependency resolution
- focused Windows tests in `session::atomic_file`

**Dependency decision gate:** first evaluate a bounded set of maintained safe
wrappers or an upgrade/extension of an existing safe dependency. The selected
API must accept `Path`/`OsStr` or wide input, encapsulate Win32 unsafe code,
provide ReplaceFileW-equivalent atomic replacement and write-through behavior,
and support Rust 1.96.1. Record the evidence in the commit message/body.

Current `winsafe 0.0.19::ReplaceFile(&str, ...)` is insufficient. If no safe
API meets the gate, stop this task and report the blocker. Do not add `unsafe`
inside Neo, UTF-8/lossy conversion, remove-then-rename, PowerShell, a helper
binary, or a non-atomic fallback. Other completed tasks remain valid.

**Repair/retirement track:** replace only the Windows publication call, then
delete the Unicode-path error branches and any obsolete dependency feature.

**Focused Windows evidence:**

```powershell
cargo test --package neo-agent-core --lib -- session::atomic_file::tests::atomic_replace_accepts_unpaired_surrogate_paths --exact --nocapture
```

Expected: replacement is atomic and content changes at a filename containing
an unpaired surrogate without a remove window.

## Task 12: Delete The Audited Low-Value Tests, Wrappers, And Dependencies

**Findings:** remaining C1 and C2.
**Commit:** `refactor: remove audited duplicate coverage and wrappers`

**Closed file and symbol set:**

- delete `crates/neo-tui/tests/trust_dialog.rs`; retain the startup trust tests
  in `crates/neo-agent/src/modes/interactive/tests.rs`
- delete `crates/neo-tui/tests/shell_mode_theme.rs`
- delete `crates/neo-tui/tests/shell_mode_state.rs`
- delete `component_defaults_to_live_and_ignored_input` from
  `crates/neo-tui/tests/core_components.rs`
- delete `api_type_config_strings_round_trip` from
  `crates/neo-ai/src/types.rs`
- delete `reasoning_selection_round_trips_structured_modes` from
  `crates/neo-ai/tests/env_and_options.rs`
- delete `run_emits_jsonl_events_from_mock_provider_without_fake_output` from
  `crates/neo-agent/tests/mock_provider_e2e.rs`; retain the typed JSON output
  test and the production-adapter text test
- delete `role_selection_guide_mentions_every_role` from
  `crates/neo-agent-core/tests/multi_agent_roles.rs`; retain
  `delegate_and_swarm_schemas_surface_role_guide`
- `crates/neo-agent/tests/cli_commands.rs`
- `crates/neo-tui/src/widgets/btw_panel.rs`: delete `wrap_ansi` and
  `repeat_char`, calling `wrap_width` and `std::iter::repeat_n` directly
- `crates/neo-agent-core/src/session/mod.rs`: delete its forwarding
  `write_file_atomic` wrapper and call `atomic_file::write_file_atomic`
- `crates/neo-agent/src/resources.rs`: delete
  `append_system_prompt_candidates` and call
  `resource_candidates(APPEND_SYSTEM_PROMPT_FILE)` directly
- `crates/neo-ai/Cargo.toml`: remove direct `tokio-util`
- `crates/neo-tui/Cargo.toml`: remove direct `tokio`
- `crates/neo-agent-core/Cargo.toml`: remove direct `chrono`, `toml`, and dev
  `temp-env`
- `crates/neo-agent/Cargo.toml`: remove dev `ed25519-dalek` and `tar`
- root `Cargo.toml`: remove workspace `ed25519-dalek` and `tar` only after the
  direct edges above are gone and no other member uses them
- Cargo-generated `Cargo.lock`

**Repair track:** replace `127.0.0.1:1` with a bound local mock that
deterministically returns the intended failure. Preserve tests that protect
wire shape, schema, lifecycle order, security, rendered behavior, or platform
contracts.

**Retirement track:** delete exactly the listed low-value tests and wrappers.
Before deleting each dependency, reproduce the original proof with `cargo
machete` plus a symbol/manifest search; if later tasks introduced a real use,
keep that dependency and record the evidence. Do not search for or delete any
additional test, wrapper, or dependency in this task.

**Focused evidence:** run the one strongest surviving behavior test for every
deleted cluster and the exact CLI failure test after mock replacement. For
example:

```bash
cargo nextest run -p neo-agent --test cli_commands mcp_add_remote_http_reports_failure_deterministically
cargo test --package neo-agent --bin neo -- modes::interactive::tests::startup_trust_dialog_opens_when_unknown_and_trusts_workspace --exact --nocapture --include-ignored
cargo nextest run -p neo-agent --test mock_provider_e2e run_output_json_emits_stable_typed_events_from_mock_provider
cargo nextest run -p neo-agent-core --test multi_agent_roles delegate_and_swarm_schemas_surface_role_guide
cargo machete
rg -n 'tokio_util|chrono::|toml::|temp_env|ed25519|tar::' crates/neo-ai crates/neo-tui crates/neo-agent-core crates/neo-agent
```

Expected: the stronger behavior tests pass, `cargo machete` no longer reports
the listed direct edges, and no removed wrapper/test remains. Do not force a
dependency removal or invent replacement tests to hit a line-count target.

## Per-Task Verification And Commit Protocol

For every task:

1. Run only the listed exact behavior tests plus a directly affected existing
   regression when needed.
2. Format/check only touched Rust files where practical:

   ```bash
   rustfmt --check --edition 2024 <touched-rust-files>
   git diff --check -- <touched-files>
   ```

3. Inspect `git diff -- <touched-files>` and run the task's retirement scan.
4. Stage only that task's files and create its named conventional commit.
5. Re-run `git status --short`; preserve unrelated files exactly.

Do not run broad `cargo test`, package-wide unfiltered nextest, or mutate Git by
reset/restore/stash/rebase/clean. Do not push.

## Cross-Platform Verification Matrix

| Boundary | macOS | Linux | Windows |
|---|---|---|---|
| credentials/env matching | Unix exact test | Unix exact test | case-insensitive test |
| RPC flushing/continuation | required | required | required |
| stream cancellation/swarm atomicity | required | required | required |
| SSE framing/URL validation | required | required | required |
| TUI incremental ANSI | required | required | required |
| workflow atomic create | required | required | required incl. dir sync |
| ancestor safety | symlink | symlink | junction/reparse |
| session-index native path | non-UTF-8 bytes | non-UTF-8 bytes | unpaired UTF-16 surrogate |
| launch barrier | not applicable | not applicable | native real spawn |
| Git footer | FIFO/non-UTF-8/large | FIFO/non-UTF-8/large | Unicode/invalid/large |
| atomic replacement | Unix rename path smoke | Unix rename path smoke | safe native-path wrapper |

Before starting a Parallels VM, check host memory and `prlctl list`; boot only
one VM at a time. Stop a VM with `prlctl stop <name>` when its work is complete.
Do not stop a VM that still has unrelated work running.

## Final Retirement And Drift Audit

Run focused scans, interpreting valid canonical field names carefully:

```bash
rg -n 'env_api_keys|env_api_key_from|find_env_keys_from|__NEO_RESOLVED_API_KEY|provider_with_invocation_overrides' crates docs/en docs/zh
rg -n 'sanitize_shell_output|live_output\.remove\(0\)' crates/neo-tui
rg -n 'sync_parent_directory|replacement path is not Unicode|target path is not Unicode' crates/neo-agent-core/src/session crates/neo-agent-core/src/workflow
rg -n 'from_utf8_lossy\(path\)|WindowsLaunchBarrier|wait_command' crates/neo-agent/src/modes/interactive crates/neo-agent-core/src/tools/shell_guard
git diff --check
```

Then inspect every task commit and decide whether the lossless path wire and
cancel-on-drop lifecycle need an ADR/baseline update. Do not create an ADR only
to restate code; create one only if the implemented contract is a durable
architecture decision not already captured by the approved design.

## Completion Evidence Required

- commit list, one logical task per commit;
- exact command and pass/fail output for every focused test;
- native OS/VM for every platform-only test;
- retirement scan results;
- list of dependencies actually removed and proof they were unused;
- any skipped task with the exact stop condition and evidence;
- final `git status --short`, preserving unrelated changes;
- no claim that local focused evidence means remote CI is green.

## Self-Review

- Every F1-F14 and C1-C2 finding maps to a task.
- No placeholder, optional compatibility branch, or second owner remains.
- RunWorkflow cost/control semantics and journal ordering are frozen.
- Delegate-family presentation is frozen.
- Windows safe-wrapper feasibility is an explicit preflight, not hidden unsafe.
- Tests are focused and behavior-bearing; cleanup is evidence-driven.
- The executor can stop safely after any committed task and hand back precise
  residual work.
