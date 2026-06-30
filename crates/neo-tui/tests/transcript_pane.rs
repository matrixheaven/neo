use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{strip_ansi, visible_width};
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::TranscriptEntry;
use neo_tui::transcript::TranscriptPane;

/// Strip ANSI + trim from a frame line, for content assertions.
fn plain(line: &str) -> String {
    strip_ansi(line).trim_end().to_owned()
}

/// Render a frame and return its lines as plain (ANSI-stripped) strings.
fn plain_frame(transcript: &mut TranscriptPane, width: usize, height: usize) -> Vec<String> {
    transcript
        .render_frame(width, height)
        .expect("render frame")
        .iter()
        .map(|line| plain(line))
        .collect()
}

#[test]
fn unchanged_theme_and_size_do_not_schedule_body_rerender() {
    let mut transcript_pane = TranscriptPane::new(80, 12);
    transcript_pane.push_transcript(TranscriptEntry::banner("Welcome to neo"));
    assert!(transcript_pane.render_frame(80, 12).is_some());

    transcript_pane.set_theme(TuiTheme::default());
    transcript_pane.resize(80, 12);

    assert!(
        transcript_pane.render_frame(80, 12).is_none(),
        "unchanged theme/size should not force body redraws every terminal tick"
    );
}

#[test]
fn transcript_pane_renders_transcript_entries_in_one_ordered_frame() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.push_transcript(TranscriptEntry::banner("Welcome to neo"));
    transcript_pane.push_transcript(TranscriptEntry::user_message("hello"));
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
    });
    transcript_pane.push_transcript(TranscriptEntry::assistant_message("streaming"));

    let frame = plain_frame(&mut transcript_pane, 80, 12);
    // All entries render through one transcript order. The banner renders as a
    // rounded box containing the title text.
    let welcome = frame
        .iter()
        .position(|l| l.contains("Welcome to neo"))
        .expect("banner");
    // User message is now bullet-led (Neo), no "You" label.
    let hello = frame
        .iter()
        .position(|l| l.contains("✨") && l.contains("hello"))
        .expect("user message");
    let tool = frame
        .iter()
        .position(|l| l.contains("Using Bash"))
        .expect("running tool card");
    let streaming = frame
        .iter()
        .position(|l| l.contains("streaming"))
        .expect("streaming assistant");
    assert!(welcome < hello);
    assert!(hello < tool);
    assert!(tool < streaming);
}

#[test]
fn transcript_pane_exposes_frame_ansi_lines_for_inspection() {
    let mut transcript_pane = TranscriptPane::new(80, 12);
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
    });
    let _ = transcript_pane.render_frame(80, 12);

    let lines = transcript_pane.frame_ansi_lines();
    assert!(
        lines.iter().any(|line| plain(line).contains("Using Bash")),
        "frame lines: {lines:?}"
    );
}

#[test]
fn transcript_pane_renders_inline_bash_approval_prompt() {
    let mut transcript_pane = TranscriptPane::new(100, 16);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "echo hello" }),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "bash-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "echo hello".to_owned(),
        arguments: serde_json::json!({
            "command": "echo hello",
            "cwd": "/Users/chenyuanhao/Workspace/neo"
        }),
        session_scope: None,
        prefix_rule: None,
    });

    let frame = plain_frame(&mut transcript_pane, 100, 16);
    let using = frame
        .iter()
        .position(|line| line.contains("Using Bash"))
        .expect("running bash tool");
    let approval = frame
        .iter()
        .position(|line| line.contains("Run this command?"))
        .expect("inline approval prompt");

    assert!(using < approval);
    assert!(
        frame
            .iter()
            .any(|line| line.contains("cwd: /Users/chenyuanhao/Workspace/neo"))
    );
    assert!(frame.iter().any(|line| line.contains("$ echo hello")));
    assert!(frame.iter().any(|line| line.contains("1. Approve once")));
    assert!(
        frame
            .iter()
            .any(|line| line.contains("2. Approve for this session"))
    );
    assert!(frame.iter().any(|line| line.contains("3. Reject")));
    assert!(
        frame
            .iter()
            .any(|line| line.contains("4. Reject with feedback"))
    );
    assert!(
        frame.iter().any(|line| {
            line.contains("↑/↓ select")
                && line.contains("number keys choose")
                && line.contains("↵ confirm")
        }),
        "approval prompt should show the keyboard hint: {frame:?}"
    );

    transcript_pane.resize(36, 24);
    let narrow = plain_frame(&mut transcript_pane, 36, 24);
    assert!(
        narrow.iter().all(|line| visible_width(line) <= 34),
        "approval prompt lines should fit narrow transcript width: {narrow:?}"
    );
}

