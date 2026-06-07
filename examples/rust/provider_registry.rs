use neo_ai::{
    ApiKind, CacheRetention, ChatMessage, ChatRequest, ModelCapabilities, ModelRegistry,
    ModelSpec, ProviderId, RequestMetadata, RequestOptions,
};

fn main() {
    let mut registry = ModelRegistry::new();
    registry.register(ModelSpec {
        provider: ProviderId("fake".to_owned()),
        model: "fake".to_owned(),
        api: ApiKind::Local,
        capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(8192),
    });

    let model = registry.default_model().expect("registered model").clone();
    let request = ChatRequest {
        model,
        messages: vec![ChatMessage::User {
            content: vec![neo_ai::ContentPart::Text {
                text: "hello".to_owned(),
            }],
        }],
        tools: Vec::new(),
        options: RequestOptions {
            temperature: Some(0.2),
            max_tokens: Some(256),
            cache: CacheRetention::Short,
            metadata: RequestMetadata::from_pairs([("source", "example")]),
            ..RequestOptions::default()
        },
    };

    println!(
        "{} messages for {}",
        request.messages.len(),
        request.model.model
    );
}
