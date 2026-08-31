//! Auxiliary request executor fixed points: reasoning plan application and
//! the disable-rejection retry contract.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use futures::{StreamExt, stream};
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, ChatMessage, ChatRequest, ContentPart, ModelCapabilities,
    ModelClient, ModelSpec, ProviderId, ReasoningCapability, ReasoningEffort, ReasoningSelection,
    RequestOptions,
};

use super::aux_stream_text;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct ScriptedClient {
    responses: Arc<Mutex<VecDeque<Result<Vec<AiStreamEvent>, AiError>>>>,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
}

impl ScriptedClient {
    fn scripted(responses: Vec<Result<Vec<AiStreamEvent>, AiError>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            requests: Arc::default(),
        }
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl ModelClient for ScriptedClient {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        self.requests.lock().expect("request lock").push(request);
        let outcome = self
            .responses
            .lock()
            .expect("response lock")
            .pop_front()
            .expect("scripted outcome for each attempt");
        match outcome {
            Ok(events) => stream::iter(events.into_iter().map(Ok)).boxed(),
            Err(error) => stream::iter(vec![Err(error)]).boxed(),
        }
    }
}

fn model(capability: ReasoningCapability) -> ModelSpec {
    ModelSpec {
        provider: ProviderId("test".to_owned()),
        model: "aux-model".to_owned(),
        api: ApiKind::OpenAi,
        capabilities: ModelCapabilities {
            reasoning: capability,
            ..ModelCapabilities::tool_chat()
        },
    }
}

fn effort_capability(disable_supported: bool) -> ReasoningCapability {
    ReasoningCapability::Effort {
        values: vec![ReasoningEffort::low(), ReasoningEffort::high()],
        disable_supported,
    }
}

fn messages() -> Vec<ChatMessage> {
    vec![ChatMessage::User {
        content: vec![ContentPart::Text {
            text: "summarize".to_owned(),
        }],
    }]
}

fn options() -> RequestOptions {
    RequestOptions {
        temperature: Some(0.2),
        max_tokens: Some(512),
        ..RequestOptions::default()
    }
}

fn text_delta(value: &str) -> Result<Vec<AiStreamEvent>, AiError> {
    Ok(vec![AiStreamEvent::TextDelta {
        text: value.to_owned(),
    }])
}

fn protocol_rejection() -> AiError {
    AiError::Protocol {
        message: "http status 400: This model always engages in thinking".to_owned(),
    }
}

#[tokio::test]
async fn disable_rejection_retries_once_with_cheapest_effort() {
    let client = ScriptedClient::scripted(vec![Err(protocol_rejection()), text_delta("hello")]);
    let spec = model(effort_capability(true));

    let text = aux_stream_text(
        &client,
        &spec,
        messages(),
        options(),
        &neo_ai::AuxReasoning::Auto,
        None,
        None,
    )
    .await
    .expect("retry succeeds");

    assert_eq!(text, "hello");
    let requests = client.requests();
    assert_eq!(requests.len(), 2, "exactly one disable-rejection retry");
    assert!(
        requests[0].options.disable_reasoning,
        "auto + disable capability sends the explicit disable first"
    );
    assert_eq!(requests[0].options.reasoning, ReasoningSelection::Off);
    assert_eq!(
        requests[1].options.reasoning,
        ReasoningSelection::Effort {
            effort: ReasoningEffort::low()
        },
        "retry carries the cheapest declared effort"
    );
    assert!(!requests[1].options.disable_reasoning);
    assert_eq!(requests[1].options.temperature, Some(0.2));
}

#[tokio::test]
async fn protocol_error_without_disable_sent_does_not_retry() {
    let client = ScriptedClient::scripted(vec![Err(protocol_rejection())]);
    let spec = model(effort_capability(false));

    let error = aux_stream_text(
        &client,
        &spec,
        messages(),
        options(),
        &neo_ai::AuxReasoning::Auto,
        None,
        None,
    )
    .await
    .expect_err("protocol error surfaces");

    assert!(matches!(error, AiError::Protocol { .. }));
    assert_eq!(
        client.requests().len(),
        1,
        "no disable was sent, so there is nothing to retry"
    );
}

#[tokio::test]
async fn non_protocol_error_after_disable_does_not_retry() {
    let client = ScriptedClient::scripted(vec![Err(AiError::RateLimit {
        message: "slow down".to_owned(),
        retry_after: None,
    })]);
    let spec = model(effort_capability(true));

    let error = aux_stream_text(
        &client,
        &spec,
        messages(),
        options(),
        &neo_ai::AuxReasoning::Auto,
        None,
        None,
    )
    .await
    .expect_err("rate limit surfaces");

    assert!(matches!(error, AiError::RateLimit { .. }));
    assert_eq!(client.requests().len(), 1);
}

#[tokio::test]
async fn second_failure_after_retry_surfaces_the_error() {
    let client =
        ScriptedClient::scripted(vec![Err(protocol_rejection()), Err(protocol_rejection())]);
    let spec = model(effort_capability(true));

    let result = aux_stream_text(
        &client,
        &spec,
        messages(),
        options(),
        &neo_ai::AuxReasoning::Auto,
        None,
        None,
    )
    .await;

    assert!(result.is_err());
    assert_eq!(client.requests().len(), 2, "retry happens exactly once");
}

#[tokio::test]
async fn cancelled_token_aborts_the_aux_stream_without_retry() {
    let client = ScriptedClient::scripted(vec![text_delta("ignored")]);
    let spec = model(effort_capability(true));
    let token = CancellationToken::new();
    token.cancel();

    let error = aux_stream_text(
        &client,
        &spec,
        messages(),
        options(),
        &neo_ai::AuxReasoning::Auto,
        Some(&token),
        None,
    )
    .await
    .expect_err("cancelled requests surface Cancelled");

    assert!(matches!(error, AiError::Cancelled));
    assert_eq!(client.requests().len(), 1, "no retry after cancellation");
}
