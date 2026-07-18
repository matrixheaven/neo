# Shell Admission Scheduler

## Status

Approved design for replacing fail-fast Bash and Terminal admission with a
process-local scheduler. This design also removes default shell execution
deadlines, makes command resource budgets independent of scheduler capacity,
adds a shell-free `Sleep` tool, and defines queue state across the main
transcript, Delegate, and `DelegateSwarm`.

This document supersedes only the following parts of
`2026-07-15-supervised-shell-execution-design.md`:

- immediate `ResourceLimited(active_commands)` failure when capacity is full;
- the default `max_active_commands = 2`;
- division of descendant and memory budgets by active-command capacity;
- the 10-minute foreground and 30-minute background/Terminal deadlines;
- `SetBackgroundDeadline`; and
- the old shell resource configuration names.

The guardian ownership model, process-tree cleanup, bounded output, persisted
guardian status, fail-closed sampling, cross-platform containment, and
local-only architecture remain unchanged. The later Shell OS Sandbox design
continues to compose at the guardian launch boundary; this scheduler neither
weakens nor replaces sandbox policy resolution.

## Problem

Neo currently acquires a shell permit with `ShellRuntime::try_acquire` while
starting the guardian. When the process-wide count is already at its limit, a
Bash or Terminal tool call fails immediately with `Resource limit exceeded`.
This is particularly disruptive in multi-agent work: long-running builds or
tests can occupy both default slots, so a harmless command such as reading the
tail of a log fails even though it would be safe to run later.

Returning a queued task handle to the model would avoid the immediate error but
would break the tool call's causal chain. The model would receive another turn
without the requested command result, would need to invent a polling or shell
`sleep` loop, and could spend additional model requests while the same shell
capacity remains unavailable. Queue timeouts have the same problem for commands
that legitimately run for tens of minutes.

The correct product contract is therefore transparent backpressure: the
original tool call stays pending until it can start, is cancelled, or its owner
is cancelled. Capacity saturation is not a command failure and does not create
a second task lifecycle.

## Goals

- Wait transparently when Bash or Terminal Start reaches process-local shell
  capacity.
- Keep the original Tool Use pending; do not release a model turn before the
  requested command actually produces its normal result.
- Let one shared scheduler coordinate the main agent, every subagent, user shell
  mode, foreground Bash, background Bash, and active Terminal sessions.
- Give a queued user shell command the next available slot without preempting a
  running command.
- Prevent background Bash and Terminal sessions from consuming more than three
  slots, leaving foreground capacity under the default four-slot configuration.
- Preserve per-agent fairness while retaining foreground preference.
- Show a truthful queued state in the original transcript card, including live
  position and elapsed wait, without exposing an ETA.
- Preserve the same queue semantics inside Delegate and `DelegateSwarm` cards.
- Make omission of `timeout_secs` mean no execution timeout.
- Keep real descendant-process, memory, and sampler failures hard and
  actionable.
- Provide a cancellable, shell-free `Sleep` tool for genuine time-based waits.
- Keep the implementation in-process, dependency-light, and portable across
  Windows, Linux, and macOS.

## Non-Goals

- Persisting or replaying queued work after Neo exits.
- Returning a queue handle or adding a queue-specific cancellation tool.
- Automatically converting a foreground command into a background task.
- Queue TTLs, admission timeouts, ETAs, or queue-length limits.
- Preempting, suspending, or reprioritizing a running process.
- Reserving memory or descendant-process budget across multiple commands.
- Replacing `WaitDelegate` or blocking `TaskOutput` with time-based polling.
- Adding a daemon, distributed queue, Redis dependency, or cross-process
  scheduler.
- Changing guardian process ownership, output retention, or OS containment.

## Product Contract

### Transparent waiting

When no permit is available, foreground Bash, background Bash, and Terminal
Start remain inside their original Tool Use. Neo does not return a failure, a
background task ID, or a queue handle. No new model turn begins. Once admitted,
the call follows its existing foreground, background, or Terminal result
contract.

A request admitted in the same scheduler mutation that receives it proceeds
directly to Started. It does not emit or persist Queued/Position transitions;
those transitions exist only when capacity actually suspends the request. This
prevents ordinary below-capacity shell calls from briefly appearing queued in
the live transcript or historical JSONL.

