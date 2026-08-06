//! End-to-end presentation regressions for the progressive native-scrollback
//! transcript behavior: a live area actually bounded by `live_budget`, stable
//! facts keeping canonical order behind ordinary live entries, and a single
//! canonical commit per entry at finalization.

use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentToolActivityPhase, DelegateContext,
    SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_agent_core::workflow::{WorkflowId, WorkflowSnapshot, WorkflowState};
use neo_tui::transcript::{TranscriptEntry, TranscriptPane};

/// One completed (`Done`) child tool activity entry.
fn done_tool(id: &str, name: &str, summary: &str) -> AgentActivityEntry {
    AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            summary: Some(summary.to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    }
}

fn running_agent(id: &str, activity: Vec<AgentActivityEntry>) -> AgentSnapshot {
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id),
        display_name: AgentDisplayName::new(id),
        path: AgentPath::root_child(&AgentDisplayName::new(id)),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: format!("{id} task"),
        task_title: format!("{id} task"),
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: false,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: activity
            .iter()
            .filter(|entry| matches!(entry.kind, AgentActivityKind::Tool { .. }))
            .count(),
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: std::time::Duration::ZERO,
        latest_text: None,
        activity,
        prior_messages: Vec::new(),
        outcome: None,
    }
}

fn completed_agent(id: &str, activity: Vec<AgentActivityEntry>) -> AgentSnapshot {
    let mut agent = running_agent(id, activity);
    agent.state = AgentLifecycleState::Completed;
    agent.terminal_at_ms = Some(3);
    agent.updated_at_ms = 3;
    agent.outcome = Some(neo_agent_core::multi_agent::AgentTerminalOutcome {
        summary: format!("{id} done"),
        is_error: false,
    });
    agent
}

fn running_swarm(id: &str, children: Vec<AgentSnapshot>) -> SwarmSnapshot {
    let children = children
        .into_iter()
        .enumerate()
        .map(|(item_index, agent)| SwarmChildSnapshot {
            item_index,
            item: format!("item {item_index}"),
            agent,
        })
        .collect::<Vec<_>>();
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    SwarmSnapshot {
        swarm_id: id.to_owned(),
        description: "test swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 2,
        aggregate,
        children,
    }
}

fn running_workflow(id: &str, sequence: u64, phase: &str) -> WorkflowSnapshot {
    WorkflowSnapshot {
        id: WorkflowId(id.to_owned()),
        title: "runtime workflow".to_owned(),
        state: WorkflowState::Running,
        current_phase: Some(phase.to_owned()),
        projection_sequence: Some(sequence),
        recovery_failure: false,
        started_at_ms: Some(1_000),
        updated_at_ms: Some(2_000),
        invocation_count: 1,
        failure_count: 0,
        actual_usage: None,
        latest_log_summary: None,
        latest_report_summary: None,
        terminal_reason: None,
        display_name: "runtime workflow".to_owned(),
        purpose: "test".to_owned(),
    }
}

/// Strip ANSI escape sequences so assertions can match rendered text.
fn strip_ansi(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(ch);
        }
    }
    plain
}

fn slice_text(pane: &mut TranscriptPane, width: usize, height: usize) -> String {
    pane.render_visible_slice(width, height)
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn start_wait_delegate(pane: &mut TranscriptPane, id: &str, target: &str) {
    pane.transcript_mut().push_tool_run(
        id,
        "WaitDelegate",
        Some(serde_json::json!({ "ids": [target] }).to_string()),
    );
}

fn finish_wait_delegate(pane: &mut TranscriptPane, id: &str, outcome: &str, is_error: bool) {
    assert!(pane.transcript_mut().mutate_tool(id, |tool| {
        tool.set_result(
            Some(format!("outcome: {outcome}")),
            Some(serde_json::json!({
                "kind": "delegate_wait",
                "outcome": outcome,
                "aggregate": {
                    "total": 1,
                    "terminal": 0,
                    "pending": 1,
                    "not_found": usize::from(outcome == "not_found"),
                },
                "items": []
            })),
            is_error,
            None,
        )
    }));
}

fn start_tool(pane: &mut TranscriptPane, id: &str, command: &str) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: id.to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": command }),
        workflow_origin: None,
        output_ref: None,
    });
}

fn stream_tool_output(pane: &mut TranscriptPane, id: &str, lines: usize) {
    let body = (0..lines)
        .map(|index| format!("tool-output-sentinel-{index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: id.to_owned(),
        name: "Bash".to_owned(),
        partial_result: neo_agent_core::ToolResult::ok(body),
        workflow_origin: None,
        output_ref: None,
    });
}

fn finish_tool(pane: &mut TranscriptPane, id: &str, result: &str) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: id.to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok(result),
        workflow_origin: None,
        output_ref: None,
    });
}

