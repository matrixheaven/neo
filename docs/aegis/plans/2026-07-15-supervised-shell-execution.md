# Supervised Shell Execution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use aegis:subagent-driven-development (recommended) or aegis:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace every direct Bash and Terminal spawn with a crash-resistant guardian that owns the complete process tree, enforces bounded runtime/resources/output, and cannot outlive Neo.

**Architecture:** `neo-agent-core` owns one private framed protocol, guardian runtime, bounded output types, and lightweight Bash/Terminal clients. The `neo-agent` binary exposes only a hidden `__process-guard` entry point and maps `[runtime.shell]` into a shared `ShellRuntime`. Each guardian owns one process group or Windows Job Object and its PTY when applicable; Neo owns only the control pipe and client handle.

**Tech Stack:** Rust 2024, Tokio processes/I/O, `portable-pty`, `rustix`, `win32job`, `sysinfo`, Serde JSON metadata, existing session atomic-file helper.

## Global Constraints

- Rust edition 2024, minimum Rust 1.96.1, `unsafe_code = "forbid"`.
- Windows, Linux, and macOS are required; platform code stays behind `cfg(unix)` / `cfg(windows)`.
- One execution path only: delete direct Bash/Terminal spawning and do not add fallback or compatibility branches.
- Defaults are exactly: foreground 600s, background/Terminal 1800s, 2 active commands, parallelism 4, 64 aggregate descendants, 50% aggregate RSS, 65,536 retained bytes, 10,485,760 background-log bytes.
- Per-guardian hard allowances are integer-divided by `max_active_commands`; unused capacity is not borrowed.
- Set `CARGO_BUILD_JOBS`, `NEXTEST_TEST_THREADS`, and `RAYON_NUM_THREADS` only when absent.
- Every child stream is drained in 8 KiB chunks after retention is full; termination is graceful request/TERM, 500 ms, then Job close/KILL.
- Tool inputs may lower but never raise configured timeout/output ceilings.
- No broad Cargo tests. Every test command names one package, one target selector, and one test filter.
- Existing unrelated dirty-worktree changes must be preserved.
- Do not execute git add/commit/branch/worktree/push or any other git mutation without a new explicit authorization. Plan commit steps are intentionally omitted because project policy overrides the generic writing-plans template.

---

### Task 1: Shell Limits, Admission, and Bounded Output

**Files:**
- Create: `crates/neo-agent-core/src/tools/shell_guard/mod.rs`
- Create: `crates/neo-agent-core/src/tools/shell_guard/output.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Test: `crates/neo-agent-core/src/tools/shell_guard/mod.rs`
- Test: `crates/neo-agent-core/src/tools/shell_guard/output.rs`

**Interfaces:**
- Produces: `ShellLimits`, `ShellRuntime`, `ShellCommandPermit`, `StreamKind`, `TaggedHeadTailBuffer`.
- `ShellRuntime::try_acquire()` is the sole admission gate later used by Bash and Terminal.

- [ ] **Step 1: Write failing limit and admission tests**

```rust
#[test]
fn limits_allocate_static_forest_budget() {
    let limits = ShellLimits::default();
    assert_eq!(limits.per_command_descendants(), 32);
    assert_eq!(limits.per_command_memory_percent(), 25);
}

#[test]
fn third_command_is_rejected_without_queueing() {
    let runtime = ShellRuntime::for_tests(ShellLimits::default());
    let first = runtime.try_acquire().unwrap();
    let second = runtime.try_acquire().unwrap();
    assert_eq!(runtime.try_acquire().unwrap_err(), ResourceLimitCause::ActiveCommands);
    drop(first);
    assert!(runtime.try_acquire().is_ok());
    drop(second);
}
```

- [ ] **Step 2: Run the exact failing tests**

Run: `cargo nextest run -p neo-agent-core --lib limits_allocate_static_forest_budget`

Expected: FAIL because `ShellLimits` does not exist.

Run: `cargo nextest run -p neo-agent-core --lib third_command_is_rejected_without_queueing`

Expected: FAIL because `ShellRuntime` does not exist.

- [ ] **Step 3: Implement the minimum typed limits and atomic permit**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellLimits {
    pub foreground_timeout_secs: u64,
    pub background_timeout_secs: u64,
    pub max_active_commands: usize,
    pub max_parallelism: usize,
    pub max_descendant_processes: usize,
    pub max_tree_memory_percent: u8,
    pub max_output_bytes: usize,
    pub max_background_log_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ShellRuntime {
    limits: ShellLimits,
    active: Arc<AtomicUsize>,
    guardian_executable: Arc<PathBuf>,
}

pub struct ShellCommandPermit {
    active: Arc<AtomicUsize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceLimitCause {
    ActiveCommands,
    ProcessCount,
    TreeMemory,
    SamplerUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimitDetail {
    pub cause: ResourceLimitCause,
    pub configured: Option<u64>,
    pub observed: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardLimits {
    pub timeout_ms: u64,
    pub max_parallelism: usize,
    pub max_descendant_processes: usize,
    pub max_tree_memory_percent: u8,
    pub max_output_bytes: usize,
    pub max_background_log_bytes: u64,
}
```

