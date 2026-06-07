use neo_ai::{
    CacheRetention, ChatMessage, ChatRequest, ModelRegistry, ProviderRegistry, RequestMetadata,
    RequestOptions,
};

fn main() {
    let registry = ModelRegistry::seeded();
    let providers = ProviderRegistry::production();

    let model = registry
        .get("openai", "gpt-4.1")
        .expect("seeded OpenAI model")
        .clone();
    let provider = providers
        .get(&model.provider.0)
        .expect("production provider");
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
        "{} messages for {}/{} via {:?}",
        request.messages.len(),
        provider.id,
        request.model.model,
        provider.api
    );
}
