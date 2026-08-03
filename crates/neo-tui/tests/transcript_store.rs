use std::time::Duration;

use neo_agent_core::instructions::{
    InstructionBundleMetadata, InstructionEpochData, InstructionEpochOutcome, InstructionScopeData,
    InstructionScopeKind,
};
use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome,
    AgentToolActivityPhase, DelegateContext, SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_agent_core::workflow::{WorkflowId, WorkflowSnapshot, WorkflowState};
use neo_agent_core::{
    ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
    PermissionOperation,
};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Component, Finalization, strip_ansi};
use neo_tui::transcript::{
    ShellRunComponent, ThinkingPart, ThinkingPhase, TranscriptEntry, TranscriptPane,
    TranscriptStore,
};

fn tool_activity(
    id: &str,
    name: &str,
    summary: &str,
    phase: AgentToolActivityPhase,
) -> AgentActivityEntry {
    AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            summary: Some(summary.to_owned()),
            phase,
            output: None,
            files: Vec::new(),
        },
    }
}

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

fn workflow_snapshot_at(id: &str, state: WorkflowState, sequence: u64) -> WorkflowSnapshot {
    WorkflowSnapshot {
        projection_sequence: Some(sequence),
        ..workflow_snapshot(id, state)
    }
}

fn finish_test_tool(pane: &mut TranscriptPane) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),

        workflow_origin: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("done"),

        workflow_origin: None,
    });
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

fn approved_resolution() -> ApprovalResolution {
    ApprovalResolution::Selected {
        action: ApprovalAction::PermitOnce,
        label: "Approved".to_owned(),
        feedback: None,
    }
}

fn request_test_approval(pane: &mut TranscriptPane) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: shell_test_request("approval-1", "printf 1"),
    });
}

fn thinking_contents(store: &TranscriptStore) -> Vec<String> {
    store
        .entries()
        .iter()
        .filter_map(TranscriptEntry::thinking_content)
        .collect()
}

fn plain_rows(store: &TranscriptStore) -> Vec<String> {
    store
        .render_rows(80, &TuiTheme::default())
        .into_iter()
        .map(|row| strip_ansi(&row.to_ansi()).trim_end().to_owned())
        .collect()
}

#[test]
fn transcript_store_renders_entries_without_draining_them() {
    let mut store = TranscriptStore::new();

    store.push(TranscriptEntry::banner("Welcome to neo"));
    store.push(TranscriptEntry::user_message("hello"));

    let first = plain_rows(&store);
    let second = plain_rows(&store);

    assert!(first.iter().any(|row| row.contains("Welcome to neo")));
    assert!(
        first
            .iter()
            .any(|row| row.contains("✨") && row.contains("hello"))
    );
    assert_eq!(first, second);
    assert_eq!(store.entries().len(), 2);
}

#[test]
fn streaming_assistant_uses_the_same_rows_after_finish() {
    let mut store = TranscriptStore::new();

    store.push(TranscriptEntry::user_message("hello"));
    store.start_assistant();
    store.append_assistant_delta("working");
    let streaming = plain_rows(&store);

    store.finish_assistant();
    let complete = plain_rows(&store);

    assert_eq!(streaming, complete);
    assert!(
        complete
            .iter()
            .any(|row| row.contains("●") && row.contains("working"))
    );
}

#[test]
fn entry_ids_survive_in_place_updates_and_track_removal() {
    let mut store = TranscriptStore::new();
    store.push(TranscriptEntry::status("first"));
    store.start_assistant();

    let ids = store.entry_ids().to_vec();
    let revisions = store.entry_revisions().to_vec();

    store.append_assistant_delta("answer");

    assert_eq!(store.entry_ids(), ids);
    assert_eq!(store.entry_revisions()[0], revisions[0]);
    assert!(store.entry_revisions()[1] > revisions[1]);

    store.remove(0);

    assert_eq!(store.entry_ids(), &ids[1..]);
    assert_eq!(store.entry_revisions().len(), 1);
}

