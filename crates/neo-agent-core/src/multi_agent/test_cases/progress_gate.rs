use super::*;

#[test]
fn child_text_updates_are_rate_limited_without_delaying_boundaries() {
    let mut last = None;
    let text = AgentEvent::TextDelta {
        turn: 1,
        text: "delta".to_owned(),
    };
    assert!(child_progress_update_is_due(
        &text,
        Duration::ZERO,
        &mut last
    ));
    assert!(!child_progress_update_is_due(
        &text,
        Duration::from_millis(32),
        &mut last
    ));
    assert!(child_progress_update_is_due(
        &text,
        Duration::from_millis(33),
        &mut last
    ));

    let tool = AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({}),
        workflow_origin: None,
        output_ref: None,
    };
    assert!(child_progress_update_is_due(
        &tool,
        Duration::from_millis(34),
        &mut last
    ));
    assert!(!child_progress_update_is_due(
        &text,
        Duration::from_millis(35),
        &mut last
    ));

    let runtime = MultiAgentRuntime::new();
    let child = runtime.start_foreground_delegate_for_test("stream many deltas");
    let started_at = Instant::now();
    let mut last = None;
    let mut projected = 0;
    for _ in 0..10_000 {
        projected += usize::from(
            runtime
                .apply_child_event_and_project_when(&child.id, started_at, &text, || {
                    child_progress_update_is_due(&text, Duration::ZERO, &mut last)
                })
                .is_some(),
        );
    }
    assert_eq!(projected, 1);
    let snapshot = runtime.agent_snapshot(child.id.as_str()).expect("child");
    assert_eq!(
        latest_text_activity(&snapshot.activity, false)
            .expect("latest text")
            .chars()
            .count(),
        MAX_LATEST_MODEL_TEXT_CHARS
    );
}
