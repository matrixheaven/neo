//! Task browser behavior (moved from `task_browser.rs`).

use super::*;

fn bash_snapshot(status: BackgroundTaskStatus) -> BackgroundTaskSnapshot {
    BackgroundTaskSnapshot {
        task_id: "bash-1".to_owned(),
        kind: BackgroundTaskKind::Bash,
        status,
        description: "cargo test".to_owned(),
        elapsed: Duration::from_secs(65),
        output: Some(CommandOutput {
            exit_code: Some(0),
            signal: None,
            stdout: "ok\nnext".to_owned(),
            stderr: "warn".to_owned(),
            stdout_truncated: true,
            stderr_truncated: false,
            resource_limit: None,
        }),
        answers: None,
        delegate: None,
        swarm: None,
        workflow: None,
    }
}

#[test]
fn task_browser_adapter_maps_bash_snapshot() {
    let item = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Running));

    assert_eq!(item.id, "bash-1");
    assert_eq!(item.kind, TaskBrowserKind::Bash);
    assert_eq!(item.status, TaskBrowserStatus::Running);
    assert_eq!(item.title, "cargo test");
    assert_eq!(item.elapsed, "01:05");
    assert!(item.can_stop);
    assert!(item.detail_lines.iter().any(|line| line.contains("bash-1")));
    assert!(item.preview_lines.iter().any(|line| line == "stdout:"));
    assert!(item.preview_lines.iter().any(|line| line == "ok"));
    assert!(
        item.preview_lines
            .iter()
            .any(|line| line == "[stdout truncated]")
    );
}

#[test]
fn task_browser_adapter_maps_terminal_statuses() {
    let completed = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Completed));
    let failed = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Failed));
    let cancelled = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Cancelled));
    let timed_out = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::TimedOut));

    assert_eq!(completed.status, TaskBrowserStatus::Completed);
    assert_eq!(failed.status, TaskBrowserStatus::Failed);
    assert_eq!(cancelled.status, TaskBrowserStatus::Cancelled);
    assert_eq!(timed_out.status, TaskBrowserStatus::TimedOut);
    assert!(!completed.can_stop);
    assert!(failed.status.is_interrupted());
    assert!(cancelled.status.is_interrupted());
    assert!(timed_out.status.is_interrupted());
}

#[test]
fn task_browser_adapter_maps_question_snapshot() {
    let snapshot = BackgroundTaskSnapshot {
        task_id: "question-1".to_owned(),
        kind: BackgroundTaskKind::Question,
        status: BackgroundTaskStatus::WaitingForUser,
        description: "Pick one".to_owned(),
        elapsed: Duration::from_secs(2),
        output: None,
        answers: Some(vec!["yes".to_owned()]),
        delegate: None,
        swarm: None,
        workflow: None,
    };

    let item = snapshot_to_item(&snapshot);

    assert_eq!(item.kind, TaskBrowserKind::Question);
    assert_eq!(item.status, TaskBrowserStatus::Waiting);
    assert!(item.can_stop);
    assert_eq!(item.preview_lines, vec!["answer 1: yes".to_owned()]);
}

#[test]
fn task_browser_adapter_shows_waiting_question_prompt() {
    let snapshot = BackgroundTaskSnapshot {
        task_id: "question-1".to_owned(),
        kind: BackgroundTaskKind::Question,
        status: BackgroundTaskStatus::WaitingForUser,
        description: "Pick one".to_owned(),
        elapsed: Duration::from_secs(2),
        output: None,
        answers: None,
        delegate: None,
        swarm: None,
        workflow: None,
    };

    let item = snapshot_to_item(&snapshot);

    assert!(
        item.detail_lines
            .iter()
            .any(|line| line == "prompt: Pick one")
    );
    assert_eq!(item.preview_lines, vec!["Pick one".to_owned()]);
}

#[test]
fn task_browser_adapter_builds_snapshot_collection() {
    let snapshot = bash_snapshot(BackgroundTaskStatus::Running);
    let browser_snapshot = snapshots_to_browser_snapshot(&[snapshot]);

    assert_eq!(browser_snapshot.items().len(), 1);
    assert_eq!(browser_snapshot.items()[0].id, "bash-1");
}

