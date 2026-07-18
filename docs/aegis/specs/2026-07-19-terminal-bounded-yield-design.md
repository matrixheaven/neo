# Terminal Raw PTY and Bounded Yield

## Status

Approved direction from the 2026-07-19 Terminal tool review. Written review is
required before implementation planning.

This design refines the existing `Terminal` tool contract. It does not replace
the shared shell admission scheduler, guardian runtime, or raw PTY output
owner.

## Decision

Keep `Terminal` as a raw PTY tool and add bounded output collection to
`start`, `write`, and `read` through one optional `yield_time_ms` field.

The three time domains remain independent:

```text
admission wait         command lifetime              output yield
unbounded              unbounded by default          bounded
tool call stays        ends only on exit, explicit   returns output plus
pending                timeout, cancel, or stop       status: running
```

`yield_time_ms` starts only after admission succeeds and the relevant terminal
operation is ready. Reaching its deadline never stops the guardian, child
process, process tree, or shell admission permit.

## Problem

The current tool already provides a supervised PTY, typed `cwd`, bounded output,
incremental reads, resize, raw control-byte writes, and a three-second maximum
wait for `read`. The external review therefore misclassified several existing
capabilities as missing.

One real interaction cost remains: `start` and `write` return without collecting
the output they normally trigger. The model usually needs a second `read` call
to see an initial banner, prompt, command response, or interrupt result. The
fixed three-second `read` wait is also not selectable by the caller.

The repair must not weaken two existing shell contracts:

- admission saturation applies transparent backpressure and keeps the original
  tool call pending; and
- omission of an execution timeout permits commands such as long test suites to
  run for tens of minutes without Neo stopping them.

## Goals

- Return initial PTY output from `start` without a separate `read` call.
- Return resulting PTY output from `write` without a separate `read` call.
- Let `read` choose a bounded observation window.
- Preserve incremental, capped, UTF-8-safe output consumption.
- Return promptly after output becomes quiet, the process exits, or the
  selected yield deadline expires, while preserving existing cancellation
  ownership.
- Keep raw PTY bytes as the only output source of truth.
- Make existing control-byte input discoverable in the tool schema and public
  documentation.
- Preserve Windows, Linux, and macOS behavior through the existing
  `portable-pty` and guardian boundaries.

## Non-Goals

- Admission timeouts, queue handles, queue polling, or releasing the model while
  shell capacity is unavailable.
- A default command execution timeout or any change to `timeout_secs`.
- A wall-clock deadline on `GuardianClient::request`.
- Killing a command when an output yield expires.
- ANSI stripping, VT screen rendering, or a second plain-output projection.
- Echo suppression or subtracting previously written input from PTY output.
- A new signal API. Raw JSON strings already carry control bytes.
- Terminal reattach, persistence across Neo restarts, or cursor replay.
- Changing `resize` or `stop` result semantics.
- Changing Bash foreground, background, admission, or output behavior.

## Product Contract

### Input

`TerminalInput` gains one optional field:

```text
yield_time_ms: optional integer
valid modes: start, write, read
effective range: 0..=30000
```

Mode-specific defaults are:

| Mode | Default | Reason |
| --- | ---: | --- |
| `start` | 250 ms | Capture ordinary banners/prompts without delaying handle creation for silent processes. |
| `write` | 250 ms | Capture the immediate response while keeping interactive input responsive. |
| `read` | 3000 ms | Preserve the current delayed-prompt observation window. |

`yield_time_ms = 0` performs an immediate snapshot after the operation is ready.
Values above 30000 are rejected rather than silently clamped so the model sees
the actual contract.

The schema description must state that yield is not an execution timeout and
does not include admission queue time.

### Timing

For `start`, the yield clock begins after all of the following have completed:

1. permission and typed-path validation;
2. shell admission;
3. post-admission revalidation;
4. guardian spawn and handshake; and
5. terminal session insertion into `ShellRuntime`.

For `write`, the clock begins after the guardian acknowledges the input bytes.
For `read`, it begins when the mode starts observing the existing session.

Collection returns at the first applicable condition:

1. new output has remained unchanged for the existing 50 ms quiet period;
2. the command reaches a terminal state;
3. the caller is cancelled, subject to the start ownership rule below; or
4. the yield deadline expires.

Admission waiting remains unbounded and keeps the original tool future pending.
Command lifetime remains unbounded when `timeout_secs` is omitted. A yield
deadline changes neither contract.

### Output

`start`, `write`, and `read` use one canonical output collector and return the
same observation fields where applicable:

```text
handle
status
exit_code
output
truncated
read_offset_before
read_offset_after
total_output_bytes
unread_bytes_after
cols
rows
```

`start` continues to include command and process metadata. `write` continues to
report that input was accepted. These mode-specific fields do not create
separate output owners.

Collected bytes advance the session's existing read offset. Output returned by
`start` or `write` must therefore not be repeated by the next `read`. When
`max_output_bytes` leaves unread bytes, the next operation continues from the
first unread byte rather than skipping it.