Queueing has no timeout and no item-count limit. A request may wait longer than
30 minutes when the running workload requires it. The queue is bounded
indirectly by the number of in-flight tool calls in this Neo process; each
scheduler item contains only identity, scheduling metadata, a one-shot grant
sender, and a callback. Command arguments remain owned by the suspended tool
future rather than copied into the scheduler.

### Queue classes and priority

The scheduler has three classes, selected at the point where the tool's command
kind is already known:

1. `User`: commands entered through Neo's local `!` shell mode.
2. `AgentForeground`: foreground Bash from the main agent or any subagent.
3. `AgentBackground`: Bash with `run_in_background = true` and Terminal Start.

When a permit becomes available, the scheduler selects the first non-empty,
eligible class in that order. A newly queued user request cannot interrupt a
running command, but it is selected before any queued agent request at the next
release. Agent foreground work may starve background work while foreground
requests remain continuously queued; this is an intentional responsiveness
trade-off, not a weighted-fairness guarantee.

The total running count may not exceed `max_active_commands`. The running
`AgentBackground` count may not exceed both three and
`max_active_commands`. The limit of three is fixed rather than another config
key. With the default total capacity of four, at least one slot cannot be held
by an agent-started background Bash or Terminal. A user who deliberately
configures a total capacity below four also reduces or removes that reserved
foreground headroom.

A local `!` command admitted as `User` keeps that class if the user later moves
the already-running command to the background. Detach neither starts a new
workload nor re-enters admission, so it does not retroactively increment the
`AgentBackground` counter. This explicit user action is the only way a detached
shell can sit outside the three-slot agent-background cap.

### Per-owner fairness

`AgentForeground` and `AgentBackground` each maintain a FIFO queue per owner and
a round-robin ring of owners. An owner is `ToolContext.agent_id`, with the main
runtime's canonical main-agent ID used when the field is absent. A newly
non-empty owner queue is appended to its class ring. Granting pops one request
from the ring's front; if that owner still has requests, the owner is appended
to the ring's back.

FIFO is therefore guaranteed within one owner and one class. Round-robin is
guaranteed between owners within the same class. Separate foreground and
background queues mean an ineligible background request cannot head-of-line
block a foreground request from the same agent.

The user class is one global FIFO because user commands do not need owner
fairness.

### Position semantics

The displayed `#N` is the request's current one-based rank inside its scheduling
class after applying that class's owner round-robin order. It is recomputed when
a request enters, leaves, or is granted. Higher-priority arrivals and the
background eligibility cap can delay a request even when it displays `#1`, so
Neo never presents the rank as an ETA.

## Scheduler Architecture

`ShellRuntime` owns one `Arc<ShellScheduler>` shared by every clone of the
runtime:

```rust
pub enum ShellAdmissionClass {
    User,
    AgentForeground,
    AgentBackground,
}

pub struct ShellAdmissionRequest {
    pub owner: String,
    pub class: ShellAdmissionClass,
}

pub enum ShellAdmissionEvent {
    Queued,
    Position {
        position: usize,
        waiting: std::time::Duration,
    },
    Started,
}

pub type ShellAdmissionCallback =
    std::sync::Arc<dyn Fn(ShellAdmissionEvent) + Send + Sync>;

impl ShellRuntime {
    pub(crate) async fn acquire(
        &self,
        request: ShellAdmissionRequest,
        callback: Option<ShellAdmissionCallback>,
    ) -> ShellCommandPermit;
}
```

The four admission metadata/callback types are public only because local user
shell mode lives in the `neo-agent` crate while scheduling lives in
`neo-agent-core`. `ShellScheduler`, waiter state, and `ShellCommandPermit`
remain crate-private implementation details.

`ShellScheduler` uses a short-held `std::sync::Mutex<SchedulerState>`, standard
library maps/deques, and Tokio one-shot channels. It is not a scheduler actor
and it is not a bare semaphore: a semaphore cannot express user priority,
foreground/background classes, the background cap, or per-owner round-robin
ordering.

`ShellScheduler` owns the mutex and capacity directly. `ShellRuntime` and every
running permit share it through `Arc<ShellScheduler>`; there is no second
`ShellSchedulerInner` indirection. A live config refresh preserves the current
`ShellRuntime`, so queued/running commands and newly created agent contexts keep
one scheduler. Shell-limit changes continue to take effect on the next Neo
process start rather than splitting admission across old and new schedulers.