/// An ordinary running tool renders inside the bounded document slice and its
/// canonical final card persists after finalization — never an unbounded
/// mutable suffix.
#[test]
fn unsupported_live_entry_stays_bounded_and_commits_once() {
    let mut pane = TranscriptPane::new(60, 8);
    start_tool(&mut pane, "bash-1", "make");
    stream_tool_output(&mut pane, "bash-1", 20);

    let slice = pane.render_visible_slice(60, 8);
    let live = slice_text(&mut pane, 60, 8);
    assert!(
        slice.len() <= 8,
        "slice must be bounded by the terminal height: {} rows\n{live}",
        slice.len()
    );
    // Tail follow shows the newest output rows of the running card.
    assert!(
        live.contains("tool-output-sentinel-19"),
        "newest output row missing:\n{live}"
    );
    pane.scroll_transcript_up(usize::MAX);
    let top = slice_text(&mut pane, 60, 8);
    assert!(top.contains("Using Bash"), "card header:\n{top}");
    pane.scroll_transcript_down(usize::MAX);

    // Finalization keeps the canonical card in the document.
    finish_tool(&mut pane, "bash-1", "done");
    let finished = slice_text(&mut pane, 60, 8);
    assert!(finished.contains("Used Bash"), "slice:\n{finished}");
    assert!(finished.contains("done"), "slice:\n{finished}");
}

/// Stable facts after an ordinary live entry keep canonical order inside the
/// one document slice.
#[test]
fn stable_facts_after_ordinary_live_entry_keep_canonical_order() {
    let mut pane = TranscriptPane::new(60, 12);
    start_tool(&mut pane, "bash-1", "make");
    stream_tool_output(&mut pane, "bash-1", 4);
    pane.push_status("later-status-0");
    pane.push_status("later-status-1");

    let slice = slice_text(&mut pane, 60, 12);
    assert!(slice.contains("later-status-0"), "slice:\n{slice}");
    assert!(slice.contains("later-status-1"), "slice:\n{slice}");
    assert!(
        slice.find("later-status-0").unwrap() < slice.find("later-status-1").unwrap(),
        "later stable facts must keep canonical order"
    );
    assert!(slice.contains("Using Bash"), "slice:\n{slice}");

    // Completion keeps the tool card in the document at its entry position,
    // before the later statuses.
    finish_tool(&mut pane, "bash-1", "done");
    let finished = slice_text(&mut pane, 60, 12);
    assert!(finished.contains("later-status-0"), "slice:\n{finished}");
    assert!(finished.contains("later-status-1"), "slice:\n{finished}");
    assert!(
        finished.find("Used Bash").unwrap() < finished.find("later-status-0").unwrap(),
        "the tool card keeps its entry position:\n{finished}"
    );
}

