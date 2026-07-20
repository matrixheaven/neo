# RunWorkflow Neo-Native Dynamic Workflow Implementation Plan

## Goal

Implement the approved design in
`docs/aegis/specs/2026-07-20-runworkflow-dynamic-workflow-design.md`.
Replace the current foreground recorder with a durable, always-background,
inspectable Neo-native Dynamic Workflow runtime.

The implementation must solve all six confirmed defects in one canonical owner
model: durable run/journal truth, awaited children, canonical permissions and
instruction preflight, strict Lua/schema behavior, resumable replay, shared
human/model controls, inspectable projections, and hard retirement of the old
paths.

## Architecture

`WorkflowRuntime` owns run metadata, append-only journal, state
transitions, Lua execution, replay, recovery, limits, and aggregate results.

Existing owners remain authoritative:

- runtime permission/approval pipeline: launch approval and child effects;
- instruction registry/preflight: instruction scope and epoch behavior;
- `ShellRuntime`: Bash/Terminal admission and process execution;
- `MultiAgentRuntime`: Delegate/Swarm lifecycle and actual usage;
- `BackgroundTaskManager`: task query/control handle only;
- notification owner: one typed completion/recovery notification;
- session JSONL/TUI: projections only;
- existing Delegate-family/Bash/Terminal cards: unchanged presentation owner.

No durable workflow state may be placed in `ToolContext`,
`BackgroundTaskManager`, transcript cards, or session event payloads.

## Tech Stack

- Rust workspace, edition 2024, minimum Rust 1.96.1.
- `mlua` for the Lua VM; use its memory limit and instruction hooks.
- Tokio for background workers, cancellation, and async host calls.
- Existing `serde`, `serde_json`, `schemars`,
  `sha2`, `uuid`, and Neo runtime/event infrastructure.
- Existing `FakeModelClient`/`FakeHarness` for provider-free tests.
- No new dependency without explicit owner/benefit justification.

## Baseline / Authority Refs

Read before source edits:

- `docs/aegis/specs/2026-07-20-runworkflow-dynamic-workflow-design.md`
- `docs/aegis/specs/2026-07-17-canonical-approval-protocol-design.md`
- `docs/aegis/specs/2026-07-18-shell-admission-scheduler-design.md`
- `docs/aegis/specs/2026-07-20-bash-terminal-tool-card-brief.md`
- `docs/aegis/specs/2026-07-13-transcript-boundary-semantics-design.md`
- repository `AGENTS.md`, `CX.md`, and `RTK.md`.

Current owners to inspect:

- `crates/neo-agent-core/src/tools/workflow.rs`
- `crates/neo-agent-core/src/workflow/{mod.rs,lua.rs,state.rs,host_api.rs,error.rs}`
- `crates/neo-agent-core/src/tools/background_tasks.rs`
- `crates/neo-agent-core/src/runtime/{config.rs,permission.rs,tool_dispatch.rs,events.rs}`
- `crates/neo-agent-core/src/tools/mod.rs`
- `crates/neo-agent-core/src/multi_agent/{runtime.rs,scheduler.rs,state.rs}`
- `crates/neo-agent-core/src/events.rs`
- `crates/neo-agent-core/src/session/{mod.rs,event_persistence.rs}`
- `crates/neo-agent/src/modes/interactive/{mod.rs,slash_commands.rs,prompt_completion.rs,input.rs,turn.rs}`
- `crates/neo-agent/src/config/{mod.rs,types.rs,loader.rs}`
- `crates/neo-tui/src/{tasks_browser,transcript}`

## Compatibility Boundary

This is a hard internal replacement:

- delete synchronous foreground execution;
- replace `{title, script}` with strict
  `{name, description, phases, script, args}`;
- delete `WorkflowHostRecorder`, recorder-only `run_script()`,
  and in-memory steps/reports as state owners;
- delete direct workflow `child_tools.run()` dispatch;
- reject workflow `mode=background` and `max_concurrency`;
- delete copied workflow snapshots from `BackgroundTaskManager`;
- delete tests/docs that preserve retired behavior;
- do not add fallback fields, compatibility enums, aliases, or migration runners.

Existing session files are not deleted or rewritten. Historical workflow events
remain readable as historical cards, but never synthesize
`run.json`/`journal.jsonl` and cannot resume.

Delegate, DelegateGroup, DelegateSwarm, Bash, and Terminal card structure,
budgets, ordering, content, and expansion remain unchanged.

## TDD Route

- Mode: off.
- Decision: skipped.
- Strict authority: no explicit strict-TDD request.
- Test posture: focused post-change regression and deterministic owner tests.
- Reason: the user approved design and plan, not strict TDD. High-risk slices
  still require narrow tests that fail on contract regressions.
- Verification: each task names one package, one target selector, and exact
  test-name filters; never use a broad workspace run as sole evidence.

## Verification Baseline

Per task:

