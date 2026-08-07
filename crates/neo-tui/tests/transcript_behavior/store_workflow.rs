use neo_agent_core::workflow::{WorkflowId, WorkflowSnapshot, WorkflowState};
use neo_tui::primitive::{Component, Finalization};
use neo_tui::transcript::{TranscriptEntry, TranscriptPane, TranscriptStore};

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