/// A Delegate remains one card so its title always precedes child tool rows.
#[test]
fn ordinary_delegate_commits_one_complete_card_with_header_before_child_tools() {
    use neo_agent_core::multi_agent::{
        AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
        AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentToolActivityPhase, DelegateContext,
    };

    let mut pane = TranscriptPane::new(120, 24);
    let mut running = AgentSnapshot {
        id: AgentId::from_suffix_for_test("agent-a"),
        display_name: AgentDisplayName::new("agent-a"),
        path: AgentPath::root_child(&AgentDisplayName::new("agent-a")),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: "implement feature".to_owned(),
        task_title: "implement feature".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: false,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 1,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: std::time::Duration::ZERO,
        latest_text: None,
        activity: vec![AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "read-1".to_owned(),
                name: "Read".to_owned(),
                summary: Some("one.rs".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: None,
                files: Vec::new(),
                output_ref: None,
            },
        }],
        prior_messages: Vec::new(),
        outcome: None,
    };
    pane.transcript_mut().upsert_delegate(1, running.clone());

    // The delegate card is one document entry: nothing is split into an
    // earlier partition.
    let slice = slice_text(&mut pane, 120, 24);
    assert!(slice.contains("agent-a"), "slice:\n{slice}");

    // Completion keeps the existing complete card once.
    running.state = AgentLifecycleState::Completed;
    running.terminal_at_ms = Some(3);
    running.updated_at_ms = 3;
    running.outcome = Some(neo_agent_core::multi_agent::AgentTerminalOutcome {
        summary: "feature implemented".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate(1, running);

    let summary = slice_text(&mut pane, 120, 24);
    assert!(summary.contains("agent-a"), "summary:\n{summary}");
    assert!(summary.contains("Used Read"), "summary:\n{summary}");
    assert!(
        summary.contains("feature implemented"),
        "summary:\n{summary}"
    );
    assert!(
        summary.find("Delegate").unwrap() < summary.find("Used Read").unwrap(),
        "delegate header must precede child tools:\n{summary}"
    );
}

/// A pending approval defers every later stable fact; resolution releases
/// them once in canonical transcript order.
#[test]
fn pending_approval_defers_later_facts_in_canonical_order() {
    use neo_agent_core::multi_agent::{
        AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
        AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome,
        AgentToolActivityPhase, DelegateContext,
    };
    use neo_agent_core::{
        ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
        PermissionOperation,
    };

    let mut pane = TranscriptPane::new(120, 24);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            turn: 1,
            id: "approval-1".to_owned(),
            operation: PermissionOperation::Shell,
            presentation: ApprovalPresentation::Tool {
                title: "Run tests?".to_owned(),
                details: vec!["cargo test".to_owned()],
            },
            options: vec![ApprovalOption {
                action: ApprovalAction::PermitOnce,
                label: "Allow once".to_owned(),
                description: None,
            }],
            workflow_origin: None,
        },
    });

    // A later delegate completes a tool while the approval stays pending.
    let running = AgentSnapshot {
        id: AgentId::from_suffix_for_test("agent-a"),
        display_name: AgentDisplayName::new("agent-a"),
        path: AgentPath::root_child(&AgentDisplayName::new("agent-a")),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: "implement feature".to_owned(),
        task_title: "implement feature".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: false,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 1,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: std::time::Duration::ZERO,
        latest_text: None,
        activity: vec![AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "read-1".to_owned(),
                name: "Read".to_owned(),
                summary: Some("one.rs".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: None,
                files: Vec::new(),
                output_ref: None,
            },
        }],
        prior_messages: Vec::new(),
        outcome: None,
    };
    pane.transcript_mut().upsert_delegate(1, running.clone());

    // The approval is the earliest blocking entry; the delegate card stays
    // in the store but is deferred out of the visible window while the
    // approval is pending.
    let slice = slice_text(&mut pane, 120, 24);
    assert!(slice.contains("Run tests?"), "slice:\n{slice}");
    assert!(
        !slice.contains("Used Read"),
        "later facts must not enter the visible window while a blocking entry is pending:\n{slice}"
    );
    assert_eq!(
        pane.earliest_blocking_entry(),
        Some(neo_tui::transcript::BlockingEntryKind::Approval(
            "approval-1".to_owned()
        ))
    );
    let entries = pane.transcript().entries();
    let approval_position = entries
        .iter()
        .position(|entry| {
            matches!(entry, TranscriptEntry::ApprovalPrompt(data) if data.id() == "approval-1")
        })
        .expect("approval entry");
    let delegate_position = entries
        .iter()
        .position(|entry| matches!(entry, TranscriptEntry::Delegate { .. }))
        .expect("delegate entry");
    assert!(
        approval_position < delegate_position,
        "the store keeps the approval before the later delegate card"
    );

    // Resolution releases the visible focus: the resolved approval and the
    // delegate card render in canonical order.
    pane.resolve_approval(
        "approval-1",
        &ApprovalResolution::Selected {
            action: ApprovalAction::PermitOnce,
            label: "Allow once".to_owned(),
            feedback: None,
        },
    );
    let slice = slice_text(&mut pane, 120, 24);
    assert!(slice.contains("approval: Allow once"), "slice:\n{slice}");

    let completed = AgentSnapshot {
        state: AgentLifecycleState::Completed,
        updated_at_ms: 3,
        terminal_at_ms: Some(3),
        outcome: Some(AgentTerminalOutcome {
            summary: "feature implemented".to_owned(),
            is_error: false,
        }),
        ..running
    };
    pane.transcript_mut().upsert_delegate(1, completed);
    let slice = slice_text(&mut pane, 120, 24);
    assert!(slice.contains("Used Read"), "slice:\n{slice}");
    assert!(
        slice.find("approval: Allow once").unwrap() < slice.find("Used Read").unwrap(),
        "canonical order violated:\n{slice}"
    );
}

/// A long approval (taller than the viewport) that arrives while the user
/// has scrolled up must take visible focus: its action area is revealed by
/// default, scrolling moves inside the card (title and full command at the
/// top, action area at the bottom), and resolving it restores the user's
/// locked view.
#[test]
fn long_approval_scrolls_without_truncation() {
    use neo_agent_core::{
        ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
        PermissionOperation,
    };

    let mut pane = TranscriptPane::new(100, 8);
    for index in 0..6 {
        pane.push_status(format!("context-{index}"));
    }
    // The user scrolled up to read history: the view is locked above the
    // tail, far from where the approval will land.
    let _ = pane.render_visible_slice(100, 8);
    pane.scroll_transcript_up(2);
    assert!(
        !pane.document().is_following_tail(),
        "the user's upward scroll locks the view"
    );

    // A long approval (taller than the 8-row viewport) arrives after the
    // context rows.
    let long_command = format!("run-{}", "echo step && ".repeat(20));
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            turn: 1,
            id: "long-approval".to_owned(),
            operation: PermissionOperation::Shell,
            presentation: ApprovalPresentation::Command {
                title: "Approve long command?".to_owned(),
                command: long_command.clone(),
                cwd: None,
            },
            options: vec![ApprovalOption {
                action: ApprovalAction::PermitOnce,
                label: "Allow once".to_owned(),
                description: None,
            }],
            workflow_origin: None,
        },
    });
    let approval_index = pane
        .transcript()
        .entries()
        .iter()
        .position(|entry| {
            matches!(entry, TranscriptEntry::ApprovalPrompt(data) if data.id() == "long-approval")
        })
        .expect("approval entry");

    // The blocking focus overrides the user's lock: the action area is
    // visible by default even though the user had scrolled up.
    let slice = slice_text(&mut pane, 100, 8);
    assert!(
        slice.contains("Allow once"),
        "action area default-visible after an up-scroll:\n{slice}"
    );
    assert!(
        slice.contains("↑/↓ select"),
        "action hint default-visible:\n{slice}"
    );
    let block_rows = pane
        .document()
        .block_height(approval_index)
        .expect("approval block");
    assert!(
        block_rows > 8,
        "the long approval must be taller than the viewport: {block_rows}"
    );

    // The document never truncates the card: the full block stays in the
    // document while the visible window confines itself to the card.
    let full = pane.render_frame(100, 40).expect("full frame");
    assert_eq!(pane.document().total_rows(), full.len(), "geometry exact");
    let full_text = full
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(full_text.contains("echo step"), "complete card in document");

    // Scrolling to the top reaches the title and the command; the window
    // never leaks into the context rows above the card.
    pane.scroll_transcript_up(usize::MAX);
    let top = slice_text(&mut pane, 100, 8);
    assert!(
        top.contains("Approve long command?"),
        "title reachable by scrolling up:\n{top}"
    );
    assert!(top.contains("echo step"), "command reachable:\n{top}");
    assert!(
        !top.contains("context-"),
        "scrolling stays inside the card:\n{top}"
    );
    assert!(
        !top.contains("Allow once"),
        "the action area is above the viewport at the card top:\n{top}"
    );

    // Scrolling back down returns to the action area.
    pane.scroll_transcript_down(usize::MAX);
    let bottom = slice_text(&mut pane, 100, 8);
    assert!(
        bottom.contains("Allow once"),
        "action area reachable again:\n{bottom}"
    );
    assert!(
        bottom.contains("↑/↓ select"),
        "action hint reachable again:\n{bottom}"
    );

    // Resolving the approval restores the user's locked view.
    pane.resolve_approval(
        "long-approval",
        &ApprovalResolution::Selected {
            action: ApprovalAction::PermitOnce,
            label: "Allow once".to_owned(),
            feedback: None,
        },
    );
    assert_eq!(pane.earliest_blocking_entry(), None);
    let restored = slice_text(&mut pane, 100, 8);
    assert!(
        restored.contains("context-"),
        "the user's locked view is restored:\n{restored}"
    );
    assert!(
        pane.document().view().anchor.is_some() && !pane.document().is_following_tail(),
        "the lock survives the blocking focus"
    );
}

