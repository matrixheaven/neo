use std::collections::BTreeMap;

use neo_ai::{
    AiError, ApiKind, ModelCapabilities, ModelSpec, ProviderId, registry::ProviderRegistry,
};

fn model(provider: &str, name: &str, api: ApiKind) -> ModelSpec {
    ModelSpec {
        provider: ProviderId(provider.to_owned()),
        model: name.to_owned(),
        api,
        capabilities: ModelCapabilities::tool_chat(),
    }
}

#[test]
fn provider_registry_reports_credentials_without_secret_values() {
    let registry = ProviderRegistry::production();
    let env = BTreeMap::from([
        ("OPENAI_API_KEY".to_owned(), "sk-secret".to_owned()),
        (
            "ANTHROPIC_API_KEY".to_owned(),
            "anthropic-secret".to_owned(),
        ),
        ("AWS_PROFILE".to_owned(), "ambient-profile".to_owned()),
    ]);

    let openai = registry
        .credential_status_from("openai", &env)
        .expect("openai provider should exist");
    assert!(openai.configured);
    assert_eq!(openai.env_keys, vec!["OPENAI_API_KEY"]);
    assert_eq!(openai.authenticated_label, None);
    assert!(!format!("{openai:?}").contains("sk-secret"));

    let bedrock = registry
        .credential_status_from("amazon-bedrock", &env)
        .expect("bedrock provider should exist");
    assert!(bedrock.configured);
    assert!(bedrock.env_keys.is_empty());
    assert_eq!(
        bedrock.authenticated_label.as_deref(),
        Some("<authenticated>")
    );
    assert!(!format!("{bedrock:?}").contains("ambient-profile"));
}

#[test]
fn provider_resolver_builds_real_clients_by_model_api() {
    let registry = ProviderRegistry::production();
    let env = BTreeMap::from([
        ("OPENAI_API_KEY".to_owned(), "openai-key".to_owned()),
        ("ANTHROPIC_API_KEY".to_owned(), "anthropic-key".to_owned()),
    ]);
    let resolver = registry.resolver_from(env);

    resolver
        .resolve(&model("openai", "gpt-test", ApiKind::OpenAiResponses))
        .expect("openai responses client should resolve");
    resolver
        .resolve(&model(
            "openai",
            "gpt-chat-test",
            ApiKind::OpenAiChatCompletions,
        ))
        .expect("openai chat completions client should resolve");
    resolver
        .resolve(&model(
            "anthropic",
            "claude-test",
            ApiKind::AnthropicMessages,
        ))
        .expect("anthropic messages client should resolve");
}

#[test]
fn provider_resolver_rejects_missing_credentials_and_test_only_fake() {
    let registry = ProviderRegistry::production();
    let resolver = registry.resolver_from(BTreeMap::new());

    let Err(missing) = resolver.resolve(&model("openai", "gpt-test", ApiKind::OpenAiResponses))
    else {
        panic!("missing credentials should fail");
    };
    assert!(matches!(missing, AiError::Configuration(_)));
    assert!(missing.to_string().contains("OPENAI_API_KEY"));

    let Err(fake) = resolver.resolve(&model("fake", "test-model", ApiKind::Local)) else {
        panic!("production registry must not resolve fake clients");
    };
    assert!(fake.to_string().contains("provider fake is not registered"));
}
