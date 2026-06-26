use neo_tui::input::{KeyId, KeybindingAction, KeybindingsManager};
use neo_tui::primitive::{truncate_width, visible_width, wrap_width};
use neo_tui::shell::{NeoChromeState, PromptEdit, PromptState, SelectItem, SelectListState};
use neo_tui::transcript::{TranscriptEntry, TranscriptStore, TranscriptViewport};

fn strip_ansi_escapes(text: &str) -> String {
    let mut visible = String::new();
    let mut index = 0;
    while index < text.len() {
        if text.as_bytes().get(index).copied() == Some(0x1b)
            && let Some(end) = text[index..].find('m')
        {
            index += end + 1;
            continue;
        }

        let Some(character) = text[index..].chars().next() else {
            break;
        };
        visible.push(character);
        index += character.len_utf8();
    }
    visible
}

#[test]
fn keybinding_manager_matches_defaults_overrides_and_conflicts() {
    let mut manager = KeybindingsManager::default();

    assert!(manager.matches(
        &KeyId::new("ctrl+b").expect("valid key"),
        KeybindingAction::EditorCursorLeft
    ));
    assert!(manager.matches(
        &KeyId::new("left").expect("valid key"),
        KeybindingAction::EditorCursorLeft
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+n").expect("valid key"),
        KeybindingAction::SessionFork
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+space").expect("valid key"),
        KeybindingAction::TranscriptSelectionStart
    ));
    assert!(manager.matches(
        &KeyId::new("shift+up").expect("valid key"),
        KeybindingAction::TranscriptSelectionExtendUp
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+c").expect("valid key"),
        KeybindingAction::TranscriptCopySelection
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+o").expect("valid key"),
        KeybindingAction::ToolOutputToggle
    ));
    assert!(!manager.matches(
        &KeyId::new("ctrl+o").expect("valid key"),
        KeybindingAction::ModelPickerOpen
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+c").expect("valid key"),
        KeybindingAction::AppClear
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+d").expect("valid key"),
        KeybindingAction::AppExit
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+z").expect("valid key"),
        KeybindingAction::AppSuspend
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+_").expect("valid key"),
        KeybindingAction::EditorUndo
    ));
    assert!(manager.matches(
        &KeyId::new("ctrl+p").expect("valid key"),
        KeybindingAction::CommandPaletteOpen
    ));
    assert!(!manager.matches(
        &KeyId::new("ctrl+c").expect("valid key"),
        KeybindingAction::EditorCursorLeft
    ));

    manager.set_user_bindings([(
        KeybindingAction::EditorCursorLeft,
        vec![KeyId::new("alt+h").expect("valid key")],
    )]);

    assert!(manager.matches(
        &KeyId::new("alt+h").expect("valid key"),
        KeybindingAction::EditorCursorLeft
    ));
    assert!(!manager.matches(
        &KeyId::new("left").expect("valid key"),
        KeybindingAction::EditorCursorLeft
    ));

    manager.set_user_bindings([
        (
            KeybindingAction::EditorCursorLeft,
            vec![KeyId::new("alt+h").expect("valid key")],
        ),
        (
            KeybindingAction::EditorCursorRight,
            vec![KeyId::new("alt+h").expect("valid key")],
        ),
    ]);

    let conflicts = manager.conflicts();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].key, KeyId::new("alt+h").expect("valid key"));
}

#[test]
fn transcript_viewport_tracks_bottom_and_manual_scroll() {
    let mut view = TranscriptViewport::new();

    view.sync(8, 3);
    let bottom = view.visible_row_range(8, 3);
    assert_eq!(bottom, 5..8);

    view.scroll_up(2);
    assert_eq!(view.visible_row_range(8, 3), 3..6);

    view.scroll_down(1);
    assert_eq!(view.visible_row_range(8, 3), 4..7);

    view.follow_bottom();
    assert_eq!(view.visible_row_range(8, 3), 5..8);
}

