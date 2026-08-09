use neo_agent_core::AgentEvent;
use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome, AgentTerminalReason,
    AgentToolActivityPhase, AgentToolOutputPreview, DelegateContext,
};
use neo_tui::primitive::strip_ansi;
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::transcript::{DelegateGroupComponent, TranscriptPane};
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
        input_token_count: 0,
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
        input_token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::from_secs(0),
        latest_text: None,
        activity: Vec::new(),
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
fn delegate_group_child_rows_keep_left_border_muted() {
    let theme = TuiTheme::default();
    let mut nova = option_b_running_delegate();
    nova.token_count = 6_468_100;
    nova.input_token_count = 6_390_000;
    nova.cache_read_token_count = 6_300_000;
    nova.state = AgentLifecycleState::Completed;
    nova.terminal_at_ms = Some(31_000);
    nova.terminal_reason = Some(AgentTerminalReason::Completed);
    nova.outcome = Some(AgentTerminalOutcome {
        summary: "All edits applied.".to_owned(),
        is_error: false,
    });
    let vega = option_b_delegate(
        "vega",
        "Vega",
        AgentRole::Explorer,
        AgentLifecycleState::Queued,
        "queued task",
    );

    // Nova is not the last agent, so its child rows use a │ continuation.
    let group = DelegateGroupComponent::new(1, vec![nova, vega]);
    let lines = group.render_with_theme(160, &theme);

    let used_line = lines
        .iter()
        .find(|line| line.text().contains("Used Read"))
        .expect("used tool row");
    let spans = used_line.spans();
    assert_eq!(spans[0].text(), "  │      ");
    assert_eq!(spans[0].style().fg, Some(theme.text_muted));

    let thinking_line = lines
        .iter()
        .find(|line| line.text().contains("◌ thinking"))
        .expect("thinking row");
    let spans = thinking_line.spans();
    assert_eq!(spans[0].text(), "  │      ");
    assert_eq!(spans[0].style().fg, Some(theme.text_muted));

    let body_line = lines
        .iter()
        .find(|line| line.text().contains("I found"))
        .expect("body row");
    let spans = body_line.spans();
    assert_eq!(spans[0].text(), "  │      ");
    assert_eq!(spans[0].style().fg, Some(theme.text_muted));
    assert_eq!(spans[1].text(), "│ ");
    assert_eq!(spans[1].style().fg, Some(theme.text_muted));

    let final_line = lines
        .iter()
        .find(|line| line.text().contains("└ All edits"))
        .expect("final row");
    let spans = final_line.spans();
    assert_eq!(spans[0].text(), "  │      ");
    assert_eq!(spans[0].style().fg, Some(theme.text_muted));
    assert_eq!(spans[1].text(), "└ ");
    assert_eq!(spans[1].style().fg, Some(theme.text_muted));

    let text = lines
        .iter()
        .map(neo_tui::primitive::Line::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("6.5M tok"), "{text}");
    assert!(text.contains("cache 6.3M read · hit 98.6%"), "{text}");
}

#[test]
fn delegate_group_styles_header_names_muted_tree_and_role_badges() {
    let theme = TuiTheme::default();
    let nova = option_b_running_delegate();
    let vega = option_b_delegate(
        "vega",
        "Vega",
        AgentRole::Explorer,
        AgentLifecycleState::Queued,
        "搜索历史卡片回归点",
    );
    let orion = option_b_delegate(
        "orion",
        "Orion",
        AgentRole::Planner,
        AgentLifecycleState::Queued,
        "规划分支测试",
    );
    let sage = option_b_delegate(
        "sage",
        "Sage",
        AgentRole::Reviewer,
        AgentLifecycleState::Queued,
        "审查分支测试",
    );
    let group = DelegateGroupComponent::new(7, vec![nova, vega, orion, sage]);

    let lines = group.render_with_theme(160, &theme);
    let header_spans = lines[0].spans();
    assert_eq!(header_spans[0].style().fg, Some(theme.brand));
    assert_eq!(header_spans[1].text(), " Delegate group · ");
    assert_eq!(header_spans[1].style().fg, Some(theme.brand));
    assert_eq!(header_spans[2].style().fg, Some(theme.brand));

    let assert_role_row = |needle: &str, branch: &str, badge: &str, color| {
        let row = lines
            .iter()
            .find(|line| line.text().contains(needle))
            .expect("agent row should render");
        let spans = row.spans();
        assert_eq!(spans[0].text(), branch);
        assert_eq!(spans[0].style().fg, Some(theme.text_muted));
        assert_eq!(spans[1].style().fg, Some(theme.brand));
        assert_eq!(spans[3].text(), badge);
        assert_eq!(spans[3].style().fg, Some(color));
    };
    assert_role_row("├─ Nova  [Coder]", "  ├─ ", "[Coder]", theme.status_warn);
    assert_role_row(
        "├─ Vega  [Explorer]",
        "  ├─ ",
        "[Explorer]",
        theme.shell_mode,
    );
    assert_role_row("├─ Orion  [Planner]", "  ├─ ", "[Planner]", theme.brand);
    assert_role_row(
        "└─ Sage  [Reviewer]",
        "  └─ ",
        "[Reviewer]",
        theme.status_ok,
    );
}

