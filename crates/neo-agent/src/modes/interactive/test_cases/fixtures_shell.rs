//! Interactive test fixtures: shell execution result scaffolding (moved from `mod.rs`).

pub fn completed_shell_result(
    stdout: impl Into<String>,
) -> neo_agent_core::tools::ShellExecutionResult {
    neo_agent_core::tools::ShellExecutionResult {
        stdout: stdout.into(),
        stderr: String::new(),
        exit_code: Some(0),
        signal: None,
        stdout_truncated: false,
        stderr_truncated: false,
        truncated: false,
        outcome: neo_agent_core::ShellCommandOutcome::Completed,
        foreground_task_id: None,
        resource_limit: None,
        capture_error: None,
        output_ref: None,
    }
}
