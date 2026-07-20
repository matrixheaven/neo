# RunWorkflow Neo-Native Dynamic Workflow Design

## Status

Approved direction recorded on 2026-07-20. Written review is required before
implementation planning.

Architecture review required: yes. This design changes a model-facing tool
contract, runtime ownership, persistence, approval, background-task control,
resume behavior, resource limits, and transcript projection.

## Summary

Neo will replace the current foreground, one-shot `RunWorkflow` implementation
with a Neo-native durable dynamic workflow runtime.

`RunWorkflow` always launches in the background. `WorkflowRuntime` owns Lua
execution, immutable run metadata, the append-only invocation journal, state
transitions, replay, recovery, and result snapshots. `BackgroundTaskManager`
only exposes the workflow through the canonical task query/control surface.
Session JSONL and TUI cards are projections, not workflow state.

The redesign deliberately does not pursue full Claude Code parity. It keeps Lua
and a small orchestration API, reuses Neo's permission, instruction, shell,
multi-agent, background-task, approval, notification, and transcript owners,
and deletes the old foreground/recorder paths.

## Problem

The current implementation has six structural defects:

1. `neo.verify_command` calls Bash through `child_tools.run()` and cannot obtain
   shell permission through the canonical runtime dispatch path.
2. Delegate and swarm calls can use background semantics, allowing a workflow
   to report completion while children still run.
3. The Lua VM and workflow persistence have no instruction, memory, journal,
   or orchestration-specific resource boundaries.
4. The model-facing input schema and prompt expose only `title` and `script`,
   while host functions accept weakly validated forwarded JSON.
5. Execution is foreground and one-shot; there is no durable resume after host
   exit.
6. Script source, commands, approvals, child execution, state transitions, and
   final results are split across in-memory recorder data and transcript events,
   making the run insufficiently inspectable and non-recoverable.

These are ownership defects, not isolated missing guards. Adding permission
special cases, another recorder, or detached-child polling would retain the
same duplicate state and incomplete lifecycle.

## Product Direction

The product is **Neo-native Dynamic Workflow**, not a clone of every Claude
workflow feature.

The core user value is durable, inspectable orchestration of long and complex
work with explicit human/model controls and canonical Neo child execution.

The design does not predict task size. A small-looking planning task may use
roughly 100 million tokens, while a dynamic workflow may coordinate a very
large refactor. Neo must never ask a human or model to estimate tokens, cost,
agent count, or task scale, and must never auto-limit, auto-pause, auto-degrade,
or downscope work from such estimates.

Only observable actual usage and explicit controls may affect cost behavior.
Machine-safety limits protect the local process and disk; they are not cost
governance.

## Goals

- Make `RunWorkflow` always-background and immediately return a stable run ID.
- Ensure every started child reaches a terminal journal outcome before its
  workflow reaches a terminal state.
- Route every effectful host call through Neo's canonical instruction,
  permission, scheduler, tool, and event owners.
- Persist enough source and invocation history to resume deterministically
  without serializing a Lua VM or future.
- Give both humans and models pause, resume, stop, list, and output controls.
- Make workflow source, phase/log/report events, child references, commands,
  approvals, actual usage, local failures, and final state inspectable.
- Keep Delegate, DelegateGroup, DelegateSwarm, Bash, and Terminal presentation
  owned by their existing components.
- Use strict model-facing schemas with one canonical contract and no aliases.
- Protect the Lua process and journal with explicit high machine-safety limits.
- Delete the old synchronous and recorder-based implementation.

## Non-Goals

- Full Claude Code workflow parity.
- A project/user named workflow library in V1.
- `/workflow <name>` discovery or packaging saved workflows as Skills.
- JavaScript, a declarative workflow DSL, or a second workflow language.
- Lua VM, coroutine, Tokio future, or provider-stream serialization.
- Recursive workflows, generic `neo.tool`, detached child controls, arbitrary
  pipelines, or a second concurrency primitive.
- Predictive token/cost/agent/task estimates or fuzzy cost warnings.
- A default workflow wall-clock timeout or default workflow token cap.
- A Lua-specific permission system, shell executor, child scheduler, task
  registry, notification queue, or transcript store.
- Migrating historical one-shot workflow events into durable runs.
- Changing the existing Delegate-family card design.

## Baseline Alignment

### Product / Requirement Baseline

The approved conversation decisions in this design are the requirement source.
The target is durable orchestration with canonical Neo permissions, explicit
control, no predictive cost governance, and complete inspectability.

