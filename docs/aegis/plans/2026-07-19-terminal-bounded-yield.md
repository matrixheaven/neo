# Terminal Raw PTY and Bounded Yield Implementation Plan

> **For agentic workers:** Execute with `aegis:executing-plans` or
> `aegis:subagent-driven-development`. Preserve unrelated dirty work, avoid
> shell sleep/retry loops, and run only the exact verification below.

**Goal:** Make Terminal `start`, `write`, and `read` return bounded incremental
raw PTY output while preserving transparent admission waiting and unlimited
command lifetime when `timeout_secs` is omitted.

**Architecture:** Keep `TerminalTool` as the only observation/offset owner.
Replace its read-only quiet-period helper with one collector shared by
`start`, `write`, and `read`. Keep `ShellRuntime` as admission/session owner
and the guardian buffer as raw output source of truth. Replace the
`portable-pty` package with the API-compatible `xpty` backend so headless
Windows ConPTY creation does not request inherited cursor state before a
Terminal handle can be disclosed.

**Tech Stack:** Rust 2024, Tokio, Serde/Schemars, existing guardian IPC,
`xpty` behind the existing Cargo dependency key, `ToolResult` details,
Markdown.

**Baseline/Authority Refs:**

- `docs/aegis/specs/2026-07-19-terminal-bounded-yield-design.md`
- `docs/aegis/specs/2026-07-18-shell-admission-scheduler-design.md`
- `docs/aegis/specs/2026-07-15-supervised-shell-execution-design.md`
- `AGENTS.md`

**Compatibility Boundary:** Queue waiting stays unbounded and keeps the tool
call pending. Command lifetime stays unbounded unless the caller supplies
`timeout_secs`, cancels, or stops. Yield expiry never kills or releases the
process, guardian, or permit. Raw echo/ANSI/control bytes, resize, stop, output
caps, and cleanup stay canonical. Add no fallback, alias, projection, watchdog,
or persistent handle migration.

**TDD Route:**

- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change regression
- Reason: strict TDD was not requested; focused real-PTY regressions cover only
  the new contract and retain distinct lifecycle/cap tests.
- Verification: exact commands in each task.

**Verification:** Every Cargo command names one package, one target, and one
exact test. Run through `rtk`; do not run workspace-wide tests.

---

## Scope Check

**Requirement Ready Check:** Ready. The approved spec fixes defaults, range,
timing, output semantics, non-goals, and acceptance evidence.

**Change Necessity:** Code change. Docs cannot return initial/post-write output
or expose a selectable read window. Review also proved that the runtime
cancellation wrapper can drop a post-registration Terminal future before its
cleanup settles. Minimum boundary: `tools/terminal.rs`, the wrapper in
`runtime/tool_dispatch.rs`, their narrow regressions, existing Windows process
guard coverage, the PTY backend dependency, and zh/en tool docs.

**Existence Check:** Reuse current owners and replace one backend dependency in
place. No new module, actor, buffer, projection, signal adapter, request
watchdog, or compatibility runtime path.

**Architecture Integrity Lens:** `TerminalTool` observes, the guardian stores
raw bytes, `ShellRuntime` owns admission/session lifetime, and the PTY backend
owns ConPTY creation flags. One collector replaces the old read-only helper;
`xpty` removes the inherited-cursor requirement at its source. Verdict: edit in
place.

**Plan-Time Complexity Check:** `terminal.rs` is a single-purpose owner of about
580 lines. The change consolidates existing read logic and remains within
budget. Do not extract a file without implementation evidence.

**Plan Pressure Test:** Proceed. The user's later instruction to fix all review
findings authorizes the runtime/test scope expansion recorded below. Any further
guardian wire-protocol/Bash/runtime/TUI change pauses execution and returns to
the plan.

---

## File Map

**Modify**

