use neo_agent_core::{ProcessSupervisor, ToolAccess, ToolContext, ToolError, ToolRegistry};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocked_write_in_one_terminal_does_not_block_other_handles() {
    let workspace = tempfile::tempdir().expect("workspace");
    let supervisor = ProcessSupervisor::default();
    let registry = std::sync::Arc::new(ToolRegistry::with_builtin_tools());
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_process_supervisor(supervisor);

    let blocked = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": "python3 -c 'import sys, time; sys.stdin.read(1); print(\"writer-started\", flush=True); time.sleep(5)'"
            }),
        )
        .await
        .expect("blocked terminal start");
    let blocked_handle = blocked.details.as_ref().expect("blocked details")["handle"]
        .as_str()
        .expect("blocked handle")
        .to_owned();
    let healthy = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "printf healthy-terminal; sleep 1" }),
        )
        .await
        .expect("healthy terminal start");
    let healthy_handle = healthy.details.as_ref().expect("healthy details")["handle"]
        .as_str()
        .expect("healthy handle")
        .to_owned();

    let writer_registry = std::sync::Arc::clone(&registry);
    let writer_context = context.clone();
    let writer_handle = blocked_handle.clone();
    let write = tokio::spawn(async move {
        writer_registry
            .run(
                "Terminal",
                &writer_context,
                json!({
                    "mode": "write",
                    "handle": writer_handle,
                    "input": format!("x\n{}", "x".repeat(16 * 1024 * 1024)),
                }),
            )
            .await
    });
    let blocked_output =
        read_terminal_until(&registry, &context, &blocked_handle, "writer-started").await;
    assert!(blocked_output.contains("writer-started"));

    let healthy_read = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        registry.run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": healthy_handle.clone() }),
        ),
    )
    .await
    .expect("another terminal must remain available")
    .expect("healthy terminal read");
    assert!(healthy_read.content.contains("healthy-terminal"));

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": blocked_handle }),
        )
        .await
        .expect("blocked terminal stop");
    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": healthy_handle }),
        )
        .await
        .expect("healthy terminal stop");
    let _write_result = tokio::time::timeout(std::time::Duration::from_secs(1), write)
        .await
        .expect("blocked write must finish after terminal stop")
        .expect("blocked write task join");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_stop_returns_promptly_for_interactive_shell() {
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
            json!({
                "mode": "start",
                "command": "bash --noprofile --norc"
            }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    let stopped = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        registry.run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        ),
    )
    .await
    .expect("terminal stop should not block the async runtime")
    .expect("terminal stop should succeed");

    assert_eq!(supervisor.active_count().await, 0);
    assert_eq!(
        stopped
            .details
            .as_ref()
            .expect("stop details")
            .get("reader_drained")
            .cloned()
            .and_then(|v| v.as_bool()),
        Some(true),
        "reader should drain promptly under normal stop: {stopped:?}"
    );
}

#[tokio::test]
async fn terminal_read_waits_briefly_for_fresh_running_output() {
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
            json!({
                "mode": "start",
                "command": "sleep 0.02; printf settle-output; sleep 1"
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

    assert!(
        read.content.contains("settle-output"),
        "terminal read should wait briefly for fresh running output: {read:?}"
    );

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop should succeed");
}

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
        "python3 -c '",
        "import sys, time;",
        "sys.stdout.write(\"diff --git a/file b/file\\n\");",
        "sys.stdout.flush();",
        "time.sleep(0.04);",
        "sys.stdout.write(\"Stage this hunk [y,n,q,a,d,j,J,g,/,s,e,p,?]? \");",
        "sys.stdout.flush();",
        "sys.stdin.readline()",
        "'"
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

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_read_quiet_period_does_not_block_other_terminal_operations() {
    let workspace = tempfile::tempdir().expect("workspace");
    let supervisor = ProcessSupervisor::default();
    let registry = std::sync::Arc::new(ToolRegistry::with_builtin_tools());
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_process_supervisor(supervisor.clone());

    let slow = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "sleep 1" }),
        )
        .await
        .expect("slow terminal start should succeed");
    let slow_handle = slow.details.as_ref().expect("slow details")["handle"]
        .as_str()
        .expect("slow handle")
        .to_owned();
    let second = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "printf second-terminal; sleep 1" }),
        )
        .await
        .expect("second terminal start should succeed");
    let second_handle = second.details.as_ref().expect("second details")["handle"]
        .as_str()
        .expect("second handle")
        .to_owned();

    let read_registry = std::sync::Arc::clone(&registry);
    let read_context = context.clone();
    let read_handle = slow_handle.clone();
    let read_task = tokio::spawn(async move {
        read_registry
            .run(
                "Terminal",
                &read_context,
                json!({ "mode": "read", "handle": read_handle }),
            )
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    tokio::time::timeout(
        std::time::Duration::from_millis(500),
        registry.run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": second_handle }),
        ),
    )
    .await
    .expect("stop on another terminal should not wait for quiet-period polling")
    .expect("second terminal stop should succeed");

    read_task
        .await
        .expect("read task join")
        .expect("read succeeds");
    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": slow_handle }),
        )
        .await
        .expect("slow terminal stop should succeed");
}

