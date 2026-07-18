use neo_agent_core::instructions::{
    InstructionBundleMetadata, InstructionEpochData, InstructionEpochOutcome, InstructionScopeData,
    InstructionScopeKind,
};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Color, Finalization, strip_ansi, visible_width};
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::{
    McpStartupPhase, McpStartupStatusData, StatusSeverity, TranscriptBrowserState, TranscriptEntry,
    TranscriptPane,
};

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

fn schedule_and_resume_retry(pane: &mut TranscriptPane, turn: u32) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: connection reset".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn,
        retry: 1,
        max_retries: 5,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryResumed { turn, retry: 1 });
}

fn ansi_for_color(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        Color::Indexed(index) => format!("\x1b[38;5;{index}m"),
        Color::Green => "\x1b[32m".to_owned(),
        Color::Yellow => "\x1b[33m".to_owned(),
        Color::Red => "\x1b[31m".to_owned(),
        other => panic!("test helper does not support color {other:?}"),
    }
}

#[test]
fn streaming_assistant_commits_stable_prefix_and_bounds_live_tail() {
    let mut pane = TranscriptPane::new(40, 8);
    pane.start_assistant_message();
    for index in 0..8 {
        pane.append_assistant_delta(&format!("complete paragraph {index}\n\n"));
    }
    pane.append_assistant_delta("mutable tail that is still streaming");

    let update = pane.render_terminal_update(40, 8);
    let history = update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    let live = update
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(history.contains("complete paragraph 0"));
    assert!(!live.contains("complete paragraph 0"));
    assert!(live.contains("mutable tail"));
    assert!(update.live.len() <= 4, "live rows must fit above chrome");
}

#[test]
fn long_unstable_assistant_tail_is_truncated_inside_live_budget() {
    let mut pane = TranscriptPane::new(30, 8);
    pane.start_assistant_message();
    pane.append_assistant_delta(&"unfinished ".repeat(80));

    let update = pane.render_terminal_update(30, 8);
    let live = update
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(update.history.is_empty());
    assert!(update.live.len() <= 4, "live rows must fit above chrome");
    assert!(live.contains("earlier rows omitted"));
}

#[test]
fn consecutive_streaming_tool_cards_keep_each_header_when_live_budget_truncates() {
    let mut pane = TranscriptPane::new(80, 8);
    for (id, path) in [("read-1", "one.rs"), ("read-2", "two.rs")] {
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: id.to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({"path": path}),
        });
    }
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "read-2".to_owned(),
        name: "Read".to_owned(),
        partial_result: neo_agent_core::ToolResult::ok(
            "output-1\noutput-2\noutput-3\noutput-4\noutput-5\nlatest-output",
        ),
    });

    let update = pane.render_terminal_update(80, 8);
    let live = update
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(live.matches("Using Read").count(), 2, "live:\n{live}");
    assert!(live.contains("one.rs"), "live:\n{live}");
    assert!(live.contains("two.rs"), "live:\n{live}");
    assert!(live.contains("latest-output"), "live:\n{live}");
    assert!(update.live.len() <= 4, "live rows: {:#?}", update.live);
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
fn mcp_startup_status_updates_pending_spinner_to_green_connected_row() {
    let theme = TuiTheme::default().with_status_ok(Color::Rgb(1, 180, 90));
    let mut transcript_pane = TranscriptPane::new(100, 12);
    transcript_pane.set_theme(theme);

    transcript_pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        phase: McpStartupPhase::Connecting,
    });
    transcript_pane.advance_animation_at_ms(80);

    let pending = plain_frame(&mut transcript_pane, 100, 12);
    assert!(
        pending
            .iter()
            .any(|line| line.contains("MCP server \"linear\" connecting")),
        "pending frame: {pending:?}"
    );
    assert_eq!(
        transcript_pane
            .transcript()
            .entries()
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::McpStartupStatus { .. }))
            .count(),
        1
    );

    transcript_pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        phase: McpStartupPhase::Connected { tool_count: 47 },
    });
    let _ = transcript_pane.render_frame(100, 12);

    let connected_ansi = transcript_pane.frame_ansi_lines().join("\n");
    let connected_plain = strip_ansi(&connected_ansi);
    assert!(
        connected_plain.contains("MCP server \"linear\" connected · 47 tools (http)"),
        "{connected_plain}"
    );
    assert!(
        connected_ansi.contains(&ansi_for_color(theme.status_ok)),
        "{connected_ansi}"
    );
    assert_eq!(
        transcript_pane
            .transcript()
            .entries()
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::McpStartupStatus { .. }))
            .count(),
        1
    );
}