- `crates/neo-agent-core/src/tools/terminal.rs`
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- `crates/neo-agent-core/Cargo.toml`
- `Cargo.lock`
- `crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs`
- `crates/neo-agent-core/src/tools/shell_guard/guardian.rs`
- `crates/neo-agent-core/src/tools/shell_guard/protocol.rs`
- `crates/neo-agent/tests/tool_terminal_guardian.rs`
- `crates/neo-agent/tests/process_guard_windows.rs`
- `docs/en/reference/tools.md`
- `docs/zh/reference/tools.md`

This is an approved scope expansion under the user's instruction to fix all
review findings. It is not a second implementation path or a broader runtime
redesign.

**Do not modify**

- `shell_guard/client.rs`, `scheduler.rs`, or protocol frames.
- Bash, runtime event, or TUI owners unless a focused regression proves a real
  break and the plan is revised first.

---

### Task 1: Implement One Bounded Observation Contract

**Files:** Modify `crates/neo-agent-core/src/tools/terminal.rs`.

**Why:** Remove start/write-to-read round-trips without changing admission or
execution lifetime.

**Change Necessity:** The hardcoded read wait and output-less start/write are
owned here; no other source file is necessary.

**Impact/Compatibility:** Adds one optional field. Existing calls stay valid,
read retains its 3000 ms default, and resize/stop/guardian IPC stay unchanged.

**Repair Track:** Consolidate timing, snapshot, offset, status, cap, callback,
and details construction in `TerminalTool`.

**Retirement Track:** Delete `wait_for_output_quiet_period`; keep no alias.

- [ ] **Step 1: Add schema constants, field, and validation**

Add:

```rust
const TERMINAL_START_WRITE_YIELD: Duration = Duration::from_millis(250);
const TERMINAL_READ_YIELD: Duration = Duration::from_secs(3);
const TERMINAL_MAX_YIELD_MS: u64 = 30_000;
```

Add this `TerminalInput` field:

```rust
#[schemars(
    description = "Wait for incremental PTY output after start/write or while reading. The clock starts only after admission and operation readiness; expiry returns current output with status running and never stops the command. Defaults: 250 ms for start/write, 3000 ms for read. Valid only for start/write/read.",
    range(min = 0, max = 30000)
)]
yield_time_ms: Option<u64>,
```

Make `TerminalMode` `Clone + Copy`. Reject the field on resize/stop and reject
values above 30000; do not clamp. Resolve defaults through:

```rust
fn terminal_yield(mode: TerminalMode, requested: Option<u64>) -> Duration {
    requested.map_or_else(
        || match mode {
            TerminalMode::Start | TerminalMode::Write => TERMINAL_START_WRITE_YIELD,
            TerminalMode::Read => TERMINAL_READ_YIELD,
            TerminalMode::Resize | TerminalMode::Stop => Duration::ZERO,
        },
        Duration::from_millis,
    )
}
```

Update tool/input descriptions to document raw `\\u0003`, `\\u0004`, and
`\\u001a` bytes without promising portable signal semantics.

- [ ] **Step 2: Replace the read-only wait with one collector**

Move the existing read lock, offset, quiet wait, final-result fallback, UTF-8
conversion, truncation, callback, status, resource limit, and structured
details into:

```rust
async fn collect_terminal_output(
    ctx: &ToolContext,
    tool: &str,
    handle: &str,
    session: &TerminalClientSession,
    max_output_bytes: usize,
    yield_for: Duration,
) -> Result<ToolResult, ToolError>
```

With zero yield, snapshot immediately. Otherwise preserve the 50 ms quiet
period and 10 ms polling, returning on quiet output, final state, cancellation,
or deadline. For `start`, do not let quiet output return before
`min(250 ms, yield_for)` so raw PTY bootstrap bytes cannot prematurely end the
initial observation; `write` and `read` have no settle floor. Deadline returns
the current snapshot and is not an error. Hold the read lock through offset
advancement; different handles stay independent.

Preserve these base details:

```rust
json!({
    "handle": handle,
    "status": status,
    "exit_code": exit_code,
    "output": output,
    "output_truncated": unread > 0,
    "truncated": truncated,
    "read_offset_before": read_offset,
    "read_offset_after": snapshot.offset,
    "total_output_bytes": snapshot.total,
    "unread_bytes_after": unread,
    "discarded_bytes_before_read": snapshot.discarded,
    "cols": cols,
    "rows": rows,
})
```

