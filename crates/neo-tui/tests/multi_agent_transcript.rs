use std::time::Duration;

use neo_agent_core::AgentEvent;
use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome, SwarmChildSnapshot,
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
                    failed: false,
                },
            },
            AgentActivityEntry {
                kind: AgentActivityKind::Tool {
                    id: "grep-1".to_owned(),
                    name: "Grep".to_owned(),
                    summary: Some("from_spans|pub struct Span|pub struct Line".to_owned()),
                    failed: true,
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

    assert!(text.contains("\u{25cf} Gibbs Agent Running"), "{text}");
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
fn delegate_card_uses_short_title_and_keeps_stats_visible_for_long_prompts() {
    let mut snapshot = running_delegate();
    snapshot.task = "Look up the line count of crates/neo-agent-core/src/lib.rs using `wc -l` and report back. Reply with exactly one line: `<count> lines` where <count> is the actual number from wc -l. Do not modify any files.".to_owned();
    snapshot.latest_text = Some("34 lines".to_owned());

    let rows =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()));
    let text = rows.join("\n");

    assert!(text.contains("Gibbs Agent Running"), "{text}");
    assert!(text.contains("1m?") == false, "{text}");
    assert!(text.contains("3 tools"), "{text}");
    assert!(text.contains("24s"), "{text}");
    assert!(text.contains("25.6k tok"), "{text}");
    assert!(
        !text.contains("Reply with exactly one line"),
        "header should not include the full prompt: {text}"
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
                failed: false,
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
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-1".to_owned(),
        description: "Audit and fix Neo tool schemas".to_owned(),
        mode: AgentRunMode::Foreground,
        max_concurrency: 1,
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "Search tools: Grep, Find".to_owned(),
            agent: child,
        }],
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
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-1".to_owned(),
        description: "Audit and fix Neo tool schemas".to_owned(),
        mode: AgentRunMode::Foreground,
        max_concurrency: 1,
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "Search tools: Grep, Find".to_owned(),
            agent: running_delegate(),
        }],
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
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-queued".to_owned(),
        description: "single-file counts".to_owned(),
        mode: AgentRunMode::Foreground,
        max_concurrency: 1,
        children: vec![
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
        ],
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
    child.latest_text = Some("34 lines".to_owned());
    child.outcome = Some(AgentTerminalOutcome {
        summary: "34 lines".to_owned(),
        is_error: false,
    });
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-1".to_owned(),
        description: "Read-only codebase investigations".to_owned(),
        mode: AgentRunMode::Foreground,
        max_concurrency: 1,
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "Look up the line count of crates/neo-agent-core/src/lib.rs using `wc -l` and report back. Reply with exactly one line: `<count> lines` where <count> is the actual number from wc -l. Do not modify any files.".to_owned(),
            agent: child,
        }],
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

    assert!(text.contains("Gibbs Agent Running"), "{text}");
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
    let started = SwarmSnapshot {
        swarm_id: "swarm-out-of-order".to_owned(),
        description: "merge test".to_owned(),
        mode: AgentRunMode::Foreground,
        max_concurrency: 2,
        children: vec![
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
        ],
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
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-style".to_owned(),
        description: "Style-rich swarm".to_owned(),
        mode: AgentRunMode::Foreground,
        max_concurrency: 2,
        children: vec![
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
        ],
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
        "{expanded_text}"
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
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-1".to_owned(),
        description: "Progress test".to_owned(),
        mode: AgentRunMode::Foreground,
        max_concurrency: 2,
        children: vec![
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
        ],
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
    let snapshot = SwarmSnapshot {
        swarm_id: "swarm-susp".to_owned(),
        description: "Suspended test".to_owned(),
        mode: AgentRunMode::Foreground,
        max_concurrency: 1,
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "rate limited".to_owned(),
            agent: child,
        }],
    };
    let mut card = SwarmCardComponent::new(snapshot);

    let rows = plain(card.render(120));
    let text = rows.join("\n");

    assert!(text.contains("Suspended"), "{text}");
}
