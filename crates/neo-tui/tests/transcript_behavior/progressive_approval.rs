//! End-to-end presentation regressions for the progressive native-scrollback
//! transcript behavior: a live area actually bounded by `live_budget`, stable
//! facts keeping canonical order behind ordinary live entries, and a single
//! canonical commit per entry at finalization.

use neo_tui::transcript::{TranscriptEntry, TranscriptPane};

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
        input_token_count: 0,
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
