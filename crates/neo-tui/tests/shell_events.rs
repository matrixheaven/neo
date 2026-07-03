use neo_agent_core::AgentMessage;
use neo_agent_core::{AgentEvent, ShellCommandOrigin, ShellCommandOutcome};
use neo_tui::transcript::TranscriptPane;

fn rendered(pane: &mut TranscriptPane) -> String {
    let lines = pane
        .render_frame(80, 12)
        .unwrap_or_else(|| pane.frame_ansi_lines());
    lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn user_shell_origin_creates_shell_run_not_tool_card() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        command: "whoami".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::UserShellMode,
    });
    let rendered = rendered(&mut pane);
    assert!(rendered.contains("$ whoami"));
    assert!(!rendered.contains("Bash"));
}

#[test]
fn user_shell_finish_updates_existing_shell_run() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        command: "whoami".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::UserShellMode,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "shell-1".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "me\n".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::UserShellMode,
        outcome: ShellCommandOutcome::Completed,
    });
    let rendered = rendered(&mut pane);
    assert!(rendered.contains("me"));
}

#[test]
fn replay_shell_command_message_renders_shell_run_without_xml() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.replay_message(&AgentMessage::shell_command(
        "echo hi",
        "hi\n",
        "",
        Some(0),
        ShellCommandOutcome::Completed,
        false,
    ));

    let rendered = rendered(&mut pane);
    assert!(rendered.contains("$ echo hi"));
    assert!(rendered.contains("hi"));
    assert!(!rendered.contains("<bash-input>"));
}

#[test]
fn replay_shell_command_message_preserves_truncated_marker() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.replay_message(&AgentMessage::shell_command(
        "printf long",
        "long",
        "",
        Some(0),
        ShellCommandOutcome::Completed,
        true,
    ));

    let rendered = rendered(&mut pane);
    assert!(rendered.contains("[output truncated]"));
}
