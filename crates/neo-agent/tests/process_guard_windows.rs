#![cfg(windows)]

use std::{path::PathBuf, process::Stdio, time::Duration};

use base64::Engine as _;
use neo_agent_core::{ShellLimits, ShellRuntime, ToolAccess, ToolContext, ToolRegistry};
use serde_json::json;
use tokio::io::AsyncWriteExt as _;

fn guarded_context(workspace: &tempfile::TempDir) -> ToolContext {
    ToolContext::new(workspace.path())
        .expect("tool context")
        .with_access(ToolAccess::all())
        .with_shell_runtime(ShellRuntime::new(
            ShellLimits::default(),
            PathBuf::from(env!("CARGO_BIN_EXE_neo")),
            workspace.path().join("runtime"),
        ))
}

#[tokio::test]
async fn windows_terminal_stop_closes_job_with_descendant() {
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace);
    let registry = ToolRegistry::with_builtin_tools();
    let pid_file = workspace.path().join("stop-child.pid");
    let started = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": descendant_command(&pid_file) }),
        )
        .await
        .expect("terminal start");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle");
    answer_cursor_position_query(&registry, &context, handle).await;
    let descendant = wait_for_terminal_pid(&registry, &context, handle, &pid_file).await;

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop");

    assert!(wait_for_process_exit(descendant).await);
}

#[tokio::test]
async fn windows_terminal_guardian_loss_closes_job_with_descendant() {
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace);
    let registry = ToolRegistry::with_builtin_tools();
    let pid_file = workspace.path().join("guardian-loss-child.pid");
    let started = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": descendant_command(&pid_file) }),
        )
        .await
        .expect("terminal start");
    let details = started.details.as_ref().expect("start details");
    let handle = details["handle"].as_str().expect("handle");
    let guardian_pid = u32::try_from(details["guardian_pid"].as_u64().expect("guardian pid"))
        .expect("guardian pid u32");
    answer_cursor_position_query(&registry, &context, handle).await;
    let descendant = wait_for_terminal_pid(&registry, &context, handle, &pid_file).await;

    kill_process(guardian_pid);
    assert!(wait_for_process_exit(descendant).await);

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("remove terminal handle");
}

#[tokio::test]
async fn windows_terminal_natural_exit_closes_job_with_descendant() {
    let workspace = tempfile::tempdir().expect("workspace");
    let context = guarded_context(&workspace);
    let registry = ToolRegistry::with_builtin_tools();
    let pid_file = workspace.path().join("natural-exit-child.pid");
    let started = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": natural_exit_descendant_command(&pid_file)
            }),
        )
        .await
        .expect("terminal start");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle");
    answer_cursor_position_query(&registry, &context, handle).await;
    let descendant = wait_for_terminal_pid(&registry, &context, handle, &pid_file).await;

    assert!(wait_for_process_exit(descendant).await);

    let stopped = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("remove terminal handle");
    assert_eq!(
        stopped.details.as_ref().expect("stop details")["status"],
        "completed"
    );
}

#[tokio::test]
async fn windows_parent_eof_closes_assigned_job_with_descendant() {
    let workspace = tempfile::tempdir().expect("workspace");
    let pid_file = workspace.path().join("parent-eof-child.pid");
    let mut guardian = tokio::process::Command::new(env!("CARGO_BIN_EXE_neo"))
        .arg("__process-guard")
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn process guard");
    let mut control = guardian.stdin.take().expect("guard stdin");
    let mut stdout = guardian.stdout.take().expect("guard stdout");
    let mut stderr = guardian.stderr.take().expect("guard stderr");
    let stdout_drain =
        tokio::spawn(async move { tokio::io::copy(&mut stdout, &mut tokio::io::sink()).await });
    let stderr_drain =
        tokio::spawn(async move { tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await });

    control
        .write_all(&start_bash_frame(
            workspace.path(),
            "windows-parent-eof",
            &descendant_command(&pid_file),
        ))
        .await
        .expect("write Start frame");
    control.flush().await.expect("flush Start frame");
    let descendant = wait_for_pid(&pid_file)
        .await
        .unwrap_or_else(|| panic!("PID file was not written: {}", pid_file.display()));

    drop(control);
    let status = tokio::time::timeout(Duration::from_secs(10), guardian.wait())
        .await
        .expect("guardian exits after parent EOF")
        .expect("wait guardian");
    assert!(status.success(), "guardian failed: {status:?}");
    assert!(wait_for_process_exit(descendant).await);
    stdout_drain
        .await
        .expect("stdout drain join")
        .expect("stdout drain");
    stderr_drain
        .await
        .expect("stderr drain join")
        .expect("stderr drain");

    let final_status: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace.path().join("windows-parent-eof.status.json"))
            .expect("read final status"),
    )
    .expect("parse final status");
    assert_eq!(final_status["exit"]["status"], "parent_exited");
}

