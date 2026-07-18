use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input, schema};

#[derive(Debug, Deserialize, JsonSchema)]
struct SleepInput {
    #[schemars(range(min = 1, max = 3600))]
    duration_seconds: u64,
    #[schemars(description = "Short single-line reason for waiting (maximum 160 characters).")]
    reason: String,
}

/// Built-in timer wait that never touches shell admission or processes.
pub struct SleepTool;

fn invalid_sleep(message: &str) -> ToolError {
    ToolError::InvalidInput {
        tool: "Sleep".to_owned(),
        message: message.to_owned(),
    }
}

impl Tool for SleepTool {
    fn name(&self) -> &str {
        "Sleep"
    }

    fn description(&self) -> &str {
        "Pause this agent without starting a shell command. Use only for a \
         genuine time-based wait. Prefer WaitDelegate for a known agent or \
         swarm, and TaskOutput with block=true for a known background task. \
         The wait is cancellable and duration_seconds must be 1..=3600."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<SleepInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: SleepInput = parse_input(self.name(), input)?;
            let reason = input.reason.trim();
            if !(1..=3600).contains(&input.duration_seconds) {
                return Err(invalid_sleep("duration_seconds must be between 1 and 3600"));
            }
            if reason.is_empty()
                || reason.contains('\r')
                || reason.contains('\n')
                || reason.chars().count() > 160
            {
                return Err(invalid_sleep(
                    "reason must be a non-empty single line of at most 160 characters",
                ));
            }
            tokio::select! {
                biased;
                () = ctx.cancel_token.cancelled() => Err(ToolError::Cancelled),
                () = tokio::time::sleep(Duration::from_secs(input.duration_seconds)) => {
                    Ok(ToolResult::ok(format!(
                        "Waited {} seconds: {reason}",
                        input.duration_seconds
                    )))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::tools::{
        ShellAdmissionClass, ShellAdmissionRequest, ShellLimits, ShellRuntime, ToolContext,
        ToolError,
    };

    #[tokio::test]
    async fn sleep_validates_bounds_reason_and_cancellation() {
        let tool = SleepTool;
        let spec = tool.spec();
        let schema = spec
            .input_schema
            .get("schema")
            .unwrap_or(&spec.input_schema);
        let required = schema["required"].as_array().expect("required fields");
        assert!(
            required
                .iter()
                .any(|field| field.as_str() == Some("duration_seconds"))
        );
        assert!(
            required
                .iter()
                .any(|field| field.as_str() == Some("reason"))
        );
        assert!(spec.description.contains("WaitDelegate"));
        assert!(spec.description.contains("TaskOutput"));
        assert!(spec.description.contains("block=true"));
        let temp = tempfile::tempdir().expect("tempdir");
        let context = ToolContext::new(temp.path()).expect("tool context");
        for input in [
            json!({"duration_seconds": 0, "reason": "wait"}),
            json!({"duration_seconds": 3601, "reason": "wait"}),
            json!({"duration_seconds": 1, "reason": ""}),
            json!({"duration_seconds": 1, "reason": "line one\nline two"}),
            json!({"duration_seconds": 1, "reason": "x".repeat(161)}),
        ] {
            assert!(tool.execute(&context, input).await.is_err());
        }

        let cancelled = ToolContext::new(temp.path()).expect("cancelled context");
        cancelled.cancel_token.cancel();
        let error = tool
            .execute(
                &cancelled,
                json!({"duration_seconds": 60, "reason": "backoff"}),
            )
            .await
            .expect_err("cancelled Sleep completed");
        assert!(matches!(error, ToolError::Cancelled));
    }

    #[tokio::test]
    async fn sleep_does_not_consume_or_wait_for_shell_admission() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = ShellRuntime::new(
            ShellLimits {
                max_active_commands: 1,
                ..ShellLimits::default()
            },
            PathBuf::from("unused-guardian"),
            temp.path().join("runtime"),
        );
        let held = runtime
            .acquire(
                ShellAdmissionRequest {
                    owner: "held".to_owned(),
                    class: ShellAdmissionClass::AgentForeground,
                },
                None,
            )
            .await;
        let context = ToolContext::new(temp.path())
            .expect("tool context")
            .with_shell_runtime(runtime);
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            SleepTool.execute(
                &context,
                json!({"duration_seconds": 1, "reason": "timer backoff"}),
            ),
        )
        .await
        .expect("Sleep waited for shell admission")
        .expect("Sleep result");
        assert!(result.content.contains("Waited 1 seconds"));
        drop(held);
    }
}