#[test]
fn transcript_viewport_syncs_visual_row_scrollback_and_follow_tail() {
    let mut view = TranscriptViewport::new();

    view.sync(40, 10);
    assert_eq!(view.visible_row_range(40, 10), 30..40);

    view.scroll_up(12);
    assert_eq!(view.visible_row_range(40, 10), 18..28);

    view.sync(8, 10);
    assert_eq!(view.visible_row_range(8, 10), 0..8);

    view.scroll_up(4);
    view.follow_bottom();
    view.sync(80, 12);
    assert_eq!(view.visible_row_range(80, 12), 68..80);
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
fn prompt_history_does_not_overwrite_non_empty_draft_on_first_up() {
    let mut prompt = PromptState::new("partial").with_cursor(7);
    prompt.remember_history("old prompt");

    assert!(!prompt.recall_previous_history());
    assert_eq!(prompt.text, "partial");
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
    prompt.apply_edit_with_width(PromptEdit::MoveEnd, 4);
    // Cursor is on the last line; viewport should scroll so the cursor is visible.
    assert!(prompt.scroll_offset() > 0);

    // Move to the first line; viewport should scroll back to the top.
    prompt.apply_edit_with_width(PromptEdit::MoveHome, 4);
    assert_eq!(prompt.scroll_offset(), 0);
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
fn ansi_width_cases_are_display_width_safe() {
    struct Case {
        name: &'static str,
        input: &'static str,
        width: usize,
        expected_width: usize,
    }

    let cases = [
        Case {
            name: "plain ascii",
            input: "hello",
            width: 10,
            expected_width: 5,
        },
        Case {
            name: "ansi sgr ignored",
            input: "\x1b[31mred\x1b[0m",
            width: 10,
            expected_width: 3,
        },
        Case {
            name: "osc ignored",
            input: "\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\",
            width: 10,
            expected_width: 4,
        },
        Case {
            name: "wide cjk",
            input: "你好",
            width: 10,
            expected_width: 4,
        },
    ];

    for case in &cases {
        assert_eq!(
            visible_width(case.input),
            case.expected_width,
            "{}",
            case.name
        );
        for line in wrap_width(case.input, case.width) {
            assert!(
                visible_width(&line) <= case.width,
                "{} overflowed: {line:?}",
                case.name
            );
        }
    }
}

#[test]
fn wrap_width_preserves_display_width_for_wide_text() {
    let lines = wrap_width("ab界cd🙂ef", 5);

    assert_eq!(lines.concat(), "ab界cd🙂ef");
    assert!(
        lines
            .iter()
            .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) <= 5)
    );
}

#[test]
fn wrap_width_preserves_ansi_sequences_without_counting_them() {
    let red = "\x1b[31m";
    let reset = "\x1b[0m";
    let input = format!("{red}abcdef{reset}");
    let lines = wrap_width(&input, 3);

    assert_eq!(
        lines
            .iter()
            .map(|line| strip_ansi_escapes(line))
            .collect::<String>(),
        "abcdef"
    );
    assert_eq!(lines.len(), 2);
    assert!(lines[0].starts_with(red));
    assert!(lines[1].ends_with(reset));
    assert!(lines.iter().all(|line| visible_width(line) <= 3));
}

#[test]
fn wrap_width_rehydrates_active_ansi_style_on_continuation_lines() {
    let red_bold = "\x1b[31;1m";
    let reset = "\x1b[0m";
    let input = format!("{red_bold}abcdef{reset}");
    let lines = wrap_width(&input, 3);

    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines
            .iter()
            .map(|line| strip_ansi_escapes(line))
            .collect::<String>(),
        "abcdef"
    );
    assert!(lines[0].starts_with(red_bold));
    assert!(lines[1].starts_with(red_bold));
    assert!(lines[1].ends_with(reset));
    assert!(lines.iter().all(|line| visible_width(line) <= 3));
}

#[test]
fn wrap_width_rehydrates_multiple_active_ansi_styles_on_continuation_lines() {
    let red = "\x1b[31m";
    let bold = "\x1b[1m";
    let reset = "\x1b[0m";
    let input = format!("{red}{bold}abcdef{reset}");
    let lines = wrap_width(&input, 3);

    assert_eq!(lines.len(), 2);
    assert!(lines[1].starts_with(&format!("{red}{bold}")));
    assert_eq!(visible_width(&lines[1]), 3);
}

#[test]
fn wrap_width_rehydrates_sgr_sequences_that_reset_then_set_style() {
    let reset_then_red = "\x1b[0;31m";
    let reset = "\x1b[0m";
    let input = format!("{reset_then_red}abcdef{reset}");
    let lines = wrap_width(&input, 3);

    assert_eq!(lines.len(), 2);
    assert!(lines[1].starts_with(reset_then_red));
    assert_eq!(visible_width(&lines[1]), 3);
}

#[test]
fn wrap_width_stops_rehydrating_style_after_reset() {
    let red = "\x1b[31m";
    let reset = "\x1b[0m";
    let input = format!("{red}ab{reset}cdef");
    let lines = wrap_width(&input, 3);

    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines
            .iter()
            .map(|line| strip_ansi_escapes(line))
            .collect::<String>(),
        "abcdef"
    );
    assert!(!lines[1].starts_with(red));
    assert_eq!(visible_width(&lines[1]), 3);
}

