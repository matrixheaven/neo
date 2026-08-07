use super::*;
use neo_ai::ReasoningEffort;

fn state() -> CustomEndpointWizardState {
    CustomEndpointWizardState::new(CustomEndpointWizardOptions {
        theme: TuiTheme::default(),
    })
}

fn visible(state: &CustomEndpointWizardState) -> String {
    state
        .render_lines(72)
        .into_iter()
        .map(|line| crate::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn provider_step_renders_empty_fields_as_muted_placeholders() {
    let state = state();

    let rendered = state.render_lines(72).join("\n");
    let visible = visible(&state);

    assert!(visible.contains("Name shown in Neo"), "{visible}");
    assert!(visible.contains("lowercase-id-used-in-config"), "{visible}");
    assert!(!visible.contains("Acme Gateway"), "{visible}");
    assert!(!visible.contains("\n  acme▏"), "{visible}");
    assert!(
        rendered.contains(&paint(
            "Name shown in Neo",
            Style::default().fg(TuiTheme::default().text_muted)
        )),
        "{rendered:?}"
    );
}

#[test]
fn endpoint_and_model_steps_render_empty_fields_as_muted_placeholders() {
    let mut state = state();

    state.step = WizardStep::EndpointAuth;
    let endpoint = visible(&state);
    assert!(endpoint.contains("Endpoint base URL"), "{endpoint}");
    assert!(endpoint.contains("ENV_VAR_WITH_API_KEY"), "{endpoint}");
    assert!(!endpoint.contains("gateway.example"), "{endpoint}");
    assert!(!endpoint.contains("ACME_API_KEY"), "{endpoint}");

    state.step = WizardStep::ModelIdentity;
    let rendered = state.render_lines(72).join("\n");
    let model = visible(&state);
    assert!(model.contains("Model id from provider"), "{model}");
    assert!(model.contains("provider-id/model-id"), "{model}");
    assert!(model.contains("Optional display name"), "{model}");
    assert!(!model.contains("qwen2.5"), "{model}");
    assert!(!model.contains("acme/"), "{model}");
    assert!(
        rendered.contains(&paint(
            "Model id from provider",
            Style::default().fg(TuiTheme::default().text_muted)
        )),
        "{rendered:?}"
    );
}

#[test]
fn model_identity_limits_are_selectable_and_editable() {
    let mut state = state();
    state.provider_id = "provider".to_owned();
    state.step = WizardStep::ModelIdentity;
    state.draft_model = CustomEndpointWizardState::empty_model(&state.provider_id, "model");
    state.selected = 2;

    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    assert_eq!(state.selected, 3);
    assert_eq!(
        state.handle_input(&InputEvent::Paste("131072".to_owned())),
        InputResult::Handled
    );
    assert_eq!(state.draft_model.max_context_tokens, Some(131_072));

    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    assert_eq!(state.selected, 4);
    assert_eq!(
        state.handle_input(&InputEvent::Paste("8192".to_owned())),
        InputResult::Handled
    );
    assert_eq!(state.draft_model.max_output_tokens, Some(8_192));

    let visible = visible(&state);
    assert!(visible.contains("▸ Output tokens"), "{visible}");
    assert!(visible.contains("Context tokens"), "{visible}");
}

#[test]
fn selected_editable_field_renders_brand_cursor_and_hierarchy() {
    let mut state = state();
    state.step = WizardStep::ModelIdentity;
    state.selected = 0;

    let rendered = state.render_lines(72).join("\n");
    let visible = visible(&state);

    assert!(visible.contains("▸ Model id"), "{visible}");
    assert!(visible.contains("▏"), "{visible}");
    assert!(
        rendered.contains(&paint(
            "▏",
            Style::default().fg(TuiTheme::default().brand).bold()
        )),
        "{rendered:?}"
    );
    assert!(
        rendered.contains(&paint(
            "Model",
            Style::default().fg(TuiTheme::default().text_primary).bold()
        )),
        "{rendered:?}"
    );
}

#[test]
fn failed_test_result_offers_save_anyway() {
    let mut state = state();
    state.apply_test_result(Err("401 Unauthorized".to_owned()));
    let visible = visible(&state);
    assert!(visible.contains("Request failed"), "{visible}");
    assert!(visible.contains("401 Unauthorized"), "{visible}");
    assert!(visible.contains("Save anyway"), "{visible}");
    assert!(visible.contains("↑/↓ select"), "{visible}");
}

#[test]
fn api_type_render_includes_all_current_protocols() {
    let mut state = state();
    state.step = WizardStep::ApiType;
    let visible = visible(&state);
    assert!(visible.contains("OpenAI-compatible"), "{visible}");
    assert!(visible.contains("type = \"openai\""), "{visible}");
    assert!(visible.contains("OpenAI Responses"), "{visible}");
    assert!(visible.contains("type = \"openai_response\""), "{visible}");
    assert!(visible.contains("Anthropic Messages"), "{visible}");
    assert!(visible.contains("type = \"anthropic\""), "{visible}");
    assert!(visible.contains("Google Generative AI"), "{visible}");
    assert!(visible.contains("type = \"google\""), "{visible}");
    assert!(visible.contains("↑/↓ select"), "{visible}");
}

#[test]
fn fetched_model_review_shows_blank_limits_as_review_points() {
    let mut state = state();
    state.provider_id = "acme".to_owned();
    state.step = WizardStep::ModelIdentity;
    state.draft_model = CustomEndpointModelDraft {
        source: CustomEndpointModelSource::Fetched {
            owned_by: Some("acme".to_owned()),
            created: Some(1_700_000_000),
        },
        model_id: "qwen2.5-coder-32b-instruct".to_owned(),
        alias: "acme/qwen2.5-coder-32b-instruct".to_owned(),
        display_name: None,
        max_context_tokens: None,
        max_output_tokens: None,
        streaming: true,
        tools: true,
        images: false,
        embeddings: false,
        reasoning: ReasoningCapability::None,
    };

    let visible = visible(&state);
    assert!(visible.contains("Source: /models"), "{visible}");
    assert!(visible.contains("owned_by = \"acme\""), "{visible}");
    assert!(visible.contains("Context tokens"), "{visible}");
    assert!(visible.contains("Unset context limit"), "{visible}");
    assert!(visible.contains("Output tokens"), "{visible}");
    assert!(visible.contains("Unset output limit"), "{visible}");
    assert!(visible.contains("↑/↓ select"), "{visible}");
}

#[test]
fn queued_fetched_model_for_review_preserves_source_metadata() {
    let mut state = state();
    state.provider_id = "acme".to_owned();
    state.apply_fetched_models(vec![CustomEndpointFetchedModel {
        id: "qwen2.5-coder-32b-instruct".to_owned(),
        owned_by: Some("model-lab".to_owned()),
        created: Some(1_700_000_000),
    }]);

    state.queue_selected_fetched_models_for_review();

    let visible = visible(&state);
    assert!(visible.contains("Source: /models"), "{visible}");
    assert!(visible.contains("owned_by = \"model-lab\""), "{visible}");
    assert!(visible.contains("created = 1700000000"), "{visible}");
    assert!(visible.contains("Context tokens"), "{visible}");
    assert!(visible.contains("Unset context limit"), "{visible}");
    assert!(visible.contains("Output tokens"), "{visible}");
    assert!(visible.contains("Unset output limit"), "{visible}");
}

#[test]
fn non_api_selectable_screens_render_marker_from_selected_index() {
    let mut state = state();
    state.step = WizardStep::AuthSource;
    state.selected = 1;

    let visible = visible(&state);

    assert!(visible.contains("  Environment variable"), "{visible}");
    assert!(visible.contains("▸ Paste secret"), "{visible}");
}

#[test]
fn renderable_steps_fit_narrow_widths() {
    let mut state = state();
    state.apply_fetched_models(vec![CustomEndpointFetchedModel {
        id: "qwen2.5-coder-32b-instruct".to_owned(),
        owned_by: Some("acme".to_owned()),
        created: Some(1_700_000_000),
    }]);

    for step in [
        WizardStep::Provider,
        WizardStep::ApiType,
        WizardStep::EndpointAuth,
        WizardStep::AuthSource,
        WizardStep::ModelSource,
        WizardStep::FetchSelect,
        WizardStep::ModelIdentity,
        WizardStep::ModelCapabilities,
        WizardStep::ReasoningType,
        WizardStep::ReasoningEffort,
        WizardStep::ReasoningBudget,
        WizardStep::ReasoningCombined,
        WizardStep::AddedModels,
        WizardStep::Review,
        WizardStep::TestResult,
        WizardStep::ValidationError,
        WizardStep::Saved,
    ] {
        state.step = step;
        for width in [1, 8, 24] {
            for line in state.render_lines(width) {
                let visible_width =
                    crate::primitive::visible_width(&crate::primitive::strip_ansi(&line));
                assert!(
                    visible_width <= width,
                    "{step:?} at width {width} rendered width {visible_width}: {line:?}"
                );
            }
        }
    }
}

#[test]
fn all_selectable_screens_render_select_hint() {
    let mut state = state();
    state.apply_fetched_models(vec![CustomEndpointFetchedModel {
        id: "qwen2.5-coder-32b-instruct".to_owned(),
        owned_by: Some("acme".to_owned()),
        created: Some(1_700_000_000),
    }]);

    for step in [
        WizardStep::Provider,
        WizardStep::ApiType,
        WizardStep::EndpointAuth,
        WizardStep::AuthSource,
        WizardStep::ModelSource,
        WizardStep::FetchSelect,
        WizardStep::ModelCapabilities,
        WizardStep::ReasoningType,
        WizardStep::ReasoningEffort,
        WizardStep::ReasoningBudget,
        WizardStep::ReasoningCombined,
        WizardStep::AddedModels,
        WizardStep::Review,
    ] {
        state.step = step;
        let visible = visible(&state);
        assert!(
            visible.contains("↑/↓ select"),
            "missing select hint in {step:?}:\n{visible}"
        );
    }
}

#[test]
fn provider_validation_rejects_uppercase_id() {
    let mut state = state();
    state.display_name = "Acme Gateway".to_owned();
    state.provider_id = "Acme".to_owned();

    assert!(state.provider_step_error().is_some());
    assert!(state.provider_step_error().unwrap().contains("lowercase"));
}

#[test]
fn cancel_from_api_type_returns_to_provider_without_cancel_action() {
    let mut state = state();
    state.step = WizardStep::ApiType;
    state.selected = 1;

    assert_eq!(
        state.handle_input(&InputEvent::Cancel),
        InputResult::Handled
    );

    assert_eq!(state.step, WizardStep::Provider);
    assert_eq!(state.selected, 2);
    assert!(state.take_action().is_none());
}

#[test]
fn provider_validation_error_clears_stale_fetch_action() {
    let mut state = state();
    state.action = Some(CustomEndpointWizardAction::FetchModels);
    state.display_name = "Acme Gateway".to_owned();
    state.provider_id = "Acme".to_owned();

    assert_eq!(state.submit_provider(), InputResult::Handled);

    assert_eq!(state.step, WizardStep::ValidationError);
    assert!(state.take_action().is_none());
}

#[test]
fn multiple_selected_fetched_models_are_reviewed_one_by_one() {
    let mut state = state();
    state.provider_id = "acme".to_owned();
    state.apply_fetched_models(vec![
        CustomEndpointFetchedModel {
            id: "qwen2".to_owned(),
            owned_by: Some("lab-a".to_owned()),
            created: Some(1),
        },
        CustomEndpointFetchedModel {
            id: "qwen3".to_owned(),
            owned_by: Some("lab-b".to_owned()),
            created: Some(2),
        },
    ]);

    assert!(state.queue_selected_fetched_models_for_review());
    assert_eq!(state.step, WizardStep::ModelIdentity);
    assert_eq!(state.draft_model.model_id, "qwen2");

    assert_eq!(state.submit_model_identity(), InputResult::Handled);
    assert_eq!(state.submit_model_capabilities(), InputResult::Handled);

    assert_eq!(state.models.len(), 1);
    assert_eq!(state.models[0].model_id, "qwen2");
    assert_eq!(state.step, WizardStep::ModelIdentity);
    assert_eq!(state.draft_model.model_id, "qwen3");
    assert_eq!(state.draft_model.alias, "acme/qwen3");
    assert_eq!(
        state.draft_model.source,
        CustomEndpointModelSource::Fetched {
            owned_by: Some("lab-b".to_owned()),
            created: Some(2),
        }
    );

    assert_eq!(state.submit_model_identity(), InputResult::Handled);
    assert_eq!(state.submit_model_capabilities(), InputResult::Handled);

    assert_eq!(state.step, WizardStep::AddedModels);
    assert_eq!(
        state
            .models
            .iter()
            .map(|model| model.model_id.as_str())
            .collect::<Vec<_>>(),
        vec!["qwen2", "qwen3"]
    );
}

#[test]
fn provider_text_input_edits_display_name_and_provider_id() {
    let mut state = state();

    assert_eq!(
        state.handle_input(&InputEvent::Paste("Acme Gateway".to_owned())),
        InputResult::Handled
    );
    assert_eq!(state.display_name, "Acme Gateway");

    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    state.handle_input(&InputEvent::Paste("Acm".to_owned()));
    state.handle_input(&InputEvent::Backspace);
    state.handle_input(&InputEvent::Insert('e'));

    assert_eq!(state.provider_id, "Ace");
}

#[test]
fn provider_tab_moves_to_next_field() {
    let mut state = state();

    assert_eq!(
        state.handle_input(&InputEvent::Insert('\t')),
        InputResult::Handled
    );

    assert_eq!(state.selected, 1);
    assert!(state.display_name.is_empty());
}

#[test]
fn model_identity_model_id_input_auto_updates_blank_alias() {
    let mut state = state();
    state.provider_id = "acme".to_owned();
    state.step = WizardStep::ModelIdentity;
    state.draft_model = CustomEndpointWizardState::empty_model(&state.provider_id, "");

    assert_eq!(
        state.handle_input(&InputEvent::Paste("qwen2".to_owned())),
        InputResult::Handled
    );

    assert_eq!(state.draft_model.model_id, "qwen2");
    assert_eq!(state.draft_model.alias, "acme/qwen2");
}

#[test]
fn empty_save_clears_stale_action_and_shows_validation_error() {
    let mut state = state();
    state.action = Some(CustomEndpointWizardAction::FetchModels);
    state.models.clear();
    state.step = WizardStep::Review;

    assert_eq!(state.submit_review(), InputResult::Handled);

    assert!(state.take_action().is_none());
    assert_eq!(state.step, WizardStep::ValidationError);
    assert_eq!(
        state.validation_error.as_deref(),
        Some("Add at least one model before saving.")
    );
}

#[test]
fn save_action_contains_reviewed_provider_and_model_draft() {
    let mut state = state();
    state.display_name = "Acme Gateway".to_owned();
    state.provider_id = "acme".to_owned();
    state.api_type = ApiType::OpenAi;
    state.base_url = "https://gateway.example.com/v1".to_owned();
    state.auth_source = CustomEndpointAuthDraft::EnvVar("ACME_API_KEY".to_owned());
    state.models.push(CustomEndpointModelDraft {
        source: CustomEndpointModelSource::Manual,
        model_id: "qwen2.5-coder-32b-instruct".to_owned(),
        alias: "acme/qwen2.5-coder-32b-instruct".to_owned(),
        display_name: Some("Qwen 2.5 Coder 32B".to_owned()),
        max_context_tokens: Some(131_072),
        max_output_tokens: Some(8_192),
        streaming: true,
        tools: true,
        images: false,
        embeddings: false,
        reasoning: ReasoningCapability::Effort {
            values: vec![
                ReasoningEffort::low(),
                ReasoningEffort::medium(),
                ReasoningEffort::high(),
            ],
            disable_supported: true,
        },
    });
    state.step = WizardStep::Review;

    state.submit_review();

    let Some(CustomEndpointWizardAction::Save(draft)) = state.take_action() else {
        panic!("expected save action");
    };
    assert_eq!(draft.provider_id, "acme");
    assert_eq!(draft.api_type, ApiType::OpenAi);
    assert_eq!(draft.models[0].alias, "acme/qwen2.5-coder-32b-instruct");
}

#[test]
fn effort_reasoning_page_edits_selected_values_before_saving_model() {
    let mut state = state();
    state.provider_id = "acme".to_owned();
    state.draft_model = CustomEndpointWizardState::empty_model(&state.provider_id, "reasoner");
    state.step = WizardStep::ReasoningType;
    state.selected = 2;

    assert_eq!(state.submit_reasoning_type(), InputResult::Handled);
    assert_eq!(state.step, WizardStep::ReasoningEffort);

    assert_eq!(
        state.handle_input(&InputEvent::Insert(' ')),
        InputResult::Handled
    );
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    assert_eq!(
        state.handle_input(&InputEvent::Insert(' ')),
        InputResult::Handled
    );
    assert_eq!(state.submit_step(), InputResult::Handled);

    assert_eq!(
        state.draft_model.reasoning,
        ReasoningCapability::Effort {
            values: vec![
                ReasoningEffort::minimal(),
                ReasoningEffort::medium(),
                ReasoningEffort::high(),
            ],
            disable_supported: true,
        }
    );
}

#[test]
fn budget_reasoning_page_edits_token_limits_before_saving_model() {
    let mut state = state();
    state.provider_id = "acme".to_owned();
    state.draft_model = CustomEndpointWizardState::empty_model(&state.provider_id, "reasoner");
    state.step = WizardStep::ReasoningType;
    state.selected = 3;

    assert_eq!(state.submit_reasoning_type(), InputResult::Handled);
    assert_eq!(state.step, WizardStep::ReasoningBudget);

    assert_eq!(
        state.handle_input(&InputEvent::Paste("1024".to_owned())),
        InputResult::Handled
    );
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    assert_eq!(
        state.handle_input(&InputEvent::Paste("8192".to_owned())),
        InputResult::Handled
    );
    assert_eq!(state.submit_step(), InputResult::Handled);

    assert_eq!(
        state.draft_model.reasoning,
        ReasoningCapability::BudgetTokens {
            min: Some(1024),
            max: Some(8192),
            disable_supported: true,
        }
    );
}

#[test]
fn combined_reasoning_page_reaches_effort_and_budget_detail_pages() {
    let mut state = state();
    state.provider_id = "acme".to_owned();
    state.draft_model = CustomEndpointWizardState::empty_model(&state.provider_id, "reasoner");
    state.step = WizardStep::ReasoningType;
    state.selected = 4;

    assert_eq!(state.submit_reasoning_type(), InputResult::Handled);
    assert_eq!(state.step, WizardStep::ReasoningCombined);

    state.selected = 1;
    assert_eq!(state.submit_step(), InputResult::Handled);
    assert_eq!(state.step, WizardStep::ReasoningEffort);
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    state.handle_input(&InputEvent::Insert(' '));
    assert_eq!(state.submit_step(), InputResult::Handled);
    assert_eq!(state.step, WizardStep::ReasoningCombined);

    state.selected = 2;
    assert_eq!(state.submit_step(), InputResult::Handled);
    assert_eq!(state.step, WizardStep::ReasoningBudget);
    assert_eq!(
        state.handle_input(&InputEvent::Paste("4096".to_owned())),
        InputResult::Handled
    );
    assert_eq!(state.submit_step(), InputResult::Handled);

    assert_eq!(
        state.draft_model.reasoning,
        ReasoningCapability::Combined {
            toggle: true,
            effort: vec![ReasoningEffort::low(), ReasoningEffort::medium()],
            budget: Some(neo_ai::ReasoningBudget {
                min: Some(4096),
                max: None,
            }),
            disable_supported: true,
        }
    );
}
