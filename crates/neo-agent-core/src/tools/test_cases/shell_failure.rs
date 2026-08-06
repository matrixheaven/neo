use super::*;

#[test]
fn format_shell_failure_nonzero_exit_code() {
    assert_eq!(
        format_shell_failure(Some(1), None),
        "Command failed with exit code: 1."
    );
    assert_eq!(
        format_shell_failure(Some(127), None),
        "Command failed with exit code: 127."
    );
}

#[test]
fn format_shell_failure_sigpipe_includes_hint() {
    let msg = format_shell_failure(None, Some(13));
    assert!(msg.contains("signal 13"), "{msg}");
    assert!(msg.contains("SIGPIPE"), "{msg}");
    assert!(msg.contains("pipe exited early"), "{msg}");
}

#[test]
fn format_shell_failure_sigkill_includes_hint() {
    let msg = format_shell_failure(None, Some(9));
    assert!(msg.contains("signal 9"), "{msg}");
    assert!(msg.contains("SIGKILL"), "{msg}");
    assert!(msg.contains("OOM"), "{msg}");
}

#[test]
fn format_shell_failure_unknown_signal() {
    let msg = format_shell_failure(None, Some(99));
    assert!(msg.contains("signal 99"), "{msg}");
    assert!(msg.contains("unknown signal"), "{msg}");
}

#[test]
fn format_shell_failure_no_code_no_signal() {
    assert_eq!(
        format_shell_failure(None, None),
        "Command terminated before returning an exit code."
    );
}

#[test]
fn command_timeout_error_recommends_a_larger_or_omitted_timeout() {
    let message = ToolError::CommandTimedOut {
        timeout_ms: 300_000,
    }
    .to_string();

    assert!(message.contains("Increase or double timeout_secs"));
    assert!(message.contains("omit it"));
}
