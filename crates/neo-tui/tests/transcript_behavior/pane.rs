use neo_tui::primitive::Color;
use neo_tui::primitive::strip_ansi;
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::transcript::TranscriptEntry;
use neo_tui::transcript::TranscriptPane;
use neo_tui::transcript::{McpStartupPhase, McpStartupStatusData, StatusSeverity};

fn plain(line: &str) -> String {
    strip_ansi(line).trim_end().to_owned()
}
fn plain_frame(transcript: &mut TranscriptPane, width: usize, height: usize) -> Vec<String> {
    transcript
        .render_frame(width, height)
        .expect("render frame")
        .iter()
        .map(|line| plain(line))
        .collect()
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
fn visible_slice_and_snapshot_render_the_same_document() {
    let mut pane = TranscriptPane::new(80, 6);
    let status_lines = (0..12)
        .map(|index| format!("status line {index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    pane.push_status(status_lines);

    // The bounded slice follows the tail; the full snapshot composes the
    // entire document. Both come from the same document geometry.
    let slice = pane.render_visible_slice(80, 6).join("\n");
    let slice = strip_ansi(&slice);
    assert!(slice.contains("status line 11"), "slice:\n{slice}");
    assert!(!slice.contains("status line 00"), "slice:\n{slice}");

    pane.mark_dirty();
    let _ = pane.render_frame(80, 6).expect("snapshot render");
    let canonical = pane.frame_ansi_lines().join("\n");
    let canonical = strip_ansi(&canonical);
    assert!(canonical.contains("status line 00"));
    assert!(canonical.contains("status line 11"));
}

#[test]
fn visible_slice_renders_committed_content_every_frame() {
    let mut pane = TranscriptPane::new(80, 6);
    pane.push_status("committed status");

    let first = pane.render_visible_slice(80, 6).join("\n");
    assert!(strip_ansi(&first).contains("committed status"));

    // The fullscreen document owns the content: every frame re-renders the
    // same bounded slice with no history acknowledgement between frames.
    let second = pane.render_visible_slice(80, 6).join("\n");
    assert!(strip_ansi(&second).contains("committed status"));
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

        workflow_origin: None,
        output_ref: None,
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
fn list_delegates_renders_structured_rows_without_opaque_cursor() {
    let mut pane = TranscriptPane::new(80, 24);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "list-1".to_owned(),
        name: "ListDelegates".to_owned(),
        arguments: serde_json::json!({}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "list-1".to_owned(),
        name: "ListDelegates".to_owned(),
        result: neo_agent_core::ToolResult::ok(
            "total: 5\nnext_cursor: opaque-cursor-value\n- agent rows...",
        )
        .with_details(serde_json::json!({
            "kind": "delegate_list",
            "count": 2,
            "total": 5,
            "next_cursor": "opaque-cursor-value",
            "cursor_query": { "offset": 2 },
            "delegates": [
                {
                    "kind": "agent",
                    "display_name": "Pascal",
                    "status": "running",
                    "title": "implement overflow viewport"
                },
                {
                    "kind": "swarm",
                    "description": "parallel research swarm",
                    "status": "running",
                    "aggregate": {
                        "total": 3,
                        "queued": 0,
                        "running": 2,
                        "completed": 1,
                        "failed": 0,
                        "cancelled": 0,
                        "timed_out": 0
                    }
                }
            ]
        })),

        workflow_origin: None,
        output_ref: None,
    });

    let slice = pane.render_visible_slice(80, 24);
    let text = slice
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("2 of 5"), "count/total missing: {text}");
    assert!(text.contains("Pascal"), "agent name missing: {text}");
    assert!(
        text.contains("implement overflow viewport"),
        "agent title missing: {text}"
    );
    assert!(
        text.contains("parallel research swarm"),
        "swarm description missing: {text}"
    );
    assert!(
        text.contains("aggregate") && text.contains("total=3"),
        "swarm aggregate missing: {text}"
    );
    assert!(
        !text.contains("opaque-cursor-value"),
        "cursor leaked: {text}"
    );
    assert!(
        !text.contains("next_cursor:"),
        "raw next_cursor leaked: {text}"
    );
    assert!(
        !text.contains("cursor_query"),
        "cursor_query leaked: {text}"
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
    transcript_pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        phase: McpStartupPhase::Connecting,
    });

    let pending = transcript_pane.render_visible_slice(100, 12);
    assert!(
        pending
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
    let failed = transcript_pane.render_visible_slice(100, 12);
    let failed_plain = failed
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        failed_plain.contains("✗ MCP server \"linear\" failed · timeout connecting to server"),
        "{failed_plain}"
    );
    assert!(
        failed
            .iter()
            .any(|line| line.contains(&ansi_for_color(theme.status_error))),
        "failed state must keep the error color"
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
fn provider_message_finished_error_renders_one_terminal_error_row() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        phase: neo_ai::MessagePhase::Unknown,
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

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "skill-1".to_owned(),
        name: "Skill".to_owned(),
        result: neo_agent_core::ToolResult::ok("expanded skill body"),

        workflow_origin: None,
        output_ref: None,
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
fn sleep_renders_total_remaining_and_reason_without_duplicate_result() {
    let mut pane = TranscriptPane::new(80, 16);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "sleep-1".to_owned(),
        name: "Sleep".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "sleep-1".to_owned(),
        json_fragment: r#"{"duration_seconds":30,"reason":"backoff before retry"}"#.to_owned(),
    });

    let pending = pane.render_visible_slice(80, 16);
    let pending_text = pending
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        pending_text.contains("30s total"),
        "pending total missing: {pending_text}"
    );
    assert!(
        !pending_text.contains(" remaining"),
        "countdown must not start before Sleep execution: {pending_text}"
    );

    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "sleep-1".to_owned(),
        name: "Sleep".to_owned(),
        arguments: serde_json::json!({
            "duration_seconds": 30,
            "reason": "backoff before retry"
        }),

        workflow_origin: None,
        output_ref: None,
    });

    let running = pane.render_visible_slice(80, 16);
    let running_text = running
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        running_text.contains("30s total"),
        "total missing while running: {running_text}"
    );
    assert!(
        running_text.contains("remaining"),
        "remaining missing while running: {running_text}"
    );
    assert!(
        running_text.contains("backoff before retry"),
        "reason missing while running: {running_text}"
    );

    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "sleep-1".to_owned(),
        name: "Sleep".to_owned(),
        result: neo_agent_core::ToolResult::ok("Waited 30 seconds: backoff before retry"),

        workflow_origin: None,
        output_ref: None,
    });
    let finished = pane.render_visible_slice(80, 16);
    let finished_text = finished
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        finished_text.contains("30s total"),
        "total missing after success: {finished_text}"
    );
    assert!(
        finished_text.contains("backoff before retry"),
        "reason missing after success: {finished_text}"
    );
    assert!(
        !finished_text.contains("Waited 30 seconds"),
        "generic Waited body should be suppressed: {finished_text}"
    );
    assert!(
        !finished_text.contains(" remaining"),
        "remaining should hide after success: {finished_text}"
    );
}

