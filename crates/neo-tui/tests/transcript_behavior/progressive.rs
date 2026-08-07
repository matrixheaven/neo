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
