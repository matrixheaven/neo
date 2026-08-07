use super::*;

fn entry(id: &str, name: &str) -> ThemeCatalogEntrySnapshot {
    ThemeCatalogEntrySnapshot {
        id: id.to_owned(),
        display_name: name.to_owned(),
        theme: Some(TuiTheme::default()),
        error: None,
        active: false,
        startup_default: false,
    }
}

fn invalid_entry(id: &str, name: &str) -> ThemeCatalogEntrySnapshot {
    ThemeCatalogEntrySnapshot {
        id: id.to_owned(),
        display_name: name.to_owned(),
        theme: None,
        error: Some("expected token".to_owned()),
        active: false,
        startup_default: false,
    }
}

fn sample(entries: Vec<ThemeCatalogEntrySnapshot>) -> ThemeManagerState {
    let mut state = ThemeManagerState::new("openai/gpt-4.1");
    state.apply_snapshot(entries);
    state
}

fn action(state: &mut ThemeManagerState) -> Option<ThemeManagerAction> {
    state.take_action()
}

#[test]
fn snapshot_sorts_by_display_name_then_id() {
    let state = sample(vec![
        entry("z.json", "Zebra"),
        entry("a.json", "alpha"),
        entry("b.json", "Alpha"),
    ]);
    let ids = state
        .entries()
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    // Case-insensitive name sort; the "alpha" tie is broken by id.
    assert_eq!(ids, vec!["a.json", "b.json", "z.json"]);
}

#[test]
fn snapshot_keeps_stable_selection_by_id() {
    let mut state = sample(vec![
        entry("a.json", "Alpha"),
        entry("b.json", "Beta"),
        entry("c.json", "Gamma"),
    ]);
    assert_eq!(state.selected_id(), Some("a.json"));
    assert!(state.select_id("c.json"));
    assert_eq!(state.preview(), Some(TuiTheme::default()));

    // Refresh with the same catalog restores the id.
    state.apply_snapshot(vec![
        entry("a.json", "Alpha"),
        entry("b.json", "Beta"),
        entry("c.json", "Gamma"),
    ]);
    assert_eq!(state.selected_id(), Some("c.json"));
}

#[test]
fn snapshot_restores_nearest_entry_after_delete() {
    let mut state = sample(vec![
        entry("a.json", "Alpha"),
        entry("b.json", "Beta"),
        entry("c.json", "Gamma"),
        entry("d.json", "Delta"),
    ]);
    assert!(state.select_id("d.json"));
    // The selected id disappears; the entry nearest its old position wins.
    state.apply_snapshot(vec![
        entry("a.json", "Alpha"),
        entry("b.json", "Beta"),
        entry("c.json", "Gamma"),
    ]);
    assert_eq!(state.selected_id(), Some("c.json"));
}

#[test]
fn selection_moves_preview_only_and_never_emits() {
    let mut state = sample(vec![entry("a.json", "Alpha"), entry("b.json", "Beta")]);
    assert!(!state.has_action());
    let _ = state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
    assert_eq!(state.selected_id(), Some("b.json"));
    assert_eq!(state.preview(), Some(TuiTheme::default()));
    assert!(!state.has_action(), "navigation must not emit actions");
    let _ = state.handle_input(&InputEvent::Insert('k'));
    assert_eq!(state.selected_id(), Some("a.json"));
}

#[test]
fn jk_navigation_wraps() {
    let mut state = sample(vec![entry("a.json", "Alpha"), entry("b.json", "Beta")]);
    let _ = state.handle_input(&InputEvent::Insert('k'));
    assert_eq!(state.selected_id(), Some("b.json"));
    let _ = state.handle_input(&InputEvent::Insert('j'));
    assert_eq!(state.selected_id(), Some("a.json"));
}