#[test]
fn truncate_width_does_not_split_ansi_or_osc_sequences() {
    let input = "\x1b]133;A\x07\x1b[32mabcdef\x1b[0m";
    let truncated = truncate_width(input, 4, "..", false);

    assert!(truncated.starts_with("\x1b]133;A\x07\x1b[32m"));
    assert_eq!(visible_width(&truncated), 4);
    assert_eq!(truncated, "\x1b]133;A\x07\x1b[32mab..");
}

#[test]
fn truncate_width_is_display_width_safe_and_can_pad() {
    assert_eq!(truncate_width("abcdef", 4, "...", false), "a...");
    assert_eq!(truncate_width("abcdef", 4, "", false), "abcd");

    let truncated = truncate_width("ab界🙂cd", 6, "..", true);
    assert_eq!(unicode_width::UnicodeWidthStr::width(truncated.as_str()), 6);
    assert!(truncated.contains(".."));
}

#[test]
fn wrap_width_breaks_long_words_and_keeps_blank_lines() {
    let lines = wrap_width("alpha\n\nsuperwide", 4);

    assert_eq!(lines[0], "alph");
    assert_eq!(lines[1], "a");
    assert_eq!(lines[2], "");
    assert!(
        lines
            .iter()
            .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) <= 4)
    );
}

#[test]
fn select_list_filters_wraps_and_reports_visible_window() {
    let mut list = SelectListState::new(
        [
            SelectItem::new("open", "Open", Some("Open a file")),
            SelectItem::new("close", "Close", Some("Close the active file")),
            SelectItem::new("copy", "Copy", Some("Copy selection")),
            SelectItem::new("commit", "Commit", Some("Commit staged changes")),
        ],
        2,
    );

    list.set_filter("c");
    assert_eq!(list.filtered_len(), 3);
    assert_eq!(list.selected_item().expect("selected").value, "close");

    list.move_down();
    assert_eq!(list.selected_item().expect("selected").value, "copy");
    assert_eq!(list.visible_range(), 0..2);

    list.move_down();
    assert_eq!(list.selected_item().expect("selected").value, "commit");
    assert_eq!(list.visible_range(), 1..3);

    list.move_down();
    assert_eq!(list.selected_item().expect("selected").value, "close");

    let lines = list.render_lines(18);
    assert_eq!(lines.len(), 3);
    assert!(lines[0].contains("> Close"));
    assert!(lines[2].contains("(1/3)"));
    assert!(
        lines
            .iter()
            .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) <= 18)
    );
}

#[test]
fn select_list_pages_by_visible_window_and_clamps() {
    let mut list = SelectListState::new(
        (0..10).map(|index| {
            SelectItem::new(
                format!("item-{index}"),
                format!("Item {index}"),
                None::<String>,
            )
        }),
        4,
    );

    list.page_down();
    assert_eq!(list.selected_item().expect("selected").value, "item-4");
    assert_eq!(list.visible_range(), 2..6);

    list.page_down();
    assert_eq!(list.selected_item().expect("selected").value, "item-8");
    assert_eq!(list.visible_range(), 6..10);

    list.page_down();
    assert_eq!(list.selected_item().expect("selected").value, "item-9");
    assert_eq!(list.visible_range(), 6..10);

    list.page_up();
    assert_eq!(list.selected_item().expect("selected").value, "item-5");
    assert_eq!(list.visible_range(), 3..7);

    list.page_up();
    assert_eq!(list.selected_item().expect("selected").value, "item-1");
    assert_eq!(list.visible_range(), 0..4);

    list.page_up();
    assert_eq!(list.selected_item().expect("selected").value, "item-0");
    assert_eq!(list.visible_range(), 0..4);
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
fn transcript_selection_copies_item_range_with_roles() {
    let mut transcript = TranscriptStore::new();
    transcript.push(TranscriptEntry::user_message("first prompt"));
    transcript.push(TranscriptEntry::assistant_message("first answer"));
    transcript.push_tool_run("tool-1", "shell.run", Some("exit 0".to_owned()));
    if let Some(tool) = transcript.tool_mut("tool-1") {
        tool.set_result(Some("exit 0".to_owned()), None, false, Some(0));
    }
    transcript.push(TranscriptEntry::status("done"));

    transcript.select_visible_entry();
    transcript.extend_selection_up(2);

    assert_eq!(
        transcript.copy_selection().as_deref(),
        Some("Assistant\nfirst answer\n\nTool\n+ shell.run (exit 0)\n\nStatus\ndone")
    );
}
