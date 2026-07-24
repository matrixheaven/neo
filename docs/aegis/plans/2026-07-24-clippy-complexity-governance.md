# Clippy Complexity Governance Implementation Plan

## Goal

Implement the approved
`docs/aegis/specs/2026-07-24-clippy-complexity-governance-design.md`: repair the
four known CI failures, remove all 68 local complexity-lint attributes, and
remove both local exemptions and enforce the source-level policy in CI.

## Architecture

This is an owner-preserving refactor. Existing runtime, tool, TUI, session, and
test owners remain authoritative. Parameter structs are permitted only for an
existing semantic group; long functions are split only at cohesive phases.

## Tech Stack

Rust workspace, edition 2024, minimum Rust 1.96.1; existing Cargo, Clippy,
rustfmt, nextest, `FakeModelClient`, and `FakeHarness`. No new dependency.

## Baseline / Authority Refs

- `AGENTS.md`, `CX.md`, and `RTK.md`.
- `docs/aegis/specs/2026-07-24-clippy-complexity-governance-design.md`.
- `docs/aegis/specs/2026-07-17-canonical-approval-protocol-design.md`.
- `docs/aegis/baseline/2026-07-23-runworkflow-runtime-contract.md`.
- Current `.github/workflows/ci.yml`, `.config/nextest.toml`, and Cargo lints.

## Compatibility Boundary

No public schema, CLI, persistence, permission, session-lock, workflow runtime,
card, provider, or cross-platform behavior changes. The retired synchronous
workflow test is not a compatibility boundary.

## TDD Route

- Mode: off.
- Decision: skipped.
- Strict authority: not applicable.
- Test posture: diagnostic reproduction followed by focused regression.
- Reason: no strict-TDD request; all four failures already reproduce.
- Verification: one package and one explicit target per focused command.

## Planning Readback

- Requirement ready: approved design and acceptance criteria are complete.
- Change necessity: source changes are required because tests self-lock or
  assert a regressed message, one test preserves a retired contract, and lint
  attributes cannot be removed by docs/config alone.
- Existence check: reuse existing owners and local structs/helpers; no new
  subsystem or compatibility path.
- Architecture integrity: canonical owners stay unchanged; obsolete test-only
  ownership is deleted.
- Complexity budget: current target files include several 800-4000 line owners.
  The plan uses `move-out/extract-first` or local semantic parameter grouping,
  never adds a new responsibility to those files.
- Plan pressure result: proceed in owner-isolated batches with review after each.

## Execution Readiness View

- Intent lock: zero local attributes plus four repaired failures.
- Scope fence: only listed source/tests, workspace lint config, and Aegis records.
- Baseline lock: session lifetime locking, canonical approval, and durable
  workflow ownership remain unchanged.
- Retirement boundary: delete only obsolete synchronous workflow test/probe.
- Test obligations: focused regressions and target-specific lint per batch.
- Review gates: spec compliance, then code quality, then final integrated review.
- Drift rule: any public contract change, fallback, new dependency, or unrelated
  failure stops the slice for replanning.
- Completion evidence: zero-hit search, CI guard, format, Clippy, build,
  and focused tests.

## Task 1: Repair The Four Known Failures

Files:

- `crates/neo-agent/src/modes/run/mod.rs`
- `crates/neo-agent-core/src/runtime/permission.rs`
- `crates/neo-agent-core/tests/runtime_turn.rs`

Steps:

1. Add `drop(seed)` after each seed writer flush in the two timeout fixtures.
2. Make the shared approval helper emit
   `User requested revisions. {mode} remains active.` for Plan and Goal modes.
3. Delete the obsolete synchronous RunWorkflow event test and its private,
   single-use `WorkflowToolEventProbe`.
4. Run:

   ```bash
   rtk cargo nextest run -p neo-agent --bin neo -E 'test(run_prompt_with_runtime_appends_continuation_to_existing_session_context) | test(prepare_existing_streaming_turn_uses_session_root_for_main_wire_session)'
   rtk cargo nextest run -p neo-agent-core --test session_jsonl jsonl_session_compaction_waits_for_live_writer_before_reading
   rtk cargo nextest run -p neo-agent-core --test runtime_turn -E 'test(exit_goal_mode_reject_and_revise_create_no_goal) | test(stored_workflow_handle_routes_nested_events_only_to_the_active_turn)'
   rtk cargo nextest run -p neo-agent-core --test workflow_dispatch -E 'test(active_route_is_exclusive_and_draining_events_release_to_idle) | test(idle_route_waits_for_active_stream_drop_after_receiver_exhaustion)'
   rtk git diff --check
   ```

5. Commit `fix(runtime): resolve known CI regressions`.

Repair track: change only the failing fixtures/message helper and remove stale
coverage. Retirement track: synchronous workflow test/probe are deleted with no
replacement path; current durable tests remain canonical.

## Task 2: Clean Neo Agent Core Production Owners

Files:

- `crates/neo-agent-core/src/compaction/summary.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs`
- `crates/neo-agent-core/src/runtime/{agent.rs,permission.rs,tool_dispatch.rs,turn_loop.rs}`
- `crates/neo-agent-core/src/tools/{ask_user.rs,background_tasks.rs,delegate.rs,delegate_controls.rs,mcp_manager.rs}`
- `crates/neo-agent-core/src/tools/shell_guard/client.rs`
- `crates/neo-agent-core/src/workflow/lua.rs`

Steps:

1. Remove all matching attributes in these files.
2. Reuse existing seed/input/state structs for semantic argument groups; add a
   private struct only where no existing type owns that exact group.
