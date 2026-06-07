use futures::StreamExt;
use neo_ai::{
    AiStreamEvent, ApiKind, ChatRequest, ModelCapabilities, ModelClient, ModelSpec, ProviderId,
    StopReason, providers::fake::FakeModelClient,
};

fn request() -> ChatRequest {
    ChatRequest {
        model: ModelSpec {
            provider: ProviderId("fake".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::Local,
            capabilities: ModelCapabilities::tool_chat(),
        },
        messages: Vec::new(),
        tools: Vec::new(),
        temperature: None,
        max_tokens: None,
    }
}

#[tokio::test]
async fn fake_model_client_streams_events_and_records_requests() {
    let events = vec![
        AiStreamEvent::MessageStart {
            id: "msg-1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "hello".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ];
    let client = FakeModelClient::new(events.clone());
    let request = request();

    let streamed = client
        .stream_chat(request.clone())
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("fake stream should not fail");

    assert_eq!(streamed, events);
    assert_eq!(client.requests(), vec![request]);
}
