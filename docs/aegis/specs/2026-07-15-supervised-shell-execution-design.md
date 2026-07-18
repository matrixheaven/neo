# Supervised Shell Execution

## Incident

On 2026-07-15 at 13:24:48, Neo started this command with
`run_in_background = true`:

```text
cargo nextest run --workspace --all-features ...
```

At 14:47, the macOS WindowServer watchdog reported its main thread as
unresponsive for 40 seconds. The process snapshot still contained one
`cargo-nextest` process that had run for roughly 4,900 seconds and ten concurrent
spawned children blocked in `syspolicyd` / `AppleSystemPolicy.mig`. Neo itself was no
longer running, so the test workload had become an orphan.

Output accumulation was not the primary failure. Bash already capped each stream at
64 KiB, read in 8 KiB chunks, and the TUI retained only a small live window. The
failure was process ownership and workload admission: background meant detached,
the deadline depended on later task polling, and there was no process-count or
tree-memory guard.

## Goals

- Every Bash and Terminal process tree is owned for its entire lifetime by a local
  guardian process.
- Background means that a command does not block the conversation. It never means
  that a command may outlive Neo.
- Normal exit, cancellation, panic, crash, or forced termination of Neo closes the
  ownership pipe and causes every owned command tree to terminate.
- Expensive commands cannot exhaust the machine through unbounded command count,
  descendant count, memory, runtime, or retained output.
- The TUI event loop never performs process enumeration, blocking PTY I/O, output
  draining, or termination waits.
- The design works on Windows, Linux, and macOS without command-specific handling.
- Existing direct-spawn and unlimited-background paths are removed rather than kept
  as compatibility paths.

## Non-Goals

- Preserving a shell or terminal after the Neo process that created it has exited.
- Reattaching to a command from a resumed session.
- Treating intentionally adversarial daemonization as a security sandbox. Neo still
  tracks and kills process groups and discovered descendants, but the shell tool is
  local code execution under the user's account.
- Adding CPU quotas, cgroups, containers, a resident daemon, or a remote exec server.
- Special-casing Cargo, nextest, rustc, or macOS security services.
- Guaranteeing cleanup if Neo and its guardian are killed simultaneously or the
  operating system itself stops running.

## Approaches Considered

1. Keep direct child spawning and improve `kill_on_drop`. This cannot cover Neo being
   killed with `SIGKILL` on macOS, because destructors do not run.
2. Route every Bash and Terminal launch through a short-lived guardian process. This
   is selected because it provides a parent-death signal on all target platforms
   without adding a daemon.
3. Add a persistent exec service that owns all commands and supports reattachment.
   This solves a broader problem but introduces service lifecycle, authentication,
   protocol compatibility, and recovery machinery that Neo does not need.

## Configuration

The single user-facing configuration surface is:

```toml
[runtime.shell]
foreground_timeout_secs = 600
background_timeout_secs = 1800
max_active_commands = 2
max_parallelism = 4
max_descendant_processes = 64
max_tree_memory_percent = 50
max_output_bytes = 65536
max_background_log_bytes = 10485760
```

All values must be positive. `max_tree_memory_percent` must be in `1..=100`, and both
`max_descendant_processes` and `max_tree_memory_percent` must be at least
`max_active_commands` so static per-command allowances cannot round to zero. Invalid
configuration prevents Neo from starting and identifies the exact key.

Tool inputs may reduce a deadline or returned-output limit for one call, but may not
raise any configured limit. The configured values are therefore hard ceilings from
the tool's perspective. The three concurrency environment variables are advisory
defaults: Neo sets `CARGO_BUILD_JOBS`, `NEXTEST_TEST_THREADS`, and
`RAYON_NUM_THREADS` to `max_parallelism` only when the user has not already set the
variable. Explicit user values are preserved; command-count, descendant-count,
memory, runtime, and output ceilings still apply.

`max_active_commands` covers the combined active Bash and Terminal count for one Neo
runtime, including subagents and background tasks. A call arriving at the limit does
not spawn or queue a guardian; it immediately returns `ResourceLimited` with cause
`active_commands`. This applies equally to foreground Bash, background Bash, and
Terminal Start, so there is no second queued-task lifecycle to persist or recover.

`max_descendant_processes` and `max_tree_memory_percent` cover the aggregate forest
owned by one Neo runtime, not a separate allowance for every command. To keep this
guarantee independent of Neo's event loop, each admitted guardian receives a static
per-command descendant allowance of
`max_descendant_processes / max_active_commands` and a memory allowance of
`max_tree_memory_percent / max_active_commands`; integer division rounds down and
validated configuration must leave both allowances at least one. Unused allowance is
not borrowed. This deliberately trades peak throughput for a forest-wide bound that
still holds when Neo is alive but stalled. Process count excludes guardians and
command leaders and counts all discovered descendants. Memory is the resident memory
of the command leader plus descendants divided by physical memory.