3. Extract cohesive parse/prepare/execute/finalize or render/setup phases until
   each function satisfies Clippy without hiding the lint elsewhere.
4. Run:

   ```bash
   rtk cargo clippy -p neo-agent-core --lib --all-features -- -A clippy::all -D clippy::too_many_lines -D clippy::too_many_arguments
   rtk rustfmt --check --edition 2024 <each-touched-rust-file>
   rtk git diff --check
   ```

5. Commit `refactor(core): remove complexity lint exemptions`.

## Task 3: Clean Neo Agent Binary Owners

Files:

- `crates/neo-agent/src/config/loader.rs`
- `crates/neo-agent/src/modes/run/{mod.rs,mcp_cli.rs}`
- `crates/neo-agent/src/modes/interactive/{mod.rs,controller_factory.rs}`
- `crates/neo-agent/src/modes/task_browser.rs`

Steps:

1. Remove matching attributes and refactor only existing construction/run phases.
2. Prefer existing config/controller inputs over new generic argument bags.
3. Run:

   ```bash
   rtk cargo clippy -p neo-agent --bin neo --all-features -- -A clippy::all -D clippy::too_many_lines -D clippy::too_many_arguments
   rtk cargo nextest run -p neo-agent --bin neo -E 'test(run_prompt_with_runtime_appends_continuation_to_existing_session_context) | test(prepare_existing_streaming_turn_uses_session_root_for_main_wire_session)'
   rtk git diff --check
   ```

4. Commit `refactor(agent): remove complexity lint exemptions`.

## Task 4: Clean Neo TUI Owners

Files:

- `crates/neo-tui/src/dialogs/model_selector.rs`
- `crates/neo-tui/src/input/raw_input.rs`
- `crates/neo-tui/src/shell/{event_router.rs,prompt.rs,session_picker.rs}`
- `crates/neo-tui/src/transcript/{event_handler.rs,shell_run.rs,swarm_card.rs}`
- `crates/neo-tui/src/transcript/entry/{mod.rs,render_status.rs}`

Steps:

1. Remove matching attributes.
2. Preserve card layout/content and split only event-routing, state-update, and
   rendering phases already present in the functions.
3. Run:

   ```bash
   rtk cargo clippy -p neo-tui --lib --all-features -- -A clippy::all -D clippy::too_many_lines -D clippy::too_many_arguments
   rtk git diff --check
   ```

4. Commit `refactor(tui): remove complexity lint exemptions`.

## Task 5: Clean Maintained Tests

Files:

- `crates/neo-agent/src/modes/interactive/tests.rs`
- `crates/neo-agent-core/tests/{multi_agent_background.rs,multi_agent_runtime.rs,runtime_turn.rs}`
- `crates/neo-tui/tests/multi_agent_transcript.rs`

Steps:

1. Remove matching attributes.
2. Extract repeated fixtures or named setup/action/assert phases without adding
   duplicate coverage or changing assertions.
3. Run one explicit target at a time:

   ```bash
   rtk cargo clippy -p neo-agent --bin neo --all-features -- -A clippy::all -D clippy::too_many_lines -D clippy::too_many_arguments
   rtk cargo clippy -p neo-agent-core --test runtime_turn --all-features -- -A clippy::all -D clippy::too_many_lines -D clippy::too_many_arguments
   rtk cargo clippy -p neo-agent-core --test multi_agent_runtime --all-features -- -A clippy::all -D clippy::too_many_lines -D clippy::too_many_arguments
   rtk cargo clippy -p neo-agent-core --test multi_agent_background --all-features -- -A clippy::all -D clippy::too_many_lines -D clippy::too_many_arguments
   rtk cargo clippy -p neo-tui --test multi_agent_transcript --all-features -- -A clippy::all -D clippy::too_many_lines -D clippy::too_many_arguments
   rtk git diff --check
   ```

4. Commit `refactor(tests): remove complexity lint exemptions`.

## Task 6: Enforce And Verify The Workspace Policy

Files:

- `Cargo.toml`
- `.github/workflows/ci.yml` only if Cargo lint inheritance does not survive the
  current command-line group settings.

Steps:

1. Add a native CI `git grep` guard for the two exact local attributes. Keep the
   existing workspace lint command unchanged; the current `-A clippy::pedantic`
   intentionally leaves historical long functions outside this focused cleanup.
2. Confirm all four crates inherit workspace lints.
3. Verify no matching attributes remain:

   ```bash
   rtk rg -n '#\[allow\(clippy::(too_many_lines|too_many_arguments)' crates
   ```

   Expected result: exit 1 with no matches.

4. Run:

   ```bash
   rtk cargo fmt --all --check
   rtk cargo clippy --workspace --all-targets --all-features -- -D clippy::all -A clippy::pedantic
   rtk cargo build -p neo-agent
   rtk git diff --check
   ```

5. Re-run Task 1 focused regressions and commit
   `chore(clippy): guard complexity lint exemptions`.

## Risks And Stop Conditions

- If a refactor needs a public signature change, new fallback, dependency, or
  compatibility adapter, stop and revise the plan.
- If another agent changes a touched file, do not revert it; rebase the slice
  manually through targeted edits or pause.
- A Clippy-clean extraction without focused behavior evidence is insufficient
  for runtime bug fixes.
- Actual GitHub Actions status requires a later authorized push; local evidence
  must not be described as a remote run.

## Retirement

All 68 local attributes retire. The obsolete synchronous workflow test/probe
retire. No old path, alias, lint threshold increase, `expect` replacement, or
custom grep enforcement is retained.