Use a compare-exchange loop in `try_acquire`; `Drop` decrements exactly once. `ShellLimits::validate` rejects zero values, memory percentages above 100, and static divisions that produce zero. Add `clamp_foreground_timeout`, `clamp_output_bytes`, `per_command_descendants`, and `per_command_memory_percent`.

- [ ] **Step 4: Write the failing shared head/tail test**

```rust
#[test]
fn stdout_and_stderr_share_one_head_tail_budget() {
    let mut buffer = TaggedHeadTailBuffer::new(8);
    buffer.push(StreamKind::Stdout, b"abcd");
    buffer.push(StreamKind::Stderr, b"EFGH");
    buffer.push(StreamKind::Stdout, b"ijkl");
    let output = buffer.finish();
    assert_eq!(output.retained_bytes(), 8);
    assert_eq!(output.omitted_bytes, 4);
    assert_eq!(output.stdout, b"abcdijkl");
    assert!(output.stderr.is_empty());
}
```

- [ ] **Step 5: Run RED, implement one tagged 50/50 head-tail buffer, then run GREEN**

Run RED: `cargo nextest run -p neo-agent-core --lib stdout_and_stderr_share_one_head_tail_budget`

Implementation requirements: store tagged segments, reserve `capacity / 2` for the head and the remainder for the tail, never grow past capacity, and materialize stdout/stderr without losing each stream's byte order.

Run GREEN:

```bash
cargo nextest run -p neo-agent-core --lib shell_guard
```

Expected: all `shell_guard` unit tests PASS.

### Task 2: Private Framed Protocol and Final Status

**Files:**
- Create: `crates/neo-agent-core/src/tools/shell_guard/protocol.rs`
- Create: `crates/neo-agent-core/src/tools/shell_guard/status.rs`
- Modify: `crates/neo-agent-core/src/session/atomic_file.rs`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/mod.rs`
- Test: `crates/neo-agent-core/src/tools/shell_guard/protocol.rs`
- Test: `crates/neo-agent-core/src/tools/shell_guard/status.rs`

**Interfaces:**
- Produces: `GuardRequest`, `GuardResponse`, `StartRequest`, `GuardExit`, `GuardStatus`, `read_frame`, `write_frame`, `write_final_status`.
- Frame header is exactly `u32 body_len | u8 kind | u64 request_id`, big-endian.

- [ ] **Step 1: Write codec RED tests**

```rust
#[tokio::test]
async fn codec_round_trips_raw_terminal_bytes_and_request_id() {
    let request = GuardRequest::Write { request_id: 7, data: vec![0, 0xff, b'\n'] };
    let bytes = encode_for_test(&request).unwrap();
    assert_eq!(decode_request_for_test(&bytes).unwrap(), request);
}

