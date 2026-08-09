use neo_agent_core::multi_agent::{
    AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
    AgentSnapshot, DelegateContext, SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_agent_core::workflow::{WorkflowId, WorkflowSnapshot, WorkflowState};
use neo_agent_core::{
    ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, PermissionOperation,
};
use neo_tui::primitive::Finalization;
use neo_tui::transcript::{ShellRunComponent, TranscriptEntry, TranscriptPane, TranscriptStore};
use std::time::Duration;

fn agent_snapshot(id: &str, state: AgentLifecycleState) -> AgentSnapshot {
    let display_name = AgentDisplayName::new(id);
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id),
        display_name: display_name.clone(),
        path: AgentPath::root_child(&display_name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state,
        task: "test task".to_owned(),
        task_title: "test task".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        terminal_at_ms: state.is_terminal().then_some(2),
        detached_from_foreground: false,
        terminal_reason: None,
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
        elapsed: Duration::ZERO,
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    }
}
fn swarm_snapshot(id: &str, children: Vec<AgentSnapshot>) -> SwarmSnapshot {
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
fn workflow_snapshot(id: &str, state: WorkflowState) -> WorkflowSnapshot {
    WorkflowSnapshot {
        id: WorkflowId(id.to_owned()),
        title: "test workflow".to_owned(),
        state,
        current_phase: None,
        projection_sequence: None,
        recovery_failure: false,
        started_at_ms: None,
        updated_at_ms: None,
        invocation_count: 0,
        failure_count: 0,
        actual_usage: None,
        latest_log_summary: None,
        latest_report_summary: None,
        terminal_reason: None,
        display_name: "test workflow".to_owned(),
        purpose: String::new(),
    }
}
fn finish_test_tool(pane: &mut TranscriptPane) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("done"),

        workflow_origin: None,
        output_ref: None,
    });
}
fn request_test_approval(pane: &mut TranscriptPane) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: shell_test_request("approval-1", "printf 1"),
    });
}
fn shell_test_request(id: &str, command: &str) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: command.to_owned(),
            cwd: None,
        },
        options: shell_test_options(),

        workflow_origin: None,
    }
}
fn shell_test_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            label: "Approve once".to_owned(),
            description: None,
            action: ApprovalAction::PermitOnce,
        },
        ApprovalOption {
            label: "Reject".to_owned(),
            description: None,
            action: ApprovalAction::Reject,
        },
    ]
}

#[test]
fn terminal_delegate_ignores_late_running_snapshot() {
    let mut store = TranscriptStore::new();
    store.upsert_delegate(
        1,
        agent_snapshot("delegate", AgentLifecycleState::Completed),
    );

    store.upsert_delegate(1, agent_snapshot("delegate", AgentLifecycleState::Running));

    assert_eq!(store.entry_finalization(0), Some(Finalization::Finalized));
}