`max_output_bytes` applies to every output-bearing `start`, `write`, and `read`
result and remains capped by the runtime limit. The guardian's bounded raw
buffer remains authoritative.

### Raw PTY Semantics

PTY echo, ANSI escape sequences, carriage returns, backspaces, and cursor
control are valid terminal output. Neo must not infer which bytes are program
output versus terminal echo and must not delete byte sequences based on prior
`write` calls.

The `input` schema description must explain that escaped control bytes are
supported, including:

```text
Ctrl+C = \u0003
Ctrl+D = \u0004
Ctrl+Z = \u001a
```

These are raw terminal inputs, not cross-platform signal guarantees. Programs
and host PTY implementations retain authority over their meaning.

## Architecture

The existing ownership remains unchanged:

```text
TerminalTool
  -> ShellRuntime admission and terminal session map
  -> GuardianClient control protocol
  -> terminal guardian
  -> portable-pty child and bounded raw output buffer
```

The implementation should replace `wait_for_output_quiet_period` with one
collector used by `start`, `write`, and `read`. The collector owns observation
timing and offset advancement; it does not own admission, process lifetime,
termination, output storage, or presentation filtering.

No new module, actor, dependency, fallback, compatibility path, or alternate
buffer is required.

## Error and Cancellation Semantics

- Invalid mode use or out-of-range `yield_time_ms` is an input error.
- Cancellation while waiting for admission follows the existing scheduler
  cancellation path.
- Cancellation during `read` or `write` collection follows the existing
  cancelled tool outcome because the caller already owns the handle.
- After `start` inserts a session into `ShellRuntime`, cancellation must not
  leave a running process whose handle was never returned. The start path must
  either return the current handle-bearing result or stop and remove the
  session through its existing owner before returning cancellation.
- Guardian exit and protocol EOF continue to settle pending responses through
  the existing reader/final-result path.
- Yield expiry is not an error. It returns the available output and current
  `status`, normally `running`.
- This design adds no guardian request watchdog. A live guardian control-plane
  deadlock would require separate reproduction evidence and a cleanup design
  that cannot be confused with command execution timeout.

## Compatibility and Retirement

This is one canonical Terminal contract, not a second API:

- `yield_time_ms` replaces the hidden fixed wait as the public observation
  control for `read`.
- The existing 3000 ms behavior remains the `read` default.
- `start` and `write` gain a bounded 250 ms observation phase and may now
  include output in their successful results.
- No legacy `read_initial`, `strip_ansi`, `echo`, `signal`, or alternate output
  fields are introduced.
- The old read-only wait helper is retired once all three modes use the shared
  collector.

There are no persistent Terminal handles to migrate and no session JSONL schema
change.

## Verification

Use the narrowest existing targets. New coverage must prove behavior rather
than duplicate the existing lifecycle and resize tests:

1. A silent `read` returns `status: running` within its selected yield bound and
   leaves the process alive.
2. `start` returns initial output and the next `read` does not repeat it.
3. `write` returns resulting output and the next `read` does not repeat it.
4. Queue time is excluded from the start yield clock and the tool call remains
   pending until admission.
5. Omitting `timeout_secs` still permits a long-running command after repeated
   yields.
6. A typed relative `cwd` is used by the launched process.
7. Sending `\u0003` interrupts an interruptible command and leaves the shell
   session usable where the platform PTY supports that behavior.
8. Output caps and UTF-8 boundaries remain correct for output returned by
   `start`, `write`, and `read`.
9. Cancellation after start registration cannot leave an undisclosed live
   terminal session.

Public English and Chinese tool documentation must describe the same yield,
queue, lifetime, raw output, and control-byte semantics.

## Scope and Governance

Task intent:

- Outcome: make the existing Terminal interaction loop efficient for models
  without weakening shell backpressure or long-command execution.
- Success evidence: the focused contract tests above and exact target
  verification pass.
- Stop condition: `start`, `write`, and `read` share the bounded collector;
  documentation is synchronized; no alternate output path is added.

Baseline usage:

- Product requirement: the approved conversation decision and the shell
  admission scheduler's transparent-backpressure contract.
- Runtime boundary: the supervised shell guardian and current `TerminalTool`
  ownership.
- Alignment result: aligned; this spec refines the Terminal observation
  contract without changing scheduler or process-lifetime authority.

Impact statement:

- Affected layers: Terminal input schema, Terminal orchestration, focused
  guardian-backed integration tests, and English/Chinese tool docs.
- Canonical owner: `TerminalTool` for yield and offset behavior; guardian raw
  buffer for output storage; `ShellRuntime` for admission and lifetime.
- Architecture review required: yes, because the public tool contract changes.
- ADR signal: no new ADR. Ownership and dependency direction remain unchanged;
  this design spec is the durable contract refinement.

Minimality decision:

- Reuse the existing Terminal session state, read lock, guardian snapshots,
  quiet period, and output cap.
- Reject new projections, signal adapters, watchdogs, or persistence surfaces.
- Edit in place; no new owner file is justified.
