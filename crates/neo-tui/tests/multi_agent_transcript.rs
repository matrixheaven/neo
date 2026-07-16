use std::time::Duration;

use neo_agent_core::AgentEvent;
use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentProgressSnapshot, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome,
    AgentTerminalReason, AgentToolActivityPhase, AgentToolOutputPreview, DelegateContext,
    SwarmAggregate, SwarmChildProgress, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Color, Component, Expandable, Line, strip_ansi};
use neo_tui::transcript::{
    DelegateCardComponent, DelegateGroupComponent, SwarmCardComponent, TranscriptEntry,
    TranscriptPane,
};

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
                },
            },
            AgentActivityEntry {
                kind: AgentActivityKind::Tool {
                    id: "grep-1".to_owned(),
                    name: "Grep".to_owned(),
                    summary: Some("from_spans|pub struct Span|pub struct Line".to_owned()),
                    phase: AgentToolActivityPhase::Failed,
                    output: None,
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

fn ansi(lines: &[Line]) -> String {
    lines
        .iter()
        .map(Line::to_ansi)
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_ansi_contains_color(ansi: &str, color: Color) {
    let expected = match color {
        Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        Color::Indexed(n) => format!("\x1b[38;5;{n}m"),
        _ => return,
    };
    assert!(
        ansi.contains(&expected),
        "missing color {expected:?} in {ansi:?}"
    );
}

#[test]
fn delegate_card_renders_kimi_style_running_summary() {
    let mut card = DelegateCardComponent::new(running_delegate());

    let rows = plain(card.render(180));
    let text = rows.join("\n");

    assert!(text.contains("● Gibbs  [Coder] · Delegate"), "{text}");
    assert!(text.contains("running"), "{text}");
    assert!(text.contains("3 tools"), "{text}");
    assert!(text.contains("24s"), "{text}");
    assert!(text.contains("25.6k tok"), "{text}");
    assert!(text.contains("Press Ctrl+B to run in background"), "{text}");
    assert!(text.contains("• Used Read"), "{text}");
    assert!(text.contains("✗ Used Grep"), "{text}");
    assert!(text.contains("◌ thinking"), "{text}");
    assert!(text.contains("Let me start by reading"), "{text}");
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
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 7,
        agent: vega,
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
        });
    }
    let committed = pane.render_terminal_update(160, 30);
    assert!(!committed.history.is_empty());
    pane.acknowledge_history(&committed.history);

    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 7,
        agent: option_b_delegate(
            "later_euler",
            "Euler",
            AgentRole::Reviewer,
            AgentLifecycleState::Running,
            "review committed output",
        ),
    });
    let update = pane.render_terminal_update(160, 30);
    let live = update
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(live.contains("Euler"), "live transcript:\n{live}");
}

#[test]
fn compact_delegate_progress_replays_as_delegate_card() {
    let mut pane = TranscriptPane::new(160, 30);
    let mut started = option_b_delegate(
        "compact",
        "Compact",
        AgentRole::Coder,
        AgentLifecycleState::Running,
        "compact progress replay",
    );
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 11,
        agent: started.clone(),
    });

    started.latest_text = Some("persisted compact progress".to_owned());
    started.tool_count = 1;
    started.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "read-compact".to_owned(),
            name: "Read".to_owned(),
            summary: Some("crates/neo-agent-core/src/events.rs".to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
        },
    });
    pane.apply_agent_event(AgentEvent::DelegateProgressUpdated {
        turn: 11,
        progress: started.progress_snapshot(),
    });
    let _ = pane.render_frame(160, 30);

    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Compact  [Coder]"), "{text}");
    assert!(text.contains("1 tool"), "{text}");
    assert!(text.contains("• Used Read"), "{text}");
    assert!(text.contains("persisted compact progress"), "{text}");
}

