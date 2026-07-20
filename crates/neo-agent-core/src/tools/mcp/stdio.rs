//! Stdio MCP client builder.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
use process_wrap::tokio::{CommandWrap, KillOnDrop};
use rmcp::{ServiceExt, transport::TokioChildProcess};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    task::JoinHandle,
};

use super::{
    McpError,
    client::{BoundedByteTail, McpClient, SharedStderrTail},
};
use crate::tools::ProcessSupervisor;

#[derive(Debug, Clone)]
pub struct StdioConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
}

/// Configure a `tokio::process::Command` for an MCP stdio server.
///
/// Stderr is configured on `TokioChildProcessBuilder`, because the builder
/// overwrites the command's stdio settings during `spawn()`.
pub(crate) fn build_command(config: &StdioConfig) -> CommandWrap {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    if let Some(cwd) = &config.cwd {
        cmd.current_dir(cwd);
    }
    let mut cmd = CommandWrap::from(cmd);
    cmd.wrap(KillOnDrop);
    #[cfg(unix)]
    cmd.wrap(ProcessGroup::leader());
    #[cfg(windows)]
    cmd.wrap(JobObject);
    cmd
}

pub async fn build_stdio_client(
    server_id: &str,
    attempt_id: u64,
    config: StdioConfig,
    supervisor: &ProcessSupervisor,
) -> Result<Arc<dyn McpClient>, McpError> {
    let cmd = build_command(&config);

    // Pipe stderr so MCP server log output never leaks onto the terminal.
    // Must use the builder's .stderr() method — spawn() overwrites any stderr
    // already set on the Command with the builder's value.
    let (transport, stderr_opt) = TokioChildProcess::builder(cmd)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| McpError::protocol(format!("failed to spawn stdio MCP server: {e}")))?;

    // Drain raw stderr bytes so the child cannot block on a full pipe. The
    // bounded tail is retained for diagnostics but never inherited by the TUI.
    let stderr_tail: SharedStderrTail = Arc::new(Mutex::new(BoundedByteTail::default()));
    let mut stderr_drain = stderr_opt.map(|stderr| {
        let tail = Arc::clone(&stderr_tail);
        tokio::spawn(async move { drain_stderr(stderr, tail).await })
    });

    let request_timeout = config.tool_timeout_ms.map(Duration::from_millis);
    let startup_timeout = Duration::from_millis(config.startup_timeout_ms.unwrap_or(5_000));

    let service = match tokio::time::timeout(startup_timeout, ().serve(transport)).await {
        Ok(Ok(service)) => service,
        Ok(Err(error)) => {
            return Err(
                stdio_startup_error(error.to_string(), &mut stderr_drain, &stderr_tail).await,
            );
        }
        Err(_) => {
            return Err(stdio_startup_error(
                format!("timeout connecting to MCP server {server_id}"),
                &mut stderr_drain,
                &stderr_tail,
            )
            .await);
        }
    };

    let client: Arc<dyn McpClient> = Arc::new(super::client::RmcpClient::new(
        service,
        request_timeout,
        Some(stderr_tail),
    ));

    let handle = process_handle(server_id, attempt_id);
    let cleanup_server_id = server_id.to_owned();
    let client_for_cleanup = Arc::clone(&client);
    supervisor
        .register(handle, move |handle| {
            let client = Arc::clone(&client_for_cleanup);
            let server_id = cleanup_server_id.clone();
            Box::pin(async move {
                if let Err(error) = client.shutdown().await {
                    tracing::warn!(
                        %server_id,
                        attempt_id,
                        %handle,
                        error = %error.message(),
                        "failed to shut down supervised stdio MCP client"
                    );
                }
            })
        })
        .await;

    Ok(client)
}

async fn stdio_startup_error(
    message: String,
    stderr_drain: &mut Option<JoinHandle<()>>,
    stderr_tail: &SharedStderrTail,
) -> McpError {
    if let Some(mut drain) = stderr_drain.take()
        && tokio::time::timeout(Duration::from_millis(250), &mut drain)
            .await
            .is_err()
    {
        drain.abort();
    }
    McpError::protocol(message).with_stderr_tail(Some(
        stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot(),
    ))
}

pub(crate) fn process_handle(server_id: &str, attempt_id: u64) -> String {
    format!("mcp_stdio_{server_id}_{attempt_id}")
}

