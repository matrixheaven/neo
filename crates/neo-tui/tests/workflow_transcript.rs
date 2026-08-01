use std::time::Duration;

use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentProgressSnapshot, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome,
    AgentToolActivityPhase, DelegateContext, SwarmAggregate, SwarmChildProgress,
    SwarmChildSnapshot, SwarmSnapshot,
};
use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter};
use neo_agent_core::workflow::{
    WorkflowExecutionOrigin, WorkflowId, WorkflowSnapshot, WorkflowState,
};
use neo_agent_core::{
    AgentEvent, ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest,
    ApprovalResolution, PermissionOperation, QuestionEventData, QuestionOptionData,
    ShellCommandOrigin, ShellCommandOutcome, ToolResult,
};
use neo_tui::dialogs::{QuestionDisplayData, QuestionDisplayOption};
use neo_tui::primitive::{Component, Finalization, Line, strip_ansi, visible_width};
use neo_tui::shell::{StreamUpdate, ToolStatusKind};
use neo_tui::transcript::{BlockingEntryKind, TranscriptEntry, WorkflowCardComponent};

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

fn text(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|line| strip_ansi(&line.to_ansi()))
        .collect::<Vec<_>>()
        .join("\n")
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
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed_ms: 2,
        latest_text: Some("progress merged".to_owned()),
        last_tool: None,
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

fn terminal_text(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn workflow_tool_started(
    pane: &mut neo_tui::transcript::TranscriptPane,
    id: &str,
    name: &str,
    arguments: serde_json::Value,
) -> WorkflowExecutionOrigin {
    let tool_origin = origin("wf-test", id);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: id.to_owned(),
        name: name.to_owned(),
        arguments,
        workflow_origin: Some(tool_origin.clone()),
    });
    tool_origin
}

fn workflow_delegate_started(
    pane: &mut neo_tui::transcript::TranscriptPane,
    invocation_id: &str,
    agent: AgentSnapshot,
) {
    let tool_origin = workflow_tool_started(
        pane,
        invocation_id,
        "Delegate",
        serde_json::json!({"task": agent.task.clone()}),
    );
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 1,
        agent,
        workflow_origin: Some(tool_origin),
    });
}

fn workflow_swarm_started(
    pane: &mut neo_tui::transcript::TranscriptPane,
    invocation_id: &str,
    swarm: SwarmSnapshot,
) {
    let tool_origin = workflow_tool_started(
        pane,
        invocation_id,
        "DelegateSwarm",
        serde_json::json!({"description": swarm.description.clone()}),
    );
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 1,
        swarm,
        workflow_origin: Some(tool_origin),
    });
}

#[test]
fn workflow_card_projects_runtime_summary() {
    let rendered = text(&WorkflowCardComponent::new(snapshot(WorkflowState::Running)).render(120));

    assert!(rendered.contains("Runtime audit and fix"), "{rendered}");
    assert!(rendered.contains("phase verify"), "{rendered}");
    assert!(rendered.contains("3 invocations"), "{rendered}");
    assert!(rendered.contains("25 tokens"), "{rendered}");
    assert!(
        rendered.contains("focused verification running"),
        "{rendered}"
    );
    assert!(rendered.contains("TaskPause · TaskStop"), "{rendered}");
    assert!(
        rendered.find("phase verify") < rendered.find("TaskPause · TaskStop"),
        "workflow context must precede controls:\n{rendered}"
    );
}

