//! Tests for the Workflow Operator projection types and paging.

use neo_agent_core::workflow::{
    ChildCounts, StepRowState, WorkflowChildPage, WorkflowOperatorRequest,
    WorkflowOperatorSnapshot, WorkflowStepKey, WorkflowStepRow, PendingUserRequest,
    WorkflowChildRow, WorkflowChildState, WorkflowChildKey, WorkflowChildKind,
};

#[test]
fn operator_projection_types_construct_and_inspect() {
    let key = WorkflowStepKey {
        phase_id: Some("review".to_owned()),
        phase_marker_sequence: 1,
    };
    assert_eq!(key.phase_id.as_deref(), Some("review"));
    assert_eq!(key.phase_marker_sequence, 1);

    let counts = ChildCounts {
        done: 5,
        working: 2,
        queued: 3,
        failed: 1,
    };
    assert_eq!(counts.done + counts.working + counts.queued + counts.failed, 11);

    let step = WorkflowStepRow {
        key: key.clone(),
        title: "Review".to_owned(),
        order: 0,
        state: StepRowState::Active,
        done_count: 3,
        working_count: 1,
        queued_count: 0,
        failed_count: 0,
    };
    assert_eq!(step.title, "Review");
    assert!(matches!(step.state, StepRowState::Active));

    let request = WorkflowOperatorRequest {
        step: Some(key),
        cursor: None,
        limit: 20,
    };
    assert_eq!(request.limit, 20);

    let page = WorkflowChildPage {
        items: Vec::new(),
        next_cursor: Some("cursor-1".to_owned()),
        has_more: true,
        query_hash: "abc".to_owned(),
    };
    assert!(page.has_more);
}

#[test]
fn child_state_transitions() {
    assert!(WorkflowChildState::Completed.is_terminal());
    assert!(WorkflowChildState::Failed.is_terminal());
    assert!(WorkflowChildState::Cancelled.is_terminal());
    assert!(WorkflowChildState::Interrupted.is_terminal());
    assert!(!WorkflowChildState::Running.is_terminal());
    assert!(!WorkflowChildState::Queued.is_terminal());
    assert!(!WorkflowChildState::Recovering.is_terminal());
}

#[test]
fn child_key_display_and_equality() {
    let d = WorkflowChildKey::DirectDelegate {
        invocation_id: "inv1".to_owned(),
    };
    let s = WorkflowChildKey::SwarmItem {
        swarm_id: "sw1".to_owned(),
        item_id: "item1".to_owned(),
    };
    assert_eq!(d, d);
    assert_ne!(d, s);
    assert!(d.display_key().contains("inv1"));
    assert!(s.display_key().contains("sw1"));
}

#[test]
fn operator_request_cursor_construction() {
    let no_cursor = WorkflowOperatorRequest {
        step: None,
        cursor: None,
        limit: 50,
    };
    assert!(no_cursor.cursor.is_none());

    let with_cursor = WorkflowOperatorRequest {
        step: None,
        cursor: Some("100:abc123".to_owned()),
        limit: 10,
    };
    assert_eq!(with_cursor.cursor.unwrap(), "100:abc123");
}

#[test]
fn step_row_states_construct_directly() {
    for (state, label) in [
        (StepRowState::Pending, "Pending"),
        (StepRowState::Active, "Active"),
        (StepRowState::Completed, "Completed"),
        (StepRowState::Failed, "Failed"),
        (StepRowState::Paused, "Paused"),
    ] {
        let row = WorkflowStepRow {
            key: WorkflowStepKey {
                phase_id: None,
                phase_marker_sequence: 0,
            },
            title: label.to_owned(),
            order: 0,
            state,
            done_count: 0,
            working_count: 0,
            queued_count: 0,
            failed_count: 0,
        };
        assert_eq!(row.state, state);
    }
}
