use neo_tui::shell::{NeoChromeState, PromptEdit, PromptState};

#[test]
fn prompt_completion_prefix_replaces_token_before_cursor() {
    let mut prompt = PromptState::new("open src/ma").with_cursor(11);
    let prefix = prompt
        .completion_prefix()
        .expect("cursor is inside a completable token");

    assert_eq!(prefix.start, 5);
    assert_eq!(prefix.end, 11);
    assert_eq!(prefix.text, "src/ma");

    assert_eq!(
        prompt.replace_completion_prefix(&prefix, "src/main.rs"),
        Some("src/main.rs".to_owned())
    );
    assert_eq!(prompt.text, "open src/main.rs");
    assert_eq!(prompt.cursor, 16);

    prompt.apply_edit(PromptEdit::Undo);
    assert_eq!(prompt.text, "open src/ma");
    assert_eq!(prompt.cursor, 11);
}

#[test]
fn prompt_copy_uses_internal_buffer_without_mutating_editor_state() {
    let mut prompt = PromptState::new("hello world").with_cursor(5);

    assert_eq!(prompt.copy_text().as_deref(), Some("hello world"));
    assert_eq!(prompt.text, "hello world");
    assert_eq!(prompt.cursor, 5);
    assert_eq!(prompt.apply_edit(PromptEdit::Yank), None);

    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.prompt_mut().apply_edit(PromptEdit::Insert("copy me"));

    assert_eq!(app.copy_prompt_text().as_deref(), Some("copy me"));
    assert_eq!(app.copy_buffer(), Some("copy me"));
    assert_eq!(app.prompt().text, "copy me");
    assert_eq!(app.prompt().cursor, 7);
}

#[test]
fn prompt_delete_to_line_end_stays_on_current_logical_line() {
    let mut prompt = PromptState::new("ABC\nQWE\nXYZ").with_cursor(5);

    let deleted = prompt.apply_edit(PromptEdit::DeleteToLineEnd);

    assert_eq!(deleted, Some("WE".to_owned()));
    assert_eq!(prompt.text, "ABC\nQ\nXYZ");
    assert_eq!(prompt.cursor, 5);
}

#[test]
fn prompt_delete_to_line_start_stays_on_current_logical_line() {
    let mut prompt = PromptState::new("ABC\nQWE\nXYZ").with_cursor(11);

    let deleted = prompt.apply_edit(PromptEdit::DeleteToLineStart);

    assert_eq!(deleted, Some("XYZ".to_owned()));
    assert_eq!(prompt.text, "ABC\nQWE\n");
    assert_eq!(prompt.cursor, 8);
}

#[test]
fn prompt_edit_applies_character_and_word_operations() {
    let mut prompt = PromptState::new("hello world").with_cursor(5);

    assert_eq!(
        prompt.apply_edit(PromptEdit::Insert(", brave")),
        Some(", brave".into())
    );
    assert_eq!(prompt.text, "hello, brave world");
    assert_eq!(prompt.cursor, 12);

    assert_eq!(prompt.apply_edit(PromptEdit::MoveWordLeft), None);
    assert_eq!(prompt.cursor, 7);

    assert_eq!(
        prompt.apply_edit(PromptEdit::DeleteWordForward),
        Some("brave".into())
    );
    assert_eq!(prompt.text, "hello,  world");
    assert_eq!(prompt.cursor, 7);

    assert_eq!(prompt.apply_edit(PromptEdit::MoveEnd), None);
    assert_eq!(
        prompt.apply_edit(PromptEdit::DeleteWordBackward),
        Some("world".into())
    );
    assert_eq!(prompt.text, "hello,  ");
    assert_eq!(prompt.cursor, 8);

    assert_eq!(
        prompt.apply_edit(PromptEdit::DeleteToLineStart),
        Some("hello,  ".into())
    );
    assert_eq!(prompt.text, "");
    assert_eq!(prompt.cursor, 0);
}

#[test]
fn prompt_edit_clear_removes_text_and_can_be_undone() {
    let mut prompt = PromptState::new("draft text");

    assert_eq!(
        prompt.apply_edit(PromptEdit::Clear).as_deref(),
        Some("draft text")
    );
    assert_eq!(prompt.text, "");
    assert_eq!(prompt.cursor, 0);

    prompt.apply_edit(PromptEdit::Undo);
    assert_eq!(prompt.text, "draft text");
    assert_eq!(prompt.cursor, 10);
}

#[test]
fn prompt_edit_tracks_undo_and_kill_ring_yank() {
    let mut prompt = PromptState::new("hello brave world").with_cursor(6);

    assert_eq!(
        prompt.apply_edit(PromptEdit::DeleteToLineEnd),
        Some("brave world".into())
    );
    assert_eq!(prompt.text, "hello ");

    assert_eq!(
        prompt.apply_edit(PromptEdit::Yank),
        Some("brave world".into())
    );
    assert_eq!(prompt.text, "hello brave world");
    assert_eq!(prompt.cursor, 17);

    assert_eq!(prompt.apply_edit(PromptEdit::Undo), None);
    assert_eq!(prompt.text, "hello ");
    assert_eq!(prompt.cursor, 6);

    assert_eq!(prompt.apply_edit(PromptEdit::Undo), None);
    assert_eq!(prompt.text, "hello brave world");
    assert_eq!(prompt.cursor, 6);
}