`SchedulerState` contains:

- one user FIFO;
- per-owner foreground FIFOs and a foreground owner ring;
- per-owner background FIFOs and a background owner ring;
- the total running count;
- the running background count; and
- monotonically increasing waiter IDs used only inside the process.

The scheduler never invokes callbacks while holding its mutex. Every mutation
computes a list of grants and position notifications, releases the lock, then
sends grants and invokes callbacks. No callback may call back into locked
scheduler state.

An immediately granted request receives no Queued or Position callback. A
request that remains enqueued receives Queued before its initial Position.
Later mutations may update Position, while Started is invoked by the launch
caller only after post-grant revalidation succeeds.

`ShellCommandPermit` is the only running-capacity token. It retains the shared
scheduler and its class. Its `Drop` implementation decrements the appropriate
running counters exactly once and dispatches the next eligible request. The
permit moves into `GuardianClient` and remains owned by the guardian reader task
until command exit. Terminal keeps the same permit for the complete session,
not for each Write/Read/Resize/Stop operation.

## Admission Flow

The canonical model-tool sequence is:

```text
parse and validate tool input
  -> resolve permission / user approval
  -> resolve initial cwd and hard launch inputs
  -> acquire or queue shell permit
  -> revalidate hard launch boundaries
  -> emit ToolExecutionStarted
  -> emit shell-specific started event
  -> spawn and handshake with guardian
  -> run or register background/Terminal handle
  -> emit ToolExecutionFinished
```

Permission resolution always completes before enqueue. A long queue wait never
causes a second approval prompt. Immediately after grant and before guardian
spawn, Neo re-runs the non-interactive hard checks that can drift while waiting:
shell access, cwd containment/canonicalization, output limit clamping, and
validated timeout conversion. A failed revalidation drops the permit and
finishes the original tool call with that validation error. It does not spawn a
guardian and does not grant a task handle.

`ToolExecutionStarted` currently fires before approval in both sequential and
parallel dispatch. That ordering is replaced. Non-shell tools emit Started
after permission and immediately before execution. Admission-controlled Bash
and Terminal Start receive a callback in `ToolContext`; they emit Started only
after `ShellRuntime::acquire` returns and launch revalidation succeeds. Terminal
Write/Read/Resize/Stop are not admission-controlled and start normally after
permission.

User shell mode follows the same admission order but emits user-shell queue and
start events instead of model-tool events. Pressing the existing background
key while a user command is still queued reports that the command has not
started yet; it does not change class or create a task ID. Once started,
backgrounding preserves the same permit and timeout.

## Cancellation and Race Safety

A queued request is owned by the future awaiting its one-shot grant. Dropping
that future removes the waiter. Turn cancellation, Agent cancellation, Swarm
cancellation, and Neo shutdown already cancel or drop the owning execution
future, so they all use this same path. There is no separate queued-foreground
handle or cancellation API.

Grant and cancellation may race. The implementation must satisfy exactly one of
these outcomes:

- the waiter is removed before grant and never increments running counters; or
- the scheduler grants one `ShellCommandPermit`; if the receiver has already
  gone, sending fails and the returned permit is immediately dropped, releasing
  the counters and dispatching another waiter.

After a successful receive, the launch caller checks its cancellation token
again before Started or spawn. This closes the race where grant and owner
cancellation become ready together and the grant branch wins the local select.

Waiter removal and grant selection occur under the scheduler mutex. A waiter ID
can be removed or granted only once. Permit Drop is idempotent by ownership: the
permit is not clonable and its counters are decremented in its sole destructor.
Cancellation never leaks capacity and never starts a guardian after the owning
tool future has gone away.

## Timeout Contract

Bash replaces its old `timeout` field with:

```rust
timeout_secs: Option<u64>
```

Terminal adds the same optional field, valid only for `mode = "start"`.
Providing it for Write, Read, Resize, or Stop is an invalid-input error. There
is no alias for Bash `timeout`.

Omission means no execution timeout for foreground Bash, background Bash,
Terminal, and local user shell mode. An explicit positive value starts one
monotonic deadline when the command process starts; queue wait is not counted.
The same deadline remains in force if a running foreground user shell command
is moved to the background. Neo does not reset or replace it.

Both tool schemas use this language-neutral guidance verbatim:

> Optional execution timeout in seconds. Omit this field to allow the command
> to run until it finishes or is cancelled. For potentially long-running work,
> prefer omission; if a limit is necessary, do not set it below 7200 seconds.
> Use shorter values only for commands that are explicitly expected to finish
> quickly.

The 7,200-second value is guidance, not a validation minimum: explicit values
from 1 second upward are accepted. Zero is rejected.

At the guardian boundary, `GuardLimits.timeout_ms` becomes `Option<u64>` and
`background_timeout_ms` is deleted. Both Bash and Terminal supervision select
between an optional deadline future and their other control/resource events;
no far-future sentinel sleep is used. `GuardRequest::SetBackgroundDeadline`,
its codec tags, handlers, client method, and callers are deleted.

## Resource Configuration

The canonical user-facing configuration is:

```toml
[runtime.shell]
max_active_commands = 4
max_command_parallelism = 4
max_command_descendant_processes = 32
max_command_memory_percent = 25
max_output_bytes = 65536
max_background_log_bytes = 10485760
```

`max_active_commands` controls scheduler capacity only. The three
`max_command_*` values are direct per-command guardian budgets and are never
divided by scheduler capacity:

- `max_command_parallelism` supplies advisory `CARGO_BUILD_JOBS`,
  `NEXTEST_TEST_THREADS`, and `RAYON_NUM_THREADS` values when the environment
  variable is absent;
- `max_command_descendant_processes` is the maximum observed descendants for
  each command tree; and
- `max_command_memory_percent` is the maximum resident-memory percentage for
  each command tree.

All integer limits must be positive. `max_command_memory_percent` must be in
`1..=100`. `max_output_bytes` must still fit the protocol's 32-bit output
length. There is no validation relationship between scheduler capacity and any
per-command resource budget.

The following old keys are removed and rejected as unknown configuration; Neo
does not deserialize aliases or maintain compatibility branches:

- `foreground_timeout_secs`
- `background_timeout_secs`
- `max_parallelism`
- `max_descendant_processes`
- `max_tree_memory_percent`

The loader must report the exact unknown key and list the canonical expected
fields. It does not contain migration mappings for removed names. Existing
output/log keys retain their meaning and defaults.

## Resource Failure Semantics

Capacity saturation no longer constructs `ToolError::ResourceLimited` and
`ResourceLimitCause::ActiveCommands` is deleted. Real guardian observations
remain hard terminal failures:

- descendant count includes configured and observed counts;
- tree memory includes configured and observed percentages; and
- sampler unavailability remains fail-closed because Neo cannot prove that the
  other limits are being enforced.

User-visible output identifies the observed value and configured limit where
both exist and names the relevant canonical config key. Sampler-unavailable
output explains that the command was stopped because resource monitoring could
not be enforced and gives an actionable retry/troubleshooting direction. It
never recommends calling `Sleep`: sleeping does not repair a process or memory
violation.

## Event and Persistence Contract

Queue state uses explicit events rather than overloading Started:

```rust
AgentEvent::ToolExecutionQueued {
    turn: u32,
    id: String,
    name: String,
    arguments: serde_json::Value,
}

AgentEvent::ToolExecutionQueueUpdated {
    turn: u32,
    id: String,
    position: usize,
    waiting_ms: u64,
}

AgentEvent::ShellCommandQueued {
    turn: u32,
    id: String,
    command: String,
    cwd: PathBuf,
    origin: ShellCommandOrigin,
}

AgentEvent::ShellCommandQueueUpdated {
    turn: u32,
    id: String,
    position: usize,
    waiting_ms: u64,
}
```

Queued, Started, and Finished transitions are persisted to session JSONL.
`ToolExecutionQueueUpdated` and `ShellCommandQueueUpdated` are live-only and
must be filtered by `SessionEventPersistence`; they may be emitted immediately
on enqueue and whenever rank changes. The TUI advances elapsed wait from the
last live baseline without emitting one event per clock tick.

The same filtering applies to derived parent snapshots. Before persisting any
`DelegateStarted`, `DelegateFinished`, `DelegateProgressUpdated`,
`DelegateSwarmStarted`, `DelegateSwarmFinished`, or
`DelegateSwarmProgressUpdated` payload, Neo clears queued position and elapsed
baseline fields from nested child activity/progress. Thus a cancellation that
finishes a child while its shell call is still queued cannot leak live queue
metadata through a parent lifecycle event. Progress coalescing compares the
normalized snapshot as well, so position/wait-only changes neither write a
compact progress event nor reset its persistence gate.