#[test]
fn in_place_card_updates_preserve_active_thinking() {
    let mut pane = TranscriptPane::new(160, 30);
    let mut delegate = running_delegate();
    let mut swarm = swarm_with_child_states(vec![AgentLifecycleState::Queued]);
    let swarm_child = swarm.children[0].clone();

    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 1,
        agent: delegate.clone(),
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 1,
        swarm: swarm.clone(),
    });
    pane.apply_agent_event(AgentEvent::ThinkingStarted {
        turn: 2,
        id: "reasoning".to_owned(),
    });
    pane.apply_agent_event(AgentEvent::ThinkingDelta {
        turn: 2,
        text: "Arch".to_owned(),
    });

    delegate.updated_at_ms += 1;
    delegate.latest_text = Some("delegate update".to_owned());
    pane.apply_agent_event(AgentEvent::DelegateUpdated {
        turn: 1,
        agent: delegate.clone(),
    });
    pane.apply_agent_event(AgentEvent::ThinkingDelta {
        turn: 2,
        text: "i".to_owned(),
    });

    delegate.updated_at_ms += 1;
    delegate.latest_text = Some("delegate progress".to_owned());
    pane.apply_agent_event(AgentEvent::DelegateProgressUpdated {
        turn: 1,
        progress: AgentProgressSnapshot::from_agent(&delegate),
    });
    pane.apply_agent_event(AgentEvent::ThinkingDelta {
        turn: 2,
        text: "m".to_owned(),
    });

    swarm.children[0].agent.state = AgentLifecycleState::Running;
    swarm.children[0].agent.updated_at_ms += 1;
    swarm.children[0].agent.latest_text = Some("swarm update".to_owned());
    swarm.aggregate = SwarmAggregate::from_states([AgentLifecycleState::Running]);
    swarm.state = swarm.aggregate.status();
    pane.apply_agent_event(AgentEvent::DelegateSwarmUpdated { turn: 1, swarm });
    pane.apply_agent_event(AgentEvent::ThinkingDelta {
        turn: 2,
        text: "e".to_owned(),
    });

    let mut updated_child = swarm_child.agent;
    updated_child.state = AgentLifecycleState::Running;
    updated_child.updated_at_ms += 1;
    updated_child.latest_text = Some("swarm progress".to_owned());
    let aggregate = SwarmAggregate::from_states([AgentLifecycleState::Running]);
    pane.apply_agent_event(AgentEvent::DelegateSwarmProgressUpdated {
        turn: 1,
        swarm_id: "swarm_test".to_owned(),
        state: AgentLifecycleState::Running,
        aggregate,
        child_progress: SwarmChildProgress {
            item_index: swarm_child.item_index,
            progress: AgentProgressSnapshot::from_agent(&updated_child),
        },
    });
    pane.apply_agent_event(AgentEvent::ThinkingDelta {
        turn: 2,
        text: "des".to_owned(),
    });
    pane.apply_agent_event(AgentEvent::ThinkingFinished {
        turn: 2,
        signature: None,
        redacted: false,
    });

    let thinking = pane
        .transcript()
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::ThinkingBlock { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(thinking, vec!["Archimedes"]);

    let _ = pane.render_frame(160, 30);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("delegate progress"), "{text}");
    assert!(text.contains("swarm progress"), "{text}");
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
fn delegate_group_child_rows_keep_left_border_muted() {
    let theme = TuiTheme::default();
    let mut nova = option_b_running_delegate();
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
    iris.token_count = 8_200;
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
            summary: Some("docs/superpowers/plans/...".to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
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
    assert!(text.contains("• Used Read"), "{text}");
    assert!(text.contains("• Using Bash"), "{text}");
    assert!(text.contains("◌ thinking"), "{text}");
    assert!(text.contains("│ All edits applied"), "{text}");
    assert!(
        text.contains("└ The implementation should stay inside transcript cards."),
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
        .position(|row| row.contains("• Used Read"))
        .expect("used row");
    let using_index = rows
        .iter()
        .position(|row| row.contains("• Using Bash"))
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
fn delegate_card_marks_unfinished_tool_as_using_with_neutral_marker() {
    let theme = TuiTheme::default()
        .with_text_primary(Color::Rgb(230, 230, 230))
        .with_status_ok(Color::Rgb(1, 220, 120));
    let mut snapshot = running_delegate();
    snapshot.tool_count = 1;
    snapshot.activity = vec![
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "read-1".to_owned(),
                name: "Read".to_owned(),
                summary: Some("crates/neo-tui/src/transcript/delegate_card.rs".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: None,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "bash-1".to_owned(),
                name: "Bash".to_owned(),
                summary: Some(
                    "cargo nextest run -p neo-tui --test multi_agent_transcript".to_owned(),
                ),
                phase: AgentToolActivityPhase::Ongoing,
                output: None,
            },
        },
    ];

    let rows = DelegateCardComponent::new(snapshot).render_with_theme(140, &theme);
    let plain_rows = plain(rows.clone());
    let text = plain_rows.join("\n");

    assert!(text.contains("• Used Read"), "{text}");
    assert!(text.contains("• Using Bash"), "{text}");
    let using_line = rows
        .iter()
        .find(|row| strip_ansi(&row.to_ansi()).contains("Using Bash"))
        .expect("using line")
        .to_ansi();
    assert_ansi_contains_color(&using_line, theme.text_primary);
    assert!(
        !using_line.contains("\u{1b}[38;2;1;220;120m"),
        "pending tool marker should be neutral, not success green: {using_line:?}"
    );
}

#[test]
fn delegate_card_uses_short_title_and_keeps_stats_visible_for_long_prompts() {
    let mut snapshot = running_delegate();
    snapshot.task = "Look up the line count of crates/neo-agent-core/src/lib.rs using `wc -l` and report back. Reply with exactly one line: `<count> lines` where <count> is the actual number from wc -l. Do not modify any files.".to_owned();
    snapshot.latest_text = Some("34 lines".to_owned());

    let rows =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()));
    let text = rows.join("\n");

    assert!(text.contains("Gibbs  [Coder]"), "{text}");
    assert!(text.contains("running"), "{text}");
    assert!(!text.contains("1m?"), "{text}");
    assert!(text.contains("3 tools"), "{text}");
    assert!(text.contains("24s"), "{text}");
    assert!(text.contains("25.6k tok"), "{text}");
    assert!(
        !text.contains("Reply with exactly one line"),
        "header should not include the full prompt: {text}"
    );
}

