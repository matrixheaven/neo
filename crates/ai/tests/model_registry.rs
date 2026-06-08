use neo_ai::{ApiKind, ModelCapabilities, ModelRegistry, ModelSpec, ProviderId};

fn model(provider: &str, name: &str, capabilities: ModelCapabilities) -> ModelSpec {
    ModelSpec {
        provider: ProviderId(provider.to_owned()),
        model: name.to_owned(),
        api: ApiKind::OpenAiCompatible,
        capabilities,
    }
}

#[test]
fn model_capabilities_default_to_text_chat_streaming() {
    let capabilities = ModelCapabilities::default();

    assert!(capabilities.streaming);
    assert!(!capabilities.tools);
    assert!(!capabilities.images);
    assert!(!capabilities.reasoning);
    assert!(!capabilities.embeddings);
    assert_eq!(capabilities.max_context_tokens, None);
}

#[test]
fn model_capabilities_helpers_describe_common_shapes() {
    let chat = ModelCapabilities::chat();
    let tool_chat = ModelCapabilities::tool_chat().with_max_context_tokens(128_000);
    let embedding = ModelCapabilities::embedding();

    assert_eq!(chat, ModelCapabilities::default());
    assert!(tool_chat.streaming);
    assert!(tool_chat.tools);
    assert_eq!(tool_chat.max_context_tokens, Some(128_000));
    assert!(!embedding.streaming);
    assert!(embedding.embeddings);
}

#[test]
fn model_registry_registers_lists_gets_and_tracks_default_model() {
    let mut registry = ModelRegistry::new();
    let haiku = model(
        "anthropic",
        "claude-3-haiku",
        ModelCapabilities::tool_chat(),
    );
    let sonnet = model("anthropic", "claude-3-sonnet", ModelCapabilities::chat());

    registry.register(haiku.clone());
    registry.register(sonnet.clone());

    assert_eq!(registry.list(), vec![haiku.clone(), sonnet.clone()]);
    assert_eq!(registry.get("anthropic", "claude-3-haiku"), Some(&haiku));
    assert_eq!(registry.default_model(), Some(&haiku));
}

#[test]
fn model_registry_replaces_existing_model_without_moving_default() {
    let mut registry = ModelRegistry::new();
    let first = model("local", "tiny", ModelCapabilities::chat());
    let replacement = model("local", "tiny", ModelCapabilities::tool_chat());
    let second = model("local", "large", ModelCapabilities::chat());

    registry.register(first);
    registry.register(second.clone());
    registry.register(replacement.clone());

    assert_eq!(registry.list(), vec![replacement.clone(), second]);
    assert_eq!(registry.default_model(), Some(&replacement));
}

#[test]
fn model_registry_can_seed_common_builtin_chat_models() {
    let registry = ModelRegistry::seeded();

    assert!(registry.get("openai", "gpt-4.1").is_some());
    assert!(registry.get("anthropic", "claude-sonnet-4-5").is_some());
    assert!(registry.get("google", "gemini-2.5-pro").is_some());
    assert_eq!(
        registry.get("openai", "gpt-5-mini").map(|model| &model.api),
        Some(&ApiKind::OpenAiResponses)
    );
    assert_eq!(
        registry.default_model().map(|model| model.model.as_str()),
        Some("gpt-5.4")
    );
}

#[test]
fn model_registry_seed_helper_deduplicates_existing_models() {
    let mut registry = ModelRegistry::new();
    registry.register(model("openai", "gpt-4.1", ModelCapabilities::chat()));

    registry.register_builtin_models();

    let openai_gpt_41 = registry
        .list()
        .into_iter()
        .filter(|model| model.provider.0 == "openai" && model.model == "gpt-4.1")
        .count();
    assert_eq!(openai_gpt_41, 1);
    assert!(registry.get("openai", "gpt-4o-mini").is_some());
}

#[test]
fn model_registry_loads_models_from_json_catalog() {
    let mut registry = ModelRegistry::seeded();

    registry
        .load_catalog_str(
            r#"
{
  "models": [
    {
      "provider": "openrouter",
      "model": "anthropic/claude-sonnet-4.5",
      "api": "OpenAiCompatible",
      "capabilities": {
        "streaming": true,
        "tools": true,
        "images": false,
        "reasoning": true,
        "embeddings": false,
        "max_context_tokens": 200000
      }
    }
  ],
  "default": {
    "provider": "openrouter",
    "model": "anthropic/claude-sonnet-4.5"
  }
}
"#,
            "test catalog",
        )
        .expect("load model catalog");

    let model = registry
        .get("openrouter", "anthropic/claude-sonnet-4.5")
        .expect("catalog model");
    assert_eq!(model.api, ApiKind::OpenAiCompatible);
    assert!(model.capabilities.streaming);
    assert!(model.capabilities.tools);
    assert!(model.capabilities.reasoning);
    assert_eq!(model.capabilities.max_context_tokens, Some(200_000));
    assert_eq!(
        registry.default_model().map(|model| model.model.as_str()),
        Some("anthropic/claude-sonnet-4.5")
    );
}

