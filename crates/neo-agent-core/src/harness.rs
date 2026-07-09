use std::sync::Arc;
use std::sync::Mutex;

use futures::{StreamExt, stream};
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, ChatRequest, ModelCapabilities, ModelClient, ModelSpec,
    ProviderId,
};

#[derive(Clone)]
pub struct FakeHarness {
    model: ModelSpec,
    client: Arc<RecordingFakeModelClient>,
}

impl FakeHarness {
    #[must_use]
    pub fn from_events(events: impl IntoIterator<Item = AiStreamEvent>) -> Self {
        Self::from_turns([events.into_iter().collect::<Vec<_>>()])
    }

    #[must_use]
    pub fn from_turns(turns: impl IntoIterator<Item = Vec<AiStreamEvent>>) -> Self {
        Self::from_result_turns(
            turns
                .into_iter()
                .map(|turn| turn.into_iter().map(Ok).collect()),
        )
    }

    #[must_use]
    pub fn from_result_turns(
        turns: impl IntoIterator<Item = Vec<Result<AiStreamEvent, AiError>>>,
    ) -> Self {
        Self {
            model: fake_model(),
            client: Arc::new(RecordingFakeModelClient::new(turns.into_iter().collect())),
        }
    }

    #[must_use]
    pub fn model(&self) -> ModelSpec {
        self.model.clone()
    }

    #[must_use]
    pub fn client(&self) -> Arc<dyn ModelClient> {
        self.client.clone()
    }

    #[must_use]
    pub fn requests(&self) -> Vec<neo_ai::ChatRequest> {
        self.client.requests()
    }
}

struct RecordingFakeModelClient {
    turns: Mutex<Vec<Vec<Result<AiStreamEvent, AiError>>>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl RecordingFakeModelClient {
    fn new(turns: Vec<Vec<Result<AiStreamEvent, AiError>>>) -> Self {
        let mut turns = turns;
        turns.reverse();
        Self {
            turns: Mutex::new(turns),
            requests: Mutex::default(),
        }
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().expect("request lock poisoned").clone()
    }
}

impl ModelClient for RecordingFakeModelClient {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        let events = self
            .turns
            .lock()
            .expect("turn lock poisoned")
            .pop()
            .unwrap_or_default();
        stream::iter(events).boxed()
    }
}

#[must_use]
pub fn fake_model() -> ModelSpec {
    ModelSpec {
        provider: ProviderId("fake".to_owned()),
        model: "fake-agent-model".to_owned(),
        api: ApiKind::Local,
        capabilities: ModelCapabilities {
            streaming: true,
            tools: true,
            images: false,
            reasoning: neo_ai::ReasoningCapability::None,
            embeddings: false,
            max_context_tokens: None,
            max_output_tokens: None,
        },
    }
}
