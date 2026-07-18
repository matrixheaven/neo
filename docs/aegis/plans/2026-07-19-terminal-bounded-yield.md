# Terminal Raw PTY and Bounded Yield Implementation Plan

> **For agentic workers:** Execute with `aegis:executing-plans` or
> `aegis:subagent-driven-development`. Preserve unrelated dirty work, avoid
> shell sleep/retry loops, and run only the exact verification below.

**Goal:** Make Terminal `start`, `write`, and `read` return bounded incremental
raw PTY output while preserving transparent admission waiting and unlimited
command lifetime when `timeout_secs` is omitted.

**Architecture:** Keep `TerminalTool` as the only observation/offset owner.
Replace its read-only quiet-period helper with one collector shared by
`start`, `write`, and `read`. Keep `ShellRuntime` as admission/session owner,
the guardian buffer as raw output source of truth, and `portable-pty` process
lifecycle unchanged.

**Tech Stack:** Rust 2024, Tokio, Serde/Schemars, existing guardian IPC,
`portable-pty`, `ToolResult` details, Markdown.

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
or expose a selectable read window. Minimum boundary: `tools/terminal.rs`, its
existing guardian integration target, and zh/en tool docs.

**Existence Check:** Reuse current owners. No new module, actor, buffer,
dependency, projection, signal adapter, or request watchdog.

**Architecture Integrity Lens:** `TerminalTool` observes, the guardian stores
raw bytes, and `ShellRuntime` owns admission/session lifetime. One collector
replaces the old read-only helper. Verdict: edit in place.

**Plan-Time Complexity Check:** `terminal.rs` is a single-purpose owner of about
580 lines. The change consolidates existing read logic and remains within
budget. Do not extract a file without implementation evidence.

**Plan Pressure Test:** Proceed. Any required guardian/protocol/Bash/runtime/TUI
change pauses execution and returns to the plan.

---

## File Map

**Modify**

- `crates/neo-agent-core/src/tools/terminal.rs`
- `crates/neo-agent/tests/tool_terminal_guardian.rs`
- `docs/en/reference/tools.md`
- `docs/zh/reference/tools.md`

**Do not modify**

- `shell_guard/client.rs`, `terminal_guard.rs`, `scheduler.rs`, or protocol
  frames.
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
or deadline. Deadline returns the current snapshot and is not an error. Hold
the read lock through offset advancement; different handles stay independent.

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
command/PID metadata. If cancellation wins after insertion, remove the session
and call existing `client.stop()` before returning `ToolError::Cancelled`.
Write sends raw input first, then collects and adds `written: true`. Read calls
the same collector. All three advance one offset; never filter echo.

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

### Task 2: Prove the Real PTY Boundaries

**Files:** Modify `crates/neo-agent/tests/tool_terminal_guardian.rs`.

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

- [ ] **Step 2: Add control-byte coverage**

Add `terminal_ctrl_c_interrupts_command_and_keeps_session_usable`: start
`bash --noprofile --norc`, write `sleep 30\n`, write `\u0003`, then write
`printf control-alive\\n\n`; assert `control-alive` and stop. Isolate only a
proven ConPTY-specific assertion behind `cfg`; add no signal adapter.

- [ ] **Step 3: Add start-cancellation cleanup coverage**

Add `terminal_start_cancellation_after_registration_cleans_up_process`: attach
a shared `CancellationToken` with `with_cancel_token`, spawn silent `sleep 30`
with 30000 ms yield, observe its running marker with existing bounded polling,
cancel, assert `ToolError::Cancelled`, and assert the marker disappears within
an outer Tokio timeout. Do not add a public terminal-count API.

- [ ] **Step 4: Run exact integration verification**

```bash
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_start_write_and_read_share_incremental_bounded_output --exact --nocapture --include-ignored
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_ctrl_c_interrupts_command_and_keeps_session_usable --exact --nocapture --include-ignored
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_start_cancellation_after_registration_cleans_up_process --exact --nocapture --include-ignored
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_tool_start_write_read_resize_and_stop_uses_real_pty --exact --nocapture --include-ignored
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_read_details_do_not_leak_output_past_max_output_bytes --exact --nocapture --include-ignored
```

Expected: one passing test per command.

- [ ] **Step 5: Check touched Rust formatting**

```bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent/tests/tool_terminal_guardian.rs
```

- [ ] **Step 6: Commit code and tests together**

```bash
git add crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent/tests/tool_terminal_guardian.rs
git diff --cached --check
git commit -m "feat(terminal): add bounded PTY yield"
```

---

### Task 3: Synchronize the Public Contract

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

Repeat the six exact Cargo tests from Tasks 1-2, then run:

```bash
rustfmt --check --edition 2024 crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent/tests/tool_terminal_guardian.rs
git diff --check
```

Architecture review checklist:

- one Terminal observation owner;
- no yield influence on admission or execution deadline;
- no guardian/protocol/Bash/runtime/TUI changes;
- no echo/ANSI filtering or duplicate output field;
- old read-only helper deleted;
- cancellation cannot orphan an undisclosed start;
- English/Chinese docs agree.

Do not claim Windows source verification from a macOS-only run. Report any
unavailable Windows CI/toolchain as residual verification risk.

## Execution Readiness View

- Intent Lock: approved raw PTY + bounded yield only.
- Scope Fence: Terminal owner, focused guardian tests, zh/en docs.
- Baseline Lock: approved design plus admission and supervised-shell specs.
- Approved Behavior: bounded start/write/read observation; queue and default
  command lifetime remain unbounded.
- Owner Constraints: TerminalTool observes, guardian stores, ShellRuntime owns.
- Compatibility: existing calls valid; read default and resize/stop unchanged.
- Retirement: delete old read-only helper; add no alias.
- Task Batches: source contract, real-PTY tests, docs.
- Test Obligations: six exact tests, touched rustfmt, diff hygiene.
- Review Gates: behavior, architecture invariants, docs parity, staged audit.
- Drift Rule: any wider owner change pauses and returns to the plan/spec.
- Evidence: passing exact commands and architecture review.
- Advisory Boundary: method-pack guidance only; not completion authority.

## Risks and Retirement

- PTY prompt timing varies; deterministic tests use explicit output.
- Cancellation after registration is the only new ownership-sensitive await;
  cleanup stays in the existing runtime/client path.
- Offset errors can duplicate or skip bytes; immediate-read regressions prove
  the boundary.
- The worktree is dirty elsewhere. Never stage unrelated runtime/TUI files.
- No rollback compatibility path is retained; reverting the feature commit
  restores the prior code contract.
