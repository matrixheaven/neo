use neo_agent_core::{AgentEvent, AgentMessage, AgentToolCall, Content, StopReason};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Component, Expandable, Line};
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::{ToolCallComponent, ToolCallState, TranscriptPane};
use serde_json::json;

fn plain(rows: Vec<Line>) -> Vec<String> {
    rows.into_iter()
        .map(|row| neo_tui::primitive::strip_ansi(&row.to_ansi()))
        .collect()
}
fn rendered(pane: &mut TranscriptPane) -> String {
    let lines = pane
        .render_frame(80, 20)
        .unwrap_or_else(|| pane.frame_ansi_lines());
    lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n")
}
fn apply_queued_bash(
    pane: &mut TranscriptPane,
    id: &str,
    command: &str,
    position: usize,
    waiting_ms: u64,
) {
    let arguments = json!({"command": command});
    pane.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: id.to_owned(),
        name: "Bash".to_owned(),
    });
    pane.apply_agent_event(AgentEvent::ToolCallFinished {
        turn: 1,
        tool_call: AgentToolCall {
            id: id.into(),
            name: "Bash".into(),
            raw_arguments: arguments.to_string().into(),
        },
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueued {
        turn: 1,
        id: id.to_owned(),
        name: "Bash".to_owned(),
        arguments,

        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: id.to_owned(),
        position,
        waiting_ms,
    });
}

#[test]
fn bash_queue_event_renders_position_and_wait_in_original_card() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "call-1".to_owned(),
        name: "Bash".to_owned(),
    });
    pane.apply_agent_event(AgentEvent::ToolCallFinished {
        turn: 1,
        tool_call: AgentToolCall {
            id: "call-1".into(),
            name: "Bash".into(),
            raw_arguments: r#"{"command":"cargo test"}"#.into(),
        },
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueued {
        turn: 1,
        id: "call-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: json!({"command": "cargo test"}),

        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: "call-1".to_owned(),
        position: 2,
        waiting_ms: 18_000,
    });
    let rendered = rendered(&mut pane);
    assert!(rendered.contains("Queued Bash · #2 · waiting 18s"));
    assert!(rendered.contains("$ cargo test"));
    assert_eq!(rendered.matches("Queued Bash").count(), 1);
}

#[test]
fn bash_running_card_shows_live_output_tail() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: Some(r#"{"command":"cargo test"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });

    for n in 1..=10 {
        card.append_live_output(format!("line {n}\n"));
    }

    let rows = plain(card.render(80));
    assert!(rows.iter().any(|line| line.contains("cargo test")));
    assert!(rows.iter().any(|line| line.contains("line 10")));
    assert!(rows.iter().any(|line| line.contains("earlier lines")));
    assert!(!rows.iter().any(|line| line.trim() == "line 1"));
}

#[test]
fn bash_shell_failure_summary_survives_empty_tool_result_finish() {
    use neo_agent_core::{AgentEvent, ShellCommandOrigin, ShellCommandOutcome};
    use neo_tui::primitive::strip_ansi;

    let mut runtime = TranscriptPane::new(80, 20);
    runtime.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        command: "git push origin main".to_owned(),
        cwd: "/workspace/neo".into(),
        origin: ShellCommandOrigin::ModelBashTool,
    });
    runtime.apply_agent_event(AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        exit_code: Some(1),
        signal: None,
        stdout: String::new(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome: ShellCommandOutcome::Completed,
        output_ref: None,
    });
    runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::error("").with_details(serde_json::json!({
            "exit_code": 1,
            "signal": null,
            "stdout": "",
            "stderr": "",
            "stdout_truncated": false,
            "stderr_truncated": false,
            "truncated": false,
            "outcome": "completed"
        })),

        workflow_origin: None,
        output_ref: None,
    });

    let frame = runtime
        .render_frame(80, 20)
        .expect("frame renders")
        .iter()
        .map(|line| strip_ansi(line).clone())
        .collect::<Vec<_>>();

    assert!(
        frame
            .iter()
            .any(|line| line.contains("Command failed with exit code: 1.")),
        "failed Bash card must not render with an empty body: {frame:?}"
    );
}