#[tokio::test]
async fn terminal_write_then_read_observes_interactive_shell_output() {
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
            json!({
                "mode": "start",
                "command": "bash --noprofile --norc",
                "cols": 44,
                "rows": 9
            }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": "printf terminal-event-ok\\n\n"
            }),
        )
        .await
        .expect("terminal write should succeed");

    let read = read_terminal_until(&registry, &context, &handle, "terminal-event-ok").await;
    assert!(
        read.contains("terminal-event-ok"),
        "terminal read should include interactive shell output: {read:?}"
    );

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop should succeed");
}

#[tokio::test]
async fn terminal_tool_start_write_read_resize_and_stop_uses_real_pty() {
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
            json!({
                "mode": "start",
                "command": "bash --noprofile --norc",
                "cols": 40,
                "rows": 8
            }),
        )
        .await
        .expect("terminal start should succeed");
    let details = started.details.as_ref().expect("start details");
    let handle = details["handle"].as_str().expect("terminal handle");
    assert_eq!(details["status"], "running");
    assert_eq!(details["cols"], 40);
    assert_eq!(details["rows"], 8);

    registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": "printf 'pty:%s\\n' \"$COLUMNS:$LINES\"\n"
            }),
        )
        .await
        .expect("terminal write should succeed");

    let read = read_terminal_until(&registry, &context, handle, "pty:40:8").await;
    assert!(
        read.contains("pty:40:8"),
        "read output should include PTY-sized command output: {read:?}"
    );

    let resized = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "resize", "handle": handle, "cols": 72, "rows": 18 }),
        )
        .await
        .expect("terminal resize should succeed");
    assert_eq!(
        resized.details.as_ref().expect("resize details")["cols"],
        72
    );
    assert_eq!(
        resized.details.as_ref().expect("resize details")["rows"],
        18
    );

    registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": "stty size\n"
            }),
        )
        .await
        .expect("terminal write after resize should succeed");
    let read = read_terminal_until(&registry, &context, handle, "18 72").await;
    assert!(
        read.contains("18 72"),
        "read output should include resized PTY dimensions: {read:?}"
    );

    let stopped = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop should succeed");
    assert_eq!(
        stopped.details.as_ref().expect("stop details")["status"],
        "cancelled"
    );
    assert_eq!(supervisor.active_count().await, 0);

    let missing = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": handle }),
        )
        .await
        .expect_err("stopped terminal handle should be removed");
    assert!(matches!(missing, ToolError::InvalidInput { .. }));
}

#[tokio::test]
async fn process_supervisor_cleanup_stops_terminal_handles() {
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
            json!({ "mode": "start", "command": "bash --noprofile --norc" }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();
    assert_eq!(supervisor.active_count().await, 1);

    supervisor.cleanup_all().await;
    assert_eq!(supervisor.active_count().await, 0);

    let missing = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": handle }),
        )
        .await
        .expect_err("supervisor cleanup should remove terminal handle");
    assert!(matches!(missing, ToolError::InvalidInput { .. }));
}

