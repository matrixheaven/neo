use std::{process::Stdio, time::Duration};

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

use super::{
    Tool, ToolContext, ToolError, ToolFuture, ToolResult, cap_output, parse_input, schema,
};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BashInput {
    command: String,
    timeout_ms: Option<u64>,
    max_output_bytes: Option<usize>,
}

pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &'static str {
        "bash"
    }

    fn description(&self) -> &'static str {
        "Run a foreground shell command in the workspace."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<BashInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            ctx.ensure_shell_allowed()?;
            let input: BashInput = parse_input(self.name(), input)?;
            let timeout_ms = input
                .timeout_ms
                .unwrap_or_else(|| u64::try_from(ctx.bash_timeout.as_millis()).unwrap_or(u64::MAX));
            let max_output_bytes = input.max_output_bytes.unwrap_or(ctx.max_output_bytes);
            let output =
                run_command(ctx, &input.command, Duration::from_millis(timeout_ms)).await?;
            let (stdout_capped, stdout_truncated) = cap_output(&output.stdout, max_output_bytes);
            let (stderr_capped, stderr_truncated) = cap_output(&output.stderr, max_output_bytes);
            let truncated = stdout_truncated || stderr_truncated;
            let combined = format!(
                "exit_code: {:?}\nstdout:\n{}\nstderr:\n{}",
                output.exit_code, stdout_capped, stderr_capped
            );
            Ok(
                ToolResult::ok(format!("{combined}\ntruncated: {truncated}")).with_details(json!({
                    "exit_code": output.exit_code,
                    "stdout": output.stdout,
                    "stderr": output.stderr,
                    "stdout_truncated": stdout_truncated,
                    "stderr_truncated": stderr_truncated,
                    "truncated": truncated,
                })),
            )
        })
    }
}

struct CommandOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

async fn run_command(
    ctx: &ToolContext,
    command: &str,
    timeout_duration: Duration,
) -> Result<CommandOutput, ToolError> {
    let mut child = Command::new("bash")
        .arg("-lc")
        .arg(command)
        .current_dir(&ctx.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdout = child.stdout.take().expect("stdout was piped");
    let mut stderr = child.stderr.take().expect("stderr was piped");

    let stdout_task = tokio::spawn(async move {
        let mut buffer = Vec::new();
        stdout.read_to_end(&mut buffer).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&buffer).into_owned())
    });
    let stderr_task = tokio::spawn(async move {
        let mut buffer = Vec::new();
        stderr.read_to_end(&mut buffer).await?;
        Ok::<_, std::io::Error>(String::from_utf8_lossy(&buffer).into_owned())
    });

    let Ok(status) = timeout(timeout_duration, child.wait()).await else {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err(ToolError::CommandTimedOut {
            timeout_ms: u64::try_from(timeout_duration.as_millis()).unwrap_or(u64::MAX),
        });
    };
    let status = status?;

    let stdout = stdout_task.await.map_err(std::io::Error::other)??;
    let stderr = stderr_task.await.map_err(std::io::Error::other)??;
    Ok(CommandOutput {
        exit_code: status.code(),
        stdout,
        stderr,
    })
}