- [ ] **Step 3: Route start, write, and read through the collector**

Pass `yield_for` and `max_output_bytes` to start/write and `yield_for` to read.
Start inserts one cloned `TerminalClientSession`, then collects and merges
command/PID metadata. If any collector error occurs before the handle-bearing
result is returned, remove the session, call existing `client.stop()`, and then
return the original error. Write sends raw input first, then collects and adds
`written: true`. Read calls the same collector. All three advance one offset;
never filter echo.

- [ ] **Step 4: Add one focused unit test**

Add `terminal_yield_is_bounded_and_mode_scoped`, asserting:

```rust
assert_eq!(terminal_yield(TerminalMode::Start, None), Duration::from_millis(250));
assert_eq!(terminal_yield(TerminalMode::Write, None), Duration::from_millis(250));
assert_eq!(terminal_yield(TerminalMode::Read, None), Duration::from_secs(3));
assert_eq!(terminal_yield(TerminalMode::Read, Some(0)), Duration::ZERO);
```

Also execute the tool with `yield_time_ms: 30001` and with resize plus
`yield_time_ms: 1`; assert `InvalidInput` names `yield_time_ms`.

- [ ] **Step 5: Run exact unit verification**

```bash
cargo test --package neo-agent-core --lib -- tools::terminal::tests::terminal_yield_is_bounded_and_mode_scoped --exact --nocapture --include-ignored
```

Expected: one passing test.

---

### Task 2: Preserve Runtime Cancellation Settlement

**Files:** Modify `crates/neo-agent-core/src/runtime/tool_dispatch.rs`.

**Why:** The runtime wrapper currently races registry execution against the
same cancellation token. Winning the outer branch drops the in-flight tool
future, preventing Terminal's post-registration remove-and-stop path from
settling.

- [ ] **Step 1: Directly await cancel-aware Terminal start**

In `run_tool_with_cancel`, detect only `Terminal` with `mode=start` and directly
await its existing cancel-aware registry execution. Keep the existing
cancellation select for all other applicable tools. Add no timeout, watchdog,
detached cleanup task, or broader dispatch behavior change.

- [ ] **Step 2: Add the narrow runtime regression**

Add `terminal_start_cancellation_allows_internal_cleanup_to_settle` to the
existing `tool_dispatch.rs` unit-test module. Use one local Terminal test tool
that observes the supplied cancellation token, records cleanup settlement
before it returns, and assert dispatch does not return first.

- [ ] **Step 3: Run exact runtime verification**

```bash
cargo test --package neo-agent-core --lib -- runtime::tool_dispatch::tests::terminal_start_cancellation_allows_internal_cleanup_to_settle --exact --nocapture --include-ignored
```

Expected: one passing test.

---

### Task 3: Replace the Headless Windows PTY Backend

**Files:** Modify `crates/neo-agent-core/Cargo.toml`, `Cargo.lock`, and the
existing import in `crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs`.

**Why:** `portable-pty 0.9.0` unconditionally enables
`PSEUDOCONSOLE_INHERIT_CURSOR`, so Windows ConPTY emits `CSI 6n` and waits for a
terminal-emulator response before Neo can return a handle. Tool/guardian
bootstrap replies are timing-dependent duplicate owners.

**Change Necessity:** No configuration or test-only change can alter the
ConPTY creation flags. Replace the backend package at the existing dependency
key; add no local fork, feature adapter, or response parser.

**Impact/Compatibility:** `xpty 0.3.6` preserves the used `portable_pty` Rust
API while intentionally omitting inherited-cursor mode from its default
ConPTY flags. Unix PTY behavior, raw output, guardian IPC, and public tool
schema remain unchanged. Its `openpty` method requires the existing
`PtySystem` trait to be imported explicitly; this is compile-time wiring only.

