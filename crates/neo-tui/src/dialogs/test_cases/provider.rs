use super::*;

fn source(label: &str, ids: &[&str]) -> ProviderSource {
    ProviderSource {
        provider_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        label: label.to_owned(),
        kind: ProviderSourceKind::BuiltIn,
    }
}

fn theme() -> TuiTheme {
    TuiTheme::default()
}

fn manager(sources: Vec<ProviderSource>, active: Option<&str>) -> ProviderManagerState {
    ProviderManagerState::new(&ProviderManagerOptions {
        sources,
        active_provider_id: active.map(std::borrow::ToOwned::to_owned),
        theme: theme(),
    })
}

fn visible_lines(state: &ProviderManagerState, width: usize) -> Vec<String> {
    state
        .render_lines(width)
        .iter()
        .map(|line| crate::primitive::strip_ansi(line))
        .collect()
}

#[test]
fn render_shows_title_source_rows_and_add_row() {
    let state = manager(
        vec![
            source("OpenAI", &["openai"]),
            source("Anthropic", &["anthropic"]),
        ],
        Some("openai"),
    );
    let visible = visible_lines(&state, 60);
    let joined = visible.join("\n");

    assert!(joined.contains("Providers"), "title missing: {joined}");
    assert!(joined.contains("OpenAI"), "OpenAI row missing: {joined}");
    assert!(
        joined.contains("Anthropic"),
        "Anthropic row missing: {joined}"
    );
    assert!(
        joined.contains("[ Add New Platform ]"),
        "add row missing: {joined}"
    );
    assert!(
        joined.contains("← current"),
        "current marker missing: {joined}"
    );
}

#[test]
fn render_shows_hint() {
    let state = manager(vec![source("OpenAI", &["openai"])], None);
    let visible = visible_lines(&state, 80);
    let joined = visible.join("\n");
    assert!(
        joined.contains("↑↓ navigate · R refresh · D delete · Enter add · Esc close"),
        "hint missing: {joined}"
    );
}

#[test]
fn render_has_borders() {
    let state = manager(vec![source("OpenAI", &["openai"])], None);
    let visible = visible_lines(&state, 40);
    let first = visible.first().unwrap();
    let last = visible.last().unwrap();
    assert!(first.starts_with('┌') && first.ends_with('┐'));
    assert!(last.starts_with('└') && last.ends_with('┘'));
}

#[test]
fn d_arms_delete_confirmation() {
    let mut state = manager(vec![source("OpenAI", &["openai"])], None);
    let result = state.handle_input(&InputEvent::Insert('D'));
    assert_eq!(result, InputResult::Handled);

    let visible = visible_lines(&state, 60);
    let joined = visible.join("\n");
    assert!(
        joined.contains("[y/N] delete OpenAI?"),
        "confirmation prompt missing: {joined}"
    );
    assert!(state.confirm.is_some());
}

#[test]
fn y_confirms_delete_source() {
    let mut state = manager(vec![source("OpenAI", &["openai"])], None);
    state.handle_input(&InputEvent::Insert('D'));
    let result = state.handle_input(&InputEvent::Insert('Y'));
    assert_eq!(result, InputResult::Submitted);
    assert_eq!(
        state.action.clone(),
        Some(ProviderManagerAction::DeleteSource(vec![
            "openai".to_owned()
        ]))
    );
    assert!(state.confirm.is_none());
}

#[test]
fn n_cancels_delete_confirmation() {
    let mut state = manager(vec![source("OpenAI", &["openai"])], None);
    state.handle_input(&InputEvent::Insert('D'));
    let result = state.handle_input(&InputEvent::Insert('n'));
    assert_eq!(result, InputResult::Handled);
    assert!(state.action.is_none());
    assert!(state.confirm.is_none());

    let visible = visible_lines(&state, 60);
    let joined = visible.join("\n");
    assert!(
        !joined.contains("[y/N] delete"),
        "confirmation prompt should be gone: {joined}"
    );
}

#[test]
fn enter_on_add_row_returns_add() {
    let mut state = manager(
        vec![
            source("OpenAI", &["openai"]),
            source("Anthropic", &["anthropic"]),
        ],
        None,
    );
    // Move selection to the synthetic add row.
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    let result = state.handle_input(&InputEvent::Submit);
    assert_eq!(result, InputResult::Submitted);
    assert_eq!(state.action, Some(ProviderManagerAction::Add));
}

#[test]
fn enter_on_source_row_does_not_submit() {
    let mut state = manager(vec![source("OpenAI", &["openai"])], None);
    let result = state.handle_input(&InputEvent::Submit);
    assert_eq!(result, InputResult::Handled);
    assert!(state.action.is_none());
}

