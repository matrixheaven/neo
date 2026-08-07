use neo_agent_core::{
    ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, PermissionOperation,
};
use neo_tui::primitive::strip_ansi;
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::{TranscriptEntry, TranscriptPane};

fn shell_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            label: "Approve once".to_owned(),
            description: None,
            action: ApprovalAction::PermitOnce,
        },
        ApprovalOption {
            label: "Reject".to_owned(),
            description: None,
            action: ApprovalAction::Reject,
        },
    ]
}
fn plain_frame(transcript: &mut TranscriptPane, width: usize, height: usize) -> Vec<String> {
    transcript
        .render_frame(width, height)
        .expect("render frame")
        .iter()
        .map(|line| plain(line))
        .collect()
}
fn plain(line: &str) -> String {
    strip_ansi(line).trim_end().to_owned()
}

#[test]
fn transcript_does_not_render_duplicate_bash_queued_and_used_for_same_id() {
    let mut transcript_pane = TranscriptPane::new(80, 16);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "bash-1".to_owned(),
        json_fragment: r#"{"command":"echo hi"}"#.to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        command: "echo hi".to_owned(),
        cwd: std::path::PathBuf::from("/tmp"),
        origin: neo_agent_core::ShellCommandOrigin::ModelBashTool,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "hi\n".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: neo_agent_core::ShellCommandOrigin::ModelBashTool,
        outcome: neo_agent_core::ShellCommandOutcome::Completed,
        output_ref: None,
    });

    let frame = plain_frame(&mut transcript_pane, 80, 16);
    assert_eq!(
        frame
            .iter()
            .filter(|line| {
                line.contains("Bash")
                    && (line.contains("Queued")
                        || line.contains("Using")
                        || line.contains("Used")
                        || line.contains("Failed"))
            })
            .count(),
        1,
        "same tool id should render one Bash card: {frame:?}"
    );
    assert!(frame.iter().any(|line| line.contains("Used Bash")));
    assert!(
        frame.iter().any(|line| line.contains("$ echo hi")),
        "successful Bash card must retain its command: {frame:?}"
    );
    assert!(!frame.iter().any(|line| line.contains("Queued Bash")));
}

#[test]
fn transcript_marks_pending_tool_failed_when_turn_errors() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: r#"{"command":"echo hi"}"#.to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::Error {
        turn: 1,
        message: "Provider reported tool calls but emitted no structured tool calls".to_owned(),
        code: None,
        retry_after: None,
    });

    let frame = plain_frame(&mut transcript_pane, 80, 12);
    assert!(frame.iter().any(|line| line.contains("Failed Bash")));
    assert!(!frame.iter().any(|line| line.contains("Queued Bash")));

    let state = transcript_pane
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::ToolRun { component } => Some(component.state()),
            _ => None,
        })
        .expect("tool run exists");
    assert_eq!(state.status, ToolStatusKind::Failed);
    assert!(
        state
            .result
            .as_deref()
            .is_some_and(|result| result.contains("Provider reported tool calls"))
    );
}

#[test]
fn transcript_pane_accumulates_tool_argument_delta_fragments() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: "{\"path\":\"".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: "README.md\"}".to_owned(),
    });

    let frame = plain_frame(&mut transcript_pane, 80, 12);
    assert!(
        frame
            .iter()
            .any(|l| l.contains("Preparing Read (README.md)"))
    );
}

#[test]
fn transcript_pane_keeps_finished_tool_cards_in_the_same_frame_slot() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),

        workflow_origin: None,
        output_ref: None,
    });
    let running = plain_frame(&mut transcript_pane, 80, 12);
    assert!(running.iter().any(|l| l.contains("Using Read (README.md)")));

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("line one\nline two"),

        workflow_origin: None,
        output_ref: None,
    });
    let finished = plain_frame(&mut transcript_pane, 80, 12);

    assert!(finished.iter().any(|l| l.contains("Used Read (README.md)")));
    // The finished card appears exactly once.
    assert_eq!(
        finished
            .iter()
            .filter(|l| l.contains("Used Read (README.md)"))
            .count(),
        1
    );
}

