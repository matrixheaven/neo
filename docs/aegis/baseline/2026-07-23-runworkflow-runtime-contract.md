# RunWorkflow Runtime Contract Baseline

Status: `recorded-from-adr`
Date: `2026-07-23`
ADR: `docs/aegis/adr/ADR-0004-durable-runworkflow-runtime.md`

## Product / Requirement Baseline

- `RunWorkflow` accepts exactly `name`, `description`, `phases`, `script`, and
  `args`; unknown fields and model-supplied execution limits are rejected.
- The exact `/workflow` command grants one session-scoped launch capability.
  A launch always starts in the background, consumes one capability only after
  durable creation and task registration, and returns a `run_id` that is also
  the background task ID.
- Human and model callers use the same `TaskPause` and `TaskResume` controls;
  `TaskStop` cancels and `TaskOutput` reads the runtime aggregate. A pause takes
  effect at an invocation boundary and does not abandon a running child.
- Launch approval authorizes orchestration only. Delegate, DelegateSwarm, and
  Bash effects still pass through the ordinary instruction and permission path.
- Provider usage is recorded only from actual outcomes. An omitted
  `runtime.workflow.token_cap` is unbounded, and Neo does not predict token,
  cost, time, or agent usage or pause/degrade a workflow from a prediction.

## Architecture / Runtime Boundary Baseline

- `WorkflowRuntime` is the canonical owner of workflow lifecycle, control,
  durable invocation identity, replay, recovery, and aggregate output.
  `BackgroundTaskManager` is a query/control adapter, not a state owner.
- `<session_dir>/workflows/<run_id>/run.json` is immutable launch metadata.
  Append-only `journal.jsonl` is the sole durable truth for current state,
  control transitions, invocation intent/outcome, child references, and actual
  provider usage.
- Journal records have contiguous sequence numbers and validated canonical
  input hashes. An invocation start is synced before its external effect, and
  its finish is synced before a terminal workflow transition.
- If an invocation outcome exceeds the record or reserved total limit after its
  external effect completed, a compact `ResourceLimited` finish replaces only
  oversized summary/details and preserves available actual provider usage and
  all canonical child/task references.
- Replay identity is `call_index + invocation kind + canonical input hash`.
  Only a matching completed prefix is replayed. Recovery records an incomplete
  effect as `interrupted(host_exit)` and never executes it automatically.
- Every started child must have a terminal journal outcome before the workflow
  can enter Completed, Failed, Cancelled, or ResourceLimited.
- The strict Lua host exposes only `neo.phase`, `neo.log`, `neo.delegate`,
  `neo.swarm`, `neo.verify`, `neo.verify_command`, `neo.report`, and `neo.fail`.
  Arguments are recursively read-only, unknown fields are rejected, and host
  machine limits are not forwarded from script input.
- Session JSONL events and the TUI `WorkflowSnapshot` are projections. Durable
  journal sequence is the ordering watermark; projections do not own runtime
  state and do not duplicate child-card content.
- Startup, session switching, and `neo run --continue` share the same rehydrate
  and handle-registration path. Rehydration does not start a worker; a stale
  Running state becomes Paused with `host_exit` after incomplete effects are
  reconciled.
- Rehydration binds the shared `WorkflowDispatchResolver` and prewarms
  session-scoped model/tool/skill/context dependencies only for resumable runs.
  Prewarm failure does not block session loading; a resume without dispatch
  dependencies returns to an inspectable Paused state.
- If invocation or workflow terminalization cannot be journaled, the runtime
  clears the worker/current invocation and publishes an unsequenced
  recovery-failure projection. This is explicitly not a durable terminal
  record and never authorizes automatic retry of an external effect.

## Configuration And Compatibility

- `[runtime.workflow]` supplies partial host-owned overrides that resolve into
  one validated `WorkflowLimits` value. The active runtime remains the live
  authority across configuration reloads.
- Machine-safety limits cover Lua instructions, pause-hook cadence, log/report
  sizes, journal record/total bytes, host-injected swarm concurrency, and the
  optional actual-usage token cap. There is no workflow wall-clock timeout.
- Persistence uses `Path` and `PathBuf`. Parent-directory synchronization is
  Unix-gated with a portable non-Unix path; workflow code does not assume a
  shell, Unix signal, or Unix permission model.
- Existing Delegate, DelegateGroup, DelegateSwarm, Bash, and Terminal tools and
  card designs are unchanged. Historical workflow session events remain
  readable projections, but cards without durable workflow files cannot resume.

## Retirement Boundary

- Retired without fallback: `WorkflowHostRecorder`, foreground synchronous
  execution, `run_script`, `host_api.rs`, `child_tools.run`, workflow-local
  direct registry dispatch, `mode=background`, model-supplied concurrency and
  resource limits, aliases, and copied runtime state in task/session/TUI owners.
- Existing session and workflow artifacts are not deleted or migrated by this
  change. Historical compatibility is projection-only and cannot regain runtime
  ownership.

## Verification Boundary

- Deterministic exact tests cover journal validation, invocation/terminal
  ordering, canonical Bash permission, dispatch lease routing, Lua instruction
  limits, one-shot launch capability, recovery notifications, TUI projection,
  the unbounded token default, and exact model-schema rejection.
- Retirement searches must have zero active hits for the removed workflow
  recorder, host API, direct runner, direct child dispatch, foreground mode,
  aliases, or model-owned limits. Ordinary Delegate/Swarm concurrency remains a
  separate supported subsystem.
- Release-grade evidence still requires provider-backed live execution, native
  Windows and 32-bit runtime checks, and visual TUI interaction. Current local
  deterministic evidence does not cover those surfaces.

## Residual Risk

- A host crash can leave an external effect incomplete; recovery deliberately
  records interruption instead of guessing whether the effect is safe to retry.
- Unrecoverable journal I/O can leave the last durable state non-terminal; the
  live recovery-failure projection makes that uncertainty inspectable, but a
  later process must still reconcile from the last valid journal prefix.
- The append-only journal and replay identity are durable formats. Future format
  changes need explicit versioning and migration or read-compatibility review.
- Whole-crate Clippy is currently blocked by an unrelated shell-guard lint; the
  accepted evidence is workflow-scoped rather than a claim that all core lint is
  green.
