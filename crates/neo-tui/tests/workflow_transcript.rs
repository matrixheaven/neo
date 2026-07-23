use neo_agent_core::AgentEvent;
use neo_agent_core::workflow::{WorkflowId, WorkflowSnapshot, WorkflowState, WorkflowStepRecord};
use neo_tui::primitive::{Component, Finalization, Line, strip_ansi};
use neo_tui::transcript::WorkflowCardComponent;

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
        steps: Vec::new(),
    }
}

fn text(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|line| strip_ansi(&line.to_ansi()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn workflow_card_projects_orchestration_without_child_duplication() {
    let mut workflow = snapshot(WorkflowState::Running);
    workflow.steps.push(WorkflowStepRecord {
        index: 0,
        name: "delegate child secret".to_owned(),
        state: WorkflowState::Completed,
        summary: Some("child result secret".to_owned()),
        details: Some(serde_json::json!({"command": "full shell command secret"})),
        agent: None,
        swarm: None,
        has_failures: None,
    });

    let rendered = text(&WorkflowCardComponent::new(workflow).render(120));

    assert!(rendered.contains("Runtime audit and fix"), "{rendered}");
    assert!(rendered.contains("phase verify"), "{rendered}");
    assert!(rendered.contains("3 invocations"), "{rendered}");
    assert!(rendered.contains("25 tokens"), "{rendered}");
    assert!(
        rendered.contains("focused verification running"),
        "{rendered}"
    );
    assert!(rendered.contains("TaskPause · TaskStop"), "{rendered}");
    assert!(!rendered.contains("delegate child secret"), "{rendered}");
    assert!(!rendered.contains("child result secret"), "{rendered}");
    assert!(
        !rendered.contains("full shell command secret"),
        "{rendered}"
    );
}

#[test]
fn historical_workflow_events_remain_read_only() {
    for variant in ["WorkflowStarted", "WorkflowUpdated", "WorkflowFinished"] {
        let payload = serde_json::json!({
            "turn": 4,
            "workflow": {
                "id": "wf-historical",
                "title": "Historical workflow",
                "state": "running",
                "steps": [{
                    "index": 0,
                    "name": "legacy step",
                    "state": "completed",
                    "summary": "legacy summary"
                }]
            }
        });
        let event: AgentEvent = serde_json::from_value(serde_json::Value::Object(
            [(variant.to_owned(), payload)].into_iter().collect(),
        ))
        .expect("old workflow event remains readable");

        let workflow = match event {
            AgentEvent::WorkflowStarted { workflow, .. }
            | AgentEvent::WorkflowUpdated { workflow, .. }
            | AgentEvent::WorkflowFinished { workflow, .. } => workflow,
            _ => panic!("historical workflow event"),
        };
        assert_eq!(workflow.projection_sequence, None);
        assert!(!workflow.recovery_failure);
        assert_eq!(workflow.started_at_ms, None);
        assert_eq!(workflow.updated_at_ms, None);
        assert_eq!(workflow.steps.len(), 1);
    }
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