#[test]
fn delegate_card_collapses_streamed_thinking_and_renders_single_final_body_line() {
    let theme = TuiTheme::default()
        .with_text_primary(Color::Rgb(210, 220, 230))
        .with_status_ok(Color::Rgb(1, 220, 120));
    let summary = "Acknowledged. Ready as Explorer subagent in summary mode. - Role: Explorer (read-only investigation, no edits) - Mode: summary (concise results) - Constraints: no git mutations, no destructive operations Awaiting task.";
    let snapshot = AgentSnapshot {
        role: AgentRole::Explorer,
        state: AgentLifecycleState::Completed,
        tool_count: 0,
        token_count: 234,
        elapsed: Duration::from_secs(2),
        activity: vec![
            AgentActivityEntry {
                kind: AgentActivityKind::Text {
                    text: "The user is asking me".to_owned(),
                    thinking: true,
                },
            },
            AgentActivityEntry {
                kind: AgentActivityKind::Text {
                    text: " to act as a bounded Neo subagent in Explorer role.".to_owned(),
                    thinking: true,
                },
            },
            AgentActivityEntry {
                kind: AgentActivityKind::Text {
                    text: summary.to_owned(),
                    thinking: false,
                },
            },
        ],
        outcome: Some(AgentTerminalOutcome {
            summary: summary.to_owned(),
            is_error: false,
        }),
        ..running_delegate()
    };

    let rows = DelegateCardComponent::new(snapshot).render_with_theme(120, &theme);
    let plain_rows = plain(rows.clone());
    let text = plain_rows.join("\n");

    assert!(text.contains("Gibbs  [Explorer]"), "{text}");
    assert!(text.contains("done"), "{text}");
    assert_eq!(text.matches('\u{25cc}').count(), 1, "{text}");
    assert_eq!(text.matches('\u{2514}').count(), 1, "{text}");

    let thinking_index = plain_rows
        .iter()
        .position(|row| row.contains('\u{25cc}'))
        .expect("thinking row");
    let final_index = plain_rows
        .iter()
        .position(|row| row.contains('\u{2514}'))
        .expect("final row");
    assert!(thinking_index < final_index, "{text}");
    assert_eq!(final_index, plain_rows.len() - 1, "{text}");
    assert!(plain_rows[final_index].contains("..."), "{text}");

    let final_ansi = rows[final_index].to_ansi();
    assert_ansi_contains_color(&final_ansi, theme.text_primary);
    assert!(
        !final_ansi.contains("\u{1b}[38;2;1;220;120m"),
        "final body row should not be rendered in success green: {final_ansi:?}"
    );
}

#[test]
fn delegate_card_suppresses_normalized_duplicate_final_body() {
    let summary =
        "All Wave 1 tasks are complete. Here's the summary: ## Wave 1 Implementation Summary";
    let streamed_body =
        "All Wave1 tasks are complete. Here's the summary: ##Wave1 Implementation Summary";
    let snapshot = AgentSnapshot {
        state: AgentLifecycleState::Completed,
        tool_count: 0,
        token_count: 234,
        elapsed: Duration::from_secs(2),
        activity: vec![AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: streamed_body.to_owned(),
                thinking: false,
            },
        }],
        outcome: Some(AgentTerminalOutcome {
            summary: summary.to_owned(),
            is_error: false,
        }),
        ..running_delegate()
    };

    let plain_rows =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()));
    let text = plain_rows.join("\n");

    assert!(
        !plain_rows.iter().any(|row| row.contains("│ All Wave")),
        "duplicate final body preview must be suppressed: {text}"
    );
    assert!(text.contains("└ All Wave 1 tasks are complete"), "{text}");
    assert_eq!(text.matches("All Wave").count(), 1, "{text}");
}

#[test]
fn delegate_card_suppresses_body_when_final_starts_with_it() {
    let body = "I have enough to report. Let me also confirm the path.";
    let summary = "I have enough to report. Let me also confirm the path. Then I will finalize.";
    let snapshot = AgentSnapshot {
        state: AgentLifecycleState::Completed,
        terminal_at_ms: Some(31_000),
        terminal_reason: Some(AgentTerminalReason::Completed),
        activity: Vec::new(),
        latest_text: Some(body.to_owned()),
        outcome: Some(AgentTerminalOutcome {
            summary: summary.to_owned(),
            is_error: false,
        }),
        ..option_b_running_delegate()
    };

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()))
            .join("\n");

    assert!(
        !text.contains("│ I have enough"),
        "body preview must be suppressed when final starts with it: {text}"
    );
    assert!(text.contains("└ I have enough"), "{text}");
}

#[test]
fn delegate_card_trims_activity_to_recent_kimi_style_window() {
    let mut snapshot = running_delegate();
    snapshot.activity = (0..8)
        .map(|index| AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: format!("bash-{index}"),
                name: "Bash".to_owned(),
                summary: Some(format!("command-{index}")),
                phase: AgentToolActivityPhase::Done,
                output: None,
            },
        })
        .collect();

    let rows =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()));
    let text = rows.join("\n");

    assert!(!text.contains("command-0"), "{text}");
    assert!(!text.contains("command-3"), "{text}");
    assert!(text.contains("command-4"), "{text}");
    assert!(text.contains("command-7"), "{text}");
}

#[test]
fn swarm_card_renders_orchestrating_before_children_run() {
    let child = AgentSnapshot {
        state: AgentLifecycleState::Queued,
        ..running_delegate()
    };
    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "Search tools: Grep, Find".to_owned(),
        agent: child,
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-1".to_owned(),
        description: "Audit and fix Neo tool schemas".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 1,
        aggregate,
        children,
    };
    let mut card = SwarmCardComponent::new(snapshot);

    let rows = plain(card.render(180));
    let text = rows.join("\n");

    assert!(
        text.contains("DelegateSwarm · queued · Audit and fix Neo tool schemas"),
        "{text}"
    );
    assert!(text.contains("progress ["), "{text}");
    assert!(text.contains("Gibbs  [Coder]"), "{text}");
    assert!(text.contains("0%"), "{text}");
    assert!(text.contains("Orchestrating"), "{text}");
    assert!(!text.contains("001 "), "{text}");
}

#[test]
fn swarm_card_renders_working_after_child_runs() {
    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "Search tools: Grep, Find".to_owned(),
        agent: running_delegate(),
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-1".to_owned(),
        description: "Audit and fix Neo tool schemas".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 1,
        aggregate,
        children,
    };
    let mut card = SwarmCardComponent::new(snapshot);

    let rows = plain(card.render(120));
    let text = rows.join("\n");

    assert!(text.contains("Working"), "{text}");
    assert!(text.contains("Working"), "{text}");
    assert!(!text.contains("###......."), "{text}");
}

