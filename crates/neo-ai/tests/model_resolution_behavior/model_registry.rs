use neo_ai::{ApiKind, ModelCapabilities, ModelRegistry, ModelSpec, ProviderId};

fn model(provider: &str, name: &str, capabilities: ModelCapabilities) -> ModelSpec {
    ModelSpec {
        provider: ProviderId(provider.to_owned()),
        model: name.to_owned(),
        api: ApiKind::OpenAi,
        capabilities,
    }
}

#[test]
fn model_capabilities_shapes_cover_default_and_helpers() {
    // (case, capabilities, streaming, tools, images, embeddings, supports_reasoning, max_context_tokens)
    let cases = [
        (
            "default is text chat",
            ModelCapabilities::default(),
            true,
            false,
            false,
            false,
            false,
            None,
        ),
        (
            "tool chat with context window",
            ModelCapabilities::tool_chat().with_max_context_tokens(128_000),
            true,
            true,
            false,
            false,
            false,
            Some(128_000),
        ),
        (
            "embedding helper",
            ModelCapabilities::embedding(),
            false,
            false,
            false,
            true,
            false,
            None,
        ),
    ];
    for (name, caps, streaming, tools, images, embeddings, supports_reasoning, max_context) in cases
    {
        assert_eq!(caps.streaming, streaming, "case {name}: streaming");
        assert_eq!(caps.tools, tools, "case {name}: tools");
        assert_eq!(caps.images, images, "case {name}: images");
        assert_eq!(caps.embeddings, embeddings, "case {name}: embeddings");
        assert_eq!(
            caps.supports_reasoning(),
            supports_reasoning,
            "case {name}: supports_reasoning"
        );
        assert_eq!(
            caps.max_context_tokens, max_context,
            "case {name}: max_context_tokens"
        );
    }
    assert_eq!(
        ModelCapabilities::chat(),
        ModelCapabilities::default(),
        "chat helper equals default"
    );
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
        Some(&ApiKind::OpenAiResponse)
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
