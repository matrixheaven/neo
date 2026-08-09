use neo_agent_core::AgentEvent;
use neo_agent_core::instructions::InstructionEpochOutcome;
use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentProgressSnapshot, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome,
    AgentTerminalReason, AgentToolActivityPhase, AgentToolFileChange, AgentToolFileOperation,
    AgentToolFileStatus, DelegateContext, SwarmAggregate, SwarmChildProgress, SwarmChildSnapshot,
    SwarmSnapshot,
};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Color, Expandable, Line, strip_ansi, visible_width};
use neo_tui::transcript::{
    DelegateCardComponent, DelegateGroupComponent, SwarmCardComponent, TranscriptEntry,
    TranscriptPane,
};
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

#[test]
fn instruction_activity_has_a_distinct_transcript_row_in_delegate_and_swarm() {
    let mut child = running_delegate();
    child.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Instruction {
            generation: 4,
            outcome: InstructionEpochOutcome::Updated,
        },
    });

    let delegate_lines = plain(
        DelegateCardComponent::new(child.clone()).render_with_theme(120, &TuiTheme::default()),
    );
    assert!(
        delegate_lines
            .iter()
            .any(|line| line.contains("Instructions reloaded")),
        "{delegate_lines:?}"
    );

    let swarm = SwarmSnapshot {
        swarm_id: "instruction-swarm".to_owned(),
        description: "instruction update".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: AgentLifecycleState::Running,
        max_concurrency: 1,
        aggregate: SwarmAggregate::from_states([AgentLifecycleState::Running]),
        children: vec![SwarmChildSnapshot {
            item_index: 0,
            item: "reload".to_owned(),
            agent: child,
        }],
    };
    let swarm_lines =
        plain(SwarmCardComponent::new(swarm).render_with_theme(160, &TuiTheme::default()));
    let child_line = swarm_lines
        .iter()
        .find(|line| line.contains("Instructions reloaded"))
        .expect("swarm child instruction status");
    assert!(!child_line.contains("Using Bash"), "{child_line}");
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

