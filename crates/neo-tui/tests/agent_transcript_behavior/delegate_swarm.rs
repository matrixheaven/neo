use neo_agent_core::AgentEvent;
use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentProgressSnapshot, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome,
    AgentTerminalReason, AgentToolActivityPhase, AgentToolOutputPreview, DelegateContext,
    SwarmAggregate, SwarmChildProgress, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Expandable, strip_ansi};
use neo_tui::transcript::{SwarmCardComponent, TranscriptPane};
use std::time::Duration;

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
fn plain(lines: Vec<neo_tui::primitive::Line>) -> Vec<String> {
    lines
        .into_iter()
        .map(|l| strip_ansi(&l.to_ansi()))
        .collect()
}
fn out_of_order_swarm_updates() -> [SwarmSnapshot; 3] {
    let first = AgentSnapshot {
        display_name: AgentDisplayName::new("Zeno"),
        path: AgentPath::root_child(&AgentDisplayName::new("Zeno")),
        state: AgentLifecycleState::Running,
        latest_text: Some("alpha running".to_owned()),
        activity: Vec::new(),
        ..running_delegate()
    };
    let second = AgentSnapshot {
        id: AgentId::from_suffix_for_test("second"),
        display_name: AgentDisplayName::new("Gibbs"),
        path: AgentPath::root_child(&AgentDisplayName::new("Gibbs")),
        state: AgentLifecycleState::Running,
        latest_text: Some("beta running".to_owned()),
        activity: Vec::new(),
        ..running_delegate()
    };
    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "alpha prompt".to_owned(),
            agent: AgentSnapshot {
                state: AgentLifecycleState::Queued,
                latest_text: None,
                ..first.clone()
            },
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "beta prompt".to_owned(),
            agent: AgentSnapshot {
                state: AgentLifecycleState::Queued,
                latest_text: None,
                ..second.clone()
            },
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let started = SwarmSnapshot {
        swarm_id: "swarm-out-of-order".to_owned(),
        description: "merge test".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 2,
        aggregate,
        children,
    };
    let newer = SwarmSnapshot {
        children: vec![
            SwarmChildSnapshot {
                item_index: 0,
                item: "alpha prompt".to_owned(),
                agent: first.clone(),
            },
            SwarmChildSnapshot {
                item_index: 1,
                item: "beta prompt".to_owned(),
                agent: second.clone(),
            },
        ],
        ..started.clone()
    };
    let stale = SwarmSnapshot {
        children: vec![
            SwarmChildSnapshot {
                item_index: 0,
                item: "alpha prompt".to_owned(),
                agent: first,
            },
            SwarmChildSnapshot {
                item_index: 1,
                item: "beta prompt".to_owned(),
                agent: AgentSnapshot {
                    state: AgentLifecycleState::Queued,
                    latest_text: None,
                    ..second
                },
            },
        ],
        ..started.clone()
    };

    [started, newer, stale]
}
fn swarm_with_child_states(states: Vec<AgentLifecycleState>) -> SwarmSnapshot {
    let aggregate = SwarmAggregate::from_states(states.iter().copied());
    SwarmSnapshot {
        swarm_id: "swarm_test".to_owned(),
        description: "Test swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: states.len().max(1),
        aggregate,
        children: states
            .into_iter()
            .enumerate()
            .map(|(index, state)| {
                let name = AgentDisplayName::new(format!("Agent{index}"));
                SwarmChildSnapshot {
                    item_index: index + 1,
                    item: format!("item-{index}"),
                    agent: AgentSnapshot {
                        id: AgentId::from_suffix_for_test(&format!("swarm_child_{index}")),
                        display_name: name.clone(),
                        path: AgentPath::swarm_child("swarm_test", &name),
                        role: AgentRole::Coder,
                        mode: AgentRunMode::Foreground,
                        context: DelegateContext::Inherit,
                        state,
                        task_title: format!("Child {index}"),
                        task: format!("Child prompt {index}"),
                        created_at_ms: 1,
                        updated_at_ms: 1,
                        started_at_ms: (state == AgentLifecycleState::Running).then_some(1),
                        terminal_at_ms: state.is_terminal().then_some(2),
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
                    },
                }
            })
            .collect(),
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

#[test]
fn expanded_swarm_child_uses_delegate_activity_rules() {
    let mut snapshot = swarm_with_child_states(vec![AgentLifecycleState::Completed]);
    snapshot.children[0].agent.activity = vec![
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "bash_1".to_owned(),
                name: "Bash".to_owned(),
                summary: Some("printf 2".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: Some(AgentToolOutputPreview {
                    text: "1\n2\n3".to_owned(),
                    is_error: false,
                    truncated: false,
                    tail: false,
                }),
                files: Vec::new(),
                output_ref: None,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "thinking one\nthinking two".to_owned(),
                thinking: true,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "expanded child body text".to_owned(),
                thinking: false,
            },
        },
    ];
    snapshot.children[0].agent.outcome = Some(AgentTerminalOutcome {
        summary: "final child summary".to_owned(),
        is_error: false,
    });

    let mut card = SwarmCardComponent::new(snapshot);
    card.set_expanded(true);
    let rows = card.render_with_theme(120, &TuiTheme::default());
    let rows = plain(rows);
    let text = rows.join("\n");

    assert_eq!(text.matches('◌').count(), 1, "{text}");
    assert!(text.contains("Used Bash (printf 2)"), "{text}");
    assert!(rows.iter().any(|row| row.trim() == "2"), "{text}");
    assert!(rows.iter().any(|row| row.trim() == "3"), "{text}");
    assert!(!rows.iter().any(|row| row.trim() == "1"), "{text}");
    let body_index = rows
        .iter()
        .position(|row| row.contains("│ expanded child body text"))
        .expect("body row");
    let final_index = rows
        .iter()
        .position(|row| row.contains("└ final child summary"))
        .expect("final row");
    assert!(body_index < final_index, "{text}");
}

#[test]
fn option_b_collapsed_swarm_shows_names_badges_and_progress() {
    let mut iris = option_b_delegate(
        "iris",
        "Iris",
        AgentRole::Planner,
        AgentLifecycleState::Completed,
        "planner item",
    );
    iris.tool_count = 3;
    iris.token_count = 6_468_100;
    iris.input_token_count = 6_390_000;
    iris.cache_read_token_count = 6_300_000;
    iris.elapsed = Duration::from_secs(12);
    iris.terminal_at_ms = Some(12_000);
    iris.terminal_reason = Some(AgentTerminalReason::Completed);
    iris.outcome = Some(AgentTerminalOutcome {
        summary: "Plan is ready".to_owned(),
        is_error: false,
    });

    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "coder item".to_owned(),
            agent: option_b_running_delegate(),
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "planner item".to_owned(),
            agent: iris,
        },
        SwarmChildSnapshot {
            item_index: 2,
            item: "explorer item".to_owned(),
            agent: option_b_delegate(
                "vega",
                "Vega",
                AgentRole::Explorer,
                AgentLifecycleState::Running,
                "搜索历史卡片回归点",
            ),
        },
        SwarmChildSnapshot {
            item_index: 3,
            item: "queued item".to_owned(),
            agent: option_b_delegate(
                "rune",
                "Rune",
                AgentRole::Coder,
                AgentLifecycleState::Queued,
                "queued renderer task",
            ),
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "option-b-swarm".to_owned(),
        description: "角色对比测试".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 2,
        aggregate,
        children,
    };

    let rows =
        plain(SwarmCardComponent::new(snapshot).render_with_theme(160, &TuiTheme::default()));
    let text = rows.join("\n");
    let header = rows.first().expect("swarm header");

    assert!(
        text.contains("DelegateSwarm · running · 角色对比测试"),
        "{text}"
    );
    assert!(header.contains("progress ["), "{text}");
    assert!(!header.contains("bayes estimate"), "{text}");
    assert!(
        !rows.iter().any(|row| row.starts_with("  progress [")),
        "progress belongs in the swarm summary header, not its own child-like row: {text}"
    );
    assert!(text.contains("Nova  [Coder]"), "{text}");
    assert!(text.contains("Iris  [Planner]"), "{text}");
    assert!(text.contains("Vega  [Explorer]"), "{text}");
    assert!(text.contains("Rune  [Coder]"), "{text}");
    assert!(text.contains("Using Bash"), "{text}");
    assert!(text.contains("queued"), "{text}");
    assert!(text.contains("6.5M tok"), "{text}");
    assert!(text.contains("cache 6.3M read · hit 98.6%"), "{text}");
    assert!(
        !text.contains("001 "),
        "index numbers are not the primary visual language: {text}"
    );
}