#[test]
fn later_same_turn_root_delegate_remains_visible_after_prior_group_commit() {
    let mut pane = TranscriptPane::new(160, 30);
    for delegate in [
        option_b_delegate(
            "committed_nova",
            "Nova",
            AgentRole::Coder,
            AgentLifecycleState::Completed,
            "first completed task",
        ),
        option_b_delegate(
            "committed_vega",
            "Vega",
            AgentRole::Explorer,
            AgentLifecycleState::Completed,
            "second completed task",
        ),
    ] {
        pane.apply_agent_event(AgentEvent::DelegateFinished {
            turn: 7,
            agent: delegate,
            workflow_origin: None,
        });
    }
    let committed = pane.render_visible_slice(160, 30);
    assert!(!committed.is_empty());

    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 7,
        agent: option_b_delegate(
            "later_euler",
            "Euler",
            AgentRole::Reviewer,
            AgentLifecycleState::Running,
            "review committed output",
        ),
        workflow_origin: None,
    });
    let slice = pane
        .render_visible_slice(160, 30)
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(slice.contains("Euler"), "slice:\n{slice}");
    assert!(slice.contains("Nova"), "committed card remains: {slice}");
}

#[test]
fn option_b_delegate_group_absorbs_matching_tool_headers() {
    let mut pane = TranscriptPane::new(160, 40);
    let mut first = running_delegate();
    first.id = AgentId::from_suffix_for_test("first_group_absorb");
    first.display_name = AgentDisplayName::new("Pascal");
    first.path = AgentPath::root_child(&first.display_name);
    first.task_title = "resume 一个 completed agent".to_owned();

    let mut second = running_delegate();
    second.id = AgentId::from_suffix_for_test("second_group_absorb");
    second.display_name = AgentDisplayName::new("Huygens");
    second.path = AgentPath::root_child(&second.display_name);
    second.role = AgentRole::Explorer;
    second.task_title = "resume 另一个 completed agent".to_owned();

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 12,
        id: "tool_delegate_pascal".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "6*7"}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 12,
        id: "tool_delegate_huygens".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "7*8"}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 12,
        agent: first,
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 12,
        agent: second,
        workflow_origin: None,
    });

    let _ = pane.render_frame(160, 40);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("Using Delegate"), "{text}");
    assert!(!text.contains("Used Delegate"), "{text}");
    assert!(text.contains("Delegate group · Running 2 agents"), "{text}");
    assert!(text.contains("├─ Pascal  [Coder]"), "{text}");
    assert!(text.contains("└─ Huygens  [Explorer]"), "{text}");
}

#[test]
fn option_b_delegate_group_keeps_agent_names_visible() {
    let mut pane = TranscriptPane::new(160, 30);
    let nova = option_b_running_delegate();
    let mut vega = option_b_delegate(
        "vega",
        "Vega",
        AgentRole::Explorer,
        AgentLifecycleState::Queued,
        "搜索历史卡片回归点",
    );
    vega.path = AgentPath::root_child(&vega.display_name);

    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 7,
        agent: nova,
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 7,
        agent: vega,
        workflow_origin: None,
    });
    let _ = pane.render_frame(160, 30);

    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Running 2 agents"), "{text}");
    assert!(text.contains("├─ Nova  [Coder]"), "{text}");
    assert!(text.contains("└─ Vega  [Explorer]"), "{text}");
    assert!(text.contains("• Used Read"), "{text}");
    assert!(text.contains("◌ thinking"), "{text}");
    assert!(text.contains("Waiting for scheduler slot"), "{text}");
    assert!(!text.contains("Coder · 角色对比测试"), "{text}");
}

#[test]
fn option_b_delegate_group_keeps_unmatched_running_tool_header() {
    let mut pane = TranscriptPane::new(160, 40);
    let mut first = running_delegate();
    first.id = AgentId::from_suffix_for_test("first_partial_group_absorb");
    first.display_name = AgentDisplayName::new("Pascal");
    first.path = AgentPath::root_child(&first.display_name);
    first.task_title = "resume 一个 completed agent".to_owned();

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 22,
        id: "tool_delegate_partial_pascal".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "6*7"}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 22,
        id: "tool_delegate_partial_huygens".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "7*8"}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 22,
        agent: first,
        workflow_origin: None,
    });

    let _ = pane.render_frame(160, 40);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Using Delegate"), "{text}");
    assert!(text.contains("Pascal  [Coder] · Delegate"), "{text}");
}