#[test]
fn delegate_family_renders_edit_write_file_rows() {
    let edit_files = vec![
        AgentToolFileChange {
            path: "src/a.rs".to_owned(),
            operation: Some(AgentToolFileOperation::Edited),
            status: AgentToolFileStatus::Committed,
            line_count: None,
            added: Some(5),
            removed: Some(1),
            message: None,
        },
        AgentToolFileChange {
            path: "src/b.rs".to_owned(),
            operation: Some(AgentToolFileOperation::Edited),
            status: AgentToolFileStatus::Committed,
            line_count: None,
            added: Some(2),
            removed: Some(2),
            message: None,
        },
    ];
    let write_files = vec![
        AgentToolFileChange {
            path: "docs/new.md".to_owned(),
            operation: Some(AgentToolFileOperation::Created),
            status: AgentToolFileStatus::Committed,
            line_count: Some(4),
            added: Some(4),
            removed: Some(0),
            message: None,
        },
        AgentToolFileChange {
            path: "docs/existing.md".to_owned(),
            operation: Some(AgentToolFileOperation::Overwritten),
            status: AgentToolFileStatus::Failed,
            line_count: None,
            added: None,
            removed: None,
            message: Some("permission denied".to_owned()),
        },
        AgentToolFileChange {
            path: "docs/skipped.md".to_owned(),
            operation: Some(AgentToolFileOperation::Created),
            status: AgentToolFileStatus::NotAttempted,
            line_count: None,
            added: None,
            removed: None,
            message: None,
        },
    ];
    let mut snapshot = running_delegate();
    snapshot.activity = vec![
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "edit-files".to_owned(),
                name: "Edit".to_owned(),
                summary: Some("edited 2 files · +7 -3".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: None,
                files: edit_files,
                output_ref: None,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "write-files".to_owned(),
                name: "Write".to_owned(),
                summary: Some("partial 1/3 · +4 -0".to_owned()),
                phase: AgentToolActivityPhase::Failed,
                output: None,
                files: write_files,
                output_ref: None,
            },
        },
    ];

    let delegate_rows = plain(
        DelegateCardComponent::new(snapshot.clone()).render_with_theme(120, &TuiTheme::default()),
    );
    let delegate = delegate_rows.join("\n");
    for expected in [
        "M src/a.rs  +5 -1",
        "M src/b.rs  +2 -2",
        "C docs/new.md  4 lines",
        "✗ M docs/existing.md · permission denied",
        "– C docs/skipped.md",
    ] {
        assert!(
            delegate.contains(expected),
            "missing {expected}: {delegate}"
        );
    }
    let positions = ["src/a.rs", "src/b.rs", "docs/new.md", "docs/existing.md"]
        .map(|path| delegate.find(path).expect("file path"));
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{delegate}"
    );

    let group = plain(
        DelegateGroupComponent::new(1, vec![snapshot.clone()])
            .render_with_theme(120, &TuiTheme::default()),
    )
    .join("\n");
    assert!(group.contains("M src/a.rs  +5 -1"), "{group}");
    assert!(
        group.contains("✗ M docs/existing.md · permission denied"),
        "{group}"
    );

    let mut swarm = swarm_with_child_states(vec![AgentLifecycleState::Running]);
    swarm.children[0].agent.activity = snapshot.activity.clone();
    let mut swarm_card = SwarmCardComponent::new(swarm);
    swarm_card.set_expanded(true);
    let swarm_text = plain(swarm_card.render_with_theme(120, &TuiTheme::default())).join("\n");
    assert!(swarm_text.contains("M src/a.rs  +5 -1"), "{swarm_text}");
    assert!(swarm_text.contains("– C docs/skipped.md"), "{swarm_text}");

    let long_path = format!("src/{}/file.rs", "nested".repeat(10));
    let mut narrow = running_delegate();
    narrow.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "edit-long".to_owned(),
            name: "Edit".to_owned(),
            summary: Some("3 files".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
            files: vec![AgentToolFileChange {
                path: long_path.clone(),
                operation: Some(AgentToolFileOperation::Edited),
                status: AgentToolFileStatus::Pending,
                line_count: None,
                added: Some(99),
                removed: Some(98),
                message: None,
            }],
            output_ref: None,
        },
    }];
    let narrow_rows =
        plain(DelegateCardComponent::new(narrow).render_with_theme(32, &TuiTheme::default()));
    let file_start = narrow_rows
        .iter()
        .position(|row| row.contains("… src/"))
        .expect("first wrapped file row");
    let file_end = narrow_rows[file_start..]
        .iter()
        .position(|row| row.starts_with("  │"))
        .map_or(narrow_rows.len(), |offset| file_start + offset);
    let file_rows = &narrow_rows[file_start..file_end];
    assert!(
        file_rows.iter().all(|row| visible_width(row) <= 32),
        "{file_rows:#?}"
    );
    let compact = file_rows
        .join("")
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert!(compact.contains(&long_path), "{file_rows:#?}");
    assert!(!compact.contains("+99"), "pending rows show paths only");
}