#[test]
fn option_b_expanded_swarm_preserves_full_child_transcripts() {
    let mut nova = option_b_running_delegate();
    nova.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: "All edits applied. Now let me verify the paths.".to_owned(),
            thinking: false,
        },
    });
    let mut iris = option_b_delegate(
        "iris-expanded",
        "Iris",
        AgentRole::Planner,
        AgentLifecycleState::Completed,
        "Plan renderer work",
    );
    iris.tool_count = 2;
    iris.token_count = 8_200;
    iris.elapsed = Duration::from_secs(12);
    iris.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "read-plan".to_owned(),
            name: "Read".to_owned(),
            summary: Some("docs/aegis/plans/...".to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    }];
    iris.outcome = Some(AgentTerminalOutcome {
        summary: "The implementation should stay inside transcript cards.".to_owned(),
        is_error: false,
    });

    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "nova".to_owned(),
            agent: nova,
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "iris".to_owned(),
            agent: iris,
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "option-b-expanded".to_owned(),
        description: "角色对比测试".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 2,
        aggregate,
        children,
    };
    let mut card = SwarmCardComponent::new(snapshot);
    card.set_expanded(true);

    let rows = plain(card.render_with_theme(160, &TuiTheme::default()));
    let text = rows.join("\n");

    assert!(text.contains("├─ Nova  [Coder]"), "{text}");
    assert!(text.contains("└─ Iris  [Planner]"), "{text}");
    assert!(
        text.contains("  ├─ Nova  [Coder]  running · 21s · 3 tools · 22.7k tok"),
        "{text}"
    );
    assert!(
        text.contains("  └─ Iris  [Planner]  done · 12s · 2 tools · 8.2k tok"),
        "{text}"
    );
    assert!(
        text.contains("• Used Read (crates/neo-agent-core/src/tools/delegate.rs)"),
        "{text}"
    );
    assert!(
        text.contains("• Using Bash (cargo nextest run -p neo-agent-core ...)"),
        "{text}"
    );
    assert!(text.contains("◌ thinking"), "{text}");
    assert!(text.contains("│ All edits applied"), "{text}");
    assert!(
        text.contains("└ The implementation should stay inside transcript cards."),
        "{text}"
    );
}

