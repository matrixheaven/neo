use std::time::Duration;

use neo_agent_core::instructions::{
    InstructionBundleMetadata, InstructionEpochData, InstructionEpochOutcome, InstructionScopeData,
    InstructionScopeKind,
};
use neo_agent_core::multi_agent::{
    AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
    AgentSnapshot, DelegateContext, SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_agent_core::workflow::{WorkflowId, WorkflowSnapshot, WorkflowState};
use neo_agent_core::{
    ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
    PermissionOperation,
};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Finalization, strip_ansi};
use neo_tui::transcript::{ShellRunComponent, TranscriptEntry, TranscriptPane, TranscriptStore};

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
        steps: Vec::new(),
    }
}

fn finish_test_tool(pane: &mut TranscriptPane) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("done"),
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

fn thinking_contents(store: &TranscriptStore) -> Vec<&str> {
    store
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::ThinkingBlock { content, .. } => Some(content.as_str()),
            _ => None,
        })
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
fn queued_message_stays_live_until_removed() {
    let mut store = TranscriptStore::new();
    store.push(TranscriptEntry::queued_message("follow up", false));

    assert_eq!(store.entry_finalization(0), Some(Finalization::Live));

    assert!(store.remove(0).is_some());
    assert_eq!(store.entry_finalization(0), None);
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
            Some(r#"{"files":[{"path":"notes.txt","content":"hello"}]}"#.to_owned()),
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
        store.push(TranscriptEntry::queued_message("follow up", false));
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

    store.finish_thinking();
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
    store.finish_thinking();
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
    store.finish_thinking();
    store.append_assistant_delta("visible answer");
    store.finish_assistant();
    store.start_thinking();
    store.append_thinking_delta("second");
    store.finish_thinking();

    assert_eq!(thinking_contents(&store), vec!["first", "second"]);
    assert_eq!(store.entries().len(), 3);
}

#[test]
fn tool_runs_block_thinking_coalescing() {
    let mut store = TranscriptStore::new();

    store.start_thinking();
    store.append_thinking_delta("first");
    store.finish_thinking();
    store.push_tool_run("tool-1", "Bash", Some(r#"{"command":"pwd"}"#.to_owned()));
    store.start_thinking();
    store.append_thinking_delta("second");
    store.finish_thinking();

    assert_eq!(thinking_contents(&store), vec!["first", "second"]);
    assert_eq!(store.entries().len(), 3);
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
