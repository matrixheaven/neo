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

fn live_text(pane: &mut TranscriptPane, width: usize, height: usize) -> String {
    pane.render_terminal_update(width, height)
        .live
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
    });
}

fn finish_tool(pane: &mut TranscriptPane, id: &str, result: &str) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: id.to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok(result),
        workflow_origin: None,
    });
}

/// A live entry without a progressive projection (an ordinary running tool)
/// must stay bounded by `live_budget` and commit its canonical final card
/// exactly once at finalization — never an unbounded mutable suffix.
#[test]
fn unsupported_live_entry_stays_bounded_and_commits_once() {
    let mut pane = TranscriptPane::new(60, 8);
    start_tool(&mut pane, "bash-1", "make");
    stream_tool_output(&mut pane, "bash-1", 20);

    let live = live_text(&mut pane, 60, 8);
    let update = pane.render_terminal_update(60, 8);
    assert!(update.history.is_empty());
    assert!(
        update.live.len() <= 4,
        "live must be bounded by live_budget: {} rows\n{live}",
        update.live.len()
    );
    assert!(live.contains("Using Bash"), "live:\n{live}");

    // Finalization commits the canonical card once and removes the live area.
    finish_tool(&mut pane, "bash-1", "done");
    let finished = pane.render_terminal_update(60, 8);
    assert_eq!(finished.history.len(), 1, "one canonical commit");
    assert!(
        finished.history[0]
            .lines
            .iter()
            .any(|line| strip_ansi(line).contains("Bash")),
        "history: {:?}",
        finished.history[0].lines
    );
    assert!(
        finished.history[0]
            .lines
            .iter()
            .any(|line| strip_ansi(line).contains("done")),
        "history: {:?}",
        finished.history[0].lines
    );
    assert!(finished.live.is_empty());

    // Acknowledged history never replays.
    pane.acknowledge_history(&finished.history);
    assert!(pane.render_terminal_update(60, 8).history.is_empty());
}

/// Stable facts after an ordinary live entry commit to native history in
/// canonical order while the live entry remains mutable and bounded.
#[test]
fn stable_facts_after_ordinary_live_entry_keep_canonical_order() {
    let mut pane = TranscriptPane::new(60, 12);
    start_tool(&mut pane, "bash-1", "make");
    stream_tool_output(&mut pane, "bash-1", 4);
    pane.push_status("later-status-0");
    pane.push_status("later-status-1");

    let update = pane.render_terminal_update(60, 12);
    let history = update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    let live = update
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(history.contains("later-status-0"), "history:\n{history}");
    assert!(history.contains("later-status-1"), "history:\n{history}");
    assert!(
        history.find("later-status-0").unwrap() < history.find("later-status-1").unwrap(),
        "later stable facts must keep canonical order"
    );
    assert!(!history.contains("Using Bash"), "history:\n{history}");
    assert!(live.contains("Using Bash"), "live:\n{live}");

    // Completion appends the canonical card once, after the committed facts.
    finish_tool(&mut pane, "bash-1", "done");
    let finished = pane.render_terminal_update(60, 12);
    assert_eq!(finished.history.len(), 3, "statuses then the tool card");
    assert!(finished.live.is_empty());
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
            },
        }],
        prior_messages: Vec::new(),
        outcome: None,
    };
    pane.transcript_mut().upsert_delegate(1, running.clone());

    // Nothing from the child card is split into earlier history.
    let update = pane.render_terminal_update(120, 24);
    assert!(update.history.is_empty(), "history: {:#?}", update.history);

    // Completion commits the existing complete card once.
    running.state = AgentLifecycleState::Completed;
    running.terminal_at_ms = Some(3);
    running.updated_at_ms = 3;
    running.outcome = Some(neo_agent_core::multi_agent::AgentTerminalOutcome {
        summary: "feature implemented".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate(1, running);

    let finished = pane.render_terminal_update(120, 24);
    assert_eq!(finished.history.len(), 1, "one complete delegate card");
    let summary = finished.history[0]
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
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
    assert!(finished.live.is_empty());
    pane.acknowledge_history(&finished.history);
    assert!(pane.render_terminal_update(120, 24).history.is_empty());
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
            },
        }],
        prior_messages: Vec::new(),
        outcome: None,
    };
    pane.transcript_mut().upsert_delegate(1, running.clone());

    // The earliest unresolved approval owns the live focus; the later stable
    // fact stays deferred.
    let update = pane.render_terminal_update(120, 24);
    let live = update
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(update.history.is_empty(), "no later fact may commit");
    assert!(live.contains("Run tests?"), "live:\n{live}");
    assert!(!live.contains("Used Read"), "live:\n{live}");
    assert_eq!(
        pane.earliest_blocking_entry(),
        Some(neo_tui::transcript::BlockingEntryKind::Approval(
            "approval-1".to_owned()
        ))
    );

    // Resolution releases only the approval. Child facts remain capture-only
    // until their parent card terminalizes.
    pane.resolve_approval(
        "approval-1",
        &ApprovalResolution::Selected {
            action: ApprovalAction::PermitOnce,
            label: "Allow once".to_owned(),
            feedback: None,
        },
    );
    let update = pane.render_terminal_update(120, 24);
    let history = update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        history.contains("approval: Allow once"),
        "history:\n{history}"
    );
    assert!(!history.contains("Used Read"), "history:\n{history}");

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
    let update = pane.render_terminal_update(120, 24);
    let history = update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(history.contains("Used Read"), "history:\n{history}");
    assert!(
        history.find("approval: Allow once").unwrap() < history.find("Used Read").unwrap(),
        "canonical order violated:\n{history}"
    );
}