/// A short blocking card (shorter than the viewport) pins the window's
/// lower boundary at the card's end while leaving the window free to drift
/// above the card: the default shows the action area at the viewport bottom
/// with preceding context above; scrolling up reveals earlier rows (the
/// card slides below the window), scrolling down always returns to the
/// action area, and later content past the card's end never enters the
/// window. The user's own view state is never touched while the focus is
/// engaged, so releasing it restores the ordinary follow or locked view.
#[test]
fn short_approval_scroll_stays_within_blocking_focus() {
    use neo_agent_core::{
        ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, PermissionOperation,
    };

    fn request_approval(pane: &mut TranscriptPane, id: &str) {
        pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
            request: ApprovalRequest {
                turn: 1,
                id: id.to_owned(),
                operation: PermissionOperation::Shell,
                presentation: ApprovalPresentation::Tool {
                    title: "Approve short?".to_owned(),
                    details: vec!["run the tool?".to_owned()],
                },
                options: vec![ApprovalOption {
                    action: ApprovalAction::PermitOnce,
                    label: "Approve".to_owned(),
                    description: None,
                }],
                workflow_origin: None,
            },
        });
    }

    let mut pane = TranscriptPane::new(100, 12);
    for index in 0..8 {
        pane.push_status(format!("context-{index}"));
    }
    request_approval(&mut pane, "short-approval");
    pane.push_status("after-card");

    // The card is shorter than the 12-row viewport.
    let default_slice = slice_text(&mut pane, 100, 12);
    let approval_index = pane
        .transcript()
        .entries()
        .iter()
        .position(|entry| {
            matches!(entry, TranscriptEntry::ApprovalPrompt(data) if data.id() == "short-approval")
        })
        .expect("approval entry");
    assert!(
        pane.document()
            .block_height(approval_index)
            .expect("approval block")
            < 12,
        "the approval must be shorter than the viewport"
    );

    // Default: the action area sits at the viewport bottom with preceding
    // context above the card, and later content never enters the window.
    assert!(
        default_slice.contains("context-7"),
        "context above the card stays visible:\n{default_slice}"
    );
    assert!(
        default_slice.contains("1. Approve"),
        "action area visible:\n{default_slice}"
    );
    assert!(
        default_slice.contains("↑/↓ select"),
        "action hint visible:\n{default_slice}"
    );
    assert!(
        !default_slice.contains("after-card"),
        "content past the card end never enters the window:\n{default_slice}"
    );

    // Scrolling up drifts the window above the card: earlier context rows
    // are revealed and the action area slides below the viewport bottom,
    // while the user's own view state stays untouched.
    pane.scroll_transcript_up(3);
    let drifted = slice_text(&mut pane, 100, 12);
    assert!(
        drifted.contains("context-6"),
        "scrolling up reveals earlier context:\n{drifted}"
    );
    assert!(
        !drifted.contains("↑/↓ select"),
        "the action area slides below the window:\n{drifted}"
    );
    assert!(
        !drifted.contains("after-card"),
        "the lower boundary never passes the card end:\n{drifted}"
    );
    assert_eq!(
        pane.document().view().anchor,
        None,
        "blocking scrolls never move the user's anchor"
    );
    assert!(
        pane.document().is_following_tail(),
        "blocking scrolls never touch the user's follow state"
    );

    // Scrolling back down restores the default: the action area returns to
    // the viewport bottom.
    pane.scroll_transcript_down(3);
    let restored = slice_text(&mut pane, 100, 12);
    assert!(
        restored.contains("↑/↓ select"),
        "action hint reachable again:\n{restored}"
    );
    assert!(
        !restored.contains("context-6"),
        "the window returned to the default position:\n{restored}"
    );
    assert!(
        !restored.contains("after-card"),
        "still bounded by the card end:\n{restored}"
    );

    // Even a maximum scroll-down never passes the card's end while the
    // focus is engaged.
    pane.scroll_transcript_down(usize::MAX);
    let bottom = slice_text(&mut pane, 100, 12);
    assert!(
        !bottom.contains("after-card"),
        "the lower boundary is the card end:\n{bottom}"
    );

    // When the document prefix is shorter than the viewport, the window
    // clamps at the card end instead of overflowing into later content;
    // scrolling up has nothing to reveal and changes nothing.
    let mut pane = TranscriptPane::new(100, 12);
    request_approval(&mut pane, "short-approval-2");
    pane.push_status("after-card-2");
    let clamped = slice_text(&mut pane, 100, 12);
    assert!(
        clamped.contains("1. Approve"),
        "action area visible:\n{clamped}"
    );
    assert!(
        !clamped.contains("after-card-2"),
        "the window clamps at the card end:\n{clamped}"
    );
    pane.scroll_transcript_up(3);
    let clamped_up = slice_text(&mut pane, 100, 12);
    assert!(
        clamped_up.contains("1. Approve"),
        "the clamped window is unchanged:\n{clamped_up}"
    );
    assert!(
        !clamped_up.contains("after-card-2"),
        "still clamped at the card end:\n{clamped_up}"
    );
}