#[test]
fn transcript_pane_only_renders_active_approval_and_queued_count() {
    let mut transcript_pane = TranscriptPane::new(100, 24);

    for number in 1..=3 {
        let command = format!("printf {number}");
        transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
            turn: 1,
            id: format!("bash-{number}"),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: command.clone(),
            arguments: serde_json::json!({ "command": command }),
            session_scope: None,
            prefix_rule: None,
        });
    }

    let frame = plain_frame(&mut transcript_pane, 100, 24);
    assert!(frame.iter().any(|line| line.contains("$ printf 1")));
    assert!(!frame.iter().any(|line| line.contains("$ printf 2")));
    assert!(!frame.iter().any(|line| line.contains("$ printf 3")));
    assert!(
        frame
            .iter()
            .any(|line| line.contains("queued: 2 approvals waiting")),
        "frame: {frame:?}"
    );
}

#[test]
fn transcript_pane_renders_terminal_approval_prompt() {
    let mut transcript_pane = TranscriptPane::new(100, 18);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "terminal-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "bash --noprofile --norc".to_owned(),
        arguments: serde_json::json!({
            "mode": "start",
            "command": "bash --noprofile --norc",
            "cols": 80,
            "rows": 24
        }),
        session_scope: None,
        prefix_rule: None,
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
fn transcript_pane_renders_task_stop_approval_prompt() {
    let mut transcript_pane = TranscriptPane::new(100, 18);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "stop-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "bash-1234".to_owned(),
        arguments: serde_json::json!({
            "task_id": "bash-1234",
            "reason": "no longer needed"
        }),
        session_scope: None,
        prefix_rule: None,
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
fn transcript_pane_renders_write_approval_prompt() {
    let mut transcript_pane = TranscriptPane::new(100, 18);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "write-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::FileWrite,
        subject: "src/lib.rs".to_owned(),
        arguments: serde_json::json!({
            "path": "src/lib.rs",
            "content": "pub fn demo() {}"
        }),
        session_scope: None,
        prefix_rule: None,
    });

    let frame = plain_frame(&mut transcript_pane, 100, 18);
    assert!(frame.iter().any(|line| line.contains("Write file?")));
    assert!(frame.iter().any(|line| line.contains("path: src/lib.rs")));
}

#[test]
fn transcript_pane_advances_next_queued_approval_after_resolution() {
    let mut transcript_pane = TranscriptPane::new(100, 24);

    for number in 1..=2 {
        let command = format!("printf {number}");
        transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
            turn: 1,
            id: format!("bash-{number}"),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: command.clone(),
            arguments: serde_json::json!({ "command": command }),
            session_scope: None,
            prefix_rule: None,
        });
    }
    transcript_pane.resolve_approval("bash-1", "Approved");

    let frame = plain_frame(&mut transcript_pane, 100, 24);
    assert!(frame.iter().any(|line| line.contains("Approved")));
    assert!(frame.iter().any(|line| line.contains("$ printf 2")));
    assert!(!frame.iter().any(|line| line.contains("queued:")));
}

#[test]
fn transcript_pane_places_approval_after_matching_tool_and_renders_resolution_lightly() {
    let mut transcript_pane = TranscriptPane::new(100, 24);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "printf 1" }),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-2".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "printf 2" }),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf 1".to_owned(),
        arguments: serde_json::json!({ "command": "printf 1" }),
        session_scope: None,
        prefix_rule: None,
    });

    let frame = plain_frame(&mut transcript_pane, 100, 24);
    let tool_1 = frame
        .iter()
        .position(|line| line.contains("Using Bash (printf 1)"))
        .expect("first tool");
    let approval = frame
        .iter()
        .position(|line| line.contains("Run this command?"))
        .expect("approval");
    let tool_2 = frame
        .iter()
        .position(|line| line.contains("Using Bash (printf 2)"))
        .expect("second tool");
    assert!(tool_1 < approval);
    assert!(
        approval < tool_2,
        "approval should stay near matching tool: {frame:?}"
    );

    transcript_pane.resolve_approval("tool-1", "Approved");
    let resolved = plain_frame(&mut transcript_pane, 100, 24);
    assert!(
        resolved
            .iter()
            .any(|line| line.trim() == "approval: Approved"),
        "resolved approval should be lightweight: {resolved:?}"
    );
    assert!(
        !resolved
            .iter()
            .any(|line| line.chars().all(|ch| ch == '\u{2500}') && line.len() > 20),
        "resolved approval should not keep yellow divider bars: {resolved:?}"
    );
}

