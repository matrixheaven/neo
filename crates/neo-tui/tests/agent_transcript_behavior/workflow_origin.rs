use neo_agent_core::multi_agent::{
    AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentProgressSnapshot, AgentRole,
    AgentRunMode, AgentSnapshot, DelegateContext, SwarmAggregate, SwarmChildProgress,
    SwarmChildSnapshot, SwarmSnapshot,
};
use neo_agent_core::workflow::{
    WorkflowExecutionOrigin, WorkflowId, WorkflowSnapshot, WorkflowState,
};
use neo_agent_core::{AgentEvent, ShellCommandOrigin, ShellCommandOutcome, ToolResult};
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::TranscriptEntry;
use std::time::Duration;

fn snapshot(state: WorkflowState) -> WorkflowSnapshot {
    WorkflowSnapshot {
        id: WorkflowId("wf-test".to_owned()),
        title: "Runtime audit and fix".to_owned(),
        state,
        current_phase: Some("verify".to_owned()),
        projection_sequence: Some(7),
        recovery_failure: false,
        started_at_ms: Some(1_000),
        updated_at_ms: Some(6_000),
        invocation_count: 3,
        failure_count: 1,
        actual_usage: Some(neo_agent_core::AgentTokenUsage {
            input_tokens: 20,
            output_tokens: 5,
            input_cache_read_tokens: 10,
            input_cache_write_tokens: 0,
        }),
        latest_log_summary: Some("focused verification running".to_owned()),
        latest_report_summary: Some("all scoped checks passed".to_owned()),
        terminal_reason: state
            .is_terminal()
            .then(|| "workflow reached its durable boundary".to_owned()),
        display_name: "Runtime audit and fix".to_owned(),
        purpose: "Verify runtime correctness".to_owned(),
    }
}
fn origin(run_id: &str, invocation_id: &str) -> WorkflowExecutionOrigin {
    WorkflowExecutionOrigin {
        run_id: WorkflowId(run_id.to_owned()),
        human_handle: None,
        definition_name: "test-workflow".to_owned(),
        definition_revision: None,
        phase_id: Some("verify".to_owned()),
        invocation_id: Some(invocation_id.to_owned()),
        swarm_item_id: None,
    }
}
fn agent_snapshot(id: &str) -> AgentSnapshot {
    let display_name = AgentDisplayName::new(id);
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id),
        display_name: display_name.clone(),
        path: AgentPath::root_child(&display_name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: "verify workflow".to_owned(),
        task_title: "verify workflow".to_owned(),
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
fn agent_progress(agent: &AgentSnapshot, state: AgentLifecycleState) -> AgentProgressSnapshot {
    AgentProgressSnapshot {
        agent_id: agent.id.clone(),
        state,
        mode: agent.mode,
        detached_from_foreground: false,
        started_at_ms: agent.started_at_ms,
        updated_at_ms: 3,
        terminal_at_ms: state.is_terminal().then_some(3),
        terminal_reason: None,
        run_count: agent.run_count,
        live_messages_received: 1,
        tool_count: 0,
        token_count: 0,
        input_token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed_ms: 2,
        latest_text: Some("progress merged".to_owned()),
        latest_thinking: None,
        last_tool: None,
        last_instruction: None,
        outcome: None,
    }
}
fn swarm_snapshot(id: &str, agent: AgentSnapshot) -> SwarmSnapshot {
    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "verify item".to_owned(),
        agent,
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    SwarmSnapshot {
        swarm_id: id.to_owned(),
        description: "workflow swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 1,
        aggregate,
        children,
    }
}
fn tool_result(content: &str, is_error: bool) -> ToolResult {
    ToolResult {
        content: content.to_owned(),
        is_error,
        details: None,
        terminate: false,
    }
}

#[test]
fn workflow_origin_conflict_is_one_terminal_error_and_blocks_id_only_updates() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });
    let original_origin = origin("wf-test", "original-invocation");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "shared-tool".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf original"}),
        workflow_origin: Some(original_origin.clone()),
        output_ref: None,
    });
    let conflicting_origin = origin("wf-test", "conflicting-invocation");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "shared-tool".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf conflicting"}),
        workflow_origin: Some(conflicting_origin.clone()),
        output_ref: None,
    });

    let workflow_index = pane
        .transcript()
        .entries()
        .iter()
        .position(|entry| matches!(entry, TranscriptEntry::Workflow { .. }))
        .expect("workflow entry");
    let revision_after_error = pane.transcript().entry_revisions()[workflow_index];
    let TranscriptEntry::Workflow { component } = &pane.transcript().entries()[workflow_index]
    else {
        panic!("workflow entry")
    };
    let tool = &component.direct_tools()[0];
    let route_error = tool.result().expect("route error").to_owned();
    assert_eq!(tool.status(), ToolStatusKind::Failed);
    assert_eq!(tool.workflow_origin(), Some(&original_origin));
    assert!(route_error.len() < 100, "{route_error}");
    assert!(!route_error.contains("wf-test"), "{route_error}");
    assert!(
        !route_error.contains("original-invocation"),
        "{route_error}"
    );
    assert!(
        !route_error.contains("conflicting-invocation"),
        "{route_error}"
    );

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "shared-tool".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf conflicting"}),
        workflow_origin: Some(conflicting_origin),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: "shared-tool".to_owned(),
        position: 1,
        waiting_ms: 50,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shared-tool".to_owned(),
        command: "printf later".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::ModelBashTool,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "shared-tool".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "later".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome: ShellCommandOutcome::Completed,
        output_ref: None,
    });

    assert_eq!(
        pane.transcript().entry_revisions()[workflow_index],
        revision_after_error
    );
    let TranscriptEntry::Workflow { component } = &pane.transcript().entries()[workflow_index]
    else {
        panic!("workflow entry")
    };
    assert_eq!(component.direct_tools().len(), 1);
    let tool = &component.direct_tools()[0];
    assert_eq!(tool.status(), ToolStatusKind::Failed);
    assert_eq!(tool.result(), Some(route_error.as_str()));
    assert_eq!(tool.workflow_origin(), Some(&original_origin));
    assert!(!pane.transcript().entries().iter().any(
        |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == "shared-tool")
    ));
}

