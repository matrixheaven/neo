use std::collections::BTreeMap;

use neo_ai::{
    ApiKind, ChatMessage, ContentPart, ModelCapabilities, ModelSpec, ProviderId,
    ReasoningContinuation, ReasoningEffort, ReasoningPolicy, env_api_key_from, find_env_keys_from,
    sanitize_reasoning_continuation,
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
