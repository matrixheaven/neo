use std::collections::BTreeMap;

use neo_ai::{
    AiError, ApiKind, ApiType, CredentialResolver, CredentialSource, ModelCapabilities, ModelSpec,
    ProviderId, registry::ProviderRegistry,
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
fn production_registry_excludes_unsupported_bedrock() {
    let registry = ProviderRegistry::production();
    assert!(registry.get("amazon-bedrock").is_none());
}

#[test]
fn credential_resolver_prefers_cli_env_then_auth_file_without_leaking_values() {
    let resolver = CredentialResolver::new("openai")
        .with_cli_api_key(Some("cli-secret".to_owned()))
        .with_env(
            ["OPENAI_API_KEY"],
            &BTreeMap::from([("OPENAI_API_KEY".to_owned(), "env-secret".to_owned())]),
        )
        .with_auth_file_credentials(BTreeMap::from([(
            "openai".to_owned(),
            "auth-file-secret".to_owned(),
        )]));

    let credential = resolver.resolve().expect("credential should resolve");
    assert_eq!(credential.secret(), "cli-secret");
    assert_eq!(credential.source(), CredentialSource::Cli);
    assert_eq!(credential.redacted_label(), "cli --api-key");
    assert!(!format!("{credential:?}").contains("cli-secret"));
    assert!(!format!("{credential:?}").contains("env-secret"));
    assert!(!format!("{credential:?}").contains("auth-file-secret"));

    let auth_file = CredentialResolver::new("openai")
        .with_auth_file_credentials(BTreeMap::from([(
            "openai".to_owned(),
            "auth-file-secret".to_owned(),
        )]))
        .resolve()
        .expect("auth-file credential should resolve");
    assert_eq!(auth_file.secret(), "auth-file-secret");
    assert_eq!(auth_file.source(), CredentialSource::AuthFile);
}

#[cfg(windows)]
#[test]
fn credential_resolver_matches_environment_names_case_insensitively_on_windows() {
    let credential = CredentialResolver::new("openai")
        .with_env(
            ["OPENAI_API_KEY"],
            &BTreeMap::from([("openai_api_key".to_owned(), "secret".to_owned())]),
        )
        .resolve()
        .expect("Windows environment names are case-insensitive");

    assert_eq!(credential.secret(), "secret");
}

#[cfg(not(windows))]
#[test]
fn credential_resolver_matches_environment_names_exactly_on_unix() {
    let credential = CredentialResolver::new("openai")
        .with_env(
            ["OPENAI_API_KEY"],
            &BTreeMap::from([("openai_api_key".to_owned(), "secret".to_owned())]),
        )
        .resolve();

    assert!(credential.is_none());
}

#[test]
fn provider_resolver_builds_clients_from_registered_provider_types() {
    let registry = ProviderRegistry::production();
    let env = BTreeMap::from([
        ("OPENAI_API_KEY".to_owned(), "openai-key".to_owned()),
        ("ANTHROPIC_API_KEY".to_owned(), "anthropic-key".to_owned()),
        ("GEMINI_API_KEY".to_owned(), "google-key".to_owned()),
        ("OPENROUTER_API_KEY".to_owned(), "openrouter-key".to_owned()),
    ]);
    let resolver = registry.resolver_from(env);

    resolver
        .resolve(&model("openai", "gpt-test", ApiKind::OpenAiResponse))
        .expect("openai responses client should resolve");
    resolver
        .resolve(&model("openai", "gpt-chat-test", ApiKind::OpenAi))
        .expect("openai chat completions client should resolve");
    resolver
        .resolve(&model(
            "anthropic",
            "claude-test",
            ApiKind::AnthropicMessages,
        ))
        .expect("anthropic messages client should resolve");
    resolver
        .resolve(&model("google", "gemini-test", ApiKind::GoogleGenerativeAi))
        .expect("google generative ai client should resolve");
    resolver
        .resolve(&model("openrouter", "openrouter-test", ApiKind::OpenAi))
        .expect("openrouter compatible client should resolve");
    resolver
        .resolve(&model(
            "openrouter",
            "openrouter-chat-test",
            ApiKind::OpenAi,
        ))
        .expect("openrouter chat completions client should resolve");
}

#[test]
fn provider_resolver_uses_provider_type_as_wire_identity() {
    let registry = ProviderRegistry::production();
    let env = BTreeMap::from([("OPENAI_API_KEY".to_owned(), "openai-key".to_owned())]);
    let resolver = registry.resolver_from(env);
    let result = resolver.resolve(&model("openai", "some-model", ApiKind::AnthropicMessages));
    assert!(
        result.is_ok(),
        "model catalog metadata must not override the provider wire type"
    );
}

#[test]
fn production_registry_includes_google_generative_ai_credentials() {
    let registry = ProviderRegistry::production();
    let google = registry
        .get("google")
        .expect("google provider should exist");

    assert_eq!(google.display_name, "Google Generative AI");
    assert_eq!(google.provider_type, ApiType::Google);
    assert_eq!(
        google.base_url.as_deref(),
        Some("https://generativelanguage.googleapis.com/v1beta")
    );
    assert_eq!(
        google.api_key_env_vars,
        vec!["GEMINI_API_KEY", "GOOGLE_API_KEY"]
    );
}

#[test]
fn provider_resolver_rejects_missing_credentials_and_test_only_fake() {
    let registry = ProviderRegistry::production();
    let resolver = registry.resolver_from(BTreeMap::new());

    let Err(missing) = resolver.resolve(&model("openai", "gpt-test", ApiKind::OpenAiResponse))
    else {
        panic!("missing credentials should fail");
    };
    assert!(matches!(missing, AiError::Configuration { message: _ }));
    assert!(missing.to_string().contains("OPENAI_API_KEY"));

    let Err(fake) = resolver.resolve(&model("fake", "test-model", ApiKind::Local)) else {
        panic!("production registry must not resolve fake clients");
    };
    assert!(fake.to_string().contains("provider fake is not registered"));
}
