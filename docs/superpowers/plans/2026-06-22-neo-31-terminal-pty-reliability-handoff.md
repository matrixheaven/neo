# Neo Terminal PTY Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Neo's `Terminal` PTY tool reliable for interactive programs such as `git add -p`: prompts must be observable, read state must be diagnosable, and stopped terminals must not leave child processes or lock files behind.

**Architecture:** Keep `Terminal` as a real PTY tool in `neo-agent-core`, but make its read contract stateful and observable. Replace the current "return immediately when any fresh byte exists" read wait with a quiet-period settle loop, add structured read diagnostics, stream PTY output updates through the existing `ToolUpdateCallback`, and harden stop/cleanup around process-group-like behavior and handle removal. Tests must reproduce the Minimax M3 report before implementation: prompt text not surfaced, black-box state after writes, and stale process/lock cleanup.

**Tech Stack:** Rust 2024, `portable-pty`, `tokio`, `neo-agent-core` tool registry, `ProcessSupervisor`, `nextest` via `cargo run -p xtask -- test`, real-process PTY tests in `crates/neo-agent-core/tests/tool_terminal.rs`.

---

## Source Report

The user attached a Minimax M3 usage report from a Neo session. The relevant Terminal findings:

- `Terminal` was chosen because `git add -p` needs a PTY.
- `Terminal.read` did not reliably surface the interactive prompt: `Stage this hunk [y,n,q,a,d,j,J,g,/,s,e,p,?]?`.
- The agent had to blindly send `y` because it could not tell which hunk was active.
- Terminal state was opaque: no current hunk/prompt/read offset/last command visibility.
- A first attempt left a stale `git add -p` process and `.git/index.lock`, requiring manual cleanup.

This plan treats these as product bugs in `Terminal`, not as an agent-behavior issue.

## Mandatory References

Read these before coding:

- `AGENTS.md`
- `~/.codex/RTK.md`
- `~/.codex/CX.md`
- `crates/neo-agent-core/src/tools/terminal.rs`
- `crates/neo-agent-core/tests/tool_terminal.rs`
- `crates/neo-agent-core/src/tools/bash.rs`
- `crates/neo-agent-core/src/tools/process_supervisor.rs`
- `crates/neo-agent-core/src/runtime.rs`
- `crates/neo-agent-core/tests/runtime_turn.rs`
- `.config/nextest.toml`
- `docs/tools.md`

Run the required recall before implementation:

```bash
rtk icm recall-context "Neo Terminal PTY git add -p prompt read stale process index.lock" --limit 5
```

## Non-Negotiable Project Rules

- Use `rtk` for shell commands.
- Prefer `cx` for symbol navigation before broad reads.
- Do not run bare `cargo test`; use `rtk cargo run -p xtask -- test ...`.
- Do not perform git mutations unless the user gives explicit per-command authorization. This includes `git add`, `git commit`, `git push`, `git switch`, `git checkout`, `git reset`, `git stash`, `git clean`, `git rm`, `git merge`, and `git rebase`.
- Tests may create and mutate temporary Git repositories under `tempfile::TempDir`; they must never run git mutations against the real Neo checkout.
- PTY/real-process tests must be deterministic and must not depend on a fixed port, shared home, ambient git config, or test execution order.
- If a meaningful error is resolved, store it before final response:

```bash
rtk icm store -t errors-resolved -c "Fixed Neo Terminal PTY reliability issue: interactive prompts are observable, read state is diagnosable, and stop/cleanup no longer leaves stale interactive git processes or index locks." -i high -k "neo,Terminal,PTY,git-add-p,index-lock"
```

## File Structure

- Modify `crates/neo-agent-core/src/tools/terminal.rs`
  - Owns `TerminalInput`, `TerminalMode`, PTY session storage, read/write/resize/stop behavior, and Terminal result details.
- Modify `crates/neo-agent-core/tests/tool_terminal.rs`
  - Add direct Terminal tool regressions for prompt visibility, quiet-period reads, state diagnostics, and cleanup.