#[test]
fn active_assistant_is_live_until_finish() {
    let mut store = TranscriptStore::new();
    store.start_assistant();

    assert_eq!(store.entry_finalization(0), Some(Finalization::Live));

    store.finish_assistant();

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

#[test]
fn no_op_entry_mutation_keeps_revision_stable() {
    let mut store = TranscriptStore::new();
    store.push(TranscriptEntry::status("ready"));
    let revision = store.entry_revisions()[0];

    assert!(!store.mutate_entry(0, |_| false));
    assert_eq!(store.entry_revisions()[0], revision);
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
fn resumed_delegate_appends_new_run_card() {
    let mut store = TranscriptStore::new();
    let completed = agent_snapshot("delegate", AgentLifecycleState::Completed);
    let agent_id = completed.id.clone();
    store.upsert_delegate(1, completed);
    let completed_entry_id = store.entry_ids()[0];

    let mut resumed = agent_snapshot("delegate", AgentLifecycleState::Running);
    resumed.run_count = 2;
    resumed.resumed_from = Some(agent_id);
    resumed.task_title = "resumed task".to_owned();
    store.upsert_delegate(2, resumed.clone());

    assert_eq!(store.entries().len(), 2);
    assert_eq!(store.entry_ids()[0], completed_entry_id);
    assert_eq!(store.entry_finalization(0), Some(Finalization::Finalized));
    assert_eq!(store.entry_finalization(1), Some(Finalization::Live));

    resumed.tool_count = 7;
    store.upsert_delegate_progress(2, &resumed.progress_snapshot());
    let TranscriptEntry::Delegate { component } = &store.entries()[1] else {
        panic!("resumed run should render as a new delegate card");
    };
    assert_eq!(component.snapshot().run_count, 2);
    assert_eq!(component.snapshot().tool_count, 7);
}

#[test]
fn delegate_group_replacement_preserves_entry_identity() {
    let mut store = TranscriptStore::new();
    store.upsert_delegate(1, agent_snapshot("first", AgentLifecycleState::Running));
    let entry_id = store.entry_ids()[0];

    store.upsert_delegate(1, agent_snapshot("second", AgentLifecycleState::Running));

    assert_eq!(store.entry_ids()[0], entry_id);
    assert!(matches!(
        store.entries()[0],
        TranscriptEntry::DelegateGroup { .. }
    ));
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
fn terminal_workflow_ignores_late_running_snapshot() {
    let mut store = TranscriptStore::new();
    store.upsert_workflow(workflow_snapshot("workflow", WorkflowState::Completed));

    store.upsert_workflow(workflow_snapshot("workflow", WorkflowState::Running));

    assert_eq!(store.entry_finalization(0), Some(Finalization::Finalized));
}

#[test]
fn workflow_updates_do_not_break_active_text_boundary() {
    let mut pane = TranscriptPane::new(120, 20);
    let mut workflow = workflow_snapshot_at("workflow", WorkflowState::Running, 1);
    pane.apply_agent_event(neo_agent_core::AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: workflow.clone(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 2,
        id: "reasoning".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 2,
        text: "continuous".to_owned(),
    });

    workflow.projection_sequence = Some(2);
    pane.apply_agent_event(neo_agent_core::AgentEvent::WorkflowUpdated { turn: 2, workflow });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 2,
        text: " thinking".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
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
    assert_eq!(thinking, vec!["continuous thinking"]);
}

#[test]
fn rehydrated_paused_workflow_ignores_stale_historical_running() {
    let mut store = TranscriptStore::new();
    store.upsert_workflow(workflow_snapshot("workflow", WorkflowState::Running));
    assert!(store.finalize_interrupted_live_entries());
    store.upsert_workflow(workflow_snapshot_at("workflow", WorkflowState::Paused, 2));
    store.upsert_workflow(workflow_snapshot("workflow", WorkflowState::Running));

    let TranscriptEntry::Workflow { component } = &store.entries()[0] else {
        panic!("workflow card")
    };
    assert_eq!(component.snapshot().state, WorkflowState::Paused);
}

#[test]
fn rehydrated_terminal_workflow_ignores_stale_historical_running() {
    let mut store = TranscriptStore::new();
    store.upsert_workflow(workflow_snapshot("workflow", WorkflowState::Running));
    assert!(store.finalize_interrupted_live_entries());
    store.upsert_workflow(workflow_snapshot_at(
        "workflow",
        WorkflowState::ResourceLimited,
        3,
    ));
    store.upsert_workflow(workflow_snapshot("workflow", WorkflowState::Running));

    let TranscriptEntry::Workflow { component } = &store.entries()[0] else {
        panic!("workflow card")
    };
    assert_eq!(component.snapshot().state, WorkflowState::ResourceLimited);
    assert_eq!(component.finalization(), Finalization::Finalized);
}

#[test]
fn corrupt_recovery_failure_overrides_sequenced_running_projection() {
    let mut store = TranscriptStore::new();
    store.upsert_workflow(workflow_snapshot_at("workflow", WorkflowState::Running, 4));
    let mut recovered = workflow_snapshot("workflow", WorkflowState::Failed);
    recovered.recovery_failure = true;
    recovered.terminal_reason = Some("corrupt journal".to_owned());

    store.upsert_workflow(recovered.clone());
    let recovery_revision = store.entry_revisions()[0];
    store.upsert_workflow(workflow_snapshot("workflow", WorkflowState::Running));
    store.upsert_workflow(workflow_snapshot_at("workflow", WorkflowState::Running, 4));
    store.upsert_workflow(recovered);

    let TranscriptEntry::Workflow { component } = &store.entries()[0] else {
        panic!("workflow card")
    };
    assert_eq!(component.snapshot().state, WorkflowState::Failed);
    assert!(component.snapshot().recovery_failure);
    assert_eq!(component.finalization(), Finalization::Finalized);
    assert_eq!(store.entry_revisions()[0], recovery_revision);

    store.upsert_workflow(workflow_snapshot_at(
        "workflow",
        WorkflowState::Completed,
        5,
    ));
    let TranscriptEntry::Workflow { component } = &store.entries()[0] else {
        panic!("workflow card")
    };
    assert_eq!(component.snapshot().state, WorkflowState::Completed);
    assert!(!component.snapshot().recovery_failure);
}

#[test]
fn equal_workflow_projection_sequence_is_replay_idempotent() {
    let mut store = TranscriptStore::new();
    store.upsert_workflow(workflow_snapshot_at("workflow", WorkflowState::Running, 4));
    let revision = store.entry_revisions()[0];
    store.upsert_workflow(workflow_snapshot_at(
        "workflow",
        WorkflowState::Completed,
        3,
    ));
    store.upsert_workflow(workflow_snapshot_at(
        "workflow",
        WorkflowState::Completed,
        4,
    ));

    let TranscriptEntry::Workflow { component } = &store.entries()[0] else {
        panic!("workflow card")
    };
    assert_eq!(component.snapshot().state, WorkflowState::Running);
    assert_eq!(store.entry_revisions()[0], revision);
}

#[test]
fn only_running_workflow_keeps_animation_tick_alive() {
    for (state, expected) in [
        (WorkflowState::Running, true),
        (WorkflowState::Paused, false),
        (WorkflowState::Completed, false),
        (WorkflowState::Failed, false),
        (WorkflowState::Cancelled, false),
        (WorkflowState::ResourceLimited, false),
    ] {
        let mut store = TranscriptStore::new();
        store.upsert_workflow(workflow_snapshot("workflow", state));
        assert_eq!(store.has_live_entries(), expected, "{state:?}");
    }
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
fn resolved_approval_ignores_repeated_request() {
    let mut pane = TranscriptPane::new(80, 12);
    request_test_approval(&mut pane);
    pane.resolve_approval("approval-1", &approved_resolution());
    let revision = pane.transcript().entry_revisions()[0];

    request_test_approval(&mut pane);

    assert_eq!(
        pane.transcript().entry_finalization(0),
        Some(Finalization::Finalized)
    );
    assert_eq!(pane.transcript().entry_revisions()[0], revision);
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
fn transcript_store_uses_explicit_entry_names_and_tool_runs() {
    let mut store = TranscriptStore::new();

    store.push(TranscriptEntry::user_message("hello"));
    store.push(TranscriptEntry::assistant_message("world"));
    store.push(TranscriptEntry::status("ready"));
    store.push_tool_run("tool-1", "Bash", Some(r#"{"command":"pwd"}"#.to_owned()));

    assert!(matches!(
        store.entries()[0],
        TranscriptEntry::UserMessage { .. }
    ));
    assert!(matches!(
        store.entries()[1],
        TranscriptEntry::AssistantMessage { .. }
    ));
    assert!(matches!(store.entries()[2], TranscriptEntry::Status { .. }));
    assert!(matches!(
        store.entries()[3],
        TranscriptEntry::ToolRun { .. }
    ));
}

#[test]
fn thinking_finishes_in_place_without_creating_a_second_entry() {
    let mut store = TranscriptStore::new();

    store.start_thinking();
    store.append_thinking_delta("alpha\nbeta\ngamma");
    assert_eq!(store.entries().len(), 1);

    store.finish_thinking(false);
    let rows = plain_rows(&store);

    assert_eq!(store.entries().len(), 1);
    assert!(rows.iter().any(|row| row.contains("● alpha")));
    assert!(rows.iter().any(|row| row.contains("1 more lines")));
}

#[test]
fn completed_thinking_stays_finalized_when_adjacent_thinking_starts() {
    let mut store = TranscriptStore::new();

    store.start_thinking();
    store.append_thinking_delta("first");
    store.finish_thinking(false);
    let completed_id = store.entry_ids()[0];
    assert_eq!(store.entry_finalization(0), Some(Finalization::Finalized));

    // Adjacent thinking reopens the completed block so consecutive reasoning
    // events render as one card. The entry is no longer finalized.
    store.start_thinking();
    store.append_thinking_delta("second");

    assert_eq!(thinking_contents(&store), vec!["firstsecond"]);
    assert_eq!(store.entries().len(), 1);
    assert_eq!(store.entry_ids()[0], completed_id);
    assert_eq!(store.entry_finalization(0), Some(Finalization::Live));
}

#[test]
fn multi_part_unknown_thinking_wraps_as_one_display_stream() {
    let parts = vec![
        ThinkingPart::new("abc", None),
        ThinkingPart::new("defgh", None),
    ];

    let complete = TranscriptEntry::ThinkingBlock {
        parts: parts.clone(),
        kind: neo_ai::ThinkingKind::Unknown,
        phase: ThinkingPhase::Complete,
        expanded: false,
    };
    let complete_rows = complete
        .render(6, &TuiTheme::default())
        .into_iter()
        .map(|line| line.text().clone())
        .collect::<Vec<_>>();
    assert_eq!(complete_rows, vec!["● abcd", "   efgh"]);
    assert!(
        complete_rows
            .iter()
            .all(|line| !line.contains("ctrl+o to expand"))
    );

    let streaming = TranscriptEntry::ThinkingBlock {
        parts,
        kind: neo_ai::ThinkingKind::Unknown,
        phase: ThinkingPhase::Streaming,
        expanded: false,
    };
    let streaming_rows = streaming
        .render(6, &TuiTheme::default())
        .into_iter()
        .map(|line| line.text().clone())
        .collect::<Vec<_>>();
    assert_eq!(streaming_rows, vec!["⠋ thinking...", "  abcd", "  efgh"]);
}

#[test]
fn adjacent_summary_parts_keep_ids_and_compact_visible_projection() {
    let mut live = TranscriptPane::new(80, 20);

    for (id, text) in [("summary-1", "first"), ("summary-2", "second")] {
        live.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
            turn: 1,
            id: id.to_owned(),
            kind: neo_ai::ThinkingKind::Summary,
        });
        live.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
            turn: 1,
            text: text.to_owned(),
        });
        live.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
            turn: 1,
            signature: None,
            redacted: false,
        });
    }

    assert_eq!(live.transcript().entries().len(), 1);
    let TranscriptEntry::ThinkingBlock { parts, .. } = &live.transcript().entries()[0] else {
        panic!("expected one live thinking block");
    };
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].id.as_deref(), Some("summary-1"));
    assert_eq!(parts[0].text, "first");
    assert_eq!(parts[1].id.as_deref(), Some("summary-2"));
    assert_eq!(parts[1].text, "second");
    assert_eq!(
        live.transcript().entries()[0].thinking_content().as_deref(),
        Some("firstsecond")
    );

    let replayed_parts = vec![
        neo_agent_core::Content::thinking_with_kind_and_id(
            "first",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-1".into()),
        ),
        neo_agent_core::Content::thinking_with_kind_and_id(
            "second",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-2".into()),
        ),
    ];
    let mut replay = TranscriptPane::new(80, 20);
    replay.replay_assistant_content(&replayed_parts);

    assert_eq!(replay.transcript().entries().len(), 1);
    let TranscriptEntry::ThinkingBlock { parts, .. } = &replay.transcript().entries()[0] else {
        panic!("expected one replayed thinking block");
    };
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].id.as_deref(), Some("summary-1"));
    assert_eq!(parts[0].text, "first");
    assert_eq!(parts[1].id.as_deref(), Some("summary-2"));
    assert_eq!(parts[1].text, "second");
    assert_eq!(
        plain_rows(replay.transcript()),
        plain_rows(live.transcript())
    );
}

