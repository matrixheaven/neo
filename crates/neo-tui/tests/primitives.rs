use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use neo_tui::{
    InputEvent, InputParser, KeyId, KeybindingAction, KeybindingsManager, NeoTuiApp, PromptEdit,
    PromptState, SelectItem, SelectListState, TranscriptEntry, TranscriptStore, TranscriptViewport,
    truncate_width, visible_width, wrap_width,
};

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
fn input_event_maps_printable_submit_escape_and_ctrl_c() {
    let typed =
        KeyEvent::new_with_kind(KeyCode::Char('界'), KeyModifiers::NONE, KeyEventKind::Press);
    let submit = KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Press);
    let escape = KeyEvent::new_with_kind(KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Press);
    let interrupt = KeyEvent::new_with_kind(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
        KeyEventKind::Press,
    );
    let release = KeyEvent::new_with_kind(
        KeyCode::Char('x'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    );

    assert_eq!(
        InputEvent::from_key_event(typed),
        Some(InputEvent::Insert('界'))
    );
    assert_eq!(InputEvent::from_key_event(submit), Some(InputEvent::Submit));
    assert_eq!(InputEvent::from_key_event(escape), Some(InputEvent::Cancel));
    assert_eq!(
        InputEvent::from_key_event(interrupt),
        Some(InputEvent::Interrupt)
    );
    assert_eq!(InputEvent::from_key_event(release), None);
}

#[test]
fn input_event_with_keybindings_keeps_bare_printable_chars_as_text() {
    let mut keybindings = KeybindingsManager::default();
    keybindings.set_user_bindings([(
        KeybindingAction::CommandPaletteOpen,
        vec![KeyId::new("g").expect("valid key")],
    )]);

    let event =
        KeyEvent::new_with_kind(KeyCode::Char('g'), KeyModifiers::NONE, KeyEventKind::Press);

    assert_eq!(
        InputEvent::from_key_event_with_keybindings(event, &keybindings),
        Some(InputEvent::Insert('g'))
    );
}

#[test]
fn input_event_with_keybindings_maps_modified_chars_to_keys() {
    let mut keybindings = KeybindingsManager::default();
    keybindings.set_user_bindings([(
        KeybindingAction::CommandPaletteOpen,
        vec![KeyId::new("ctrl+g").expect("valid key")],
    )]);

    let event = KeyEvent::new_with_kind(
        KeyCode::Char('g'),
        KeyModifiers::CONTROL,
        KeyEventKind::Press,
    );

    assert_eq!(
        InputEvent::from_key_event_with_keybindings(event, &keybindings),
        Some(InputEvent::Key(KeyId::new("ctrl+g").expect("valid key")))
    );
}

#[test]
fn input_event_maps_terminal_resize_events() {
    assert_eq!(
        InputEvent::from_crossterm_event(&Event::Resize(100, 30)),
        Some(InputEvent::Resize {
            columns: 100,
            rows: 30,
        })
    );
}

#[test]
fn input_event_ignores_mouse_events_in_inline_mode() {
    use crossterm::event::{MouseEvent, MouseEventKind};

    let scroll_up = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 4,
        modifiers: KeyModifiers::NONE,
    });
    let scroll_down = Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 4,
        modifiers: KeyModifiers::NONE,
    });

    // In inline mode, mouse capture is not enabled. Mouse events are ignored —
    // the terminal handles native text selection and scrollback scrolling.
    assert_eq!(InputEvent::from_crossterm_event(&scroll_up), None);
    assert_eq!(InputEvent::from_crossterm_event(&scroll_down), None);

    let mut parser = InputParser::new();
    assert!(parser.feed_crossterm_event(&scroll_up).is_empty());
    assert!(parser.feed_crossterm_event(&scroll_down).is_empty());
}

#[test]
fn input_parser_maps_crossterm_paste_to_single_paste_event() {
    let mut parser = InputParser::new();

    assert_eq!(
        parser.feed_crossterm_event(&Event::Paste("alpha\nbeta".to_owned())),
        vec![InputEvent::Paste("alpha\nbeta".to_owned())]
    );
}