#[test]
fn workflow_card_renders_paused_resource_limited_and_terminal_states() {
    for (state, label, controls) in [
        (
            WorkflowState::Paused,
            "paused",
            Some("TaskResume · TaskStop"),
        ),
        (WorkflowState::Completed, "completed", None),
        (WorkflowState::Failed, "failed", None),
        (WorkflowState::Cancelled, "cancelled", None),
        (WorkflowState::ResourceLimited, "resource limited", None),
    ] {
        let mut card = WorkflowCardComponent::new(snapshot(state));
        let rendered = text(&card.render_with_theme(120, &Default::default()));
        assert!(rendered.contains(label), "{state:?}: {rendered}");
        let expected_finalization = if state == WorkflowState::Paused {
            Finalization::Live
        } else {
            Finalization::Finalized
        };
        assert_eq!(card.finalization(), expected_finalization);
        assert!(!card.on_render_tick(10_000), "{state:?} elapsed is frozen");
        match controls {
            Some(controls) => assert!(rendered.contains(controls), "{rendered}"),
            None => assert!(!rendered.contains("Controls"), "{rendered}"),
        }
    }
    let mut running = WorkflowCardComponent::new(snapshot(WorkflowState::Running));
    assert!(running.on_render_tick(10_000));
    assert!(text(&running.render(120)).contains("9s"));
}

#[test]
fn workflow_main_card_bounds_direct_tools_and_long_content() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(48, 10);
    let mut workflow = snapshot(WorkflowState::Running);
    workflow.title = "宽字符工作流标题 with a deliberately long suffix".to_owned();
    workflow.latest_report_summary =
        Some("报告包含很长的路径 /tmp/a/very/long/path/that/must/not/overflow".to_owned());
    pane.apply_agent_event(AgentEvent::WorkflowStarted { turn: 1, workflow });

    workflow_tool_started(
        &mut pane,
        "running-bash",
        "Bash",
        serde_json::json!({
            "command": "cargo test --package neo-tui --test workflow_transcript a_very_long_test_name -- --exact --nocapture"
        }),
    );
    let failed_origin = workflow_tool_started(
        &mut pane,
        "failed-edit",
        "Edit",
        serde_json::json!({"path": "/tmp/a/very/long/path/failed.rs"}),
    );
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "failed-edit".to_owned(),
        name: "Edit".to_owned(),
        result: tool_result("FULL_TOOL_OUTPUT must stay out of the normal card", true),
        workflow_origin: Some(failed_origin),
    });
    for index in 0..6 {
        let id = format!("completed-read-{index}");
        let read_origin = workflow_tool_started(
            &mut pane,
            &id,
            "Read",
            serde_json::json!({
                "path": format!("/tmp/a/very/long/path/{index}/宽字符-report.md")
            }),
        );
        pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id,
            name: "Read".to_owned(),
            result: tool_result("FULL_TOOL_OUTPUT must stay out of the normal card", false),
            workflow_origin: Some(read_origin),
        });
    }

    let update = pane.render_terminal_update(48, 10);
    let live = terminal_text(&update.live);
    assert!(update.live.len() <= 6, "live rows:\n{live}");
    assert!(
        update.live.iter().all(|line| visible_width(line) <= 48),
        "live width:\n{live}"
    );
    assert!(live.contains("Bash"), "active tool missing:\n{live}");
    assert!(live.contains("Report"), "latest report missing:\n{live}");
    assert!(
        live.contains("direct tools omitted"),
        "omitted count:\n{live}"
    );
    assert!(live.contains('…'), "long content must be explicit:\n{live}");
    assert!(
        !live.contains("FULL_TOOL_OUTPUT"),
        "raw output leaked:\n{live}"
    );
}