#[tokio::test]
async fn terminal_read_details_do_not_leak_output_past_max_output_bytes() {
    let workspace = tempfile::tempdir().expect("workspace");
    let supervisor = ProcessSupervisor::default();
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all())
        .with_process_supervisor(supervisor);

    let started = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "printf 'keep-terminal-leak-tail'; sleep 1" }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    let result = read_terminal_result_until_truncated_or_leaked(
        &registry,
        &context,
        &handle,
        "leak-tail",
        4,
    )
    .await;
    let serialized = serde_json::to_string(&result).expect("result serializes");

    assert!(result.content.contains("truncated: true"));
    assert!(!result.content.contains("terminal-leak-tail"));
    assert!(!serialized.contains("terminal-leak-tail"));
    let details = result.details.expect("read details");
    let output = details["output"].as_str().expect("details output");
    assert!(
        output.len() <= 4,
        "details output should be capped: {output:?}"
    );
    assert_eq!(details["output_truncated"], true);

    let stopped = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle, "max_output_bytes": 4 }),
        )
        .await
        .expect("terminal stop should succeed");
    let stopped = serde_json::to_string(&stopped).expect("stop result serializes");
    assert!(!stopped.contains("terminal-leak-tail"));
}

#[tokio::test]
async fn terminal_rejects_missing_mode() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let error = registry
        .run("Terminal", &context, json!({}))
        .await
        .expect_err("terminal should require mode");

    assert!(matches!(error, ToolError::InvalidInput { .. }));
}

#[tokio::test]
async fn terminal_rejects_unknown_handle() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::all());

    let error = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": "no-such-handle" }),
        )
        .await
        .expect_err("terminal read should reject unknown handle");

    assert!(matches!(error, ToolError::InvalidInput { .. }));
}

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
        details["read_offset_after"].as_u64().expect("after offset")
            >= "prompt-visible".len() as u64
    );
    assert!(
        details["total_output_bytes"].as_u64().expect("total bytes")
            >= "prompt-visible".len() as u64
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_stop_cleans_interactive_git_add_patch_and_index_lock() {
    let workspace = tempfile::tempdir().expect("workspace");
    run_git(workspace.path(), &["init"]);
    run_git(
        workspace.path(),
        &["config", "user.email", "neo@example.invalid"],
    );
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
    assert!(
        read.contains("Stage this hunk"),
        "git prompt should be observable: {read:?}"
    );

    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        registry.run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        ),
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

async fn read_terminal_until(
    registry: &ToolRegistry,
    context: &ToolContext,
    handle: &str,
    needle: &str,
) -> String {
    let mut latest = String::new();
    for _ in 0..50 {
        let read = registry
            .run(
                "Terminal",
                context,
                json!({ "mode": "read", "handle": handle, "max_output_bytes": 4096 }),
            )
            .await
            .expect("terminal read should succeed");
        latest.push_str(
            read.details
                .as_ref()
                .and_then(|details| details["output"].as_str())
                .unwrap_or_default(),
        );
        if latest.contains(needle) {
            return latest;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    latest
}

async fn read_terminal_result_until_truncated_or_leaked(
    registry: &ToolRegistry,
    context: &ToolContext,
    handle: &str,
    leak_tail: &str,
    max_output_bytes: usize,
) -> neo_agent_core::ToolResult {
    let mut latest = None;
    for _ in 0..50 {
        let read = registry
            .run(
                "Terminal",
                context,
                json!({ "mode": "read", "handle": handle, "max_output_bytes": max_output_bytes }),
            )
            .await
            .expect("terminal read should succeed");
        let serialized = serde_json::to_string(&read).expect("result serializes");
        let truncated = read
            .details
            .as_ref()
            .and_then(|details| details["output_truncated"].as_bool())
            .unwrap_or(false);
        if truncated || serialized.contains(leak_tail) {
            return read;
        }
        latest = Some(read);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    latest.expect("terminal read should have been attempted")
}