#[test]
fn option_b_swarm_absorption_keeps_completed_mismatched_tool_when_snapshot_arrives_late() {
    let mut pane = TranscriptPane::new(160, 30);
    let snapshot = swarm_with_child_states(vec![
        AgentLifecycleState::Running,
        AgentLifecycleState::Queued,
    ]);

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 19,
        id: "tool_swarm_mismatch_before_snapshot".to_owned(),
        name: "DelegateSwarm".to_owned(),
        arguments: serde_json::json!({
            "description": "Test swarm",
            "max_concurrency": 2
        }),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 19,
        id: "tool_swarm_mismatch_before_snapshot".to_owned(),
        name: "DelegateSwarm".to_owned(),
        result: neo_agent_core::ToolResult::ok("swarm_id: swarm_other").with_details(
            serde_json::json!({
                "kind": "delegate_swarm",
                "swarm_id": "swarm_other"
            }),
        ),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 19,
        swarm: snapshot,
        workflow_origin: None,
    });

    let _ = pane.render_frame(160, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Used DelegateSwarm"), "{text}");
    assert!(text.contains("swarm_id: swarm_other"), "{text}");
    assert!(text.contains("DelegateSwarm · running"), "{text}");
}

#[test]
fn option_b_swarm_absorption_restores_failed_tool_result() {
    let mut pane = TranscriptPane::new(160, 30);
    let snapshot = swarm_with_child_states(vec![
        AgentLifecycleState::Running,
        AgentLifecycleState::Queued,
    ]);

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 18,
        id: "tool_swarm_failed".to_owned(),
        name: "DelegateSwarm".to_owned(),
        arguments: serde_json::json!({
            "description": "Test swarm",
            "max_concurrency": 2
        }),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 18,
        swarm: snapshot,
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 18,
        id: "tool_swarm_failed".to_owned(),
        name: "DelegateSwarm".to_owned(),
        result: neo_agent_core::ToolResult::error("swarm failed before returning ids"),

        workflow_origin: None,
        output_ref: None,
    });

    let _ = pane.render_frame(160, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Failed DelegateSwarm"), "{text}");
    assert!(text.contains("swarm failed before returning ids"), "{text}");
    assert!(text.contains("DelegateSwarm · running"), "{text}");
}

