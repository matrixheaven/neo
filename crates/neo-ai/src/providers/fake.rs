use std::sync::{Arc, Mutex};

use futures::{StreamExt, stream};

use crate::{
    AiError, AiStreamEvent, ChatRequest, MediaTransportCapabilities, MediaTransportMode,
    ModelClient,
};

#[derive(Clone, Default)]
pub struct FakeModelClient {
    events: Arc<Mutex<Vec<AiStreamEvent>>>,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
}

impl FakeModelClient {
    #[must_use]
    pub fn new(events: Vec<AiStreamEvent>) -> Self {
        Self {
            events: Arc::new(Mutex::new(events)),
            requests: Arc::default(),
        }
    }

    #[must_use]
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().expect("request lock poisoned").clone()
    }
}

impl ModelClient for FakeModelClient {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        let events = self.events.lock().expect("events lock poisoned").clone();
        stream::iter(events.into_iter().map(Ok)).boxed()
    }

    fn media_transport(&self) -> MediaTransportCapabilities {
        // Test double: accepts any media at any position so deterministic
        // tests exercise projection without a real wire protocol.
        MediaTransportCapabilities {
            user_image: MediaTransportMode::Inline,
            user_video: MediaTransportMode::Inline,
            tool_image: MediaTransportMode::InPlace,
            tool_video: MediaTransportMode::InPlace,
        }
    }
}