Result: `Design Defect`, scope requirements. The old foreground/weak-schema
contract cannot satisfy the target behavior.

### Architecture / Runtime Boundary Baseline

The design must remain aligned with:

- `docs/aegis/specs/2026-07-17-canonical-approval-protocol-design.md`
- `docs/aegis/specs/2026-07-18-shell-admission-scheduler-design.md`
- `docs/aegis/specs/2026-07-20-bash-terminal-tool-card-brief.md`
- `docs/aegis/specs/2026-07-13-transcript-boundary-semantics-design.md`

Those baselines require runtime-owned typed approvals, transparent shell
admission without implicit timeouts, inspectable shell commands, unchanged
Delegate-family presentation, and transcript-store ownership of projection
boundaries.

Result: aligned after redesign. `WorkflowRuntime` becomes the workflow owner
without displacing those existing owners.

## Canonical Ownership

| Surface | Canonical owner |
| --- | --- |
| Run source, journal, state, replay, recovery, aggregate result | `WorkflowRuntime` |
| Launch capability and launch approval resolution | runtime permission/approval pipeline |
| Child permission and instruction preflight | canonical runtime tool dispatch |
| Bash/Terminal process admission and execution | `ShellRuntime` |
| Delegate/Swarm lifecycle and actual usage | multi-agent runtime |
| Task list/output/control routing | `BackgroundTaskManager` |
| Completion notification queue | runtime notification owner |
| Workflow orchestration card | workflow transcript projection |
| Delegate/Swarm/Bash/Terminal cards | existing transcript components |
| Historical session conversation | session JSONL |

`BackgroundTaskManager` must not copy a workflow snapshot as mutable state. It
registers a query/control handle whose methods read or mutate
`WorkflowRuntime`.

## Identity Model

`run_id` is the single stable run identity.

- `run_id` is also the background `task_id`.
- The workflow transcript card uses the same ID.
- `TaskOutput`, `TaskPause`, `TaskResume`, and `TaskStop` take that ID.
- No independent workflow/background/transcript ID mapping exists.

Every Lua host call receives one stable `invocation_id`. It correlates journal
records, permission requests, Bash/Delegate/Swarm events, child references,
and transcript projections.

A retry of an edited or terminal run creates a new `run_id` and records only a
`parent_run_id`. No separate lineage entity is required.

## Model-Facing RunWorkflow Contract

The canonical input is strict and rejects unknown fields:

```json
{
  "name": "refactor-runtime",
  "description": "Refactor the runtime in reviewed phases",
  "phases": [
    {"id": "inspect", "description": "Map current ownership"},
    {"id": "implement", "description": "Apply scoped changes"},
    {"id": "verify", "description": "Run exact verification"}
  ],
  "script": "neo.phase('inspect')\n...",
  "args": {"target": "crates/neo-agent-core"}
}
```

Requirements:

- `name`, `description`, `phases`, and `script` are required.
- `args` is optional, defaults to an empty object, and is read-only in Lua.
- Every phase is a strict `{id, description}` object. IDs are non-empty and
  unique; descriptions are non-empty; unknown phase fields are invalid.
- `phases` supplies model/human intent metadata; Lua remains the execution
  authority and calls `neo.phase()` as control flow reaches a phase.
- Model-supplied workflow `limits` are invalid.
- Legacy `title`, mode, limit, parameter-forwarding, and alias fields are
  invalid.
- Source size is validated before launch capability consumption.
- The complete serialized immutable `run.json` must fit the same 16 MiB
  single-record safety boundary used for journal records.
- `RunWorkflow` returns immediately after durable creation and registration:

```text
task_id: <run_id>
kind: workflow
status: running
automatic_notification: true
next_step: Use TaskOutput with this task_id to inspect the workflow.
```

The complete terminal result is never injected into an active model turn.

## Lua Sandbox and Host API

Lua remains the only workflow language. The VM exposes no filesystem, process,
network, package/module loading, clock-based nondeterminism, or random API.
`math.random`, dynamic module loading, and equivalent nondeterministic surfaces
are removed.

V1 exposes exactly:

```text
neo.phase
neo.log
neo.delegate
neo.swarm
neo.verify
neo.verify_command
neo.report
neo.fail
```