#[test]
fn codec_rejects_oversized_and_unknown_frames() {
    assert!(matches!(decode_request_for_test(&oversized_frame()), Err(ProtocolError::FrameTooLarge { .. })));
    assert!(matches!(decode_request_for_test(&unknown_frame()), Err(ProtocolError::UnknownKind(_))));
}
```

- [ ] **Step 2: Run RED**

Run: `cargo nextest run -p neo-agent-core --lib codec_round_trips_raw_terminal_bytes_and_request_id`

Expected: FAIL because the codec is absent.

- [ ] **Step 3: Implement the protocol without JSONL output**

```rust
pub const MAX_FRAME_BODY: usize = 1024 * 1024;
pub const MAX_TERMINAL_WRITE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardTaskKind { Bash, Terminal }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartRequest {
    pub task_id: String,
    pub kind: GuardTaskKind,
    pub command: String,
    pub limits: GuardLimits,
    pub status_dir: PathBuf,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardStatusKind {
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    ResourceLimited,
    ParentExited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardExit {
    pub status: GuardStatusKind,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub resource_limit: Option<ResourceLimitDetail>,
    pub omitted_output_bytes: u64,
    pub omitted_log_bytes: u64,
}

pub enum GuardRequest {
    Start(StartRequest),
    Write { request_id: u64, data: Vec<u8> },
    Read { request_id: u64, offset: u64, max_bytes: usize },
    Resize { request_id: u64, cols: u16, rows: u16 },
    SetBackgroundDeadline { request_id: u64 },
    Stop { request_id: u64 },
}

pub enum GuardResponse {
    Started { guardian_pid: u32, command_pid: u32, command_start_id: u64 },
    Output { stream: StreamKind, data: Vec<u8> },
    Ack { request_id: u64 },
    Busy { request_id: u64 },
    Snapshot { request_id: u64, offset: u64, total: u64, discarded: u64, data: Vec<u8> },
    Exited(GuardExit),
    Error { request_id: u64, message: String },
}
```

Use Serde JSON only for typed metadata payloads. Encode raw Output/Write/Snapshot bytes directly. Reject truncated frames and split logical Snapshot/Exited payloads across ordered frames with a final flag.

- [ ] **Step 4: Write status create-once RED test and implement it**

```rust
#[test]
fn final_status_is_atomic_and_create_once() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("task.status.json");
    write_final_status(&path, &GuardStatus::parent_exited_for_test()).unwrap();
    assert!(matches!(
        write_final_status(&path, &GuardStatus::completed_for_test()),
        Err(StatusWriteError::AlreadyExists(_))
    ));
}
```

Extend `crate::session::atomic_file` with a create-once variant that uses the existing safe-directory and temporary-file convention but refuses an existing destination; do not implement a second temp-file convention.

- [ ] **Step 5: Verify Task 2**

Run: `cargo nextest run -p neo-agent-core --lib shell_guard`

Expected: protocol/status/buffer tests PASS.

### Task 3: Configuration and Shared Runtime Wiring

**Files:**
- Modify: `crates/neo-agent/src/config/types.rs`
- Modify: `crates/neo-agent/src/config/mod.rs`
- Modify: `crates/neo-agent/src/config/loader.rs`
- Modify: `crates/neo-agent/src/config/mutations.rs`
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
- Modify: `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Modify: `crates/neo-agent/src/modes/run/runtime/agent.rs`
- Test: `crates/neo-agent/src/config/mod.rs`

**Interfaces:**
- Consumes: `ShellLimits`, `ShellRuntime` from Task 1.
- Produces: one process-level `AppConfig.shell_runtime`, cloned into parent/child `AgentConfig` values and copied into each `ToolContext`.

- [ ] **Step 1: Write failing TOML default/override/validation tests**

```rust
#[test]
fn runtime_shell_defaults_and_overrides_are_loaded() {
    let parsed: FileConfig = toml::from_str("[runtime.shell]\nmax_active_commands = 1\n").unwrap();
    let runtime = runtime_from_file_for_tests(parsed.runtime);
    assert_eq!(runtime.shell.max_active_commands, 1);
    assert_eq!(runtime.shell.background_timeout_secs, 1800);
}

#[test]
fn runtime_shell_rejects_zero_static_allowance() {
    let mut limits = ShellLimits::default();
    limits.max_active_commands = 51;
    assert_eq!(limits.validate().unwrap_err().key(), "runtime.shell.max_tree_memory_percent");
}
```

- [ ] **Step 2: Run RED**

Run: `cargo nextest run -p neo-agent --bin neo runtime_shell_defaults_and_overrides_are_loaded`

Expected: FAIL because `[runtime.shell]` is unknown/ignored.

- [ ] **Step 3: Add the file-config overlay and runtime field**

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct FileRuntimeShellConfig {
    pub(crate) foreground_timeout_secs: Option<u64>,
    pub(crate) background_timeout_secs: Option<u64>,
    pub(crate) max_active_commands: Option<usize>,
    pub(crate) max_parallelism: Option<usize>,
    pub(crate) max_descendant_processes: Option<usize>,
    pub(crate) max_tree_memory_percent: Option<u8>,
    pub(crate) max_output_bytes: Option<usize>,
    pub(crate) max_background_log_bytes: Option<u64>,
}
```

Overlay each optional field onto `ShellLimits::default()` and call the typed validator from `validate_runtime_config`.

- [ ] **Step 4: Wire one shared runtime through AgentConfig and ToolContext**

```rust
#[serde(skip)]
#[schemars(skip)]
pub shell_runtime: ShellRuntime,
```

`AppConfig::load` constructs `ShellRuntime` once with `std::env::current_exe()`, the loaded limits, and the no-session runtime root. `agent_config_for_app` clones that value; `default_tool_context` clones it into `ToolContext`. Child AgentConfig clones retain the same admission counter and Terminal handle registry. Config mutation/reload paths preserve the existing runtime rather than creating a second gate.

- [ ] **Step 5: Verify Task 3**

Run: `cargo nextest run -p neo-agent --bin neo runtime_shell_`

Expected: default, override, and validation tests PASS.

### Task 4: Bash Guardian Runtime and Hidden Entry Point

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/neo-agent-core/Cargo.toml`
- Create: `crates/neo-agent-core/src/tools/shell_guard/guardian.rs`
- Create: `crates/neo-agent-core/src/tools/shell_guard/process_tree.rs`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/mod.rs`
- Modify: `crates/neo-agent/src/cli.rs`
- Modify: `crates/neo-agent/src/main.rs`
- Create: `crates/neo-agent/tests/process_guard.rs`

**Interfaces:**
- Consumes: protocol/status/limits from Tasks 1-2.
- Produces: `run_process_guard(stdin, stdout)`, `GuardedProcessTree`, hidden `Command::ProcessGuard`.

- [ ] **Step 1: Add the cross-platform process sampler dependency**

```toml
# workspace dependencies
sysinfo = "0.37.2"

# neo-agent-core dependencies
sysinfo.workspace = true
```

Also enable Tokio's `io-std` feature for async inherited stdin/stdout. Use sysinfo only for process identity, parent/descendant enumeration, RSS, and physical memory. Keep signal/Job operations in existing rustix/win32job code.

- [ ] **Step 2: Write the parent-EOF process test first**

```rust
#[tokio::test]
#[cfg(unix)]
async fn process_guard_parent_eof_kills_bash_descendant() {
    let fixture = GuardFixture::spawn_bash("sleep 30 & echo $! > child.pid; wait").await;
    let child_pid = fixture.wait_for_pid_file().await;
    drop(fixture.control_stdin);
    fixture.assert_status("parent_exited").await;
    assert!(!process_exists(child_pid));
}
```

- [ ] **Step 3: Run RED**

Run: `cargo nextest run -p neo-agent --test process_guard process_guard_parent_eof_kills_bash_descendant`

Expected: FAIL because the hidden guard command is absent.

- [ ] **Step 4: Implement hidden dispatch before normal config/TUI startup**

```rust
#[command(name = "__process-guard", hide = true)]
ProcessGuard,
```

After `Cli::parse_from`, match this command before tracing and `AppConfig::load`; call the core guardian with inherited stdin/stdout and return. Guardian diagnostics must not reach TUI stderr.

- [ ] **Step 5: Implement one Bash guardian event loop**

The loop must establish cwd, close/non-inherit ownership handles, apply missing parallelism environment variables, spawn the existing resolved shell into an isolated PGID or non-breakaway Job, drain 8 KiB stdout/stderr chunks, sample every 250 ms, select on control EOF/deadline/root exit, and converge all terminal causes through one cleanup function. Unix uses TERM/500 ms/KILL; Windows owns the only kill-on-close Job handle.

```rust
tokio::select! {
    control = read_request(&mut stdin) => handle_control(control?),
    status = child.wait() => finish_after_root_exit(status?),
    _ = deadline_timer => terminate(GuardStatusKind::TimedOut),
    _ = sample_tick.tick() => enforce_tree_limits(&mut sampler)?,
}
```

- [ ] **Step 6: Add focused fault tests and verify**

Add a focused environment test that starts the guard with one of the three variables already set and the other two absent; assert the explicit value is preserved and both absent values become `max_parallelism`.

Run: `cargo nextest run -p neo-agent --test process_guard process_guard_parent_eof_kills_bash_descendant`

Run: `cargo nextest run -p neo-agent --test process_guard process_guard_deadline_kills_tree_without_polling`

Run: `cargo nextest run -p neo-agent --test process_guard process_guard_descendant_limit_returns_resource_limited`

Expected: all three PASS; each asserts final status and dead descendants.

### Task 5: Replace Bash and Background Direct Spawning

**Files:**
- Create: `crates/neo-agent-core/src/tools/shell_guard/client.rs`
- Modify: `crates/neo-agent-core/src/tools/bash.rs`
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Modify: `crates/neo-agent-core/src/messages.rs`
- Create: `crates/neo-agent/tests/tool_bash_guardian.rs`
- Modify: `crates/neo-agent-core/tests/tool_bash.rs`
- Modify: `crates/neo-agent-core/tests/shell_runner.rs`

**Interfaces:**
- Consumes: `ShellRuntime::try_acquire`, guardian protocol, tagged output buffer.
- Produces: `GuardianClient`, manager-owned `ManagedBackgroundCommand`, `ShellCommandOutcome::ResourceLimited`.

- [ ] **Step 1: Write RED tests for shared admission and guardian-owned background deadline**

```rust
#[tokio::test]
async fn bash_and_terminal_share_the_active_command_limit() {
    let ctx = context_with_shell_limits(ShellLimits { max_active_commands: 1, ..ShellLimits::default() });
    let task = start_background_bash(&ctx, long_running_command()).await;
    let error = start_second_bash(&ctx).await.unwrap_err();
    assert_resource_limited(error, "active_commands");
    stop_task(&ctx, task).await;
}

#[tokio::test]
async fn background_timeout_finishes_without_task_output_polling() {
    let ctx = context_with_background_timeout(Duration::from_millis(100));
    let task_id = start_background_bash(&ctx, long_running_command()).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(ctx.background_tasks.snapshot(&task_id).await.unwrap().status, BackgroundTaskStatus::TimedOut);
}
```

- [ ] **Step 2: Run RED**

Run: `cargo nextest run -p neo-agent --test tool_bash_guardian background_timeout_finishes_without_task_output_polling`

Expected: FAIL because timeout still depends on the old manager path.

- [ ] **Step 3: Implement the lightweight guardian client**

`GuardianClient::start` acquires the permit before spawning, sends Start, waits for Started, and owns the permit until Exited. One reader task demultiplexes responses, updates the shared tagged output, invokes bounded live callbacks, and completes the final-status future. Closing/dropping the client closes stdin; it never detaches the guardian.

- [ ] **Step 4: Adapt BackgroundTaskManager instead of duplicating it**

Replace `ManagedBackgroundCommand`'s separate unbounded stdout/stderr buffers and direct-child closures with:

```rust
pub struct ManagedBackgroundCommand {
    pub output: Arc<Mutex<TaggedHeadTailBuffer>>,
    pub final_status: watch::Receiver<Option<GuardExit>>,
    pub stop: Arc<dyn Fn() -> BoxFuture<'static, GuardExit> + Send + Sync>,
    pub set_background_deadline: Arc<dyn Fn() -> BoxFuture<'static, Result<(), ToolError>> + Send + Sync>,
}
```

The manager observes final status but does not implement a deadline. Background logs are written by the guardian and capped at `max_background_log_bytes`.

- [ ] **Step 5: Delete the Bash direct-spawn path**

Remove `ManagedChild`, `spawn_bash_process_at`, taskkill helpers, direct pipe readers, and manager-owned timeout/kill logic. Both `execute_shell_command` and model Bash calls construct a `StartBash` request through `GuardianClient`.

- [ ] **Step 6: Verify Bash behavior**

Move guardian-dependent Bash tests from `neo-agent-core` to `neo-agent/tests/tool_bash_guardian.rs` and inject `env!("CARGO_BIN_EXE_neo")` into `ShellRuntime`. Retain only schema/pure formatting tests in core; do not add a fixture executable or in-process production launcher.

Run: `cargo nextest run -p neo-agent --test tool_bash_guardian background_timeout_finishes_without_task_output_polling`

Run: `cargo nextest run -p neo-agent --test tool_bash_guardian bash_foreground_cancellation_kills_descendant_process_group`

Run: `cargo nextest run -p neo-agent --test tool_bash_guardian user_shell_runner_registers_foreground_task_for_detach`

Expected: PASS; TaskOutput was never called in the timeout test.

### Task 6: Move Complete PTY Ownership into the Guardian

**Files:**
- Modify: `crates/neo-agent-core/src/tools/shell_guard/guardian.rs`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/client.rs`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/process_tree.rs`
- Modify: `crates/neo-agent-core/src/tools/terminal.rs`
- Delete: `crates/neo-agent-core/src/tools/terminal_process.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Create: `crates/neo-agent/tests/tool_terminal_guardian.rs`
- Modify: `crates/neo-agent-core/tests/tool_terminal.rs`

**Interfaces:**
- Consumes: `GuardRequest::{Write,Read,Resize,Stop}` and `GuardianClient`.
- Produces: guardian-owned PTY session and Neo-side `TerminalClientSession` handles only.

- [ ] **Step 1: Adapt the existing real-PTY test to assert guardian ownership**

Extend `terminal_tool_start_write_read_resize_and_stop_uses_real_pty` so details expose a guardian PID distinct from the command PID, then close the client control channel and assert both command and descendant exit.

- [ ] **Step 2: Run RED**

Run: `cargo nextest run -p neo-agent --test tool_terminal_guardian terminal_tool_start_write_read_resize_and_stop_uses_real_pty`

Expected: FAIL because Neo still owns `TerminalSession` and `portable_pty::MasterPty`.

- [ ] **Step 3: Move existing PTY internals, do not rewrite them**

Move `TerminalProcessTree`, Unix PGID cleanup, Windows Job creation, `WindowsLaunchBarrier`, UTF-8 decoder, output ring, reader thread, and PTY master/writer into the guardian module. Preserve the Windows rule that Started is sent only after Job assignment and barrier release.

- [ ] **Step 4: Make TerminalTool a protocol client**

The process-level `ShellRuntime` terminal registry retains only:

```rust
struct TerminalClientSession {
    client: GuardianClient,
    read_offset: u64,
    cols: u16,
    rows: u16,
}
```

Write uses non-blocking one-slot guardian enqueue and ordered retry after Ack; Stop bypasses it. Read preserves the existing 3-second maximum wait and 50 ms quiet period using metadata-only Snapshot requests, then requests capped bytes. Resize sends only a protocol request.

- [ ] **Step 5: Delete obsolete in-process PTY ownership and verify concurrency**

Remove the old `TerminalSession`, master/writer/reader ownership, direct Drop cleanup, and `terminal_process` module.

Move guardian-dependent Terminal tests from `neo-agent-core` to `neo-agent/tests/tool_terminal_guardian.rs`, using `env!("CARGO_BIN_EXE_neo")`. Keep only pure input/decoder/formatting tests in core.

Run: `cargo nextest run -p neo-agent --test tool_terminal_guardian blocked_write_in_one_terminal_does_not_block_other_handles`

Run: `cargo nextest run -p neo-agent --test tool_terminal_guardian terminal_stop_terminates_descendant_processes`

Run: `cargo nextest run -p neo-agent --test tool_terminal_guardian terminal_read_details_do_not_leak_output_past_max_output_bytes`

Expected: all PASS through guardian IPC.

### Task 7: Persist Terminal Causes and Finish TUI Outcomes

**Files:**
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
- Modify: `crates/neo-agent-core/src/messages.rs`
- Modify: `crates/neo-tui/src/transcript/shell_run.rs`
- Modify: `crates/neo-agent/src/modes/interactive/shell_command.rs`
- Modify: `crates/neo-agent-core/tests/tool_bash.rs`
- Modify: `crates/neo-tui/src/transcript/shell_run.rs`

**Interfaces:**
- Consumes: create-once `GuardStatus`, `ShellCommandOutcome::ResourceLimited`.
- Produces: persisted `ResourceLimited` / `ParentExited` background states and finalized TUI cards.

- [ ] **Step 1: Write RED rendering and recovery tests**

```rust
#[test]
fn resource_limited_shell_outcome_finalizes_with_measured_limit() {
    let lines = finished_plain_lines("", "", None, None, &ShellCommandOutcome::ResourceLimited, false);
    assert_eq!(lines, ["Resource limit exceeded."]);
}

#[tokio::test]
async fn resume_converges_stale_running_guard_without_claiming_status_file() {
    let fixture = stale_guard_record_fixture();
    let snapshot = restore_shell_task(&fixture.session, fixture.task_id).await.unwrap();
    assert_eq!(snapshot.status, BackgroundTaskStatus::ParentExited);
    assert!(!fixture.final_status_path.exists());
}
```

- [ ] **Step 2: Run RED**

Run: `cargo nextest run -p neo-tui --lib transcript::shell_run::tests::resource_limited_shell_outcome_finalizes_with_measured_limit`

Run: `cargo nextest run -p neo-agent-core --test tool_bash resume_converges_stale_running_guard_without_claiming_status_file`

Expected: FAIL because both variants/recovery mapping are absent.

- [ ] **Step 3: Add terminal states without compatibility aliases**

Add `ResourceLimited` to `ShellCommandOutcome`; add `ResourceLimited` and `ParentExited` to `BackgroundTaskStatus`. Carry the typed cause plus observed/configured values in ToolResult details and persisted status. Update every exhaustive match; do not map these states to generic Failed.

- [ ] **Step 4: Implement resume settlement**

When a session task has `running.json` but no `status.json`, poll for at most three seconds. If the final file appears, use it. Otherwise return a synthetic ParentExited snapshot without writing the guardian-owned final path and without signaling recorded PIDs.

For no-session runs, use `<NEO_HOME>/runtime/<instance-id>/agents/<agent-id>/tasks/` and remove only instance directories whose tasks all have terminal final statuses. Never scavenge a live running record.

- [ ] **Step 5: Finalize transcript rendering**

Render ResourceLimited as a muted terminal message including cause/observed/limit when details exist. All new outcomes must make `ShellRunComponent::finalization()` return Finalized so the spinner stops.

- [ ] **Step 6: Verify Task 7**

Run: `cargo nextest run -p neo-tui --lib transcript::shell_run::tests::resource_limited_shell_outcome_finalizes_with_measured_limit`

Run: `cargo nextest run -p neo-agent-core --test tool_bash resume_converges_stale_running_guard_without_claiming_status_file`

Expected: PASS.

### Task 8: Fault Matrix and Final Narrow Verification

**Files:**
- Modify: `crates/neo-agent/tests/process_guard.rs`
- Modify: `crates/neo-agent/tests/tool_bash_guardian.rs`
- Modify: `crates/neo-agent/tests/tool_terminal_guardian.rs`
- Modify: `crates/neo-tui/tests/shell_events.rs`

**Interfaces:**
- Verifies the complete contract; produces no new production abstraction.

- [ ] **Step 1: Add only missing high-risk fault cases**

Add exact tests for guardian loss while Neo remains alive, root leader exit with a surviving background descendant, sampler-unavailable fail-closed mapping through a deterministic injected snapshot provider, background log truncation while output drains to EOF, and ownership-pipe handles not appearing in command descendants. Reuse existing PID helpers and PTY fixtures.

- [ ] **Step 2: Run each exact boundary test**

```bash
cargo nextest run -p neo-agent --test process_guard process_guard_parent_eof_kills_bash_descendant
cargo nextest run -p neo-agent --test process_guard process_guard_root_exit_kills_remaining_descendant
cargo nextest run -p neo-agent --test tool_bash_guardian background_timeout_finishes_without_task_output_polling
cargo nextest run -p neo-agent --test tool_terminal_guardian terminal_tool_start_write_read_resize_and_stop_uses_real_pty
cargo nextest run -p neo-tui --lib transcript::shell_run::tests::resource_limited_shell_outcome_finalizes_with_measured_limit
```

Expected: every command reports the named test PASS with zero failures.

- [ ] **Step 3: Run target-specific lint/build checks**

```bash
cargo fmt --all --check
cargo clippy -p neo-agent-core --lib -- -D clippy::all
cargo clippy -p neo-agent --bin neo -- -D clippy::all
cargo clippy -p neo-tui --lib -- -D clippy::all
cargo build -p neo-agent
```

Expected: exit 0 for each command. If a pre-existing dirty file causes a failure, report it and do not edit or revert that file.

- [ ] **Step 4: Inspect deletion and scope**

Run `git diff --check`, `git diff --stat`, and `git status --short`. Confirm no direct Bash/Terminal spawn remains, no old timeout watcher remains, no fallback path was introduced, and unrelated dirty files are unchanged.

- [ ] **Step 5: Request independent code review**

Use `aegis:requesting-code-review` with the design spec, this plan, the unchanged starting HEAD SHA, and a review package containing only the scoped working-tree diff plus every new untracked file. Label the reviewed head `WORKTREE`; a SHA range alone would be empty because git mutation is not authorized. Fix every Critical/Important finding, rerun its covering exact test, then request one re-review.
