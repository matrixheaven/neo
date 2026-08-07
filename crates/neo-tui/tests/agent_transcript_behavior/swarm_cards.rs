use neo_agent_core::AgentEvent;
use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome, AgentTerminalReason,
    AgentToolActivityPhase, DelegateContext, SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Color, Component, Expandable, Line, strip_ansi};
use neo_tui::transcript::{SwarmCardComponent, TranscriptPane};
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
                files: Vec::new(),
                output_ref: None,
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
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmFinished {
        turn: 1,
        swarm: stale_completed,
        workflow_origin: None,
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
            files: Vec::new(),
            output_ref: None,
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
fn swarm_card_uses_theme_colors_for_status_and_progress() {
    let theme = TuiTheme::default();
    let snapshot = swarm_with_child_states(vec![AgentLifecycleState::Running]);
    let rows = SwarmCardComponent::new(snapshot).render_with_theme(140, &theme);
    let rendered = ansi(&rows);

    assert_ansi_contains_color(&rendered, theme.brand);
    assert_ansi_contains_color(&rendered, theme.status_warn);
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
