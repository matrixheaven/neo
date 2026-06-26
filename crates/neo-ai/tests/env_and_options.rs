use std::collections::BTreeMap;
use std::time::Duration;

use neo_ai::{
    ApiKind, CacheRetention, ChatMessage, ContentPart, ModelCapabilities, ModelSpec, ProviderId,
    ReasoningContinuation, ReasoningEffort, ReasoningPolicy, RequestMetadata, RequestOptions,
    env_api_key_from, find_env_keys_from, sanitize_reasoning_continuation,
};

#[test]
fn env_api_key_resolves_provider_specific_variables_in_priority_order() {
    let env = BTreeMap::from([
        ("ANTHROPIC_API_KEY".to_owned(), "anthropic-key".to_owned()),
        (
            "ANTHROPIC_OAUTH_TOKEN".to_owned(),
            "anthropic-oauth".to_owned(),
        ),
        ("OPENAI_API_KEY".to_owned(), "openai-key".to_owned()),
    ]);

    assert_eq!(
        env_api_key_from("openai", &env),
        Some("openai-key".to_owned())
    );
    assert_eq!(
        env_api_key_from("anthropic", &env),
        Some("anthropic-oauth".to_owned())
    );
    assert_eq!(
        find_env_keys_from("anthropic", &env),
        vec![
            "ANTHROPIC_OAUTH_TOKEN".to_owned(),
            "ANTHROPIC_API_KEY".to_owned()
        ]
    );
    assert_eq!(env_api_key_from("unknown", &env), None);
}

#[test]
fn request_options_have_typed_defaults_and_preserve_metadata() {
    let options = RequestOptions {
        temperature: Some(0.2),
        max_tokens: Some(4096),
        headers: BTreeMap::from([("x-neo-trace".to_owned(), "trace-1".to_owned())]),
        timeout: Some(Duration::from_secs(15)),
        reasoning_effort: Some(ReasoningEffort::High),
        replay_reasoning: false,
        retries: Some(2),
        cache: CacheRetention::Long,
        session_id: Some("session-123".to_owned()),
        metadata: RequestMetadata::from_pairs([("user_id", "u-1"), ("project_id", "p-1")]),
    };

    assert_eq!(options.temperature, Some(0.2));
    assert_eq!(options.max_tokens, Some(4096));
    assert_eq!(options.timeout, Some(Duration::from_secs(15)));
    assert_eq!(options.reasoning_effort, Some(ReasoningEffort::High));
    assert!(!options.replay_reasoning);
    assert_eq!(options.retries, Some(2));
    assert_eq!(options.cache, CacheRetention::Long);
    assert_eq!(options.session_id.as_deref(), Some("session-123"));
    assert_eq!(options.metadata.get("project_id"), Some("p-1"));
}

#[test]
fn reasoning_effort_serializes_as_stable_snake_case_values() {
    assert_eq!(
        serde_json::to_value(ReasoningEffort::Minimal).expect("serialize effort"),
        serde_json::json!("minimal")
    );
    assert_eq!(
        serde_json::from_value::<ReasoningEffort>(serde_json::json!("xhigh"))
            .expect("deserialize effort"),
        ReasoningEffort::XHigh
    );
}

#[test]
fn reasoning_policy_auto_is_deterministic_and_model_capability_aware() {
    let reasoning_model = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "gpt-reasoning".to_owned(),
        api: ApiKind::OpenAiResponses,
        capabilities: ModelCapabilities::reasoning_chat(),
    };
    let plain_model = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "gpt-plain".to_owned(),
        api: ApiKind::OpenAiResponses,
        capabilities: ModelCapabilities::tool_chat(),
    };

    assert_eq!(
        ReasoningPolicy::Auto.resolve_for_model(&reasoning_model),
        Some(ReasoningEffort::Medium)
    );
    assert_eq!(ReasoningPolicy::Auto.resolve_for_model(&plain_model), None);
    assert_eq!(
        ReasoningPolicy::Off.resolve_for_model(&reasoning_model),
        None
    );
    assert_eq!(
        ReasoningPolicy::XHigh.resolve_for_model(&reasoning_model),
        Some(ReasoningEffort::XHigh)
    );
    assert_eq!(
        serde_json::from_value::<ReasoningPolicy>(serde_json::json!("auto"))
            .expect("deserialize auto reasoning policy"),
        ReasoningPolicy::Auto
    );
}

#[test]
fn reasoning_continuation_strips_opaque_thinking_across_provider_or_api_boundaries() {
    let origin = ReasoningContinuation {
        provider: ProviderId("openai".to_owned()),
        api: ApiKind::OpenAiResponses,
    };
    let same_target = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "gpt-reasoning".to_owned(),
        api: ApiKind::OpenAiResponses,
        capabilities: ModelCapabilities::reasoning_chat(),
    };
    let cross_provider_target = ModelSpec {
        provider: ProviderId("anthropic".to_owned()),
        model: "claude-reasoning".to_owned(),
        api: ApiKind::AnthropicMessages,
        capabilities: ModelCapabilities::reasoning_chat(),
    };
    let messages = vec![ChatMessage::Assistant {
        content: vec![
            ContentPart::Thinking {
                text: "portable summary".to_owned(),
                signature: None,
                redacted: false,
            },
            ContentPart::Thinking {
                text: "signed opaque".to_owned(),
                signature: Some("sig-openai".to_owned()),
                redacted: false,
            },
            ContentPart::Thinking {
                text: "redacted opaque".to_owned(),
                signature: None,
                redacted: true,
            },
            ContentPart::Text {
                text: "answer".to_owned(),
            },
        ],
        tool_calls: Vec::new(),
    }];

    assert_eq!(
        sanitize_reasoning_continuation(messages.clone(), Some(&origin), &same_target),
        messages
    );

    let sanitized =
        sanitize_reasoning_continuation(messages, Some(&origin), &cross_provider_target);
    assert_eq!(
        sanitized,
        vec![ChatMessage::Assistant {
            content: vec![
                ContentPart::Thinking {
                    text: "portable summary".to_owned(),
                    signature: None,
                    redacted: false,
                },
                ContentPart::Text {
                    text: "answer".to_owned(),
                },
            ],
            tool_calls: Vec::new(),
        }]
    );
}