#[test]
fn prompt_history_continues_navigation_after_history_entry_is_active() {
    let mut prompt = PromptState::default();
    prompt.remember_history("first");
    prompt.remember_history("second");

    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "second");
    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "first");
}

#[test]
fn prompt_history_does_not_overwrite_non_empty_draft_on_first_up() {
    let mut prompt = PromptState::new("partial").with_cursor(7);
    prompt.remember_history("old prompt");

    assert!(!prompt.recall_previous_history());
    assert_eq!(prompt.text, "partial");
}

#[test]
fn prompt_history_recalls_entries_and_restores_draft() {
    let mut prompt = PromptState::default();
    prompt.remember_history("first prompt");
    prompt.remember_history("second prompt");
    // Seed a draft that the first Up must preserve (non-empty composer).
    prompt.apply_edit(PromptEdit::Insert("draft"));
    assert!(!prompt.recall_previous_history());
    assert_eq!(prompt.text, "draft");

    // Clear the draft so Up can start navigation from an empty composer.
    prompt.apply_edit(PromptEdit::Clear);
    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "second prompt");
    assert_eq!(prompt.cursor, 13);

    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "first prompt");
    assert_eq!(prompt.cursor, 12);

    assert!(prompt.recall_next_history());
    assert_eq!(prompt.text, "second prompt");

    // Down past the newest entry restores the (now empty) draft.
    assert!(prompt.recall_next_history());
    assert_eq!(prompt.text, "");

    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "second prompt");
    prompt.apply_edit(PromptEdit::Insert(" edited"));
    assert_eq!(prompt.text, "second prompt edited");
    assert!(!prompt.recall_next_history());
}

#[test]
fn prompt_history_set_history_trims_and_dedupes_consecutive_entries() {
    let mut prompt = PromptState::default();
    prompt.set_history([
        "  alpha  ".to_owned(),
        "alpha".to_owned(),
        String::new(),
        "beta".to_owned(),
        "gamma".to_owned(),
    ]);

    // Newest first.
    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "gamma");
    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "beta");
    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "alpha");
    // No duplicate alpha.
    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "alpha");
}

#[test]
fn prompt_history_skips_blank_and_consecutive_duplicates() {
    let mut prompt = PromptState::default();
    prompt.remember_history("  first prompt  ");
    prompt.remember_history("first prompt");
    prompt.remember_history("   ");
    prompt.remember_history("second prompt");

    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "second prompt");
    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "first prompt");
    // Clamped at oldest entry; no duplicate "first prompt" is stored.
    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "first prompt");
}

#[test]
fn prompt_move_end_stays_on_current_logical_line() {
    let mut prompt = PromptState::new("ABC\nQWE\nXYZ").with_cursor(5);

    prompt.apply_edit(PromptEdit::MoveEnd);

    assert_eq!(prompt.text, "ABC\nQWE\nXYZ");
    assert_eq!(prompt.cursor, 7);
}

#[test]
fn prompt_move_home_stays_on_current_logical_line() {
    let mut prompt = PromptState::new("ABC\nQWE\nXYZ").with_cursor(11);

    prompt.apply_edit(PromptEdit::MoveHome);

    assert_eq!(prompt.text, "ABC\nQWE\nXYZ");
    assert_eq!(prompt.cursor, 8);
}

#[test]
fn prompt_move_up_down_treats_tabs_as_four_columns() {
    // At body_width 4, "ab\tcd" expands to 8 columns and wraps after "ab\t".
    let mut prompt = PromptState::new("ab\tcd\nef").with_cursor(7);
    prompt.apply_edit(PromptEdit::MoveUp(4));
    // Cursor should land in the second wrapped segment of the first source line.
    assert_eq!(prompt.cursor, 4);
    prompt.apply_edit(PromptEdit::MoveDown(4));
    assert_eq!(prompt.cursor, 7);
}

#[test]
fn prompt_move_up_down_wraps_logical_lines() {
    // Body width of 4 forces each logical source line to wrap to two display rows.
    let mut prompt = PromptState::new("abcd\nefgh").with_cursor(9);
    prompt.apply_edit(PromptEdit::MoveUp(4));
    // Cursor should land near the start of the second wrapped row of the first line.
    assert_eq!(prompt.text, "abcd\nefgh");
    assert_eq!(prompt.cursor, 4);

    prompt.apply_edit(PromptEdit::MoveDown(4));
    assert_eq!(prompt.cursor, 9);
}

#[test]
fn prompt_scroll_offset_keeps_cursor_visible() {
    let mut prompt = PromptState::default();
    // Insert nine newlines so there are ten display rows at body_width 4.
    for _ in 0..9 {
        prompt.apply_edit(PromptEdit::Insert("\n"));
    }
    prompt.apply_edit(PromptEdit::Insert("x"));
    prompt.cursor = prompt.char_len();
    prompt.apply_edit_with_width(PromptEdit::MoveRight, 4);
    // Cursor is on the last line; viewport should scroll so the cursor is visible.
    assert!(prompt.scroll_offset() > 0);

    // Move to the first line; viewport should scroll back to the top.
    prompt.cursor = 0;
    prompt.apply_edit_with_width(PromptEdit::MoveLeft, 4);
    assert_eq!(prompt.scroll_offset(), 0);
}