- Modify `crates/neo-agent-core/tests/runtime_turn.rs`
  - Add one runtime-level regression proving Terminal tool updates expose prompt-like PTY output in tool cards/session events.
- Modify `.config/nextest.toml`
  - Add a real-process/PTY group only if new tests require serialization or longer slow timeout.
- Modify `docs/tools.md`
  - Document the updated `Terminal.read` details and recommended `git add -p` caveats.

## Task 1: Reproduce Prompt Visibility Failure

**Files:**
- Modify: `crates/neo-agent-core/tests/tool_terminal.rs`

- [ ] **Step 1: Add a failing prompt-settle regression**

Add this test near `terminal_read_waits_briefly_for_fresh_running_output`:

```rust
#[tokio::test]
async fn terminal_read_waits_for_prompt_after_initial_output_burst() {
    let workspace = tempfile::tempdir().expect("workspace");
    let supervisor = ProcessSupervisor::default();
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_process_supervisor(supervisor.clone());

    let script = concat!(
        "python3 - <<'PY'\n",
        "import sys, time\n",
        "sys.stdout.write('diff --git a/file b/file\\n')\n",
        "sys.stdout.flush()\n",
        "time.sleep(0.04)\n",
        "sys.stdout.write('Stage this hunk [y,n,q,a,d,j,J,g,/,s,e,p,?]? ')\n",
        "sys.stdout.flush()\n",
        "sys.stdin.readline()\n",
        "PY"
    );
    let started = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": script,
                "cols": 100,
                "rows": 24
            }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    let read = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": handle, "max_output_bytes": 4096 }),
        )
        .await
        .expect("terminal read should succeed");
    let output = read.details.as_ref().expect("read details")["output"]
        .as_str()
        .expect("details output");

    assert!(
        output.contains("Stage this hunk [y,n,q,a,d,j,J,g,/,s,e,p,?]?"),
        "Terminal.read must wait for the prompt, not return after the first diff bytes: {read:?}"
    );

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "write", "handle": handle, "input": "q\n" }),
        )
        .await
        .expect("terminal write should succeed");
    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop should succeed");
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core terminal_read_waits_for_prompt_after_initial_output_burst
```

Expected before implementation: FAIL because the current read path returns as soon as any fresh output exists and can miss the delayed prompt.

## Task 2: Implement Quiet-Period Terminal Reads

**Files:**
- Modify: `crates/neo-agent-core/src/tools/terminal.rs`

- [ ] **Step 1: Replace the read wait constants**

Replace:

```rust
const TERMINAL_READ_SETTLE_TIMEOUT: Duration = Duration::from_millis(100);
const TERMINAL_READ_SETTLE_INTERVAL: Duration = Duration::from_millis(10);
```

with:

```rust
const TERMINAL_READ_MAX_WAIT: Duration = Duration::from_millis(250);
const TERMINAL_READ_QUIET_PERIOD: Duration = Duration::from_millis(50);
const TERMINAL_READ_POLL_INTERVAL: Duration = Duration::from_millis(10);
```

- [ ] **Step 2: Replace `wait_for_fresh_output`**

Replace the existing `wait_for_fresh_output` and `has_fresh_output` functions with:

```rust
async fn wait_for_output_quiet_period(output: Arc<StdMutex<Vec<u8>>>, read_offset: usize) {
    let deadline = Instant::now() + TERMINAL_READ_MAX_WAIT;
    let mut last_len = output_len(&output);
    let mut last_change = Instant::now();

    while Instant::now() < deadline {
        sleep(TERMINAL_READ_POLL_INTERVAL).await;
        let current_len = output_len(&output);
        if current_len != last_len {
            last_len = current_len;
            last_change = Instant::now();
            continue;
        }
        if current_len > read_offset && last_change.elapsed() >= TERMINAL_READ_QUIET_PERIOD {
            break;
        }
    }
}

fn output_len(output: &StdMutex<Vec<u8>>) -> usize {
    output.lock().expect("terminal output lock poisoned").len()
}
```

