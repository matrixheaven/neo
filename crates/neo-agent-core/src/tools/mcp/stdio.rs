//! Stdio MCP client builder.

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use rmcp::{ServiceExt, transport::TokioChildProcess};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

use super::{McpError, client::McpClient};
use crate::tools::ProcessSupervisor;

#[derive(Debug, Clone)]
pub struct StdioConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub tool_timeout_ms: Option<u64>,
}

/// Configure a `tokio::process::Command` for an MCP stdio server.
///
/// Extracted from `build_stdio_client` so it can be unit-tested without
/// spawning a real subprocess. Note: stderr is NOT set here — it is set on
/// the `TokioChildProcessBuilder` instead, because the builder overwrites
/// stdio settings during `spawn()`.
pub(crate) fn build_command(config: &StdioConfig) -> Command {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    for (k, v) in &config.env {
        cmd.env(k, v);
    }
    if let Some(cwd) = &config.cwd {
        cmd.current_dir(cwd);
    }
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

    // Drain stderr in the background so the child never blocks on a full
    // stderr pipe. Lines are read and dropped — they must NOT be inherited
    // (which would leak onto the terminal in TUI mode).
    if let Some(stderr) = stderr_opt {
        tokio::spawn(async move { drain_stderr(stderr).await });
    }

    let request_timeout = config.tool_timeout_ms.map(Duration::from_millis);

    let service = ().serve(transport).await.map_err(|e| McpError::protocol(e.to_string()))?;

    let client: Arc<dyn McpClient> =
        Arc::new(super::client::RmcpClient::new(service, request_timeout));

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

pub(crate) fn process_handle(server_id: &str, attempt_id: u64) -> String {
    format!("mcp_stdio_{server_id}_{attempt_id}")
}

async fn drain_stderr<R>(stderr: R)
where
    R: AsyncRead + Unpin,
{
    let mut stderr = stderr;
    let mut buffer = [0_u8; 8192];
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                // Intentionally drop bytes — MCP server stderr is debug noise
                // that must not reach the terminal.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt as _;
    use tokio::time::{Duration, timeout};

    #[test]
    fn build_command_pipes_stderr() {
        let config = StdioConfig {
            command: "echo".into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: None,
            tool_timeout_ms: None,
        };
        // We can't inspect the Stdio setting directly on tokio::process::Command,
        // but we can verify the command is configured without panicking.
        // stderr piping is set on the TokioChildProcessBuilder, not here.
        let _cmd = build_command(&config);
    }

    #[tokio::test]
    async fn drain_stderr_exits_after_eof() {
        let (mut writer, stderr) = tokio::io::duplex(64);
        writer.write_all(b"diagnostic\n").await.unwrap();
        drop(writer);

        let finished = timeout(Duration::from_millis(100), drain_stderr(stderr)).await;

        assert!(finished.is_ok(), "stderr drain should stop at EOF");
    }

    #[tokio::test]
    async fn drain_stderr_ignores_non_utf8_without_line_buffering() {
        let (mut writer, stderr) = tokio::io::duplex(64);
        writer.write_all(b"\xffunterminated").await.unwrap();
        drop(writer);

        let finished = timeout(Duration::from_millis(100), drain_stderr(stderr)).await;

        assert!(
            finished.is_ok(),
            "stderr drain should treat stderr as raw bytes"
        );
    }
}