#[test]
fn mcp_startup_status_updates_pending_spinner_to_interrupted_row() {
    let mut transcript_pane = TranscriptPane::new(100, 12);
    transcript_pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        phase: McpStartupPhase::Connecting,
    });
    transcript_pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        phase: McpStartupPhase::Cancelled,
    });

    let rendered = plain_frame(&mut transcript_pane, 100, 12).join("\n");
    assert!(
        rendered.contains("MCP server \"linear\" startup interrupted (http)"),
        "{rendered}"
    );
    assert!(!rendered.contains("connecting..."), "{rendered}");
}

#[test]
fn mcp_startup_status_updates_pending_spinner_to_red_failed_row() {
    let theme = TuiTheme::default().with_status_error(Color::Rgb(211, 37, 69));
    let mut transcript_pane = TranscriptPane::new(100, 12);
    transcript_pane.set_theme(theme);
    transcript_pane.set_live_chrome_height(0);
    transcript_pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        phase: McpStartupPhase::Connecting,
    });

    let pending = transcript_pane.render_terminal_update(100, 12);
    assert!(pending.history.is_empty());
    assert!(
        pending
            .live
            .iter()
            .map(|line| strip_ansi(line))
            .any(|line| line.contains("MCP server \"linear\" connecting"))
    );

    transcript_pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        phase: McpStartupPhase::Failed {
            message: "timeout connecting to server".to_owned(),
        },
    });
    let failed = transcript_pane.render_terminal_update(100, 12);
    let failed_ansi = failed
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let failed_plain = strip_ansi(&failed_ansi);

    assert!(
        failed_plain.contains("✗ MCP server \"linear\" failed · timeout connecting to server"),
        "{failed_plain}"
    );
    assert!(
        failed_ansi.contains(&ansi_for_color(theme.status_error)),
        "{failed_ansi}"
    );
    assert!(
        failed
            .live
            .iter()
            .map(|line| strip_ansi(line))
            .all(|line| !line.contains("connecting"))
    );
    assert_eq!(
        transcript_pane
            .transcript()
            .entries()
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::McpStartupStatus { .. }))
            .count(),
        1
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
        suggestions: vec![],
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
            suggestions: vec![],
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
        suggestions: vec![],
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
        suggestions: vec![],
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
        suggestions: vec![],
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
            suggestions: vec![],
        });
    }
    transcript_pane.resolve_approval("bash-1", "Approved");

    let frame = plain_frame(&mut transcript_pane, 100, 24);
    assert!(frame.iter().any(|line| line.contains("Approved")));
    assert!(frame.iter().any(|line| line.contains("$ printf 2")));
    assert!(!frame.iter().any(|line| line.contains("queued:")));
}

#[test]
fn finalizing_transcript_drops_queued_approvals_before_exit() {
    let mut transcript_pane = TranscriptPane::new(100, 24);

    for number in 1..=2 {
        let command = format!("printf {number}");
        transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
            turn: 1,
            id: format!("historical-{number}"),
            operation: neo_agent_core::PermissionOperation::Shell,
            subject: command.clone(),
            arguments: serde_json::json!({ "command": command }),
            session_scope: None,
            prefix_rule: None,
            suggestions: vec![],
        });
    }
    transcript_pane.finalize_interrupted_live_entries();

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 2,
        id: "current".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf current".to_owned(),
        arguments: serde_json::json!({ "command": "printf current" }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });
    transcript_pane.resolve_approval("current", "Approved");

    let ids = transcript_pane
        .transcript()
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::ApprovalPrompt(data) => Some(data.id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(ids.contains(&"historical-1"));
    assert!(ids.contains(&"current"));
    assert!(
        !ids.contains(&"historical-2"),
        "queued approval resurrected: {ids:?}"
    );
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
        suggestions: vec![],
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
fn replay_skips_injection_origin_messages() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.replay_message(&neo_agent_core::AgentMessage::injection_text(
        "Plan mode is active. This should stay model-only.",
        "plan_mode",
    ));

    let Some(rendered) = transcript_pane.render_frame(80, 12) else {
        return;
    };
    let frame = rendered.iter().map(|line| plain(line)).collect::<Vec<_>>();

    assert!(
        frame.iter().all(
            |line| !line.contains("<system-reminder>") && !line.contains("Plan mode is active")
        ),
        "runtime system reminder should not be rendered in transcript: {frame:?}"
    );
}