#[test]
fn swarm_card_renders_scheduling_status_when_children_are_queued() {
    let running = running_delegate();
    let queued_a = option_b_delegate(
        "queued-a",
        "Hypatia",
        AgentRole::Coder,
        AgentLifecycleState::Queued,
        "count README.md",
    );
    let queued_b = option_b_delegate(
        "queued-b",
        "Athena",
        AgentRole::Coder,
        AgentLifecycleState::Queued,
        "count Cargo.toml",
    );
    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "count AGENTS.md".to_owned(),
            agent: running,
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "count README.md".to_owned(),
            agent: queued_a,
        },
        SwarmChildSnapshot {
            item_index: 2,
            item: "count Cargo.toml".to_owned(),
            agent: queued_b,
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-queued".to_owned(),
        description: "single-file counts".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 1,
        aggregate,
        children,
    };

    let rows =
        plain(SwarmCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()));
    let text = rows.join("\n");

    assert!(text.contains("Scheduling:"), "{text}");
    assert!(text.contains("1/3 running"), "{text}");
    assert!(text.contains("max concurrency 1"), "{text}");
    assert!(text.contains("2 queued"), "{text}");
}

#[test]
fn swarm_card_prefers_child_activity_over_original_item_text() {
    let mut child = running_delegate();
    child.activity.clear();
    child.latest_text = Some("34 lines".to_owned());
    child.outcome = Some(AgentTerminalOutcome {
        summary: "34 lines".to_owned(),
        is_error: false,
    });
    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "Look up the line count of crates/neo-agent-core/src/lib.rs using `wc -l` and report back. Reply with exactly one line: `<count> lines` where <count> is the actual number from wc -l. Do not modify any files.".to_owned(),
        agent: child,
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-1".to_owned(),
        description: "Read-only codebase investigations".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 1,
        aggregate,
        children,
    };

    let rows =
        plain(SwarmCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()));
    let text = rows.join("\n");

    assert!(text.contains("34 lines"), "{text}");
    assert!(
        !text.contains("Reply with exactly one line"),
        "swarm row should show dynamic child activity/result, not the full prompt: {text}"
    );
}

#[test]
fn transcript_pane_upserts_delegate_card_from_events() {
    let mut pane = TranscriptPane::new(120, 20);
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 1,
        agent: running_delegate(),
    });

    // Force a render so last_frame is populated.
    let _ = pane.render_frame(120, 20);
    let frame = pane.frame_ansi_lines();
    let text: String = frame
        .iter()
        .map(|l| strip_ansi(l))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Gibbs  [Coder]"), "{text}");
    assert!(text.contains("running"), "{text}");
}

