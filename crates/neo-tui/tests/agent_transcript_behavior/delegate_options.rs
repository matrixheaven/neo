use neo_agent_core::AgentEvent;
use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome, AgentTerminalReason,
    AgentToolActivityPhase, AgentToolOutputPreview, DelegateContext,
};
use neo_tui::primitive::strip_ansi;
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::transcript::{DelegateCardComponent, TranscriptPane};
use std::time::Duration;

fn running_delegate() -> AgentSnapshot {
    let name = AgentDisplayName::new("Gibbs");
    AgentSnapshot {
        id: AgentId::from_suffix_for_test("test"),
        display_name: name.clone(),
        path: AgentPath::root_child(&name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: "Implement Task 1: PlanBox border fix".to_owned(),
        task_title: "Implement Task 1: PlanBox border fix".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 1,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: false,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 3,
        token_count: 25_600,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::from_secs(24),
        latest_text: Some("Let me start by reading the current file.".to_owned()),
        activity: vec![
            AgentActivityEntry {
                kind: AgentActivityKind::Tool {
                    id: "read-1".to_owned(),
                    name: "Read".to_owned(),
                    summary: Some("crates/neo-tui/src/transcript/plan_box.rs".to_owned()),
                    phase: AgentToolActivityPhase::Done,
                    output: None,
                    files: Vec::new(),
                    output_ref: None,
                },
            },
            AgentActivityEntry {
                kind: AgentActivityKind::Tool {
                    id: "grep-1".to_owned(),
                    name: "Grep".to_owned(),
                    summary: Some("from_spans|pub struct Span|pub struct Line".to_owned()),
                    phase: AgentToolActivityPhase::Failed,
                    output: None,
                    files: Vec::new(),
                    output_ref: None,
                },
            },
            AgentActivityEntry {
                kind: AgentActivityKind::Text {
                    text: "Let me start by reading the current file.".to_owned(),
                    thinking: true,
                },
            },
        ],
        prior_messages: Vec::new(),
        outcome: None,
    }
}
fn option_b_running_delegate() -> AgentSnapshot {
    let mut snapshot = option_b_delegate(
        "nova",
        "Nova",
        AgentRole::Coder,
        AgentLifecycleState::Running,
        "角色对比测试 coder",
    );
    snapshot.tool_count = 3;
    snapshot.token_count = 22_700;
    snapshot.elapsed = Duration::from_secs(21);
    snapshot.activity = vec![
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "read-delegate".to_owned(),
                name: "Read".to_owned(),
                summary: Some("crates/neo-agent-core/src/tools/delegate.rs".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: None,
                files: Vec::new(),
                output_ref: None,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "bash-nextest".to_owned(),
                name: "Bash".to_owned(),
                summary: Some("cargo nextest run -p neo-agent-core ...".to_owned()),
                phase: AgentToolActivityPhase::Ongoing,
                output: Some(AgentToolOutputPreview {
                    text: "running: cargo nextest run -p neo-agent-core ...\nCompiling neo-agent-core v0.1.0".to_owned(),
                    is_error: false,
                    truncated: true,
                    tail: true,
                }),
                files: Vec::new(),
                output_ref: None,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "Let me verify the state mutation path before editing.".to_owned(),
                thinking: true,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "I found the foreground aggregation issue. Next I will make the renderer change.".to_owned(),
                thinking: false,
            },
        },
    ];
    snapshot.latest_text = Some(
        "I found the foreground aggregation issue. Next I will make the renderer change."
            .to_owned(),
    );
    snapshot
}
fn plain(lines: Vec<neo_tui::primitive::Line>) -> Vec<String> {
    lines
        .into_iter()
        .map(|l| strip_ansi(&l.to_ansi()))
        .collect()
}
fn completed_delegate() -> AgentSnapshot {
    AgentSnapshot {
        state: AgentLifecycleState::Completed,
        outcome: Some(AgentTerminalOutcome {
            summary: "Done".to_owned(),
            is_error: false,
        }),
        ..running_delegate()
    }
}
fn option_b_delegate(
    id_suffix: &str,
    name: &str,
    role: AgentRole,
    state: AgentLifecycleState,
    title: &str,
) -> AgentSnapshot {
    let display_name = AgentDisplayName::new(name);
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id_suffix),
        display_name: display_name.clone(),
        path: AgentPath::root_child(&display_name),
        role,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state,
        task: format!("{title}\n\nFull prompt that must not replace the display name."),
        task_title: title.to_owned(),
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
        started_at_ms: matches!(state, AgentLifecycleState::Running).then_some(1_000),
        terminal_at_ms: state.is_terminal().then_some(31_000),
        detached_from_foreground: false,
        terminal_reason: terminal_reason_for_state(state),
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 0,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::from_secs(0),
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    }
}
fn terminal_reason_for_state(state: AgentLifecycleState) -> Option<AgentTerminalReason> {
    match state {
        AgentLifecycleState::Queued | AgentLifecycleState::Running => None,
        AgentLifecycleState::Completed => Some(AgentTerminalReason::Completed),
        AgentLifecycleState::Failed => Some(AgentTerminalReason::Error),
        AgentLifecycleState::Cancelled => Some(AgentTerminalReason::CancelledByUser),
        AgentLifecycleState::TimedOut => Some(AgentTerminalReason::TimedOut),
        AgentLifecycleState::Interrupted => Some(AgentTerminalReason::ProcessExited),
    }
}

