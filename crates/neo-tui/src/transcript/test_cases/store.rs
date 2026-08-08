//! Transcript store behavior (moved from `store.rs`).

use super::*;
use neo_agent_core::multi_agent::{AgentActivityEntry, AgentToolActivityPhase};

fn workflow_snapshot_for_route_test() -> WorkflowSnapshot {
    WorkflowSnapshot {
        id: neo_agent_core::workflow::WorkflowId("workflow".to_owned()),
        title: "workflow".to_owned(),
        state: neo_agent_core::workflow::WorkflowState::Running,
        current_phase: None,
        projection_sequence: Some(1),
        recovery_failure: false,
        started_at_ms: None,
        updated_at_ms: None,
        invocation_count: 0,
        failure_count: 0,
        actual_usage: None,
        latest_log_summary: None,
        latest_report_summary: None,
        terminal_reason: None,
        display_name: "workflow".to_owned(),
        purpose: String::new(),
    }
}

fn workflow_origin_for_route_test(invocation_id: &str) -> WorkflowExecutionOrigin {
    WorkflowExecutionOrigin {
        run_id: neo_agent_core::workflow::WorkflowId("workflow".to_owned()),
        human_handle: None,
        definition_name: "workflow".to_owned(),
        definition_revision: None,
        phase_id: None,
        invocation_id: Some(invocation_id.to_owned()),
        swarm_item_id: None,
    }
}

fn insert_failed_workflow_placeholder(store: &mut TranscriptStore, id: &str, name: &str) {
    store
        .upsert_workflow_tool(
            workflow_origin_for_route_test(id),
            ToolCallState {
                id: id.to_owned(),
                name: name.to_owned(),
                arguments: None,
                result: Some("failed before child start".to_owned()),
                details: None,
                status: ToolStatusKind::Failed,
                exit_code: None,
            },
        )
        .expect("workflow placeholder");
}

fn running_workflow_tool(id: &str, name: &str) -> ToolCallState {
    ToolCallState {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: None,
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    }
}

fn delegate_snapshot_for_merge_test() -> AgentSnapshot {
    neo_agent_core::multi_agent::MultiAgentRuntime::new()
        .start_foreground_delegate_for_test("merge run counts")
}

fn swarm_snapshot_for_merge_test(agent: AgentSnapshot) -> SwarmSnapshot {
    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "merge child run counts".to_owned(),
        agent,
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    SwarmSnapshot {
        swarm_id: "merge-swarm".to_owned(),
        description: "merge run counts".to_owned(),
        role: neo_agent_core::multi_agent::AgentRole::Coder,
        mode: neo_agent_core::multi_agent::AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 1,
        aggregate,
        children,
    }
}

#[test]
fn delegate_and_swarm_merges_prefer_the_newest_run_count() {
    let mut old_terminal = delegate_snapshot_for_merge_test();
    old_terminal.state = AgentLifecycleState::Completed;
    old_terminal.run_count = 1;
    old_terminal.updated_at_ms = 10;
    old_terminal.terminal_at_ms = Some(10);
    old_terminal.latest_text = Some("old terminal run".to_owned());

    let new_running = AgentSnapshot {
        state: AgentLifecycleState::Running,
        run_count: 2,
        updated_at_ms: 20,
        terminal_at_ms: None,
        latest_text: Some("new running run".to_owned()),
        outcome: None,
        ..old_terminal.clone()
    };

    assert_eq!(
        merge_delegate_snapshot(&old_terminal, new_running.clone()),
        new_running
    );
    assert_eq!(
        merge_delegate_snapshot(&new_running, old_terminal.clone()),
        new_running
    );

    let merged_swarm = merge_swarm_snapshot(
        &swarm_snapshot_for_merge_test(old_terminal.clone()),
        swarm_snapshot_for_merge_test(new_running.clone()),
    );
    assert_eq!(merged_swarm.children[0].agent, new_running);

    let stale_swarm = merge_swarm_snapshot(
        &swarm_snapshot_for_merge_test(new_running.clone()),
        swarm_snapshot_for_merge_test(old_terminal),
    );
    assert_eq!(stale_swarm.children[0].agent, new_running);
}