There is no `parallel`, `pipeline`, recursive workflow, generic `neo.tool`, or
detached-task API. `neo.swarm` is the sole concurrency primitive. A pipeline
surface requires measured evidence that explicit Lua sequencing is inadequate.

All table-shaped inputs use strict typed decoding with unknown-field rejection.
`neo.delegate` and `neo.swarm` always await canonical child completion.
`mode=background` is rejected with `invalid_workflow_input`.
`neo.swarm` does not accept `max_concurrency`; runtime configuration owns it.

`neo.phase`, `neo.log`, and `neo.report` are journaled local runtime operations.
Effectful calls enter canonical tool dispatch. Lua never receives a raw
`ToolRegistry` or calls `child_tools.run()` directly.

### Exact V1 signatures

`args` is installed as a recursively read-only Lua table. Mutation fails with
`invalid_workflow_operation`.

| API | Input | Result |
| --- | --- | --- |
| `neo.phase(id)` | Declared non-empty phase ID | `nil`; unknown IDs are invalid input |
| `neo.log(message)` | Non-empty string | `nil` |
| `neo.delegate(input)` | Strict delegate table | Typed delegate outcome |
| `neo.swarm(input)` | Strict swarm table | Typed swarm outcome |
| `neo.verify(condition, message)` | Boolean and non-empty message | `nil` or catchable typed verification error |
| `neo.verify_command(input)` | Strict command table | Typed command outcome or catchable typed verification error |
| `neo.report(value)` | JSON-serializable Lua value | `nil` |
| `neo.fail(message)` | Non-empty string | Unconditionally marks the run failed and aborts Lua |

The strict delegate table accepts only:

```text
task: required non-empty string
resume: optional existing agent_id
title: optional non-empty string
role: optional canonical AgentRole
context: optional inherit | summary | none; default inherit
```

Workflow delegation always maps to canonical foreground/awaited execution.
There is no `mode` field.

The strict swarm table accepts only:

```text
description: required non-empty string
items: canonical array of { title, value }
prompt_template: required when items is non-empty
resume_agent_ids: optional canonical agent_id -> prompt map
role: optional canonical AgentRole
```

There is no `mode` or `max_concurrency` field. Existing canonical validation
for item counts, titles, values, placeholders, duplicate expanded prompts, and
resume agent IDs remains authoritative.

The strict command table accepts only:

```text
command: required non-empty string
cwd: optional typed path
failure_message: optional non-empty string
```

All host outcomes use one immutable Lua table shape:

```text
ok: boolean
status: completed | failed | denied | cancelled | resource_limited | interrupted
summary: string
details: table
actual_usage: optional table
agent_id | swarm_id | task_id: optional correlation fields
```

Delegate/Swarm failures return the table normally so Lua chooses policy.
`neo.verify` and `neo.verify_command` raise the same outcome table through a
Lua wrapper when `ok=false`; `pcall`/`xpcall` can catch it. If uncaught, the run
fails. `neo.fail` is terminal even if Lua attempts to catch the raised error.

## Child Result and Failure Contract

Delegate and swarm failures return typed script-visible outcomes. They do not
automatically fail the workflow.

Lua decides whether to retry, degrade, report, or escalate. Local failures
remain visible in aggregate `TaskOutput` metadata even if the script eventually
completes successfully.

The workflow fails only for:

- an unhandled Lua error;
- an unhandled host error;
- `neo.fail`;
- a failed `neo.verify`; or
- a failed `neo.verify_command` not handled through its typed outcome contract.

The terminal-child invariant is:

> A workflow may enter a terminal state only when every child it started has a
> terminal journal record.

## Workflow States

Canonical states are:

```text
running
paused
completed
failed
cancelled
resource_limited
```

`completed`, `failed`, `cancelled`, and `resource_limited` are terminal.

There is no workflow-level `queued`, `waiting_for_permission`,
`completed_with_failures`, `timed_out`, or default timeout state. Queue and
approval waiting remain invocation-level projections while the workflow stays
`running`.

A run found as `running` after process exit rehydrates as
`paused(reason=host_exit)`.

## Durable Storage

Each run directory is stored at:

```text
<session_dir>/workflows/<run_id>/
  run.json
  journal.jsonl
```

`run.json` is immutable after durable creation and contains:

- `run_id` and optional `parent_run_id`;
- name, description, phases, and args;
- exact Lua source and source hash;
- creation metadata and launch source;
- the journal format version.

`journal.jsonl` is append-only and contains only three record variants.

### state_changed

