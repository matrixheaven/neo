use super::*;
use neo_ai::{ReasoningCapability, ReasoningEffort, ReasoningSelection};

fn theme() -> TuiTheme {
    TuiTheme::default()
}

fn models() -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            alias: "openai/gpt-4o".into(),
            provider_id: "openai".into(),
            display_name: "GPT-4o".into(),
            model_id: "gpt-4o".into(),
            capabilities: vec!["thinking".into()],
            reasoning: ReasoningCapability::Toggle {
                disable_supported: true,
            },
            max_context_tokens: Some(128_000),
        },
        ModelEntry {
            alias: "anthropic/claude-sonnet".into(),
            provider_id: "anthropic".into(),
            display_name: "Claude Sonnet".into(),
            model_id: "claude-sonnet".into(),
            capabilities: vec!["always_thinking".into()],
            reasoning: ReasoningCapability::Toggle {
                disable_supported: false,
            },
            max_context_tokens: Some(200_000),
        },
        ModelEntry {
            alias: "google/gemini-flash".into(),
            provider_id: "google".into(),
            display_name: "Gemini Flash".into(),
            model_id: "gemini-flash".into(),
            capabilities: vec![],
            reasoning: ReasoningCapability::None,
            max_context_tokens: Some(1_000_000),
        },
    ]
}

fn reasoning_models() -> Vec<ModelEntry> {
    vec![
        ModelEntry {
            alias: "openai/gpt-reasoner".into(),
            provider_id: "openai".into(),
            display_name: "GPT Reasoner".into(),
            model_id: "gpt-reasoner".into(),
            capabilities: vec!["reasoning".into()],
            reasoning: ReasoningCapability::Effort {
                values: vec![
                    ReasoningEffort::low(),
                    ReasoningEffort::medium(),
                    ReasoningEffort::high(),
                    ReasoningEffort::xhigh(),
                ],
                disable_supported: true,
            },
            max_context_tokens: Some(128_000),
        },
        ModelEntry {
            alias: "google/gemini-budget".into(),
            provider_id: "google".into(),
            display_name: "Gemini Budget".into(),
            model_id: "gemini-budget".into(),
            capabilities: vec!["reasoning".into()],
            reasoning: ReasoningCapability::BudgetTokens {
                min: Some(0),
                max: Some(24_576),
                disable_supported: true,
            },
            max_context_tokens: Some(1_000_000),
        },
        ModelEntry {
            alias: "qwen/qwen-toggle".into(),
            provider_id: "qwen".into(),
            display_name: "Qwen Toggle".into(),
            model_id: "qwen-toggle".into(),
            capabilities: vec!["reasoning".into()],
            reasoning: ReasoningCapability::Toggle {
                disable_supported: true,
            },
            max_context_tokens: Some(128_000),
        },
        ModelEntry {
            alias: "openai/gpt-plain".into(),
            provider_id: "openai".into(),
            display_name: "GPT Plain".into(),
            model_id: "gpt-plain".into(),
            capabilities: vec![],
            reasoning: ReasoningCapability::None,
            max_context_tokens: Some(128_000),
        },
    ]
}

#[test]
fn effort_reasoning_renders_supported_values_and_returns_selection() {
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: reasoning_models(),
        current_alias: "openai/gpt-reasoner".into(),
        selected_alias: None,
        current_reasoning: ReasoningSelection::Effort {
            effort: ReasoningEffort::medium(),
        },
        theme: theme(),
    });

    let combined = state.render_lines(80).join("\n");
    assert!(combined.contains("Reasoning:"));
    assert!(combined.contains("off"));
    assert!(combined.contains("low"));
    assert!(combined.contains("[medium]"));
    assert!(combined.contains("xhigh"));
    assert!(!combined.contains("minimal"));
    assert!(!combined.contains("max"));

    state.handle_input(&InputEvent::MoveRight);
    state.handle_input(&InputEvent::Submit);

    assert_eq!(
        state.take_result(),
        Some(ModelSelectorResult::Selected(ModelSelection {
            alias: "openai/gpt-reasoner".to_owned(),
            thinking: true,
            reasoning: ReasoningSelection::Effort {
                effort: ReasoningEffort::high(),
            },
        }))
    );
}

#[test]
fn model_declared_custom_effort_renders_and_selects_exact_value() {
    let custom = ReasoningEffort::try_from("UltraMax").expect("custom effort");
    let mut model = reasoning_models().remove(0);
    model.reasoning = ReasoningCapability::Effort {
        values: vec![custom.clone()],
        disable_supported: true,
    };
    let alias = model.alias.clone();
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: vec![model],
        current_alias: alias.clone(),
        selected_alias: None,
        current_reasoning: ReasoningSelection::Off,
        theme: theme(),
    });

    assert!(state.render_lines(80).join("\n").contains("UltraMax"));
    state.handle_input(&InputEvent::MoveRight);
    state.handle_input(&InputEvent::Submit);

    assert_eq!(
        state.take_result(),
        Some(ModelSelectorResult::Selected(ModelSelection {
            alias,
            thinking: true,
            reasoning: ReasoningSelection::Effort { effort: custom },
        }))
    );
}