#[test]
fn empty_thinking_delta_does_not_create_an_entry() {
    let mut store = TranscriptStore::new();

    store.append_thinking_delta("");

    assert!(store.entries().is_empty());
}

#[test]
fn assistant_text_blocks_thinking_coalescing() {
    let mut store = TranscriptStore::new();

    store.start_thinking();
    store.append_thinking_delta("first");
    store.finish_thinking(false);
    store.append_assistant_delta("visible answer");
    store.finish_assistant();
    store.start_thinking();
    store.append_thinking_delta("second");
    store.finish_thinking(false);

    assert_eq!(thinking_contents(&store), vec!["first", "second"]);
    assert_eq!(store.entries().len(), 3);
}

#[test]
fn tool_runs_block_thinking_coalescing() {
    let mut store = TranscriptStore::new();

    store.start_thinking();
    store.append_thinking_delta("first");
    store.finish_thinking(false);
    store.push_tool_run("tool-1", "Bash", Some(r#"{"command":"pwd"}"#.to_owned()));
    store.start_thinking();
    store.append_thinking_delta("second");
    store.finish_thinking(false);

    assert_eq!(thinking_contents(&store), vec!["first", "second"]);
    assert_eq!(store.entries().len(), 3);
}

#[test]
fn replayed_empty_id_thinking_part_is_retained() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.replay_assistant_content(&[neo_agent_core::Content::thinking_with_kind_and_id(
        "",
        None,
        false,
        neo_ai::ThinkingKind::Summary,
        Some("empty-summary".into()),
    )]);

    let entries = pane.transcript().entries();
    assert_eq!(entries.len(), 1);
    let TranscriptEntry::ThinkingBlock { parts, .. } = &entries[0] else {
        panic!("expected one empty thinking block");
    };
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].id.as_deref(), Some("empty-summary"));
    assert!(parts[0].text.is_empty());

    let mut historical = TranscriptPane::new(80, 20);
    historical.replay_assistant_content(&[neo_agent_core::Content::thinking("", None, false)]);
    assert!(historical.transcript().entries().is_empty());
}

