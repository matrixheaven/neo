use neo_agent_core::multi_agent::{
    AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
    AgentSnapshot, DelegateContext, SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter};
use neo_agent_core::workflow::{
    WorkflowExecutionOrigin, WorkflowId, WorkflowSnapshot, WorkflowState,
};
use neo_agent_core::{
    AgentEvent, QuestionEventData, QuestionOptionData, ShellCommandOrigin, ShellCommandOutcome,
    ToolResult,
};
use neo_tui::dialogs::{QuestionDisplayData, QuestionDisplayOption};
use neo_tui::shell::StreamUpdate;
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
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::ZERO,
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
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
fn replayed_reference() -> neo_agent_core::session::ToolOutputRef {
    neo_agent_core::session::ToolOutputRef {
        agent_id: "main".to_owned(),
        task_id: "bash-replay-artifact".to_owned(),
        byte_len: 8192,
        line_count: 24,
        complete: true,
    }
}

#[tokio::test]
async fn jsonl_replay_preserves_workflow_question_tool_and_child_grouping() {
    let delegate_origin = origin("wf-test", "delegate-replay-call");
    let swarm_origin = origin("wf-test", "swarm-replay-call");
    let bash_origin = origin("wf-test", "bash-replay-call");
    let question_origin = origin("wf-test", "question-replay-call");
    let delegate = agent_snapshot("delegate-replay");
    let swarm = swarm_snapshot("swarm-replay", agent_snapshot("swarm-child-replay"));
    let events = vec![
        AgentEvent::WorkflowStarted {
            turn: 1,
            workflow: snapshot(WorkflowState::Running),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "bash-replay-call".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({"command": "printf replay"}),
            workflow_origin: Some(bash_origin.clone()),
            output_ref: Some(replayed_reference()),
        },
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "bash-replay-call".to_owned(),
            name: "Bash".to_owned(),
            result: tool_result("replayed", false),
            workflow_origin: Some(bash_origin),
            output_ref: Some(replayed_reference()),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "delegate-replay-call".to_owned(),
            name: "Delegate".to_owned(),
            arguments: serde_json::json!({"task": "delegate replay"}),
            workflow_origin: Some(delegate_origin.clone()),
            output_ref: None,
        },
        AgentEvent::DelegateStarted {
            turn: 1,
            agent: delegate,
            workflow_origin: Some(delegate_origin),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "swarm-replay-call".to_owned(),
            name: "DelegateSwarm".to_owned(),
            arguments: serde_json::json!({"tasks": ["swarm replay"]}),
            workflow_origin: Some(swarm_origin.clone()),
            output_ref: None,
        },
        AgentEvent::DelegateSwarmStarted {
            turn: 1,
            swarm,
            workflow_origin: Some(swarm_origin),
        },
        AgentEvent::QuestionRequested {
            turn: 1,
            id: "question-replay".to_owned(),
            questions: vec![QuestionEventData {
                question: "Continue replay?".to_owned(),
                header: None,
                body: None,
                options: vec![QuestionOptionData {
                    label: "Continue".to_owned(),
                    description: None,
                }],
                multi_select: false,
            }],
            workflow_origin: Some(question_origin.clone()),
        },
    ];

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.jsonl");
    let mut writer = JsonlSessionWriter::create(&path).await.expect("writer");
    for event in &events {
        writer.append(event).await.expect("append event");
    }
    writer.flush().await.expect("flush");
    let replayed = JsonlSessionReader::read_all(&path)
        .await
        .expect("read events");

    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    for event in replayed {
        match event {
            AgentEvent::QuestionRequested {
                id,
                questions,
                workflow_origin,
                ..
            } => {
                let questions = questions
                    .into_iter()
                    .map(|question| QuestionDisplayData {
                        question: question.question,
                        header: question.header,
                        body: question.body,
                        options: question
                            .options
                            .into_iter()
                            .map(|option| QuestionDisplayOption {
                                label: option.label,
                                description: option.description,
                            })
                            .collect(),
                        multi_select: question.multi_select,
                    })
                    .collect();
                pane.apply_question_stream_update(StreamUpdate::QuestionRequested {
                    id,
                    questions,
                    workflow_origin,
                });
            }
            event => pane.apply_agent_event(event),
        }
    }

    let workflow = pane
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::Workflow { component } => Some(component),
            _ => None,
        })
        .expect("workflow entry");
    assert_eq!(workflow.direct_tools().len(), 1);
    assert_eq!(workflow.direct_tools()[0].id(), "bash-replay-call");
    assert_eq!(
        workflow.direct_tools()[0].output_ref(),
        Some(&replayed_reference()),
        "the typed reference must rehydrate onto the Workflow direct tool from JSONL"
    );
    assert_eq!(
        workflow.delegates()[0].display_name.as_str(),
        "delegate-replay"
    );
    assert_eq!(workflow.swarms()[0].swarm_id, "swarm-replay");
    assert!(!pane.transcript().entries().iter().any(|entry| matches!(
        entry,
        TranscriptEntry::Delegate { .. } | TranscriptEntry::DelegateSwarm { .. }
    )));
    assert!(pane.transcript().entries().iter().any(|entry| matches!(
        entry,
        TranscriptEntry::QuestionPrompt(data)
            if data.workflow_origin.as_ref() == Some(&question_origin)
    )));
}

#[test]
fn orphan_model_shell_events_do_not_create_top_level_tools() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "orphan-shell".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf orphan"}),
        workflow_origin: Some(origin("missing-workflow", "orphan-shell")),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "orphan-shell".to_owned(),
        command: "printf orphan".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::ModelBashTool,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "orphan-shell".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "orphan".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome: ShellCommandOutcome::Completed,
        output_ref: None,
    });

    assert_eq!(pane.transcript().entries().len(), 1);
    assert!(matches!(
        &pane.transcript().entries()[0],
        TranscriptEntry::Status { text, .. }
            if text == "Workflow activity could not be displayed because the workflow has not started."
    ));
    assert!(!pane.transcript().entries().iter().any(
        |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == "orphan-shell")
    ));
}