## Ownership Architecture

Production commands use one topology only:

```text
Neo
 `- neo __process-guard
     `- isolated shell process group or Windows Job Object
         `- command descendants
```

`__process-guard` is a hidden subcommand of the current Neo executable, not a daemon
or separately installed binary. `neo-agent-core` contains the protocol and guardian
runtime; the `neo-agent` binary only exposes the hidden entry point and supplies its
current executable path to tool contexts. Tests may override that executable path.

Neo creates the control and response pipes, starts the guardian in the resolved
command working directory, sends one validated Start frame, and retains the control
pipe for the complete command lifetime. The guardian creates the status location and
resource sampler before spawning user code. It reports Started only after the command
has entered its process group or Job Object. A launch that cannot establish ownership
fails without spawning the command.

Ownership pipe handles are close-on-exec on Unix and non-inheritable on Windows. The
guardian uses an explicit handle allowlist when spawning user code: a Bash command
receives only its stdin/stdout/stderr handles, and a Terminal command receives only the
PTY slave handles it requires. No command descendant can inherit either side of the
Neo/guardian control channel and keep parent-death detection open.

The guardian is outside the command's process group or Job Object. Neo records the
guardian identity and the command group/job identity from the Started response. This
creates complementary failure handling:

- If Neo disappears, the guardian sees control-pipe EOF and kills the command tree.
- If the guardian disappears while Neo is alive, response-pipe EOF makes Neo perform
  emergency cleanup only after validating the last confirmed command identity. On
  Unix, Started carries leader PID, leader start identity, and process-group ID; Neo
  verifies the leader identity before signaling the group and refuses a signal if the
  identity has been reused. On Windows, the guardian is the only owner of the
  kill-on-close Job handle, so guardian loss closes the Job and the operating system
  performs emergency cleanup automatically.
- During orderly shutdown, Neo requests termination, closes every control pipe, and
  waits for guardians without blocking the TUI thread.

No message can detach ownership. Moving a foreground command out of the active tool
call sends only `SetBackgroundDeadline`; the command retains the same ownership pipe
and guardian.

## IPC Protocol

The control protocol is a private, length-prefixed binary protocol between processes
from the same Neo executable:

```text
u32 big-endian body length | u8 frame kind | u64 request id | payload
```

The maximum frame body is 1 MiB. Terminal Write payloads are at most 64 KiB and are
split by the Neo client when necessary. Unknown kinds, oversized frames, truncated
frames, and malformed payloads are fatal protocol errors that terminate the owned
tree. There is no version negotiation or legacy decoder; both endpoints always come
from the same binary. Low-frequency typed metadata may use JSON inside a frame, while
command and PTY bytes remain raw. A logical Snapshot or Exited result larger than one
frame is split into ordered frames with the same request ID and an explicit final flag;
no physical frame may exceed the limit.

Neo-to-guardian frames are:

- `StartBash`: command, foreground/background class, deadline, limits, task identity,
  and persistence metadata. The guardian inherits the already-resolved working
  directory instead of serializing paths through UTF-8.
- `StartTerminal`: the same metadata plus PTY rows and columns.
- `Write`: raw Terminal input.
- `Read`: Terminal output offset and requested byte ceiling.
- `Resize`: Terminal rows and columns.
- `SetBackgroundDeadline`: changes a detached foreground Bash command to a deadline
  of `now + background_timeout_secs`.
- `Stop`: explicit cancellation.

Guardian-to-Neo frames are:

- `Started`: guardian identity, command identity, and initial offsets.
- `Output`: bounded Bash stdout or stderr live data. Unsolicited events use request ID
  zero.
- `Ack`: successful Write, Resize, deadline update, or Stop acceptance.
- `Snapshot`: Terminal output/status for a Read request, including total, returned,
  discarded, and unread byte counts.
- `Exited`: the single terminal status and output metadata.
- `Error`: a typed launch, protocol, I/O, or supervision failure.

Request IDs allow a Stop or Read to complete independently of a slow PTY write. The
guardian uses a dedicated PTY writer worker with a one-item bounded queue; large
writes are chunked and acknowledged in order. Write uses a non-blocking queue send; a
full slot returns `Busy`, and the client retries only after the outstanding Write is
acknowledged. Stop bypasses the writer queue and starts termination immediately. Read
and Resize never wait for writer-queue capacity. Response queues and Bash live-output
queues are bounded. Control replies have priority, and live deltas are coalesced or
dropped with omission metadata. Queue saturation never stops child-pipe draining.
Response-pipe breakage is treated as parent loss and terminates the tree.

Terminal output remains pull-based. The guardian owns the PTY master, writer, reader,
resize handle, UTF-8 decoder, read offset data, and bounded output ring. Neo issues
metadata-only reads while observing the existing quiet-period behavior and then asks
for one capped snapshot. High-frequency PTY bytes never flow continuously into the
TUI process.

## Guardian Runtime

Each guardian has four independent activities:

1. Drain child stdout/stderr or PTY output in 8 KiB chunks until EOF, even after the
   in-memory or live-event limit is reached.
2. Process bounded control frames without performing blocking PTY writes inline.
3. Watch the monotonic deadline, process tree, resident memory, root exit, and parent
   pipes at a 250 ms cadence.
4. Serialize response frames and persist the final state without blocking the other
   activities.

Process enumeration or physical-memory discovery failure is fail-closed: the command
ends as `resource_limited` with a sampler-unavailable reason. A watcher is created for
every foreground Bash, background Bash, and Terminal command. Deadlines do not depend
on `TaskOutput`, Terminal Read, TUI rendering, or any later user action.

An explicitly backgrounded Bash command and every Terminal session start with
`background_timeout_secs`. A foreground Bash command starts with
`foreground_timeout_secs`. Moving a foreground Bash command to the background sends
`SetBackgroundDeadline` and resets the deadline to the current monotonic time plus
`background_timeout_secs`.

The first terminal cause wins atomically:

```text
starting -> running -> completed
                    -> failed
                    -> cancelled
                    -> timed_out
                    -> resource_limited
                    -> parent_exited