#[test]
fn option_b_delegate_transcript_absorbs_matching_tool_header() {
    let mut pane = TranscriptPane::new(140, 30);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 11,
        id: "tool_delegate_single".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 11,
        agent: running_delegate(),
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
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 16,
        id: "tool_delegate_late".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),
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
fn option_b_delegate_absorption_suppresses_matching_tool_result_details() {
    let mut pane = TranscriptPane::new(140, 30);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 20,
        id: "tool_delegate_matched_result".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 20,
        agent: running_delegate(),
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
fn option_b_delegate_absorption_restores_failed_tool_result() {
    let mut pane = TranscriptPane::new(140, 30);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 14,
        id: "tool_delegate_failed".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 14,
        agent: running_delegate(),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 14,
        id: "tool_delegate_failed".to_owned(),
        name: "Delegate".to_owned(),
        result: neo_agent_core::ToolResult::error("delegate failed before snapshot settled"),
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
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 15,
        agent: running_delegate(),
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
fn option_b_delegate_absorption_keeps_completed_mismatched_tool_when_snapshot_arrives_late() {
    let mut pane = TranscriptPane::new(140, 30);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 17,
        id: "tool_delegate_mismatch_before_snapshot".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "answer 5+5"}),
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
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 17,
        agent: running_delegate(),
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
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 22,
        id: "tool_delegate_partial_huygens".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "7*8"}),
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 22,
        agent: first,
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
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 23,
        id: "tool_delegate_mixed_huygens".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "7*8"}),
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 23,
        agent: first,
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
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 23,
        id: "tool_delegate_mixed_huygens".to_owned(),
        name: "Delegate".to_owned(),
        result: neo_agent_core::ToolResult::error("second delegate failed before starting"),
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
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 12,
        id: "tool_delegate_huygens".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "7*8"}),
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 12,
        agent: first,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 12,
        agent: second,
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
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 13,
        swarm: snapshot,
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
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 21,
        swarm: snapshot,
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
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 18,
        swarm: snapshot,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 18,
        id: "tool_swarm_failed".to_owned(),
        name: "DelegateSwarm".to_owned(),
        result: neo_agent_core::ToolResult::error("swarm failed before returning ids"),
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
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 19,
        swarm: snapshot,
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
#[allow(clippy::too_many_lines)]
fn transcript_pane_merges_out_of_order_swarm_updates_without_regressing_children() {
    let mut pane = TranscriptPane::new(160, 30);
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

    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 1,
        swarm: started,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmUpdated {
        turn: 1,
        swarm: newer,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmUpdated {
        turn: 1,
        swarm: stale,
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

#[test]
fn delegate_card_styles_terminal_states_and_ctrl_b_only_for_foreground_running() {
    let theme = TuiTheme::default()
        .with_status_ok(Color::Rgb(1, 180, 90))
        .with_status_error(Color::Rgb(220, 20, 20))
        .with_status_warn(Color::Rgb(230, 160, 20));

    let completed = AgentSnapshot {
        state: AgentLifecycleState::Completed,
        outcome: Some(AgentTerminalOutcome {
            summary: "Merged focused fix".to_owned(),
            is_error: false,
        }),
        ..running_delegate()
    };
    let failed = AgentSnapshot {
        state: AgentLifecycleState::Failed,
        outcome: Some(AgentTerminalOutcome {
            summary: "Tests failed".to_owned(),
            is_error: true,
        }),
        ..running_delegate()
    };
    let cancelled = AgentSnapshot {
        state: AgentLifecycleState::Cancelled,
        outcome: Some(AgentTerminalOutcome {
            summary: "Stopped by user".to_owned(),
            is_error: false,
        }),
        ..running_delegate()
    };
    let background_running = AgentSnapshot {
        mode: AgentRunMode::Background,
        ..running_delegate()
    };

    let completed_rows = DelegateCardComponent::new(completed).render_with_theme(120, &theme);
    let failed_rows = DelegateCardComponent::new(failed).render_with_theme(120, &theme);
    let cancelled_rows = DelegateCardComponent::new(cancelled).render_with_theme(120, &theme);
    let background_rows =
        DelegateCardComponent::new(background_running).render_with_theme(120, &theme);

    let completed_ansi = ansi(&completed_rows);
    let failed_ansi = ansi(&failed_rows);
    let cancelled_ansi = ansi(&cancelled_rows);

    assert_ansi_contains_color(&completed_ansi, theme.status_ok);
    assert_ansi_contains_color(&failed_ansi, theme.status_error);
    assert_ansi_contains_color(&cancelled_ansi, theme.status_warn);
    assert!(
        completed_ansi.contains("Merged focused fix"),
        "{completed_ansi}"
    );
    assert!(failed_ansi.contains("Tests failed"), "{failed_ansi}");
    assert!(
        cancelled_ansi.contains("Stopped by user"),
        "{cancelled_ansi}"
    );
    assert!(
        !ansi(&background_rows).contains("Press Ctrl+B to run in background"),
        "{}",
        ansi(&background_rows)
    );
}

#[test]
fn swarm_card_uses_theme_styles_and_expanded_child_details() {
    let theme = TuiTheme::default()
        .with_brand(Color::Rgb(120, 80, 240))
        .with_status_ok(Color::Rgb(1, 180, 90))
        .with_status_error(Color::Rgb(220, 20, 20))
        .with_status_warn(Color::Rgb(230, 160, 20))
        .with_text_primary(Color::Rgb(210, 220, 230));
    let completed = AgentSnapshot {
        state: AgentLifecycleState::Completed,
        tool_count: 5,
        token_count: 4200,
        elapsed: Duration::from_secs(75),
        latest_text: Some("Collected candidate files".to_owned()),
        outcome: Some(AgentTerminalOutcome {
            summary: "Found two style gaps".to_owned(),
            is_error: false,
        }),
        ..running_delegate()
    };
    let failed = AgentSnapshot {
        state: AgentLifecycleState::Failed,
        display_name: AgentDisplayName::new("Ada"),
        path: AgentPath::root_child(&AgentDisplayName::new("Ada")),
        tool_count: 2,
        token_count: 850,
        elapsed: Duration::from_secs(9),
        latest_text: Some("cargo nextest failed".to_owned()),
        outcome: Some(AgentTerminalOutcome {
            summary: "Focused test failed".to_owned(),
            is_error: true,
        }),
        ..running_delegate()
    };
    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "audit transcript".to_owned(),
            agent: completed,
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "fix workflow".to_owned(),
            agent: failed,
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-style".to_owned(),
        description: "Style-rich swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 2,
        aggregate,
        children,
    };
    let mut card = SwarmCardComponent::new(snapshot);
    let collapsed = card.render_with_theme(140, &theme);
    card.set_expanded(true);
    let expanded = card.render_with_theme(140, &theme);
    let expanded_ansi = ansi(&expanded);
    let expanded_text = plain(expanded.clone()).join("\n");

    assert_ansi_contains_color(&ansi(&collapsed), theme.brand);
    assert_ansi_contains_color(&expanded_ansi, theme.status_ok);
    assert_ansi_contains_color(&expanded_ansi, theme.status_error);
    assert!(expanded_text.contains("Gibbs"), "{expanded_text}");
    assert!(expanded_text.contains("Ada"), "{expanded_text}");
    assert!(expanded_text.contains("5 tools"), "{expanded_text}");
    assert!(expanded_text.contains("4.2k tok"), "{expanded_text}");
    assert!(expanded_text.contains("1m 15s"), "{expanded_text}");
    assert!(
        expanded_text.contains("Collected candidate files"),
        "expanded child transcript should include the latest body row: {expanded_text}"
    );
    assert!(
        expanded_text.contains("Found two style gaps"),
        "{expanded_text}"
    );
    assert!(
        expanded_text.contains("Focused test failed"),
        "{expanded_text}"
    );
    assert!(
        expanded.len() > collapsed.len(),
        "expanded should add child details"
    );
}

#[test]
fn swarm_card_renders_progress_percent() {
    use neo_agent_core::multi_agent::{AgentSnapshot, SwarmChildSnapshot, SwarmSnapshot};

    let child = AgentSnapshot {
        state: AgentLifecycleState::Completed,
        ..running_delegate()
    };
    let child2 = AgentSnapshot {
        state: AgentLifecycleState::Running,
        ..running_delegate()
    };
    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "done item".to_owned(),
            agent: child,
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "running item".to_owned(),
            agent: child2,
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-1".to_owned(),
        description: "Progress test".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 2,
        aggregate,
        children,
    };
    let mut card = SwarmCardComponent::new(snapshot);

    let rows = plain(card.render(120));
    let text = rows.join("\n");

    assert!(text.contains('%'), "{text}");
    assert!(text.contains("Working"), "{text}");
}

#[test]
fn swarm_card_renders_full_progress_when_all_children_are_done() {
    let snapshot = swarm_with_child_states(vec![
        AgentLifecycleState::Completed,
        AgentLifecycleState::Completed,
        AgentLifecycleState::Completed,
        AgentLifecycleState::Completed,
    ]);
    let text =
        plain(SwarmCardComponent::new(snapshot).render_with_theme(160, &TuiTheme::default()))
            .join("\n");

    assert!(text.contains("DelegateSwarm · done"), "{text}");
    assert!(text.contains("100%"), "{text}");
    assert!(text.contains("Done... 100%"), "{text}");
    assert!(!text.contains("Working"), "{text}");
}

#[test]
fn swarm_card_renders_child_cache_usage_when_reported() {
    use neo_agent_core::multi_agent::{AgentSnapshot, SwarmChildSnapshot, SwarmSnapshot};

    let child = AgentSnapshot {
        state: AgentLifecycleState::Completed,
        token_count: 40_800,
        cache_read_token_count: 37_200,
        cache_write_token_count: 1_100,
        ..running_delegate()
    };
    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "cached child".to_owned(),
        agent: child,
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-cache".to_owned(),
        description: "Cache test".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 1,
        aggregate,
        children,
    };
    let mut card = SwarmCardComponent::new(snapshot);

    let rows = plain(card.render(140));
    let text = rows.join("\n");

    assert!(text.contains("40.8k tok"), "{text}");
    assert!(text.contains("cache 37.2k read / 1.1k write"), "{text}");
}