Session replay reconstructs historical transitions only. It never submits a
queued event to `ShellScheduler`. If a prior process persisted Queued without a
later Started or Finished transition, replay finalizes that card as interrupted
history rather than showing it as live or restarting the command.

Queue events are UI/session lifecycle data. They are never placed in
`ToolResult.content`, `ToolResult.details`, assistant messages, or the next
model request. The eventual Tool Result remains byte-for-byte governed by the
normal Bash/Terminal success, background, cancellation, timeout, and guardian
failure contracts.

## TUI Contract

The main tool status model gains an explicit `Queued` state. Generic `Pending`
no longer means queued; it represents a tool call still being assembled or
prepared. Only a real scheduler enqueue event changes a card to Queued.

The compact queued header is:

```text
Queued Bash (<command>) · #2 · waiting 18s
```

Terminal Start uses `Terminal` and its command in the same shape. There is no
ETA. Position and elapsed text update in place. The queue transition, later
Started transition, live output, and final result all mutate the original tool
card by tool-call ID; they do not append parallel queue cards.

A queued card is a living transcript entry with a stable canonical position.
Later thinking, user input, tool calls, assistant prose, and turns may appear
after it, but queue updates, start, output, and completion never move that card
to the transcript bottom or reorder it by completion time. Delegate and
`DelegateSwarm` retain the same stable-position contract while their child
activity changes.

For user shell mode, the existing shell run changes from Queued to Running in
place and keeps the `$ <command>` identity. While queued, it shows the same
position/wait metadata and does not show the running-only background hint.

Delegate child activity gains a canonical `Queued` tool phase plus queue
metadata. Both Delegate and `DelegateSwarm` render the same compact queued line.
When Started arrives, the same activity entry changes to Ongoing and retains
its command summary. When updates or Finished arrive, Bash/Terminal output
preview remains below the tool row exactly as it does for existing child tool
output. Queue transitions must not replace the argument summary with queue
metadata or discard the final output preview.

The normal child-tool disclosure rule remains: Bash, Terminal, and MCP tools may
show bounded output previews; other tools show their name and summarized
arguments unless they already have an established preview contract. This
feature does not expose every child Tool Result.

For `DelegateSwarm`, the collapsed child summary remains one scan-friendly
tool-and-argument line; it does not inline result bodies. Expanding the child
uses the shared child transcript renderer and may show the bounded Bash,
Terminal, or MCP preview beneath that line. A single Delegate uses the same
expanded row/preview shape. Queueing does not change this disclosure boundary.

## `Sleep` Tool

Neo adds one built-in tool named `Sleep`:

```rust
struct SleepInput {
    duration_seconds: u64,
    reason: String,
}
```

- `duration_seconds` is required and must be in `1..=3600`.
- `reason` is required, trimmed, non-empty, single-line, and at most 160 Unicode
  scalar values.
- The implementation uses `tokio::time::sleep` and the tool's cancellation
  token.
- It does not acquire a shell permit, spawn a process, create a background task
  handle, write a guardian status file, or require user approval.
- It is available to the main agent and every subagent role in `ask`, `auto`,
  and `yolo` modes, including the Planner role that cannot use Bash.

Its schema description tells the model to use `Sleep` only for a genuine
time-based wait. When Neo already has a condition-aware wait, the model should
prefer `WaitDelegate` for an Agent/Swarm or `TaskOutput(block = true)` for a
background task. A shell `sleep` command is never the recommended way to wait
because it consumes scheduler capacity and creates a guarded process.

Transparent shell queueing itself does not instruct the model to call Sleep:
the model receives no turn while its Tool Use is queued. This avoids the
causal-chain and extra-request bug that motivated the scheduler.

## Cross-Platform Requirements

The scheduler is platform-neutral Rust and uses no signals, shell commands,
filesystem locks, or path encoding. It wraps the existing cross-platform
guardian client. Optional deadline handling must compile and behave on Windows,
Linux, and macOS. Platform-specific guardian containment remains behind its
current `cfg(unix)` and `cfg(windows)` boundaries.

No new crate is required for scheduling or Sleep. Standard collections,
`std::sync::Mutex`, Tokio one-shot channels, Tokio time, and the existing
`CancellationToken` are sufficient.