```

A natural command-leader exit is not enough to declare completion. The guardian first
terminates and reaps any remaining process-group, Job Object, or discovered descendant
members, preventing commands such as `cmd &` from escaping ownership. All termination
causes converge on one cleanup routine.

## Termination Semantics

On Unix, cleanup sends `SIGTERM` to the command process group and every discovered
out-of-group descendant, waits at most 500 ms, refreshes the descendant snapshot, then
sends `SIGKILL` and reaps the command leader. Missing processes are success. The current
macOS empty-process-group `EPERM` behavior during the KILL phase remains treated as
already gone.

Windows has no POSIX TERM. The graceful phase closes PTY input or the relevant command
handles and waits up to 500 ms. The force phase closes a
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` Job Object and waits for the leader. The existing
launch barrier remains mandatory so user code cannot execute before Job assignment.

Cancellation, timeout, resource limits, parent exit, protocol failure, response
backpressure, and guardian shutdown all use these semantics. Cleanup errors are
recorded in the terminal status and surfaced after the best available tree cleanup;
they never restart or detach the command.

## Output and Backpressure

Bash stdout and stderr share one in-memory budget equal to the validated
`max_output_bytes` value (65,536 bytes by default). A tagged head/tail buffer retains
the first half and last half of the combined arrival stream and records omitted bytes.
Stream tags preserve separate stdout and stderr materialization for the existing
result and transcript APIs. The omission marker is metadata and does not consume
retained payload capacity.

Terminal keeps a tail ring whose capacity is `max_output_bytes` because incremental
reads require stable total offsets rather than a head/tail view. Reads report
discarded and unread bytes. A tool call may request less output but cannot request more
than the configured ceiling.

Background Bash output is also appended by the guardian to a per-task log with a hard
`max_background_log_bytes` payload ceiling (10 MiB by default). Once full, the
guardian continues draining child pipes but stops appending and marks the log
truncated. The in-memory head/tail summary therefore still preserves recent output
even when the log contains only the first configured log budget.
Guardian diagnostics and child output never write directly to Neo's inherited TUI
stdout or stderr.

After the command leader exits, drains receive a bounded two-second grace period so a
descendant that inherited an output handle cannot keep the guardian alive forever.
The remaining tree is then terminated and the final output state is persisted.

## Persistence and Resume

For a session-backed task, supervision artifacts live at:

```text
<session>/agents/<agent-id>/tasks/<task-id>.running.json
<session>/agents/<agent-id>/tasks/<task-id>.status.json
<session>/agents/<agent-id>/tasks/<task-id>.log
```

The running record is created before user code starts and identifies the task,
guardian, owner process instance, command kind, description, and start time. The
guardian creates `status.json` exactly once through the existing atomic-file helper
after cleanup finishes. A create-once final file avoids non-atomic overwrite behavior
on Windows. Its schema has an explicit version and contains:

- terminal state and typed reason;
- start and finish timestamps;
- exit code and Unix signal when available;
- configured limit and observed value for a resource-limit event;
- retained-output, live-output, and log omission counts;
- cleanup and persistence errors.

No-session execution uses the same layout under
`<NEO_HOME>/runtime/<neo-instance-id>/agents/<agent-id>/tasks/`; Neo scavenges completed
instance directories on later startup.