#[test]
fn replay_renders_user_text_that_looks_like_system_reminder() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.replay_message(&neo_agent_core::AgentMessage::user_text(
        "<system-reminder>\nliteral user text\n</system-reminder>",
    ));

    let frame = plain_frame(&mut transcript_pane, 80, 12);

    assert!(
        frame.iter().any(|line| line.contains("<system-reminder>"))
            && frame.iter().any(|line| line.contains("literal user text")),
        "literal user text should render even when it resembles a system reminder: {frame:?}"
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
fn canonical_provider_error_codes_use_expected_severity() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.apply_agent_event(neo_agent_core::AgentEvent::Error {
        turn: 1,
        message: "connection reset".to_owned(),
        code: Some("provider.transport_error".to_owned()),
        retry_after: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::Error {
        turn: 2,
        message: "malformed stream".to_owned(),
        code: Some("provider.protocol_error".to_owned()),
        retry_after: None,
    });

    let severities = pane
        .transcript()
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Status {
                severity: Some(severity),
                ..
            } => Some(*severity),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        severities,
        vec![StatusSeverity::Warning, StatusSeverity::Error]
    );
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
    assert!(!frame.iter().any(|line| line.contains("Queued Bash")));
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

#[test]
fn skill_tool_call_renders_as_skill_activation_card_not_tool_card() {
    let mut pane = TranscriptPane::new(80, 20);
    // Simulate the full Skill tool-call lifecycle.
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "skill-1".to_owned(),
        name: "Skill".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "skill-1".to_owned(),
        name: "Skill".to_owned(),
        arguments: serde_json::json!({ "skill": "brainstorming" }),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "skill-1".to_owned(),
        name: "Skill".to_owned(),
        result: neo_agent_core::ToolResult::ok("expanded skill body"),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::SkillInvocation {
        names: vec!["brainstorming".to_owned()],
        source: neo_agent_core::SkillInvocationSource::Auto,
        outcome: neo_agent_core::SkillInvocationOutcome::Activated,
        body: String::new(),
    });

    let entries = pane.transcript().entries();
    // No ToolRun entry should exist for the Skill tool.
    assert!(
        !entries
            .iter()
            .any(|e| matches!(e, TranscriptEntry::ToolRun { .. })),
        "Skill tool should not produce a ToolRun entry"
    );
    // A SkillActivation card should be present.
    let skill_card = entries
        .iter()
        .find(|e| matches!(e, TranscriptEntry::SkillActivation { .. }))
        .expect("SkillActivation card should exist");
    assert!(
        matches!(
            skill_card,
            TranscriptEntry::SkillActivation { names, .. }
                if names == &vec!["brainstorming".to_owned()]
        ),
        "skill card should name brainstorming"
    );
    let frame = plain_frame(&mut pane, 80, 20);
    assert!(
        frame
            .iter()
            .any(|line| line.contains("✦ Skill activated: brainstorming · auto")),
        "semantic header should include the automatic source: {frame:#?}"
    );
    assert!(
        frame.iter().all(|line| !line.contains('━')),
        "an empty activation body should not render a divider: {frame:#?}"
    );
}

#[test]
fn skill_tool_with_arguments_shows_them_in_activation_body() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::SkillInvocation {
        names: vec!["review".to_owned()],
        source: neo_agent_core::SkillInvocationSource::Auto,
        outcome: neo_agent_core::SkillInvocationOutcome::Activated,
        body: "target: src/lib.rs".to_owned(),
    });

    let entries = pane.transcript().entries();
    let card = entries
        .iter()
        .find(|e| matches!(e, TranscriptEntry::SkillActivation { .. }))
        .expect("SkillActivation card");
    assert!(
        matches!(
            card,
            TranscriptEntry::SkillActivation { body, .. }
                if body == "target: src/lib.rs"
        ),
        "body should contain formatted arguments"
    );
}

#[test]
fn failed_skill_tool_renders_semantic_failure_card() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "skill-1".to_owned(),
        name: "Skill".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "skill-1".to_owned(),
        name: "Skill".to_owned(),
        result: neo_agent_core::ToolResult::error("skill `missing` is not available"),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::SkillInvocation {
        names: vec!["missing".to_owned()],
        source: neo_agent_core::SkillInvocationSource::Auto,
        outcome: neo_agent_core::SkillInvocationOutcome::Failed,
        body: "skill `missing` is not available".to_owned(),
    });

    assert!(
        pane.transcript()
            .entries()
            .iter()
            .all(|entry| !matches!(entry, TranscriptEntry::ToolRun { .. })),
        "failed Skill calls should not leak a generic tool card"
    );
    let frame = plain_frame(&mut pane, 80, 20);
    assert!(
        frame
            .iter()
            .any(|line| line.contains("✕ Skill failed: missing · auto")),
        "failure header should be visible: {frame:#?}"
    );
    assert!(
        frame
            .iter()
            .any(|line| line.contains("  skill `missing` is not available")),
        "failure body should be indented: {frame:#?}"
    );
    assert!(
        frame.iter().all(|line| !line.contains('━')),
        "failure cards should not render a divider: {frame:#?}"
    );
}