#[test]
fn option_b_backgrounded_delegate_uses_backgrounded_label_without_detach_hint() {
    let mut snapshot = option_b_running_delegate();
    snapshot.detached_from_foreground = true;

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()))
            .join("\n");

    assert!(text.contains("● Nova  [Coder] · Delegate"), "{text}");
    assert!(text.contains("backgrounded"), "{text}");
    assert!(!text.contains("Ctrl+B"), "{text}");
}

#[test]
fn option_b_child_activity_keeps_ongoing_tool_visible_after_text_tail() {
    let mut snapshot = option_b_running_delegate();
    snapshot.activity.truncate(2);
    for index in 0..8 {
        snapshot.activity.push(AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: format!("streamed body fragment {index}"),
                thinking: false,
            },
        });
    }
    snapshot.latest_text = Some("streamed body fragment 7".to_owned());

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()))
            .join("\n");

    assert!(text.contains("• Using Bash"), "{text}");
    assert!(text.contains("streamed body fragment 7"), "{text}");
}

#[test]
fn option_b_child_activity_orders_tools_thinking_body_and_final() {
    let mut snapshot = option_b_running_delegate();
    snapshot.state = AgentLifecycleState::Completed;
    snapshot.terminal_at_ms = Some(31_000);
    snapshot.terminal_reason = Some(AgentTerminalReason::Completed);
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "All edits applied. The card now shows agent name first.".to_owned(),
        is_error: false,
    });

    let rows =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()));
    let text = rows.join("\n");

    let used_index = rows
        .iter()
        .position(|row| row.contains("• Used Read (crates/neo-agent-core/src/tools/delegate.rs)"))
        .expect("used row");
    let using_index = rows
        .iter()
        .position(|row| row.contains("• Using Bash (cargo nextest run -p neo-agent-core ...)"))
        .expect("using row");
    let thinking_index = rows
        .iter()
        .position(|row| row.contains("◌ thinking"))
        .expect("thinking row");
    let body_index = rows
        .iter()
        .position(|row| row.contains("│ I found"))
        .expect("body row");
    let final_index = rows
        .iter()
        .position(|row| row.contains("└ All edits applied"))
        .expect("final row");

    assert!(used_index < using_index, "{text}");
    assert!(using_index < thinking_index, "{text}");
    assert!(thinking_index < body_index, "{text}");
    assert!(body_index < final_index, "{text}");
    assert_eq!(final_index, rows.len() - 1, "{text}");
    assert!(
        text.contains("running: cargo nextest run -p neo-agent-core"),
        "{text}"
    );
    assert_eq!(text.matches("All edits applied").count(), 1, "{text}");
}

#[test]
fn option_b_child_activity_preserves_recent_thinking_chunks() {
    let mut snapshot = option_b_running_delegate();
    snapshot.state = AgentLifecycleState::Completed;
    snapshot.terminal_at_ms = Some(31_000);
    snapshot.terminal_reason = Some(AgentTerminalReason::Completed);
    snapshot.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: "First thinking chunk.".to_owned(),
            thinking: true,
        },
    });
    snapshot.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: "Second thinking chunk.".to_owned(),
            thinking: true,
        },
    });
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "Final summary after thinking.".to_owned(),
        is_error: false,
    });

    let rows =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()));
    let text = rows.join("\n");

    let first_index = rows
        .iter()
        .position(|row| row.contains("First thinking chunk."))
        .expect("first thinking chunk");
    let second_index = rows
        .iter()
        .position(|row| row.contains("Second thinking chunk."))
        .expect("second thinking chunk");
    let final_index = rows
        .iter()
        .position(|row| row.contains("└ Final summary after thinking."))
        .expect("final row");

    assert!(first_index < second_index, "{text}");
    assert!(second_index < final_index, "{text}");
}