#[test]
fn model_registry_loads_pi_models_json_custom_models() {
    let mut registry = ModelRegistry::new();

    registry
        .load_catalog_str(
            r#"
{
  "providers": {
    "ollama": {
      "api": "openai-completions",
      "models": [
        {
          "id": "llama3.1:8b",
          "name": "Llama 3.1 8B",
          "input": ["text"],
          "contextWindow": 128000
        },
        {
          "id": "llava:latest",
          "api": "openai-completions",
          "reasoning": true,
          "input": ["text", "image"],
          "contextWindow": 65536
        }
      ]
    }
  }
}
"#,
            "pi models.json",
        )
        .expect("load pi models.json");

    let llama = registry.get("ollama", "llama3.1:8b").expect("llama");
    assert_eq!(llama.api, ApiKind::OpenAiCompatible);
    assert!(llama.capabilities.streaming);
    assert!(llama.capabilities.tools);
    assert!(!llama.capabilities.images);
    assert!(!llama.capabilities.reasoning);
    assert_eq!(llama.capabilities.max_context_tokens, Some(128_000));

    let vision_model = registry.get("ollama", "llava:latest").expect("llava");
    assert_eq!(vision_model.api, ApiKind::OpenAiCompatible);
    assert!(vision_model.capabilities.images);
    assert!(vision_model.capabilities.reasoning);
    assert_eq!(vision_model.capabilities.max_context_tokens, Some(65_536));
    assert_eq!(
        registry.default_model().map(|model| model.model.as_str()),
        Some("llama3.1:8b")
    );
}

#[test]
fn model_registry_rejects_pi_models_json_with_unsupported_api() {
    let mut registry = ModelRegistry::new();

    let error = registry
        .load_catalog_str(
            r#"
{
  "providers": {
    "amazon-bedrock": {
      "api": "bedrock-converse-stream",
      "models": [
        { "id": "anthropic.claude-opus-4-6-v1" }
      ]
    }
  }
}
"#,
            "pi models.json",
        )
        .expect_err("unsupported Pi API should be rejected");

    assert!(error.to_string().contains("unsupported pi models.json api"));
    assert!(error.to_string().contains("bedrock-converse-stream"));
}

#[test]
fn model_registry_rejects_unsupported_pi_provider_metadata() {
    for (field, value) in [
        ("baseUrl", r#""https://proxy.example.test/v1""#),
        ("apiKey", r#""$CUSTOM_PROVIDER_KEY""#),
        ("headers", r#"{"x-custom": "$CUSTOM_HEADER"}"#),
        ("compat", r#"{"supportsDeveloperRole": false}"#),
        ("authHeader", "true"),
        (
            "modelOverrides",
            r#"{"gpt-4.1": {"headers": {"x-route": "bedrock"}}}"#,
        ),
    ] {
        let mut registry = ModelRegistry::new();
        let source = format!(
            r#"
{{
  "providers": {{
    "openrouter": {{
      "api": "openai-completions",
      "{field}": {value},
      "models": [
        {{ "id": "safe-model", "name": "Safe Model" }}
      ]
    }}
  }}
}}
"#
        );

        let error = registry
            .load_catalog_str(&source, "pi models.json")
            .expect_err("unsupported provider metadata should be rejected");

        assert!(error.to_string().contains("provider openrouter"));
        assert!(error.to_string().contains(field));
        assert!(
            error
                .to_string()
                .contains("unsupported pi models.json provider metadata")
        );
    }
}

#[test]
fn model_registry_rejects_unsupported_pi_model_metadata() {
    for (field, value) in [
        ("baseUrl", r#""https://model-route.example.test/v1""#),
        (
            "cost",
            r#"{"input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0}"#,
        ),
        ("maxTokens", "8192"),
        ("headers", r#"{"x-model": "$MODEL_HEADER"}"#),
        ("compat", r#"{"maxTokensField": "max_tokens"}"#),
        ("thinkingLevelMap", r#"{"minimal": null, "high": "max"}"#),
    ] {
        let mut registry = ModelRegistry::new();
        let source = format!(
            r#"
{{
  "providers": {{
    "openrouter": {{
      "api": "openai-completions",
      "models": [
        {{
          "id": "routed-model",
          "name": "Routed Model",
          "{field}": {value}
        }}
      ]
    }}
  }}
}}
"#
        );

        let error = registry
            .load_catalog_str(&source, "pi models.json")
            .expect_err("unsupported model metadata should be rejected");

        assert!(error.to_string().contains("provider openrouter"));
        assert!(error.to_string().contains("model routed-model"));
        assert!(error.to_string().contains(field));
        assert!(
            error
                .to_string()
                .contains("unsupported pi models.json model metadata")
        );
    }
}

#[test]
fn model_registry_rejects_invalid_json_catalog_entries() {
    let mut registry = ModelRegistry::new();

    let error = registry
        .load_catalog_str(
            r#"
{
  "models": [
    {
      "provider": "",
      "model": "broken",
      "api": "OpenAiCompatible",
      "capabilities": {
        "streaming": true,
        "tools": false,
        "images": false,
        "reasoning": false,
        "embeddings": false
      }
    }
  ]
}
"#,
            "bad catalog",
        )
        .expect_err("empty provider should be rejected");

    assert!(error.to_string().contains("provider must not be empty"));
}

#[test]
fn model_registry_rejects_unknown_catalog_default() {
    let mut registry = ModelRegistry::new();

    let error = registry
        .load_catalog_str(
            r#"
{
  "models": [
    {
      "provider": "openrouter",
      "model": "known",
      "api": "OpenAiCompatible",
      "capabilities": {
        "streaming": true,
        "tools": false,
        "images": false,
        "reasoning": false,
        "embeddings": false
      }
    }
  ],
  "default": {
    "provider": "openrouter",
    "model": "missing"
  }
}
"#,
            "bad default catalog",
        )
        .expect_err("unknown default should be rejected");

    assert!(
        error
            .to_string()
            .contains("catalog default openrouter/missing")
    );
}

#[test]
fn model_registry_reports_missing_catalog_path() {
    let mut registry = ModelRegistry::new();
    let missing = std::env::temp_dir().join(format!(
        "neo-missing-model-catalog-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&missing);

    let error = registry
        .load_catalog_path(&missing)
        .expect_err("missing catalog path should be rejected");

    assert!(error.to_string().contains("failed to read model catalog"));
    assert!(error.to_string().contains(&missing.display().to_string()));
}
