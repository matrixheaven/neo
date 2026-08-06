use super::*;

#[test]
fn child_runtime_does_not_inherit_parent_event_route() {
    let session = tempfile::tempdir().expect("session");
    let resolver = crate::runtime::WorkflowDispatchResolver::default();
    let config = AgentConfig::for_model(neo_ai::ModelSpec {
        provider: neo_ai::ProviderId("test".to_owned()),
        model: "test-model".to_owned(),
        api: neo_ai::ApiKind::OpenAi,
        capabilities: neo_ai::ModelCapabilities::default(),
    })
    .with_session_directory(session.path().to_path_buf())
    .with_workflow_dispatch_resolver(resolver);

    let deps = ChildRuntimeDeps::new(
        config,
        Arc::new(neo_ai::providers::fake::FakeModelClient::default()),
        Arc::new(ToolRegistry::new()),
    );

    assert!(
        deps.config
            .workflow_dispatch_resolver
            .event_callback(Some(session.path()))
            .is_none(),
        "child runtime must use its own event emitter"
    );
}

#[test]
fn structured_child_prompt_requires_json_only_output() {
    let ordinary = child_prompt(
        "inspect the change",
        DelegateContext::None,
        AgentRole::Coder,
        None,
    );
    assert!(!ordinary.contains("exactly one JSON value"));

    let schema = serde_json::json!({"type":"object","required":["ok"]});
    let structured = child_prompt(
        "inspect the change",
        DelegateContext::None,
        AgentRole::Coder,
        Some(&schema),
    );
    assert!(structured.contains("exactly one JSON value"));
    assert!(structured.contains(&schema.to_string()));
    assert!(structured.contains("Every required field must be present"));
    assert!(structured.contains("Do not use a Markdown fence"));
    assert!(structured.contains("Do not call a formatting tool"));
}

#[test]
fn swarm_operations_use_canonical_child_state() {
    let runtime = MultiAgentRuntime::new();
    let swarm_id = runtime.new_swarm_id();
    let first = runtime.start_delegate(
        "first",
        None,
        AgentRole::Coder,
        AgentRunMode::Foreground,
        DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    let second = runtime.start_delegate(
        "second",
        None,
        AgentRole::Coder,
        AgentRunMode::Foreground,
        DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    runtime.register_swarm(crate::multi_agent::SwarmSnapshot {
        swarm_id: swarm_id.clone(),
        description: "test".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: AgentLifecycleState::Running,
        max_concurrency: 2,
        aggregate: SwarmAggregate::from_states([
            AgentLifecycleState::Running,
            AgentLifecycleState::Running,
        ]),
        children: vec![
            crate::multi_agent::SwarmChildSnapshot {
                item_index: 0,
                item: "first".to_owned(),
                agent: first.clone(),
            },
            crate::multi_agent::SwarmChildSnapshot {
                item_index: 1,
                item: "second".to_owned(),
                agent: second.clone(),
            },
        ],
    });
    runtime.cancel_agent(&first.id).expect("cancel first");
    let _ = runtime.complete_delegate_for_test(&second.id, "done");

    let projected = runtime.swarm_snapshot(&swarm_id).expect("projected swarm");
    assert_eq!(projected.aggregate.completed, 1);
    assert_eq!(projected.aggregate.cancelled, 1);
    assert_eq!(
        projected.children[0].agent.state,
        AgentLifecycleState::Cancelled
    );
    assert_eq!(runtime.list_swarms()[0].aggregate.completed, 1);
    assert_eq!(runtime.resumable_swarm_items(&swarm_id), vec![0]);

    let detached = runtime.detach_swarm(&swarm_id).expect("detach swarm");
    assert_eq!(detached.mode, AgentRunMode::Background);
    assert!(
        detached
            .children
            .iter()
            .all(|child| child.agent.mode == AgentRunMode::Background)
    );
    for agent_id in [&first.id, &second.id] {
        let canonical = runtime.snapshot(agent_id).expect("canonical child");
        assert_eq!(canonical.mode, AgentRunMode::Background);
        assert!(canonical.detached_from_foreground);
    }
}

#[test]
fn child_finalization_is_atomic_and_always_persists_messages() {
    let runtime = MultiAgentRuntime::new();
    let child = runtime.start_foreground_delegate_for_test("cancelled child");
    runtime.cancel_agent(&child.id).expect("cancel child");
    let messages = vec![AgentMessage::user_text("keep this context")];

    let output =
        runtime.finish_child_run(&child, Instant::now(), Ok((Vec::new(), messages.clone())));

    assert_eq!(output.snapshot.state, AgentLifecycleState::Cancelled);
    assert_eq!(
        runtime
            .snapshot(&output.snapshot.id)
            .expect("canonical child")
            .prior_messages,
        messages
    );

    let event_cancelled = runtime.start_foreground_delegate_for_test("event cancelled child");
    let event_messages = vec![AgentMessage::user_text("event cancel context")];
    let event_output = runtime.finish_child_run(
        &event_cancelled,
        Instant::now(),
        Ok((
            vec![AgentEvent::RunFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            }],
            event_messages.clone(),
        )),
    );
    assert_eq!(event_output.snapshot.state, AgentLifecycleState::Cancelled);
    assert_eq!(
        runtime
            .snapshot(&event_output.snapshot.id)
            .expect("event-cancelled child")
            .prior_messages,
        event_messages
    );

    let completed = runtime.start_foreground_delegate_for_test("completed child");
    let completed_messages = vec![AgentMessage::user_text("completed context")];
    let completed_output = runtime.finish_child_run(
        &completed,
        Instant::now(),
        Ok((Vec::new(), completed_messages.clone())),
    );
    assert_eq!(
        completed_output.snapshot.state,
        AgentLifecycleState::Completed
    );
    assert_eq!(
        runtime
            .snapshot(&completed_output.snapshot.id)
            .expect("completed child")
            .prior_messages,
        completed_messages
    );
    assert!(
        runtime
            .cancel_agent(&completed_output.snapshot.id)
            .is_none()
    );

    let mut errored = runtime.start_foreground_delegate_for_test("cancelled error child");
    errored.prior_messages = vec![AgentMessage::user_text("prior error context")];
    runtime
        .cancel_agent(&errored.id)
        .expect("cancel error child");
    let error_output = runtime.finish_child_run(
        &errored,
        Instant::now(),
        Err("stream failed after cancellation".to_owned()),
    );
    assert_eq!(error_output.snapshot.state, AgentLifecycleState::Cancelled);
    assert_eq!(
        runtime
            .snapshot(&error_output.snapshot.id)
            .expect("cancelled error child")
            .prior_messages,
        errored.prior_messages
    );
}