async fn drain_stderr<R>(stderr: R, tail: SharedStderrTail)
where
    R: AsyncRead + Unpin,
{
    let mut stderr = stderr;
    let mut buffer = [0_u8; 8192];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => tail
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(&buffer[..read]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt as _;
    use tokio::time::{Duration, timeout};

    #[test]
    fn stdio_command_owns_process_tree() {
        let command = build_command(&StdioConfig {
            command: "mcp-server".to_owned(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            startup_timeout_ms: None,
            tool_timeout_ms: None,
        });

        assert!(command.has_wrap::<KillOnDrop>());
        #[cfg(unix)]
        assert!(command.has_wrap::<ProcessGroup>());
        #[cfg(windows)]
        assert!(command.has_wrap::<JobObject>());
    }

    #[test]
    fn failing_stdio_server_writes_stderr() {
        if std::env::var_os("NEO_STDIO_STDERR_HELPER").is_some() {
            use std::io::Write as _;

            let mut stdout = std::io::stdout().lock();
            stdout.write_all(b"not-json\n").unwrap();
            stdout.flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));

            let mut stderr = std::io::stderr().lock();
            stderr.write_all(&[b'x'; 10_000]).unwrap();
            stderr.flush().unwrap();
        }
    }

    #[test]
    fn hanging_stdio_server_never_answers() {
        if std::env::var_os("NEO_STDIO_HANG_HELPER").is_some() {
            loop {
                std::thread::sleep(std::time::Duration::from_mins(1));
            }
        }
    }

    #[tokio::test]
    async fn stdio_handshake_honors_startup_timeout() {
        let config = StdioConfig {
            command: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            args: vec![
                "--exact".to_owned(),
                "tools::mcp::stdio::tests::hanging_stdio_server_never_answers".to_owned(),
                "--nocapture".to_owned(),
            ],
            env: BTreeMap::from([("NEO_STDIO_HANG_HELPER".to_owned(), "1".to_owned())]),
            cwd: None,
            startup_timeout_ms: Some(25),
            tool_timeout_ms: None,
        };

        let result = timeout(
            Duration::from_secs(2),
            build_stdio_client("slow", 1, config, &ProcessSupervisor::default()),
        )
        .await
        .expect("stdio startup timeout must resolve before watchdog");
        let Err(error) = result else {
            panic!("hanging handshake must fail")
        };

        assert!(
            error
                .message()
                .contains("timeout connecting to MCP server slow"),
            "{}",
            error.message()
        );
    }

    #[tokio::test]
    async fn failed_stdio_handshake_exposes_bounded_stderr_tail() {
        let config = StdioConfig {
            command: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            args: vec![
                "--exact".to_owned(),
                "tools::mcp::stdio::tests::failing_stdio_server_writes_stderr".to_owned(),
                "--nocapture".to_owned(),
            ],
            env: BTreeMap::from([("NEO_STDIO_STDERR_HELPER".to_owned(), "1".to_owned())]),
            cwd: None,
            startup_timeout_ms: None,
            tool_timeout_ms: None,
        };

        let Err(error) =
            build_stdio_client("broken", 1, config, &ProcessSupervisor::default()).await
        else {
            panic!("helper exits without completing MCP handshake");
        };
        let tail = error.stderr_tail().expect("stderr tail");
        assert_eq!(tail.len(), super::super::client::MCP_STDERR_TAIL_CAPACITY);
        assert!(tail.ends_with(b"x"));
    }

    #[tokio::test]
    async fn drain_stderr_exits_after_eof() {
        let (mut writer, stderr) = tokio::io::duplex(64);
        writer.write_all(b"diagnostic\n").await.unwrap();
        drop(writer);

        let finished = timeout(
            Duration::from_millis(100),
            drain_stderr(stderr, Arc::new(Mutex::new(BoundedByteTail::default()))),
        )
        .await;

        assert!(finished.is_ok(), "stderr drain should stop at EOF");
    }

    #[tokio::test]
    async fn drain_stderr_ignores_non_utf8_without_line_buffering() {
        let (mut writer, stderr) = tokio::io::duplex(64);
        writer.write_all(b"\xffunterminated").await.unwrap();
        drop(writer);

        let tail = Arc::new(Mutex::new(BoundedByteTail::default()));

        let finished = timeout(
            Duration::from_millis(100),
            drain_stderr(stderr, Arc::clone(&tail)),
        )
        .await;

        assert!(
            finished.is_ok(),
            "stderr drain should treat stderr as raw bytes"
        );
        assert_eq!(
            tail.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .snapshot(),
            b"\xffunterminated"
        );
    }
}
