//! Events behavior (moved from `events.rs`).

use super::*;

#[test]
fn todo_event_data_serializes() {
    let data = TodoEventData {
        title: "Task".into(),
        status: "in_progress".into(),
    };
    let json = serde_json::to_string(&data).expect("serialize");
    assert!(json.contains("\"title\":\"Task\""));
    assert!(json.contains("\"status\":\"in_progress\""));
    let back: TodoEventData = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(data, back);
}

#[test]
fn question_event_data_serializes() {
    let data = QuestionEventData {
        question: "Which?".into(),
        header: Some("Choice".into()),
        body: None,
        options: vec![QuestionOptionData {
            label: "A".into(),
            description: Some("desc".into()),
        }],
        multi_select: false,
    };
    let json = serde_json::to_string(&data).expect("serialize");
    assert!(json.contains("\"question\":\"Which?\""));
    assert!(json.contains("\"multi_select\":false"));
    // body is None and should be skipped.
    assert!(!json.contains("\"body\""));
    let back: QuestionEventData = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(data, back);
}

#[test]
fn plan_mode_entered_serializes() {
    let event = AgentEvent::PlanModeEntered {
        turn: 3,
        id: "p1".into(),
    };
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"PlanModeEntered\""));
    assert!(json.contains("\"id\":\"p1\""));
    let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(event, back);
}

#[test]
fn plan_mode_exited_serializes() {
    let event = AgentEvent::PlanModeExited {
        turn: 5,
        id: "p1".into(),
    };
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"PlanModeExited\""));
    let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(event, back);
}

#[test]
fn plan_updated_serializes() {
    let event = AgentEvent::PlanUpdated {
        turn: 2,
        enabled: true,
    };
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"PlanUpdated\""));
    assert!(json.contains("\"enabled\":true"));
    let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(event, back);
}

#[test]
fn todo_updated_serializes() {
    let event = AgentEvent::TodoUpdated {
        turn: 2,
        todos: vec![
            TodoEventData {
                title: "A".into(),
                status: "done".into(),
            },
            TodoEventData {
                title: "B".into(),
                status: "pending".into(),
            },
        ],
    };
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"TodoUpdated\""));
    assert!(json.contains("\"todos\""));
    let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(event, back);
}