#[test]
fn workflow_origin_routes_tools_and_children_into_one_entry() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });

    let read_origin = origin("wf-test", "read-call");
    pane.apply_agent_event(AgentEvent::ToolExecutionQueued {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "README.md"}),
        workflow_origin: Some(read_origin.clone()),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: "read-call".to_owned(),
        position: 2,
        waiting_ms: 10,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "README.md"}),
        workflow_origin: Some(read_origin.clone()),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        partial_result: tool_result("partial", false),
        workflow_origin: Some(read_origin.clone()),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        result: tool_result("done", false),
        workflow_origin: Some(read_origin.clone()),
        output_ref: None,
    });

    let bash_origin = origin("wf-test", "bash-call");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-call".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf ok"}),
        workflow_origin: Some(bash_origin),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "bash-call".to_owned(),
        command: "printf ok".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::ModelBashTool,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "bash-call".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "ok".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome: ShellCommandOutcome::Completed,
        output_ref: None,
    });

    let delegate_origin = origin("wf-test", "delegate-call");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "delegate-call".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "verify"}),
        workflow_origin: Some(delegate_origin.clone()),
        output_ref: None,
    });
    let delegate = agent_snapshot("delegate");
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 1,
        agent: delegate.clone(),
        workflow_origin: Some(delegate_origin.clone()),
    });
    pane.apply_agent_event(AgentEvent::DelegateProgressUpdated {
        turn: 1,
        progress: agent_progress(&delegate, AgentLifecycleState::Running),
        workflow_origin: Some(delegate_origin.clone()),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "delegate-call".to_owned(),
        name: "Delegate".to_owned(),
        result: tool_result("delegated", false),
        workflow_origin: Some(delegate_origin),
        output_ref: None,
    });

    let swarm_origin = origin("wf-test", "swarm-call");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "swarm-call".to_owned(),
        name: "DelegateSwarm".to_owned(),
        arguments: serde_json::json!({"tasks": ["verify"]}),
        workflow_origin: Some(swarm_origin.clone()),
        output_ref: None,
    });
    let swarm_agent = agent_snapshot("swarm-child");
    let swarm = swarm_snapshot("swarm", swarm_agent.clone());
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 1,
        swarm: swarm.clone(),
        workflow_origin: Some(swarm_origin.clone()),
    });
    let child_progress = SwarmChildProgress {
        item_index: 0,
        progress: agent_progress(&swarm_agent, AgentLifecycleState::Completed),
    };
    let aggregate = SwarmAggregate::from_states([AgentLifecycleState::Completed]);
    pane.apply_agent_event(AgentEvent::DelegateSwarmProgressUpdated {
        turn: 1,
        swarm_id: swarm.swarm_id.clone(),
        state: AgentLifecycleState::Completed,
        aggregate,
        child_progress,
        workflow_origin: Some(swarm_origin.clone()),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "swarm-call".to_owned(),
        name: "DelegateSwarm".to_owned(),
        result: tool_result("swarmed", false),
        workflow_origin: Some(swarm_origin),
        output_ref: None,
    });

    let failed_origin = origin("wf-test", "failed-delegate");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "failed-delegate".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "never started"}),
        workflow_origin: Some(failed_origin.clone()),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "failed-delegate".to_owned(),
        name: "Delegate".to_owned(),
        result: tool_result("failed before child start", true),
        workflow_origin: Some(failed_origin),
        output_ref: None,
    });

    let workflow = pane
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::Workflow { component } => Some(component),
            _ => None,
        })
        .expect("workflow entry");
    assert_eq!(workflow.direct_tools().len(), 3);
    assert_eq!(workflow.direct_tools()[0].id(), "read-call");
    assert_eq!(
        workflow.direct_tools()[0].status(),
        ToolStatusKind::Succeeded
    );
    assert_eq!(
        workflow.direct_tools()[0].workflow_origin(),
        Some(&read_origin)
    );
    assert_eq!(workflow.direct_tools()[1].id(), "bash-call");
    assert_eq!(
        workflow.direct_tools()[1].status(),
        ToolStatusKind::Succeeded
    );
    assert_eq!(workflow.direct_tools()[2].id(), "failed-delegate");
    assert_eq!(workflow.direct_tools()[2].status(), ToolStatusKind::Failed);
    assert_eq!(workflow.delegates().len(), 1);
    assert_eq!(
        workflow.delegates()[0].latest_text.as_deref(),
        Some("progress merged")
    );
    assert_eq!(workflow.swarms().len(), 1);
    assert_eq!(
        workflow.swarms()[0].children[0].agent.state,
        AgentLifecycleState::Completed
    );
    assert!(!pane.transcript().entries().iter().any(|entry| matches!(
        entry,
        TranscriptEntry::Delegate { .. } | TranscriptEntry::DelegateSwarm { .. }
    )));

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "other"}),
        workflow_origin: Some(origin("other-run", "other-invocation")),
        output_ref: None,
    });
    let workflow = pane
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::Workflow { component } => Some(component),
            _ => None,
        })
        .expect("workflow entry");
    assert_eq!(
        workflow.direct_tools()[0].workflow_origin(),
        Some(&read_origin)
    );

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "ordinary-call".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "ordinary"}),
        workflow_origin: None,
        output_ref: None,
    });
    assert!(pane.transcript().entries().iter().any(
        |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == "ordinary-call")
    ));

    let mut missing = neo_tui::transcript::TranscriptPane::new(120, 24);
    let missing_origin = origin("internal-run-id", "internal-invocation-id");
    for id in ["orphan-one", "orphan-two"] {
        missing.apply_agent_event(AgentEvent::ToolExecutionQueued {
            turn: 1,
            id: id.to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({"path": "missing"}),
            workflow_origin: Some(missing_origin.clone()),
        });
    }
    assert_eq!(missing.transcript().entries().len(), 1);
    let TranscriptEntry::Status { text, .. } = &missing.transcript().entries()[0] else {
        panic!("bounded presentation error")
    };
    assert!(!text.contains("internal-run-id"), "{text}");
    assert!(!text.contains("internal-invocation-id"), "{text}");
}