#[test]
fn option_b_delegate_group_suppresses_matching_finished_tool_and_keeps_failed_one() {
    let mut pane = TranscriptPane::new(160, 40);
    let mut first = running_delegate();
    first.id = AgentId::from_suffix_for_test("first_mixed_group_absorb");
    first.display_name = AgentDisplayName::new("Pascal");
    first.path = AgentPath::root_child(&first.display_name);
    first.task_title = "resume 一个 completed agent".to_owned();

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 23,
        id: "tool_delegate_mixed_pascal".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "6*7"}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 23,
        id: "tool_delegate_mixed_huygens".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "7*8"}),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 23,
        agent: first,
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 23,
        id: "tool_delegate_mixed_pascal".to_owned(),
        name: "Delegate".to_owned(),
        result: neo_agent_core::ToolResult::ok("matched delegate result should hide").with_details(
            serde_json::json!({
                "kind": "delegate",
                "agent_id": "agent_first_mixed_group_absorb"
            }),
        ),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 23,
        id: "tool_delegate_mixed_huygens".to_owned(),
        name: "Delegate".to_owned(),
        result: neo_agent_core::ToolResult::error("second delegate failed before starting"),

        workflow_origin: None,
        output_ref: None,
    });

    let _ = pane.render_frame(160, 40);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("Used Delegate"), "{text}");
    assert!(
        !text.contains("matched delegate result should hide"),
        "{text}"
    );
    assert!(text.contains("Failed Delegate"), "{text}");
    assert!(
        text.contains("second delegate failed before starting"),
        "{text}"
    );
    assert!(text.contains("Pascal  [Coder] · Delegate"), "{text}");
}

#[test]
fn same_turn_delegate_updates_do_not_duplicate_the_same_agent_in_a_group() {
    let mut pane = TranscriptPane::new(140, 40);
    let mut started = running_delegate();
    started.id = AgentId::from_suffix_for_test("single-agent");
    started.display_name = AgentDisplayName::new("Ada");
    started.path = AgentPath::root_child(&started.display_name);
    started.task_title = "context=none 测试".to_owned();
    started.role = AgentRole::Explorer;

    let mut updated = started.clone();
    updated.token_count = 18_500;
    updated.elapsed = Duration::from_secs(4);
    updated.latest_text = Some("Running...".to_owned());

    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 10,
        agent: started,
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateUpdated {
        turn: 10,
        agent: updated,
        workflow_origin: None,
    });

    let _ = pane.render_frame(140, 40);
    let frame = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!frame.contains("Running 2 agents"), "{frame}");
    assert_eq!(frame.matches("Ada  [Explorer]").count(), 1, "{frame}");
    assert!(frame.contains("running"), "{frame}");
    assert_eq!(frame.matches("context=none 测试").count(), 1, "{frame}");
}

#[test]
fn same_turn_root_delegates_render_as_one_live_group() {
    let mut pane = TranscriptPane::new(140, 40);
    let mut first = running_delegate();
    first.id = AgentId::from_suffix_for_test("first");
    first.display_name = AgentDisplayName::new("Gibbs");
    first.path = AgentPath::root_child(&first.display_name);
    first.task_title = "PlanBox border fix".to_owned();

    let mut second = running_delegate();
    second.id = AgentId::from_suffix_for_test("second");
    second.display_name = AgentDisplayName::new("Ada");
    second.path = AgentPath::root_child(&second.display_name);
    second.role = AgentRole::Explorer;
    second.task_title = "Trace markdown width".to_owned();
    second.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "read_1".to_owned(),
            name: "Read".to_owned(),
            summary: Some("crates/neo-tui/src/markdown.rs".to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    }];
    second.tool_count = 1;

    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 9,
        agent: first,
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 9,
        agent: second,
        workflow_origin: None,
    });

    let _ = pane.render_frame(140, 40);
    let frame = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(frame.contains("Running 2 agents"), "{frame}");
    assert!(frame.contains("├─ Gibbs  [Coder]"), "{frame}");
    assert!(frame.contains("PlanBox border fix"), "{frame}");
    assert!(frame.contains("└─ Ada  [Explorer]"), "{frame}");
    assert!(frame.contains("Trace markdown width"), "{frame}");
    assert!(frame.contains("Used Read"), "{frame}");
    assert_eq!(frame.matches("Agent Running").count(), 0, "{frame}");
}