- [ ] **Step 3: Update `read_terminal` to use the quiet-period wait**

Change:

```rust
if status.is_none() {
    wait_for_fresh_output(Arc::clone(&session.output), session.read_offset).await;
}
```

to:

```rust
if status.is_none() {
    wait_for_output_quiet_period(Arc::clone(&session.output), session.read_offset).await;
}
```

- [ ] **Step 4: Run the prompt regression**

Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core terminal_read_waits_for_prompt_after_initial_output_burst
```

Expected after implementation: PASS.

## Task 3: Add Read-State Diagnostics

**Files:**
- Modify: `crates/neo-agent-core/src/tools/terminal.rs`
- Modify: `crates/neo-agent-core/tests/tool_terminal.rs`

- [ ] **Step 1: Add a failing details-shape test**

Add this test:

```rust
#[tokio::test]
async fn terminal_read_details_expose_state_for_interactive_debugging() {
    let workspace = tempfile::tempdir().expect("workspace");
    let supervisor = ProcessSupervisor::default();
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_process_supervisor(supervisor.clone());

    let started = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "printf prompt-visible; sleep 1" }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    let read = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": handle, "max_output_bytes": 4096 }),
        )
        .await
        .expect("terminal read should succeed");
    let details = read.details.as_ref().expect("read details");

    assert_eq!(details["read_offset_before"], 0);
    assert!(
        details["read_offset_after"].as_u64().expect("after offset") >= "prompt-visible".len() as u64
    );
    assert!(
        details["total_output_bytes"].as_u64().expect("total bytes") >= "prompt-visible".len() as u64
    );
    assert_eq!(details["unread_bytes_after"], 0);
    assert_eq!(details["cols"], 80);
    assert_eq!(details["rows"], 24);

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop should succeed");
}
```

- [ ] **Step 2: Run the details-shape test**

Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core terminal_read_details_expose_state_for_interactive_debugging
```

Expected before implementation: FAIL because the detail fields do not exist.

- [ ] **Step 3: Add fields to `read_terminal`**

Inside `read_terminal`, capture `read_offset_before`, `read_offset_after`, `total_output_bytes`, and `unread_bytes_after` while holding the output lock:

```rust
let read_offset_before = session.read_offset;
let (output, read_offset_after, total_output_bytes, unread_bytes_after) = {
    let output = session
        .output
        .lock()
        .expect("terminal output lock poisoned");
    let output_slice = output
        .get(read_offset_before..)
        .ok_or_else(|| unknown_terminal(tool, handle))?;
    let total_output_bytes = output.len();
    session.read_offset = total_output_bytes;
    (
        String::from_utf8_lossy(output_slice).into_owned(),
        session.read_offset,
        total_output_bytes,
        0_usize,
    )
};
```

Then extend `with_details(json!({ ... }))` with:

```rust
"read_offset_before": read_offset_before,
"read_offset_after": read_offset_after,
"total_output_bytes": total_output_bytes,
"unread_bytes_after": unread_bytes_after,
"cols": session.cols,
"rows": session.rows,
```

- [ ] **Step 4: Run the details-shape test**

Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core terminal_read_details_expose_state_for_interactive_debugging
```

Expected after implementation: PASS.

## Task 4: Stream Terminal Output Updates

**Files:**
- Modify: `crates/neo-agent-core/src/tools/terminal.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

- [ ] **Step 1: Keep callback ownership out of `TerminalSession`**

Do not add a callback field to `TerminalSession`. The callback is already an
optional `Arc<ToolUpdateCallback>` on `ToolContext`; clone it in `start_terminal`
and move it directly into the reader thread. This keeps session state focused on
PTY process handles and read offsets, and avoids another mutable callback path.

- [ ] **Step 2: Change `spawn_reader_thread` to accept callback and cap**

Change the function signature to:

```rust
fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    output: Arc<StdMutex<Vec<u8>>>,
    callback: Option<ToolUpdateCallback>,
    stream_max_bytes: usize,
) -> ThreadJoinHandle<()> {
```