Resume never takes ownership of a prior process. It consumes a valid final status when
present. If only a prior-process running record exists, resume polls for up to three
seconds, covering the 500 ms termination grace, two-second output-drain grace, and
final atomic write. If no final file appears, Neo exposes a synthetic
`parent_exited` terminal snapshot but does not create `status.json`; the original
guardian remains the only writer and may still publish the more precise terminal
cause. Neo does not attach to or signal PIDs recorded by an earlier process instance,
avoiding both write races and PID-reuse hazards. Concurrently resuming the same session
from multiple Neo processes is not a supported way to share a shell task.

## Product Semantics

`ShellCommandOutcome` gains the canonical `ResourceLimited` variant. Structured result
details and background task snapshots carry the specific cause (`active_commands`,
`process_count`, `tree_memory`, or `sampler_unavailable`) and configured/observed
values. Background task status also represents `timed_out`, `resource_limited`, and
`parent_exited` as terminal states.

Every terminal guardian outcome finalizes the corresponding TUI shell run and stops
its working spinner. Resource exhaustion is rendered as a neutral limit message with
the measured value, not as an unexplained command failure. Task Output reads persisted
state and output; it does not own or drive the timeout watcher.

## Cross-Platform Process Discovery

Neo adds one maintained cross-platform process-information dependency for parent/child
enumeration, resident memory, process start identity, and total physical memory. It
does not parse `/proc`, invoke `ps` or `tasklist`, or maintain three custom samplers.
Platform-specific containment stays isolated behind existing `cfg(unix)` and
`cfg(windows)` modules:

- Linux and macOS use a separate process group plus refreshed descendant snapshots.
- Windows uses one non-breakaway Job Object owned by the guardian and the existing
  pre-execution assignment barrier.
- Other targets return a typed unsupported-containment error before spawning user
  code; they never panic or create a child they cannot own.

## Codex Reference

The useful Codex patterns are retained: 8 KiB drains, bounded live events, a 50/50
head/tail buffer with omitted-byte accounting, continued draining after retention is
full, a bounded post-exit drain, independent Unix process groups, and bounded PTY
control messages.

Neo does not copy Codex's full exec-server architecture, 64-session LRU, or polling
timeout semantics. Codex does not provide a general macOS parent-death mechanism,
absolute background deadline, tree RSS limit, descendant limit, or universal Windows
Job ownership. Those gaps are exactly what the guardian and resource watchdog address.

## Migration

- Delete Bash direct spawning, the polling-driven background deadline, per-stream
  retention limits, and the existing foreground/background ownership split.
- Move Terminal PTY ownership, output buffering, resize, writer, reader, and process
  cleanup into the guardian. Neo retains only lightweight client handles.
- Keep the public Bash and Terminal tool schemas stable except that requested timeout
  and output values are clamped to configured ceilings.
- Reuse existing process-group, Windows Job/launch-barrier, atomic-file, background
  task, and transcript components where their responsibilities remain valid.
- Do not add a fallback direct-spawn path when guardian startup fails.

## Verification

Verification is narrow and fault-oriented:

- Protocol unit tests reject oversized, unknown, malformed, and truncated frames and
  preserve request IDs and split UTF-8/raw bytes.
- Buffer tests prove shared 50/50 Bash head/tail behavior, Terminal tail offsets, and
  continued drain after all retention and live-event limits are reached.
- Config tests prove defaults, validation, tool-input clamping, and conditional
  injection of the three concurrency environment variables.
- Admission tests prove Bash and Terminal share the two-command limit and that a third
  launch fails before creating a guardian.
- Process tests close the parent pipe to simulate Neo death and assert that a real
  child plus descendant are gone after the 500 ms grace period and that status is
  `parent_exited`.
- Process tests kill the guardian while Neo remains alive and assert emergency cleanup
  from the confirmed command identity.
- Deadline and descendant-count tests assert a real tree is killed without Task Output
  polling. Memory arithmetic uses synthetic process snapshots rather than allocating
  dangerous amounts of RAM.
- Limit-allocation tests prove the static per-command allowances cannot exceed the
  configured forest-wide process or memory ceilings at maximum command occupancy.
- A natural leader-exit test leaves a background descendant and proves the guardian
  removes it before reporting completion.
- Existing real PTY start/write/read/resize/stop and blocked-write tests are adapted to
  the guardian client rather than duplicated.
- Windows retains a dedicated launch-barrier/Job descendant test; Unix retains a
  process-group descendant test.
- Resume tests prove a final status is rehydrated and an earlier nonterminal record is
  converged to `parent_exited` without PID signaling.
- TUI tests prove `ResourceLimited` and `parent_exited` finish the shell transcript and
  stop the spinner.

No broad workspace test is required as local evidence. The implementation plan must
name one package, one target selector, and a narrow test filter for every verification
step, matching the repository test policy.