#[test]
fn bash_tool_card_renders_command_body_across_lifecycle_states() {
    let arguments = json!({"command": "cargo test", "description": "focused tests"}).to_string();
    let cases = [
        (
            "preparing",
            ToolStatusKind::Pending,
            None,
            None,
            "$ cargo test",
        ),
        ("queued", ToolStatusKind::Queued, None, None, "$ cargo test"),
        (
            "running",
            ToolStatusKind::Running,
            None,
            None,
            "$ cargo test",
        ),
        (
            "succeeded",
            ToolStatusKind::Succeeded,
            Some("tests passed"),
            None,
            "tests passed",
        ),
        (
            "failed",
            ToolStatusKind::Failed,
            Some("tests failed"),
            None,
            "tests failed",
        ),
        (
            "cancelled",
            ToolStatusKind::Cancelled,
            Some("cancelled by user"),
            None,
            "cancelled by user",
        ),
        (
            "background",
            ToolStatusKind::Succeeded,
            None,
            Some(json!({"outcome": "backgrounded", "task_id": "bash-1"})),
            "task bash-1 · focused tests",
        ),
    ];

    for (label, status, result, details, expected) in cases {
        let mut card = ToolCallComponent::new(ToolCallState {
            id: format!("bash-{label}"),
            name: "Bash".to_owned(),
            arguments: Some(arguments.clone()),
            result: result.map(str::to_owned),
            details,
            status,
            exit_code: None,
        });
        let rows = plain(card.render(80));
        let header = &rows[0];
        assert!(!header.contains("cargo test"), "{label}: {rows:?}");
        assert!(!header.contains('('), "{label}: {rows:?}");
        if label == "succeeded" {
            assert_eq!(header.matches("· 1 lines").count(), 1, "{rows:?}");
        }
        if label == "background" {
            assert_eq!(header.matches("· background").count(), 1, "{rows:?}");
        }
        assert!(
            rows.iter().any(|line| line.contains("$ cargo test")),
            "{label}: {rows:?}"
        );
        assert!(
            rows.iter().any(|line| line.contains(expected)),
            "{label}: {rows:?}"
        );
    }
}

#[test]
fn bash_tool_card_replay_resize_and_expansion_use_original_arguments() {
    let raw_arguments = json!({
        "command": "printf original-alpha-original-beta-original-gamma-original-delta"
    })
    .to_string();
    let mut transcript = TranscriptPane::new(28, 24);
    transcript.replay_message(&AgentMessage::Assistant {
        content: Vec::new(),
        tool_calls: vec![AgentToolCall {
            id: "bash-replay-1".into(),
            name: "Bash".into(),
            raw_arguments: raw_arguments.clone().into(),
        }],
        stop_reason: StopReason::ToolUse,
    });
    transcript.replay_message(&AgentMessage::ToolResult {
        tool_call_id: "bash-replay-1".into(),
        tool_name: "Bash".into(),
        content: vec![Content::text(
            "output-one\noutput-two\noutput-three\noutput-four",
        )],
        is_error: false,
    });

    let narrow = transcript
        .render_frame(28, 24)
        .expect("narrow frame")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(narrow.contains("original-alpha"), "{narrow}");
    assert!(narrow.contains("original-delta"), "{narrow}");

    transcript.set_tool_output_expanded(true);
    let wide = transcript
        .render_frame(100, 24)
        .expect("wide frame")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        wide.contains("$ printf original-alpha-original-beta-original-gamma-original-delta"),
        "{wide}"
    );
    assert!(wide.contains("output-four"), "{wide}");

    let stored_arguments = transcript
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            neo_tui::transcript::TranscriptEntry::ToolRun { component }
                if component.id() == "bash-replay-1" =>
            {
                component.arguments()
            }
            _ => None,
        });
    assert_eq!(stored_arguments, Some(raw_arguments.as_str()));
}