#[test]
fn task_browser_adapter_maps_delegate_snapshot() {
    use neo_agent_core::multi_agent::{
        AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
        AgentSnapshot, DelegateContext,
    };
    let name = AgentDisplayName::new("Gibbs");
    let agent = AgentSnapshot {
        id: AgentId::from_suffix_for_test("del-1"),
        display_name: name.clone(),
        path: AgentPath::root_child(&name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: "fix the border".to_owned(),
        task_title: "fix the border".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: true,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 2,
        token_count: 1000,
        input_token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::from_secs(10),
        latest_text: Some("reading file...".to_owned()),
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    };
    let snapshot = BackgroundTaskSnapshot {
        task_id: agent.id.as_str().to_owned(),
        kind: BackgroundTaskKind::Delegate,
        status: BackgroundTaskStatus::Running,
        description: agent.task.clone(),
        elapsed: Duration::from_secs(10),
        output: None,
        answers: None,
        delegate: Some(agent),
        swarm: None,
        workflow: None,
    };
    let item = snapshot_to_item(&snapshot);
    assert_eq!(item.kind, TaskBrowserKind::Delegate);
    assert!(item.detail_lines.iter().any(|l| l.contains("name:")));
    assert!(item.preview_lines.iter().any(|l| l.contains("latest")));
}

#[test]
fn task_browser_adapter_maps_swarm_snapshot() {
    use neo_agent_core::multi_agent::{
        AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
        AgentSnapshot, DelegateContext, SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
    };
    let name = AgentDisplayName::new("Zeno");
    let agent = AgentSnapshot {
        id: AgentId::from_suffix_for_test("sw-0"),
        display_name: name.clone(),
        path: AgentPath::root_child(&name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: "item 0".to_owned(),
        task_title: "item 0".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: true,
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
        elapsed: Duration::from_secs(5),
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    };
    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "check grep".to_owned(),
        agent,
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let swarm = SwarmSnapshot {
        swarm_id: "swarm-1".to_owned(),
        description: "audit schemas".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        state: AgentLifecycleState::Running,
        max_concurrency: 1,
        aggregate,
        children,
    };
    let snapshot = BackgroundTaskSnapshot {
        task_id: swarm.swarm_id.clone(),
        kind: BackgroundTaskKind::DelegateSwarm,
        status: BackgroundTaskStatus::Running,
        description: swarm.description.clone(),
        elapsed: Duration::from_secs(5),
        output: None,
        answers: None,
        delegate: None,
        swarm: Some(swarm),
        workflow: None,
    };
    let item = snapshot_to_item(&snapshot);
    assert_eq!(item.kind, TaskBrowserKind::DelegateSwarm);
    assert!(item.detail_lines.iter().any(|l| l.contains("children:")));
}

fn completed_swarm_agent(
    name: &str,
    id: &str,
    task: &str,
    title: &str,
    summary: &str,
) -> neo_agent_core::multi_agent::AgentSnapshot {
    use neo_agent_core::multi_agent::{
        AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
        AgentSnapshot, AgentTerminalOutcome, DelegateContext,
    };
    let display_name = AgentDisplayName::new(name);
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id),
        display_name: display_name.clone(),
        path: AgentPath::swarm_child("swarm_comp", &display_name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Completed,
        task: task.to_owned(),
        task_title: title.to_owned(),
        created_at_ms: 1,
        updated_at_ms: 11,
        started_at_ms: Some(1),
        terminal_at_ms: Some(11),
        detached_from_foreground: true,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 2,
        token_count: 500,
        input_token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::from_secs(10),
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: Some(AgentTerminalOutcome {
            summary: summary.to_owned(),
            is_error: false,
        }),
    }
}

fn delegate_swarm_snapshot_with_completed_children() -> BackgroundTaskSnapshot {
    use neo_agent_core::multi_agent::{
        AgentLifecycleState, AgentRole, AgentRunMode, SwarmAggregate, SwarmChildSnapshot,
        SwarmSnapshot,
    };
    let child_a = completed_swarm_agent(
        "Alpha",
        "sw-comp-a",
        "child A prompt",
        "Child A",
        "All good",
    );
    let child_b =
        completed_swarm_agent("Beta", "sw-comp-b", "child B prompt", "Child B", "Done too");
    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "item-a".to_owned(),
            agent: child_a,
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "item-b".to_owned(),
            agent: child_b,
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    let swarm = SwarmSnapshot {
        swarm_id: "swarm_comp".to_owned(),
        description: "completed swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Background,
        state: AgentLifecycleState::Completed,
        max_concurrency: 2,
        aggregate,
        children,
    };
    BackgroundTaskSnapshot {
        task_id: swarm.swarm_id.clone(),
        kind: BackgroundTaskKind::DelegateSwarm,
        status: BackgroundTaskStatus::Completed,
        description: swarm.description.clone(),
        elapsed: Duration::from_secs(20),
        output: None,
        answers: None,
        delegate: None,
        swarm: Some(swarm),
        workflow: None,
    }
}

#[test]
fn task_browser_uses_cancelled_vocabulary_for_interrupted_tasks() {
    let cancelled = snapshot_to_item(&bash_snapshot(BackgroundTaskStatus::Cancelled));

    assert_eq!(cancelled.status, TaskBrowserStatus::Cancelled);
    assert_eq!(cancelled.status.label(), "cancelled");
    assert!(cancelled.status.is_interrupted());
}

#[test]
fn task_browser_swarm_details_include_aggregate_and_child_results() {
    let item = snapshot_to_item(&delegate_swarm_snapshot_with_completed_children());
    let details = item.detail_lines.join("\n");

    assert!(details.contains("aggregate:"), "{details}");
    assert!(details.contains("completed"), "{details}");
    assert!(details.contains("agent_"), "{details}");
}

#[test]
fn workflow_child_row_displays_projected_live_and_durable_terminal_facts() {
    use neo_agent_core::workflow::{
        WorkflowChildKey, WorkflowChildKind, WorkflowChildRow, WorkflowChildState,
    };

    let mut child = WorkflowChildRow {
        key: WorkflowChildKey::DirectDelegate {
            invocation_id: "inv-1".to_owned(),
        },
        child_kind: WorkflowChildKind::Delegate,
        phase_id: None,
        agent_id: Some("agent-workflow-live".to_owned()),
        state: WorkflowChildState::Running,
        title: Some("Review".to_owned()),
        role: None,
        queued_at_ms: Some(1_000),
        started_at_ms: Some(2_000),
        updated_at_ms: 2_000,
        terminal_at_ms: None,
        terminal_summary: None,
        error_summary: None,
        actual_usage: Some(serde_json::json!({"total_tokens": 15})),
        latest_activity: Some("Read: src/lib.rs".to_owned()),
        generated_files: vec!["notes.md".to_owned(), "summary.md".to_owned()],
    };

    let row = workflow_child_row(&child);
    assert_eq!(row.state, TaskBrowserWorkflowRowState::Working);
    assert_eq!(
        row.actual_usage,
        Some(serde_json::json!({"total_tokens": 15}))
    );
    assert_eq!(row.latest_activity.as_deref(), Some("Read: src/lib.rs"));
    assert_eq!(
        row.generated_files,
        vec!["notes.md".to_owned(), "summary.md".to_owned()]
    );

    child.state = WorkflowChildState::Completed;
    child.terminal_at_ms = Some(3_000);
    child.terminal_summary = Some("durable result".to_owned());
    child.actual_usage = Some(serde_json::json!({
        "input_tokens": 1,
        "output_tokens": 2,
        "input_cache_read_tokens": 3,
        "input_cache_write_tokens": 4,
    }));
    child.latest_activity = None;
    let terminal = workflow_child_row(&child);
    assert_eq!(terminal.state, TaskBrowserWorkflowRowState::Completed);
    assert_eq!(
        terminal.actual_usage,
        Some(serde_json::json!({
            "input_tokens": 1,
            "output_tokens": 2,
            "input_cache_read_tokens": 3,
            "input_cache_write_tokens": 4,
        }))
    );
    assert_eq!(terminal.latest_activity, None);
    assert_eq!(terminal.terminal_summary.as_deref(), Some("durable result"));
}