/// Every live-producing entry family renders inside the one bounded document
/// while live and keeps its canonical content after completion — the document
/// is the single slice owner, so there is no separate mutable suffix.
#[test]
fn every_live_entry_family_renders_bounded_and_keeps_canonical_content() {
    use neo_agent_core::{
        ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
        PermissionOperation, ShellCommandOrigin, ShellCommandOutcome,
    };
    use neo_tui::transcript::entry::{RetryPhase, RetryStatusData};
    use neo_tui::transcript::{McpStartupPhase, McpStartupStatusData};

    fn plain_slice(pane: &mut TranscriptPane, width: usize, height: usize) -> String {
        pane.render_visible_slice(width, height)
            .iter()
            .map(|line| strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn assert_live(pane: &mut TranscriptPane, width: usize, height: usize, needle: &str) {
        let slice = pane.render_visible_slice(width, height);
        assert!(
            slice.len() <= height,
            "slice must be bounded: {}",
            slice.len()
        );
        assert!(
            plain_slice(pane, width, height).contains(needle),
            "missing live {needle:?} in slice:\n{}",
            plain_slice(pane, width, height)
        );
    }

    fn assert_completed(pane: &mut TranscriptPane, width: usize, height: usize, needle: &str) {
        assert!(
            plain_slice(pane, width, height).contains(needle),
            "missing completed {needle:?} in slice:\n{}",
            plain_slice(pane, width, height)
        );
    }

    fn assert_ordered_completed(
        pane: &mut TranscriptPane,
        width: usize,
        height: usize,
        ordered: &[&str],
    ) {
        let slice = plain_slice(pane, width, height);
        let mut previous = 0;
        for (index, needle) in ordered.iter().enumerate() {
            let position = slice
                .find(needle)
                .unwrap_or_else(|| panic!("missing {needle:?}:\n{slice}"));
            if index > 0 {
                assert!(position > previous, "card order violated:\n{slice}");
            }
            previous = position;
        }
    }

    // -- ThinkingBlock: bounded until typed completion ---------------------
    let mut pane = TranscriptPane::new(100, 24);
    {
        let store = pane.transcript_mut();
        store.start_thinking();
        store.append_thinking_delta("partial thought");
    }
    assert_live(&mut pane, 100, 24, "partial thought");
    pane.transcript_mut().finish_thinking(false);
    assert_completed(&mut pane, 100, 24, "partial thought");

    // -- AssistantMessage: bounded until the attempt is canonical ----------
    let mut pane = TranscriptPane::new(100, 24);
    pane.start_assistant_message();
    pane.append_assistant_delta("streaming answer");
    assert_live(&mut pane, 100, 24, "streaming answer");
    pane.finish_assistant_message();
    assert_completed(&mut pane, 100, 24, "streaming answer");

    // -- ToolRun: bounded, commits the canonical finalized group once ------
    let mut pane = TranscriptPane::new(100, 24);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "make" }),
        workflow_origin: None,
        output_ref: None,
    });
    assert_live(&mut pane, 100, 24, "make");
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok("built"),
        workflow_origin: None,
        output_ref: None,
    });
    assert_ordered_completed(&mut pane, 100, 24, &["Used Bash", "make", "built"]);

    // -- ShellRun: bounded, commits the canonical command result once ------
    let mut pane = TranscriptPane::new(100, 24);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        command: "cargo test".to_owned(),
        cwd: "/workspace/neo".into(),
        origin: ShellCommandOrigin::UserShellMode,
    });
    assert_live(&mut pane, 100, 24, "cargo test");
    pane.transcript_mut().mutate_shell_run("shell-1", |shell| {
        shell.finish(
            "ok".to_owned(),
            String::new(),
            Some(0),
            None,
            ShellCommandOutcome::Completed,
            false,
        )
    });
    assert_completed(&mut pane, 100, 24, "cargo test");

    // -- Compaction: bounded, commits the typed terminal form once ---------
    let mut pane = TranscriptPane::new(100, 24);
    pane.transcript_mut().push(TranscriptEntry::Compaction {
        phase: Some(neo_agent_core::CompactionPhase::Summarizing),
        percent: 50,
        compacted_message_count: 3,
        tokens_before: 100,
        tokens_after: 0,
    });
    assert_live(&mut pane, 100, 24, "50%");
    pane.transcript_mut().mutate_entry(0, |entry| {
        let TranscriptEntry::Compaction { phase, percent, .. } = entry else {
            return false;
        };
        *phase = Some(neo_agent_core::CompactionPhase::Applying);
        *percent = 100;
        true
    });
    assert_completed(&mut pane, 100, 24, "Compaction complete");

    // -- RetryStatus: bounded, commits only its canonical terminal form ----
    let mut pane = TranscriptPane::new(100, 24);
    pane.transcript_mut()
        .push(TranscriptEntry::retry_status(RetryStatusData {
            turn: 1,
            retry: 1,
            max_retries: 3,
            phase: RetryPhase::Waiting,
            delay_ms: 500,
            started_at_ms: 1,
            error_code: "rate_limited".to_owned(),
            message: "slow down".to_owned(),
        }));
    assert_live(&mut pane, 100, 24, "slow down");
    pane.transcript_mut().mutate_entry(0, |entry| {
        let TranscriptEntry::RetryStatus { data } = entry else {
            return false;
        };
        data.phase = RetryPhase::Exhausted;
        true
    });
    assert_completed(&mut pane, 100, 24, "slow down");

    // -- Connecting MCP startup: bounded, commits the settled entry once ----
    let mut pane = TranscriptPane::new(100, 24);
    pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "server".to_owned(),
        transport: "stdio".to_owned(),
        phase: McpStartupPhase::Connecting,
    });
    assert_live(&mut pane, 100, 24, "server");
    pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "server".to_owned(),
        transport: "stdio".to_owned(),
        phase: McpStartupPhase::Connected { tool_count: 2 },
    });
    assert_completed(&mut pane, 100, 24, "server");
    assert!(
        !pane.upsert_mcp_startup_status(McpStartupStatusData {
            id: "server".to_owned(),
            transport: "stdio".to_owned(),
            phase: McpStartupPhase::Connecting,
        }),
        "settled MCP state rejects late updates"
    );

    // -- Pending approval: blocking projector ------------------------------
    let mut pane = TranscriptPane::new(100, 24);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            turn: 1,
            id: "approval-1".to_owned(),
            operation: PermissionOperation::Shell,
            presentation: ApprovalPresentation::Tool {
                title: "Run tests?".to_owned(),
                details: vec!["cargo test".to_owned()],
            },
            options: vec![ApprovalOption {
                action: ApprovalAction::PermitOnce,
                label: "Allow once".to_owned(),
                description: None,
            }],
            workflow_origin: None,
        },
    });
    assert_live(&mut pane, 100, 24, "Run tests?");
    pane.resolve_approval(
        "approval-1",
        &ApprovalResolution::Selected {
            action: ApprovalAction::PermitOnce,
            label: "Allow once".to_owned(),
            feedback: None,
        },
    );
    assert_completed(&mut pane, 100, 24, "approval: Allow once");

    // -- Pending question: blocking projector ------------------------------
    let mut pane = TranscriptPane::new(100, 24);
    pane.upsert_question_prompt(
        "question-1",
        vec![neo_tui::dialogs::QuestionDisplayData {
            question: "Continue?".to_owned(),
            header: Some("Choice".into()),
            body: None,
            options: vec![neo_tui::dialogs::QuestionDisplayOption {
                label: "Yes".to_owned(),
                description: None,
            }],
            multi_select: false,
        }],
    );
    assert_live(&mut pane, 100, 24, "Continue?");
    pane.resolve_question_prompt("question-1", vec!["Yes".to_owned()]);
    assert_completed(&mut pane, 100, 24, "question: answered · Yes");

    // -- Delegate: one complete card keeps header before child tools -------
    let mut pane = TranscriptPane::new(100, 24);
    pane.transcript_mut().upsert_delegate(
        1,
        running_agent("agent-a", vec![done_tool("read-1", "Read", "one.rs")]),
    );
    assert_live(&mut pane, 100, 24, "agent-a");
    pane.transcript_mut().upsert_delegate(
        1,
        completed_agent("agent-a", vec![done_tool("read-1", "Read", "one.rs")]),
    );
    assert_ordered_completed(&mut pane, 100, 24, &["Delegate", "Used Read"]);

    // -- DelegateGroup: one complete card keeps group before child tools ---
    let mut pane = TranscriptPane::new(100, 24);
    pane.transcript_mut().upsert_delegate(
        1,
        running_agent("group-a", vec![done_tool("read-a", "Read", "a.rs")]),
    );
    assert_live(&mut pane, 100, 24, "group-a");
    pane.transcript_mut().upsert_delegate(
        1,
        completed_agent("group-a", vec![done_tool("read-a", "Read", "a.rs")]),
    );
    assert_ordered_completed(&mut pane, 100, 24, &["Delegate", "Used Read"]);

    // -- DelegateSwarm: one compact terminal card keeps one row per child --
    let mut pane = TranscriptPane::new(100, 24);
    pane.transcript_mut().upsert_delegate_swarm(running_swarm(
        "swarm-1",
        vec![running_agent(
            "child-a",
            vec![done_tool("read-s", "Read", "s.rs")],
        )],
    ));
    assert_live(&mut pane, 100, 24, "child-a");
    pane.set_tool_output_expanded(true);
    pane.transcript_mut().upsert_delegate_swarm(running_swarm(
        "swarm-1",
        vec![completed_agent(
            "child-a",
            vec![done_tool("read-s", "Read", "s.rs")],
        )],
    ));
    assert_ordered_completed(&mut pane, 100, 24, &["DelegateSwarm", "child-a"]);

    // -- Workflow: mutable state stays live until one terminal commit -------
    let mut pane = TranscriptPane::new(100, 24);
    pane.transcript_mut()
        .upsert_workflow(running_workflow("wf-1", 1, "verify"));
    assert_live(&mut pane, 100, 24, "verify");
    let mut completed = running_workflow("wf-1", 9, "verify");
    completed.state = WorkflowState::Completed;
    completed.updated_at_ms = Some(9_000);
    completed.terminal_reason = Some("workflow completed".to_owned());
    pane.transcript_mut().upsert_workflow(completed);
    assert_completed(&mut pane, 100, 24, "verify");
}