#[test]
fn live_and_replayed_redacted_thinking_keep_raw_text_and_render_parity() {
    let mut live = TranscriptPane::new(80, 20);
    live.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "redacted-thinking".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
    });
    live.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: true,
    });

    let TranscriptEntry::ThinkingBlock { parts, .. } = &live.transcript().entries()[0] else {
        panic!("expected one live thinking block");
    };
    assert_eq!(parts.len(), 1);
    assert!(parts[0].text.is_empty());
    assert!(parts[0].redacted);
    assert_eq!(
        live.transcript().entries()[0].thinking_content(),
        Some("[Reasoning redacted]".to_owned())
    );
    let live_rows = plain_rows(live.transcript());
    assert!(
        live_rows
            .iter()
            .any(|row| row.contains("[Reasoning redacted]"))
    );

    let mut replay = TranscriptPane::new(80, 20);
    replay.replay_assistant_content(&[neo_agent_core::Content::thinking_with_kind_and_id(
        "",
        Some("opaque-signature".into()),
        true,
        neo_ai::ThinkingKind::Unknown,
        Some("redacted-thinking".into()),
    )]);

    let TranscriptEntry::ThinkingBlock { parts, .. } = &replay.transcript().entries()[0] else {
        panic!("expected one replayed thinking block");
    };
    assert_eq!(parts.len(), 1);
    assert!(parts[0].text.is_empty());
    assert!(parts[0].redacted);
    assert_eq!(
        replay.transcript().entries()[0].thinking_content(),
        live.transcript().entries()[0].thinking_content()
    );
    assert_eq!(plain_rows(replay.transcript()), live_rows);
}

