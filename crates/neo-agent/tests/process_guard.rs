#![cfg(unix)]

use std::{process::Stdio, time::Duration};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn process_guard_parent_eof_kills_bash_descendant() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_neo"))
        .arg("__process-guard")
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn process guard");

    let mut stdin = child.stdin.take().expect("guard stdin");
    let mut stdout = child.stdout.take().expect("guard stdout");
    let mut stderr = child.stderr.take().expect("guard stderr");
    let stdout_drain =
        tokio::spawn(async move { tokio::io::copy(&mut stdout, &mut tokio::io::sink()).await });
    let stderr_drain =
        tokio::spawn(async move { tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await });

    stdin
        .write_all(&start_bash_frame(
            workspace.path(),
            "guard-test",
            "sleep 30 & echo $! > child.pid; wait",
            30_000,
            32,
            10_485_760,
        ))
        .await
        .expect("write Start frame");
    stdin.flush().await.expect("flush Start frame");

    let descendant_pid = wait_for_pid_file(&workspace.path().join("child.pid")).await;
    drop(stdin);

    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("guard should exit after parent EOF")
        .expect("wait for guard");
    assert!(status.success(), "guard failed: {status:?}");
    assert!(wait_for_process_exit(&descendant_pid).await);

    let final_status: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace.path().join("guard-test.status.json")).expect("read final status"),
    )
    .expect("parse final status");
    assert_eq!(final_status["exit"]["status"], "parent_exited");

    stdout_drain
        .await
        .expect("join stdout drain")
        .expect("drain stdout");
    stderr_drain
        .await
        .expect("join stderr drain")
        .expect("drain stderr");
}

#[tokio::test]
async fn process_guard_descendant_limit_returns_resource_limited() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_neo"))
        .arg("__process-guard")
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn process guard");
    let mut stdin = child.stdin.take().expect("guard stdin");
    let mut stdout = child.stdout.take().expect("guard stdout");
    stdin
        .write_all(&start_bash_frame(
            workspace.path(),
            "guard-limit",
            "sleep 30 & echo $! > child.pid; sleep 30 & wait",
            30_000,
            1,
            10_485_760,
        ))
        .await
        .expect("write Start frame");
    stdin.flush().await.expect("flush Start frame");

    wait_for_started_frame(&mut stdout).await;
    let descendant_pid = wait_for_pid_file(&workspace.path().join("child.pid")).await;
    let wait = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    if wait.is_err() {
        drop(stdin);
    }
    let status = wait
        .expect("descendant watchdog should stop the guard")
        .expect("wait for guard");
    assert!(status.success(), "guard failed: {status:?}");
    assert!(wait_for_process_exit(&descendant_pid).await);

    let final_status: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace.path().join("guard-limit.status.json"))
            .expect("read final status"),
    )
    .expect("parse final status");
    assert_eq!(final_status["exit"]["status"], "resource_limited");
    assert_eq!(
        final_status["exit"]["resource_limit"]["cause"],
        "process_count"
    );
}

#[tokio::test]
async fn process_guard_deadline_kills_tree_without_polling() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_neo"))
        .arg("__process-guard")
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn process guard");
    let mut stdin = child.stdin.take().expect("guard stdin");
    let mut stdout = child.stdout.take().expect("guard stdout");
    stdin
        .write_all(&start_bash_frame(
            workspace.path(),
            "guard-deadline",
            "sleep 30 & echo $! > child.pid; wait",
            100,
            32,
            10_485_760,
        ))
        .await
        .expect("write Start frame");
    stdin.flush().await.expect("flush Start frame");

    wait_for_started_frame(&mut stdout).await;
    let descendant_pid = wait_for_pid_file(&workspace.path().join("child.pid")).await;
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("deadline should stop the guard without polling")
        .expect("wait for guard");
    assert!(status.success(), "guard failed: {status:?}");
    assert!(wait_for_process_exit(&descendant_pid).await);

    let final_status: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace.path().join("guard-deadline.status.json"))
            .expect("read final status"),
    )
    .expect("parse final status");
    assert_eq!(final_status["exit"]["status"], "timed_out");

    drop(stdin);
}