~~~text
git diff --check
rustfmt --check --edition 2024 <touched-rust-files>
cargo nextest run -p <package> --test <target> <exact-filter>
~~~

Use `--lib <exact-filter>` only for library unit tests and
`--bin <bin> <exact-filter>` for binary tests. Commit one logical slice
after its focused verification passes.

## Aegis Planning Readback

### Aegis Visibility

Planning is required because this work changes durable state, public tool
schemas, permission ownership, background lifecycle, replay, and transcript
projection. The plan prevents duplicate owners, replayed external effects,
weakened approval, and a second task system.

### Plan Basis

The approved design spec is the product contract. Tasks may choose internal
Rust decomposition but may not reopen its product decisions.

### BaselineUsageDraft

- Required refs: the four Neo Aegis specs above plus current workflow/runtime/
  task/transcript owners.
- Delivered context: CodeGraph/source inspection of workflow, ToolContext,
  execute_tool_calls, permission, event sink, task browser, config, and session.
- Acknowledged refs: approved design and four baseline specs.
- Cited refs: Goal, Architecture, task boundaries, compatibility, verification.
- Missing refs: none blocking execution.
- Decision: continue.

### Requirement Ready Check

- Source: approved design spec and six-problem decision record.
- Scenarios: large multi-agent refactor, long verification, host exit,
  pause/resume/stop, instruction change, permission denial, high actual usage.
- Acceptance: spec Acceptance Evidence plus this plan's exact tests.
- Open blocker questions: none.
- Decision: ready.

### Change Necessity

- User-visible need: current workflow cannot safely permission, await, inspect,
  or resume dynamic work.
- Non-code option: documentation/recorder patch cannot provide durable
  intent-before-effect, recovery, or canonical dispatch.
- Minimum code boundary: workflow journal/runtime plus narrow runtime, approval,
  task, session, projection, config, and docs wiring.
- Decision: code-change.

### Existence Check

- New surfaces: `WorkflowRuntime`, `journal.jsonl`, a cloneable
  canonical workflow dispatch handle, and workflow task adapters.
- Reuse candidates: recorder, session JSONL, task snapshots, direct registry run.
- Why insufficient: non-durable, projection-only, duplicate-owner, or permission
  bypass.
- Creation proof: six defects require durable intent before external effects.
- Entropy control: retire recorder/direct-run/copied-state paths in same work.
- Decision: add-with-proof for workflow runtime/journal/dispatch handle; reuse
  existing permission, shell, multi-agent, task, notification, and TUI owners.

### Architecture Integrity Lens

- Invariant: terminal workflow implies terminal child records; no effect starts
  before durable invocation intent.
- Canonical owner: WorkflowRuntime for workflow truth, existing runtime owners
  for child effects and projections.
- Overlap removed: recorder, task snapshot, and transcript are not state owners.
- Simplification: one `run_id`, one journal, one dispatch path, one task
  control backend.
- Falsifier: finish with running child, automatic incomplete-effect retry, or
  TaskOutput answered from copied state.
- Verdict: proceed.

### Plan Pressure Test

- Owner/contract/retirement: every task names owner and deletion boundary.
- Architecture: oversized generic files receive wiring only.
- Verification: every contract has a focused deterministic target.
- Executability: tasks are dependency-ordered and commit-bounded.
- Result: proceed.

### Complexity Budget

- Artifact class: source, test, and plan complexity.
- Existing pressure: `background_tasks.rs`, `permission.rs`, and
  `tool_dispatch.rs` exceed 1,200 lines; `lua.rs` mixes VM,
  host API, recorder, dispatch, and projection.
- Projected pressure: over-budget if durable logic is added in generic owners.
- Result: at-risk.
- Governance: add cohesive workflow modules; generic owners receive narrow
  registration/dispatch/control wiring; split tests by contract.

### Execution Readiness View

- Intent Lock: implement the approved spec; no Claude parity or saved library.
- Scope Fence: only listed workflow/runtime/permission/task/session/TUI/docs
  owners; ignore unrelated dirty worktree changes.
- Baseline Lock: four Aegis specs plus repository agent instructions.
- Owner constraints: WorkflowRuntime owns truth; BackgroundTaskManager adapts.
- Compatibility: historical events are read-only; no old execution.
- Retirement: foreground, recorder, bypass, aliases, copied state die.
- Batches: Tasks 1-10 in order; review after Tasks 2, 5, 7, and 10.
- Test obligations: exact per-task nextest/rustfmt/diff checks and final matrix.
- Drift rule: stop for new owner, fallback, persistent migration, token
  prediction, or changed child-card design.
- Completion evidence: all acceptance groups, negative retirement searches,
  historical fixture, cross-platform persistence, clean scoped verification.
- Advisory boundary: execution guidance, not runtime authority.

## File Map

### New workflow-owned files

- `crates/neo-agent-core/src/workflow/journal.rs`: append/read journal,
  record validation, SHA-256 canonical input hashing, byte reservation.