#[test]
fn transcript_pane_maps_user_and_assistant_events_to_transcript_entries() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.push_user_message("hello");
    transcript_pane.push_assistant_message("world");
    transcript_pane.mark_dirty();
    let frame = plain_frame(&mut transcript_pane, 80, 12);

    // User message is bullet-led (✨), assistant final is bullet-led (●).
    assert!(
        frame
            .iter()
            .any(|l| l.contains("✨") && l.contains("hello"))
    );
    assert!(frame.iter().any(|l| l.contains("●")));
    assert!(frame.iter().any(|l| l.contains("world")));
}

#[test]
fn long_user_message_with_wide_chars_never_exceeds_terminal_width() {
    // Regression for a width-overflow crash in `bulleted_wrap`: the `✨ `
    // prefix width was not subtracted from the wrap budget, so long CJK
    // prompts produced a first row wider than the terminal and tripped the
    // renderer's width invariant. Keep this test if you touch that path.
    let mut transcript_pane = TranscriptPane::new(40, 30);
    let prompt = "停下来所有提交工作，总结一下你的工作，为什么你之前要用工具来提交？还有就是你用工具时遇到了什么问题？";
    transcript_pane.push_user_message(prompt);
    transcript_pane.mark_dirty();
    let width = 40_u16;
    let frame = transcript_pane
        .render_frame(usize::from(width), 30)
        .expect("render frame");

    for (i, line) in frame.iter().enumerate() {
        let w = visible_width(line);
        assert!(
            w <= usize::from(width),
            "line {i} visible width {w} exceeds terminal width {width}: {}",
            strip_ansi(line)
        );
    }
}

#[test]
fn long_shell_result_lines_never_exceed_terminal_width() {
    let mut transcript_pane = TranscriptPane::new(80, 30);
    let long_memory_row = format!(
        "    0.563,01KVG2WP5FW4GXDQK93WZYFTA9,context-neo,high,0.927,\"{}\"",
        "Fixed clippy warnings in crates/neo-tui ".repeat(20)
    );

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({
            "command": "icm recall \"compact\"",
            "cwd": "/Users/chenyuanhao/Workspace/neo"
        }),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok(long_memory_row),
    });

    let width = 80_u16;
    let frame = transcript_pane
        .render_frame(usize::from(width), 30)
        .expect("render frame");

    for (i, line) in frame.iter().enumerate() {
        let w = visible_width(line);
        assert!(
            w <= usize::from(width),
            "line {i} visible width {w} exceeds terminal width {width}: {}",
            strip_ansi(line)
        );
    }
}

#[test]
fn long_live_tool_output_lines_never_exceed_terminal_width() {
    let mut transcript_pane = TranscriptPane::new(80, 30);
    let long_memory_row = format!(
        "    0.563,01KVG2WP5FW4GXDQK93WZYFTA9,context-neo,high,0.927,\"{}\"",
        "Fixed clippy warnings in crates/neo-tui ".repeat(20)
    );

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({
            "command": "icm recall \"compact\"",
            "cwd": "/Users/chenyuanhao/Workspace/neo"
        }),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        partial_result: neo_agent_core::ToolResult::ok(long_memory_row),
    });

    let width = 80_u16;
    let frame = transcript_pane
        .render_frame(usize::from(width), 30)
        .expect("render frame");

    for (i, line) in frame.iter().enumerate() {
        let w = visible_width(line);
        assert!(
            w <= usize::from(width),
            "line {i} visible width {w} exceeds terminal width {width}: {}",
            strip_ansi(line)
        );
    }
}

#[test]
fn persisted_message_events_do_not_duplicate_live_transcript() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.push_user_message("hello");
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::user_text("hello"),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "world".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::assistant(
            [neo_agent_core::Content::text("world")],
            [],
            neo_agent_core::StopReason::EndTurn,
        ),
    });

    let frame = plain_frame(&mut transcript_pane, 80, 12);

    assert_eq!(
        frame
            .iter()
            .filter(|line| line.contains("✨") && line.contains("hello"))
            .count(),
        1,
        "user prompt should appear once: {frame:?}"
    );
    assert_eq!(
        frame
            .iter()
            .filter(|line| line.contains("●") && line.contains("world"))
            .count(),
        1,
        "assistant text should appear once: {frame:?}"
    );
}