#[test]
fn delegate_group_completion_order_keeps_done_rows_in_group() {
    let tools = |agent: &str| {
        (0..6)
            .map(|index| {
                done_tool(
                    &format!("{agent}-tool-{index}"),
                    "Read",
                    &format!("{agent}-{index}"),
                )
            })
            .collect::<Vec<_>>()
    };
    let tools_with_new_content = |agent: &str| {
        let mut activity = tools(agent);
        activity.push(done_tool(
            &format!("{agent}-tool-6"),
            "Read",
            &format!("{agent}-6"),
        ));
        activity
    };
    let mut pane = TranscriptPane::new(100, 40);
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-a", tools("A")));
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-b", tools("B")));
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-c", tools("C")));
    let initial = pane.render_visible_slice(100, 40);
    assert!(
        !initial.iter().any(|line| line.contains("more rows")),
        "initial group was silently shortened: {:?}",
        initial
    );
    let initial_text = initial
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    // The running group keeps every child and their latest activity.
    assert!(
        initial_text.contains("agent-a")
            && initial_text.contains("agent-b")
            && initial_text.contains("agent-c"),
        "group keeps every child: {initial_text}"
    );

    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-a", tools_with_new_content("A")));
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-b", tools_with_new_content("B")));

    let completed_c = completed_agent("agent-c", tools("C"));
    pane.transcript_mut().upsert_delegate(1, completed_c);
    let c_slice = pane.render_visible_slice(100, 40);
    let c_text = c_slice
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        c_slice.iter().any(|line| {
            let line = strip_ansi(line);
            line.contains("agent-c") && line.contains("done")
        }),
        "completed C lost its status row: {:?}",
        c_slice
    );
    assert!(
        c_text.contains("A-6") && c_text.contains("B-6"),
        "A/B live activity: {c_text}"
    );
    assert!(
        c_slice
            .iter()
            .all(|line| !strip_ansi(line).contains("more rows")),
        "C-first group was silently shortened: {:?}",
        c_slice
    );

    pane.transcript_mut()
        .upsert_delegate(1, completed_agent("agent-a", tools_with_new_content("A")));
    let a_slice = pane.render_visible_slice(100, 40);
    assert!(
        a_slice
            .iter()
            .any(|line| strip_ansi(line).contains("agent-a")),
        "completed A stays in the group: {:?}",
        a_slice
    );

    pane.transcript_mut()
        .upsert_delegate(1, completed_agent("agent-b", tools_with_new_content("B")));
    let b_slice = pane.render_visible_slice(100, 40);
    let b_text = b_slice
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        b_text.contains("Delegate group")
            && b_text.contains("agent-a")
            && b_text.contains("agent-b")
            && b_text.contains("agent-c"),
        "completed group must keep one row per child: {b_text}"
    );
    assert_eq!(
        b_text.matches("Delegate group").count(),
        1,
        "group summary was duplicated: {b_text}"
    );
    assert!(
        b_text.contains("A-6") && b_text.contains("B-6"),
        "archived results lost the latest child activity: {b_text}"
    );
}