- `crates/neo-agent-core/src/workflow/runtime.rs`: run owner, worker,
  replay/recovery, pause/resume/stop, snapshots.
- `crates/neo-agent-core/src/workflow/limits.rs`: runtime defaults and
  Lua/journal/token accounting.
- `crates/neo-agent-core/src/workflow/capability.rs`: one-shot
  `/workflow` capability store.
- `crates/neo-agent-core/src/runtime/workflow_dispatch.rs`: cloneable
  bridge to canonical tool dispatch.

### Existing files expected to change

`workflow/{mod.rs,lua.rs,state.rs,error.rs}`,
`tools/workflow.rs`, `tools/background_tasks.rs`,
`tools/mod.rs`, `runtime/{config.rs,permission.rs,tool_dispatch.rs,events.rs,agent.rs}`,
`permissions.rs`, `approval.rs`, `events.rs`,
`session/{mod.rs,event_persistence.rs}`, interactive slash/turn/input/
task-browser modules, config mapping, and TUI workflow/task-browser modules.

### New/updated focused test targets

- `crates/neo-agent-core/tests/workflow_journal.rs`
- `crates/neo-agent-core/tests/workflow_runtime.rs`
- `crates/neo-agent-core/tests/workflow_dispatch.rs`
- `crates/neo-agent-core/tests/workflow_lua.rs` (replace old assertions)
- `crates/neo-agent/tests/workflow_controls.rs`
- `crates/neo-agent/tests/workflow_notifications.rs`
- `crates/neo-tui/tests/workflow_transcript.rs`
- existing `crates/neo-tui/tests/transcript_store.rs` where historical
  workflow replay needs coverage.

## Implementation Tasks

### Task 1: Replace workflow state types and add durable journal

**Files**

- Create `crates/neo-agent-core/src/workflow/journal.rs` and
  `workflow/limits.rs`.
- Modify `workflow/state.rs`, `workflow/mod.rs`, and
  `workflow/error.rs`.
- Create `crates/neo-agent-core/tests/workflow_journal.rs`.

**Why / change necessity**

Later slices require a durable state/outcome/journal contract. Extending the
recorder would preserve the wrong owner. Journal I/O and limits are new cohesive
workflow responsibilities; session JSONL and BackgroundTaskManager remain
projections/adapters.

**Required contract**

Define/export:

~~~rust
pub enum WorkflowState {
    Running, Paused, Completed, Failed, Cancelled, ResourceLimited,
}
pub enum WorkflowActor { Human, Model, Runtime }
pub enum WorkflowInvocationKind {
    Phase, Log, Delegate, Swarm, Verify, VerifyCommand, Report, Fail,
}
pub struct WorkflowPhase { pub id: String, pub description: String }
pub struct WorkflowRunMetadata {
    pub run_id: WorkflowId,
    pub parent_run_id: Option<WorkflowId>,
    pub name: String,
    pub description: String,
    pub phases: Vec<WorkflowPhase>,
    pub script: String,
    pub script_sha256: String,
    pub args: serde_json::Value,
    pub launch_source: String,
    pub journal_format_version: u32,
}
pub struct WorkflowInvocationOutcome {
    pub ok: bool,
    pub status: WorkflowOutcomeStatus,
    pub summary: String,
    pub details: serde_json::Value,
    pub actual_usage: Option<ActualUsage>,
    pub child_refs: Vec<WorkflowChildRef>,
}
~~~

Use the existing actual-usage type if available. Do not create a second token
usage model.

Journal records have common monotonic sequence/timestamp and exactly three
variants: `state_changed`, `invocation_started`, and
`invocation_finished`. Start records are durable before external
effects. Canonical input is recursively key-sorted, compact UTF-8, and SHA-256
lowercase hex.

**Steps**

1. Replace the three-state enum and recorder-shaped projection fields while
   retaining serde defaults needed for read-only historical events.
2. Implement journal append/read/sequence validation and atomic metadata
   creation.
3. Enforce 16 MiB record size and 4 GiB journal reservation. Before an
   invocation reserve its start, one max-size finish, and a 64 KiB terminal
   tail; if insufficient, record resource-limited without starting the effect.
4. Add focused tests for hash stability, record order, malformed sequence,
   incomplete invocation detection, record rejection, reservation, and
   PathBuf run directories.
5. Run and commit:

~~~bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/workflow/journal.rs crates/neo-agent-core/src/workflow/limits.rs crates/neo-agent-core/src/workflow/state.rs crates/neo-agent-core/src/workflow/error.rs
cargo nextest run -p neo-agent-core --test workflow_journal journal_writes_and_reads_append_only_records
cargo nextest run -p neo-agent-core --test workflow_journal incomplete_invocation_is_detected_without_reexecution
git diff --check
git commit -m "feat(workflow): add durable journal contracts"
~~~

**Repair/retirement**

Repair missing durable intent in the workflow owner. Do not add a recorder
adapter. Delete `WorkflowHostRecorder` after Task 4 migrates all Lua
consumers; no dual owner may be committed.