Contains sequence, timestamp, previous/new state, reason, and actor
`human | model | runtime`.

### invocation_started

Contains sequence, timestamp, `invocation_id`, `call_index`, host-call kind,
canonical input, and `canonical_input_hash`.

The record is durably appended before any external effect begins.

### invocation_finished

Contains sequence, timestamp, `invocation_id`, typed outcome, script-visible
result, actual provider usage when available, local failure metadata, and
child/task references.

The journal does not duplicate complete Delegate/Swarm transcript content.

The writer reserves enough capacity before every invocation for its serialized
`invocation_started`, one maximum-size `invocation_finished`, and a 64 KiB
terminal-state tail. If that reservation would cross the 4 GiB run limit, no
external effect starts and the reserved tail records `resource_limited`. This
prevents the safety limit itself from making the terminal state unrecordable.

Canonical input hashing recursively sorts JSON object keys and hashes the
resulting compact stable UTF-8 bytes with SHA-256, encoded as lowercase hex.
Neo already carries the workspace `sha2` dependency; no new dependency or
alternate hash algorithm is introduced.

## Replay and Resume

Resume re-executes the immutable Lua source. It does not serialize or restore a
Lua VM, coroutine, future, provider stream, or in-process child task.

At each host call, replay compares:

```text
call_index + kind + canonical_input_hash
```

The runtime returns recorded results for the longest completely matching
prefix. The first mismatch stops replay and starts live execution from that
call.

An `invocation_started` record without `invocation_finished` is never
automatically executed again. Recovery first reconciles the deterministic
`invocation_id` with the canonical child/background owner:

- if a terminal result exists, append `invocation_finished` from that result;
- if no terminal result can be established, complete it as
  `interrupted(host_exit)`;
- Lua explicitly decides whether to issue a new retry invocation.

This favors no duplicate effects over pretending exactly-once external
execution is possible.

An unchanged paused run resumes under the same `run_id`. Editing source or args,
or retrying any terminal run, creates a linked run with a new `run_id`.

## Host Exit Recovery

Session/runtime startup discovers persisted workflow runs in the session's
workflow directory and rebuilds `WorkflowRuntime` state from `run.json` and
`journal.jsonl`.

Recovery performs only local reconstruction:

1. validate run metadata and journal sequence;
2. reconcile incomplete invocations;
3. append `paused(reason=host_exit)` for dangling running runs;
4. register background task handles;
5. rebuild workflow cards and task snapshots as projections; and
6. enqueue at most one recovery notification for the next natural model turn.

Recovery does not execute Lua, resume a child, request permission, or open a
model turn. New live calls use the current instructions, permission mode,
model, and provider.

## Pause, Resume, and Stop

Humans and models share the canonical task-control backend.

- Models use `TaskPause`, `TaskResume`, and `TaskStop`.
- Humans use the existing `/tasks` browser.
- `BackgroundTaskManager` forwards to the registered workflow handle.

Pause is cooperative at a durable invocation boundary:

- the runtime records a pause request;
- an active Delegate/Swarm/Bash/verify invocation reaches a terminal journal
  record;
- no next host call starts;
- pure Lua execution observes pause/stop at the instruction hook and rewinds to
  the last durable boundary on resume.

Stop cancels the active canonical child/process, records its terminal outcome,
and transitions the workflow to terminal `cancelled`.

Model control exists for automation efficiency. It must never pause or stop a
workflow because it guessed tokens, cost, agent count, or task scale.

## Launch Capability

Only an exact slash-parser `/workflow` action creates a launch capability.
Ordinary user text, model inference, Auto/Yolo mode, AGENTS.md parallel guidance,
or a tool argument cannot create or forge it.

The capability is runtime state, never a model-visible token value. It expires
when:

- one workflow is durably launched;
- the user cancels it;
- `/new` resets the session; or
- the process exits.

Invalid `RunWorkflow` input does not consume it. Consumption occurs only after
`run.json` and the initial `state_changed(... -> running)` record are durable.

Unchanged paused lineage may resume without a fresh capability. Edited source,
edited args, a new run, or a terminal retry requires a new exact `/workflow`.

## Launch Approval UX

Ask mode shows a blocking launch review before durable creation. The approval
reuses the canonical Plan/Goal review protocol rather than inventing a second
dialog contract.

It displays:

- name and description;
- ordered phases;
- canonical args;
- complete Lua source with syntax highlighting;
- total lines/bytes and scroll position; and
- the statement that launch approval authorizes orchestration only and child
  tool effects remain independently authorized.