#[test]
fn summary_projection_is_global_across_ordered_parts() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.replay_assistant_content(&[
        neo_agent_core::Content::thinking_with_kind_and_id(
            "**Plan**\n**Cross",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-1".into()),
        ),
        neo_agent_core::Content::thinking_with_kind_and_id(
            " title**\n**Plan**\n**Latest**",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-2".into()),
        ),
    ]);

    let rendered = plain_rows(pane.transcript()).join("\n");
    assert!(rendered.contains("● Plan"), "rendered summary: {rendered}");
    assert!(
        rendered.contains("  Cross title"),
        "rendered summary: {rendered}"
    );
    assert!(
        rendered.contains("… 1 more lines (ctrl+o to expand)"),
        "rendered summary: {rendered}"
    );

    let mut streaming = TranscriptPane::new(80, 20);
    streaming.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "summary-1".to_owned(),
        kind: neo_ai::ThinkingKind::Summary,
    });
    streaming.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "**First**".to_owned(),
    });
    streaming.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    streaming.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "summary-2".to_owned(),
        kind: neo_ai::ThinkingKind::Summary,
    });
    streaming.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "**Latest**".to_owned(),
    });

    let rendered = plain_rows(streaming.transcript()).join("\n");
    assert!(
        rendered.contains("thinking · Latest"),
        "rendered summary: {rendered}"
    );
}