### Task 2: Implement WorkflowRuntime, replay, recovery, and limits

**Files**

- Create `crates/neo-agent-core/src/workflow/runtime.rs`.
- Modify workflow runtime/state/journal/limits/error files.
- Add only workflow-directory discovery hooks to
  `crates/neo-agent-core/src/session/mod.rs`.
- Create `crates/neo-agent-core/tests/workflow_runtime.rs`.

**Why / change necessity**

The journal needs one owner that launches background workers, replays the longest
matching prefix, reconciles incomplete calls, and exposes a task/control
handle. No existing recorder, task registry, or session transcript can do this.

**Required handle boundary**

Implement an owned, cloneable API equivalent to:

~~~rust
pub struct WorkflowRuntime { /* shared run registry */ }
impl WorkflowRuntime {
    pub async fn create_run(&self, request: WorkflowLaunchRequest)
        -> Result<WorkflowHandle, WorkflowError>;
    pub async fn snapshot(&self, run_id: &WorkflowId)
        -> Result<WorkflowSnapshot, WorkflowError>;
    pub async fn output(&self, run_id: &WorkflowId)
        -> Result<WorkflowOutput, WorkflowError>;
    pub async fn pause(&self, run_id: &WorkflowId, actor: WorkflowActor)
        -> Result<(), WorkflowError>;
    pub async fn resume(&self, run_id: &WorkflowId, actor: WorkflowActor)
        -> Result<(), WorkflowError>;
    pub async fn stop(&self, run_id: &WorkflowId, actor: WorkflowActor)
        -> Result<(), WorkflowError>;
    pub async fn rehydrate(&self, session_dir: &Path)
        -> Result<Vec<WorkflowHandle>, WorkflowError>;
}
~~~

The handle is the only object registered with BackgroundTaskManager. It reads
and mutates WorkflowRuntime; it does not own a copied snapshot.

**Steps**

1. Create immutable metadata and run directories at
   `<session_dir>/workflows/<run_id>/`.
2. Add per-run state, cancellation/pause tokens, current invocation, journal
   cursor, actual-usage accumulator, and event sink inside workflow-owned code.
3. Make metadata and the initial running journal durable before worker spawn and
   before RunWorkflow returns.
4. Implement prefix replay by
   `call_index + kind + canonical_input_hash` and switch to live at the
   first mismatch.
5. Reconcile incomplete invocations by `invocation_id`: adopt a known
   terminal child result; otherwise record `interrupted(host_exit)` and
   never execute that effect automatically.
6. Implement cooperative pause at durable invocation boundaries and stop
   cancellation with terminal child reconciliation.
7. Rehydrate dangling running runs as `paused(reason=host_exit)` without
   executing Lua, requesting permission, or opening a model turn.
8. Account actual provider usage; an active call finishes, then the next
   provider-backed call is blocked when an explicit cap is reached.
9. Add tests for immediate background return, terminal-child invariant, replay
   idempotency, incomplete no-retry, same-run resume, linked edited retry,
   pause/stop race, host-exit rehydration, and actual-only token cap.
10. Run and commit:

~~~bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/workflow/runtime.rs crates/neo-agent-core/src/workflow/state.rs crates/neo-agent-core/src/workflow/journal.rs crates/neo-agent-core/src/workflow/limits.rs
cargo nextest run -p neo-agent-core --test workflow_runtime workflow_returns_running_before_worker_finishes
cargo nextest run -p neo-agent-core --test workflow_runtime incomplete_invocation_is_never_reexecuted
cargo nextest run -p neo-agent-core --test workflow_runtime host_exit_rehydrates_running_run_as_paused
git diff --check
git commit -m "feat(workflow): add durable runtime and replay"
~~~

**Repair/retirement**

Do not add a workflow wall-clock timeout, VM/future serialization, or old-session
migration. Any workflow state field added to BackgroundTaskManager is a plan
violation.

### Task 3: Extract a canonical workflow tool-dispatch bridge

**Files**

- Create `crates/neo-agent-core/src/runtime/workflow_dispatch.rs`.
- Modify `runtime/tool_dispatch.rs`, `runtime/events.rs`,
  `tools/mod.rs`, and narrow runtime config wiring.
- Create `crates/neo-agent-core/tests/workflow_dispatch.rs`.

**Why / change necessity**

Current workflow calls `child_tools.run()`, skipping instruction
preflight, permission resolution, approval persistence, shell admission, and
canonical events. A runtime-owned bridge must reuse the existing dispatch core.

**Steps**

1. Extract shared preparation/authorization/execution phases from
   `execute_tool_calls` without changing ordinary model-batch behavior.
2. Build a cloneable `WorkflowDispatchHandle::run_one` carrying the session's
   current model/provider resolver, registry, skills, process supervisor,
   instruction state, live permission, approval handler, and event sink.
   Resolve model/provider for every new live invocation so `/model` and
   `/provider` changes affect resumed/new calls. Serialize state mutations only;
   never hold a mutex across provider/shell awaits.