#[test]
fn skill_activation_toggle_expands_collapsed_body() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::SkillInvocation {
        names: vec!["review".to_owned()],
        source: neo_agent_core::SkillInvocationSource::Auto,
        outcome: neo_agent_core::SkillInvocationOutcome::Activated,
        body: "one\ntwo\nthree\nfour\nfive".to_owned(),
    });

    let collapsed = plain_frame(&mut pane, 80, 20);
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("ctrl+o to expand"))
    );

    assert!(pane.toggle_tool_output_expanded());
    let expanded = plain_frame(&mut pane, 80, 20);
    assert!(expanded.iter().any(|line| line.contains("five")));
    assert!(
        expanded
            .iter()
            .all(|line| !line.contains("ctrl+o to expand"))
    );
}

#[test]
fn browser_snapshot_expands_and_collapses_committed_tool_without_mutating_source() {
    let mut pane = TranscriptPane::new(80, 20);
    let transcript = pane.transcript_mut();
    transcript.push_tool_run("tool-1", "Read", Some("{\"path\":\"a\"}".to_owned()));
    assert!(transcript.mutate_tool("tool-1", |tool| {
        tool.set_result(Some("ok".to_owned()), None, false, None)
    }));

    let committed = pane.render_terminal_update(80, 20);
    assert!(!committed.history.is_empty());
    assert!(!pane.has_committed_expandable_entries());
    pane.acknowledge_history(&committed.history);
    assert!(pane.has_committed_expandable_entries());
    assert!(!pane.tool_output_expanded());

    pane.push_status("after-browser");
    let mut browser = TranscriptBrowserState::new(true);

    let expanded = pane.render_browser_rows(&mut browser, 80, 20).join("\n");
    assert!(expanded.contains("{\"path\":\"a\"}"));
    assert!(pane.has_committed_expandable_entries());
    assert!(!pane.tool_output_expanded());

    browser.toggle();
    let collapsed = pane.render_browser_rows(&mut browser, 80, 20).join("\n");
    assert!(!collapsed.contains("{\"path\":\"a\"}"));
    assert!(pane.has_committed_expandable_entries());
    assert!(!pane.tool_output_expanded());

    let native = pane.render_terminal_update(80, 20);
    let native_history = native
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(native_history.contains("after-browser"));
    assert!(!native_history.contains("{\"path\":\"a\"}"));
}

#[test]
fn browser_rows_are_bounded_and_scrollable() {
    let mut pane = TranscriptPane::new(80, 20);
    for index in 0..40 {
        pane.push_status(format!("row-{index}"));
    }
    let mut browser = TranscriptBrowserState::new(false);
    assert_eq!(pane.render_browser_rows(&mut browser, 80, 5).len(), 5);
    browser.scroll_up(usize::MAX);
    assert!(
        pane.render_browser_rows(&mut browser, 80, 5)
            .join("\n")
            .contains("row-0")
    );
}