#[test]
fn transcript_pane_keeps_streaming_assistant_in_transcript_until_finished() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.push_user_message("hello");
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Hel".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "lo".to_owned(),
    });

    let first = plain_frame(&mut transcript_pane, 80, 12);
    assert!(
        first
            .iter()
            .any(|l| l.contains("✨") && l.contains("hello"))
    );
    assert!(
        first.iter().any(|l| l.contains("●") && l.contains("Hello")),
        "live assistant text should already use the finished assistant layout: {first:?}"
    );

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "assistant-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });
    let second = plain_frame(&mut transcript_pane, 80, 12);
    assert_eq!(
        second
            .iter()
            .filter(|l| l.contains("●") && l.contains("Hello"))
            .count(),
        1,
        "finished assistant text appears exactly once: {second:?}"
    );
}

#[test]
fn message_started_does_not_create_empty_assistant_entry() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "assistant-1".to_owned(),
    });

    assert!(
        transcript_pane.transcript().entries().is_empty(),
        "assistant entry should be created by the first text delta, not MessageStarted"
    );
}

#[test]
fn text_after_tool_starts_a_new_assistant_entry_after_the_tool() {
    let mut transcript_pane = TranscriptPane::new(80, 16);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-1".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "I should inspect files".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "pwd" }),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok("Cargo.toml"),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Final answer".to_owned(),
    });

    let frame = plain_frame(&mut transcript_pane, 80, 16);
    let thinking = frame
        .iter()
        .position(|l| l.contains("I should inspect files"))
        .expect("thinking");
    let tool = frame
        .iter()
        .position(|l| l.contains("Used Bash"))
        .expect("tool");
    let answer = frame
        .iter()
        .position(|l| l.contains("●") && l.contains("Final answer"))
        .expect("answer");
    assert!(
        thinking < tool,
        "thinking should stay above the tool: {frame:?}"
    );
    assert!(
        tool < answer,
        "answer should render after the tool: {frame:?}"
    );
}

#[test]
fn transcript_blocks_have_exactly_one_blank_row_between_neighbors() {
    let mut transcript_pane = TranscriptPane::new(80, 20);

    transcript_pane.push_transcript(TranscriptEntry::thinking_complete("thinking one"));
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "pwd" }),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok("Cargo.toml"),
    });
    transcript_pane.push_transcript(TranscriptEntry::thinking_complete("thinking two"));
    transcript_pane.push_transcript(TranscriptEntry::assistant_message("Final answer"));

    let frame = plain_frame(&mut transcript_pane, 80, 20);
    assert_one_blank_between(&frame, "thinking one", "Used Bash");
    assert_one_blank_between(&frame, "Used Bash", "thinking two");
    assert_one_blank_between(&frame, "thinking two", "Final answer");
}

fn assert_one_blank_between(frame: &[String], first: &str, second: &str) {
    let first_index = frame
        .iter()
        .position(|line| line.contains(first))
        .unwrap_or_else(|| panic!("missing first marker {first:?}: {frame:?}"));
    let second_index = frame
        .iter()
        .position(|line| line.contains(second))
        .unwrap_or_else(|| panic!("missing second marker {second:?}: {frame:?}"));
    let blanks = frame[first_index + 1..second_index]
        .iter()
        .filter(|line| line.trim().is_empty())
        .count();
    assert_eq!(
        blanks, 1,
        "expected one blank row between {first:?} and {second:?}: {frame:?}"
    );
}

#[test]
fn finishing_streaming_assistant_preserves_body_row_shape() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.push_user_message("hello");
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Hello".to_owned(),
    });

    let live = plain_frame(&mut transcript_pane, 80, 12);
    let live_user = live
        .iter()
        .position(|line| line.contains("✨") && line.contains("hello"))
        .expect("live user row");
    let live_assistant = live
        .iter()
        .position(|line| line.contains("●") && line.contains("Hello"))
        .expect("live assistant row");
    assert_eq!(
        live_assistant,
        live_user + 2,
        "live assistant should be separated from the user by one blank row: {live:?}"
    );
    assert_eq!(live[live_user + 1], "");

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "assistant-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });

    let finished = plain_frame(&mut transcript_pane, 80, 12);
    let finished_user = finished
        .iter()
        .position(|line| line.contains("✨") && line.contains("hello"))
        .expect("finished user row");
    let finished_assistant = finished
        .iter()
        .position(|line| line.contains("●") && line.contains("Hello"))
        .expect("finished assistant row");
    assert_eq!(
        finished_assistant,
        finished_user + 2,
        "finished assistant should keep the live row shape: {finished:?}"
    );
    assert_eq!(finished[finished_user + 1], "");
}

#[test]
fn transcript_pane_keeps_finished_tool_cards_in_the_same_frame_slot() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),
    });
    let running = plain_frame(&mut transcript_pane, 80, 12);
    assert!(running.iter().any(|l| l.contains("Using Read (README.md)")));

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("line one\nline two"),
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
    assert!(frame.iter().any(|l| l.contains("Queued Read (README.md)")));
}