The script must be fully inspectable. Viewport cropping is explicit and
scrollable; no source is silently truncated.

Actions are:

- `Launch`: authorize durable creation and consume on success;
- `Revise`: return feedback to the model and preserve capability; and
- `Cancel`: revoke capability and do not launch.

Auto and Yolo launch without a second prompt once capability exists. The run
metadata/card still records `/workflow` as the source and the active permission
mode. Child effects continue to follow the current permission mode.

## Canonical Effect Dispatch

The host-call sequence is:

```text
strictly decode host input
  -> derive invocation_id and canonical input hash
  -> append invocation_started durably
  -> canonical instruction preflight
  -> canonical permission resolution
  -> canonical scheduler/tool owner
  -> existing child/shell events and cards
  -> append invocation_finished durably
  -> emit workflow orchestration projection
```

`neo.verify_command` passes an exact typed command and cwd into normal Bash
authorization, instruction preflight, session/prefix approval logic,
`ShellRuntime`, and Bash event rendering. Permission denial is a typed script
outcome. It is not detected by parsing an error string.

If instruction preflight discovers a new or changed applicable instruction
scope, the external effect does not run. The instruction epoch is appended
through the canonical instruction owner and the workflow becomes
`paused(reason=instruction_replan_required)`. The next natural model turn is
notified to inspect `TaskOutput` and the new instructions. The model may resume
the unchanged run or use a fresh `/workflow` capability for edited source. Lua
must never bypass or summarize the newly applicable instructions itself.

Launch approval never grants permission for child effects.

## Machine-Safety Defaults

Runtime configuration owns all workflow safety limits. Scripts cannot provide
them.

| Limit | Default |
| --- | ---: |
| Lua source bytes | 1 MiB |
| Lua VM memory | 256 MiB |
| pause/stop hook interval | 10,000 instructions |
| uninterrupted instructions between host calls | 100,000,000 |
| journal record bytes | 16 MiB |
| journal bytes per run | 4 GiB |
| swarm children per call | existing canonical maximum 8 |
| workflow swarm concurrency | runtime-owned 4 |
| workflow wall-clock timeout | none |
| workflow lifetime host-call count | none |
| workflow lifetime agent count | none |
| workflow token cap | none |

Swarm children beyond active concurrency queue in the canonical scheduler. The
single-call child-count validator remains an input boundary; Lua may issue
additional awaited swarms without a workflow lifetime agent cap.

Instruction accounting resets after every completed host call. Hitting the
memory, uninterrupted-instruction, journal-record, or journal-total limit
transitions the run to terminal `resource_limited`. Journal output is never
silently truncated.

An optional finite user-configured workflow token cap counts only actual
provider usage reported by workflow-hosted Delegate/Swarm calls. The active call
is allowed to finish because future usage cannot be predicted. Once accumulated
actual usage reaches the cap, the runtime records the result and prevents the
next provider-backed call by transitioning to `resource_limited`.

Raising a limit and retrying creates a new linked run. Machine limits must not
be described as expected project size, expected cost, or budget advice.

## Event and Data Flow

```text
exact /workflow
  -> runtime capability
  -> model calls RunWorkflow
  -> optional Ask launch review
  -> WorkflowRuntime writes run.json + initial journal
  -> BackgroundTaskManager registers run_id handle
  -> RunWorkflow returns running + run_id
  -> background Lua worker executes/replays
       -> canonical host dispatch
       -> child events/cards remain canonical
       -> journal remains workflow truth
       -> WorkflowUpdated remains a projection
  -> terminal state journaled
  -> one typed completion notification queued
  -> next natural model turn is told to call TaskOutput
```

`TaskOutput` is the only canonical model-facing reader for complete workflow
status/result. It returns:

- run metadata and state;
- current phase and orchestration log/report summary;
- invocation aggregates and typed local failures;
- child/task references;
- actual provider usage;
- final Lua return/report data; and
- resource-limit or cancellation detail.

It does not duplicate complete child transcript content.

## Completion Notification

After a terminal `state_changed` record is durable, the runtime queues one typed
notification with a deterministic ID derived from `run_id` and terminal state.

The notification:

- never interrupts an active turn;
- never masquerades as user input;
- never injects the complete workflow result;
- tells the next natural idle model turn to call `TaskOutput`; and
- is deduplicated against an existing queued or persisted transcript projection.