3. Use `invocation_id` as the generated AgentToolCall ID and
   `run_id` as workflow/task/card identity. Keep existing child IDs and
   card layouts.
4. Map instruction Defer/Block to typed workflow outcome; no effect runs and the
   run pauses `instruction_replan_required` without auto turn.
5. Reuse existing event/shell callbacks and `emit_authorized_call_result`;
   do not duplicate event mapping.
6. Test exact verify_command command/cwd approval, typed denial, instruction
   deferral without shell effect, and normal Delegate/Swarm lifecycle events.
7. Run and commit:

~~~bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/runtime/workflow_dispatch.rs crates/neo-agent-core/src/runtime/tool_dispatch.rs crates/neo-agent-core/src/runtime/events.rs crates/neo-agent-core/src/tools/mod.rs
cargo nextest run -p neo-agent-core --test workflow_dispatch verify_command_uses_canonical_bash_permission_path
cargo nextest run -p neo-agent-core --test workflow_dispatch instruction_replan_blocks_effect_without_model_turn
git diff --check
git commit -m "refactor(runtime): share canonical workflow dispatch"
~~~

**Repair/retirement**

No fallback to `ToolRegistry::run` in workflow code. Existing ordinary
model and child runtime dispatch behavior remains supported.

### Task 4: Replace Lua recorder execution with strict resumable host APIs

**Files**

- Modify `crates/neo-agent-core/src/workflow/lua.rs`,
  `workflow/error.rs`, `workflow/state.rs`, and runtime bridge.
- Delete `crates/neo-agent-core/src/workflow/host_api.rs` after migration.
- Replace old assertions in `crates/neo-agent-core/tests/workflow_lua.rs`.

**Why / change necessity**

The recorder is not durable and current Lua hosts forward weak raw JSON. V1
needs strict deterministic APIs, read-only args, typed outcomes, and VM limits.

**Steps**

1. Replace `LuaWorkflowRunner { recorder }` with a runner constructed
   from the invocation bridge and limits. Delete recorder-only
   `run_script()`.
2. Install exactly `neo.phase`, `neo.log`,
   `neo.delegate`, `neo.swarm`, `neo.verify`,
   `neo.verify_command`, `neo.report`, and `neo.fail`.
3. Install recursively read-only `args`; reject mutation with
   `invalid_workflow_operation`. Disable random, module/package,
   clock, raw I/O, process, network, and filesystem APIs while retaining
   deterministic `pcall/xpcall` handling.
4. Add the 10,000-instruction hook, 100,000,000 uninterrupted-instruction
   limit, and 256 MiB VM memory limit. Convert cancellation/pause/limit events
   into typed outcomes.
5. Decode exact delegate/swarm/command tables. Reject unknown fields,
   `mode=background`, `max_concurrency`, aliases, and raw
   parameter forwarding.
6. Return child failure outcomes normally. Raise verify failures as catchable
   outcome tables; make `neo.fail` terminal even if Lua catches.
7. Journal every host operation before/after effect through WorkflowRuntime.
8. Add tests for read-only args, disabled APIs, strict fields, child failure
   handling, catchable verify errors, fatal fail, phase validation, instruction
   limit, and memory limit.
9. Run and commit:

~~~bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/workflow/lua.rs crates/neo-agent-core/src/workflow/error.rs crates/neo-agent-core/src/workflow/state.rs
cargo nextest run -p neo-agent-core --test workflow_lua workflow_rejects_unknown_host_fields
cargo nextest run -p neo-agent-core --test workflow_lua workflow_args_are_recursively_read_only
cargo nextest run -p neo-agent-core --test workflow_lua infinite_lua_hits_instruction_resource_limit
git diff --check
git commit -m "feat(workflow): replace recorder with strict Lua host"
~~~

**Repair/retirement**

Delete `WorkflowHostRecorder` and recorder-only tests; no adapter exists
only to keep retired tests green.

### Task 5: Add exact /workflow capability and launch approval

**Files**

- Create `crates/neo-agent-core/src/workflow/capability.rs`.
- Modify `workflow/mod.rs`, `permissions.rs`,
  `approval.rs`, `runtime/permission.rs`,
  `runtime/config.rs`, `tools/workflow.rs`, and
  `tools/mod.rs`.
- Modify interactive `mod.rs`, `slash_commands.rs`, and
  `prompt_completion.rs`.
- Create `crates/neo-agent/tests/workflow_controls.rs`.

**Why / change necessity**

`RunWorkflow` must not be forgeable by ordinary text or permission
mode. Exact `/workflow` grants one capability; Ask additionally reviews
the complete script, while Auto/Yolo may launch once capability exists.

**Steps**

1. Implement session-scoped capability grant/inspect/consume-on-durable-success/
   revise-preserve/cancel-revoke/`/new` reset/process-exit semantics.
   Never expose a token string to Lua/model.
