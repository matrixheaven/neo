use neo_ai::{ReasoningCapability, ReasoningEffort, ReasoningSelection};

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