/// Every live-producing entry family must be handled by a bounded live
/// projection, a blocking projection, or the bounded finalization fallback:
/// bounded while live, exactly one canonical commit at finalization, and no
/// mutable data acknowledged as history.
#[test]
fn every_live_entry_family_is_bounded_and_commits_once() {
    use neo_agent_core::{
        ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
        PermissionOperation, ShellCommandOrigin, ShellCommandOutcome,
    };
    use neo_tui::transcript::entry::{RetryPhase, RetryStatusData};
    use neo_tui::transcript::{McpStartupPhase, McpStartupStatusData};

    fn plain_history(pane: &mut TranscriptPane, width: usize, height: usize) -> String {
        pane.render_terminal_update(width, height)
            .history
            .iter()
            .flat_map(|block| block.lines.iter())
            .map(|line| strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn assert_bounded_live(pane: &mut TranscriptPane, width: usize, height: usize) {
        let update = pane.render_terminal_update(width, height);
        assert!(
            update.live.len() <= height.saturating_sub(4),
            "live must be bounded by live_budget: {}",
            update.live.len()
        );
        assert!(!update.live.is_empty(), "the mutable entry stays visible");
    }

    fn assert_one_canonical_commit(pane: &mut TranscriptPane, width: usize, height: usize) {
        let update = pane.render_terminal_update(width, height);
        assert_eq!(update.history.len(), 1, "one canonical commit");
        assert!(update.live.is_empty(), "live area must clear");
        pane.acknowledge_history(&update.history);
        assert!(
            pane.render_terminal_update(width, height)
                .history
                .is_empty(),
            "acked history never replays"
        );
    }

    fn assert_ordered_canonical_commit(
        pane: &mut TranscriptPane,
        width: usize,
        height: usize,
        ordered: &[&str],
    ) {
        let update = pane.render_terminal_update(width, height);
        assert_eq!(update.history.len(), 1, "one complete card commit");
        assert!(update.live.is_empty(), "live area must clear");
        let history = update.history[0]
            .lines
            .iter()
            .map(|line| strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");
        let mut previous = 0;
        for (index, needle) in ordered.iter().enumerate() {
            let position = history
                .find(needle)
                .unwrap_or_else(|| panic!("missing {needle:?}:\n{history}"));
            if index > 0 {
                assert!(position > previous, "card order violated:\n{history}");
            }
            previous = position;
        }
        pane.acknowledge_history(&update.history);
        assert!(
            pane.render_terminal_update(width, height)
                .history
                .is_empty(),
            "acked complete card never replays"
        );
    }

    // -- ThinkingBlock: bounded until typed completion ---------------------
    let mut pane = TranscriptPane::new(100, 24);
    {
        let store = pane.transcript_mut();
        store.start_thinking();
        store.append_thinking_delta("partial thought");
    }
    assert_bounded_live(&mut pane, 100, 24);
    pane.transcript_mut().finish_thinking();
    assert_one_canonical_commit(&mut pane, 100, 24);

    // -- AssistantMessage: bounded until the attempt is canonical ----------
    let mut pane = TranscriptPane::new(100, 24);
    pane.start_assistant_message();
    pane.append_assistant_delta("streaming answer");
    assert_bounded_live(&mut pane, 100, 24);
    pane.finish_assistant_message();
    assert_one_canonical_commit(&mut pane, 100, 24);

    // -- ToolRun: bounded, commits the canonical finalized group once ------
    let mut pane = TranscriptPane::new(100, 24);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "make" }),
        workflow_origin: None,
    });
    assert_bounded_live(&mut pane, 100, 24);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok("built"),
        workflow_origin: None,
    });
    assert_one_canonical_commit(&mut pane, 100, 24);

    // -- ShellRun: bounded, commits the canonical command result once ------
    let mut pane = TranscriptPane::new(100, 24);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        command: "cargo test".to_owned(),
        cwd: "/workspace/neo".into(),
        origin: ShellCommandOrigin::UserShellMode,
    });
    assert_bounded_live(&mut pane, 100, 24);
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
    assert_one_canonical_commit(&mut pane, 100, 24);

    // -- Compaction: bounded, commits the typed terminal form once ---------
    let mut pane = TranscriptPane::new(100, 24);
    pane.transcript_mut().push(TranscriptEntry::Compaction {
        phase: Some(neo_agent_core::CompactionPhase::Summarizing),
        percent: 50,
        compacted_message_count: 3,
        tokens_before: 100,
        tokens_after: 0,
    });
    assert_bounded_live(&mut pane, 100, 24);
    pane.transcript_mut().mutate_entry(0, |entry| {
        let TranscriptEntry::Compaction { phase, percent, .. } = entry else {
            return false;
        };
        *phase = Some(neo_agent_core::CompactionPhase::Applying);
        *percent = 100;
        true
    });
    assert_one_canonical_commit(&mut pane, 100, 24);

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
    assert_bounded_live(&mut pane, 100, 24);
    pane.transcript_mut().mutate_entry(0, |entry| {
        let TranscriptEntry::RetryStatus { data } = entry else {
            return false;
        };
        data.phase = RetryPhase::Exhausted;
        true
    });
    assert_one_canonical_commit(&mut pane, 100, 24);

    // -- Connecting MCP startup: bounded, commits the settled entry once ----
    let mut pane = TranscriptPane::new(100, 24);
    pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "server".to_owned(),
        transport: "stdio".to_owned(),
        phase: McpStartupPhase::Connecting,
    });
    assert_bounded_live(&mut pane, 100, 24);
    pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "server".to_owned(),
        transport: "stdio".to_owned(),
        phase: McpStartupPhase::Connected { tool_count: 2 },
    });
    assert_one_canonical_commit(&mut pane, 100, 24);
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
    assert_bounded_live(&mut pane, 100, 24);
    assert!(plain_history(&mut pane, 100, 24).is_empty());
    pane.resolve_approval(
        "approval-1",
        &ApprovalResolution::Selected {
            action: ApprovalAction::PermitOnce,
            label: "Allow once".to_owned(),
            feedback: None,
        },
    );
    assert_one_canonical_commit(&mut pane, 100, 24);

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
    assert_bounded_live(&mut pane, 100, 24);
    assert!(plain_history(&mut pane, 100, 24).is_empty());
    pane.resolve_question_prompt("question-1", vec!["Yes".to_owned()]);
    assert_one_canonical_commit(&mut pane, 100, 24);

    // -- Delegate: one complete card keeps header before child tools -------
    let mut pane = TranscriptPane::new(100, 24);
    pane.transcript_mut().upsert_delegate(
        1,
        running_agent("agent-a", vec![done_tool("read-1", "Read", "one.rs")]),
    );
    assert_bounded_live(&mut pane, 100, 24);
    assert!(plain_history(&mut pane, 100, 24).is_empty());
    pane.transcript_mut().upsert_delegate(
        1,
        completed_agent("agent-a", vec![done_tool("read-1", "Read", "one.rs")]),
    );
    assert_ordered_canonical_commit(&mut pane, 100, 24, &["Delegate", "Used Read"]);

    // -- DelegateGroup: one complete card keeps group before child tools ---
    let mut pane = TranscriptPane::new(100, 24);
    pane.transcript_mut().upsert_delegate(
        1,
        running_agent("group-a", vec![done_tool("read-a", "Read", "a.rs")]),
    );
    assert_bounded_live(&mut pane, 100, 24);
    assert!(plain_history(&mut pane, 100, 24).is_empty());
    pane.transcript_mut().upsert_delegate(
        1,
        completed_agent("group-a", vec![done_tool("read-a", "Read", "a.rs")]),
    );
    assert_ordered_canonical_commit(&mut pane, 100, 24, &["Delegate", "Used Read"]);

    // -- DelegateSwarm: one compact terminal card keeps one row per child --
    let mut pane = TranscriptPane::new(100, 24);
    pane.transcript_mut().upsert_delegate_swarm(running_swarm(
        "swarm-1",
        vec![running_agent(
            "child-a",
            vec![done_tool("read-s", "Read", "s.rs")],
        )],
    ));
    assert_bounded_live(&mut pane, 100, 24);
    assert!(plain_history(&mut pane, 100, 24).is_empty());
    pane.set_tool_output_expanded(true);
    pane.transcript_mut().upsert_delegate_swarm(running_swarm(
        "swarm-1",
        vec![completed_agent(
            "child-a",
            vec![done_tool("read-s", "Read", "s.rs")],
        )],
    ));
    assert_ordered_canonical_commit(&mut pane, 100, 24, &["DelegateSwarm", "child-a"]);

    // -- Workflow: mutable state stays live until one terminal commit -------
    let mut pane = TranscriptPane::new(100, 24);
    pane.transcript_mut()
        .upsert_workflow(running_workflow("wf-1", 1, "verify"));
    assert_bounded_live(&mut pane, 100, 24);
    assert!(plain_history(&mut pane, 100, 24).is_empty());
    let mut completed = running_workflow("wf-1", 9, "verify");
    completed.state = WorkflowState::Completed;
    completed.updated_at_ms = Some(9_000);
    completed.terminal_reason = Some("workflow completed".to_owned());
    pane.transcript_mut().upsert_workflow(completed);
    assert_one_canonical_commit(&mut pane, 100, 24);
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
    let mut pane = TranscriptPane::new(100, 14);
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-a", tools("A")));
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-b", tools("B")));
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-c", tools("C")));
    let initial = pane.render_terminal_update(100, 14);
    assert!(
        !initial.live.iter().any(|line| line.contains("more rows")),
        "initial live group was silently shortened: {:?}",
        initial.live
    );
    assert_eq!(
        initial
            .live
            .iter()
            .filter(|line| strip_ansi(line).contains("Used Read"))
            .count(),
        3,
        "live group should show one tool per running child: {:?}",
        initial.live
    );
    pane.acknowledge_history(&initial.history);

    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-a", tools_with_new_content("A")));
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-b", tools_with_new_content("B")));

    let completed_c = completed_agent("agent-c", tools("C"));
    pane.transcript_mut().upsert_delegate(1, completed_c);
    let c_update = pane.render_terminal_update(100, 14);
    let c_history = c_update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        c_history.is_empty(),
        "completed C must stay in the live group: {c_history}"
    );
    assert!(
        c_update.live.iter().any(|line| {
            let line = strip_ansi(line);
            line.contains("agent-c") && line.contains("done")
        }),
        "completed C lost its mutable status row: {:?}",
        c_update.live
    );
    let c_live = c_update
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        c_live.contains("A-6") && c_live.contains("B-6"),
        "A/B live activity: {c_live}"
    );
    assert!(
        c_update
            .live
            .iter()
            .all(|line| !strip_ansi(line).contains("more rows")),
        "C-first live group was silently shortened: {:?}",
        c_update.live
    );
    pane.acknowledge_history(&c_update.history);

    pane.transcript_mut()
        .upsert_delegate(1, completed_agent("agent-a", tools_with_new_content("A")));
    let a_update = pane.render_terminal_update(100, 14);
    let a_history = a_update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        a_history.is_empty(),
        "completed A must stay in the live group: {a_history}"
    );
    pane.acknowledge_history(&a_update.history);

    pane.transcript_mut()
        .upsert_delegate(1, completed_agent("agent-b", tools_with_new_content("B")));
    let b_update = pane.render_terminal_update(100, 14);
    let b_history = b_update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        b_history.contains("Delegate group")
            && b_history.contains("agent-a")
            && b_history.contains("agent-b")
            && b_history.contains("agent-c"),
        "completed group must commit one status row per child: {b_history}"
    );
    assert_eq!(
        b_history.matches("Delegate group").count(),
        1,
        "group summary was duplicated: {b_history}"
    );
    assert_eq!(
        b_history.matches("Used Read").count(),
        0,
        "group history must not expand child tools: {b_history}"
    );
    assert!(
        b_update.live.is_empty(),
        "all children are terminal but the group stayed live: {:?}",
        b_update.live
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

    let update = pane.render_terminal_update(100, 9);
    let live = update
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>();
    for agent in ["agent-a", "agent-b", "agent-c", "agent-d", "agent-e"] {
        assert!(
            live.iter().any(|line| line.contains(agent)),
            "missing {agent} status row: {live:?}"
        );
    }
    assert!(
        live.iter().all(|line| !line.contains("more rows")),
        "status rows were replaced by a generic truncation summary: {live:?}"
    );
}

#[test]
fn pending_wait_delegate_is_above_running_group_regardless_of_entry_order() {
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
    let before_live = live_text(&mut before, 120, 12);
    assert_wait_group_order(&before_live);

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
    let after_live = live_text(&mut after, 120, 12);
    assert_wait_group_order(&after_live);
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

    let live = live_text(&mut pane, 120, 14);
    let wait = live.find("Waiting for").expect("wait row missing");
    let swarm = live.find("DelegateSwarm").expect("swarm header missing");
    assert!(wait < swarm, "wait must render above swarm: {live}");
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

        let update = pane.render_terminal_update(120, 12);
        let history = update
            .history
            .iter()
            .flat_map(|block| block.lines.iter())
            .map(|line| strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");
        let live = update
            .live
            .iter()
            .map(|line| strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            history.contains("Wait timed out") || history.contains("Target not found"),
            "wait outcome missing for {outcome}: {history}"
        );
        assert!(live.contains("Delegate group"), "group disappeared: {live}");
        assert!(
            !history.contains("Delegate group"),
            "group moved to history: {history}"
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
