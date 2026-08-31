//! Reasoning intensity resolution fixed points: canonical ladder mapping,
//! auxiliary policy selection, and config string round-trips.

use crate::{
    AuxReasoning, EffortResolution, ReasoningCapability, ReasoningEffort, ReasoningSelection,
    automatic_reasoning_selection, aux_reasoning_selection, resolve_requested_effort,
};

fn efforts(values: &[&str]) -> Vec<ReasoningEffort> {
    values
        .iter()
        .map(|value| ReasoningEffort::try_from((*value).to_owned()).expect("valid effort"))
        .collect()
}

fn effort_capability(values: &[&str], disable_supported: bool) -> ReasoningCapability {
    ReasoningCapability::Effort {
        values: efforts(values),
        disable_supported,
    }
}

fn nearest(value: &str) -> EffortResolution {
    EffortResolution::Nearest(ReasoningEffort::try_from(value.to_owned()).expect("valid effort"))
}

#[test]
fn resolve_requested_effort_prefers_exact_declared_value() {
    let values = efforts(&["5k", "10k", "20k"]);
    assert_eq!(
        resolve_requested_effort(&values, "10k"),
        EffortResolution::Exact(efforts(&["10k"]).remove(0))
    );
    assert_eq!(
        resolve_requested_effort(&values, "5K"),
        EffortResolution::Exact(efforts(&["5k"]).remove(0)),
        "case-insensitive declared-value match is still exact"
    );
}

#[test]
fn canonical_names_map_by_ladder_position_onto_numeric_values() {
    let values = efforts(&["5k", "10k", "20k"]);
    assert_eq!(resolve_requested_effort(&values, "low"), nearest("5k"));
    assert_eq!(resolve_requested_effort(&values, "medium"), nearest("10k"));
    assert_eq!(resolve_requested_effort(&values, "high"), nearest("10k"));
    assert_eq!(resolve_requested_effort(&values, "max"), nearest("20k"));
}

#[test]
fn canonical_names_map_by_ladder_position_onto_named_values() {
    let values = efforts(&["normal", "high", "ultra"]);
    assert_eq!(
        resolve_requested_effort(&values, "minimal"),
        nearest("normal")
    );
    assert_eq!(resolve_requested_effort(&values, "low"), nearest("normal"));
    assert_eq!(resolve_requested_effort(&values, "medium"), nearest("high"));
    assert_eq!(resolve_requested_effort(&values, "max"), nearest("ultra"));
}

#[test]
fn full_ladder_maps_identity_and_single_value_collapses() {
    let full = efforts(&["minimal", "low", "medium", "high", "xhigh", "max"]);
    for name in ["minimal", "low", "medium", "high", "xhigh", "max"] {
        assert_eq!(
            resolve_requested_effort(&full, name),
            EffortResolution::Exact(efforts(&[name]).remove(0)),
            "a complete ladder matches each canonical name exactly"
        );
    }
    let single = efforts(&["turbo"]);
    assert_eq!(resolve_requested_effort(&single, "max"), nearest("turbo"));
}

#[test]
fn unknown_names_and_empty_inputs_do_not_resolve() {
    let values = efforts(&["5k", "10k", "20k"]);
    assert_eq!(
        resolve_requested_effort(&values, "turbo"),
        EffortResolution::Unresolved
    );
    assert_eq!(
        resolve_requested_effort(&values, "   "),
        EffortResolution::Unresolved
    );
    assert_eq!(
        resolve_requested_effort(&[], "low"),
        EffortResolution::Unresolved
    );
}

#[test]
fn auxiliary_auto_prefers_disable_then_cheapest_effort() {
    let disableable = effort_capability(&["low", "high"], true);
    assert_eq!(
        aux_reasoning_selection(&disableable, &AuxReasoning::Auto),
        crate::AuxReasoningPlan {
            selection: ReasoningSelection::Off,
            disable_reasoning: true,
            coerced: false,
        }
    );
    let always_thinking = effort_capability(&["low", "high", "max"], false);
    assert_eq!(
        aux_reasoning_selection(&always_thinking, &AuxReasoning::Auto),
        crate::AuxReasoningPlan {
            selection: ReasoningSelection::Effort {
                effort: efforts(&["low"]).remove(0)
            },
            disable_reasoning: false,
            coerced: false,
        },
        "always-thinking models get the cheapest declared effort"
    );
}

