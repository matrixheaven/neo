use futures::{StreamExt, stream};
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::multi_agent::{
    AgentActivityKind, AgentLifecycleState, AgentPathKind, AgentRole, AgentRunMode,
    MultiAgentRuntime, SwarmAggregate,
};
use neo_agent_core::tools::ToolRegistry;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, PermissionMode,
};
use neo_ai::{AiError, AiStreamEvent, ChatRequest, ModelClient, StopReason, ThinkingKind};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

fn child_text_turn(text: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: format!("msg_{text}"),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]
}

#[tokio::test]
async fn child_run_uses_parent_cancellation_token() {
    use neo_agent_core::multi_agent::{ChildRuntimeDeps, DelegateContext, DelegateRequest};

    let cancel = CancellationToken::new();
    cancel.cancel();
    let harness = FakeHarness::from_turns([child_text_turn("should not run")]);
    let deps = ChildRuntimeDeps::new(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        Arc::new(ToolRegistry::new()),
    )
    .with_cancel_token(cancel);
    let request = DelegateRequest {
        task: "cancel child".to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
        output_schema: None,
    };

    let output = MultiAgentRuntime::new()
        .run_child_turn(deps, &request, AgentRunMode::Foreground)
        .await
        .expect("child run returns failed snapshot");

    assert_eq!(output.snapshot.state, AgentLifecycleState::Cancelled);
    assert!(harness.requests().is_empty(), "{:#?}", harness.requests());
}

#[tokio::test]
async fn foreground_delegate_cancel_marks_child_cancelled_when_tool_future_is_dropped() {
    let multi_agent = MultiAgentRuntime::new();
    let model = Arc::new(DelayedTurnModel::new(vec![
        vec![
            DelayedStep::Event(AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "parent".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::ToolCallStart {
                id: "delegate_call".to_owned(),
                name: "Delegate".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::ToolCallArgsDelta {
                id: "delegate_call".to_owned(),
                json_fragment: r#"{"task":"slow child"}"#.to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::ToolCallEnd {
                id: "delegate_call".to_owned(),
                raw_arguments: r#"{"task":"slow child"}"#.to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::ToolUse,
                usage: None,
            }),
        ],
        vec![DelayedStep::Delay(std::time::Duration::from_secs(30))],
    ]));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(neo_agent_core::harness::fake_model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_multi_agent(multi_agent.clone()),
        model,
        ToolRegistry::with_builtin_tools(),
    );
    let cancel = CancellationToken::new();
    let mut context = AgentContext::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("delegate"),
        cancel.clone(),
    );
    let mut agent_id = None;

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some(event) = stream.next().await {
            let event = event.expect("runtime event");
            if let AgentEvent::DelegateStarted { agent, .. } = event {
                agent_id = Some(agent.id.as_str().to_owned());
                cancel.cancel();
            }
        }
    })
    .await
    .expect("cancelled delegate turn should finish");

    let agent_id = agent_id.expect("delegate started");
    let snapshot = multi_agent
        .agent_snapshot(&agent_id)
        .expect("delegate snapshot");
    assert_eq!(snapshot.state, AgentLifecycleState::Cancelled);
}