#[test]
fn esc_returns_close() {
    let mut state = manager(vec![source("OpenAI", &["openai"])], None);
    let result = state.handle_input(&InputEvent::Cancel);
    assert_eq!(result, InputResult::Cancelled);
    assert_eq!(state.action, Some(ProviderManagerAction::Close));
}

#[test]
fn esc_cancels_delete_confirmation() {
    let mut state = manager(vec![source("OpenAI", &["openai"])], None);
    state.handle_input(&InputEvent::Insert('D'));
    let result = state.handle_input(&InputEvent::Cancel);
    assert_eq!(result, InputResult::Handled);
    assert!(state.confirm.is_none());
    assert!(state.action.is_none());
}

#[test]
fn r_refreshes_selected_provider_but_not_add_or_confirmation() {
    for key in ['r', 'R'] {
        let mut state = manager(vec![source("OpenAI", &["openai"])], None);
        assert_eq!(
            state.handle_input(&InputEvent::Insert(key)),
            InputResult::Submitted
        );
        assert_eq!(
            state.take_action(),
            Some(ProviderManagerAction::Refresh("openai".to_owned()))
        );
    }

    let mut add = manager(vec![source("OpenAI", &["openai"])], None);
    add.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    assert_eq!(
        add.handle_input(&InputEvent::Insert('R')),
        InputResult::Handled
    );
    assert!(add.take_action().is_none());

    let mut confirming = manager(vec![source("OpenAI", &["openai"])], None);
    confirming.handle_input(&InputEvent::Insert('D'));
    assert_eq!(
        confirming.handle_input(&InputEvent::Insert('R')),
        InputResult::Ignored
    );
    assert!(confirming.take_action().is_none());
    assert!(confirming.confirm.is_some());
}

#[test]
fn set_options_preserves_selection_by_label() {
    let mut state = manager(
        vec![
            source("OpenAI", &["openai"]),
            source("Anthropic", &["anthropic"]),
        ],
        None,
    );
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    assert_eq!(state.selected_index, 1);

    state.set_options(&ProviderManagerOptions {
        sources: vec![
            source("OpenAI", &["openai"]),
            source("Anthropic", &["anthropic"]),
            source("Google", &["google"]),
        ],
        active_provider_id: None,
        theme: theme(),
    });

    assert_eq!(state.selected_index, 1);
}

#[test]
fn set_options_preserves_selection_by_provider_id() {
    let mut state = manager(
        vec![
            source("OpenAI", &["openai"]),
            source("Anthropic", &["anthropic"]),
        ],
        None,
    );
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    assert_eq!(state.selected_index, 1);

    state.set_options(&ProviderManagerOptions {
        sources: vec![source("Anthropic Renamed", &["anthropic"])],
        active_provider_id: None,
        theme: theme(),
    });

    assert_eq!(state.selected_index, 0);
}

#[test]
fn set_options_clamps_index_when_rows_shrink() {
    let mut state = manager(
        vec![
            source("OpenAI", &["openai"]),
            source("Anthropic", &["anthropic"]),
        ],
        None,
    );
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    // add row at index 2
    assert_eq!(state.selected_index, 2);

    state.set_options(&ProviderManagerOptions {
        sources: vec![source("OpenAI", &["openai"])],
        active_provider_id: None,
        theme: theme(),
    });

    assert_eq!(state.selected_index, 1);
}

#[test]
fn initial_selection_defaults_to_active_provider() {
    let state = manager(
        vec![
            source("OpenAI", &["openai"]),
            source("Anthropic", &["anthropic"]),
        ],
        Some("anthropic"),
    );
    assert_eq!(state.selected_index, 1);
}

#[test]
fn move_down_and_up_changes_selection() {
    let mut state = manager(
        vec![
            source("OpenAI", &["openai"]),
            source("Anthropic", &["anthropic"]),
        ],
        None,
    );
    assert_eq!(state.selected_index, 0);
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    assert_eq!(state.selected_index, 1);
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectUp));
    assert_eq!(state.selected_index, 0);
}

#[test]
fn delete_on_add_row_is_ignored() {
    let mut state = manager(
        vec![
            source("OpenAI", &["openai"]),
            source("Anthropic", &["anthropic"]),
        ],
        None,
    );
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    assert!(matches!(
        state.rows.get(state.selected_index),
        Some(Row::Add)
    ));

    let result = state.handle_input(&InputEvent::Insert('D'));
    assert_eq!(result, InputResult::Handled);
    assert!(state.confirm.is_none());
}
