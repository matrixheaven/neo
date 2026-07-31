use neo_agent_core::workflow::{WorkflowId, WorkflowSnapshot, WorkflowState};
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
/// Accepted workflow transitions project once by typed projection sequence,
/// and the terminal workflow outcome commits once as the entry final status.
#[test]
fn workflow_phase_and_terminal_facts_commit_once_by_projection_sequence() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);

    // Intermediate phase transitions project as stable facts while the card
    // stays live.
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
    assert!(history.contains("running"), "history:\n{history}");
    assert!(history.contains("phase verify"), "history:\n{history}");
    assert!(
        update.live.join("\n").contains("Runtime audit and fix"),
        "the mutable card stays live"
    );
    pane.acknowledge_history(&update.history);

    // The newest snapshot overwrites the prior one, but the accepted
    // transition survives by projection sequence.
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
    assert!(history.contains("phase build"), "history:\n{history}");
    assert!(
        !history.contains("phase verify"),
        "acknowledged transition must never replay:\n{history}"
    );
    pane.acknowledge_history(&update.history);

    // The terminal outcome commits once as the entry final status — never a
    // complete duplicate card after the progressive rows.
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