2. Thread the shared store through AppConfig/AgentConfig/ToolContext like live
   permission and background task state.
3. Handle only exact `/workflow` in slash parsing/completion/help.
   `/workflow <anything>` must not grant capability.
4. Add `PermissionOperation::WorkflowLaunch`,
   `ApprovalPresentation::Workflow`, and typed approve/revise actions.
   Reuse canonical approval option objects; never parse labels or indices.
5. Ask presentation contains complete metadata, args, full scrollable source,
   and the orchestration-only warning. Revise preserves capability; Cancel
   creates no run state.
6. Gate RunWorkflow before durable creation; consume only after metadata and
   initial journal are durable. Auto/Yolo skip the second launch prompt only
   after capability validation and record source/mode.
7. Test ordinary-prompt denial, one-shot grant, invalid non-consumption,
   Ask revise/cancel, Auto/Yolo no-bypass, and fresh capability for edited run.
8. Run and commit:

~~~bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/workflow/capability.rs crates/neo-agent-core/src/permissions.rs crates/neo-agent-core/src/approval.rs crates/neo-agent-core/src/runtime/permission.rs crates/neo-agent-core/src/tools/workflow.rs crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/prompt_completion.rs
cargo nextest run -p neo-agent --test workflow_controls exact_workflow_slash_grants_one_capability
cargo nextest run -p neo-agent --test workflow_controls auto_mode_cannot_bypass_missing_workflow_capability
git diff --check
git commit -m "feat(workflow): add launch capability and approval"
~~~

**Repair/retirement**

Do not infer capability from `PermissionMode`, `/auto`,
`/yolo`, prompt text, AGENTS guidance, or JSON fields. Do not add a
second launch command.

### Task 6: Add shared model/human task controls

**Files**

- Modify `tools/background_tasks.rs` and register TaskPause/TaskResume
  in `tools/mod.rs`.
- Modify `neo-tui/src/tasks_browser/state.rs`,
  `neo-agent/src/modes/task_browser.rs`, and interactive
  `input.rs`.
- Extend `crates/neo-agent/tests/workflow_controls.rs` and add core
  control tests if no existing target covers them.

**Why / change necessity**

Human and model controls must call the same WorkflowRuntime handle. The task
manager remains an adapter, not a workflow state owner.

**Steps**

1. Add a background record kind for a workflow query/control handle whose
   snapshot delegates to WorkflowRuntime.
2. Add strict TaskPause/TaskResume inputs. Restrict mutation to workflow
   handles; other task kinds return typed unsupported results. Existing
   TaskStop routes through the same handle.
3. Register model tools with descriptions that state orchestration control is
   not predictive cost governance.
4. Extend task-browser actions with pause/resume confirmation for workflow rows;
   preserve all existing stop/non-workflow behavior.
5. Test model/browser control equivalence, pause boundary, stop cancellation,
   and unsupported task kinds.
6. Run and commit:

~~~bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/tools/background_tasks.rs crates/neo-agent-core/src/tools/mod.rs crates/neo-tui/src/tasks_browser/state.rs crates/neo-agent/src/modes/task_browser.rs crates/neo-agent/src/modes/interactive/input.rs
cargo nextest run -p neo-agent --test workflow_controls model_and_human_controls_share_workflow_handle
cargo nextest run -p neo-agent --test workflow_controls task_browser_pause_resume_only_targets_workflows
git diff --check
git commit -m "feat(tasks): control workflows through the task surface"
~~~

**Repair/retirement**

Do not add `neo.pause`/`neo.resume`/`neo.stop`.
Do not add workflow internals to BackgroundTaskRecord beyond the control/query
handle.

### Task 7: Rehydrate sessions and queue exactly-once notifications

**Files**

- Modify `session/mod.rs` and `session/event_persistence.rs` only
  for recovery/projection hooks.
- Modify `runtime/events.rs`, `runtime/queue.rs`, and
  `runtime/agent.rs` for typed notification delivery.
- Modify interactive `mod.rs`, `turn.rs`, and
  `sessions.rs` as needed.
- Create `crates/neo-agent/tests/workflow_notifications.rs`.

**Why / change necessity**

Terminal workflows notify the next natural model turn once, without interrupting
an active turn or injecting fake user input. Host exit rebuilds paused runs
without auto-execution.

**Steps**

1. Define a typed notification with deterministic ID from `run_id` and
   terminal/recovery reason, distinct from user follow-up and steering queues.
2. Queue only after terminal journal durability; deduplicate by ID.
3. At the next natural idle model turn, append one internal reminder to call
   `TaskOutput`. Never start a turn from the notification.
4. Persist only delivery/projection identity needed for deduplication; workflow
   state remains journal-owned.
5. Rehydrate workflow directories, rebuild handles/cards, mark dangling running
   runs `paused(host_exit)`, and enqueue one recovery notification.
