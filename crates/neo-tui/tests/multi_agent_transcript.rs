use std::time::Duration;

use neo_agent_core::AgentEvent;
use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome, AgentTerminalReason,
    AgentToolActivityPhase, AgentToolOutputPreview, SwarmAggregate, SwarmChildSnapshot,
    SwarmSnapshot,
};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Color, Component, Expandable, Line, strip_ansi};
use neo_tui::transcript::{DelegateCardComponent, SwarmCardComponent, TranscriptPane};

fn running_delegate() -> AgentSnapshot {
    let name = AgentDisplayName::new("Gibbs");
    AgentSnapshot {
        id: AgentId::from_suffix_for_test("test"),
        display_name: name.clone(),
        path: AgentPath::root_child(&name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: AgentLifecycleState::Running,
        task: "Implement Task 1: PlanBox border fix".to_owned(),
        task_title: "Implement Task 1: PlanBox border fix".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 1,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: false,
        terminal_reason: None,
        tool_count: 3,
        token_count: 25_600,
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

#[test]
fn delegate_card_renders_kimi_style_running_summary() {
    let mut card = DelegateCardComponent::new(running_delegate());

    let rows = plain(card.render(120));
    let text = rows.join("\n");

    assert!(
        text.contains("\u{25cf} Gibbs Coder Agent Running"),
        "{text}"
    );
    assert!(text.contains("3 tools"), "{text}");
    assert!(text.contains("24s"), "{text}");
    assert!(text.contains("25.6k tok"), "{text}");
    assert!(text.contains("Press Ctrl+B to run in background"), "{text}");
    assert!(text.contains("• Used Read"), "{text}");
    assert!(text.contains("✗ Used Grep"), "{text}");
    assert!(text.contains("◌ Let me start by reading"), "{text}");
    assert!(text.contains("Let me start by reading"), "{text}");
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

    assert!(text.contains("Gibbs Coder Agent Running"), "{text}");
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

    assert!(text.contains("Gibbs Explorer Agent Completed"), "{text}");
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

    let rows = plain(card.render(120));
    let text = rows.join("\n");

    assert!(text.contains("Agent Swarm"), "{text}");
    assert!(text.contains("001"), "{text}");
    assert!(text.contains("0%"), "{text}");
    assert!(text.contains("Orchestrating"), "{text}");
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
    let queued_a = AgentSnapshot {
        id: AgentId::from_suffix_for_test("queued-a"),
        display_name: AgentDisplayName::new("Hypatia"),
        state: AgentLifecycleState::Queued,
        tool_count: 0,
        token_count: 0,
        elapsed: Duration::ZERO,
        latest_text: None,
        activity: Vec::new(),
        ..running_delegate()
    };
    let queued_b = AgentSnapshot {
        id: AgentId::from_suffix_for_test("queued-b"),
        display_name: AgentDisplayName::new("Athena"),
        state: AgentLifecycleState::Queued,
        tool_count: 0,
        token_count: 0,
        elapsed: Duration::ZERO,
        latest_text: None,
        activity: Vec::new(),
        ..running_delegate()
    };
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

    assert!(text.contains("Gibbs Coder Agent Running"), "{text}");
}

#[test]
fn transcript_pane_merges_out_of_order_swarm_updates_without_regressing_children() {
    let mut pane = TranscriptPane::new(160, 30);
    let first = AgentSnapshot {
        display_name: AgentDisplayName::new("Zeno"),
        state: AgentLifecycleState::Running,
        latest_text: Some("alpha running".to_owned()),
        activity: Vec::new(),
        ..running_delegate()
    };
    let second = AgentSnapshot {
        id: AgentId::from_suffix_for_test("second"),
        display_name: AgentDisplayName::new("Gibbs"),
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
        !expanded_text.contains("Collected candidate files"),
        "expanded child should prefer shared tool/thinking/final rows over stale latest_text: {expanded_text}"
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
                        state,
                        task_title: format!("Child {index}"),
                        task: format!("Child prompt {index}"),
                        created_at_ms: 1,
                        updated_at_ms: 1,
                        started_at_ms: (state == AgentLifecycleState::Running).then_some(1),
                        terminal_at_ms: state.is_terminal().then_some(2),
                        detached_from_foreground: false,
                        terminal_reason: terminal_reason_for_state(state),
                        tool_count: 0,
                        token_count: 0,
                        elapsed: Duration::from_secs(0),
                        latest_text: None,
                        activity: Vec::new(),
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

    assert!(text.contains("(Count public modules)"), "{text}");
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
fn delegate_card_header_uses_role_display_label() {
    let mut snapshot = running_delegate();
    snapshot.display_name = AgentDisplayName::new("Hypatia");
    snapshot.role = AgentRole::Explorer;
    snapshot.task_title = "Map auth module".to_owned();

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()))
            .join("\n");

    assert!(text.contains("Hypatia Explorer Agent Running"), "{text}");
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
    assert!(
        joined.contains("Running") || joined.contains("Queued"),
        "{joined}"
    );
    assert!(
        joined.contains("0%") || joined.contains("1%") || joined.contains("2%"),
        "{joined}"
    );
    assert!(!joined.contains("100%"), "{joined}");
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
fn render_tick_marks_transcript_dirty_for_live_delegate_elapsed() {
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

    pane.render_tick_at_ms_for_test(61_000);
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

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()))
            .join("\n");

    assert!(text.contains("Backgrounded"), "{text}");
    assert!(!text.contains("Press Ctrl+B"), "{text}");
    assert!(!text.contains("Completed"), "{text}");
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

    let text =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()))
            .join("\n");

    assert!(text.contains("Lost") || text.contains("Failed"), "{text}");
    assert!(text.contains("Background agent lost"), "{text}");
    assert!(!text.contains("Completed"), "{text}");
}

#[test]
fn same_turn_root_delegates_render_as_one_live_group() {
    let mut pane = TranscriptPane::new(140, 40);
    let mut first = running_delegate();
    first.id = AgentId::from_suffix_for_test("first");
    first.display_name = AgentDisplayName::new("Gibbs");
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

    let frame = pane.render_frame(140, 40).unwrap_or_default().join("\n");

    assert!(frame.contains("Running 2 agents"), "{frame}");
    assert!(frame.contains("Coder · PlanBox border fix"), "{frame}");
    assert!(frame.contains("Explorer · Trace markdown width"), "{frame}");
    assert!(frame.contains("Used Read"), "{frame}");
    assert_eq!(frame.matches("Agent Running").count(), 0, "{frame}");
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
                text: "final child summary".to_owned(),
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
    let text = plain(rows).join("\n");

    assert_eq!(text.matches('◌').count(), 1, "{text}");
    assert_eq!(text.matches('└').count(), 1, "{text}");
    assert!(text.contains("Used Bash"), "{text}");
    assert!(text.contains("final child summary"), "{text}");
}
