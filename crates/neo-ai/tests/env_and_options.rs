use std::collections::BTreeMap;

use neo_ai::{
    ApiKind, ModelCapabilities, ModelSpec, ProviderId, ReasoningCapability, ReasoningEffort,
    ReasoningPolicy, ReasoningSelection, env_api_key_from, find_env_keys_from,
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
        serde_json::to_value(ReasoningEffort::minimal()).expect("serialize effort"),
        serde_json::json!("minimal")
    );
    assert_eq!(
        serde_json::from_value::<ReasoningEffort>(serde_json::json!("xhigh"))
            .expect("deserialize effort"),
        ReasoningEffort::xhigh()
    );
}

#[test]
fn reasoning_effort_preserves_custom_provider_value() {
    let effort: ReasoningEffort =
        serde_json::from_str(r#""UltraMax""#).expect("deserialize custom effort");

    assert_eq!(effort.as_str(), "UltraMax");
    assert_eq!(
        serde_json::to_string(&effort).expect("serialize custom effort"),
        r#""UltraMax""#
    );
}

#[test]
fn reasoning_effort_rejects_empty_values() {
    for value in [r#"""#, r#""   ""#] {
        assert!(serde_json::from_str::<ReasoningEffort>(value).is_err());
    }
}

#[test]
fn reasoning_effort_schema_requires_non_whitespace_content() {
    let schema = serde_json::to_value(schemars::schema_for!(ReasoningEffort))
        .expect("serialize reasoning effort schema");

    assert_eq!(schema["pattern"], r"\S");
}

#[test]
fn reasoning_effort_serializes_max_and_stable_names() {
    assert_eq!(
        serde_json::to_value(ReasoningEffort::max()).expect("serialize max"),
        serde_json::json!("max")
    );
    assert_eq!(
        serde_json::from_value::<ReasoningEffort>(serde_json::json!("max"))
            .expect("deserialize lowercase max"),
        ReasoningEffort::max()
    );
    assert_eq!(
        serde_json::from_value::<ReasoningEffort>(serde_json::json!("Max"))
            .expect("deserialize uppercase max"),
        ReasoningEffort::try_from("Max").expect("uppercase custom effort")
    );
}

#[test]
fn reasoning_selection_round_trips_structured_modes() {
    let effort = ReasoningSelection::Effort {
        effort: ReasoningEffort::high(),
    };
    let encoded = serde_json::to_value(&effort).expect("serialize effort selection");
    assert_eq!(
        encoded,
        serde_json::json!({ "mode": "effort", "effort": "high" })
    );
    assert_eq!(
        serde_json::from_value::<ReasoningSelection>(encoded).expect("deserialize effort"),
        effort
    );

    let budget = ReasoningSelection::BudgetTokens {
        budget_tokens: 8192,
    };
    let encoded = serde_json::to_value(&budget).expect("serialize budget selection");
    assert_eq!(
        encoded,
        serde_json::json!({ "mode": "budget_tokens", "budget_tokens": 8192 })
    );
    assert_eq!(
        serde_json::from_value::<ReasoningSelection>(encoded).expect("deserialize budget"),
        budget
    );

    assert_eq!(
        serde_json::to_value(ReasoningSelection::Off).expect("serialize off"),
        serde_json::json!({ "mode": "off" })
    );
}

#[test]
fn reasoning_capability_validates_supported_selection() {
    let capability = ReasoningCapability::Effort {
        values: vec![ReasoningEffort::low(), ReasoningEffort::high()],
        disable_supported: true,
    };
    assert!(capability.supports(&ReasoningSelection::Off));
    assert!(capability.supports(&ReasoningSelection::Effort {
        effort: ReasoningEffort::high(),
    }));
    assert!(!capability.supports(&ReasoningSelection::Effort {
        effort: ReasoningEffort::medium(),
    }));
    assert!(!capability.supports(&ReasoningSelection::BudgetTokens {
        budget_tokens: 1024,
    }));
}

#[test]
fn reasoning_capability_serializes_stable_shape() {
    let effort = ReasoningCapability::Effort {
        values: vec![ReasoningEffort::low(), ReasoningEffort::high()],
        disable_supported: true,
    };
    assert_eq!(
        serde_json::to_value(&effort).expect("serialize effort capability"),
        serde_json::json!({
            "type": "effort",
            "values": ["low", "high"],
            "disable_supported": true
        })
    );
    assert_eq!(
        serde_json::from_value::<ReasoningCapability>(serde_json::json!({
            "type": "effort",
            "values": ["low", "high"],
            "disable_supported": true
        }))
        .expect("deserialize effort capability"),
        effort
    );

    let budget = ReasoningCapability::BudgetTokens {
        min: Some(512),
        max: Some(24_576),
        disable_supported: false,
    };
    assert_eq!(
        serde_json::to_value(&budget).expect("serialize budget capability"),
        serde_json::json!({
            "type": "budget_tokens",
            "min": 512,
            "max": 24576,
            "disable_supported": false
        })
    );
    assert_eq!(
        serde_json::from_value::<ReasoningCapability>(serde_json::json!({
            "type": "budget_tokens",
            "min": 512,
            "max": 24576,
            "disable_supported": false
        }))
        .expect("deserialize budget capability"),
        budget
    );
}

#[test]
fn reasoning_budget_bounds_accept_only_range_values() {
    let capability = ReasoningCapability::BudgetTokens {
        min: Some(512),
        max: Some(24_576),
        disable_supported: true,
    };
    assert!(capability.supports(&ReasoningSelection::BudgetTokens { budget_tokens: 512 }));
    assert!(capability.supports(&ReasoningSelection::BudgetTokens {
        budget_tokens: 8192,
    }));
    assert!(!capability.supports(&ReasoningSelection::BudgetTokens { budget_tokens: 128 }));
    assert!(!capability.supports(&ReasoningSelection::BudgetTokens {
        budget_tokens: 32_000,
    }));
}

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