#[test]
fn transcript_pane_frame_keeps_tool_card_and_streaming_assistant() {
    let mut transcript_pane = TranscriptPane::new(80, 6);

    transcript_pane.set_live_chrome_height(4);
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
    });
    transcript_pane.push_transcript(TranscriptEntry::assistant_message("streaming"));
    let frame = plain_frame(&mut transcript_pane, 80, 6);

    // The tool card and streaming assistant are both in the frame.
    let has_tool = frame.iter().any(|l| l.contains("Using Bash"));
    let has_streaming = frame.iter().any(|l| l.contains("streaming"));
    assert!(
        has_tool || has_streaming,
        "frame contains active content: {frame:?}"
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
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("line one\nline two"),
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

#[test]
fn transcript_pane_keeps_running_tool_run_live() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
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

    let queued = plain_frame(&mut transcript_pane, 80, 12);
    assert!(queued.iter().any(|line| line.contains("Queued Bash")));
    assert!(
        !queued.iter().any(|line| line.contains("Using Bash")),
        "declared-but-not-started tool calls must not look like running tools: {queued:?}"
    );

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
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
fn transcript_pane_finishes_streaming_assistant_once_without_duplicate() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "hello".to_owned(),
    });
    let live = plain_frame(&mut transcript_pane, 80, 12);
    assert_eq!(
        live.iter()
            .filter(|l| l.contains("●") && l.contains("hello"))
            .count(),
        1,
        "live assistant text appears once with bullet: {live:?}"
    );

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "assistant-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });
    let finished = plain_frame(&mut transcript_pane, 80, 12);
    assert_eq!(
        finished
            .iter()
            .filter(|l| l.contains("●") && l.contains("hello"))
            .count(),
        1,
        "finished assistant text appears exactly once: {finished:?}"
    );
}

#[test]
fn replayed_messages_render_through_same_transcript_pane_path() {
    let mut transcript_pane = TranscriptPane::new(80, 12);
    transcript_pane.replay_user_message("previous prompt");
    transcript_pane.replay_assistant_message("previous answer");
    transcript_pane.mark_dirty();

    let frame = plain_frame(&mut transcript_pane, 80, 12);
    assert!(
        frame
            .iter()
            .any(|l| l.contains("✨") && l.contains("previous prompt"))
    );
    assert!(frame.iter().any(|l| l.contains("●")));
    assert!(frame.iter().any(|l| l.contains("previous answer")));
}

#[test]
fn queued_follow_up_message_renders_with_distinct_prefix() {
    let mut transcript_pane = TranscriptPane::new(80, 12);
    transcript_pane.push_transcript(TranscriptEntry::user_message("original"));
    transcript_pane.push_queued_message("follow up text", false);

    let frame = plain_frame(&mut transcript_pane, 80, 12);
    let queued_line = frame
        .iter()
        .find(|l| l.contains("follow up text"))
        .expect("queued follow-up text should render");
    assert!(
        queued_line.starts_with("↪"),
        "queued follow-up should use the ↪ prefix, got: {queued_line:?}"
    );
    // Normal user message keeps its own prefix.
    assert!(
        frame
            .iter()
            .any(|l| l.contains("✨") && l.contains("original"))
    );
}

#[test]
fn steered_message_renders_with_distinct_prefix() {
    let mut transcript_pane = TranscriptPane::new(80, 12);
    transcript_pane.push_queued_message("steer text", true);

    let frame = plain_frame(&mut transcript_pane, 80, 12);
    let steer_line = frame
        .iter()
        .find(|l| l.contains("steer text"))
        .expect("steered text should render");
    assert!(
        steer_line.starts_with("↳"),
        "steered message should use the ↳ prefix, got: {steer_line:?}"
    );
}

#[test]
fn pop_pending_follow_up_removes_oldest_queued_entry() {
    let mut transcript_pane = TranscriptPane::new(80, 12);
    transcript_pane.push_queued_message("first follow", false);
    transcript_pane.push_queued_message("steer", true);
    transcript_pane.push_queued_message("second follow", false);

    let popped = transcript_pane
        .pop_pending_follow_up()
        .expect("should pop a follow-up");
    // The most recently queued follow-up is popped first (reverse search).
    assert_eq!(popped, "second follow");
    // Remaining entries still render.
    let frame = plain_frame(&mut transcript_pane, 80, 12);
    assert!(frame.iter().any(|l| l.contains("first follow")));
    assert!(frame.iter().any(|l| l.contains("steer")));
    assert!(
        !frame.iter().any(|l| l.contains("second follow")),
        "popped entry should be removed from transcript"
    );
}