fn descendant_command(pid_file: &std::path::Path) -> String {
    let path = pid_file.display().to_string().replace('\'', "''");
    powershell_command(&format!(
        "Set-Content -LiteralPath '{path}' -Value $PID; Start-Sleep -Seconds 300"
    ))
}

fn natural_exit_descendant_command(pid_file: &std::path::Path) -> String {
    let path = pid_file.display().to_string().replace('\'', "''");
    powershell_command(&format!(
        "$child = Start-Process ping.exe -ArgumentList '-t','127.0.0.1' -PassThru; Set-Content -LiteralPath '{path}' -Value $child.Id"
    ))
}

fn powershell_command(script: &str) -> String {
    let bytes = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("powershell.exe -NoProfile -EncodedCommand {encoded}")
}

fn start_bash_frame(status_dir: &std::path::Path, task_id: &str, command: &str) -> Vec<u8> {
    let payload = serde_json::to_vec(&json!({
        "task_id": task_id,
        "kind": "bash",
        "command": command,
        "limits": {
            "timeout_ms": 30_000,
            "background_timeout_ms": 1_800_000,
            "max_parallelism": 4,
            "max_descendant_processes": 32,
            "max_tree_memory_percent": 25,
            "max_output_bytes": 65_536,
            "max_background_log_bytes": 10_485_760
        },
        "status_dir": status_dir,
        "cols": null,
        "rows": null
    }))
    .expect("serialize Start payload");
    let body_len = 9usize.checked_add(payload.len()).expect("body length");
    let mut frame = Vec::with_capacity(4 + body_len);
    frame.extend_from_slice(
        &u32::try_from(body_len)
            .expect("body length u32")
            .to_be_bytes(),
    );
    frame.push(1);
    frame.extend_from_slice(&1u64.to_be_bytes());
    frame.extend_from_slice(&payload);
    frame
}

async fn wait_for_terminal_pid(
    registry: &ToolRegistry,
    context: &ToolContext,
    handle: &str,
    path: &std::path::Path,
) -> u32 {
    if let Some(pid) = wait_for_pid(path).await {
        return pid;
    }
    let snapshot = registry
        .run(
            "Terminal",
            context,
            json!({ "mode": "read", "handle": handle }),
        )
        .await
        .expect("read terminal diagnostics");
    panic!(
        "PID file was not written: {}\nterminal output:\n{}",
        path.display(),
        snapshot.content
    );
}

async fn answer_cursor_position_query(
    registry: &ToolRegistry,
    context: &ToolContext,
    handle: &str,
) {
    if let Err(error) = registry
        .run(
            "Terminal",
            context,
            json!({ "mode": "write", "handle": handle, "input": "\u{1b}[1;1R" }),
        )
        .await
    {
        let snapshot = registry
            .run(
                "Terminal",
                context,
                json!({ "mode": "read", "handle": handle }),
            )
            .await
            .expect("read failed terminal after cursor query");
        panic!(
            "answer cursor position query: {error}\n{}",
            snapshot.content
        );
    }
}

async fn wait_for_pid(path: &std::path::Path) -> Option<u32> {
    for _ in 0..250 {
        if let Ok(pid) = std::fs::read_to_string(path)
            && let Ok(pid) = pid.trim().parse()
        {
            return Some(pid);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

async fn wait_for_process_exit(pid: u32) -> bool {
    for _ in 0..250 {
        if !process_exists(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    !process_exists(pid)
}

fn process_exists(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .expect("tasklist");
    assert!(
        output.status.success(),
        "tasklist failed with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
}

fn kill_process(pid: u32) {
    let status = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status()
        .expect("taskkill guardian");
    assert!(status.success(), "taskkill guardian {pid}");
}