#[test]
fn delegate_merge_keeps_activity_when_live_snapshot_is_partial() {
    let mut current = delegate_snapshot_for_merge_test();
    current.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "tool-1".to_owned(),
            name: "Read".to_owned(),
            summary: Some("file".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    });
    current.latest_text = None;

    let mut partial = current.clone();
    partial.activity.clear();
    partial.updated_at_ms = current.updated_at_ms.saturating_add(1);

    assert_eq!(merge_delegate_snapshot(&current, partial), current);
}

#[test]
fn workflow_children_require_parent_placeholder_and_existing_children_can_update() {
    let mut store = TranscriptStore::new();
    store.upsert_workflow(workflow_snapshot_for_route_test());

    let delegate_origin = workflow_origin_for_route_test("delegate-call");
    let delegate = delegate_snapshot_for_merge_test();
    let before_delegate = store.entries()[0].clone();
    let before_delegate_revision = store.entry_revisions()[0];

    assert_eq!(
        store.upsert_workflow_delegate(&delegate_origin, delegate.clone()),
        Ok(false)
    );
    assert_eq!(store.entries()[0], before_delegate);
    assert_eq!(store.entry_revisions()[0], before_delegate_revision);
    assert!(!store.is_tool_run_suppressed("delegate-call"));

    assert_eq!(
        store.upsert_workflow_tool(
            delegate_origin.clone(),
            running_workflow_tool("delegate-call", "Delegate"),
        ),
        Ok(true)
    );
    assert!(store.tool("delegate-call").is_some());
    assert_eq!(
        store.upsert_workflow_delegate(&delegate_origin, delegate.clone()),
        Ok(true)
    );
    assert!(store.is_tool_run_suppressed("delegate-call"));

    let updated_delegate = AgentSnapshot {
        latest_text: Some("updated delegate".to_owned()),
        updated_at_ms: delegate.updated_at_ms.saturating_add(1),
        ..delegate
    };
    assert_eq!(
        store.upsert_workflow_delegate(&delegate_origin, updated_delegate),
        Ok(true)
    );
    let TranscriptEntry::Workflow { component } = &store.entries()[0] else {
        panic!("workflow entry")
    };
    assert_eq!(
        component.delegates()[0].latest_text.as_deref(),
        Some("updated delegate")
    );

    let swarm_origin = workflow_origin_for_route_test("swarm-call");
    let swarm = swarm_snapshot_for_merge_test(delegate_snapshot_for_merge_test());
    let before_swarm = store.entries()[0].clone();
    let before_swarm_revision = store.entry_revisions()[0];

    assert_eq!(
        store.upsert_workflow_swarm(&swarm_origin, swarm.clone()),
        Ok(false)
    );
    assert_eq!(store.entries()[0], before_swarm);
    assert_eq!(store.entry_revisions()[0], before_swarm_revision);
    assert!(!store.is_tool_run_suppressed("swarm-call"));

    assert_eq!(
        store.upsert_workflow_tool(
            swarm_origin.clone(),
            running_workflow_tool("swarm-call", "DelegateSwarm"),
        ),
        Ok(true)
    );
    assert!(store.tool("swarm-call").is_some());
    assert_eq!(
        store.upsert_workflow_swarm(&swarm_origin, swarm.clone()),
        Ok(true)
    );
    assert!(store.is_tool_run_suppressed("swarm-call"));

    let mut updated_swarm = swarm;
    updated_swarm.description = "updated swarm".to_owned();
    updated_swarm.children[0].agent.latest_text = Some("updated child".to_owned());
    assert_eq!(
        store.upsert_workflow_swarm(&swarm_origin, updated_swarm),
        Ok(true)
    );
    let TranscriptEntry::Workflow { component } = &store.entries()[0] else {
        panic!("workflow entry")
    };
    assert_eq!(component.swarms()[0].description, "updated swarm");
    assert_eq!(
        component.swarms()[0].children[0]
            .agent
            .latest_text
            .as_deref(),
        Some("updated child")
    );
}