#[test]
fn retry_status_renders_fixed_waiting_connecting_and_exhausted_states() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: error decoding response body".to_owned(),
    });

    let waiting_frame_0 = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(
        waiting_frame_0.contains("⠋ Reconnecting 1/5 · retry in 12s · esc interrupt"),
        "waiting retry status: {waiting_frame_0}"
    );
    assert_eq!(
        waiting_frame_0
            .matches("Network · error decoding response body")
            .count(),
        1,
        "waiting retry detail: {waiting_frame_0}"
    );
    assert!(!waiting_frame_0.contains("Network · transport error:"));
    pane.advance_animation_at_ms(80);
    let waiting_frame_1 = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(
        waiting_frame_1.contains("⠙ Reconnecting 1/5 · retry in 12s · esc interrupt"),
        "waiting retry animation: {waiting_frame_1}"
    );

    let mut connecting_pane = TranscriptPane::new(80, 20);
    connecting_pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: error decoding response body".to_owned(),
    });
    connecting_pane.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn: 1,
        retry: 1,
        max_retries: 5,
    });
    let connecting_frame_0 = plain_frame(&mut connecting_pane, 80, 20).join("\n");
    assert!(
        connecting_frame_0.contains("⠋ Reconnecting 1/5 · connecting · esc interrupt"),
        "connecting retry status: {connecting_frame_0}"
    );
    connecting_pane.advance_animation_at_ms(80);
    let connecting_frame_1 = plain_frame(&mut connecting_pane, 80, 20).join("\n");
    assert!(
        connecting_frame_1.contains("⠙ Reconnecting 1/5 · connecting · esc interrupt"),
        "connecting retry animation: {connecting_frame_1}"
    );

    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryExhausted {
        turn: 1,
        retries_used: 5,
        error_code: "provider.transport_error".to_owned(),
        message: "error decoding response body".to_owned(),
    });
    let exhausted = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(
        exhausted.contains("Reconnect failed after 5 retries"),
        "exhausted retry status: {exhausted}"
    );
    assert!(
        exhausted.contains("Network · error decoding response body"),
        "exhausted retry detail: {exhausted}"
    );

    for (turn, retries_used, expected) in [
        (3, 0, "Reconnect failed · retry disabled"),
        (4, 1, "Reconnect failed after 1 retry"),
    ] {
        let mut terminal = TranscriptPane::new(80, 20);
        terminal.apply_agent_event(neo_agent_core::AgentEvent::RetryExhausted {
            turn,
            retries_used,
            error_code: "provider.transport_error".to_owned(),
            message: String::new(),
        });
        let rendered = plain_frame(&mut terminal, 80, 20).join("\n");
        assert!(
            rendered.contains(expected),
            "terminal retry status: {rendered}"
        );
    }

    let mut high_attempt = TranscriptPane::new(80, 20);
    high_attempt.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 2,
        retry: 99,
        max_retries: 100,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "connection reset".to_owned(),
    });
    let waiting = plain_frame(&mut high_attempt, 80, 20).join("\n");
    high_attempt.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn: 2,
        retry: 99,
        max_retries: 100,
    });
    let connecting = plain_frame(&mut high_attempt, 80, 20).join("\n");
    assert!(waiting.contains("Reconnecting 99/100 · retry in 12s"));
    assert!(connecting.contains("Reconnecting 99/100 · connecting"));
    high_attempt.apply_agent_event(neo_agent_core::AgentEvent::RetrySucceeded {
        turn: 2,
        retries_used: 99,
    });
    assert!(
        high_attempt
            .transcript()
            .entries()
            .iter()
            .all(|entry| !matches!(entry, TranscriptEntry::RetryStatus { .. }))
    );
}

#[test]
fn retry_status_mutates_original_position() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.push_user_message("question");
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "attempt-1".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "discard me".to_owned(),
    });
    let original_id = pane.transcript().entry_ids()[1];

    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "error decoding response body".to_owned(),
    });
    assert_eq!(pane.transcript().entries().len(), 2);
    assert_eq!(pane.transcript().entry_ids()[1], original_id);
    assert!(matches!(
        pane.transcript().entries()[1],
        TranscriptEntry::RetryStatus { .. }
    ));

    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn: 1,
        retry: 1,
        max_retries: 5,
    });
    assert_eq!(pane.transcript().entry_ids()[1], original_id);

    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryResumed { turn: 1, retry: 1 });
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "attempt-2".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "replacement".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetrySucceeded {
        turn: 1,
        retries_used: 1,
    });

    assert_eq!(pane.transcript().entries().len(), 2);
    assert_eq!(pane.transcript().entry_ids()[1], original_id);
    assert!(matches!(
        &pane.transcript().entries()[1],
        TranscriptEntry::AssistantMessage { content } if content == "replacement"
    ));

    let mut exhausted = TranscriptPane::new(80, 20);
    exhausted.push_user_message("question");
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 2,
        text: "first partial".to_owned(),
    });
    let original_id = exhausted.transcript().entry_ids()[1];
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 2,
        retry: 1,
        max_retries: 1,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "connection reset".to_owned(),
    });
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn: 2,
        retry: 1,
        max_retries: 1,
    });
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::RetryResumed { turn: 2, retry: 1 });
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 2,
        text: "last partial".to_owned(),
    });
    exhausted.apply_agent_event(neo_agent_core::AgentEvent::RetryExhausted {
        turn: 2,
        retries_used: 1,
        error_code: "provider.transport_error".to_owned(),
        message: "connection reset".to_owned(),
    });

    assert_eq!(exhausted.transcript().entries().len(), 2);
    assert_eq!(exhausted.transcript().entry_ids()[1], original_id);
    assert!(matches!(
        &exhausted.transcript().entries()[1],
        TranscriptEntry::RetryStatus { data }
            if data.phase == neo_tui::transcript::entry::RetryPhase::Exhausted
                && data.message == "connection reset"
    ));
}