#[test]
fn option_b_swarm_absorption_suppresses_matching_tool_result_details() {
    let mut pane = TranscriptPane::new(160, 30);
    let snapshot = swarm_with_child_states(vec![
        AgentLifecycleState::Running,
        AgentLifecycleState::Queued,
    ]);

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 21,
        id: "tool_swarm_matched_result".to_owned(),
        name: "DelegateSwarm".to_owned(),
        arguments: serde_json::json!({
            "description": "Test swarm",
            "max_concurrency": 2
        }),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 21,
        swarm: snapshot,
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 21,
        id: "tool_swarm_matched_result".to_owned(),
        name: "DelegateSwarm".to_owned(),
        result: neo_agent_core::ToolResult::ok("swarm_id: swarm_test").with_details(
            serde_json::json!({
                "kind": "delegate_swarm",
                "swarm_id": "swarm_test"
            }),
        ),

        workflow_origin: None,
        output_ref: None,
    });

    let _ = pane.render_frame(160, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("Using DelegateSwarm"), "{text}");
    assert!(!text.contains("Used DelegateSwarm"), "{text}");
    assert!(text.contains("DelegateSwarm · running"), "{text}");
    assert!(text.contains("swarm_test"), "{text}");
}

#[test]
fn option_b_swarm_transcript_absorbs_matching_tool_header() {
    let mut pane = TranscriptPane::new(160, 30);
    let snapshot = swarm_with_child_states(vec![
        AgentLifecycleState::Running,
        AgentLifecycleState::Queued,
    ]);

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 13,
        id: "tool_delegate_swarm".to_owned(),
        name: "DelegateSwarm".to_owned(),
        arguments: serde_json::json!({
            "description": "Test swarm",
            "max_concurrency": 2
        }),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 13,
        swarm: snapshot,
        workflow_origin: None,
    });

    let _ = pane.render_frame(160, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!text.contains("Using DelegateSwarm"), "{text}");
    assert!(!text.contains("Used DelegateSwarm"), "{text}");
    assert!(text.contains("DelegateSwarm · running"), "{text}");
    assert!(text.contains("progress ["), "{text}");
    assert!(!text.contains("bayes estimate"), "{text}");
}