#[test]
fn summary_projection_deduplicates_unclosed_title_across_parts() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.replay_assistant_content(&[
        neo_agent_core::Content::thinking_with_kind_and_id(
            "**Plan**\n**Pla",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-1".into()),
        ),
        neo_agent_core::Content::thinking_with_kind_and_id(
            "n",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-2".into()),
        ),
    ]);

    let rendered = plain_rows(pane.transcript());
    let plan_rows = rendered
        .iter()
        .filter(|row| row.contains("Plan"))
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(plan_rows, vec!["● Plan"]);
}

#[test]
fn summary_projection_keeps_first_line_fallback_across_parts() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.replay_assistant_content(&[
        neo_agent_core::Content::thinking_with_kind_and_id(
            "first fallback\nbody",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-1".into()),
        ),
        neo_agent_core::Content::thinking_with_kind_and_id(
            "second fallback",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-2".into()),
        ),
    ]);

    let rendered = plain_rows(pane.transcript()).join("\n");
    assert!(
        rendered.contains("● first fallback"),
        "rendered summary: {rendered}"
    );
    assert!(
        !rendered.contains("more lines"),
        "fallback should remain one projected title: {rendered}"
    );
}

#[test]
fn retry_status_countdown_formats_long_delay() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 3_878_000,
        error_code: "provider.transport_error".to_owned(),
        message: "error decoding response body".to_owned(),
    });

    let rows = plain_rows(pane.transcript()).join("\n");
    assert!(
        rows.contains("Reconnecting 1/5 · retry in 1h 04m 38s · esc interrupt"),
        "long retry delay: {rows}"
    );
}

// ── Instruction epoch cards (path-scoped AGENTS.md instructions) ────────────

fn instruction_test_epoch(generation: u64, deferred_tool_ids: &[&str]) -> InstructionEpochData {
    let nested = std::path::PathBuf::from("/workspace/neo/crates/neo-tui");
    InstructionEpochData {
        agent_id: "main".to_owned(),
        generation,
        outcome: InstructionEpochOutcome::Activated,
        scopes: vec![InstructionScopeData {
            display_path: nested.clone(),
            kind: InstructionScopeKind::Nested,
            revision: Some("7af13c2e".to_owned()),
            token_estimate: 31_800,
        }],
        selected_bundles: vec![InstructionBundleMetadata {
            display_path: nested,
            revision: "7af13c2e".to_owned(),
            token_estimate: 31_800,
            byte_size: 127_200,
            source_count: 3,
            import_count: 2,
            import_paths: Vec::new(),
        }],
        ignored_bundles: Vec::new(),
        replacements: Vec::new(),
        failure: None,
        deferred_tool_ids: deferred_tool_ids
            .iter()
            .map(|id| (*id).to_owned())
            .collect(),
        budget: neo_agent_core::instructions::InstructionBudget {
            nominal: 65_536,
            actual: 65_536,
        },
        body_revisions: None,
        model_content: Some("scoped rules".to_owned()),
    }
}

fn instruction_order(store: &TranscriptStore) -> Vec<String> {
    store
        .entries()
        .iter()
        .map(|entry| match entry {
            TranscriptEntry::InstructionEpoch { component } => {
                format!("card:{}", component.id())
            }
            TranscriptEntry::ToolRun { component } => format!("tool:{}", component.id()),
            _ => "other".to_owned(),
        })
        .collect()
}

#[test]
fn instruction_epoch_replaces_deferred_placeholders_at_earliest_position() {
    let mut store = TranscriptStore::new();
    store.push_tool_run("read-1", "Read", None);
    store.push_tool_run("grep-1", "Grep", None);
    store.push_tool_run("bash-1", "Bash", None);

    // Deferred ids arrive in provider batch order, not transcript order; the
    // card must still land at the earliest placeholder's canonical position.
    let epoch = instruction_test_epoch(3, &["bash-1", "read-1", "grep-1"]);
    let card_id = store.insert_instruction_epoch(
        &epoch,
        std::path::PathBuf::from("/workspace/neo"),
        Some(std::path::PathBuf::from("/home/user")),
        false,
    );

    assert!(matches!(
        store.entries().first(),
        Some(TranscriptEntry::InstructionEpoch { .. })
    ));
    assert_eq!(store.entry_ids().first(), Some(&card_id));
    for id in ["read-1", "grep-1", "bash-1"] {
        assert!(
            store.is_tool_run_suppressed(id),
            "deferred placeholder {id} must be absorbed"
        );
    }
    assert_eq!(
        store.entries().len(),
        4,
        "placeholders are suppressed, never deleted"
    );

    // The model replans and re-issues the batch under fresh ids; the retried
    // tools append after the fixed card instead of displacing it.
    store.push_tool_run("read-2", "Read", None);
    store.push_tool_run("grep-2", "Grep", None);
    store.push_tool_run("bash-2", "Bash", None);

    assert_eq!(
        instruction_order(&store),
        [
            "card:instruction-epoch-main-3",
            "tool:read-1",
            "tool:grep-1",
            "tool:bash-1",
            "tool:read-2",
            "tool:grep-2",
            "tool:bash-2",
        ]
    );
    for id in ["read-2", "grep-2", "bash-2"] {
        assert!(
            !store.is_tool_run_suppressed(id),
            "retried tool {id} must stay visible"
        );
    }
    assert_eq!(
        store.entry_finalization(0),
        Some(Finalization::Finalized),
        "the instruction card is a finalized semantic entry"
    );
}

