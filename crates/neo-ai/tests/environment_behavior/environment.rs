use neo_ai::{
    ApiKind, ModelCapabilities, ModelSpec, ProviderId, ReasoningCapability, ReasoningEffort,
    ReasoningPolicy, ReasoningSelection,
};

#[test]
fn reasoning_policy_auto_respects_model_capability() {
    let effort_model = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "gpt-effort".to_owned(),
        api: ApiKind::OpenAiResponse,
        capabilities: ModelCapabilities {
            reasoning: ReasoningCapability::Effort {
                values: vec![ReasoningEffort::low(), ReasoningEffort::medium()],
                disable_supported: true,
            },
            ..ModelCapabilities::tool_chat()
        },
    };
    let toggle_model = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "gpt-toggle".to_owned(),
        api: ApiKind::OpenAiResponse,
        capabilities: ModelCapabilities::reasoning_chat(),
    };
    let budget_model = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "gpt-budget".to_owned(),
        api: ApiKind::OpenAiResponse,
        capabilities: ModelCapabilities {
            reasoning: ReasoningCapability::BudgetTokens {
                min: Some(512),
                max: Some(24_576),
                disable_supported: true,
            },
            ..ModelCapabilities::tool_chat()
        },
    };
    let empty_effort_model = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "gpt-empty-effort".to_owned(),
        api: ApiKind::OpenAiResponse,
        capabilities: ModelCapabilities {
            reasoning: ReasoningCapability::Effort {
                values: Vec::new(),
                disable_supported: true,
            },
            ..ModelCapabilities::tool_chat()
        },
    };
    let plain_model = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "gpt-plain".to_owned(),
        api: ApiKind::OpenAiResponse,
        capabilities: ModelCapabilities::tool_chat(),
    };

    assert_eq!(
        ReasoningPolicy::Auto.resolve_for_model(&effort_model),
        ReasoningSelection::Effort {
            effort: ReasoningEffort::medium()
        }
    );
    assert_eq!(
        ReasoningPolicy::Auto.resolve_for_model(&toggle_model),
        ReasoningSelection::On
    );
    assert_eq!(
        ReasoningPolicy::Auto.resolve_for_model(&budget_model),
        ReasoningSelection::BudgetTokens { budget_tokens: 512 }
    );
    assert_eq!(
        ReasoningPolicy::Auto.resolve_for_model(&empty_effort_model),
        ReasoningSelection::Off
    );
    assert_eq!(
        ReasoningPolicy::Auto.resolve_for_model(&plain_model),
        ReasoningSelection::Off
    );
    assert_eq!(
        ReasoningPolicy::Off.resolve_for_model(&toggle_model),
        ReasoningSelection::Off
    );
    assert_eq!(
        ReasoningPolicy::XHigh.resolve_for_model(&effort_model),
        ReasoningSelection::Effort {
            effort: ReasoningEffort::xhigh()
        }
    );
    assert_eq!(
        ReasoningPolicy::Max.resolve_for_model(&effort_model),
        ReasoningSelection::Effort {
            effort: ReasoningEffort::max()
        }
    );
    assert_eq!(
        serde_json::from_value::<ReasoningPolicy>(serde_json::json!("auto"))
            .expect("deserialize auto reasoning policy"),
        ReasoningPolicy::Auto
    );
}