#[test]
fn custom_effort_control_characters_are_escaped_only_for_display() {
    let custom = ReasoningEffort::try_from("Ultra\n\u{1b}Max").expect("custom effort");
    let rendered = render_effort_segments(
        std::slice::from_ref(&custom),
        false,
        &ReasoningSelection::Effort {
            effort: custom.clone(),
        },
    );

    assert!(!rendered.contains('\n'));
    assert!(!rendered.contains('\u{1b}'));
    assert!(rendered.contains(r"Ultra\n\u{1b}Max"));
    assert_eq!(custom.as_str(), "Ultra\n\u{1b}Max");
}

#[test]
fn budget_reasoning_supports_presets_custom_value_and_invalid_state() {
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: reasoning_models(),
        current_alias: "google/gemini-budget".into(),
        selected_alias: Some("google/gemini-budget".into()),
        current_reasoning: ReasoningSelection::BudgetTokens {
            budget_tokens: 8192,
        },
        theme: theme(),
    });

    let combined = state.render_lines(80).join("\n");
    assert!(combined.contains("Reasoning budget:"));
    assert!(combined.contains("off"));
    assert!(combined.contains("1k"));
    assert!(combined.contains("[8k]"));
    assert!(combined.contains("24k"));
    assert!(combined.contains("Range: 0..24576 tokens"));

    state.handle_input(&InputEvent::Insert('e'));
    for ch in "40000".chars() {
        state.handle_input(&InputEvent::Insert(ch));
    }

    let invalid = state.render_lines(80).join("\n");
    assert!(invalid.contains("Custom: 40000"));
    assert!(invalid.contains("budget must be between 0 and 24576 tokens"));
    assert_eq!(
        state.handle_input(&InputEvent::Submit),
        InputResult::Handled
    );
    assert!(state.result().is_none());

    for _ in 0.."40000".len() {
        state.handle_input(&InputEvent::Backspace);
    }
    for ch in "12000".chars() {
        state.handle_input(&InputEvent::Insert(ch));
    }
    state.handle_input(&InputEvent::Submit);

    assert_eq!(
        state.take_result(),
        Some(ModelSelectorResult::Selected(ModelSelection {
            alias: "google/gemini-budget".to_owned(),
            thinking: true,
            reasoning: ReasoningSelection::BudgetTokens {
                budget_tokens: 12_000,
            },
        }))
    );
}

#[test]
fn filtered_selection_preserves_selected_model_reasoning_draft() {
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: reasoning_models(),
        current_alias: "google/gemini-budget".into(),
        selected_alias: Some("google/gemini-budget".into()),
        current_reasoning: ReasoningSelection::BudgetTokens {
            budget_tokens: 8192,
        },
        theme: theme(),
    });

    state.handle_input(&InputEvent::Paste("Gemini".to_owned()));
    state.handle_input(&InputEvent::Submit);

    assert_eq!(
        state.take_result(),
        Some(ModelSelectorResult::Selected(ModelSelection {
            alias: "google/gemini-budget".to_owned(),
            thinking: true,
            reasoning: ReasoningSelection::BudgetTokens {
                budget_tokens: 8192,
            },
        }))
    );
}

#[test]
fn esc_exits_budget_edit_before_clearing_query_or_cancelling() {
    let budget_model = reasoning_models()
        .into_iter()
        .find(|model| model.alias == "google/gemini-budget")
        .expect("budget model");
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: vec![budget_model],
        current_alias: "google/gemini-budget".into(),
        selected_alias: Some("google/gemini-budget".into()),
        current_reasoning: ReasoningSelection::BudgetTokens {
            budget_tokens: 8192,
        },
        theme: theme(),
    });

    state.handle_input(&InputEvent::Paste("Gemini".to_owned()));
    state.handle_input(&InputEvent::Insert('e'));
    state.handle_input(&InputEvent::Insert('4'));
    assert!(
        state
            .selected_draft()
            .is_some_and(|draft| draft.editing_budget)
    );

    assert_eq!(
        state.handle_input(&InputEvent::Cancel),
        InputResult::Handled
    );
    assert_eq!(state.list.query(), "Gemini");
    assert!(
        state
            .selected_draft()
            .is_some_and(|draft| !draft.editing_budget)
    );
    assert!(state.result().is_none());

    assert_eq!(
        state.handle_input(&InputEvent::Cancel),
        InputResult::Handled
    );
    assert!(state.list.query().is_empty());
    assert!(state.result().is_none());

    assert_eq!(
        state.handle_input(&InputEvent::Cancel),
        InputResult::Cancelled
    );
    assert!(matches!(
        state.take_result(),
        Some(ModelSelectorResult::Cancelled)
    ));
}