#[test]
fn workflow_child_summaries_use_two_sibling_cards_and_one_row_per_agent() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 30);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });

    let mut euclid = agent_snapshot("delegate-euclid");
    euclid.display_name = AgentDisplayName::new("Euclid");
    euclid.role = AgentRole::Explorer;
    euclid.elapsed = Duration::from_secs(4);
    euclid.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "read-report".to_owned(),
            name: "Read".to_owned(),
            summary: Some("report.md".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
            files: Vec::new(),
        },
    });
    workflow_delegate_started(&mut pane, "delegate-euclid-call", euclid);

    let mut hypatia = agent_snapshot("delegate-hypatia");
    hypatia.display_name = AgentDisplayName::new("Hypatia");
    hypatia.role = AgentRole::Reviewer;
    hypatia.state = AgentLifecycleState::Completed;
    hypatia.terminal_at_ms = Some(4_000);
    hypatia.elapsed = Duration::from_secs(3);
    hypatia.outcome = Some(AgentTerminalOutcome {
        summary: "review completed".to_owned(),
        is_error: false,
    });
    workflow_delegate_started(&mut pane, "delegate-hypatia-call", hypatia);

    let mut alpha = agent_snapshot("swarm-alpha");
    alpha.display_name = AgentDisplayName::new("Alpha");
    alpha.role = AgentRole::Coder;
    alpha.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "cargo-test".to_owned(),
            name: "Bash".to_owned(),
            summary: Some("cargo test --package neo-tui".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
            files: Vec::new(),
        },
    });
    let mut beta = agent_snapshot("swarm-beta");
    beta.display_name = AgentDisplayName::new("Beta");
    beta.role = AgentRole::Reviewer;
    beta.state = AgentLifecycleState::Queued;
    beta.started_at_ms = None;
    let children = vec![
        SwarmChildSnapshot {
            item_index: 1,
            item: "beta item".to_owned(),
            agent: beta,
        },
        SwarmChildSnapshot {
            item_index: 0,
            item: "alpha item".to_owned(),
            agent: alpha,
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    workflow_swarm_started(
        &mut pane,
        "swarm-call",
        SwarmSnapshot {
            swarm_id: "swarm-security-review".to_owned(),
            description: "security review".to_owned(),
            role: AgentRole::Reviewer,
            mode: AgentRunMode::Foreground,
            state: aggregate.status(),
            max_concurrency: 2,
            aggregate,
            children,
        },
    );

    let update = pane.render_terminal_update(120, 30);
    let live = terminal_text(&update.live);
    let main = live
        .find("Workflow  Runtime audit and fix")
        .expect("main card");
    let delegates = live.find("Workflow Delegates").expect("delegate summary");
    let swarms = live.find("Workflow Swarms").expect("swarm summary");
    assert!(
        main < delegates && delegates < swarms,
        "sibling order:\n{live}"
    );
    for name in ["Euclid", "Hypatia", "Alpha", "Beta"] {
        assert_eq!(live.matches(name).count(), 1, "one row for {name}:\n{live}");
    }
    assert!(
        live.find("Alpha") < live.find("Beta"),
        "swarm rows must stay in item order:\n{live}"
    );
    assert!(live.contains("[Explorer]"), "delegate role:\n{live}");
    assert!(live.contains("[Reviewer]"), "reviewer role:\n{live}");
    assert!(live.contains("security review"), "swarm label:\n{live}");
    assert!(!pane.transcript().entries().iter().any(|entry| matches!(
        entry,
        TranscriptEntry::Delegate { .. }
            | TranscriptEntry::DelegateGroup { .. }
            | TranscriptEntry::DelegateSwarm { .. }
    )));
}

#[test]
fn non_workflow_delegate_family_cards_remain_unchanged() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 30);
    for (turn, id) in [
        (1, "single-one"),
        (2, "single-two"),
        (3, "group-one"),
        (3, "group-two"),
    ] {
        pane.apply_agent_event(AgentEvent::DelegateStarted {
            turn,
            agent: agent_snapshot(id),
            workflow_origin: None,
        });
    }
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 4,
        swarm: swarm_snapshot("ordinary-swarm", agent_snapshot("ordinary-swarm-child")),
        workflow_origin: None,
    });

    let entries = pane.transcript().entries();
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::Delegate { .. }))
    );
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::DelegateGroup { .. }))
    );
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::DelegateSwarm { .. }))
    );
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::Workflow { .. }))
    );
}