#[test]
fn terminal_exit_finalizes_every_live_entry_variant() {
    let mut pane = TranscriptPane::new(80, 24);
    {
        let store = pane.transcript_mut();
        store.start_assistant();
        store.append_assistant_delta("partial answer");
        store.start_thinking();
        store.append_thinking_delta("partial thought");
        store.push_tool_run(
            "tool-1",
            "Write",
            Some(r#"{"path":"notes.txt","content":"hello"}"#.to_owned()),
        );
        store.push_shell_run(ShellRunComponent::running("shell-1", "sleep 10"));
    }
    request_test_approval(&mut pane);
    pane.upsert_mcp_startup_status(neo_tui::transcript::McpStartupStatusData {
        id: "server".to_owned(),
        transport: "stdio".to_owned(),
        phase: neo_tui::transcript::McpStartupPhase::Connecting,
    });
    {
        let store = pane.transcript_mut();
        store.push(TranscriptEntry::Compaction {
            phase: Some(neo_agent_core::CompactionPhase::Summarizing),
            percent: 50,
            compacted_message_count: 3,
            tokens_before: 100,
            tokens_after: 0,
        });
        store.upsert_delegate(1, agent_snapshot("delegate", AgentLifecycleState::Running));
        store.upsert_delegate(2, agent_snapshot("group-a", AgentLifecycleState::Running));
        store.upsert_delegate(2, agent_snapshot("group-b", AgentLifecycleState::Queued));
        store.upsert_delegate_swarm(swarm_snapshot(
            "swarm",
            vec![agent_snapshot("child", AgentLifecycleState::Running)],
        ));
        store.upsert_workflow(workflow_snapshot("workflow", WorkflowState::Running));
    }

    assert!(
        (0..pane.transcript().entries().len())
            .any(|index| pane.transcript().entry_finalization(index) == Some(Finalization::Live))
    );

    assert!(pane.finalize_interrupted_live_entries());

    assert!((0..pane.transcript().entries().len()).all(|index| {
        pane.transcript().entry_finalization(index) == Some(Finalization::Finalized)
    }));
}

#[test]
fn terminal_mcp_status_ignores_late_connecting_update() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.upsert_mcp_startup_status(neo_tui::transcript::McpStartupStatusData {
        id: "server".to_owned(),
        transport: "stdio".to_owned(),
        phase: neo_tui::transcript::McpStartupPhase::Connected { tool_count: 3 },
    });
    let revision = pane.transcript().entry_revisions()[0];

    pane.upsert_mcp_startup_status(neo_tui::transcript::McpStartupStatusData {
        id: "server".to_owned(),
        transport: "stdio".to_owned(),
        phase: neo_tui::transcript::McpStartupPhase::Connecting,
    });

    assert_eq!(pane.transcript().entry_revisions()[0], revision);
    assert_eq!(
        pane.transcript().entry_finalization(0),
        Some(Finalization::Finalized)
    );
}

#[test]
fn terminal_swarm_ignores_late_snapshot_with_running_child() {
    let mut store = TranscriptStore::new();
    store.upsert_delegate_swarm(swarm_snapshot(
        "swarm",
        vec![agent_snapshot("first", AgentLifecycleState::Completed)],
    ));

    store.upsert_delegate_swarm(swarm_snapshot(
        "swarm",
        vec![
            agent_snapshot("first", AgentLifecycleState::Running),
            agent_snapshot("late", AgentLifecycleState::Running),
        ],
    ));

    assert_eq!(store.entry_finalization(0), Some(Finalization::Finalized));
}

#[test]
fn terminal_swarm_tick_keeps_revision_stable() {
    let mut store = TranscriptStore::new();
    store.upsert_delegate_swarm(swarm_snapshot(
        "swarm",
        vec![agent_snapshot("done", AgentLifecycleState::Completed)],
    ));
    let revision = store.entry_revisions()[0];

    assert!(!store.tick_live_entries(100));
    assert_eq!(store.entry_revisions()[0], revision);
    assert_eq!(store.entry_finalization(0), Some(Finalization::Finalized));
}

#[test]
fn terminal_tool_ignores_late_running_update() {
    let mut pane = TranscriptPane::new(80, 12);
    finish_test_tool(&mut pane);
    let revision = pane.transcript().entry_revisions()[0];

    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),

        workflow_origin: None,
        output_ref: None,
    });

    assert_eq!(
        pane.transcript().entry_finalization(0),
        Some(Finalization::Finalized)
    );
    assert_eq!(pane.transcript().entry_revisions()[0], revision);
}

#[test]
fn terminal_tool_noop_mark_unfinished_keeps_revision() {
    let mut pane = TranscriptPane::new(80, 12);
    finish_test_tool(&mut pane);
    let revision = pane.transcript().entry_revisions()[0];

    pane.apply_agent_event(neo_agent_core::AgentEvent::Error {
        turn: 1,
        message: "late turn error".to_owned(),
        code: None,
        retry_after: None,
    });

    assert_eq!(pane.transcript().entry_revisions()[0], revision);
    assert_eq!(
        pane.transcript().entry_finalization(0),
        Some(Finalization::Finalized)
    );
}
