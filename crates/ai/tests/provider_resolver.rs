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
    assert_eq!(openai.missing_reason, None);
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
    assert_eq!(bedrock.missing_reason, None);
    assert!(!format!("{bedrock:?}").contains("ambient-profile"));
}

#[test]
fn provider_registry_reports_explicit_missing_credential_reasons() {
    let registry = ProviderRegistry::production();
    let env = BTreeMap::from([("OPENAI_API_KEY".to_owned(), String::new())]);

    let openai = registry
        .credential_status_from("openai", &env)
        .expect("openai provider should exist");

    assert!(!openai.configured);
    assert!(openai.env_keys.is_empty());
    assert_eq!(
        openai.missing_reason.as_deref(),
        Some("missing OPENAI_API_KEY")
    );

    let bedrock = registry
        .credential_status_from("amazon-bedrock", &BTreeMap::new())
        .expect("bedrock provider should exist");

    assert!(!bedrock.configured);
    assert_eq!(
        bedrock.missing_reason.as_deref(),
        Some(
            "missing one of: AWS_PROFILE; AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY; AWS_BEARER_TOKEN_BEDROCK; AWS_CONTAINER_CREDENTIALS_RELATIVE_URI; AWS_CONTAINER_CREDENTIALS_FULL_URI; AWS_WEB_IDENTITY_TOKEN_FILE"
        )
    );
}

#[test]
fn provider_registry_accepts_configured_environment_key_names_without_secret_storage() {
    let mut registry = ProviderRegistry::production();
    let mut provider = registry.get("openai").cloned().expect("openai provider");
    provider.api_key_env_vars = vec!["PROJECT_OPENAI_KEY".to_owned()];
    registry.register(provider);

    let configured = registry
        .credential_status_from(
            "openai",
            &BTreeMap::from([("PROJECT_OPENAI_KEY".to_owned(), "secret".to_owned())]),
        )
        .expect("openai provider should exist");
    assert!(configured.configured);
    assert_eq!(configured.env_keys, vec!["PROJECT_OPENAI_KEY"]);
    assert!(!format!("{configured:?}").contains("secret"));

    let missing = registry
        .credential_status_from("openai", &BTreeMap::new())
        .expect("openai provider should exist");
    assert!(!missing.configured);
    assert_eq!(
        missing.missing_reason.as_deref(),
        Some("missing PROJECT_OPENAI_KEY")
    );
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