#[test]
fn delegate_group_live_status_rows_survive_a_short_viewport() {
    let tools = |agent: &str| {
        (0..6)
            .map(|index| {
                done_tool(
                    &format!("{agent}-tool-{index}"),
                    "Read",
                    &format!("{agent}-{index}"),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut pane = TranscriptPane::new(100, 9);
    for agent in ["agent-a", "agent-b", "agent-c", "agent-d", "agent-e"] {
        pane.transcript_mut()
            .upsert_delegate(1, running_agent(agent, tools(agent)));
    }

    let slice = pane
        .render_visible_slice(100, 9)
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>();
    assert!(slice.len() <= 9, "slice must stay bounded: {slice:?}");
    // Tail follow shows the newest child rows; earlier children stay
    // reachable by scrolling up.
    assert!(
        slice.iter().any(|line| line.contains("agent-e")),
        "missing newest agent status row: {slice:?}"
    );
    assert!(
        slice.iter().all(|line| !line.contains("more rows")),
        "status rows were replaced by a generic truncation summary: {slice:?}"
    );
    pane.scroll_transcript_up(usize::MAX);
    let top = pane
        .render_visible_slice(100, 9)
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>();
    assert!(
        top.iter().any(|line| line.contains("agent-a")),
        "agent-a stays reachable: {top:?}"
    );
}

#[test]
fn pending_wait_delegate_renders_in_entry_order_around_the_running_group() {
    let mut before = TranscriptPane::new(120, 12);
    let agent = running_agent(
        "wait-before",
        vec![done_tool("wait-before-tool", "Read", "before.rs")],
    );
    let target = agent.id.as_str().to_owned();
    let other = running_agent("wait-before-other", Vec::new());
    start_wait_delegate(&mut before, "wait-before-call", &target);
    before.transcript_mut().upsert_delegate(1, agent);
    before.transcript_mut().upsert_delegate(1, other);
    let before_slice = slice_text(&mut before, 120, 12);
    assert_wait_group_order(&before_slice);

    // Entry order is canonical: when the wait call arrives after the group,
    // it renders after it.
    let mut after = TranscriptPane::new(120, 12);
    let agent = running_agent(
        "wait-after",
        vec![done_tool("wait-after-tool", "Read", "after.rs")],
    );
    let target = agent.id.as_str().to_owned();
    let other = running_agent("wait-after-other", Vec::new());
    after.transcript_mut().upsert_delegate(1, agent);
    after.transcript_mut().upsert_delegate(1, other);
    start_wait_delegate(&mut after, "wait-after-call", &target);
    let after_slice = slice_text(&mut after, 120, 12);
    assert!(
        after_slice.find("Delegate group").unwrap() < after_slice.find("Waiting for").unwrap(),
        "wait call renders after the group it follows in entry order: {after_slice}"
    );
}

#[test]
fn pending_wait_delegate_is_above_running_swarm() {
    let mut pane = TranscriptPane::new(120, 14);
    let swarm = running_swarm(
        "swarm-wait",
        vec![
            running_agent("swarm-wait-a", Vec::new()),
            running_agent("swarm-wait-b", Vec::new()),
        ],
    );
    let target = swarm.swarm_id.clone();
    start_wait_delegate(&mut pane, "wait-swarm-call", &target);
    pane.transcript_mut().upsert_delegate_swarm(swarm);

    let slice = slice_text(&mut pane, 120, 14);
    let wait = slice.find("Waiting for").expect("wait row missing");
    let swarm = slice.find("DelegateSwarm").expect("swarm header missing");
    assert!(wait < swarm, "wait must render above swarm: {slice}");
}

#[test]
fn ended_wait_delegate_does_not_remove_running_group() {
    for (outcome, is_error) in [("wait_timed_out", false), ("not_found", true)] {
        let mut pane = TranscriptPane::new(120, 12);
        let agent = running_agent(
            "wait-ended",
            vec![done_tool("wait-ended-tool", "Read", "ended.rs")],
        );
        let target = agent.id.as_str().to_owned();
        let other = running_agent("wait-ended-other", Vec::new());
        pane.transcript_mut().upsert_delegate(1, agent);
        pane.transcript_mut().upsert_delegate(1, other);
        start_wait_delegate(&mut pane, "wait-ended-call", &target);
        finish_wait_delegate(&mut pane, "wait-ended-call", outcome, is_error);

        let slice = slice_text(&mut pane, 120, 12);
        assert!(
            slice.contains("Wait timed out") || slice.contains("Target not found"),
            "wait outcome missing for {outcome}: {slice}"
        );
        assert!(
            slice.contains("Delegate group"),
            "group disappeared: {slice}"
        );
    }
}

fn assert_wait_group_order(live: &str) {
    let wait = live.find("Waiting for").expect("wait row missing");
    let group = live
        .find("Delegate group")
        .unwrap_or_else(|| panic!("group header missing: {live}"));
    assert!(wait < group, "wait must render above group: {live}");
    assert_eq!(
        live.matches("Waiting for").count(),
        1,
        "duplicate wait: {live}"
    );
    assert_eq!(
        live.matches("Delegate group").count(),
        1,
        "duplicate group: {live}"
    );
}