#[test]
fn swarm_card_renders_suspended_rate_limit() {
    use neo_agent_core::multi_agent::{AgentSnapshot, SwarmChildSnapshot, SwarmSnapshot};

    let child = AgentSnapshot {
        state: AgentLifecycleState::Running,
        latest_text: Some("suspended".to_owned()),
        ..running_delegate()
    };
    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "rate limited".to_owned(),
        agent: child,
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-susp".to_owned(),
        description: "Suspended test".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 1,
        aggregate,
        children,
    };
    let mut card = SwarmCardComponent::new(snapshot);

    let rows = plain(card.render(120));
    let text = rows.join("\n");

    assert!(text.contains("Suspended"), "{text}");
}

#[test]
fn swarm_card_freezes_stale_running_child_progress_and_marks_waiting() {
    let mut child = running_delegate();
    child.tool_count = 0;
    child.token_count = 0;
    child.cache_read_token_count = 0;
    child.cache_write_token_count = 0;
    child.created_at_ms = 1;
    child.updated_at_ms = 1;
    child.started_at_ms = Some(1);
    child.elapsed = Duration::from_secs(0);
    child.latest_text = None;
    child.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "icm-recall".to_owned(),
            name: "Bash".to_owned(),
            summary: Some("icm recall-context \"concurrency thread safety\" --limit 5".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
        },
    }];
    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "concurrency review".to_owned(),
        agent: child,
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-stale".to_owned(),
        description: "Stale child test".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 1,
        aggregate,
        children,
    };
    let mut card = SwarmCardComponent::new(snapshot);
    let initial = card.weighted_progress();

    card.on_render_tick(10 * 60 * 1_000);
    let stale = card.weighted_progress();
    let text = plain(card.render_with_theme(160, &TuiTheme::default())).join("\n");

    assert!(
        stale <= initial + 0.02,
        "initial={initial} stale={stale}\n{text}"
    );
    assert!(text.contains("waiting"), "{text}");
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
fn delegate_card_header_uses_task_title_not_full_prompt() {
    let mut snapshot = running_delegate();
    snapshot.task = "Read crates/neo-agent-core/src/lib.rs, count the public modules, then explain every module in detail with exact line references".to_owned();
    snapshot.task_title = "Count public modules".to_owned();

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(80, &TuiTheme::default()))
            .join("\n");

    assert!(
        text.contains("Gibbs  [Coder] · Delegate · Count public modules"),
        "{text}"
    );
    assert!(!text.contains("explain every module in detail"), "{text}");
    assert!(text.contains("tools"), "{text}");
}

#[test]
fn delegate_card_keeps_only_recent_activity_rows_when_collapsed() {
    let mut snapshot = running_delegate();
    snapshot.activity = (0..8)
        .map(|index| AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: format!("activity row {index}"),
                thinking: index % 2 == 0,
            },
        })
        .collect();

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()));

    assert!(
        !text.iter().any(|l| l.contains("activity row 0")),
        "{text:?}"
    );
    assert!(
        !text.iter().any(|l| l.contains("activity row 1")),
        "{text:?}"
    );
    assert!(
        text.iter().any(|l| l.contains("activity row 7")),
        "{text:?}"
    );
    assert!(text.len() <= 7, "{text:?}");
}

#[test]
fn completed_delegate_card_does_not_duplicate_identical_latest_text_and_summary() {
    let mut snapshot = completed_delegate();
    snapshot.latest_text = Some("34 lines".to_owned());
    snapshot.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: "34 lines".to_owned(),
            thinking: false,
        },
    });
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "34 lines".to_owned(),
        is_error: false,
    });

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()));

    let count: usize = text.iter().map(|l| l.matches("34 lines").count()).sum();
    assert_eq!(count, 1, "{text:?}");
}

#[test]
fn completed_delegate_card_suppresses_body_when_markdown_formatting_differs_only() {
    let mut snapshot = completed_delegate();
    snapshot.latest_text = Some("## Result**File changed:** `path/to/file.rs`".to_owned());
    snapshot.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: "## Result**File changed:** `path/to/file.rs`".to_owned(),
            thinking: false,
        },
    });
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "## Result **File changed:** `path/to/file.rs`".to_owned(),
        is_error: false,
    });

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()));

    let count: usize = text.iter().map(|l| l.matches("File changed").count()).sum();
    assert_eq!(count, 1, "{text:?}");
}