#[test]
fn retry_attempt_stays_out_of_terminal_history_until_message_finishes() {
    let mut pane = TranscriptPane::new(40, 8);
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "attempt-1".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-1".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "failed reasoning".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "failed answer prefix\n\nfailed mutable tail".to_owned(),
    });

    let provisional = pane.render_terminal_update(40, 8);
    let provisional_live = provisional
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    let mut terminal_history = provisional
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(provisional_live.contains("failed mutable tail"));
    pane.acknowledge_history(&provisional.history);

    schedule_and_resume_retry(&mut pane, 1);
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "attempt-2".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "winning answer".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "attempt-2".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });

    let finished = pane.render_terminal_update(40, 8);
    terminal_history.push('\n');
    terminal_history.push_str(
        &finished
            .history
            .iter()
            .flat_map(|block| block.lines.iter())
            .map(|line| strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    pane.acknowledge_history(&finished.history);
    assert!(pane.render_terminal_update(40, 8).history.is_empty());

    assert!(
        !terminal_history.contains("failed reasoning"),
        "{terminal_history}"
    );
    assert!(
        !terminal_history.contains("failed answer prefix"),
        "{terminal_history}"
    );
    assert_eq!(
        terminal_history.matches("winning answer").count(),
        1,
        "{terminal_history}"
    );
}

#[test]
fn retry_thinking_first_reuses_anchor_before_intervening_finalized_entry() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "failed answer".to_owned(),
    });
    let anchor_id = pane.transcript().entry_ids()[0];
    schedule_and_resume_retry(&mut pane, 1);
    pane.transcript_mut()
        .push(TranscriptEntry::status("intervening"));
    let intervening_id = pane.transcript().entry_ids()[1];

    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-2".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "winning reasoning".to_owned(),
    });

    assert_eq!(pane.transcript().entries().len(), 2);
    assert_eq!(pane.transcript().entry_ids(), &[anchor_id, intervening_id]);
    assert!(matches!(
        &pane.transcript().entries()[0],
        TranscriptEntry::ThinkingBlock { content, .. } if content == "winning reasoning"
    ));
    assert!(matches!(
        &pane.transcript().entries()[1],
        TranscriptEntry::Status { text, .. } if text == "intervening"
    ));
}

#[test]
fn retry_tool_first_reuses_anchor_before_intervening_finalized_entry() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "failed answer".to_owned(),
    });
    let anchor_id = pane.transcript().entry_ids()[0];
    schedule_and_resume_retry(&mut pane, 1);
    pane.transcript_mut()
        .push(TranscriptEntry::status("intervening"));
    let intervening_id = pane.transcript().entry_ids()[1];

    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-2".to_owned(),
        name: "Read".to_owned(),
    });

    assert_eq!(pane.transcript().entries().len(), 2);
    assert_eq!(pane.transcript().entry_ids(), &[anchor_id, intervening_id]);
    assert!(matches!(
        &pane.transcript().entries()[0],
        TranscriptEntry::ToolRun { component } if component.id() == "tool-2"
    ));
    assert!(matches!(
        &pane.transcript().entries()[1],
        TranscriptEntry::Status { text, .. } if text == "intervening"
    ));
}

#[test]
fn provider_message_finished_error_renders_one_terminal_error_row() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "assistant-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::Error,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::Error,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RunFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::Error,
    });

    let entries = pane.transcript().entries();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::Status { .. }))
            .count(),
        1
    );
    assert!(entries.iter().any(|entry| matches!(
        entry,
        TranscriptEntry::Status {
            text,
            severity: Some(StatusSeverity::Error),
        } if text == "Provider response ended with an error."
    )));
    let rendered = plain_frame(&mut pane, 80, 20).join("\n");
    assert_eq!(
        rendered
            .matches("Provider response ended with an error.")
            .count(),
        1,
        "{rendered}"
    );
}

#[test]
fn retry_exhaustion_suppresses_followup_error_card() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "partial".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryExhausted {
        turn: 1,
        retries_used: 5,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: connection reset".to_owned(),
    });
    let entry_count = pane.transcript().entries().len();
    pane.apply_agent_event(neo_agent_core::AgentEvent::Error {
        turn: 1,
        message: "transport error: connection reset".to_owned(),
        code: Some("provider.transport_error".to_owned()),
        retry_after: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RunFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::Error,
    });

    assert_eq!(pane.transcript().entries().len(), entry_count);
    assert_eq!(
        pane.transcript()
            .entries()
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::RetryStatus { .. }))
            .count(),
        1
    );
    let rendered = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(!rendered.contains("runtime error"), "{rendered}");
}