#[test]
fn option_b_child_activity_uses_latest_body_text_only() {
    let mut snapshot = option_b_running_delegate();
    snapshot.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: "Older body text that should disappear.".to_owned(),
            thinking: false,
        },
    });
    snapshot.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: "Newest body text wins.".to_owned(),
            thinking: false,
        },
    });

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()))
            .join("\n");

    assert!(text.contains("│ Newest body text wins."), "{text}");
    assert!(
        !text.contains("Older body text that should disappear."),
        "{text}"
    );
}

#[test]
fn option_b_completed_delegate_uses_name_badge_and_final_row() {
    let mut snapshot = option_b_running_delegate();
    snapshot.state = AgentLifecycleState::Completed;
    snapshot.terminal_at_ms = Some(31_000);
    snapshot.terminal_reason = Some(AgentTerminalReason::Completed);
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "All edits applied. The card now shows agent name first.".to_owned(),
        is_error: false,
    });

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()))
            .join("\n");

    assert!(text.contains("✓ Nova  [Coder] · Delegate"), "{text}");
    assert!(text.contains("done"), "{text}");
    assert!(text.contains("3 tools"), "{text}");
    assert!(text.contains("└ All edits applied"), "{text}");
    assert!(!text.contains("Agent Completed"), "{text}");
}

#[test]
fn option_b_delegate_absorption_distinguishes_completed_agent_from_failed_schema_result() {
    let mut pane = TranscriptPane::new(140, 30);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 18,
        id: "tool_delegate_schema_failure".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),
        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 18,
        agent: running_delegate(),
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateFinished {
        turn: 18,
        agent: completed_delegate(),
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 18,
        id: "tool_delegate_schema_failure".to_owned(),
        name: "Delegate".to_owned(),
        result: neo_agent_core::ToolResult::error("required property `echoed` is missing")
            .with_details(serde_json::json!({
                "kind": "delegate",
                "agent_id": "agent_test",
                "status": "completed",
                "schema_error_code": "schema_invalid",
                "schema_error": "required property `echoed` is missing"
            })),
        workflow_origin: None,
        output_ref: None,
    });

    pane.set_tool_output_expanded(true);
    let _ = pane.render_frame(140, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Failed Delegate"), "{text}");
    assert!(text.contains("Agent lifecycle: completed"), "{text}");
    assert!(text.contains("Requested result: failed"), "{text}");
    assert!(
        text.contains("output did not match the requested format"),
        "{text}"
    );
    assert!(
        text.contains("schema error: required property `echoed` is missing"),
        "{text}"
    );
    assert!(!text.contains("status: completed"), "{text}");
    assert!(text.contains("Gibbs  [Coder] · Delegate"), "{text}");
}

#[test]
fn option_b_delegate_absorption_keeps_completed_mismatched_tool_when_snapshot_arrives_late() {
    let mut pane = TranscriptPane::new(140, 30);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 17,
        id: "tool_delegate_mismatch_before_snapshot".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 17,
        id: "tool_delegate_mismatch_before_snapshot".to_owned(),
        name: "Delegate".to_owned(),
        result: neo_agent_core::ToolResult::ok("agent_id: agent_other").with_details(
            serde_json::json!({
                "kind": "delegate",
                "agent_id": "agent_other"
            }),
        ),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 17,
        agent: running_delegate(),
        workflow_origin: None,
    });

    let _ = pane.render_frame(140, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Used Delegate"), "{text}");
    assert!(text.contains("agent_id: agent_other"), "{text}");
    assert!(text.contains("Gibbs  [Coder] · Delegate"), "{text}");
}

#[test]
fn option_b_delegate_absorption_restores_failed_tool_result() {
    let mut pane = TranscriptPane::new(140, 30);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 14,
        id: "tool_delegate_failed".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 14,
        agent: running_delegate(),
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 14,
        id: "tool_delegate_failed".to_owned(),
        name: "Delegate".to_owned(),
        result: neo_agent_core::ToolResult::error("delegate failed before snapshot settled"),

        workflow_origin: None,
        output_ref: None,
    });

    let _ = pane.render_frame(140, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Failed Delegate"), "{text}");
    assert!(
        text.contains("delegate failed before snapshot settled"),
        "{text}"
    );
    assert!(text.contains("Gibbs  [Coder] · Delegate"), "{text}");
}

#[test]
fn option_b_delegate_absorption_restores_mismatched_tool_result_details() {
    let mut pane = TranscriptPane::new(140, 30);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 15,
        id: "tool_delegate_mismatch".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 15,
        agent: running_delegate(),
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 15,
        id: "tool_delegate_mismatch".to_owned(),
        name: "Delegate".to_owned(),
        result: neo_agent_core::ToolResult::ok("agent_id: agent_other").with_details(
            serde_json::json!({
                "kind": "delegate",
                "agent_id": "agent_other"
            }),
        ),

        workflow_origin: None,
        output_ref: None,
    });

    let _ = pane.render_frame(140, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Used Delegate"), "{text}");
    assert!(text.contains("agent_id: agent_other"), "{text}");
    assert!(text.contains("Gibbs  [Coder] · Delegate"), "{text}");
}