#[test]
fn delegate_family_tool_activity_uses_theme_and_collapsed_file_hint() {
    let theme = TuiTheme {
        brand: Color::Rgb(1, 2, 3),
        status_ok: Color::Rgb(4, 5, 6),
        status_error: Color::Rgb(7, 8, 9),
        status_warn: Color::Rgb(10, 11, 12),
        status_pending: Color::Rgb(13, 14, 15),
        text_muted: Color::Rgb(16, 17, 18),
        text_primary: Color::Rgb(19, 20, 21),
        diff_added: Color::Rgb(22, 23, 24),
        diff_removed: Color::Rgb(25, 26, 27),
        diff_hunk: Color::Rgb(28, 29, 30),
        ..TuiTheme::default()
    };

    let file = |path: &str, status: AgentToolFileStatus| AgentToolFileChange {
        path: path.to_owned(),
        operation: Some(AgentToolFileOperation::Edited),
        status,
        line_count: None,
        added: Some(1),
        removed: Some(1),
        message: None,
    };
    let tool = |id: &str,
                name: &str,
                phase: AgentToolActivityPhase,
                files: Vec<AgentToolFileChange>| AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            summary: None,
            phase,
            output: None,
            files,
            output_ref: None,
        },
    };

    let mut failed = file("detail/failed.rs", AgentToolFileStatus::Failed);
    failed.message = Some("permission denied".to_owned());
    let mut delegate = running_delegate();
    delegate.activity = vec![
        tool(
            "done",
            "Edit",
            AgentToolActivityPhase::Done,
            vec![file("detail/committed.rs", AgentToolFileStatus::Committed)],
        ),
        tool(
            "failed",
            "Write",
            AgentToolActivityPhase::Failed,
            vec![failed],
        ),
    ];
    let detailed = DelegateCardComponent::new(delegate).render_with_theme(180, &theme);

    let span = |text: &str| {
        detailed
            .iter()
            .flat_map(neo_tui::primitive::Line::spans)
            .find(|span| span.text() == text)
            .expect("styled span")
    };
    for (verb, name, color) in [
        ("Used", "Edit", theme.status_ok),
        ("Failed", "Write", theme.status_error),
    ] {
        assert_eq!(span(verb).style().fg, Some(color));
        assert_eq!(span(name).style().fg, Some(theme.brand));
        assert!(span(name).style().bold);
    }
    for (path, marker, color) in [
        ("detail/committed.rs", "M", theme.diff_hunk),
        ("detail/failed.rs", "✗ M", theme.status_error),
    ] {
        assert_eq!(span(marker).style().fg, Some(color));
        assert_eq!(span(path).style().fg, Some(theme.text_primary));
    }
    assert_eq!(span("+1").style().fg, Some(theme.diff_added));
    assert_eq!(span("-1").style().fg, Some(theme.diff_removed));
    assert_eq!(
        span("permission denied").style().fg,
        Some(theme.status_error)
    );

    let complete_files = vec![
        file("complete-first.rs", AgentToolFileStatus::Committed),
        file("complete-second.rs", AgentToolFileStatus::Committed),
    ];
    let priority_files = vec![
        file("priority-canonical.rs", AgentToolFileStatus::Committed),
        file("priority-pending.rs", AgentToolFileStatus::Pending),
        file(
            "priority-unsynced.rs",
            AgentToolFileStatus::CommittedUnsynced,
        ),
        file("priority-failed.rs", AgentToolFileStatus::Failed),
    ];
    let mut long_summary = tool(
        "long-summary",
        "Read",
        AgentToolActivityPhase::Done,
        Vec::new(),
    );
    let AgentActivityKind::Tool { summary, .. } = &mut long_summary.kind else {
        unreachable!();
    };
    *summary = Some("x".repeat(160));
    let mut swarm = swarm_with_child_states(vec![AgentLifecycleState::Running; 7]);
    let activities = vec![
        tool(
            "single-edit",
            "Edit",
            AgentToolActivityPhase::Done,
            vec![file("single-edit.rs", AgentToolFileStatus::Committed)],
        ),
        tool(
            "single-write",
            "Write",
            AgentToolActivityPhase::Ongoing,
            vec![file("single-write.md", AgentToolFileStatus::Pending)],
        ),
        tool(
            "complete",
            "Edit",
            AgentToolActivityPhase::Done,
            complete_files,
        ),
        tool(
            "priority-failed",
            "Edit",
            AgentToolActivityPhase::Done,
            priority_files.clone(),
        ),
        tool(
            "priority-unsynced",
            "Edit",
            AgentToolActivityPhase::Done,
            priority_files[..3].to_vec(),
        ),
        tool(
            "priority-pending",
            "Edit",
            AgentToolActivityPhase::Done,
            priority_files[..2].to_vec(),
        ),
        long_summary,
    ];
    for (child, activity) in swarm.children.iter_mut().zip(activities) {
        child.agent.tool_count = 1;
        child.agent.latest_text = None;
        child.agent.activity = vec![activity];
    }
    let mut swarm_card = SwarmCardComponent::new(swarm);
    let collapsed = swarm_card.render_with_theme(300, &theme);
    let child_rows = collapsed
        .iter()
        .filter(|line| (0..7).any(|index| line.text().contains(&format!("Agent{index}  ["))))
        .collect::<Vec<_>>();
    assert_eq!(
        child_rows.len(),
        7,
        "collapsed swarm must use one row per child"
    );

    let row = |name: &str| {
        child_rows
            .iter()
            .copied()
            .find(|line| line.text().contains(name))
            .expect("collapsed child row")
    };
    for name in ["Agent0", "Agent1"] {
        assert!(!row(name).text().contains("1 file"), "{}", row(name).text());
    }
    assert!(row("Agent0").text().contains("single-edit.rs"));
    assert!(row("Agent1").text().contains("single-write.md"));
    assert!(row("Agent2").text().contains("2 files"));
    assert!(row("Agent2").text().contains("complete-first.rs"));
    assert!(row("Agent2").text().contains("total +2 -2"));
    assert!(!row("Agent2").text().contains("complete-second.rs"));
    assert!(row("Agent3").text().contains("priority-failed.rs"));
    assert!(row("Agent4").text().contains("priority-unsynced.rs"));
    assert!(row("Agent5").text().contains("priority-pending.rs"));
    assert!(!row("Agent5").text().contains("total"));
    assert!(!row("Agent5").text().contains('+'));
    let ordinary = row("Agent6").text();
    let ordinary_status = &ordinary[ordinary.find("Used Read").expect("ordinary tool status")..];
    assert_eq!(ordinary_status.chars().count(), 96, "{ordinary_status}");
    assert!(ordinary_status.ends_with("..."), "{ordinary_status}");
    assert!(!ordinary_status.contains('…'), "{ordinary_status}");

    let style = |line: &Line, text: &str| {
        line.spans()
            .iter()
            .find(|span| span.text() == text)
            .expect("collapsed styled span")
            .style()
    };
    let single = row("Agent0");
    assert_eq!(style(single, "Used").fg, Some(theme.status_ok));
    assert_eq!(style(single, "Edit").fg, Some(theme.brand));
    assert!(style(single, "Edit").bold);
    assert_eq!(style(single, "M").fg, Some(theme.diff_hunk));
    assert_eq!(style(single, "single-edit.rs").fg, Some(theme.text_primary));
    assert_eq!(style(single, " +1").fg, Some(theme.diff_added));
    assert_eq!(style(single, " -1").fg, Some(theme.diff_removed));
    assert_eq!(style(row("Agent1"), "Using").fg, Some(theme.status_ok));
    assert_eq!(style(row("Agent3"), "M").fg, Some(theme.status_error));
    assert_eq!(style(row("Agent4"), "! M").fg, Some(theme.status_warn));
    assert_eq!(style(row("Agent5"), "…").fg, Some(theme.status_pending));

    swarm_card.set_expanded(true);
    let expanded = swarm_card.render_with_theme(300, &theme);
    let agent_two = expanded
        .iter()
        .rposition(|line| line.text().contains("Agent2  ["))
        .expect("expanded child header");
    let heading = agent_two
        + expanded[agent_two + 1..]
            .iter()
            .position(|line| line.text().contains("• Used Edit"))
            .expect("expanded tool heading")
        + 1;
    assert!(
        !expanded[heading].text().contains("complete-first.rs"),
        "detailed heading must not repeat an inline path: {}",
        expanded[heading].text()
    );
    let positions = ["complete-first.rs", "complete-second.rs"].map(|path| {
        expanded[heading + 1..]
            .iter()
            .position(|line| line.text().contains(path))
            .expect("expanded file row")
    });
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        style(&expanded[heading + 1 + positions[0]], "complete-first.rs").fg,
        Some(theme.text_primary)
    );
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
fn explicit_animation_tick_marks_transcript_dirty_for_live_delegate_elapsed() {
    let mut pane = TranscriptPane::new(120, 30);
    let mut snapshot = running_delegate();
    snapshot.elapsed = Duration::from_secs(0);
    snapshot.started_at_ms = Some(1);
    snapshot.terminal_at_ms = None;
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 7,
        agent: snapshot,
        workflow_origin: None,
    });

    let _ = pane.render_frame(120, 30);
    assert!(!pane.is_dirty_for_test());

    pane.advance_animation_at_ms(61_000);
    assert!(pane.is_dirty_for_test());
    let frame = pane.render_frame(120, 30).unwrap_or_default().join("\n");
    assert!(frame.contains("1m 0s") || frame.contains("1m"), "{frame}");
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
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 1,
        swarm: swarm.clone(),
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ThinkingStarted {
        turn: 2,
        id: "reasoning".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
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
        workflow_origin: None,
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
        workflow_origin: None,
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
    pane.apply_agent_event(AgentEvent::DelegateSwarmUpdated {
        turn: 1,
        swarm,
        workflow_origin: None,
    });
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
        workflow_origin: None,
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
        .filter_map(TranscriptEntry::thinking_content)
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
fn delegate_card_terminal_summary_renders_cache_usage_and_hit_rate() {
    let mut snapshot = completed_delegate();
    snapshot.token_count = 6_468_100;
    snapshot.input_token_count = 6_390_000;
    snapshot.cache_read_token_count = 6_300_000;

    let text =
        plain(DelegateCardComponent::new(snapshot).terminal_summary(160, &TuiTheme::default()))
            .join("\n");

    assert!(text.contains("6.5M tok"), "{text}");
    assert!(text.contains("cache 6.3M read · hit 98.6%"), "{text}");
}

#[test]
fn transcript_pane_upserts_delegate_card_from_events() {
    let mut pane = TranscriptPane::new(120, 20);
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 1,
        agent: running_delegate(),
        workflow_origin: None,
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