Session JSONL remains a transcript projection. Using its stable message/event
identity to avoid duplicate projection does not make it workflow state.

## Transcript and UI

The workflow card summarizes orchestration only:

- name, state, current phase, elapsed time, invocation counts, actual usage, and
  latest `neo.log`/`neo.report` summary;
- local failure count and terminal reason; and
- task-control availability.

It does not embed child tool output, child conversations, complete shell
commands, or duplicate Delegate/Swarm cards.

Existing Delegate, DelegateGroup, and DelegateSwarm card structure, ordering,
row budgets, expansion, output previews, and progress semantics remain exactly
unchanged.

`verify_command` emits the normal canonical Bash card. It shows the exact
command and typed cwd, syntax highlighting, width-safe wrapping,
head/omission/tail preview, explicit hidden-character count, global Ctrl+O
expansion, output, and adjacent approval state according to the Bash/Terminal
presentation brief.

Workflow card updates are ordinary transcript upserts. Updating an existing
card does not complete an active thinking/text block.

## Saved Workflow Boundary

V1 persists run source and journal only. It does not create a reusable named
workflow library.

There is no:

- user/project workflow discovery directory;
- `/workflow <name>` lookup;
- saved-workflow versioning or precedence;
- workflow-to-Skill packaging rule; or
- template registry.

This surface may be reconsidered only after measured reuse demand establishes
that per-run history and ordinary Skills are insufficient.

## Compatibility and Retirement

### Delete-first code retirement

Implementation must hard-delete:

- synchronous foreground `RunWorkflow::execute` behavior;
- legacy `{title, script}` schema;
- `WorkflowHostRecorder` and recorder-only `run_script()`;
- in-memory `steps/reports` as a state owner;
- direct workflow `child_tools.run()` dispatch;
- raw Delegate/Swarm parameter forwarding and aliases;
- workflow `mode=background` and `max_concurrency`;
- automatic workflow failure because any child outcome failed;
- copied workflow snapshots in `BackgroundTaskManager`; and
- tests/docs whose only purpose is preserving retired behavior.

No adapter, fallback, dual schema, alternate input field, or recorder recovery
path is retained.

### Historical session boundary

Existing session files are not deleted or rewritten. Historical
`WorkflowStarted/Updated/Finished` events remain readable as historical cards.
They do not synthesize `run.json` or `journal.jsonl`, are not resumable, and do
not activate a legacy runtime path.

This is read-only transcript compatibility, not execution compatibility.

## Error Handling

- Input/schema failure: `invalid_workflow_input`; capability remains available.
- Missing launch capability: deny without creating run state.
- Launch approval revise/cancel: no run state is created.
- Failure writing immutable metadata or initial journal: no launch and no
  capability consumption.
- Failure appending `invocation_started`: no external effect begins.
- Failure appending `invocation_finished`: run pauses/fails safely and recovery
  reconciles the invocation; it must not claim terminal success.
- Permission denial: typed invocation outcome available to Lua.
- New/changed instruction scope: no effect; append the instruction epoch and
  pause with `instruction_replan_required` for model review.
- Child failure: typed local outcome; Lua decides policy.
- Lua/host/verify failure: terminal `failed` after child terminal reconciliation.
- Pause: durable boundary semantics.
- Stop: canonical child/process cancellation, then terminal `cancelled`.
- Machine limit: terminal `resource_limited`, never partial successful output.
- Corrupt metadata/journal: do not execute; expose an inspectable failed task
  snapshot and preserve files for diagnosis.

## Cross-Platform Contract

- All storage and cwd values use `Path`/`PathBuf`.
- No shell string is used for workflow persistence, replay, or control.
- Atomic file creation/append behavior must have Windows, Linux, and macOS
  implementations through existing filesystem helpers or standard APIs.
- Tests must not rely on Unix signals, permissions, path separators, fixed
  ports, ambient cwd, or shared environment variables.
- Bash/Terminal platform behavior remains owned by `ShellRuntime` and its
  existing `cfg` boundaries.

## Acceptance Evidence

### Launch capability

- No-capability `RunWorkflow` is denied.
- Exact `/workflow` grants one successful launch.
- Invalid input does not consume the capability.
- Ask `Revise` preserves and `Cancel` revokes it.
- Auto/Yolo cannot bypass the capability.

### Background lifecycle and controls