#[test]
fn queued_shell_card_keeps_relative_position_across_later_entries() {
    let mut pane = TranscriptPane::new(80, 20);
    apply_queued_bash(&mut pane, "call-1", "cargo test", 1, 4_000);
    pane.push_assistant_message("later assistant text");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "call-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: json!({"command": "cargo test"}),

        workflow_origin: None,
        output_ref: None,
    });
    let rendered = rendered(&mut pane);
    let tool = rendered.find("$ cargo test").expect("tool row");
    let later = rendered.find("later assistant text").expect("later row");
    assert!(tool < later, "living tool card drifted after later content");
}

#[test]
fn shell_live_output_bounds_eviction_without_losing_partial_tail() {
    use neo_tui::primitive::theme::TuiTheme;
    use neo_tui::transcript::ShellRunComponent;

    let mut card = ShellRunComponent::running("shell-1", "yes");
    for n in 1..=15 {
        card.append_live_output(format!("line {n}\n"));
    }
    card.append_live_output("tail");

    let rows = plain(card.render(80, &TuiTheme::default()));
    let joined = rows.join("\n");
    assert!(
        joined.contains("tail"),
        "partial tail must survive eviction: {joined:?}"
    );
    assert!(
        !joined.contains("line 1\n"),
        "oldest complete line should be evicted: {joined:?}"
    );
    assert!(
        joined.contains("earlier lines"),
        "eviction marker should be shown: {joined:?}"
    );
}

#[test]
fn shell_run_live_output_reassembles_split_control_sequences() {
    use neo_tui::primitive::theme::TuiTheme;
    use neo_tui::transcript::ShellRunComponent;

    let mut card = ShellRunComponent::running("shell-1", "echo test");
    card.append_live_output("\x1b]0;ti");
    card.append_live_output("tle\x07hello\n\x1b[3");
    card.append_live_output("1mworld\x1b[0m\npartial");

    let rows = plain(card.render(80, &TuiTheme::default()));
    let joined = rows.join("\n");
    assert!(
        joined.contains("hello"),
        "visible text should appear: {joined:?}"
    );
    assert!(
        joined.contains("world"),
        "sanitized visible text should appear: {joined:?}"
    );
    assert!(
        joined.contains("partial"),
        "trailing partial line should appear: {joined:?}"
    );
    assert!(
        !joined.contains('\x1b'),
        "escape sequences should not leak: {joined:?}"
    );
}

#[test]
fn shell_run_sanitizes_split_control_strings_with_canonical_ansi_state() {
    use neo_tui::primitive::theme::TuiTheme;
    use neo_tui::transcript::ShellRunComponent;

    let mut card = ShellRunComponent::running("shell-1", "echo test");
    card.append_live_output("\x1b[3");
    card.append_live_output("1mvisible\x1b[0m");

    let rows = plain(card.render(80, &TuiTheme::default()));
    let joined = rows.join("\n");
    assert!(
        joined.contains("visible"),
        "visible text should appear: {joined:?}"
    );
    assert!(
        !joined.contains('\x1b'),
        "escape sequences should not leak: {joined:?}"
    );
}

#[test]
fn terminal_output_controls_cannot_escape_tool_card() {
    let raw = "\x0c\x1b[24;1H\"/tmp/test.txt\" 1 line, 24 bytes\x1b[1;1Hhello from neo terminal\r";
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "terminal-1".to_owned(),
        name: "Terminal".to_owned(),
        arguments: Some(r#"{"mode":"write","handle":"term-1"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });
    assert!(card.append_live_output(raw));

    let running = card
        .render(80)
        .into_iter()
        .map(|line| line.to_ansi())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!running.contains('\x0c'), "running output leaked form feed");
    assert!(
        !running.contains('\r'),
        "running output leaked carriage return"
    );
    assert!(
        !running.contains("\x1b[24;1H"),
        "running output leaked cursor positioning"
    );

    assert!(card.set_result(Some(raw.to_owned()), None, false, None));
    card.set_expanded(true);
    let finished = card
        .render(80)
        .into_iter()
        .map(|line| line.to_ansi())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!finished.contains('\x0c'), "final output leaked form feed");
    assert!(
        !finished.contains('\r'),
        "final output leaked carriage return"
    );
    assert!(
        !finished.contains("\x1b[24;1H"),
        "final output leaked cursor positioning"
    );
}