#[test]
fn delegate_terminal_history_keeps_latest_four_tools_after_activity_trimming() {
    let mut pane = TranscriptPane::new(100, 24);
    let mut running = agent_snapshot("delegate-a", AgentLifecycleState::Running);
    running.activity = vec![
        tool_activity("read-1", "Read", "one.rs", AgentToolActivityPhase::Done),
        tool_activity("bash-1", "Bash", "make", AgentToolActivityPhase::Failed),
        tool_activity("grep-1", "Grep", "pattern", AgentToolActivityPhase::Done),
        tool_activity("find-1", "Find", "src", AgentToolActivityPhase::Done),
        tool_activity("edit-1", "Edit", "two.rs", AgentToolActivityPhase::Done),
        tool_activity("write-1", "Write", "three.rs", AgentToolActivityPhase::Done),
    ];
    running.tool_count = 6;
    pane.transcript_mut().upsert_delegate(1, running);

    let update = pane.render_terminal_update(100, 24);
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
        history.is_empty(),
        "running facts entered history:\n{history}"
    );
    assert!(!live.contains("Used Read"), "live:\n{live}");
    assert!(!live.contains("Failed Bash"), "live:\n{live}");
    for tool in ["Used Grep", "Used Find", "Used Edit", "Used Write"] {
        assert!(live.contains(tool), "missing {tool}:\n{live}");
    }

    // A later snapshot trims the completed tools away. The captured facts
    // were taken at update time and must survive the trimming.
    let mut trimmed = agent_snapshot("delegate-a", AgentLifecycleState::Running);
    trimmed.activity = vec![tool_activity(
        "grep-1",
        "Grep",
        "pattern",
        AgentToolActivityPhase::Ongoing,
    )];
    pane.transcript_mut().upsert_delegate(1, trimmed);

    let after_trim = pane.render_terminal_update(100, 24);
    assert!(
        after_trim.history.is_empty(),
        "trimmed running card entered history"
    );

    let mut completed = agent_snapshot("delegate-a", AgentLifecycleState::Completed);
    completed.tool_count = 6;
    completed.outcome = Some(AgentTerminalOutcome {
        summary: "delegate result".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate(1, completed);
    let terminal = pane.render_terminal_update(100, 24);
    assert_eq!(terminal.history.len(), 1, "one complete parent card");
    let history = terminal
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    let header = history.find("delegate-a").expect("parent header");
    let grep = history.find("Used Grep").expect("retained Grep fact");
    let find = history.find("Used Find").expect("retained Find fact");
    let edit = history.find("Used Edit").expect("retained Edit fact");
    let write = history.find("Used Write").expect("retained Write fact");
    let result = history.find("delegate result").expect("terminal result");
    assert!(!history.contains("Used Read"), "history:\n{history}");
    assert!(!history.contains("Failed Bash"), "history:\n{history}");
    assert!(
        header < grep && grep < find && find < edit && edit < write && write < result,
        "history:\n{history}"
    );

    pane.acknowledge_history(&terminal.history);
    assert!(pane.render_terminal_update(100, 24).history.is_empty());
}

#[test]
fn delegate_to_group_replacement_preserves_progressive_fact_identity() {
    let mut pane = TranscriptPane::new(120, 24);
    let mut first = agent_snapshot("first-agent", AgentLifecycleState::Running);
    first.activity = vec![tool_activity(
        "read-1",
        "Read",
        "one.rs",
        AgentToolActivityPhase::Done,
    )];
    first.tool_count = 1;
    pane.transcript_mut().upsert_delegate(7, first.clone());

    let update = pane.render_terminal_update(120, 24);
    let history = update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        history.is_empty(),
        "running fact entered history:\n{history}"
    );

    // A second root delegate replaces the card with a DelegateGroup in place;
    // the entry identity (and therefore the fact identity) is preserved.
    let second = agent_snapshot("second-agent", AgentLifecycleState::Running);
    pane.transcript_mut().upsert_delegate(7, second.clone());
    assert!(matches!(
        pane.transcript().entries()[0],
        TranscriptEntry::DelegateGroup { .. }
    ));

    let running = pane.render_terminal_update(120, 24);
    assert!(running.history.is_empty());
    let live = running
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(live.contains("first-agent"), "live:\n{live}");
    assert!(live.contains("second-agent"), "live:\n{live}");

    let mut second_completed = second;
    second_completed.state = AgentLifecycleState::Completed;
    second_completed.terminal_at_ms = Some(3);
    second_completed.updated_at_ms = 3;
    pane.transcript_mut()
        .upsert_delegate(7, second_completed.clone());
    second_completed.updated_at_ms = 4;
    second_completed.outcome = Some(AgentTerminalOutcome {
        summary: "second-agent result".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate(7, second_completed);

    let mut first_completed = first;
    first_completed.state = AgentLifecycleState::Completed;
    first_completed.terminal_at_ms = Some(5);
    first_completed.updated_at_ms = 5;
    first_completed.outcome = Some(AgentTerminalOutcome {
        summary: "first-agent result".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate(7, first_completed);
    let terminal = pane.render_terminal_update(120, 24);
    assert_eq!(terminal.history.len(), 1);
    let history = terminal.history[0]
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    let header = history.find("2 agents finished").expect("parent header");
    let first_agent = history.find("first-agent").expect("first child");
    let second_agent = history.find("second-agent").expect("second child");
    let tool = history.find("Used Read").expect("retained tool");
    let first_result = history.find("first-agent result").expect("first result");
    let second_result = history.find("second-agent result").expect("second result");
    assert!(
        header < first_agent
            && first_agent < tool
            && tool < first_result
            && first_result < second_agent
            && second_agent < second_result,
        "history:\n{history}"
    );
}

#[test]
fn delegate_swarm_terminal_block_uses_one_row_per_child() {
    let mut pane = TranscriptPane::new(120, 24);
    let mut first = agent_snapshot("swarm-first", AgentLifecycleState::Running);
    let mut second = agent_snapshot("swarm-second", AgentLifecycleState::Running);
    second.activity = vec![tool_activity(
        "bash-1",
        "Bash",
        "cargo test",
        AgentToolActivityPhase::Done,
    )];
    second.tool_count = 1;
    pane.transcript_mut().upsert_delegate_swarm(swarm_snapshot(
        "swarm-a",
        vec![first.clone(), second.clone()],
    ));
    assert!(pane.render_terminal_update(120, 24).history.is_empty());

    second.activity.clear();
    pane.transcript_mut().upsert_delegate_swarm(swarm_snapshot(
        "swarm-a",
        vec![first.clone(), second.clone()],
    ));
    second.state = AgentLifecycleState::Completed;
    second.terminal_at_ms = Some(3);
    second.updated_at_ms = 3;
    pane.transcript_mut().upsert_delegate_swarm(swarm_snapshot(
        "swarm-a",
        vec![first.clone(), second.clone()],
    ));
    second.updated_at_ms = 4;
    second.outcome = Some(AgentTerminalOutcome {
        summary: "swarm second result".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate_swarm(swarm_snapshot(
        "swarm-a",
        vec![first.clone(), second.clone()],
    ));
    first.state = AgentLifecycleState::Completed;
    first.terminal_at_ms = Some(5);
    first.updated_at_ms = 5;
    first.outcome = Some(AgentTerminalOutcome {
        summary: "swarm first result".to_owned(),
        is_error: false,
    });
    pane.transcript_mut()
        .upsert_delegate_swarm(swarm_snapshot("swarm-a", vec![first, second]));

    let terminal = pane.render_terminal_update(120, 24);
    assert_eq!(terminal.history.len(), 1);
    let history = terminal.history[0]
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>();
    assert_eq!(history.len(), 3, "history:\n{}", history.join("\n"));
    assert!(history[0].contains("DelegateSwarm"), "{history:#?}");
    assert!(history[1].contains("swarm-first"), "{history:#?}");
    assert!(history[1].contains("swarm first result"), "{history:#?}");
    assert!(history[2].contains("swarm-second"), "{history:#?}");
    assert!(history[2].contains("swarm second result"), "{history:#?}");
    assert!(!history.iter().any(|line| line.contains("Used Bash")));
}