- `RunWorkflow` returns `running + run_id` before Lua completion.
- Every terminal workflow has terminal records for all started children.
- Human and model pause/resume/stop use the same backend.
- Pause waits for the active invocation; Stop cancels it.

### Permission and inspection

- `verify_command` approval contains exact command/cwd and passes instruction
  preflight.
- Denial returns a typed outcome.
- Delegate/Swarm effects use canonical dispatch.
- The normal Bash card proves complete inspectable command presentation.

### Journal and recovery

- A complete matching prefix replays without repeated effects.
- An incomplete invocation becomes `interrupted(host_exit)` when unrecoverable.
- Host exit rebuilds a paused run without a model turn.
- Unchanged resume retains `run_id`; changed retry creates a linked run.
- Terminal completion is projected once across recovery.

### Failure semantics

- Lua can handle a child failure and still complete.
- Unhandled Lua/host error, `neo.fail`, and failed verification produce
  `failed`.
- `cancelled` and `resource_limited` remain distinct states.

### Resource safety

- Instruction, memory, record-size, and journal-size limits independently
  produce `resource_limited`.
- Default wall-clock and workflow token caps are absent.
- A configured token cap uses only actual reported usage and blocks the next
  provider call after the threshold is reached.
- Swarm runtime concurrency is four and queued order is observable.

### Projection and retirement

- Workflow cards contain orchestration summaries without child duplication.
- Delegate-family render snapshots remain unchanged.
- `TaskOutput` returns structured run data and child references.
- Retired recorder, synchronous, bypass, alias, and copied-state paths are
  absent.
- An old session fixture still renders a historical workflow card but cannot
  resume it.

Provider behavior uses `FakeModelClient`/`FakeHarness`; crash/replay tests use
isolated temporary session directories. Verification commands must name one
package, one target selector, and at least one exact test filter. No broad
workspace test run is required as evidence for this design.

## Architecture Integrity Lens

- Invariant: terminal workflow implies terminal records for every started
  child, and external effects begin only after durable invocation intent.
- Canonical owner: `WorkflowRuntime` for run truth; existing Neo owners for
  child effects, permission, scheduling, tasks, notification, and projection.
- Responsibility overlap: removed by deleting recorder and copied task state.
- Higher-level simplification: one `run_id`, one journal, one task-control
  surface, and one canonical dispatch path.
- Retirement: foreground execution, weak schema, raw forwarding, and direct
  child dispatch die in the same migration.
- Falsifier: any implementation that can complete with a running child, repeat
  an incomplete effect automatically, or answer TaskOutput from copied state
  violates this design.
- Verdict: aligned.

## Anti-Entropy Declaration

- Deletion class: internal code retirement plus contract-carrying code.
- Old paths: foreground execution, recorder owner, direct child dispatch, weak
  schemas, copied task state, aliases, and detached workflow child semantics.
- New canonical owner: `WorkflowRuntime` plus existing Neo runtime owners.
- Preserved behavior: Lua orchestration, canonical child tools, historical
  transcript display, and existing child cards.
- Retired behavior: synchronous result delivery, implicit background children,
  permission bypass, non-durable state, and compatibility aliases.
- External boundary touched: model-facing tool contract and local session
  replay.
- Source-of-truth data risk: none; existing session files are retained.
- Retirement decision: `delete-first`; historical session events remain
  read-only projections only.

## Complexity and Minimality

Expected affected owners include workflow runtime/state/Lua modules,
`RunWorkflow`, runtime dispatch/capability handling, background task adapters,
session workflow storage/recovery, task controls, and workflow transcript
projection.

The implementation should add owner files only where they isolate one durable
responsibility, such as journal persistence or runtime coordination. It must not
create interfaces with one speculative implementation, a workflow-specific
permission engine, another task registry, or a saved-workflow package.

The smallest correct architecture is not a patch to the current recorder. The
journal and background lifecycle are the minimum required to solve the six
confirmed defects without duplicate owners.

## ADR Signal

ADR-worthy decisions are present:

- `WorkflowRuntime` as the durable source of truth;
- append-only replay instead of VM serialization;
- canonical runtime dispatch for workflow effects;
- exact `/workflow` launch capability; and
- hard retirement of foreground/recorder semantics.

After implementation is verified, completion review should decide whether to
record one ADR for the durable workflow owner/replay contract and synchronize
any architecture baseline that describes workflow execution.

## Planning Gate