#[test]
fn workflow_child_progress_suppresses_only_an_absorbed_parent_placeholder() {
    let mut store = TranscriptStore::new();
    store.upsert_workflow(workflow_snapshot_for_route_test());

    let delegate_origin = workflow_origin_for_route_test("delegate-progress-call");
    let delegate = delegate_snapshot_for_merge_test();
    store
        .upsert_workflow_tool(
            delegate_origin.clone(),
            running_workflow_tool("delegate-progress-call", "Delegate"),
        )
        .expect("delegate parent");
    store
        .upsert_workflow_delegate(&delegate_origin, delegate.clone())
        .expect("delegate child");
    store.unsuppress_tool_run("delegate-progress-call");
    store.push_tool_run("delegate-progress-call", "Delegate", None);
    let mut delegate_progress = AgentProgressSnapshot::from_agent(&delegate);
    delegate_progress.updated_at_ms = delegate_progress.updated_at_ms.saturating_add(1);
    delegate_progress.latest_text = Some("progress without parent".to_owned());

    assert_eq!(
        store.upsert_workflow_delegate_progress(&delegate_origin, &delegate_progress),
        Ok(true)
    );
    assert!(
        !store.is_tool_run_suppressed("delegate-progress-call"),
        "an unrelated top-level placeholder must remain visible"
    );

    store
        .upsert_workflow_tool(
            delegate_origin.clone(),
            running_workflow_tool("delegate-progress-call", "Delegate"),
        )
        .expect("recreated delegate parent");
    assert_eq!(
        store.upsert_workflow_delegate_progress(&delegate_origin, &delegate_progress),
        Ok(true),
        "an unchanged child still absorbs its recreated parent"
    );
    assert!(store.is_tool_run_suppressed("delegate-progress-call"));

    let swarm_origin = workflow_origin_for_route_test("swarm-progress-call");
    let swarm = swarm_snapshot_for_merge_test(delegate_snapshot_for_merge_test());
    store
        .upsert_workflow_tool(
            swarm_origin.clone(),
            running_workflow_tool("swarm-progress-call", "DelegateSwarm"),
        )
        .expect("swarm parent");
    store
        .upsert_workflow_swarm(&swarm_origin, swarm.clone())
        .expect("swarm child");
    store.unsuppress_tool_run("swarm-progress-call");
    store.push_tool_run("swarm-progress-call", "DelegateSwarm", None);
    let mut swarm_progress = AgentProgressSnapshot::from_agent(&swarm.children[0].agent);
    swarm_progress.updated_at_ms = swarm_progress.updated_at_ms.saturating_add(1);
    swarm_progress.latest_text = Some("swarm progress without parent".to_owned());
    let child_progress = SwarmChildProgress {
        item_index: 0,
        progress: swarm_progress,
    };

    assert_eq!(
        store.upsert_workflow_swarm_progress(
            &swarm_origin,
            &swarm.swarm_id,
            swarm.state,
            swarm.aggregate,
            &child_progress,
        ),
        Ok(true)
    );
    assert!(
        !store.is_tool_run_suppressed("swarm-progress-call"),
        "an unrelated top-level swarm placeholder must remain visible"
    );
}

#[test]
fn workflow_delegate_origin_conflict_is_atomic() {
    let mut store = TranscriptStore::new();
    store.upsert_workflow(workflow_snapshot_for_route_test());
    store
        .upsert_workflow_tool(
            workflow_origin_for_route_test("different-invocation"),
            ToolCallState {
                id: "delegate-call".to_owned(),
                name: "Delegate".to_owned(),
                arguments: None,
                result: None,
                details: None,
                status: ToolStatusKind::Running,
                exit_code: None,
            },
        )
        .expect("workflow tool");
    let before_entry = store.entries()[0].clone();
    let before_revision = store.entry_revisions()[0];
    let before_cache = store.render_cache[0].is_some();
    let agent = neo_agent_core::multi_agent::MultiAgentRuntime::new()
        .start_foreground_delegate_for_test("task");

    let result =
        store.upsert_workflow_delegate(&workflow_origin_for_route_test("delegate-call"), agent);

    assert_eq!(
        result,
        Err(WorkflowActivityRouteError::ConflictingOrigin {
            tool_id: "delegate-call".to_owned(),
        })
    );
    assert_eq!(store.entries()[0], before_entry);
    assert_eq!(store.entry_revisions()[0], before_revision);
    assert_eq!(store.render_cache[0].is_some(), before_cache);
}