- [ ] **Step 1: Replace the package at the existing dependency key**

```toml
portable-pty = { package = "xpty", version = "0.3.6" }
```

Add `PtySystem` to the existing `portable_pty` import in `terminal_guard.rs`.

- [ ] **Step 2: Regenerate the lockfile through Cargo**

Run the focused unit test from Task 1; Cargo must remove `portable-pty` from the
`neo-agent-core` dependency path and lock `xpty 0.3.6`. The workspace lockfile
may retain `portable-pty` through the unrelated `skim` dependency.

- [ ] **Step 3: Verify Terminal dependency retirement**

```bash
cargo tree --package neo-agent-core --depth 1
cargo tree --invert portable-pty@0.9.0
```

Expected: `neo-agent-core` directly uses `xpty`; any remaining
`portable-pty` package is outside the Terminal/core dependency path. The
current workspace retains it only through `skim`, which is out of scope.

---

### Task 4: Preserve In-Flight Guardian Protocol Reads

**Files:** Modify `crates/neo-agent-core/src/tools/shell_guard/protocol.rs`,
`guardian.rs`, and `terminal_guard.rs`.

**Why:** Both supervision loops construct `read_request(control)` directly in
`tokio::select!`. A process/resource timer can cancel that future after it has
consumed the frame length but before the body is complete; the next loop then
interprets the request kind as a new length and corrupts the control stream.

**Change Necessity:** Timing changes, retries, or a larger frame cap cannot
restore bytes already consumed by a cancelled future. The protocol owner must
retain the in-flight decoder future across cancelled `next()` observations.

**Impact/Compatibility:** Request encoding and wire bytes are unchanged. One
`futures::stream::unfold` helper owns the persistent read future; Bash and
Terminal supervision reuse it. No task, channel, timeout, protocol version, or
fallback is added.

- [ ] **Step 1: Add the cancellation-safe request stream**

Add `request_stream` beside `read_request`. It repeatedly calls the existing
decoder while retaining its internal future inside the stream.

- [ ] **Step 2: Route both supervision loops through the stream**

Pin one stream before each loop and select on `requests.next()`. Delete the two
direct `read_request(control)` branches.

- [ ] **Step 3: Add and run the focused regression**

The test writes only a frame's 4-byte length, polls then cancels `next()`, writes
the body, and asserts the same stream decodes the complete request:

```bash
cargo test --package neo-agent-core --lib -- tools::shell_guard::protocol::tests::request_stream_preserves_partial_frame_across_cancelled_poll --exact --nocapture --include-ignored
```

Expected: one passing test.

---

### Task 5: Prove the Real PTY Boundaries

**Files:** Modify `crates/neo-agent/tests/tool_terminal_guardian.rs` and
`crates/neo-agent/tests/process_guard_windows.rs`.

**Why:** Only the real guardian/PTY proves offset consumption, survival, typed
cwd, and control input.

**Change Necessity:** Existing tests cover lifecycle/resize and read caps, not
start/write output or post-registration cancellation.

**Impact/Compatibility:** Reuse current helpers and binary target. Add no fixture
framework or shell launcher.

- [ ] **Step 1: Add incremental-output and cwd coverage**

Add `terminal_start_write_and_read_share_incremental_bounded_output`:

1. Create `workspace/subdir/marker`.
2. Start with `cwd: "subdir"`, `yield_time_ms: 1000`, and command:

```text
test -f marker && printf initial-output; read line; printf 'reply:%s' "$line"; sleep 30
```

3. Assert start output contains `initial-output` and status is running.
4. Immediate read with yield zero must be empty.
5. Write `hello\n` with 1000 ms yield; assert `reply:hello` without rejecting
   valid echoed input.
6. Another immediate read must be empty; then stop.

On Windows, keep the normal nonzero start yield. The `xpty` backend must not
request inherited cursor state, so tests must neither observe nor answer a
startup `CSI 6n`. Require the child to check the relative `cwd` marker before
reporting `initial-output`; do not synthesize that output through a command
protocol. Resize coverage must query the child-observed console geometry before
and after `resize`; never hardcode requested dimensions. Remove whole-test
retries.

