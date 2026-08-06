use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AgentRuntimeError, Tool,
    ToolContext, ToolFuture, ToolResult, harness::FakeHarness,
};
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, ChatRequest, MessagePhase, ModelCapabilities, ModelClient,
    ModelSpec, ProviderId, ReasoningCapability,
};
use serde_json::json;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::sleep;

pub(crate) async fn collect_turn_events(
    harness: &FakeHarness,
    config: AgentConfig,
    context: &mut AgentContext,
    input: AgentMessage,
) -> Vec<AgentEvent> {
    let runtime = AgentRuntime::new(config, harness.client());
    runtime
        .run_turn(context, input)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed")
}

pub(crate) async fn assert_runtime_rejects_unsupported_capability(
    config: AgentConfig,
    harness: &FakeHarness,
    message: AgentMessage,
    expected_substring: &str,
    expectation: &str,
) {
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();
    let error = runtime
        .run_turn(&mut context, message)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect_err(expectation);

    assert!(matches!(
        error,
        AgentRuntimeError::Model(AiError::Configuration { message: _ })
    ));
    assert!(
        error.to_string().contains(expected_substring),
        "expected {expected_substring:?}, got {error}"
    );
    assert!(
        harness.requests().is_empty(),
        "request should not reach provider"
    );
}

pub(crate) fn text_turn_events(id: &str, text: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: id.to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]
}

pub(crate) fn echo_tool_harness(text: &str) -> FakeHarness {
    FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": text }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ])
}

pub(crate) fn final_done_turn() -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_2".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]
}

pub(crate) struct EchoTool;

impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Echo text."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
            "required": ["text"]
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            Ok(ToolResult::ok(
                input
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            ))
        })
    }
}

pub(crate) struct RecordingEchoTool {
    pub(crate) executed: Arc<Mutex<Vec<String>>>,
}

impl Tool for RecordingEchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Record and echo text."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
            "required": ["text"]
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let text = input
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            self.executed
                .lock()
                .expect("executed lock poisoned")
                .push(text.clone());
            Ok(ToolResult::ok(text))
        })
    }
}

#[derive(Clone)]
pub(crate) struct DelayedHarness {
    pub(crate) model: ModelSpec,
    pub(crate) client: Arc<DelayedModelClient>,
}

pub(crate) fn model_with_capabilities(capabilities: ModelCapabilities) -> ModelSpec {
    ModelSpec {
        provider: ProviderId("capability-test".to_owned()),
        model: "capability-test-model".to_owned(),
        api: ApiKind::Local,
        capabilities,
    }
}

impl DelayedHarness {
    pub(crate) fn new(steps: Vec<DelayedStep>) -> Self {
        Self::from_turns([steps])
    }

    pub(crate) fn from_turns(turns: impl IntoIterator<Item = Vec<DelayedStep>>) -> Self {
        Self {
            model: ModelSpec {
                provider: ProviderId("delayed".to_owned()),
                model: "delayed-agent-model".to_owned(),
                api: ApiKind::Local,
                capabilities: ModelCapabilities {
                    streaming: true,
                    tools: true,
                    images: false,
                    reasoning: ReasoningCapability::None,
                    embeddings: false,
                    max_context_tokens: None,
                    max_output_tokens: None,
                },
            },
            client: Arc::new(DelayedModelClient {
                steps: Mutex::new(turns.into_iter().collect()),
                requests: Mutex::new(Vec::new()),
            }),
        }
    }

    pub(crate) fn model(&self) -> ModelSpec {
        self.model.clone()
    }

    pub(crate) fn client(&self) -> Arc<dyn ModelClient> {
        self.client.clone()
    }

    pub(crate) fn requests(&self) -> Vec<ChatRequest> {
        self.client
            .requests
            .lock()
            .expect("request lock poisoned")
            .clone()
    }
}

#[derive(Clone)]
pub(crate) enum DelayedStep {
    Event(AiStreamEvent),
    Delay(Duration),
}

pub(crate) struct DelayedModelClient {
    pub(crate) steps: Mutex<VecDeque<Vec<DelayedStep>>>,
    pub(crate) requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for DelayedModelClient {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        let steps = self
            .steps
            .lock()
            .expect("steps lock poisoned")
            .pop_front()
            .unwrap_or_default();
        futures::stream::unfold(steps.into_iter(), |mut steps| async move {
            loop {
                match steps.next()? {
                    DelayedStep::Event(event) => return Some((Ok(event), steps)),
                    DelayedStep::Delay(duration) => sleep(duration).await,
                }
            }
        })
        .boxed()
    }
}

pub(crate) fn end_turn_events(text: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]
}

pub(crate) async fn run_turn_collect(
    runtime: &AgentRuntime,
    context: &mut AgentContext,
    input: &str,
) -> Vec<AgentEvent> {
    runtime
        .run_turn(context, AgentMessage::user_text(input))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed")
}

pub(crate) fn tool_call_turn(calls: &[(&str, &str, serde_json::Value)]) -> Vec<AiStreamEvent> {
    let mut events = vec![AiStreamEvent::MessageStart {
        phase: MessagePhase::Unknown,
        id: "msg_tools".to_owned(),
    }];
    for (id, name, arguments) in calls {
        events.push(AiStreamEvent::ToolCallStart {
            id: (*id).to_owned(),
            name: (*name).to_owned(),
        });
        events.push(AiStreamEvent::ToolCallEnd {
            id: (*id).to_owned(),
            raw_arguments: arguments.to_string(),
        });
    }
    events.push(AiStreamEvent::MessageEnd {
        phase: MessagePhase::Unknown,
        stop_reason: neo_ai::StopReason::ToolUse,
        usage: None,
    });
    events
}