#[test]
fn delegate_card_header_uses_role_display_label() {
    let mut snapshot = running_delegate();
    snapshot.display_name = AgentDisplayName::new("Hypatia");
    snapshot.path = AgentPath::root_child(&snapshot.display_name);
    snapshot.role = AgentRole::Explorer;
    snapshot.task_title = "Map auth module".to_owned();

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()))
            .join("\n");

    assert!(text.contains("Hypatia  [Explorer]"), "{text}");
    assert!(text.contains("running"), "{text}");
}

#[test]
fn swarm_card_progress_starts_near_zero_when_all_children_queued() {
    let snapshot = swarm_with_child_states(vec![
        AgentLifecycleState::Queued,
        AgentLifecycleState::Queued,
        AgentLifecycleState::Queued,
    ]);

    let text =
        plain(SwarmCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()));

    let joined = text.join("\n");
    assert!(joined.contains("Orchestrating"), "{joined}");
    assert!(joined.contains("3 wait"), "{joined}");
    assert!(joined.contains("queued"), "{joined}");
    assert!(
        joined.contains("0%") || joined.contains("1%") || joined.contains("2%"),
        "{joined}"
    );
    assert!(!joined.contains("100%"), "{joined}");
}

#[test]
fn swarm_card_counts_queued_children_in_aggregate_progress() {
    let without_queued = SwarmCardComponent::new(swarm_with_child_states(vec![
        AgentLifecycleState::Completed,
        AgentLifecycleState::Running,
    ]));
    let with_queued = SwarmCardComponent::new(swarm_with_child_states(vec![
        AgentLifecycleState::Completed,
        AgentLifecycleState::Running,
        AgentLifecycleState::Queued,
    ]));

    assert!(
        with_queued.weighted_progress() < without_queued.weighted_progress(),
        "queued children must count as zero-progress tasks: with_queued={} without_queued={}",
        with_queued.weighted_progress(),
        without_queued.weighted_progress(),
    );
}

#[test]
fn swarm_card_child_row_prefers_latest_activity_over_full_prompt() {
    let mut snapshot = swarm_with_child_states(vec![AgentLifecycleState::Running]);
    snapshot.children[0].agent.task = "Run a very long investigation prompt that should not remain visible after activity arrives".to_owned();
    snapshot.children[0]
        .agent
        .activity
        .push(AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "call_1".to_owned(),
                name: "Read".to_owned(),
                summary: Some("crates/neo-agent-core/src/lib.rs".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: None,
            },
        });

    let text =
        plain(SwarmCardComponent::new(snapshot).render_with_theme(140, &TuiTheme::default()));

    let joined = text.join("\n");
    assert!(joined.contains("Used Read"), "{joined}");
    assert!(
        !joined.contains("very long investigation prompt"),
        "{joined}"
    );
}

#[test]
fn swarm_card_uses_theme_colors_for_status_and_progress() {
    let theme = TuiTheme::default();
    let snapshot = swarm_with_child_states(vec![AgentLifecycleState::Running]);
    let rows = SwarmCardComponent::new(snapshot).render_with_theme(140, &theme);
    let rendered = ansi(&rows);

    assert_ansi_contains_color(&rendered, theme.brand);
    assert_ansi_contains_color(&rendered, theme.status_warn);
}

#[test]
fn delegate_card_renders_ongoing_tool_from_explicit_phase_with_output_preview() {
    let mut snapshot = running_delegate();
    snapshot.tool_count = 0;
    snapshot.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "call_bash".to_owned(),
            name: "Bash".to_owned(),
            summary: Some("cargo nextest run -p neo-tui --test multi_agent_transcript".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: Some(AgentToolOutputPreview {
                text: "line 1\nline 2\nline 3\nline 4".to_owned(),
                is_error: false,
                truncated: false,
                tail: true,
            }),
        },
    }];

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()))
            .join("\n");

    assert!(text.contains("• Using Bash"), "{text}");
    assert!(text.contains("line 3"), "{text}");
    assert!(text.contains("line 4"), "{text}");
    assert!(!text.contains("line 1"), "{text}");
}

#[test]
fn delegate_card_fixed_thinking_window_renders_before_single_final_row() {
    let mut snapshot = completed_delegate();
    snapshot.activity = vec![
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "thinking one\nthinking two\nthinking three".to_owned(),
                thinking: true,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "final answer".to_owned(),
                thinking: false,
            },
        },
    ];
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "final answer".to_owned(),
        is_error: false,
    });

    let rows =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(90, &TuiTheme::default()));
    let text = rows.join("\n");

    assert_eq!(text.matches('◌').count(), 1, "{text}");
    assert_eq!(text.matches('└').count(), 1, "{text}");
    assert!(
        rows.iter().position(|line| line.contains('◌')).unwrap()
            < rows.iter().position(|line| line.contains('└')).unwrap()
    );
    assert!(rows.last().unwrap().contains("final answer"), "{text}");
}

#[test]
fn explicit_animation_tick_marks_transcript_dirty_for_live_delegate_elapsed() {
    let mut pane = TranscriptPane::new(120, 30);
    let mut snapshot = running_delegate();
    snapshot.elapsed = Duration::from_secs(0);
    snapshot.started_at_ms = Some(1);
    snapshot.terminal_at_ms = None;
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 7,
        agent: snapshot,
    });

    let _ = pane.render_frame(120, 30);
    assert!(!pane.is_dirty_for_test());

    pane.advance_animation_at_ms(61_000);
    assert!(pane.is_dirty_for_test());
    let frame = pane.render_frame(120, 30).unwrap_or_default().join("\n");
    assert!(frame.contains("1m 0s") || frame.contains("1m"), "{frame}");
}