#[test]
fn toggle_and_no_reasoning_states_render_distinct_controls() {
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: reasoning_models(),
        current_alias: "qwen/qwen-toggle".into(),
        selected_alias: Some("qwen/qwen-toggle".into()),
        current_reasoning: ReasoningSelection::On,
        theme: theme(),
    });

    let combined = state.render_lines(80).join("\n");
    assert!(combined.contains("Reasoning:"));
    assert!(combined.contains("[on]"));
    assert!(combined.contains("off"));

    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    let unavailable = state.render_lines(80).join("\n");
    assert!(unavailable.contains("Reasoning: unavailable for this model"));

    state.handle_input(&InputEvent::Submit);
    assert_eq!(
        state.take_result(),
        Some(ModelSelectorResult::Selected(ModelSelection {
            alias: "openai/gpt-plain".to_owned(),
            thinking: false,
            reasoning: ReasoningSelection::Off,
        }))
    );
}

#[test]
fn renders_title_and_rows() {
    let state = ModelSelectorState::new(ModelSelectorOptions {
        models: models(),
        current_alias: "openai/gpt-4o".into(),
        selected_alias: None,
        current_reasoning: ReasoningSelection::Off,
        theme: theme(),
    });
    let lines = state.render_lines(60);
    let combined: String = lines.join("\n");
    assert!(combined.contains("Models"));
    assert!(combined.contains("GPT-4o"));
    assert!(combined.contains("Claude Sonnet"));
}

#[test]
fn current_marker_shown() {
    let state = ModelSelectorState::new(ModelSelectorOptions {
        models: models(),
        current_alias: "openai/gpt-4o".into(),
        selected_alias: None,
        current_reasoning: ReasoningSelection::Off,
        theme: theme(),
    });
    let lines = state.render_lines(60);
    let combined: String = lines.join("\n");
    assert!(combined.contains("← current"));
}

#[test]
fn fuzzy_filter_reduces_items() {
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: models(),
        current_alias: "openai/gpt-4o".into(),
        selected_alias: None,
        current_reasoning: ReasoningSelection::Off,
        theme: theme(),
    });
    state.handle_input(&InputEvent::Insert('c'));
    state.handle_input(&InputEvent::Insert('l'));
    // Should match "Claude Sonnet"
    assert_eq!(state.list.total_filtered(), 1);
}

#[test]
fn reasoning_control_respects_capabilities() {
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: models(),
        current_alias: "openai/gpt-4o".into(),
        selected_alias: None,
        current_reasoning: ReasoningSelection::Off,
        theme: theme(),
    });

    // First model (gpt-4o) supports thinking → toggles on
    let entry = state.selected_entry().cloned().unwrap();
    assert_eq!(entry.alias, "openai/gpt-4o");
    assert_eq!(state.effective_reasoning(&entry), ReasoningSelection::Off);
    state.handle_input(&InputEvent::MoveRight); // toggle
    let entry2 = state.selected_entry().cloned().unwrap();
    assert_eq!(state.effective_reasoning(&entry2), ReasoningSelection::On);

    // Second model (claude) always_thinking → stays on regardless
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    let entry3 = state.selected_entry().cloned().unwrap();
    assert_eq!(entry3.alias, "anthropic/claude-sonnet");
    assert_eq!(state.effective_reasoning(&entry3), ReasoningSelection::On);

    // Third model (gemini) no thinking → stays off
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    let entry4 = state.selected_entry().cloned().unwrap();
    assert_eq!(entry4.alias, "google/gemini-flash");
    assert_eq!(state.effective_reasoning(&entry4), ReasoningSelection::Off);
}

#[test]
fn enter_returns_selected() {
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: models(),
        current_alias: "openai/gpt-4o".into(),
        selected_alias: None,
        current_reasoning: ReasoningSelection::Off,
        theme: theme(),
    });
    state.handle_input(&InputEvent::Submit);
    let result = state.take_result().unwrap();
    match result {
        ModelSelectorResult::Selected(sel) => {
            assert_eq!(sel.alias, "openai/gpt-4o");
            assert!(!sel.thinking);
            assert_eq!(sel.reasoning, ReasoningSelection::Off);
        }
        ModelSelectorResult::Cancelled => panic!("expected Selected"),
    }
}

#[test]
fn esc_clears_query_then_cancels() {
    let mut state = ModelSelectorState::new(ModelSelectorOptions {
        models: models(),
        current_alias: "openai/gpt-4o".into(),
        selected_alias: None,
        current_reasoning: ReasoningSelection::Off,
        theme: theme(),
    });
    state.handle_input(&InputEvent::Insert('a'));
    assert!(!state.list.query().is_empty());

    state.handle_input(&InputEvent::Cancel);
    assert!(state.list.query().is_empty());
    assert!(state.result.is_none()); // first Esc just cleared

    state.handle_input(&InputEvent::Cancel);
    assert!(matches!(
        state.take_result(),
        Some(ModelSelectorResult::Cancelled)
    ));
}