6. Test active-turn deferral, idle natural-turn delivery, duplicate recovery,
   exactly-once projection, and no auto-turn behavior.
7. Run and commit:

~~~bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/session/mod.rs crates/neo-agent-core/src/session/event_persistence.rs crates/neo-agent-core/src/runtime/events.rs crates/neo-agent-core/src/runtime/queue.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/turn.rs
cargo nextest run -p neo-agent --test workflow_notifications terminal_workflow_notification_waits_for_natural_turn
cargo nextest run -p neo-agent --test workflow_notifications host_exit_recovery_does_not_start_model_turn
git diff --check
git commit -m "feat(workflow): recover runs and queue notifications"
~~~

**Repair/retirement**

Do not reuse the background-question helper that immediately starts a follow-up
turn. Do not represent the notification as user-authored input.

### Task 8: Project workflow state and preserve child-card boundaries

**Files**

- Modify `crates/neo-agent-core/src/events.rs` only for minimal workflow
  projection event changes.
- Modify `crates/neo-tui/src/transcript/workflow_card.rs`,
  `transcript/event_handler.rs`, and `transcript/store.rs`.
- Keep Delegate-family/Bash/Terminal card files unchanged unless compile-only
  adaptation is unavoidable.
- Update `crates/neo-tui/tests/workflow_transcript.rs` and
  `crates/neo-tui/tests/transcript_store.rs`.

**Why / change necessity**

Workflow cards show orchestration without duplicating child content. Historical
events remain readable and TranscriptStore remains the projection-boundary owner.

**Steps**

1. Project run ID, name/state/current phase, elapsed/invocation counts, actual
   usage, orchestration summaries, local failures, and terminal reason.
2. Preserve serde defaults for historical WorkflowStarted/Updated/Finished
   fixtures; never reconstruct a runnable workflow from events.
3. Emit projection updates through existing event sink and upsert/mutate; do not
   create a new visible boundary for an update.
4. Render no child transcript/result/complete shell command. Existing child cards
   keep layout, budgets, ordering, and expansion unchanged.
5. Verify verify_command delegates to the ordinary Bash card with exact command/
   cwd, syntax highlighting, safe wrapping, explicit omission, and Ctrl+O.
6. Test terminal/paused/resource-limited cards, historical replay, no child
   duplication, transcript boundary safety, and Bash presentation delegation.
7. Run and commit:

~~~bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/events.rs crates/neo-tui/src/transcript/workflow_card.rs crates/neo-tui/src/transcript/event_handler.rs crates/neo-tui/src/transcript/store.rs
cargo nextest run -p neo-tui --test workflow_transcript workflow_card_projects_orchestration_without_child_duplication
cargo nextest run -p neo-tui --test workflow_transcript historical_workflow_events_remain_read_only
cargo nextest run -p neo-tui --test transcript_store workflow_updates_do_not_break_active_text_boundary
git diff --check
git commit -m "feat(tui): project durable workflow state"
~~~

**Repair/retirement**

Any child-card layout/content/expansion change is out of scope and must be
reverted before commit.

### Task 9: Add runtime configuration and synchronized docs

**Files**

- Modify `crates/neo-agent/src/config/{mod.rs,types.rs,loader.rs}` and
  `crates/neo-agent-core/src/runtime/config.rs`.
- Modify mapping in `crates/neo-agent/src/modes/run/runtime/agent.rs`.
- Modify matching English/Chinese tools, slash-command, and data-location docs:
  `docs/en/reference/tools.md`,
  `docs/zh/reference/tools.md`,
  `docs/en/reference/slash-commands.md`,
  `docs/zh/reference/slash-commands.md`,
  `docs/en/configuration/data-locations.md`, and its Chinese file.
- Add focused config/schema tests under `workflow_controls.rs` or the
  existing config target.

**Why / change necessity**

Machine limits belong to runtime configuration; model schemas cannot provide
them. User-facing docs must describe durable files, exact capability, controls,
pause behavior, actual-only usage, and no-default-token-cap in both languages.

**Steps**

1. Add one serializable workflow config section with defaults: source 1 MiB,
   VM memory 256 MiB, hook interval 10,000, uninterrupted instructions
   100,000,000, journal record 16 MiB, journal 4 GiB, swarm concurrency 4,
   optional token cap `None`, and no wall-clock timeout.
2. Map defaults once into `WorkflowLimits`; do not duplicate constants.
3. Keep model input free of `limits`, `max_concurrency`, and
   projected cost/usage.
4. Document run files at `<session_dir>/workflows/<run_id>/`,
   `/workflow`, Ask/Auto/Yolo, TaskPause/Resume/Stop, TaskOutput,
   boundary pause, actual-only usage, and historical-session behavior.
5. Add tests for TOML defaults, explicit token cap, and schema rejection of
   model-supplied limits.
6. Run and commit:

~~~bash
rustfmt --check --edition 2024 crates/neo-agent/src/config/mod.rs crates/neo-agent/src/config/types.rs crates/neo-agent/src/config/loader.rs crates/neo-agent-core/src/runtime/config.rs crates/neo-agent/src/modes/run/runtime/agent.rs
cargo nextest run -p neo-agent --test workflow_controls workflow_machine_defaults_leave_token_cap_unbounded
cargo nextest run -p neo-agent --test workflow_controls workflow_schema_rejects_model_limits
git diff --check
git commit -m "docs(workflow): expose runtime controls and contracts"
~~~

**Repair/retirement**

Do not add cost estimators, projected usage fields, fuzzy warnings, or
model-supplied limits. Leave unrelated runtime config semantics unchanged.

### Task 10: Delete residual legacy paths and run final acceptance

**Files**

- All workflow files changed in Tasks 1-9.
- New/updated workflow test targets.
- Narrow existing historical-session fixtures.
- The design spec only if implementation review finds a factual contract error;
  never silently change a product decision.

**Why / change necessity**

The migration is incomplete while old foreground/recorder/bypass/alias paths are
reachable. Final acceptance must prove the main path and negative retirement.

**Steps**

1. Delete old recorder exports, synchronous result formatting, old step state
   ownership, unused imports, and `host_api.rs`.
2. Run:

~~~bash
rg -n "WorkflowHostRecorder|install_recorder_neo_table|run_script\\(|child_tools\\.run|title.*script|mode=background|max_concurrency" crates/neo-agent-core/src crates/neo-agent-core/tests crates/neo-tui/src crates/neo-tui/tests
~~~

Active workflow-path hits must be zero. Historical spec/design mentions are
allowed and must not be changed just to silence this search.

3. Complete focused acceptance tests for capability, lifecycle, canonical
   permission, journal/recovery, failure states, limits, projections,
   retirement, old-session historical replay, and PathBuf cross-platform
   behavior.
4. Run final exact targets:

~~~bash
cargo nextest run -p neo-agent-core --test workflow_journal journal_writes_and_reads_append_only_records
cargo nextest run -p neo-agent-core --test workflow_runtime terminal_workflow_has_terminal_child_records
cargo nextest run -p neo-agent-core --test workflow_dispatch verify_command_uses_canonical_bash_permission_path
cargo nextest run -p neo-agent-core --test workflow_lua infinite_lua_hits_instruction_resource_limit
cargo nextest run -p neo-agent --test workflow_controls exact_workflow_slash_grants_one_capability
cargo nextest run -p neo-agent --test workflow_notifications terminal_workflow_notification_waits_for_natural_turn
cargo nextest run -p neo-tui --test workflow_transcript workflow_card_projects_orchestration_without_child_duplication
~~~

5. Run scoped rustfmt and `git diff --check` for every touched file.
6. Review staged diff for owner integrity, no fallback growth, no token
   prediction, no child-card changes, and no unrelated dirty-worktree changes.
7. Commit:

~~~bash
git diff --check
git commit -m "refactor(workflow): retire legacy execution paths"
~~~

**Repair/retirement**

The canonical owner must carry every behavior and negative searches must prove
old paths are dead. Historical session data is retained and only projection-
tested.

## Final Review and ADR Backfill

Before claiming implementation complete:

1. Confirm only authorized files changed with `git status --short`.
2. Run all exact acceptance commands, scoped rustfmt, and `git diff --check`.
3. Perform an independent architecture review focused on terminal-child
   invariant, no duplicate state owners, canonical permission, incomplete
   no-retry, no default cost prediction, unchanged child cards, and complete
   retirement.
4. Use Aegis `recording-architecture-decisions` to decide whether the
   durable owner/replay contract needs an ADR and whether the architecture
   baseline needs synchronization. Do not create an ADR before implementation
   evidence exists.
5. Use `verification-before-completion` and report covered/uncovered
   scope, residual risk, and confidence. Do not claim provider-level or
   cross-platform runtime success from local unit tests alone.

## Risks and Stop Conditions

Stop and return to the spec owner if:

- canonical dispatch requires a second permission system;
- incomplete external effects would be automatically retried;
- current model/provider switching cannot affect new live calls;
- instruction preflight would require Lua to summarize/bypass new rules;
- pause/stop leaves a running child without terminal journal record;
- a second task registry, notification queue, or transcript state owner appears;
- finite default token/cost/agent estimates appear;
- Delegate-family card design must change;
- historical session deletion/migration becomes necessary; or
- a request exceeds the approved compatibility/non-goal boundary.

Do not revert unrelated dirty-worktree changes. Isolate failures and report
unrelated baseline interference.

## Handoff Stop Condition

The plan is complete when Tasks 1-10, focused evidence, negative retirement
searches, historical fixture checks, cross-platform persistence checks, and the
final architecture/ADR decision are complete. Do not broaden into a saved
workflow library, Claude parity, daemon, provider/cache redesign, or global
session/context rewrite.