#[test]
fn workflow_updates_stay_in_one_live_entry_without_transition_history() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);

    let mut running_a = snapshot(WorkflowState::Running);
    running_a.projection_sequence = Some(1);
    running_a.current_phase = Some("verify".to_owned());
    pane.transcript_mut().upsert_workflow(running_a);
    let update = pane.render_terminal_update(120, 24);
    let history = update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(history.is_empty(), "history:\n{history}");
    assert!(
        update.live.join("\n").contains("Runtime audit and fix"),
        "the mutable card stays live"
    );
    let mut running_b = snapshot(WorkflowState::Running);
    running_b.projection_sequence = Some(2);
    running_b.current_phase = Some("build".to_owned());
    pane.transcript_mut().upsert_workflow(running_b);
    let update = pane.render_terminal_update(120, 24);
    let history = update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(history.is_empty(), "history:\n{history}");
    let live = update.live.join("\n");
    assert!(live.contains("phase build"), "live:\n{live}");
    assert!(!live.contains("phase verify"), "live:\n{live}");

    let mut completed = snapshot(WorkflowState::Completed);
    completed.projection_sequence = Some(9);
    completed.updated_at_ms = Some(9_000);
    pane.transcript_mut().upsert_workflow(completed);
    let update = pane.render_terminal_update(120, 24);
    let history = update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(history.contains("completed"), "history:\n{history}");
    assert_eq!(
        history.matches("Workflow").count(),
        1,
        "one terminal status, no duplicate card:\n{history}"
    );
    assert!(update.live.is_empty());
    pane.acknowledge_history(&update.history);
    assert!(pane.render_terminal_update(120, 24).history.is_empty());
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
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        partial_result: tool_result("partial", false),
        workflow_origin: Some(read_origin.clone()),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        result: tool_result("done", false),
        workflow_origin: Some(read_origin.clone()),
    });

    let bash_origin = origin("wf-test", "bash-call");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-call".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf ok"}),
        workflow_origin: Some(bash_origin),
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
    });

    let delegate_origin = origin("wf-test", "delegate-call");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "delegate-call".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "verify"}),
        workflow_origin: Some(delegate_origin.clone()),
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
    });

    let swarm_origin = origin("wf-test", "swarm-call");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "swarm-call".to_owned(),
        name: "DelegateSwarm".to_owned(),
        arguments: serde_json::json!({"tasks": ["verify"]}),
        workflow_origin: Some(swarm_origin.clone()),
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
    });

    let failed_origin = origin("wf-test", "failed-delegate");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "failed-delegate".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "never started"}),
        workflow_origin: Some(failed_origin.clone()),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "failed-delegate".to_owned(),
        name: "Delegate".to_owned(),
        result: tool_result("failed before child start", true),
        workflow_origin: Some(failed_origin),
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
        },
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "bash-replay-call".to_owned(),
            name: "Bash".to_owned(),
            result: tool_result("replayed", false),
            workflow_origin: Some(bash_origin),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "delegate-replay-call".to_owned(),
            name: "Delegate".to_owned(),
            arguments: serde_json::json!({"task": "delegate replay"}),
            workflow_origin: Some(delegate_origin.clone()),
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
    });
    let conflicting_origin = origin("wf-test", "conflicting-invocation");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "shared-tool".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf conflicting"}),
        workflow_origin: Some(conflicting_origin.clone()),
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
fn orphan_model_shell_events_do_not_create_top_level_tools() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "orphan-shell".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf orphan"}),
        workflow_origin: Some(origin("missing-workflow", "orphan-shell")),
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