Inside the thread, after appending to `output`, stream capped chunks:

```rust
let mut streamed = 0_usize;
std::thread::spawn(move || {
    let mut local = [0_u8; 8192];
    loop {
        match reader.read(&mut local) {
            Ok(0) | Err(_) => break,
            Ok(bytes_read) => {
                let chunk = &local[..bytes_read];
                output
                    .lock()
                    .expect("terminal output lock poisoned")
                    .extend_from_slice(chunk);
                if streamed < stream_max_bytes {
                    let remaining = stream_max_bytes - streamed;
                    let streamed_chunk = &chunk[..chunk.len().min(remaining)];
                    streamed += streamed_chunk.len();
                    if let Some(callback) = &callback {
                        callback(&String::from_utf8_lossy(streamed_chunk));
                    }
                }
            }
        }
    }
})
```

- [ ] **Step 3: Pass the callback from `start_terminal`**

Change:

```rust
let reader_thread = spawn_reader_thread(reader, Arc::clone(&output));
```

to:

```rust
let reader_thread = spawn_reader_thread(
    reader,
    Arc::clone(&output),
    ctx.tool_update.clone(),
    ctx.max_output_bytes,
);
```

- [ ] **Step 4: Add a runtime regression**

Add a runtime-level test near `runtime_emits_terminal_lifecycle_events_for_terminal_tool`:

```rust
#[tokio::test]
async fn runtime_streams_terminal_prompt_updates_before_read() {
    let prompt = "Stage this hunk [y,n,q,a,d,j,J,g,/,s,e,p,?]?";
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "Terminal".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            arguments: json!({
                "mode": "start",
                "command": format!(
                    "python3 - <<'PY'\nimport sys, time\nsys.stdout.write('{prompt} ')\nsys.stdout.flush()\ntime.sleep(1)\nPY"
                ),
                "cols": 100,
                "rows": 24
            }),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("open terminal prompt"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionUpdate {
            id,
            name,
            partial_result,
            ..
        } if id == "tool_1" && name == "Terminal" && partial_result.content.contains(prompt)
    )));
}
```