## Migration

- Add `shell_guard/scheduler.rs`; replace the atomic active count and
  `try_acquire` with the shared scheduler and async acquisition.
- Move permit acquisition out of guardian handshake so the caller can report
  queue/start transitions and revalidate before spawn.
- Delete the old timeout keys, old resource keys, static division helpers,
  `ResourceLimitCause::ActiveCommands`, and `SetBackgroundDeadline` end to end.
- Rename Bash `timeout` to `timeout_secs` without an alias; add
  `Terminal.timeout_secs` for Start.
- Update config documentation and English/Chinese tool references together.
- Add explicit queue event handling to session persistence, the main
  transcript, child activity, Delegate, and `DelegateSwarm`.
- Register `Sleep`, add it to every role profile, and make it default-approved.
- Do not migrate queued work or synthesize background task IDs for it.

## Testing Strategy

### Scheduler unit tests

- immediate admission below capacity, with no Queued/Position callback;
- transparent waiting at capacity and admission after permit Drop;
- user next-slot priority without preemption;
- foreground-before-background ordering;
- per-owner FIFO and round-robin ordering in each agent class;
- foreground bypass of an ineligible background request from the same owner;
- total capacity four and background cap three;
- cancellation before grant removes a waiter;
- grant/cancel race does not leak or double-release capacity; and
- queue position updates reflect class-local round-robin rank; and
- `ShellRuntime` clones and live config refresh share the same scheduler.

### Shell and guardian tests

- foreground/background Bash and Terminal Start share one scheduler;
- a queued guardian is not spawned before grant;
- active Terminal sessions retain their background permit until exit;
- explicit timeout starts after admission and terminates the owned tree;
- omitted timeout allows a command beyond the old deadline;
- backgrounding preserves the original optional deadline;
- optional deadline codecs round-trip and `SetBackgroundDeadline` is absent;
- direct per-command process/memory budgets are not divided by capacity;
- sampler failure remains fail-closed and actionable; and
- queued cancellation through turn, Agent, and Swarm cancellation starts no
  command.

### Runtime, persistence, and TUI tests

- approval precedes Queued and Started;
- Started is not emitted while a request waits;
- queue updates are not persisted, while Queued/Started/Finished are;
- dangling queued replay is interrupted rather than restarted;
- queue metadata is absent from model-visible Tool Results;
- main Bash, Terminal, and user shell cards update in place;
- Delegate and `DelegateSwarm` show command, queued metadata, live output, and
  final preview on one child activity row; and
- generic Pending is not rendered as a real scheduler queue.

### Sleep tests

- schema requires both fields and describes condition-aware alternatives;
- 0 and 3601 seconds, blank/multiline reasons, and reasons over 160 characters
  are rejected;
- cancellation finishes promptly;
- Sleep appears for every role and uses no shell permit; and
- Sleep is default-approved in every permission mode.

## Acceptance Criteria

1. A fifth shell request under the default configuration waits rather than
   returning `Resource limit exceeded`.
2. At most four commands run, at most three of them were admitted as agent
   background Bash or Terminal sessions, and the next released slot goes to a
   queued user command.
3. Agent foreground/background requests preserve per-owner FIFO and class-local
   owner round-robin behavior.
4. A queued Tool Use does not return to the model, create a task handle, expire,
   or survive process restart.
5. Cancellation at every ownership level removes queued work without spawning
   it or leaking a permit.
6. Queue, start, and finish are truthful transitions; live position/wait
   updates stay out of JSONL and all queue metadata stays out of model results.
7. Main, Delegate, and `DelegateSwarm` cards update in place, retain their
   canonical transcript position, and retain command summaries plus bounded
   Bash/Terminal output previews.
8. Omitted `timeout_secs` means no timeout; explicit positive deadlines begin
   only after admission; old timeout fields and `SetBackgroundDeadline` are
   gone.
9. Scheduler capacity no longer changes a command's descendant, memory, or
   parallelism budgets; old config names are rejected.
10. Real process, memory, and sampler failures remain hard, measured,
    actionable guardian outcomes.
11. `Sleep` is cancellable, shell-free, default-approved, available to every
    role, and directs condition-aware waits to the existing tools.
12. The complete behavior works on Windows, Linux, and macOS without a new
    dependency or compatibility path.