#[test]
fn quota_exhausted_error_preserves_provider_detail() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::Error {
        turn: 1,
        message: "quota exhausted: balance is 0; purchase extra usage".to_owned(),
        code: Some("provider.quota_exhausted".to_owned()),
        retry_after: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RunFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::Error,
    });

    let rendered = plain_frame(&mut pane, 80, 20).join("\n");
    assert_eq!(rendered.matches("Quota Exhausted").count(), 1, "{rendered}");
    assert_eq!(
        rendered
            .matches("balance is 0; purchase extra usage")
            .count(),
        1,
        "{rendered}"
    );
    for unexpected in [
        "Check API key",
        "quota exhausted:",
        "runtime error",
        "Reconnecting",
    ] {
        assert!(!rendered.contains(unexpected), "{rendered}");
    }
}

#[test]
fn retry_error_interrupts_connecting_status_and_keeps_terminal_error_visible() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: connection reset".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryStarted {
        turn: 1,
        retry: 1,
        max_retries: 5,
    });
    let retry_entry_id = pane.transcript().entry_ids()[0];

    pane.apply_agent_event(neo_agent_core::AgentEvent::Error {
        turn: 1,
        message: "terminal connection failure".to_owned(),
        code: None,
        retry_after: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::Error,
    });

    assert_eq!(pane.transcript().entry_ids()[0], retry_entry_id);
    assert!(matches!(
        &pane.transcript().entries()[0],
        TranscriptEntry::Status { text, .. }
            if text == "Reconnect interrupted during attempt 1"
    ));
    assert!(
        pane.transcript()
            .entries()
            .iter()
            .all(|entry| !matches!(entry, TranscriptEntry::RetryStatus { .. }))
    );
    let rendered = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(rendered.contains("Reconnect interrupted"), "{rendered}");
    assert!(
        rendered.contains("Error: terminal connection failure"),
        "{rendered}"
    );
    assert!(!rendered.contains("connecting"), "{rendered}");
}

#[test]
fn retry_wait_cancel_becomes_interrupted_terminal_status() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: connection reset".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::Cancelled,
    });

    assert!(pane.transcript().entries().iter().all(|entry| !matches!(
        entry,
        TranscriptEntry::RetryStatus { data }
            if data.phase != neo_tui::transcript::entry::RetryPhase::Exhausted
    )));
    let rendered = plain_frame(&mut pane, 80, 20).join("\n");
    assert!(rendered.contains("Reconnect interrupted"), "{rendered}");
    assert!(!rendered.contains("Reconnect failed"), "{rendered}");
}

#[test]
fn retry_reset_preserves_earlier_turn_live_entry() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "older-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),
    });
    let older_id = pane.transcript().entry_ids()[0];
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 2,
        id: "attempt-2".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 2,
        text: "discard current turn only".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 2,
        retry: 1,
        max_retries: 5,
        delay_ms: 12_000,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: connection reset".to_owned(),
    });

    assert_eq!(pane.transcript().entry_ids()[0], older_id);
    assert!(matches!(
        pane.transcript().entries()[0],
        TranscriptEntry::ToolRun { .. }
    ));
    assert!(matches!(
        pane.transcript().entries()[1],
        TranscriptEntry::RetryStatus { .. }
    ));
}

// ── Instruction epoch cards (path-scoped AGENTS.md instructions) ────────────

fn instruction_test_epoch(generation: u64, deferred_tool_ids: &[&str]) -> InstructionEpochData {
    let nested = std::path::PathBuf::from("/workspace/neo/crates/neo-tui");
    InstructionEpochData {
        agent_id: "main".to_owned(),
        generation,
        outcome: InstructionEpochOutcome::Activated,
        scopes: vec![InstructionScopeData {
            display_path: nested.clone(),
            kind: InstructionScopeKind::Nested,
            revision: Some("7af13c2e".to_owned()),
            token_estimate: 31_800,
        }],
        selected_bundles: vec![InstructionBundleMetadata {
            display_path: nested,
            revision: "7af13c2e".to_owned(),
            token_estimate: 31_800,
            byte_size: 127_200,
            source_count: 3,
            import_count: 2,
        }],
        ignored_bundles: Vec::new(),
        replacements: Vec::new(),
        failure: None,
        deferred_tool_ids: deferred_tool_ids
            .iter()
            .map(|id| (*id).to_owned())
            .collect(),
        model_content: Some("scoped rules".to_owned()),
    }
}

fn instruction_order(pane: &TranscriptPane) -> Vec<String> {
    pane.transcript()
        .entries()
        .iter()
        .map(|entry| match entry {
            TranscriptEntry::InstructionEpoch { component } => {
                format!("card:{}", component.id())
            }
            TranscriptEntry::ToolRun { component } => format!("tool:{}", component.id()),
            TranscriptEntry::AssistantMessage { .. } => "assistant".to_owned(),
            _ => "other".to_owned(),
        })
        .collect()
}