#[test]
fn swarm_progress_applies_text_delta() {
    let mut pane = TranscriptPane::new(160, 30);
    let started = swarm_with_child_states(vec![AgentLifecycleState::Queued]);
    let child = started.children[0].clone();
    let mut updated = child.agent.clone();
    updated.state = AgentLifecycleState::Running;
    updated.updated_at_ms += 1;
    updated.latest_text = Some("latest".to_owned());
    let aggregate = SwarmAggregate::from_states([AgentLifecycleState::Running]);

    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 1,
        swarm: started,
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmProgressUpdated {
        turn: 1,
        swarm_id: "swarm_test".to_owned(),
        state: AgentLifecycleState::Running,
        aggregate,
        child_progress: SwarmChildProgress {
            item_index: child.item_index,
            progress: AgentProgressSnapshot::from_agent(&updated),
        },
        workflow_origin: None,
    });

    let _ = pane.render_frame(160, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("latest"), "{text}");
    assert!(text.contains("running"), "{text}");
}

#[test]
fn swarm_progress_preserves_started_time_for_live_elapsed_ticks() {
    let mut pane = TranscriptPane::new(160, 30);
    let queued = swarm_with_child_states(vec![AgentLifecycleState::Queued]);
    let mut running = queued.children[0].agent.clone();
    running.state = AgentLifecycleState::Running;
    running.started_at_ms = Some(1_000);
    running.updated_at_ms = 8_000;
    running.elapsed = Duration::from_secs(7);
    running.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "bash-1".to_owned(),
            name: "Bash".to_owned(),
            summary: Some("cargo test".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    });

    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 7,
        swarm: queued,
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmProgressUpdated {
        turn: 7,
        swarm_id: "swarm_test".to_owned(),
        state: AgentLifecycleState::Running,
        aggregate: SwarmAggregate::from_states([AgentLifecycleState::Running]),
        child_progress: SwarmChildProgress {
            item_index: 1,
            progress: AgentProgressSnapshot::from_agent(&running),
        },
        workflow_origin: None,
    });

    let _ = pane.render_frame(160, 30);
    pane.advance_animation_at_ms(61_000);
    let frame = pane.render_frame(160, 30).unwrap_or_default().join("\n");

    assert!(frame.contains("1m 0s"), "{frame}");
}

#[test]
fn swarm_progress_starts_at_zero_then_moves_after_running_activity() {
    let mut card = SwarmCardComponent::new(swarm_with_child_states(vec![
        AgentLifecycleState::Queued,
        AgentLifecycleState::Queued,
    ]));

    let queued = plain(card.render_with_theme(140, &TuiTheme::default())).join("\n");
    assert!(queued.contains("0%") || queued.contains("1%"), "{queued}");
    assert!(!queued.contains("100%"), "{queued}");

    let mut running = card.snapshot().clone();
    running.children[0].agent.state = AgentLifecycleState::Running;
    running.children[0].agent.started_at_ms = Some(1_000);
    running.children[0].agent.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "call_1".to_owned(),
            name: "Read".to_owned(),
            summary: Some("README.md".to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    });
    card.update(running);
    card.on_render_tick(2_000);

    let frame = plain(card.render_with_theme(140, &TuiTheme::default())).join("\n");
    assert!(frame.contains("Working"), "{frame}");
    assert!(!frame.contains("100%"), "{frame}");
    assert!(frame.contains("Used Read"), "{frame}");
}

#[test]
fn transcript_pane_merges_out_of_order_swarm_updates_without_regressing_children() {
    let mut pane = TranscriptPane::new(160, 30);
    let [started, newer, stale] = out_of_order_swarm_updates();

    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 1,
        swarm: started,
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmUpdated {
        turn: 1,
        swarm: newer,
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmUpdated {
        turn: 1,
        swarm: stale,
        workflow_origin: None,
    });

    let _ = pane.render_frame(160, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("alpha running"), "{text}");
    assert!(text.contains("beta running"), "{text}");
    assert!(!text.contains("002 [··········]"), "{text}");
}