#[test]
fn tool_output_toggle_expands_and_collapses_in_primary_document() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.transcript_mut()
        .push_tool_run("tool-1", "Read", Some("{\"path\":\"a\"}".to_owned()));
    assert!(pane.transcript_mut().mutate_tool("tool-1", |tool| {
        tool.set_result(Some("ok".to_owned()), None, false, None)
    }));

    assert!(!pane.tool_output_expanded());
    let collapsed = strip_ansi(&pane.render_visible_slice(80, 20).join("\n"));
    assert!(
        collapsed.contains("Used Read"),
        "collapsed still shows tool header: {collapsed}"
    );
    assert!(
        collapsed.contains("1 lines"),
        "collapsed shows result line count chip: {collapsed}"
    );
    assert!(
        collapsed.contains("ok"),
        "collapsed shows result preview: {collapsed}"
    );

    // Ctrl+O routes to the primary document: no second surface, no browser
    // state, just the pane's expansion toggle.
    assert!(pane.toggle_tool_output_expanded());
    assert!(pane.tool_output_expanded());
    let expanded = strip_ansi(&pane.render_visible_slice(80, 20).join("\n"));
    assert!(
        expanded.contains("Used Read"),
        "expanded keeps the tool header: {expanded}"
    );
    assert!(
        expanded.contains("ok"),
        "expanded shows the full result: {expanded}"
    );

    pane.push_status("after-toggle");
    let slice = strip_ansi(&pane.render_visible_slice(80, 20).join("\n"));
    assert!(slice.contains("after-toggle"), "slice:\n{slice}");
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

        workflow_origin: None,
        output_ref: None,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok("Cargo.toml"),

        workflow_origin: None,
        output_ref: None,
    });
    transcript_pane.push_transcript(TranscriptEntry::thinking_complete("thinking two"));
    transcript_pane.push_transcript(TranscriptEntry::assistant_message("Final answer"));

    let frame = plain_frame(&mut transcript_pane, 80, 20);
    assert_one_blank_between(&frame, "thinking one", "Used Bash");
    assert_one_blank_between(&frame, "Used Bash", "thinking two");
    assert_one_blank_between(&frame, "thinking two", "Final answer");
}

#[test]
fn transcript_pane_exposes_frame_ansi_lines_for_inspection() {
    let mut transcript_pane = TranscriptPane::new(80, 12);
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),

        workflow_origin: None,
        output_ref: None,
    });
    let _ = transcript_pane.render_frame(80, 12);

    let lines = transcript_pane.frame_ansi_lines();
    assert!(
        lines.iter().any(|line| plain(line).contains("Using Bash")),
        "frame lines: {lines:?}"
    );
}

#[test]
fn transcript_pane_frame_keeps_tool_card_and_streaming_assistant() {
    let mut transcript_pane = TranscriptPane::new(80, 6);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),

        workflow_origin: None,
        output_ref: None,
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
fn transcript_pane_renders_transcript_entries_in_one_ordered_frame() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.push_transcript(TranscriptEntry::banner("Welcome to neo"));
    transcript_pane.push_transcript(TranscriptEntry::user_message("hello"));
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),

        workflow_origin: None,
        output_ref: None,
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