#[test]
fn input_parser_buffers_bracketed_paste_newlines_until_end_marker() {
    let mut parser = InputParser::new();
    let events = [
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('\x1b'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('['),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('2'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('0'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('0'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('~'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('b'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('\x1b'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('['),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('2'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('0'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('1'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('~'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
    ];

    let parsed = events
        .iter()
        .flat_map(|event| parser.feed_crossterm_event(event))
        .collect::<Vec<_>>();

    assert_eq!(
        parsed,
        vec![InputEvent::Paste("a\nb".to_owned()), InputEvent::Submit]
    );
}

#[test]
fn input_parser_preserves_shift_characters_inside_bracketed_paste() {
    let mut parser = InputParser::new();
    let events = [
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('\x1b'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('['),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('2'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('0'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('0'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('~'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('A'),
            KeyModifiers::SHIFT,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('Z'),
            KeyModifiers::SHIFT,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('\x1b'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('['),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('2'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('0'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('1'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('~'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
    ];

    let parsed = events
        .iter()
        .flat_map(|event| parser.feed_crossterm_event(event))
        .collect::<Vec<_>>();

    assert_eq!(parsed, vec![InputEvent::Paste("AZ".to_owned())]);
}

#[test]
fn input_parser_preserves_newline_after_non_marker_escape_inside_bracketed_paste() {
    let mut parser = InputParser::new();
    let events = [
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('\x1b'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('['),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('2'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('0'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('0'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('~'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('\x1b'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('\x1b'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('['),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('2'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('0'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('1'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('~'),
            KeyModifiers::NONE,
            KeyEventKind::Press,
        )),
    ];

    let parsed = events
        .iter()
        .flat_map(|event| parser.feed_crossterm_event(event))
        .collect::<Vec<_>>();

    assert_eq!(parsed, vec![InputEvent::Paste("\x1bx\n".to_owned())]);
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
fn transcript_store_keeps_order_and_allows_streaming_update() {
    let mut transcript = TranscriptStore::default();

    transcript.push(TranscriptEntry::user_message("hello"));
    transcript.push(TranscriptEntry::assistant_message("hello"));
    transcript.push_tool_run("tool-1", "shell.run", Some("cargo test".to_owned()));

    assert_eq!(transcript.entries().len(), 3);
    assert_eq!(
        transcript.entries()[0],
        TranscriptEntry::user_message("hello")
    );
    assert_eq!(
        transcript.entries()[1],
        TranscriptEntry::assistant_message("hello")
    );
    assert!(matches!(
        transcript.entries()[2],
        TranscriptEntry::ToolRun { .. }
    ));
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
    prompt.apply_edit(PromptEdit::Insert("draft"));

    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "second prompt");
    assert_eq!(prompt.cursor, 13);

    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "first prompt");
    assert_eq!(prompt.cursor, 12);

    assert!(prompt.recall_next_history());
    assert_eq!(prompt.text, "second prompt");

    assert!(prompt.recall_next_history());
    assert_eq!(prompt.text, "draft");
    assert_eq!(prompt.cursor, 5);

    assert!(prompt.recall_previous_history());
    assert_eq!(prompt.text, "second prompt");
    prompt.apply_edit(PromptEdit::Insert(" edited"));
    assert_eq!(prompt.text, "second prompt edited");
    assert!(!prompt.recall_next_history());
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
fn visible_width_ignores_ansi_csi_and_osc_sequences() {
    assert_eq!(visible_width("\x1b[31mred\x1b[0m plain"), 9);
    assert_eq!(visible_width("\x1b]133;A\x07hello\x1b]133;B\x07"), 5);
    assert_eq!(visible_width("\x1b]133;A\x1b\\hello\x1b]133;B\x1b\\"), 5);
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
fn line_truncate_to_width_preserves_styles_when_not_truncated() {
    use neo_tui::ansi::{Color, Style};
    use neo_tui::core::{Line, Span};

    let style = Style::default().fg(Color::Rgb(198, 120, 221)).bold();
    let line = Line::from_spans(vec![Span::styled("hello ", style), Span::raw("world")]);
    let truncated = line.truncate_to_width(20);
    assert_eq!(truncated.visible_width(), 11);
    assert!(truncated.to_ansi().contains("\x1b[38;2;198;120;221m"));
    assert!(truncated.to_ansi().contains("\x1b[1m"));
}

#[test]
fn line_truncate_to_width_preserves_styles_when_truncated() {
    use neo_tui::ansi::{Color, Style};
    use neo_tui::core::{Line, Span};

    let style = Style::default().fg(Color::Rgb(198, 120, 221));
    let line = Line::from_spans(vec![Span::styled("hello world", style)]);
    let truncated = line.truncate_to_width(8);
    assert_eq!(truncated.visible_width(), 8);
    let ansi = truncated.to_ansi();
    assert!(ansi.contains("\x1b[38;2;198;120;221m"));
    assert!(ansi.contains('…'));
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

    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
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

#[test]
fn transcript_pane_copy_uses_store_selection() {
    let mut runtime = neo_tui::TranscriptPane::new(80, 24);
    runtime.push_user_message("copy selected prompt");
    runtime.push_assistant_message("copy selected answer");
    runtime.select_visible_transcript_entry();
    runtime.extend_transcript_selection_up(1);

    assert_eq!(
        runtime.copy_selected_transcript_text().as_deref(),
        Some("You\ncopy selected prompt\n\nAssistant\ncopy selected answer")
    );
}

#[test]
fn transcript_pane_toggles_tool_detail_expansion() {
    let mut runtime = neo_tui::TranscriptPane::new(80, 24);
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "read".to_owned(),
        result: neo_agent_core::ToolResult::ok("expanded file content"),
    });

    assert!(!runtime.tool_output_expanded());
    assert!(runtime.toggle_tool_output_expanded());
    assert!(runtime.tool_output_expanded());
    assert!(runtime.toggle_tool_output_expanded());
    assert!(!runtime.tool_output_expanded());
}