#[test]
fn question_requested_serializes() {
    let event = AgentEvent::QuestionRequested {
        turn: 1,
        id: "q-123".into(),
        questions: vec![QuestionEventData {
            question: "Test?".into(),
            header: None,
            body: None,
            options: vec![
                QuestionOptionData {
                    label: "Yes".into(),
                    description: None,
                },
                QuestionOptionData {
                    label: "No".into(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        workflow_origin: None,
    };
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"QuestionRequested\""));
    assert!(json.contains("\"q-123\""));
    let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(event, back);
}

#[test]
fn workflow_child_origin_round_trips() {
    let origin = crate::workflow::WorkflowExecutionOrigin {
        run_id: crate::workflow::WorkflowId("workflow-run".into()),
        human_handle: None,
        definition_name: "workflow".into(),
        definition_revision: None,
        phase_id: Some("phase".into()),
        invocation_id: Some("invocation".into()),
        swarm_item_id: None,
    };
    let runtime = crate::multi_agent::MultiAgentRuntime::new();
    let agent = runtime.start_foreground_delegate_for_test("task");
    let progress = agent.progress_snapshot();
    let swarm = crate::multi_agent::SwarmSnapshot {
        swarm_id: "swarm".to_owned(),
        description: "items".to_owned(),
        role: crate::multi_agent::AgentRole::Coder,
        mode: crate::multi_agent::AgentRunMode::Foreground,
        state: crate::multi_agent::AgentLifecycleState::Running,
        max_concurrency: 1,
        aggregate: crate::multi_agent::SwarmAggregate {
            total: 1,
            running: 1,
            ..crate::multi_agent::SwarmAggregate::default()
        },
        children: vec![crate::multi_agent::SwarmChildSnapshot {
            item_index: 0,
            item: "item".to_owned(),
            agent: agent.clone(),
        }],
    };
    let events = vec![
        AgentEvent::QuestionRequested {
            turn: 1,
            id: "question".to_owned(),
            questions: Vec::new(),
            workflow_origin: Some(origin.clone()),
        },
        AgentEvent::DelegateStarted {
            turn: 1,
            agent: agent.clone(),
            workflow_origin: Some(origin.clone()),
        },
        AgentEvent::DelegateUpdated {
            turn: 1,
            agent: agent.clone(),
            workflow_origin: Some(origin.clone()),
        },
        AgentEvent::DelegateProgressUpdated {
            turn: 1,
            progress: progress.clone(),
            workflow_origin: Some(origin.clone()),
        },
        AgentEvent::DelegateFinished {
            turn: 1,
            agent: agent.clone(),
            workflow_origin: Some(origin.clone()),
        },
        AgentEvent::DelegateSwarmStarted {
            turn: 1,
            swarm: swarm.clone(),
            workflow_origin: Some(origin.clone()),
        },
        AgentEvent::DelegateSwarmUpdated {
            turn: 1,
            swarm: swarm.clone(),
            workflow_origin: Some(origin.clone()),
        },
        AgentEvent::DelegateSwarmProgressUpdated {
            turn: 1,
            swarm_id: swarm.swarm_id.clone(),
            state: swarm.state,
            aggregate: swarm.aggregate,
            child_progress: crate::multi_agent::SwarmChildProgress {
                item_index: 0,
                progress,
            },
            workflow_origin: Some(origin.clone()),
        },
        AgentEvent::DelegateSwarmFinished {
            turn: 1,
            swarm,
            workflow_origin: Some(origin.clone()),
        },
    ];
    for event in &events {
        let json = serde_json::to_string(event).expect("serialize");
        assert!(json.contains("workflow_origin"), "{event:?}");
        let restored: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&restored, event);
    }

    let mut old_value = serde_json::to_value(&events[1]).expect("serialize old event");
    old_value
        .get_mut("DelegateStarted")
        .and_then(serde_json::Value::as_object_mut)
        .expect("delegate event object")
        .remove("workflow_origin");
    let restored_without_origin: AgentEvent =
        serde_json::from_value(old_value).expect("deserialize old event");
    assert!(matches!(
        restored_without_origin,
        AgentEvent::DelegateStarted {
            workflow_origin: None,
            ..
        }
    ));
}

#[test]
fn error_with_code_serializes() {
    let event = AgentEvent::Error {
        turn: 1,
        message: "rate limited".into(),
        code: Some("provider.rate_limit".into()),
        retry_after: Some(30),
    };
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"code\":\"provider.rate_limit\""));
    assert!(json.contains("\"retry_after\":30"));
    let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(event, back);

    let event = AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 500,
        error_code: "provider.transport_error".into(),
        message: "body closed".into(),
    };
    let json = serde_json::to_string(&event).expect("serialize retry event");
    let decoded: AgentEvent = serde_json::from_str(&json).expect("deserialize retry event");
    assert_eq!(decoded, event);
}

#[test]
fn error_without_code_backward_compatible() {
    // Old JSONL format without code/retry_after
    let json = r#"{"Error":{"turn":1,"message":"old format"}}"#;
    let event: AgentEvent = serde_json::from_str(json).expect("deserialize");
    match event {
        AgentEvent::Error {
            turn,
            message,
            code,
            retry_after,
        } => {
            assert_eq!(turn, 1);
            assert_eq!(message, "old format");
            assert_eq!(code, None);
            assert_eq!(retry_after, None);
        }
        _ => panic!("expected Error variant"),
    }
}

#[test]
fn context_window_updated_accepts_old_json_shape() {
    let json = r#"{"ContextWindowUpdated":{"turn":3,"used_tokens":42}}"#;
    let event: AgentEvent = serde_json::from_str(json).expect("deserialize");

    assert_eq!(
        event,
        AgentEvent::ContextWindowUpdated {
            turn: 3,
            used_tokens: 42,
            projected_tokens: None,
            max_tokens: None,
            trigger_tokens: None,
            remaining_tokens: None,
            source: None,
        }
    );
}