#[test]
fn workflow_delegate_progress_without_snapshot_keeps_failed_placeholder() {
    let mut store = TranscriptStore::new();
    store.upsert_workflow(workflow_snapshot_for_route_test());
    insert_failed_workflow_placeholder(&mut store, "delegate-call", "Delegate");
    let revision = store.entry_revisions()[0];
    let agent = neo_agent_core::multi_agent::MultiAgentRuntime::new()
        .start_foreground_delegate_for_test("task");
    let progress = agent.progress_snapshot();

    let result = store.upsert_workflow_delegate_progress(
        &workflow_origin_for_route_test("delegate-call"),
        &progress,
    );

    assert_eq!(result, Ok(false));
    assert_eq!(store.entry_revisions()[0], revision);
    assert!(!store.is_tool_run_suppressed("delegate-call"));
    let tool = store.tool("delegate-call").expect("failed placeholder");
    assert_eq!(tool.status(), ToolStatusKind::Failed);
    assert_eq!(tool.result(), Some("failed before child start"));
}

#[test]
fn workflow_swarm_progress_without_snapshot_keeps_failed_placeholder() {
    let mut store = TranscriptStore::new();
    store.upsert_workflow(workflow_snapshot_for_route_test());
    insert_failed_workflow_placeholder(&mut store, "swarm-call", "DelegateSwarm");
    let revision = store.entry_revisions()[0];
    let agent = neo_agent_core::multi_agent::MultiAgentRuntime::new()
        .start_foreground_delegate_for_test("task");
    let progress = agent.progress_snapshot();
    let child_progress = SwarmChildProgress {
        item_index: 0,
        progress,
    };
    let aggregate = SwarmAggregate::from_states([child_progress.progress.state]);

    let result = store.upsert_workflow_swarm_progress(
        &workflow_origin_for_route_test("swarm-call"),
        "missing-swarm",
        child_progress.progress.state,
        aggregate,
        &child_progress,
    );

    assert_eq!(result, Ok(false));
    assert_eq!(store.entry_revisions()[0], revision);
    assert!(!store.is_tool_run_suppressed("swarm-call"));
    let tool = store.tool("swarm-call").expect("failed placeholder");
    assert_eq!(tool.status(), ToolStatusKind::Failed);
    assert_eq!(tool.result(), Some("failed before child start"));
}

#[test]
fn render_entry_ansi_cached_stores_final_ansi_rows() {
    let mut store = TranscriptStore::new();
    let theme = TuiTheme::default();
    store.push(TranscriptEntry::assistant_message("cached answer"));

    let first = store.render_entry_ansi_cached(
        0,
        EntryRenderParams {
            width: 80,
            theme: &theme,
            activity_frame: 0,
            image_render_policy: ImageRenderPolicy::default(),
            image_capabilities: TerminalImageCapabilities::default(),
            viewport_rows: 24,
        },
    );

    assert!(first.iter().any(|line| line.contains("cached answer")));
    let cached = store.render_cache[0].as_ref().expect("cached render");
    assert_eq!(cached.ansi_lines, first);
    assert_eq!(
        store.render_entry_ansi_cached(
            0,
            EntryRenderParams {
                width: 80,
                theme: &theme,
                activity_frame: 99,
                image_render_policy: ImageRenderPolicy::default(),
                image_capabilities: TerminalImageCapabilities::default(),
                viewport_rows: 24,
            },
        ),
        first
    );
}