No implementation begins from this document until the user reviews the written
spec. After approval, the next workflow is Aegis `writing-plans`; implementation
skills remain blocked until that plan is written and reviewed.

## Aegis Working Drafts

### TaskIntentDraft

- Outcome: replace the one-shot recorder with a durable, background,
  inspectable Neo workflow runtime.
- Success evidence: the acceptance matrix proves launch security, canonical
  effects, terminal-child lifecycle, recovery without duplicate effects,
  explicit controls, resource behavior, projections, and hard retirement.
- Stop condition: the written spec is approved and an implementation plan can
  map every contract without making product decisions.
- Non-goals: saved workflow library, another language, predictive cost
  governance, detached workflow children, or new child/UI owners.
- Principal risks: duplicate state owners, effect replay, permission bypass,
  uninspectable commands, silent truncation, and accidental cost prediction.

### BaselineReadSetHint

- Required: canonical approval protocol, shell admission scheduler,
  Bash/Terminal tool-card brief, transcript boundary semantics, current
  workflow runtime/tool/state/Lua source, background task manager, runtime
  permission/dispatch, and workflow card.
- Advisory references: Claude Code workflow documentation and captured prompt
  for product comparison only; they are not Neo runtime authority.

### BaselineUsageDraft

- Required baseline refs: the four Neo Aegis specs listed under Baseline
  Alignment.
- Delivered context refs: current workflow, background task, permission,
  multi-agent, and transcript owners inspected during design.
- Acknowledged before plan refs: this design and the four baseline specs.
- Cited in design refs: all required refs.
- Missing refs: none blocking design approval.
- Decision: continue to written user review.

### Requirement Ready Check

- Requirement source refs: approved decision sequence captured in this design.
- Goals and scope refs: Goals, Non-Goals, Product Direction.
- User/scenario refs: long-running, very high-token, multi-agent project work
  with human/model task controls.
- Requirement item refs: model contract, host API, persistence, recovery,
  permission, limits, notification, UI, retirement.
- Acceptance refs: Acceptance Evidence.
- Open blocker questions: none.
- Decision: ready.

### ImpactStatementDraft

- Affected layers: tool schema/prompt, runtime dispatch, workflow execution and
  persistence, background tasks, multi-agent correlation, permission/approval,
  notifications, replay, session projection, and TUI workflow presentation.
- Canonical owners: explicitly assigned in Canonical Ownership.
- Compatibility: internal hard replacement; historical workflow transcript
  events remain read-only only.
- Non-goals: no saved-library, language, daemon, or predictive-budget surface.
- Architecture review required: yes.

### Existence Check

- Proposed new surface: durable `WorkflowRuntime` and per-run journal.
- Existing reuse candidates: in-memory recorder, session JSONL, and
  `BackgroundTaskManager` snapshots.
- Why insufficient: none can durably own replayable invocation intent/results
  without becoming a duplicate or violating transcript/task boundaries.
- Creation proof: the six confirmed defects require durable intent-before-effect
  and recovery state.
- Entropy/retirement impact: recorder and copied workflow task state are
  removed in the same migration.
- Decision: add-with-proof.

### Product Risk Lens

- Value: reliable autonomous orchestration without hiding control or effects.
- Non-goals: feature parity and speculative reuse surfaces.
- Trade-offs: boundary pause is not instantaneous; incomplete external effects
  are not auto-retried; high machine limits may require a linked retry after a
  real resource breach.
- Decision: accepted.

### Complexity Budget

- Artifact class: Source Complexity and Test Complexity.
- Target pressure: `background_tasks.rs`, `permission.rs`, and
  `tool_dispatch.rs` already exceed 1,200 lines; interactive input is near
  1,000 lines. Current workflow Lua is 653 lines and mixes VM setup, host APIs,
  direct dispatch, aggregation, and projection.
- Projected pressure: over-budget if durable journal/runtime logic is added to
  those existing generic owners.
- Budget result: at-risk.
- Planned governance: add cohesive workflow-owned runtime/journal/control
  modules; keep oversized generic owners to narrow registration, dispatch, or
  adapter wiring; split focused test targets by contract rather than adding one
  monolithic integration file.

### Plan-Time Complexity Check

- Better file boundary: workflow persistence/replay and orchestration state live
  under the workflow module; task/permission/dispatch files receive wiring only.
- Recommendation: extract/replace workflow responsibilities, use wiring-only
  edits in oversized generic owners, and split the implementation plan by
  durable owner boundaries.