#[test]
fn finalized_workflow_tool_rejects_late_updates() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });
    let tool_origin = origin("wf-test", "terminal-tool");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "terminal-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "result"}),
        workflow_origin: Some(tool_origin.clone()),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "terminal-tool".to_owned(),
        name: "Read".to_owned(),
        result: tool_result("final", false),
        workflow_origin: Some(tool_origin.clone()),
    });
    let workflow_index = pane
        .transcript()
        .entries()
        .iter()
        .position(|entry| matches!(entry, TranscriptEntry::Workflow { .. }))
        .expect("workflow entry");
    let revision = pane.transcript().entry_revisions()[workflow_index];

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "terminal-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "late-start"}),
        workflow_origin: Some(tool_origin.clone()),
    });
    assert_finalized_workflow_tool(&pane, workflow_index, revision);

    pane.apply_agent_event(AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "terminal-tool".to_owned(),
        name: "Read".to_owned(),
        partial_result: tool_result("late", false),
        workflow_origin: Some(tool_origin.clone()),
    });
    assert_finalized_workflow_tool(&pane, workflow_index, revision);

    pane.apply_agent_event(AgentEvent::ToolExecutionQueued {
        turn: 1,
        id: "terminal-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "late-queue"}),
        workflow_origin: Some(tool_origin),
    });
    assert_finalized_workflow_tool(&pane, workflow_index, revision);

    pane.apply_agent_event(AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: "terminal-tool".to_owned(),
        position: 3,
        waiting_ms: 50,
    });
    assert_finalized_workflow_tool(&pane, workflow_index, revision);
}

#[test]
fn workflow_group_commits_once_at_terminal_event_position() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 30);
    let mut running = snapshot(WorkflowState::Running);
    running.projection_sequence = Some(1);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: running,
    });

    let mut delegate = agent_snapshot("terminal-delegate");
    delegate.display_name = AgentDisplayName::new("TerminalDelegate");
    delegate.state = AgentLifecycleState::Completed;
    delegate.terminal_at_ms = Some(3_000);
    delegate.outcome = Some(AgentTerminalOutcome {
        summary: "delegate done".to_owned(),
        is_error: false,
    });
    workflow_delegate_started(&mut pane, "terminal-delegate-call", delegate);
    let mut swarm_child = agent_snapshot("terminal-swarm-child");
    swarm_child.display_name = AgentDisplayName::new("TerminalSwarmChild");
    swarm_child.state = AgentLifecycleState::Completed;
    swarm_child.terminal_at_ms = Some(3_000);
    swarm_child.outcome = Some(AgentTerminalOutcome {
        summary: "swarm child done".to_owned(),
        is_error: false,
    });
    workflow_swarm_started(
        &mut pane,
        "terminal-swarm-call",
        swarm_snapshot("terminal-swarm", swarm_child),
    );

    assert!(pane.render_terminal_update(120, 30).history.is_empty());
    pane.push_status("unrelated finalized row");
    let unrelated = pane.render_terminal_update(120, 30);
    assert_eq!(unrelated.history.len(), 1);
    assert!(terminal_text(&unrelated.history[0].lines).contains("unrelated finalized row"));
    assert!(terminal_text(&unrelated.live).contains("Workflow"));
    pane.acknowledge_history(&unrelated.history);

    for (sequence, phase) in [(2, "build"), (3, "verify"), (4, "report")] {
        let mut update = snapshot(WorkflowState::Running);
        update.projection_sequence = Some(sequence);
        update.current_phase = Some(phase.to_owned());
        pane.transcript_mut().upsert_workflow(update);
        let frame = pane.render_terminal_update(120, 30);
        assert!(frame.history.is_empty(), "sequence {sequence}");
        assert!(terminal_text(&frame.live).contains(phase));
    }

    let mut completed = snapshot(WorkflowState::Completed);
    completed.projection_sequence = Some(5);
    completed.updated_at_ms = Some(12_000);
    pane.transcript_mut().upsert_workflow(completed);
    let terminal = pane.render_terminal_update(120, 30);
    assert_eq!(terminal.history.len(), 1, "one terminal group");
    assert!(terminal.live.is_empty(), "no live duplicate");
    assert!(
        matches!(
            &terminal.history[0].id,
            neo_tui::transcript::TranscriptBlockId::Workflow { .. }
        ),
        "unexpected terminal block id: {:?}",
        terminal.history[0].id
    );
    let group = terminal_text(&terminal.history[0].lines);
    assert_eq!(
        group.matches("Workflow  Runtime audit and fix").count(),
        1,
        "{group}"
    );
    assert_eq!(group.matches("Workflow Delegates").count(), 1, "{group}");
    assert_eq!(group.matches("Workflow Swarms").count(), 1, "{group}");
    pane.acknowledge_history(&terminal.history);
    let acknowledged = pane.render_terminal_update(120, 30);
    assert!(acknowledged.history.is_empty());
    assert!(!terminal_text(&acknowledged.live).contains("Workflow"));
}