#[test]
fn auxiliary_auto_falls_through_toggle_budget_and_none() {
    let toggle = ReasoningCapability::Toggle {
        disable_supported: false,
    };
    assert_eq!(
        aux_reasoning_selection(&toggle, &AuxReasoning::Auto).selection,
        ReasoningSelection::On
    );
    let budget = ReasoningCapability::BudgetTokens {
        min: Some(1024),
        max: Some(24_576),
        disable_supported: false,
    };
    assert_eq!(
        aux_reasoning_selection(&budget, &AuxReasoning::Auto).selection,
        ReasoningSelection::BudgetTokens {
            budget_tokens: 1024
        }
    );
    let none = ReasoningCapability::None;
    let plan = aux_reasoning_selection(&none, &AuxReasoning::Auto);
    assert!(
        plan.disable_reasoning,
        "metadata-less models keep the disable request"
    );
}

#[test]
fn auxiliary_off_always_requests_disable() {
    let always_thinking = effort_capability(&["low", "high", "max"], false);
    let plan = aux_reasoning_selection(&always_thinking, &AuxReasoning::Off);
    assert_eq!(plan.selection, ReasoningSelection::Off);
    assert!(plan.disable_reasoning);
}

#[test]
fn auxiliary_pinned_effort_resolves_exact_nearest_and_fallback() {
    let values = ["5k", "10k", "20k"];
    let capability = effort_capability(&values, false);
    let exact = aux_reasoning_selection(&capability, &AuxReasoning::Effort("20k".to_owned()));
    assert_eq!(
        exact.selection,
        ReasoningSelection::Effort {
            effort: efforts(&["20k"]).remove(0)
        }
    );
    assert!(!exact.coerced);

    let nearest = aux_reasoning_selection(&capability, &AuxReasoning::Effort("high".to_owned()));
    assert_eq!(
        nearest.selection,
        ReasoningSelection::Effort {
            effort: efforts(&["10k"]).remove(0)
        }
    );
    assert!(
        nearest.coerced,
        "canonical-name mapping is visible to the caller"
    );

    let fallback = aux_reasoning_selection(&capability, &AuxReasoning::Effort("turbo".to_owned()));
    assert_eq!(
        fallback.selection,
        ReasoningSelection::Effort {
            effort: efforts(&["5k"]).remove(0)
        },
        "unresolvable names fall back to the auto plan"
    );
    assert!(fallback.coerced);
}

#[test]
fn default_reasoning_selection_for_main_chat_capability() {
    let with_medium = effort_capability(&["low", "medium", "high"], true);
    assert_eq!(
        automatic_reasoning_selection(&with_medium),
        ReasoningSelection::Effort {
            effort: ReasoningEffort::medium()
        }
    );
    let without_medium = effort_capability(&["5k", "10k", "20k"], false);
    assert_eq!(
        automatic_reasoning_selection(&without_medium),
        ReasoningSelection::Effort {
            effort: efforts(&["5k"]).remove(0)
        }
    );
    let toggle = ReasoningCapability::Toggle {
        disable_supported: false,
    };
    assert_eq!(
        automatic_reasoning_selection(&toggle),
        ReasoningSelection::On
    );
    assert_eq!(
        automatic_reasoning_selection(&ReasoningCapability::None),
        ReasoningSelection::Off
    );
}

#[test]
fn aux_reasoning_config_strings_round_trip() {
    assert_eq!(
        serde_json::from_value::<AuxReasoning>(serde_json::json!("auto")).expect("auto"),
        AuxReasoning::Auto
    );
    assert_eq!(
        serde_json::from_value::<AuxReasoning>(serde_json::json!("off")).expect("off"),
        AuxReasoning::Off
    );
    assert_eq!(
        serde_json::from_value::<AuxReasoning>(serde_json::json!(" 5k ")).expect("pinned"),
        AuxReasoning::Effort("5k".to_owned()),
        "pinned values keep their trimmed spelling"
    );
    assert!(serde_json::from_value::<AuxReasoning>(serde_json::json!("")).is_err());
    assert!(serde_json::from_value::<AuxReasoning>(serde_json::json!("   ")).is_err());
    assert_eq!(
        serde_json::to_value(&AuxReasoning::Effort("5k".to_owned())).expect("serialize"),
        serde_json::json!("5k")
    );
    assert_eq!(
        serde_json::to_value(&AuxReasoning::Auto).expect("serialize"),
        serde_json::json!("auto")
    );
}