#[test]
fn page_and_boundary_navigation() {
    let mut state = sample(vec![
        entry("1.json", "Alpha"),
        entry("2.json", "Bravo"),
        entry("3.json", "Charlie"),
        entry("4.json", "Delta"),
        entry("5.json", "Echo"),
    ]);
    assert_eq!(state.selected_id(), Some("1.json"));
    state.set_page_size(2);
    let _ = state.handle_input(&InputEvent::Action(KeybindingAction::SelectPageDown));
    assert_eq!(state.selected_id(), Some("3.json"));
    let _ = state.handle_input(&InputEvent::Action(KeybindingAction::SelectPageDown));
    assert_eq!(state.selected_id(), Some("5.json"));
    let _ = state.handle_input(&InputEvent::Action(KeybindingAction::SelectPageUp));
    assert_eq!(state.selected_id(), Some("3.json"));
    let _ = state.handle_input(&InputEvent::MoveHome);
    assert_eq!(state.selected_id(), Some("1.json"));
    let _ = state.handle_input(&InputEvent::MoveEnd);
    assert_eq!(state.selected_id(), Some("5.json"));
}

#[test]
fn filter_edit_commits_backspace_and_esc_contract() {
    let mut state = sample(vec![
        entry("solar.json", "Solarized"),
        entry("aurora.json", "Aurora Night"),
    ]);
    let _ = state.handle_input(&InputEvent::Insert('/'));
    assert_eq!(state.focus(), ThemeManagerFocus::Filter);
    for character in ['s', 'o', 'l', 'a', 'r'] {
        let _ = state.handle_input(&InputEvent::Insert(character));
    }
    assert_eq!(state.filter(), "solar");
    assert_eq!(state.filtered_count(), 1);
    assert_eq!(state.selected_id(), Some("solar.json"));
    // Enter commits the filter and returns to the list.
    let _ = state.handle_input(&InputEvent::Submit);
    assert_eq!(state.focus(), ThemeManagerFocus::List);
    assert_eq!(state.filter(), "solar");
    // Backspace removes one character.
    let _ = state.handle_input(&InputEvent::Insert('/'));
    let _ = state.handle_input(&InputEvent::Backspace);
    assert_eq!(state.filter(), "sola");
    assert_eq!(state.filtered_count(), 1);
    // Esc clears first …
    let _ = state.handle_input(&InputEvent::Cancel);
    assert_eq!(state.filter(), "");
    assert!(!state.has_action());
    // … then closes: the chrome owns the close, no Close action is queued.
    assert_eq!(
        state.handle_input(&InputEvent::Cancel),
        InputResult::Cancelled
    );
    assert!(!state.has_action());
}

#[test]
fn tab_cycles_focus_and_shift_tab_goes_backward() {
    let mut state = sample(vec![entry("a.json", "Alpha")]);
    assert_eq!(state.focus(), ThemeManagerFocus::List);
    let _ = state.handle_input(&InputEvent::Insert('\t'));
    assert_eq!(state.focus(), ThemeManagerFocus::Preview);
    let _ = state.handle_input(&InputEvent::Insert('\t'));
    assert_eq!(state.focus(), ThemeManagerFocus::Actions);
    let _ = state.handle_input(&InputEvent::Insert('\t'));
    assert_eq!(state.focus(), ThemeManagerFocus::Filter);
    let _ = state.handle_input(&InputEvent::Insert('\t'));
    assert_eq!(state.focus(), ThemeManagerFocus::List);
    let shift_tab = crate::input::KeyId::new("shift+tab")
        .map(InputEvent::Key)
        .expect("valid key id");
    let _ = state.handle_input(&shift_tab);
    assert_eq!(state.focus(), ThemeManagerFocus::Filter);
}

#[test]
fn enter_applies_selected_and_closes() {
    let mut state = sample(vec![entry("a.json", "Alpha")]);
    assert_eq!(
        state.handle_input(&InputEvent::Submit),
        InputResult::Submitted
    );
    // Exactly one action: ApplySession. No Close is queued — the chrome
    // closes the overlay on `Submitted`, so a later poll cannot re-apply.
    assert_eq!(
        action(&mut state),
        Some(ThemeManagerAction::ApplySession("a.json".to_owned()))
    );
    assert!(!state.has_action());
}

#[test]
fn set_startup_default_emits_without_applying_or_closing() {
    let mut state = sample(vec![entry("a.json", "Alpha")]);
    let _ = state.handle_input(&InputEvent::Insert('D'));
    assert_eq!(
        action(&mut state),
        Some(ThemeManagerAction::SetStartupDefault("a.json".to_owned()))
    );
    assert!(!state.has_action());
    // The current session selection and preview are untouched.
    assert_eq!(state.selected_id(), Some("a.json"));
    assert_eq!(state.preview(), Some(TuiTheme::default()));
}