#[test]
fn replayed_instruction_epoch_has_identical_order_and_no_duplicate_card() {
    const DEFERRED: [(&str, &str); 3] =
        [("read-1", "Read"), ("grep-1", "Grep"), ("bash-1", "Bash")];
    const RETRIED: [(&str, &str); 3] = [("read-2", "Read"), ("grep-2", "Grep"), ("bash-2", "Bash")];
    let deferred_ids = DEFERRED.map(|(id, _)| id);
    let epoch = instruction_test_epoch(3, &deferred_ids);

    let replay = |pane: &mut TranscriptPane| {
        for (id, name) in DEFERRED {
            pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
                turn: 1,
                id: id.to_owned(),
                name: name.to_owned(),
            });
        }
        pane.apply_agent_event(neo_agent_core::AgentEvent::InstructionEpoch {
            epoch: epoch.clone(),
        });
        // Deferred calls receive provider-valid non-error results without
        // executing; on replay those results arrive through the normal
        // finish path and must stay absorbed behind the card.
        for (id, name) in DEFERRED {
            pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: id.to_owned(),
                name: name.to_owned(),
                result: neo_agent_core::ToolResult::ok("deferred by instruction epoch"),
            });
        }
        // The model replans and re-issues the batch under fresh ids.
        for (id, name) in RETRIED {
            pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: id.to_owned(),
                name: name.to_owned(),
                arguments: serde_json::json!({}),
            });
            pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: id.to_owned(),
                name: name.to_owned(),
                result: neo_agent_core::ToolResult::ok("done"),
            });
        }
    };

    let mut pane = TranscriptPane::new(80, 24);
    pane.set_workspace_root("/workspace/neo");
    replay(&mut pane);
    let first_order = instruction_order(&pane);

    replay(&mut pane);
    let second_order = instruction_order(&pane);

    let expected = vec![
        "card:instruction-epoch-main-3".to_owned(),
        "tool:read-1".to_owned(),
        "tool:grep-1".to_owned(),
        "tool:bash-1".to_owned(),
        "tool:read-2".to_owned(),
        "tool:grep-2".to_owned(),
        "tool:bash-2".to_owned(),
    ];
    assert_eq!(first_order, expected);
    assert_eq!(
        second_order, expected,
        "replay must reconstruct the same visible order via deferred_tool_ids"
    );
    for (id, _) in DEFERRED {
        assert!(
            pane.transcript().is_tool_run_suppressed(id),
            "deferred placeholder {id} stays absorbed after replay"
        );
    }
    for (id, _) in RETRIED {
        assert!(
            !pane.transcript().is_tool_run_suppressed(id),
            "retried tool {id} must stay visible"
        );
    }

    let frame = plain_frame(&mut pane, 80, 24);
    let card_rows = frame
        .iter()
        .filter(|line| line.contains("Instructions loaded"))
        .count();
    assert_eq!(
        card_rows, 1,
        "identical epochs never produce duplicate cards: {frame:?}"
    );
}

#[test]
fn finalized_instruction_card_does_not_drift_after_later_updates() {
    let mut pane = TranscriptPane::new(80, 24);
    pane.set_workspace_root("/workspace/neo");
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "read-1".to_owned(),
        name: "Read".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "grep-1".to_owned(),
        name: "Grep".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::InstructionEpoch {
        epoch: instruction_test_epoch(3, &["read-1", "grep-1"]),
    });
    assert!(matches!(
        pane.transcript().entries().first(),
        Some(TranscriptEntry::InstructionEpoch { .. })
    ));

    // Later turn activity: assistant text, the replanned tool batch, turn
    // completion, and a follow-up epoch with no deferred placeholders.
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Working on it.".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "read-2".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "read-2".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("done"),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::InstructionEpoch {
        epoch: instruction_test_epoch(4, &[]),
    });

    let entries = pane.transcript().entries();
    assert!(
        matches!(
            entries.first(),
            Some(TranscriptEntry::InstructionEpoch { component })
                if component.id() == "instruction-epoch-main-3"
        ),
        "the finalized card must not drift to the transcript bottom: {:?}",
        instruction_order(&pane)
    );
    assert!(
        matches!(
            entries.last(),
            Some(TranscriptEntry::InstructionEpoch { component })
                if component.id() == "instruction-epoch-main-4"
        ),
        "an epoch without placeholders appends after later activity: {:?}",
        instruction_order(&pane)
    );
    assert_eq!(
        pane.transcript().entry_finalization(0),
        Some(Finalization::Finalized)
    );
}
