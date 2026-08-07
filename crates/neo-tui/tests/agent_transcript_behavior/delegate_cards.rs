use neo_agent_core::AgentEvent;
use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome, AgentTerminalReason,
    AgentToolActivityPhase, AgentToolOutputPreview, DelegateContext, SwarmAggregate,
    SwarmChildSnapshot, SwarmSnapshot,
};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Color, Component, Expandable, Line, strip_ansi};
use neo_tui::transcript::{DelegateCardComponent, SwarmCardComponent, TranscriptPane};
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
        workflow_origin: None,
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
            files: Vec::new(),
            output_ref: None,
        },
    });
    pane.apply_agent_event(AgentEvent::DelegateProgressUpdated {
        turn: 11,
        progress: started.progress_snapshot(),
        workflow_origin: None,
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
fn delegate_and_swarm_render_same_bounded_shell_summary() {
    let now_ms = 20_000;
    let command = format!(
        "cargo test --package neo-tui --test multi_agent_transcript {} --exact --nocapture",
        "delegate_and_swarm_render_same_bounded_shell_summary_".repeat(3)
    );
    let mut snapshot = running_delegate();
    snapshot.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "call-1".to_owned(),
            name: "Bash".to_owned(),
            summary: Some(command),
            phase: AgentToolActivityPhase::Queued {
                position: Some(2),
                queued_at_ms: 2_000,
            },
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    }];

    for width in [160, 240] {
        let mut delegate_card = DelegateCardComponent::new(snapshot.clone());
        delegate_card.on_render_tick(now_ms);
        let delegate = delegate_card.render_with_theme(width, &TuiTheme::default());
        let mut swarm = swarm_with_child_states(vec![AgentLifecycleState::Running]);
        swarm.children[0].agent = snapshot.clone();
        let mut collapsed_card = SwarmCardComponent::new(swarm.clone());
        collapsed_card.on_render_tick(now_ms);
        let collapsed = collapsed_card.render_with_theme(width, &TuiTheme::default());
        let mut expanded_card = SwarmCardComponent::new(swarm);
        expanded_card.set_expanded(true);
        expanded_card.on_render_tick(now_ms);
        let expanded = expanded_card.render_with_theme(width, &TuiTheme::default());

        for (label, lines, expected_rows) in [
            ("delegate queued", delegate, 1),
            ("collapsed swarm queued", collapsed, 1),
            ("expanded swarm queued", expanded, 2),
        ] {
            assert!(
                lines.iter().all(|line| line.visible_width() <= width),
                "{label} exceeded width {width}"
            );
            let rows = plain(lines);
            let shell_rows = rows
                .iter()
                .filter(|row| row.contains("Queued Bash"))
                .collect::<Vec<_>>();
            assert_eq!(shell_rows.len(), expected_rows, "{label}: {rows:?}");
            for row in shell_rows {
                assert!(row.contains("cargo test --package"), "{label}: {row}");
                assert!(row.contains("--exact --nocapture"), "{label}: {row}");
                assert!(row.contains(" · #2 · waiting 18s"), "{label}: {row}");
            }
        }

        let AgentActivityKind::Tool { phase, output, .. } = &mut snapshot.activity[0].kind else {
            panic!("expected tool activity");
        };
        *phase = AgentToolActivityPhase::Done;
        *output = Some(AgentToolOutputPreview {
            text: "done".to_owned(),
            is_error: false,
            truncated: false,
            tail: false,
        });
        let delegate = DelegateCardComponent::new(snapshot.clone())
            .render_with_theme(width, &TuiTheme::default());
        let mut done_swarm = swarm_with_child_states(vec![AgentLifecycleState::Running]);
        done_swarm.children[0].agent = snapshot.clone();
        let collapsed = SwarmCardComponent::new(done_swarm.clone())
            .render_with_theme(width, &TuiTheme::default());
        let mut expanded_card = SwarmCardComponent::new(done_swarm);
        expanded_card.set_expanded(true);
        let expanded = expanded_card.render_with_theme(width, &TuiTheme::default());

        for (label, lines, expected_rows, expected_previews) in [
            ("delegate done", delegate, 1, 1),
            ("collapsed swarm done", collapsed, 1, 0),
            ("expanded swarm done", expanded, 2, 1),
        ] {
            assert!(
                lines.iter().all(|line| line.visible_width() <= width),
                "{label} exceeded width {width}"
            );
            let rows = plain(lines);
            let shell_rows = rows
                .iter()
                .filter(|row| row.contains("Used Bash"))
                .collect::<Vec<_>>();
            assert_eq!(shell_rows.len(), expected_rows, "{label}: {rows:?}");
            for row in shell_rows {
                assert!(row.contains("cargo test --package"), "{label}: {row}");
                assert!(row.contains("--exact --nocapture"), "{label}: {row}");
            }
            assert_eq!(
                rows.iter().filter(|row| row.trim() == "done").count(),
                expected_previews,
                "{label}: {rows:?}"
            );
        }

        let AgentActivityKind::Tool { phase, output, .. } = &mut snapshot.activity[0].kind else {
            panic!("expected tool activity");
        };
        *phase = AgentToolActivityPhase::Queued {
            position: Some(2),
            queued_at_ms: 2_000,
        };
        *output = None;
    }

    let mut unicode_snapshot = running_delegate();
    unicode_snapshot.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "call-wide".to_owned(),
            name: "Bash".to_owned(),
            summary: Some(format!(
                "cargo 宽字符开始e\u{301}\u{200b} {} 宽字符结束 --exact --nocapture",
                "界".repeat(60)
            )),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    }];
    let width = 160;
    let delegate = DelegateCardComponent::new(unicode_snapshot.clone())
        .render_with_theme(width, &TuiTheme::default());
    let mut unicode_swarm = swarm_with_child_states(vec![AgentLifecycleState::Running]);
    unicode_swarm.children[0].agent = unicode_snapshot.clone();
    let collapsed = SwarmCardComponent::new(unicode_swarm.clone())
        .render_with_theme(width, &TuiTheme::default());
    let mut expanded_card = SwarmCardComponent::new(unicode_swarm.clone());
    expanded_card.set_expanded(true);
    let expanded = expanded_card.render_with_theme(width, &TuiTheme::default());
    for (label, lines, expected_rows) in [
        ("delegate unicode", delegate, 1),
        ("collapsed swarm unicode", collapsed, 1),
        ("expanded swarm unicode", expanded, 2),
    ] {
        assert!(
            lines.iter().all(|line| line.visible_width() <= width),
            "{label} exceeded width {width}"
        );
        let rows = plain(lines);
        let shell_rows = rows
            .iter()
            .filter(|row| row.contains("Using Bash"))
            .collect::<Vec<_>>();
        assert_eq!(shell_rows.len(), expected_rows, "{label}: {rows:?}");
        for row in shell_rows {
            assert!(row.contains("cargo 宽字符开始"), "{label}: {row}");
            assert!(
                row.contains("宽字符结束 --exact --nocapture"),
                "{label}: {row}"
            );
        }
    }

    let tiny_delegate =
        DelegateCardComponent::new(unicode_snapshot).render_with_theme(8, &TuiTheme::default());
    let tiny_swarm =
        SwarmCardComponent::new(unicode_swarm).render_with_theme(8, &TuiTheme::default());
    assert!(!tiny_delegate.is_empty());
    assert!(!tiny_swarm.is_empty());
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
        workflow_origin: None,
    });
    // Then apply the stale completed snapshot.
    pane.apply_agent_event(AgentEvent::DelegateFinished {
        turn: 1,
        agent: stale_completed,
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
        "expected 'cancelled' in rendered output: {text}"
    );
    assert!(
        !text.contains(" · done · "),
        "stale 'done' must not regress cancelled card: {text}"
    );
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
fn delegate_card_layout_is_unchanged_by_batch_write_summary() {
    // A Write tool activity with structured batch summary renders in the same
    // card layout as any other tool activity — same row structure, ordering,
    // and expansion rules. Only the summary text content differs.
    let mut snapshot = running_delegate();
    snapshot.tool_count = 2;
    snapshot.activity = vec![
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "read-1".to_owned(),
                name: "Read".to_owned(),
                summary: Some("src/main.rs".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: None,
                files: Vec::new(),
                output_ref: None,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "write-1".to_owned(),
                name: "Write".to_owned(),
                summary: Some("wrote 3 files · 2 created · 1 overwritten · +120 -30".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: None,
                files: Vec::new(),
                output_ref: None,
            },
        },
    ];

    let rows =
        plain(DelegateCardComponent::new(snapshot).render_with_theme(120, &TuiTheme::default()));
    let text = rows.join("\n");

    // Header row is present with standard format.
    assert!(text.contains("Gibbs  [Coder] · Delegate"), "{text}");
    // Both tool activities render as "Used" rows (Done phase).
    assert!(text.contains("Used Read"), "{text}");
    assert!(text.contains("Used Write"), "{text}");
    // Write summary text is visible in the card.
    assert!(text.contains("wrote 3 files"), "{text}");
    // Ordering: Read before Write (insertion order preserved).
    let read_pos = rows
        .iter()
        .position(|l| l.contains("Used Read"))
        .expect("read row");
    let write_pos = rows
        .iter()
        .position(|l| l.contains("Used Write"))
        .expect("write row");
    assert!(
        read_pos < write_pos,
        "Read must appear before Write: {text}"
    );
    // Card is not expanded (no output preview for Write).
    assert!(!text.contains("└"), "{text}");

    // Verify ongoing Write with progress summary also renders correctly.
    let mut ongoing_snapshot = running_delegate();
    ongoing_snapshot.tool_count = 1;
    ongoing_snapshot.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "write-2".to_owned(),
            name: "Write".to_owned(),
            summary: Some("committing 2/5 · src/lib.rs".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    }];

    let ongoing_text = plain(
        DelegateCardComponent::new(ongoing_snapshot).render_with_theme(120, &TuiTheme::default()),
    )
    .join("\n");

    // Ongoing tool uses "Using" marker.
    assert!(ongoing_text.contains("Using Write"), "{ongoing_text}");
    assert!(ongoing_text.contains("committing 2/5"), "{ongoing_text}");
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
                files: Vec::new(),
                output_ref: None,
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
                files: Vec::new(),
                output_ref: None,
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
    assert!(text.contains("✗ Failed Grep"), "{text}");
    assert!(text.contains("◌ thinking"), "{text}");
    assert!(text.contains("Let me start by reading"), "{text}");
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
            files: Vec::new(),
            output_ref: None,
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
                files: Vec::new(),
                output_ref: None,
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