#[test]
fn invalid_entry_cannot_be_applied_or_defaulted() {
    let mut state = sample(vec![invalid_entry("broken.json", "Broken")]);
    assert_eq!(
        state.handle_input(&InputEvent::Submit),
        InputResult::Handled
    );
    assert!(!state.has_action());
    assert!(state.status().is_some_and(|status| status.is_error));
    let _ = state.handle_input(&InputEvent::Insert('D'));
    assert!(!state.has_action());
    assert!(state.status().is_some_and(|status| status.is_error));
}

#[test]
fn delete_requires_confirmation_and_guards_active_default() {
    let mut state = sample(vec![entry("a.json", "Alpha")]);
    let _ = state.handle_input(&InputEvent::Insert('X'));
    assert_eq!(state.pending(), Some(ThemeManagerPending::Delete));
    assert_eq!(
        state.handle_input(&InputEvent::Submit),
        InputResult::Handled
    );
    assert_eq!(state.pending(), None);
    assert_eq!(
        action(&mut state),
        Some(ThemeManagerAction::Delete("a.json".to_owned()))
    );

    // Esc cancels the confirmation without emitting.
    let _ = state.handle_input(&InputEvent::Insert('X'));
    assert_eq!(state.pending(), Some(ThemeManagerPending::Delete));
    let _ = state.handle_input(&InputEvent::Cancel);
    assert_eq!(state.pending(), None);
    assert!(!state.has_action());

    // Active and startup-default entries are guarded.
    let mut active = sample(vec![entry("a.json", "Active")]);
    active.entries[0].active = true;
    let _ = active.handle_input(&InputEvent::Insert('X'));
    assert_eq!(active.pending(), None);
    assert!(active.status().is_some_and(|status| status.is_error));

    let mut defaulted = sample(vec![entry("d.json", "Default")]);
    defaulted.entries[0].startup_default = true;
    let _ = defaulted.handle_input(&InputEvent::Insert('x'));
    assert_eq!(defaulted.pending(), None);
    assert!(defaulted.status().is_some_and(|status| status.is_error));
}

#[test]
fn import_and_copy_pending_feed_external_dialog_results() {
    let mut state = sample(vec![entry("a.json", "Alpha")]);
    let _ = state.handle_input(&InputEvent::Insert('I'));
    assert_eq!(state.pending(), Some(ThemeManagerPending::ImportPath));
    assert_eq!(
        state.submit_import_path(Some("/tmp/theme.json".to_owned())),
        Some(ThemeManagerAction::Import {
            path: "/tmp/theme.json".to_owned(),
            conflict_policy: ThemeConflictPolicy::Ask,
        })
    );
    assert_eq!(state.pending(), None);

    // Cancel leaves nothing queued.
    let _ = state.handle_input(&InputEvent::Insert('i'));
    assert!(state.submit_import_path(None).is_none());
    assert_eq!(state.pending(), None);

    let _ = state.handle_input(&InputEvent::Insert('C'));
    assert_eq!(state.pending(), Some(ThemeManagerPending::CopyName));
    assert_eq!(
        state.submit_copy_name(Some("Alpha Copy".to_owned())),
        Some(ThemeManagerAction::Duplicate {
            id: "a.json".to_owned(),
            new_display_name: "Alpha Copy".to_owned(),
        })
    );
    assert_eq!(state.pending(), None);
}

#[test]
fn refresh_emits_rescan_action() {
    let mut state = sample(vec![entry("a.json", "Alpha")]);
    let _ = state.handle_input(&InputEvent::Insert('r'));
    assert_eq!(action(&mut state), Some(ThemeManagerAction::Refresh));
}

#[test]
fn filter_text_inserts_while_filter_focused() {
    let mut state = sample(vec![entry("a.json", "Alpha")]);
    let _ = state.handle_input(&InputEvent::Insert('/'));
    // Letters type into the filter, not shortcuts.
    let _ = state.handle_input(&InputEvent::Insert('d'));
    let _ = state.handle_input(&InputEvent::Insert('X'));
    assert_eq!(state.filter(), "dX");
    assert_eq!(state.focus(), ThemeManagerFocus::Filter);
    assert!(!state.has_action());
}