- [ ] **Step 5: Run the runtime regression**

Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core runtime_streams_terminal_prompt_updates_before_read
```

Expected after implementation: PASS.

## Task 5: Harden Stop/Cleanup For Interactive Git

**Files:**
- Modify: `crates/neo-agent-core/tests/tool_terminal.rs`
- Modify: `crates/neo-agent-core/src/tools/terminal.rs`

- [ ] **Step 1: Add a temp-repo `git add -p` cleanup regression**

Add this helper to `tool_terminal.rs`:

```rust
fn run_git(workspace: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(workspace)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
```

Then add:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_stop_cleans_interactive_git_add_patch_and_index_lock() {
    let workspace = tempfile::tempdir().expect("workspace");
    run_git(workspace.path(), &["init"]);
    run_git(workspace.path(), &["config", "user.email", "neo@example.invalid"]);
    run_git(workspace.path(), &["config", "user.name", "Neo Test"]);
    std::fs::write(workspace.path().join("tracked.txt"), "one\n").expect("write tracked");
    run_git(workspace.path(), &["add", "tracked.txt"]);
    run_git(workspace.path(), &["commit", "-m", "initial"]);
    std::fs::write(workspace.path().join("tracked.txt"), "one\nsecond\n").expect("edit tracked");

    let supervisor = ProcessSupervisor::default();
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_process_supervisor(supervisor.clone());

    let started = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "git add -p tracked.txt", "cols": 100, "rows": 24 }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    let read = read_terminal_until(&registry, &context, &handle, "Stage this hunk").await;
    assert!(read.contains("Stage this hunk"), "git prompt should be observable: {read:?}");

    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        registry.run("Terminal", &context, json!({ "mode": "stop", "handle": handle })),
    )
    .await
    .expect("terminal stop should not hang")
    .expect("terminal stop should succeed");

    assert_eq!(supervisor.active_count().await, 0);
    assert!(
        !workspace.path().join(".git/index.lock").exists(),
        "Terminal stop must not leave .git/index.lock behind"
    );
}
```

- [ ] **Step 2: Run the cleanup regression**

Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core terminal_stop_cleans_interactive_git_add_patch_and_index_lock
```

Expected before hardening may fail by timeout or by leaving `.git/index.lock`.

- [ ] **Step 3: Harden `stop_session`**

If the test fails, implement the smallest proven fix:

- Drop writer and master before killing.
- Call `child.kill()`.
- Call `child.wait()` in a blocking task.
- Join the reader thread.
- Keep handle removal before blocking stop so subsequent cleanup cannot double-stop.
- If `portable_pty::Child::kill` does not kill the interactive child tree on macOS, add a Unix-only process group for Terminal child startup. Keep it local to Terminal; do not refactor Bash process groups in this task.

- [ ] **Step 4: Run cleanup and existing terminal tests**

Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core --test tool_terminal
```

Expected: all `tool_terminal` tests PASS.

## Task 6: Update Documentation

**Files:**
- Modify: `docs/tools.md`

- [ ] **Step 1: Update the Terminal row/details**

In the `Terminal` section, document that:

- `read` waits for a short quiet period so prompt text that arrives after initial output is included.
- `read.details` contains `read_offset_before`, `read_offset_after`, `total_output_bytes`, `unread_bytes_after`, `cols`, and `rows`.
- Terminal output is also streamed as tool updates while the PTY is running.
- For one-shot non-interactive commands, `Bash` remains preferred.
- For `git add -p`, agents should read until the prompt is visible before writing `y/n/q`.

- [ ] **Step 2: Run docs parity only if docs tooling requires it**

For this doc-only step, run:

```bash
rtk cargo run -p xtask -- parity
```

Expected: PASS, unless unrelated docs parity failures already exist in the dirty worktree. If unrelated failures appear, do not fix them in this task; report them.

## Task 7: Verification Gate

**Files:**
- No new files.

- [ ] **Step 1: Format check**

Run:

```bash
rtk cargo fmt --all --check
```

Expected: PASS.

- [ ] **Step 2: Focused Terminal tests**

Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core --test tool_terminal
```

Expected: PASS.

- [ ] **Step 3: Runtime Terminal tests**

Run:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core terminal
```

Expected: PASS for Terminal-related tests. If this filter picks up unrelated tests, record that in the final handoff.

- [ ] **Step 4: ICM memory**

Run:

```bash
rtk icm store -t context-neo -c "Completed Neo Terminal PTY reliability work: read waits for prompt quiet period, Terminal details expose read state, PTY output streams to tool updates, and interactive git add -p cleanup is covered by tests." -i high -k "neo,Terminal,PTY,git-add-p,tool-updates"
```

## Self-Review Checklist

Run this checklist before final response:

- [ ] Spec coverage: the plan/test changes cover prompt visibility, state observability, and stale cleanup from the Minimax M3 report.
- [ ] Placeholder scan: no task says "TODO", "TBD", "add tests", or "handle edge cases" without exact code or commands.
- [ ] Type consistency: `ToolUpdateCallback`, `ToolContext`, `ToolRegistry`, `ToolResult`, and `TerminalSession` names match current code.
- [ ] Test scope: verification uses `xtask` and stays within `neo-agent-core` Terminal/runtime tests.
- [ ] Safety: any git commands are inside temp repositories created by tests, not the Neo checkout.
- [ ] Git policy: no plan step performs `git add`, `git commit`, `git reset`, `git restore`, `git stash`, or other repository mutations without explicit user authorization.

## Execution Handoff

Plan complete. Recommended execution mode:

1. **Subagent-Driven (recommended):** one fresh subagent for Task 1-2, one for Task 3-4, one for Task 5-7, with main-agent review between tasks.
2. **Inline Execution:** execute tasks in this session using `superpowers:executing-plans`, stopping after each failing test turns green.