#[test]
fn option_b_delegate_absorption_suppresses_matching_tool_result_details() {
    let mut pane = TranscriptPane::new(140, 30);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 20,
        id: "tool_delegate_matched_result".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 20,
        agent: running_delegate(),
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 20,
        id: "tool_delegate_matched_result".to_owned(),
        name: "Delegate".to_owned(),
        result: neo_agent_core::ToolResult::ok("agent_id: agent_test").with_details(
            serde_json::json!({
                "kind": "delegate",
                "agent_id": "agent_test"
            }),
        ),

        workflow_origin: None,
        output_ref: None,
    });

    let _ = pane.render_frame(140, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("Using Delegate"), "{text}");
    assert!(!text.contains("Used Delegate"), "{text}");
    assert!(text.contains("Gibbs  [Coder] · Delegate"), "{text}");
    assert!(text.contains("agent_test"), "{text}");
}

#[test]
fn option_b_delegate_transcript_absorbs_late_tool_header_after_snapshot() {
    let mut pane = TranscriptPane::new(140, 30);
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 16,
        agent: running_delegate(),
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 16,
        id: "tool_delegate_late".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),

        workflow_origin: None,
        output_ref: None,
    });

    let _ = pane.render_frame(140, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("Using Delegate"), "{text}");
    assert!(text.contains("Gibbs  [Coder] · Delegate"), "{text}");
}

#[test]
fn option_b_delegate_transcript_absorbs_matching_tool_header() {
    let mut pane = TranscriptPane::new(140, 30);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 11,
        id: "tool_delegate_single".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 11,
        agent: running_delegate(),
        workflow_origin: None,
    });

    let _ = pane.render_frame(140, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("Using Delegate"), "{text}");
    assert!(!text.contains("Used Delegate"), "{text}");
    assert!(text.contains("Gibbs  [Coder] · Delegate"), "{text}");
    assert!(text.contains("agent_test"), "{text}");
}

#[test]
fn option_b_narrow_width_preserves_name_and_role_badge() {
    let text = plain(
        DelegateCardComponent::new(option_b_running_delegate())
            .render_with_theme(48, &TuiTheme::default()),
    )
    .join("\n");

    assert!(text.contains("Nova"), "{text}");
    assert!(text.contains("[Coder]"), "{text}");
    assert!(
        !text.contains("Full prompt that must not replace"),
        "narrow header must drop prompt/title before identity: {text}"
    );
}

#[test]
fn option_b_single_delegate_shows_name_first_and_role_badge() {
    let rows = plain(
        DelegateCardComponent::new(option_b_running_delegate())
            .render_with_theme(140, &TuiTheme::default()),
    );
    let text = rows.join("\n");
    let header = rows.first().expect("delegate header");

    assert!(header.contains("● Nova  [Coder] · Delegate"), "{text}");
    assert!(header.contains("角色对比测试 coder"), "{text}");
    assert!(header.contains("running"), "{text}");
    assert!(header.contains("21s"), "{text}");
    assert!(header.contains("22.7k"), "{text}");
    assert!(
        !header.contains("Coder Agent Running"),
        "role must be a badge, not the primary visible name: {text}"
    );
}

#[test]
fn option_b_state_markers_do_not_depend_on_color_only() {
    let completed = AgentSnapshot {
        state: AgentLifecycleState::Completed,
        terminal_reason: Some(AgentTerminalReason::Completed),
        outcome: Some(AgentTerminalOutcome {
            summary: "Done".to_owned(),
            is_error: false,
        }),
        ..option_b_running_delegate()
    };
    let failed = AgentSnapshot {
        state: AgentLifecycleState::Failed,
        terminal_reason: Some(AgentTerminalReason::Error),
        outcome: Some(AgentTerminalOutcome {
            summary: "Failed".to_owned(),
            is_error: true,
        }),
        ..option_b_running_delegate()
    };

    let completed_text =
        plain(DelegateCardComponent::new(completed).render_with_theme(120, &TuiTheme::default()))
            .join("\n");
    let failed_text =
        plain(DelegateCardComponent::new(failed).render_with_theme(120, &TuiTheme::default()))
            .join("\n");

    assert!(
        completed_text.contains("✓ Nova  [Coder] · Delegate"),
        "{completed_text}"
    );
    assert!(completed_text.contains("done"), "{completed_text}");
    assert!(
        failed_text.contains("✗ Nova  [Coder] · Delegate"),
        "{failed_text}"
    );
    assert!(failed_text.contains("failed"), "{failed_text}");
}
