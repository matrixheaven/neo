use std::collections::BTreeMap;

use neo_ai::{
    AiError, ApiKind, CredentialResolver, CredentialSource, ModelCapabilities, ModelSpec,
    ProviderId,
    registry::{ProviderRegistry, ProviderSpec},
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

#[test]
fn provider_resolver_builds_real_clients_by_model_api() {
    let registry = ProviderRegistry::production();
    let env = BTreeMap::from([
        ("OPENAI_API_KEY".to_owned(), "openai-key".to_owned()),
        ("ANTHROPIC_API_KEY".to_owned(), "anthropic-key".to_owned()),
        ("GEMINI_API_KEY".to_owned(), "google-key".to_owned()),
        ("OPENROUTER_API_KEY".to_owned(), "openrouter-key".to_owned()),
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
    resolver
        .resolve(&model("google", "gemini-test", ApiKind::GoogleGenerativeAi))
        .expect("google generative ai client should resolve");
    resolver
        .resolve(&model(
            "openrouter",
            "openrouter-test",
            ApiKind::OpenAiCompatible,
        ))
        .expect("openrouter compatible client should resolve");
    resolver
        .resolve(&model(
            "openrouter",
            "openrouter-chat-test",
            ApiKind::OpenAiChatCompletions,
        ))
        .expect("openrouter chat completions client should resolve");
}

#[test]
fn provider_resolver_rejects_model_api_mismatches() {
    let mut registry = ProviderRegistry::production();
    registry.register(ProviderSpec {
        id: "untyped-provider".to_owned(),
        display_name: "Untyped".to_owned(),
        api: ApiKind::OpenAiResponses,
        supported_apis: vec![ApiKind::OpenAiResponses],
        base_url: Some("https://api.example.com/v1".to_owned()),
        api_key: None,
        api_key_env_vars: vec!["UNTYPED_KEY".to_owned()],
        ambient_auth_env_vars: vec![],
        provider_type: None,
    });
    let env = BTreeMap::from([("UNTYPED_KEY".to_owned(), "key".to_owned())]);
    let resolver = registry.resolver_from(env);

    let Err(missing_type) = resolver.resolve(&model(
        "untyped-provider",
        "bad-claude",
        ApiKind::AnthropicMessages,
    )) else {
        panic!("untyped provider must be rejected");
    };
    assert!(matches!(missing_type, AiError::Configuration { message: _ }));
    assert!(
        missing_type
            .to_string()
            .contains("provider untyped-provider must declare a provider type")
    );

    // Provider type selects the wire client regardless of the model's api field.
    let registry2 = ProviderRegistry::production();
    let env2 = BTreeMap::from([("OPENAI_API_KEY".to_owned(), "openai-key".to_owned())]);
    let resolver2 = registry2.resolver_from(env2);
    let result = resolver2.resolve(&model("openai", "some-model", ApiKind::AnthropicMessages));
    assert!(
        result.is_ok(),
        "provider_type should override model.api mismatch"
    );
}

#[test]
fn provider_resolver_reports_api_mismatch_before_credential_lookup() {
    let mut registry = ProviderRegistry::production();
    registry.register(ProviderSpec {
        id: "untyped-provider".to_owned(),
        display_name: "Untyped".to_owned(),
        api: ApiKind::OpenAiResponses,
        supported_apis: vec![ApiKind::OpenAiResponses],
        base_url: Some("https://api.example.com/v1".to_owned()),
        api_key: None,
        api_key_env_vars: vec!["UNTYPED_KEY".to_owned()],
        ambient_auth_env_vars: vec![],
        provider_type: None,
    });
    let resolver = registry.resolver_from(BTreeMap::new());

    let Err(mismatch) = resolver.resolve(&model(
        "untyped-provider",
        "bad-claude",
        ApiKind::AnthropicMessages,
    )) else {
        panic!("api mismatch should fail before credential lookup");
    };

    assert!(
        mismatch
            .to_string()
            .contains("provider untyped-provider must declare a provider type")
    );
    assert!(!mismatch.to_string().contains("UNTYPED_KEY"));
}

#[test]
fn production_registry_includes_google_generative_ai_credentials() {
    let registry = ProviderRegistry::production();
    let google = registry
        .get("google")
        .expect("google provider should exist");

    assert_eq!(google.display_name, "Google Generative AI");
    assert_eq!(google.api, ApiKind::GoogleGenerativeAi);
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

    let Err(missing) = resolver.resolve(&model("openai", "gpt-test", ApiKind::OpenAiResponses))
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