- [ ] **Step 2: Add control-byte coverage**

On Unix, add `terminal_ctrl_c_interrupts_command_and_keeps_session_usable`:
start `bash --noprofile --norc`, write `sleep 30\n`, write `\u0003`, then write
`printf control-alive\\n\n`; assert `control-alive` and stop. On Windows, use a
separately named `terminal_windows_session_remains_usable_without_signal_guarantee`
test that
proves only continued interaction. It must not claim or fake Ctrl+C signal
semantics. Add no signal adapter.

- [ ] **Step 3: Add start-cancellation cleanup coverage**

Add `terminal_start_cancellation_after_registration_cleans_up_process`: attach
a shared `CancellationToken` with `with_cancel_token`, spawn silent `sleep 30`
with 30000 ms yield, observe its running marker with existing bounded polling,
cancel, assert `ToolError::Cancelled`, and assert the marker disappears within
an outer Tokio timeout. Do not add a public terminal-count API.

The unified `Err(error)` branch in `start_terminal` covers cancellation and
other collector errors through the same remove-and-stop path. The cancellation
cleanup regression exercises that shared path; do not add fault-injection
infrastructure or a duplicate collector-error test.

- [ ] **Step 4: Run exact integration verification**

```bash
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_start_write_and_read_share_incremental_bounded_output --exact --nocapture --include-ignored
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_start_cancellation_after_registration_cleans_up_process --exact --nocapture --include-ignored
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_tool_start_write_read_resize_and_stop_uses_real_pty --exact --nocapture --include-ignored
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_read_details_do_not_leak_output_past_max_output_bytes --exact --nocapture --include-ignored
```

On Unix, also run:

```bash
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_ctrl_c_interrupts_command_and_keeps_session_usable --exact --nocapture --include-ignored
```

On Windows, run the accurately named usability test plus the three affected
process-guard tests:

```bash
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_windows_session_remains_usable_without_signal_guarantee --exact --nocapture --include-ignored
cargo test --package neo-agent --test process_guard_windows -- windows_terminal_guardian_loss_closes_job_with_descendant --exact --nocapture --include-ignored
cargo test --package neo-agent --test process_guard_windows -- windows_terminal_natural_exit_closes_job_with_descendant --exact --nocapture --include-ignored
cargo test --package neo-agent --test process_guard_windows -- windows_terminal_stop_closes_job_with_descendant --exact --nocapture --include-ignored
```

Expected: one passing test per applicable command.

- [ ] **Step 5: Check touched Rust formatting**

```bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent-core/src/tools/shell_guard/protocol.rs crates/neo-agent-core/src/tools/shell_guard/guardian.rs crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs crates/neo-agent-core/src/runtime/tool_dispatch.rs crates/neo-agent/tests/tool_terminal_guardian.rs crates/neo-agent/tests/process_guard_windows.rs
```

- [ ] **Step 6: Commit code and tests together**

```bash
git add Cargo.lock crates/neo-agent-core/Cargo.toml crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent-core/src/tools/shell_guard/protocol.rs crates/neo-agent-core/src/tools/shell_guard/guardian.rs crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs crates/neo-agent-core/src/runtime/tool_dispatch.rs crates/neo-agent/tests/tool_terminal_guardian.rs crates/neo-agent/tests/process_guard_windows.rs
git diff --cached --check
git commit -m "feat(terminal): add bounded PTY yield"
```

---

### Task 6: Synchronize the Public Contract

**Files:** Modify `docs/en/reference/tools.md` and
`docs/zh/reference/tools.md`.

**Why:** Users and models need identical queue, lifetime, yield, raw-output,
and control-byte semantics in both languages.

**Change Necessity:** Public parity is required after the source contract
changes; keep the edit to the Terminal row and adjacent Shell text.

- [ ] **Step 1: Update English and Chinese docs**

