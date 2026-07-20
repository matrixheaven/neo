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

    let configured = registry
        .credential_status_from(
            "google",
            &BTreeMap::from([("GOOGLE_API_KEY".to_owned(), "secret".to_owned())]),
        )
        .expect("google provider should exist");
    assert!(configured.configured);
    assert_eq!(configured.env_keys, vec!["GOOGLE_API_KEY"]);
    assert!(!format!("{configured:?}").contains("secret"));
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