#[test]
fn workflow_group_keeps_earliest_blocking_input_owner() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 30);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });
    pane.apply_agent_event(AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            turn: 1,
            id: "workflow-approval".to_owned(),
            operation: PermissionOperation::Shell,
            presentation: ApprovalPresentation::Tool {
                title: "Allow workflow command?".to_owned(),
                details: vec!["cargo test".to_owned()],
            },
            options: vec![ApprovalOption {
                label: "Allow once".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            }],
            workflow_origin: Some(origin("wf-test", "approval-call")),
        },
    });
    pane.apply_question_stream_update(StreamUpdate::QuestionRequested {
        id: "workflow-question".to_owned(),
        questions: vec![QuestionDisplayData {
            question: "Choose target?".to_owned(),
            header: None,
            body: None,
            options: vec![QuestionDisplayOption {
                label: "Local".to_owned(),
                description: None,
            }],
            multi_select: false,
        }],
        workflow_origin: Some(origin("wf-test", "question-call")),
    });

    let mut later = snapshot(WorkflowState::AwaitingUser);
    later.projection_sequence = Some(8);
    later.current_phase = Some("choose_target".to_owned());
    pane.transcript_mut().upsert_workflow(later);
    let mut delegate = agent_snapshot("later-agent");
    delegate.display_name = AgentDisplayName::new("LaterAgent");
    workflow_delegate_started(&mut pane, "later-agent-call", delegate.clone());
    delegate.latest_text = Some("later progress".to_owned());
    pane.apply_agent_event(AgentEvent::DelegateProgressUpdated {
        turn: 1,
        progress: agent_progress(&delegate, AgentLifecycleState::Running),
        workflow_origin: Some(origin("wf-test", "later-agent-call")),
    });

    assert_eq!(
        pane.earliest_blocking_entry(),
        Some(BlockingEntryKind::Approval("workflow-approval".to_owned()))
    );
    let blocked = pane.render_terminal_update(120, 30);
    let live = terminal_text(&blocked.live);
    assert!(live.contains("Allow workflow command?"), "{live}");
    assert!(!live.contains("Choose target?"), "{live}");
    assert!(live.contains("Workflow"), "{live}");

    pane.resolve_approval(
        "workflow-approval",
        &ApprovalResolution::Selected {
            action: ApprovalAction::PermitOnce,
            label: "Allow once".to_owned(),
            feedback: None,
        },
    );
    assert_eq!(
        pane.earliest_blocking_entry(),
        Some(BlockingEntryKind::Question("workflow-question".to_owned()))
    );
    let promoted = terminal_text(&pane.render_terminal_update(120, 30).live);
    assert!(promoted.contains("Choose target?"), "{promoted}");
}

fn assert_finalized_workflow_tool(
    pane: &neo_tui::transcript::TranscriptPane,
    workflow_index: usize,
    revision: u64,
) {
    assert_eq!(
        pane.transcript().entry_revisions()[workflow_index],
        revision
    );
    let TranscriptEntry::Workflow { component } = &pane.transcript().entries()[workflow_index]
    else {
        panic!("workflow entry")
    };
    let tool = &component.direct_tools()[0];
    assert_eq!(tool.result(), Some("final"));
    assert_eq!(tool.status(), ToolStatusKind::Succeeded);
}