#[test]
fn terminal_tool_card_renders_operation_specific_body() {
    use neo_tui::primitive::visible_width;

    let cases = [
        (
            "start",
            json!({
                "mode": "start",
                "handle": "typed-start-fake",
                "command": "printf 'hello'",
                "cwd": "C:\\repo\\\u{1b}[31mneo\u{1b}[0m\tui\r\n",
            }),
            "handle: term-start\nstatus: running\noutput:\n",
            json!({
                "handle": "\u{1b}[32mterm-start\u{1b}[0m\ttrue\u{7f}",
                "status": "running",
                "output": ""
            }),
        ),
        (
            "write",
            json!({
                "mode": "write",
                "handle": "\u{1b}[31mterm-write\u{1b}[0m\ttyped",
                "input": [
                    {"text": r"\x03"},
                    {"control": 3},
                    {"text": "\r\n\t\u{1b}世界"},
                    {"control": 4},
                ],
            }),
            "handle: term-write\nstatus: running\noutput:\nwrite output",
            json!({
                "handle": "detail-write-fake",
                "status": "running",
                "output": "write \u{1b}[31mred\u{1b}[0m\t\u{7f}\u{85}\nnext\r"
            }),
        ),
        (
            "read",
            json!({"mode": "read", "handle": "term-read"}),
            "handle: term-read\nstatus: running\noutput:\nread one\nread two",
            json!({"handle": "term-read", "status": "running", "output": "read one\nread two"}),
        ),
        (
            "resize",
            json!({"mode": "resize", "handle": "term-resize", "cols": 120, "rows": 40}),
            "handle: term-resize\nstatus: running\ncols: 120\nrows: 40",
            json!({"handle": "term-resize", "status": "running", "cols": 120, "rows": 40}),
        ),
        (
            "stop",
            json!({"mode": "stop", "handle": "term-stop"}),
            "handle: term-stop\nstatus: completed\noutput:\nfinal output",
            json!({"handle": "term-stop", "status": "completed", "output": "final output"}),
        ),
        (
            "stop-failure",
            json!({"mode": "stop", "handle": "term-failed"}),
            "handle: term-failed\nstatus:\t\u{1b}[31mparent_exited\u{1b}[0m\u{7f}\noutput:\nparent\u{85}",
            json!({"handle": "term-failed", "status": "parent_exited", "output": "parent vanished"}),
        ),
        (
            "read-missing-details",
            json!({"mode": "read", "handle": "term-legacy"}),
            "legacy \u{1b}[31mread\u{1b}[0m\tvalue\u{7f}\nline\u{85}\r",
            json!({}),
        ),
        (
            "read-empty",
            json!({"mode": "read", "handle": "term-empty"}),
            "handle: term-empty\nstatus: running\noutput:\n",
            json!({"handle": "detail-empty-fake", "status": "running", "output": "\u{1b}[31m\u{1b}[0m"}),
        ),
    ];
    let theme = TuiTheme::default();

    for width in [24, 100] {
        for (label, arguments, result, details) in &cases {
            let mut card = ToolCallComponent::new(ToolCallState {
                id: format!("terminal-{label}"),
                name: "Terminal".to_owned(),
                arguments: Some(arguments.to_string()),
                result: Some((*result).to_owned()),
                details: Some(details.clone()),
                status: ToolStatusKind::Succeeded,
                exit_code: None,
            });
            let rows = card.render_with_theme(width, &theme);
            assert!(
                rows.iter().all(|row| visible_width(&row.text()) <= width),
                "{label} exceeded width {width}: {:?}",
                plain(rows.clone())
            );
            assert!(
                rows.iter()
                    .flat_map(Line::spans)
                    .flat_map(|span| span.text().chars())
                    .all(|character| !character.is_control()),
                "{label} leaked a terminal control byte: {:?}",
                plain(rows)
            );
        }
    }

    let render_wide = |label: &str| {
        let (_, arguments, result, details) = cases
            .iter()
            .find(|(case, ..)| *case == label)
            .expect("known terminal case");
        let mut card = ToolCallComponent::new(ToolCallState {
            id: format!("terminal-{label}"),
            name: "Terminal".to_owned(),
            arguments: Some(arguments.to_string()),
            result: Some((*result).to_owned()),
            details: Some(details.clone()),
            status: ToolStatusKind::Succeeded,
            exit_code: None,
        });
        card.set_expanded(true);
        plain(card.render_with_theme(100, &theme))
    };

    let start = render_wide("start");
    assert!(
        start[0].contains(r"Used Terminal · start · term-start\ttrue\u{7f}"),
        "{start:?}"
    );
    assert!(
        start
            .iter()
            .any(|row| row.contains(r"cwd C:\repo\neo\tui\r\n")),
        "{start:?}"
    );
    assert!(!start.join("\n").contains("typed-start-fake"), "{start:?}");
    assert!(!start.join("\n").contains(r"C:\\repo"), "{start:?}");
    assert!(
        start.iter().any(|row| row.contains("$ printf 'hello'")),
        "{start:?}"
    );
    assert!(
        start.iter().any(|row| row.contains("Terminal started.")),
        "{start:?}"
    );
    assert!(
        !start.iter().any(|row| row.contains("handle:")),
        "{start:?}"
    );

    let write = render_wide("write");
    assert!(
        write[0].contains(r"Used Terminal · write · term-write\ttyped"),
        "{write:?}"
    );
    assert!(!write.join("\n").contains("detail-write-fake"), "{write:?}");
    assert!(
        write
            .iter()
            .any(|row| row.contains(r"stdin › \\x03\x03\r\n\t\x1b世界\x04")),
        "{write:?}"
    );
    assert!(
        write
            .iter()
            .any(|row| row.contains(r"write red\t\u{7f}\u{85}")),
        "{write:?}"
    );
    assert!(write.iter().any(|row| row.contains(r"next\r")), "{write:?}");
    assert!(
        !write.iter().any(|row| row.contains("status:")),
        "{write:?}"
    );

    let read = render_wide("read");
    assert!(read.iter().any(|row| row.contains("read one")), "{read:?}");
    assert!(read.iter().any(|row| row.contains("read two")), "{read:?}");
    assert!(!read.iter().any(|row| row.contains("handle:")), "{read:?}");
    assert!(!read.iter().any(|row| row.contains("output:")), "{read:?}");

    let resize = render_wide("resize");
    assert!(
        resize.iter().any(|row| row.contains("size 120 × 40")),
        "{resize:?}"
    );
    assert!(
        !resize.iter().any(|row| row.contains("cols:")),
        "{resize:?}"
    );

    let stop = render_wide("stop");
    assert!(
        stop.iter().any(|row| row.contains("final output")),
        "{stop:?}"
    );
    assert!(
        stop.iter().any(|row| row.contains("Process tree stopped.")),
        "{stop:?}"
    );
    assert!(!stop.iter().any(|row| row.contains("status:")), "{stop:?}");

    let failure = render_wide("stop-failure");
    assert!(
        failure
            .iter()
            .any(|row| row.contains(r"status:\tparent_exited\u{7f}")),
        "{failure:?}"
    );
    assert!(
        failure.iter().any(|row| row.contains(r"parent\u{85}")),
        "{failure:?}"
    );

    let missing = render_wide("read-missing-details");
    assert!(
        missing
            .iter()
            .any(|row| row.contains(r"legacy read\tvalue\u{7f}")),
        "{missing:?}"
    );
    assert!(
        missing.iter().any(|row| row.contains(r"line\u{85}\r")),
        "{missing:?}"
    );

    let empty = render_wide("read-empty");
    assert_eq!(empty, ["● Used Terminal · read · term-empty"], "{empty:?}");
    assert!(!empty.join("\n").contains("handle:"), "{empty:?}");
    assert!(!empty.join("\n").contains("status:"), "{empty:?}");
    assert!(!empty.join("\n").contains("output:"), "{empty:?}");
}