#[test]
fn detached_foreground_delegate_renders_backgrounded_without_ctrl_b_hint() {
    let mut snapshot = running_delegate();
    snapshot.mode = AgentRunMode::Background;
    snapshot.detached_from_foreground = true;
    snapshot.state = AgentLifecycleState::Running;

    let rows =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()));
    let header = rows.first().expect("delegate header");
    let text = rows.join("\n");

    assert!(header.contains("· backgrounded ·"), "{text}");
    assert!(!text.contains("Press Ctrl+B"), "{text}");
}

#[test]
fn lost_background_delegate_renders_failed_reason_not_completed() {
    let mut snapshot = completed_delegate();
    snapshot.state = AgentLifecycleState::Failed;
    snapshot.mode = AgentRunMode::Background;
    snapshot.terminal_reason = Some(AgentTerminalReason::Lost);
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: "Background agent lost (session restarted before completion)".to_owned(),
        is_error: true,
    });

    let rows =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()));
    let header = rows.first().expect("delegate header");
    let text = rows.join("\n");

    assert!(header.contains("· lost ·"), "{text}");
    assert!(text.contains("Background agent lost"), "{text}");
    assert!(!header.contains("· done ·"), "{text}");
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
        },
    }];
    second.tool_count = 1;

    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 9,
        agent: first,
    });
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 9,
        agent: second,
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
    });
    pane.apply_agent_event(AgentEvent::DelegateUpdated {
        turn: 10,
        agent: updated,
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
    assert!(text.contains("Used Bash"), "{text}");
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
fn delegate_card_does_not_regress_cancelled_to_done() {
    let name = AgentDisplayName::new("Echo");
    let id = AgentId::from_suffix_for_test("regress-cancel");

    // First snapshot: cancelled at timestamp 2000.
    let cancelled = AgentSnapshot {
        id: id.clone(),
        display_name: name.clone(),
        path: AgentPath::root_child(&name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
        state: AgentLifecycleState::Cancelled,
        task: "regression test".to_owned(),
        task_title: "regression test".to_owned(),
        created_at_ms: 1_000,
        updated_at_ms: 2_000,
        started_at_ms: Some(1_000),
        terminal_at_ms: Some(2_000),
        detached_from_foreground: false,
        terminal_reason: terminal_reason_for_state(AgentLifecycleState::Cancelled),
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 0,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::from_secs(1),
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: Some(AgentTerminalOutcome {
            summary: "Cancelled by user.".to_owned(),
            is_error: true,
        }),
    };

    // Stale completed snapshot arriving later with a newer timestamp.
    let stale_completed = AgentSnapshot {
        state: AgentLifecycleState::Completed,
        updated_at_ms: 3_000,
        terminal_at_ms: Some(3_000),
        terminal_reason: terminal_reason_for_state(AgentLifecycleState::Completed),
        outcome: Some(AgentTerminalOutcome {
            summary: "All done.".to_owned(),
            is_error: false,
        }),
        ..cancelled.clone()
    };

    let mut pane = TranscriptPane::new(120, 20);
    // Apply the cancelled snapshot first.
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 1,
        agent: cancelled,
    });
    // Then apply the stale completed snapshot.
    pane.apply_agent_event(AgentEvent::DelegateFinished {
        turn: 1,
        agent: stale_completed,
    });

    let _ = pane.render_frame(120, 20);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        text.contains("cancelled"),
        "expected 'cancelled' in rendered output: {text}"
    );
    assert!(
        !text.contains(" · done · "),
        "stale 'done' must not regress cancelled card: {text}"
    );
}

#[test]
fn swarm_card_does_not_regress_cancelled_child_to_done() {
    let mut cancelled = swarm_with_child_states(vec![AgentLifecycleState::Cancelled]);
    cancelled.swarm_id = "swarm-regress-cancel".to_owned();
    cancelled.state = AgentLifecycleState::Cancelled;
    cancelled.children[0].agent.updated_at_ms = 2_000;
    cancelled.children[0].agent.terminal_at_ms = Some(2_000);
    cancelled.children[0].agent.outcome = Some(AgentTerminalOutcome {
        summary: "Cancelled by user.".to_owned(),
        is_error: true,
    });
    cancelled.aggregate =
        SwarmAggregate::from_states(cancelled.children.iter().map(|child| child.agent.state));

    let mut stale_completed = cancelled.clone();
    stale_completed.state = AgentLifecycleState::Completed;
    stale_completed.children[0].agent.state = AgentLifecycleState::Completed;
    stale_completed.children[0].agent.updated_at_ms = 3_000;
    stale_completed.children[0].agent.terminal_at_ms = Some(3_000);
    stale_completed.children[0].agent.terminal_reason =
        terminal_reason_for_state(AgentLifecycleState::Completed);
    stale_completed.children[0].agent.outcome = Some(AgentTerminalOutcome {
        summary: "All done.".to_owned(),
        is_error: false,
    });
    stale_completed.aggregate = SwarmAggregate::from_states(
        stale_completed
            .children
            .iter()
            .map(|child| child.agent.state),
    );

    let mut pane = TranscriptPane::new(120, 20);
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 1,
        swarm: cancelled,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmFinished {
        turn: 1,
        swarm: stale_completed,
    });

    let _ = pane.render_frame(120, 20);
    let text = pane
        .frame_ansi_lines()
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        text.contains("cancelled"),
        "expected cancelled child in rendered output: {text}"
    );
    assert!(
        !text.contains("1 done"),
        "stale completed swarm must not replace cancelled child: {text}"
    );
}