Document: start/write/read-only `yield_time_ms`; 250/250/3000 defaults; 0..=30000
range; queue time excluded and still pending; expiry returns raw output plus
running without stopping; omitted `timeout_secs` means no command deadline;
echo/ANSI remain raw; escaped Ctrl+C/D/Z bytes have no portable signal promise.

- [ ] **Step 2: Verify parity and diff hygiene**

```bash
rg -n "yield-time_ms|timeout_secs|Ctrl\\+C|raw PTY|原始 PTY" docs/en/reference/tools.md docs/zh/reference/tools.md
git diff --check -- docs/en/reference/tools.md docs/zh/reference/tools.md
```

- [ ] **Step 3: Commit docs separately**

```bash
git add docs/en/reference/tools.md docs/zh/reference/tools.md
git diff --cached --check
git commit -m "docs(terminal): document bounded PTY yield"
```

---

## Final Verification and Review

Repeat the exact Cargo tests from Tasks 1-5, including the runtime settlement
regression and platform-appropriate Windows session-usability test, then run:

```bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent-core/src/tools/shell_guard/protocol.rs crates/neo-agent-core/src/tools/shell_guard/guardian.rs crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs crates/neo-agent-core/src/runtime/tool_dispatch.rs crates/neo-agent/tests/tool_terminal_guardian.rs crates/neo-agent/tests/process_guard_windows.rs
git diff --check
```

Architecture review checklist:

- one Terminal observation owner;
- no yield influence on admission or execution deadline;
- Terminal start dispatch directly awaits its cancel-aware internal cleanup;
- `xpty` owns Windows ConPTY creation without inherited-cursor bootstrap, with
  no TerminalTool/guardian response path, guardian wire-format change, or
  Bash/TUI product-behavior change;
- no echo/ANSI filtering or duplicate output field;
- old read-only helper deleted;
- no collector error can orphan an undisclosed start;
- Windows start output, relative cwd, and resize tests observe real behavior
  without hardcoded success values or whole-test retries;
- English/Chinese docs agree.

Do not claim Windows source verification from a macOS-only run. Report any
unavailable Windows CI/toolchain as residual verification risk.

## Execution Readiness View

- Intent Lock: approved raw PTY + bounded yield only.
- Scope Fence: Terminal owner, Terminal-start dispatch branch, PTY package
  replacement, focused tool-dispatch/PTY/Windows process-guard tests, and zh/en
  docs.
- Baseline Lock: approved design plus admission and supervised-shell specs.
- Approved Behavior: bounded start/write/read observation; queue and default
  command lifetime remain unbounded.
- Owner Constraints: TerminalTool observes, guardian stores, ShellRuntime owns,
  and the backend owns ConPTY creation flags.
- Compatibility: existing calls valid; read default and resize/stop unchanged.
- Retirement: delete the old read-only helper and retire `portable-pty` from
  the Terminal/core dependency path. The unrelated `skim` transitive dependency
  remains out of scope; add no runtime compatibility path.
- Task Batches: source contract, runtime settlement, PTY backend replacement,
  cancellation-safe guardian reads, real-PTY tests, docs.
- Test Obligations: focused exact tests for the source contract, runtime
  settlement, real PTY, Windows ConPTY behavior, touched rustfmt, and diff
  hygiene.
- Review Gates: behavior, architecture invariants, docs parity, staged audit.
- Drift Rule: any wider owner change pauses and returns to the plan/spec.
- Evidence: passing exact commands and architecture review.
- Advisory Boundary: method-pack guidance only; not completion authority.

## Risks and Retirement

- PTY prompt timing varies; deterministic tests use explicit output.
- Cancellation after registration and its runtime wrapper are one ownership
  chain; the wrapper must let cleanup settle before publishing cancellation.
- Any collector error before handle disclosure removes and stops the session.
- Offset errors can duplicate or skip bytes; immediate-read regressions prove
  the boundary.
- The worktree is dirty elsewhere. Never stage unrelated runtime/TUI files.
- No rollback compatibility path is retained; reverting the feature commit
  restores the prior code contract.