#[test]
fn transcript_pane_keeps_running_tool_run_live() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),

        workflow_origin: None,
        output_ref: None,
    });

    let state = transcript_pane
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::ToolRun { component } => Some(component.state()),
            _ => None,
        })
        .expect("tool run exists");
    assert_eq!(state.id, "tool-1");
    assert_eq!(state.status, ToolStatusKind::Running);
}

#[test]
fn transcript_pane_marks_declared_tool_call_as_queued_until_execution_starts() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: r#"{"command":"cargo test"}"#.to_owned(),
    });

    let preparing = plain_frame(&mut transcript_pane, 80, 12);
    assert!(preparing.iter().any(|line| line.contains("Preparing Bash")));
    assert!(
        !preparing.iter().any(|line| line.contains("Using Bash")),
        "declared-but-not-started tool calls must not look like running tools: {preparing:?}"
    );
    assert!(
        !preparing.iter().any(|line| line.contains("Queued Bash")),
        "Pending preparation must not be labeled Queued: {preparing:?}"
    );

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),

        workflow_origin: None,
        output_ref: None,
    });

    let running = plain_frame(&mut transcript_pane, 80, 12);
    assert!(running.iter().any(|line| line.contains("Using Bash")));
}

#[test]
fn transcript_pane_records_tool_execution_updates_on_existing_run() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        partial_result: neo_agent_core::ToolResult::ok("building crate"),

        workflow_origin: None,
        output_ref: None,
    });

    let component = transcript_pane
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::ToolRun { component } => Some(component),
            _ => None,
        })
        .expect("tool run exists");
    assert_eq!(component.state().status, ToolStatusKind::Running);
    let frame = plain_frame(&mut transcript_pane, 80, 12);
    assert!(frame.iter().any(|line| line.contains("building crate")));
}

#[test]
fn transcript_pane_renders_task_stop_from_request_details() {
    let mut transcript_pane = TranscriptPane::new(100, 18);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            turn: 1,
            id: "stop-1".to_owned(),
            operation: PermissionOperation::Shell,
            presentation: ApprovalPresentation::Tool {
                title: "Stop background task?".to_owned(),
                details: vec![
                    "task_id: bash-1234".to_owned(),
                    "reason: no longer needed".to_owned(),
                ],
            },
            options: shell_options(),

            workflow_origin: None,
        },
    });

    let frame = plain_frame(&mut transcript_pane, 100, 18);
    assert!(
        frame
            .iter()
            .any(|line| line.contains("Stop background task?"))
    );
    assert!(frame.iter().any(|line| line.contains("task_id: bash-1234")));
    assert!(
        frame
            .iter()
            .any(|line| line.contains("reason: no longer needed"))
    );
}

#[test]
fn transcript_pane_renders_tool_presentation_from_request() {
    let mut transcript_pane = TranscriptPane::new(100, 18);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            turn: 1,
            id: "terminal-1".to_owned(),
            operation: PermissionOperation::Shell,
            presentation: ApprovalPresentation::Tool {
                title: "Start terminal?".to_owned(),
                details: vec![
                    "mode: start".to_owned(),
                    "$ bash --noprofile --norc".to_owned(),
                ],
            },
            options: shell_options(),

            workflow_origin: None,
        },
    });

    let frame = plain_frame(&mut transcript_pane, 100, 18);
    assert!(frame.iter().any(|line| line.contains("Start terminal?")));
    assert!(frame.iter().any(|line| line.contains("mode: start")));
    assert!(
        frame
            .iter()
            .any(|line| line.contains("$ bash --noprofile --norc"))
    );
}

#[test]
fn transcript_pane_updates_one_tool_run_entry_in_place() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: r#"{"path":"README.md"}"#.to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),

        workflow_origin: None,
        output_ref: None,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("line one\nline two"),

        workflow_origin: None,
        output_ref: None,
    });

    let tool_runs = transcript_pane
        .transcript()
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::ToolRun { component } => Some(component.state()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_runs.len(), 1);
    let state = tool_runs[0];
    assert_eq!(state.id, "tool-1");
    assert_eq!(state.status, ToolStatusKind::Succeeded);
    assert_eq!(state.arguments.as_deref(), Some(r#"{"path":"README.md"}"#));
    assert_eq!(state.result.as_deref(), Some("line one\nline two"));
}