#[test]
fn tool_call_live_output_reassembles_split_lines_and_ansi() {
    use neo_tui::transcript::ToolCallComponent;

    let mut card = ToolCallComponent::new(ToolCallState {
        id: "live-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: Some(r#"{"command":"echo test"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });

    card.append_live_output("line one\nline ");
    let rows = plain(card.render(80));
    let joined = rows.join("\n");
    assert!(
        joined.contains("line one"),
        "first complete line should appear: {joined:?}"
    );
    assert!(
        joined.contains("line "),
        "partial tail should be preserved: {joined:?}"
    );

    card.append_live_output("two\n\x1b[3");
    let rows = plain(card.render(80));
    let joined = rows.join("\n");
    assert!(
        joined.contains("line two"),
        "reassembled line should appear: {joined:?}"
    );

    card.append_live_output("1mred\x1b[0m\nline three");
    let rows = plain(card.render(80));
    let joined = rows.join("\n");
    assert!(
        joined.contains("red"),
        "sanitized visible text should appear: {joined:?}"
    );
    assert!(
        joined.contains("line three"),
        "trailing partial line should appear: {joined:?}"
    );
    assert!(
        !joined.contains('\x1b'),
        "escape sequences should not leak: {joined:?}"
    );
}

#[test]
fn transcript_pane_expansion_reaches_rendered_bash_tool_body() {
    use neo_agent_core::AgentEvent;
    use neo_tui::primitive::strip_ansi;

    let mut runtime = TranscriptPane::new(80, 20);
    let command = [
        "printf command-head",
        "printf command-middle-1",
        "printf command-middle-2",
        "printf command-middle-3",
        "printf command-middle-4",
        "printf command-tail",
    ]
    .join("\n");
    runtime.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "bash-1".to_owned(),
        json_fragment: json!({"command": command}).to_string(),
    });
    runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok("1\n2\n3\n4\n5\n6\n7\n8"),

        workflow_origin: None,
        output_ref: None,
    });

    let collapsed = runtime
        .render_frame(80, 20)
        .expect("collapsed frame")
        .iter()
        .map(|line| strip_ansi(line).clone())
        .collect::<Vec<_>>();
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("$ printf command-head")),
        "collapsed frame should keep command head: {collapsed:?}"
    );
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("printf command-tail")),
        "collapsed frame should keep command tail: {collapsed:?}"
    );
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("characters hidden · ctrl+o to expand")),
        "collapsed frame should show command expansion hint: {collapsed:?}"
    );
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("more lines, ctrl+o to expand")),
        "collapsed frame should show output expansion hint: {collapsed:?}"
    );

    runtime.set_tool_output_expanded(true);
    let expanded = runtime
        .render_frame(80, 20)
        .expect("expanded frame")
        .iter()
        .map(|line| strip_ansi(line).clone())
        .collect::<Vec<_>>();
    assert!(
        expanded
            .iter()
            .any(|line| line.contains("printf command-middle-4")),
        "expanded frame should show the complete command: {expanded:?}"
    );
    assert!(
        expanded.iter().any(|line| line.trim() == "8"),
        "expanded frame should show final result line: {expanded:?}"
    );
    assert!(
        !expanded
            .iter()
            .any(|line| line.contains("ctrl+o to expand")),
        "expanded frame should hide expansion hint: {expanded:?}"
    );
}