#[tokio::test]
async fn cancel_agent_stops_active_child_stream() {
    use neo_agent_core::multi_agent::{ChildRuntimeDeps, DelegateContext};

    let runtime = MultiAgentRuntime::new();
    let model = Arc::new(DelayedTurnModel::new(vec![vec![
        DelayedStep::Event(AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "child".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::ThinkingStart {
            id: "thinking".to_owned(),
            kind: ThinkingKind::Unknown,
        }),
        DelayedStep::Delay(std::time::Duration::from_secs(30)),
        DelayedStep::Event(AiStreamEvent::ThinkingDelta {
            text: "should not arrive".to_owned(),
        }),
    ]]));
    let deps = ChildRuntimeDeps::new(
        AgentConfig::for_model(neo_agent_core::harness::fake_model()),
        model,
        Arc::new(ToolRegistry::new()),
    );
    let snapshot = runtime.start_delegate(
        "slow child",
        None,
        AgentRole::Coder,
        AgentRunMode::Foreground,
        DelegateContext::None,
        AgentPathKind::Root,
    );
    let agent_id = snapshot.id.clone();
    let run = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            runtime
                .run_started_child_turn(deps, snapshot, DelegateContext::None, |_| {})
                .await
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let cancelled = runtime.cancel_agent(&agent_id).expect("agent cancels");
    assert_eq!(cancelled.state, AgentLifecycleState::Cancelled);

    let output = tokio::time::timeout(std::time::Duration::from_secs(2), run)
        .await
        .expect("child run should stop after interrupt")
        .expect("join should succeed");
    assert_eq!(output.snapshot.state, AgentLifecycleState::Cancelled);
    assert!(
        !output
            .snapshot
            .activity
            .iter()
            .any(|entry| matches!(&entry.kind, AgentActivityKind::Text { text, .. } if text.contains("should not arrive")))
    );
}

#[test]
fn cancel_swarm_preserves_completed_canonical_child_when_swarm_snapshot_is_stale() {
    use neo_agent_core::multi_agent::{SwarmChildSnapshot, SwarmSnapshot};

    let runtime = MultiAgentRuntime::new();
    let swarm_id = runtime.new_swarm_id();
    let first = runtime.start_delegate(
        "already finished",
        Some("finished"),
        AgentRole::Coder,
        AgentRunMode::Foreground,
        neo_agent_core::multi_agent::DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    let second = runtime.start_delegate(
        "still running",
        Some("running"),
        AgentRole::Coder,
        AgentRunMode::Foreground,
        neo_agent_core::multi_agent::DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    let stale_swarm = SwarmSnapshot {
        swarm_id: swarm_id.clone(),
        description: "stale swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: AgentLifecycleState::Running,
        max_concurrency: 2,
        aggregate: SwarmAggregate::from_states([
            AgentLifecycleState::Running,
            AgentLifecycleState::Running,
        ]),
        children: vec![
            SwarmChildSnapshot {
                item_index: 0,
                item: "first".to_owned(),
                agent: first.clone(),
            },
            SwarmChildSnapshot {
                item_index: 1,
                item: "second".to_owned(),
                agent: second.clone(),
            },
        ],
    };
    runtime.register_swarm(stale_swarm);
    let _ = runtime.complete_delegate_for_test(&first.id, "finished before interrupt");

    let cancelled = runtime
        .cancel_swarm(&swarm_id)
        .expect("stale running swarm should cancel unfinished children");

    assert_eq!(
        runtime
            .agent_snapshot(first.id.as_str())
            .expect("first agent")
            .state,
        AgentLifecycleState::Completed
    );
    assert_eq!(
        runtime
            .agent_snapshot(second.id.as_str())
            .expect("second agent")
            .state,
        AgentLifecycleState::Cancelled
    );
    assert_eq!(cancelled.aggregate.completed, 1);
    assert_eq!(cancelled.aggregate.cancelled, 1);
}

struct DelayedTurnModel {
    turns: Mutex<Vec<Vec<DelayedStep>>>,
}

impl DelayedTurnModel {
    fn new(turns: Vec<Vec<DelayedStep>>) -> Self {
        let mut turns = turns;
        turns.reverse();
        Self {
            turns: Mutex::new(turns),
        }
    }
}

enum DelayedStep {
    Event(AiStreamEvent),
    Delay(std::time::Duration),
}

impl ModelClient for DelayedTurnModel {
    fn stream_chat(
        &self,
        _request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        let steps = self
            .turns
            .lock()
            .expect("turns lock poisoned")
            .pop()
            .unwrap_or_default();
        stream::unfold(steps.into_iter(), |mut steps| async move {
            loop {
                match steps.next()? {
                    DelayedStep::Event(event) => return Some((Ok(event), steps)),
                    DelayedStep::Delay(duration) => tokio::time::sleep(duration).await,
                }
            }
        })
        .boxed()
    }
}