#[tokio::test]
async fn process_guard_preserves_existing_parallelism_limit_and_fills_missing_ones() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_neo"))
        .arg("__process-guard")
        .current_dir(workspace.path())
        .env("CARGO_BUILD_JOBS", "7")
        .env_remove("NEXTEST_TEST_THREADS")
        .env_remove("RAYON_NUM_THREADS")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn process guard");
    let mut stdin = child.stdin.take().expect("guard stdin");
    stdin
        .write_all(&start_bash_frame(
            workspace.path(),
            "guard-env",
            "printf '%s|%s|%s' \"$CARGO_BUILD_JOBS\" \"$NEXTEST_TEST_THREADS\" \"$RAYON_NUM_THREADS\" > env.txt",
            5_000,
            32,
            10_485_760,
        ))
        .await
        .expect("write Start frame");
    stdin.flush().await.expect("flush Start frame");

    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("completed command should stop the guard")
        .expect("wait for guard");
    assert!(status.success(), "guard failed: {status:?}");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("env.txt")).expect("read environment"),
        "7|4|4"
    );

    drop(stdin);
}

#[tokio::test]
async fn process_guard_caps_background_log_without_blocking_output_drain() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_neo"))
        .arg("__process-guard")
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn process guard");
    let mut stdin = child.stdin.take().expect("guard stdin");
    stdin
        .write_all(&start_bash_frame(
            workspace.path(),
            "guard-log",
            "yes x | head -c 131072",
            5_000,
            32,
            16,
        ))
        .await
        .expect("write Start frame");
    stdin.flush().await.expect("flush Start frame");

    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("log truncation must not block guardian completion")
        .expect("wait for guard");
    assert!(status.success(), "guard failed: {status:?}");
    assert_eq!(
        std::fs::metadata(workspace.path().join("guard-log.log"))
            .expect("log metadata")
            .len(),
        16
    );
    let final_status: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace.path().join("guard-log.status.json")).expect("read final status"),
    )
    .expect("parse final status");
    assert_eq!(final_status["exit"]["status"], "completed");
    assert!(
        final_status["exit"]["omitted_log_bytes"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );

    drop(stdin);
}

#[tokio::test]
async fn process_guard_root_exit_kills_remaining_descendant() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_neo"))
        .arg("__process-guard")
        .current_dir(workspace.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn process guard");
    let mut stdin = child.stdin.take().expect("guard stdin");
    stdin
        .write_all(&start_bash_frame(
            workspace.path(),
            "guard-root-exit",
            "sleep 30 & echo $! > child.pid",
            5_000,
            32,
            10_485_760,
        ))
        .await
        .expect("write Start frame");
    stdin.flush().await.expect("flush Start frame");

    let descendant_pid = wait_for_pid_file(&workspace.path().join("child.pid")).await;
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("root exit cleanup must settle")
        .expect("wait for guard");
    assert!(status.success(), "guard failed: {status:?}");
    assert!(wait_for_process_exit(&descendant_pid).await);
    let final_status: serde_json::Value = serde_json::from_slice(
        &std::fs::read(workspace.path().join("guard-root-exit.status.json"))
            .expect("read final status"),
    )
    .expect("parse final status");
    assert_eq!(final_status["exit"]["status"], "completed");

    drop(stdin);
}

fn start_bash_frame(
    status_dir: &std::path::Path,
    task_id: &str,
    command: &str,
    timeout_ms: u64,
    max_descendant_processes: usize,
    max_background_log_bytes: u64,
) -> Vec<u8> {
    let payload = serde_json::to_vec(&json!({
        "task_id": task_id,
        "kind": "bash",
        "command": command,
        "limits": {
            "timeout_ms": timeout_ms,
            "background_timeout_ms": 1_800_000,
            "max_command_parallelism": 4,
            "max_command_descendant_processes": max_descendant_processes,
            "max_command_memory_percent": 25,
            "max_output_bytes": 65_536,
            "max_background_log_bytes": max_background_log_bytes
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

async fn wait_for_started_frame(stdout: &mut tokio::process::ChildStdout) {
    let mut frame = [0; 29];
    tokio::time::timeout(Duration::from_secs(5), stdout.read_exact(&mut frame))
        .await
        .expect("Started frame timeout")
        .expect("read Started frame");
    assert_eq!(u32::from_be_bytes(frame[..4].try_into().unwrap()), 25);
    assert_eq!(frame[4], 101);
    assert_eq!(u64::from_be_bytes(frame[5..13].try_into().unwrap()), 1);
}

async fn wait_for_pid_file(path: &std::path::Path) -> String {
    for _ in 0..100 {
        if let Ok(pid) = std::fs::read_to_string(path) {
            let pid = pid.trim();
            if !pid.is_empty() {
                return pid.to_owned();
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("pid file should be written: {}", path.display());
}

async fn wait_for_process_exit(pid: &str) -> bool {
    for _ in 0..100 {
        if !process_exists(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    !process_exists(pid)
}

fn process_exists(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
