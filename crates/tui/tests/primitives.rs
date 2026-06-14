use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use neo_tui::{
    ApprovalChoice, ApprovalModal, ApprovalOption, ChatTranscript, InputEvent, InputParser, KeyId,
    KeybindingAction, KeybindingsManager, ListMarker, NeoTuiApp, PromptEdit, PromptState,
    PromptWidget, SelectItem, SelectListState, StatusWidget, ToolPresentationKind, ToolRunMetadata,
    ToolRunTranscript, ToolStatus, ToolStatusKind, TranscriptItem, TranscriptLine,
    TranscriptRenderer, TranscriptSelection, TranscriptView, TranscriptWidget, TuiTheme,
    truncate_width, visible_width, wrap_width,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::{Buffer, Cell},
    style::Color,
};

fn render_widget<W: ratatui::widgets::Widget>(width: u16, height: u16, widget: W) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend is valid");
    terminal
        .draw(|frame| frame.render_widget(widget, frame.area()))
        .expect("widget renders");
    terminal
        .backend()
        .buffer()
        .content
        .chunks(width as usize)
        .map(|line| line.iter().map(Cell::symbol).collect::<String>())
        .collect()
}

fn render_widget_buffer<W: ratatui::widgets::Widget>(width: u16, height: u16, widget: W) -> Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend is valid");
    terminal
        .draw(|frame| frame.render_widget(widget, frame.area()))
        .expect("widget renders");
    terminal.backend().buffer().clone()
}

fn find_cells(line: &[Cell], needle: &str) -> Option<usize> {
    let chars: Vec<char> = needle.chars().collect();
    line.windows(chars.len()).position(|window| {
        window
            .iter()
            .map(|cell| cell.symbol().chars().next())
            .collect::<Option<Vec<_>>>()
            == Some(chars.clone())
    })
}

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
fn input_event_maps_mouse_wheel_events_to_transcript_scroll() {
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
    let moved = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 10,
        row: 4,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(
        InputEvent::from_crossterm_event(&scroll_up),
        Some(InputEvent::ScrollUp(3))
    );
    assert_eq!(
        InputEvent::from_crossterm_event(&scroll_down),
        Some(InputEvent::ScrollDown(3))
    );
    assert_eq!(InputEvent::from_crossterm_event(&moved), None);

    let mut parser = InputParser::new();
    assert_eq!(
        parser.feed_crossterm_event(&scroll_up),
        vec![InputEvent::ScrollUp(3)]
    );
    assert_eq!(
        parser.feed_crossterm_event(&scroll_down),
        vec![InputEvent::ScrollDown(3)]
    );
    assert!(parser.feed_crossterm_event(&moved).is_empty());
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
fn chat_transcript_keeps_order_and_allows_streaming_update() {
    let mut transcript = ChatTranscript::default();

    transcript.push(TranscriptItem::user("hello"));
    transcript.push(TranscriptItem::assistant("hel"));
    transcript.update_last_assistant("hello");
    transcript.push(TranscriptItem::tool(
        "shell.run",
        "cargo test",
        ToolStatusKind::Running,
    ));

    assert_eq!(transcript.items().len(), 3);
    assert_eq!(transcript.items()[0], TranscriptItem::user("hello"));
    assert_eq!(transcript.items()[1], TranscriptItem::assistant("hello"));
}

#[test]
fn transcript_view_tracks_bottom_and_manual_scroll() {
    let transcript = ChatTranscript::from_items(
        (0..8).map(|index| TranscriptItem::notice(format!("line {index}"))),
    );
    let mut view = TranscriptView::new();

    view.sync(transcript.len(), 3);
    let bottom = view.visible_range(&transcript, 3);
    assert_eq!(bottom, 5..8);

    view.scroll_up(2);
    assert_eq!(view.visible_range(&transcript, 3), 3..6);

    view.scroll_down(1);
    assert_eq!(view.visible_range(&transcript, 3), 4..7);

    view.follow_bottom();
    assert_eq!(view.visible_range(&transcript, 3), 5..8);
}

#[test]
fn transcript_view_syncs_visual_row_scrollback_and_follow_tail() {
    let mut view = TranscriptView::new();

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
fn transcript_widget_uses_transcript_view_visible_range() {
    let transcript = ChatTranscript::from_items(
        (0..6).map(|index| TranscriptItem::notice(format!("line {index}"))),
    );
    let mut view = TranscriptView::new();
    view.sync(11, 3);
    view.scroll_up(2);

    let lines = render_widget(24, 3, TranscriptWidget::new(&transcript).with_view(&view));

    assert!(lines.iter().any(|line| line.contains("line 4")));
    assert!(!lines.iter().any(|line| line.contains("line 0")));
    assert!(!lines.iter().any(|line| line.contains("line 5")));
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
    let transcript = ChatTranscript::from_items([
        TranscriptItem::user("first prompt"),
        TranscriptItem::assistant("first answer"),
        TranscriptItem::tool("shell.run", "exit 0", ToolStatusKind::Succeeded),
        TranscriptItem::notice("done"),
    ]);
    let mut selection = TranscriptSelection::new(2);

    selection.extend_up(&transcript, 1);
    selection.extend_down(&transcript, 1);

    assert_eq!(selection.range(&transcript), Some(1..4));
    assert_eq!(
        transcript.copy_selection(&selection).as_deref(),
        Some("Assistant\nfirst answer\n\nTool\n+ shell.run (exit 0)\n\nNotice\ndone")
    );
}

#[test]
fn app_transcript_copy_uses_internal_buffer_and_clears_on_new_prompt() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.transcript_mut()
        .push(TranscriptItem::user("copy selected prompt"));
    app.transcript_mut()
        .push(TranscriptItem::assistant("copy selected answer"));

    app.select_visible_transcript_item();
    app.extend_transcript_selection_up(1);

    assert_eq!(
        app.copy_selected_transcript_text().as_deref(),
        Some("You\ncopy selected prompt\n\nAssistant\ncopy selected answer")
    );
    assert_eq!(
        app.copy_buffer(),
        Some("You\ncopy selected prompt\n\nAssistant\ncopy selected answer")
    );

    app.prompt_mut().apply_edit(PromptEdit::Insert("next turn"));
    let _ = app.submit_prompt();
    assert!(app.transcript_selection().is_none());
}

#[test]
fn app_toggles_selected_transcript_tool_detail() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.transcript_mut().push(TranscriptItem::tool(
        "read",
        "expanded file content",
        ToolStatusKind::Succeeded,
    ));

    app.select_visible_transcript_item();

    assert!(app.toggle_selected_transcript_detail());
    assert!(app.expanded_transcript_items().contains(&0));

    assert!(app.toggle_selected_transcript_detail());
    assert!(!app.expanded_transcript_items().contains(&0));
}

#[test]
fn transcript_widget_renders_roles_tools_and_wraps_content() {
    let transcript = ChatTranscript::from_items([
        TranscriptItem::user("hello world from me"),
        TranscriptItem::assistant("你好世界 and hello"),
        TranscriptItem::tool("shell.run", "cargo test", ToolStatusKind::Succeeded),
    ]);

    let lines = render_widget(48, 14, TranscriptWidget::new(&transcript));

    assert!(lines.iter().any(|line| line.contains("You")));
    assert!(lines.iter().any(|line| line.contains("Assistant")));
    assert!(lines.iter().any(|line| line.contains("shell.run")));
    assert!(lines.iter().any(|line| line.contains("Used shell.run")));
    assert!(lines.iter().any(|line| line.contains("cargo test")));
}

#[test]
fn transcript_widget_collapsed_tool_result_shows_preview_and_expand_hint() {
    let transcript = ChatTranscript::from_items([TranscriptItem::tool(
        "read",
        "first file line\nsecond file line\nthird file line\nfourth file line\nfifth file line\nsixth file line",
        ToolStatusKind::Succeeded,
    )]);

    let lines = render_widget(64, 8, TranscriptWidget::new(&transcript));

    assert!(lines.iter().any(|line| line.contains("✓ Used read")));
    assert!(lines.iter().any(|line| line.contains("· 6 lines")));
    // Read body is hidden when collapsed — no content lines shown.
    assert!(!lines.iter().any(|line| line.contains("first file line")));
    assert!(!lines.iter().any(|line| line.contains("fourth file line")));
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("... (3 more lines, ctrl+o to expand)")),
        "read body should be hidden when collapsed"
    );
    assert!(!lines.iter().any(|line| line.contains("succeeded")));
}

#[test]
fn transcript_widget_expanded_tool_result_shows_full_output() {
    let transcript = ChatTranscript::from_items([TranscriptItem::tool(
        "read",
        "first file line\nsecond file line\nthird file line\nfourth file line\nfifth file line",
        ToolStatusKind::Succeeded,
    )]);
    let expanded = [0usize].into_iter().collect();

    let lines = render_widget(
        64,
        8,
        TranscriptWidget::new(&transcript).with_expanded_items(&expanded),
    );

    assert!(lines.iter().any(|line| line.contains("✓ Used read")));
    assert!(lines.iter().any(|line| line.contains("first file line")));
    assert!(lines.iter().any(|line| line.contains("second file line")));
    assert!(lines.iter().any(|line| line.contains("third file line")));
    assert!(lines.iter().any(|line| line.contains("fourth file line")));
    assert!(lines.iter().any(|line| line.contains("fifth file line")));
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("more lines, ctrl+o to expand"))
    );
}

#[test]
fn transcript_widget_renders_edit_diff_summary_and_preview() {
    let transcript = ChatTranscript::from_items([TranscriptItem::tool(
        "edit",
        "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -10,5 +10,6 @@\n unchanged before\n-old value\n+new value\n+another line\n unchanged after",
        ToolStatusKind::Succeeded,
    )]);

    let buffer = render_widget_buffer(72, 9, TranscriptWidget::new(&transcript));
    let lines = buffer
        .content
        .chunks(72)
        .map(|line| line.iter().map(Cell::symbol).collect::<String>())
        .collect::<Vec<_>>();

    assert!(
        lines
            .iter()
            .any(|line| line.contains("◌ Edited src/lib.rs +2 -1"))
    );
    assert!(lines.iter().any(|line| line.contains("@@ -10,5 +10,6 @@")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("10  unchanged before"))
    );
    assert!(lines.iter().any(|line| line.contains("11 -old value")));
    assert!(lines.iter().any(|line| line.contains("11 +new value")));
    assert!(lines.iter().any(|line| line.contains("12 +another line")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("13  unchanged after"))
    );

    let removed_row = lines
        .iter()
        .position(|line| line.contains("-old value"))
        .expect("removed row");
    let added_row = lines
        .iter()
        .position(|line| line.contains("+new value"))
        .expect("added row");
    let removed = buffer
        .cell((5, u16::try_from(removed_row).expect("row fits")))
        .expect("removed prefix cell");
    let added = buffer
        .cell((5, u16::try_from(added_row).expect("row fits")))
        .expect("added prefix cell");
    let theme = TuiTheme::default();
    assert_eq!(removed.fg, theme.diff_removed);
    assert_eq!(added.fg, theme.diff_added);
}

#[test]
fn transcript_widget_does_not_render_plain_read_output_as_edit_diff() {
    let transcript = ChatTranscript::from_items([TranscriptItem::tool(
        "read",
        "[workspace]\nmembers = [\n    \"crates/ai\",\n    \"crates/tui\",\n]",
        ToolStatusKind::Succeeded,
    )]);

    let lines = render_widget(72, 8, TranscriptWidget::new(&transcript));

    assert!(lines.iter().any(|line| line.contains("✓ Used read")));
    // Read body is hidden when collapsed — no content shown, no diff misfire.
    assert!(!lines.iter().any(|line| line.contains("Edited")));
    assert!(!lines.iter().any(|line| line.contains("+0 -0")));
}

#[test]
fn transcript_widget_renders_compaction_boundary_with_progress() {
    let transcript = ChatTranscript::from_items([TranscriptItem::compaction(9, 12_400)]);

    let lines = render_widget(64, 7, TranscriptWidget::new(&transcript));

    assert!(lines.iter().any(|line| line.contains("Compact")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Compacting conversation..."))
    );
    assert!(lines.iter().any(|line| line.contains("[########")));
    assert!(lines.iter().any(|line| line.contains("100%")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Compacted 9 messages"))
    );
    assert!(lines.iter().any(|line| line.contains("12k tokens before")));
    assert!(lines.iter().any(|line| line.contains("Use /compact")));
}

#[test]
fn transcript_widget_renders_active_compaction_phase_and_percent() {
    let transcript = ChatTranscript::from_items([TranscriptItem::Compaction {
        phase: Some(neo_agent_core::CompactionPhase::Summarizing),
        percent: 70,
        compacted_message_count: 9,
        tokens_before: 12_400,
    }]);

    let lines = render_widget(64, 8, TranscriptWidget::new(&transcript));

    assert!(lines.iter().any(|line| line.contains("70%")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Summarizing older context"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("[#####################"))
    );
}

#[test]
fn transcript_widget_bottom_follows_visual_rows_not_item_count() {
    let transcript = ChatTranscript::from_items([
        TranscriptItem::assistant("line 0\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7"),
        TranscriptItem::notice("bottom message"),
    ]);
    let view = TranscriptView::new();

    let lines = render_widget(40, 5, TranscriptWidget::new(&transcript).with_view(&view));

    assert!(lines.iter().any(|line| line.contains("bottom message")));
    assert!(!lines.iter().any(|line| line.contains("line 0")));
}

#[test]
fn transcript_widget_adds_space_between_adjacent_messages() {
    let transcript = ChatTranscript::from_items([
        TranscriptItem::user("你好"),
        TranscriptItem::assistant("你好，有什么可以帮你？"),
    ]);

    let lines = render_widget(32, 6, TranscriptWidget::new(&transcript));
    let you_row = lines
        .iter()
        .position(|line| line.contains("You"))
        .expect("user label renders");
    let assistant_row = lines
        .iter()
        .position(|line| line.contains("Assistant"))
        .expect("assistant label renders");

    assert!(assistant_row >= you_row + 3);
    assert!(lines[assistant_row - 1].trim().is_empty());
}

#[test]
fn transcript_widget_uses_supplied_theme_colors() {
    let transcript = ChatTranscript::from_items([
        TranscriptItem::user("hello"),
        TranscriptItem::assistant("answer"),
        TranscriptItem::notice("heads up"),
    ]);
    let theme = TuiTheme::default()
        .with_user(Color::Magenta)
        .with_assistant(Color::Blue)
        .with_notice(Color::Yellow);

    let buffer = render_widget_buffer(24, 8, TranscriptWidget::new(&transcript).with_theme(theme));

    let user = buffer.cell((0, 0)).expect("user label cell");
    let assistant = buffer.cell((0, 3)).expect("assistant label cell");
    let notice = buffer.cell((0, 6)).expect("notice label cell");
    assert_eq!(user.fg, Color::Magenta);
    assert_eq!(assistant.fg, Color::Blue);
    assert_eq!(notice.fg, Color::Yellow);
}

#[test]
fn transcript_widget_styles_thinking_separately_from_answer() {
    let transcript = ChatTranscript::from_items([TranscriptItem::assistant_with_thinking(
        "I should inspect the UI hierarchy",
        "Here is the answer",
    )]);

    let buffer = render_widget_buffer(44, 8, TranscriptWidget::new(&transcript));
    let lines = buffer
        .content
        .chunks(44)
        .map(|line| line.iter().map(Cell::symbol).collect::<String>())
        .collect::<Vec<_>>();
    let thinking_row = lines
        .iter()
        .position(|line| line.contains("I should inspect"))
        .expect("thinking renders");
    let answer_row = lines
        .iter()
        .position(|line| line.contains("Here is the answer"))
        .expect("answer renders");

    let theme = TuiTheme::default();
    assert_eq!(
        buffer
            .cell((2, u16::try_from(thinking_row).expect("row fits")))
            .expect("thinking cell")
            .fg,
        theme.thinking
    );
    assert_eq!(
        buffer
            .cell((2, u16::try_from(answer_row).expect("row fits")))
            .expect("answer cell")
            .fg,
        theme.assistant
    );
}

#[test]
fn transcript_widget_highlights_selected_items() {
    let transcript = ChatTranscript::from_items([
        TranscriptItem::user("first"),
        TranscriptItem::assistant("second"),
    ]);
    let selection = TranscriptSelection::new(1);

    let buffer = render_widget_buffer(
        24,
        6,
        TranscriptWidget::new(&transcript).with_selection(Some(&selection)),
    );

    let selected_heading = &buffer.content[24 * 3];
    assert_eq!(selected_heading.symbol(), "A");
    assert_eq!(selected_heading.style().bg, Some(Color::DarkGray));
}

#[test]
fn transcript_widget_renders_unified_diff_lines_with_diff_colors() {
    let transcript = ChatTranscript::from_items([TranscriptItem::assistant(
        "--- src/lib.rs\n+++ src/lib.rs\n@@\n-old\n+new\n unchanged",
    )]);

    let buffer = render_widget_buffer(32, 9, TranscriptWidget::new(&transcript));
    let lines = buffer
        .content
        .chunks(32)
        .map(|line| line.iter().map(Cell::symbol).collect::<String>())
        .collect::<Vec<_>>();

    assert!(lines.iter().any(|line| line.contains("--- src/lib.rs")));
    assert!(lines.iter().any(|line| line.contains("+++ src/lib.rs")));
    assert!(lines.iter().any(|line| line.contains("-old")));
    assert!(lines.iter().any(|line| line.contains("+new")));
    let removed_row = lines
        .iter()
        .position(|line| line.contains("-old"))
        .expect("removed row");
    let added_row = lines
        .iter()
        .position(|line| line.contains("+new"))
        .expect("added row");
    let context_row = lines
        .iter()
        .position(|line| line.contains(" unchanged"))
        .expect("context row");
    let removed_row = u16::try_from(removed_row).expect("removed row fits terminal height");
    let added_row = u16::try_from(added_row).expect("added row fits terminal height");
    let context_row = u16::try_from(context_row).expect("context row fits terminal height");
    let removed = buffer.cell((2, removed_row)).expect("removed prefix cell");
    let added = buffer.cell((2, added_row)).expect("added prefix cell");
    let context = buffer.cell((2, context_row)).expect("context prefix cell");
    let theme = TuiTheme::default();
    assert_eq!(removed.fg, theme.diff_removed);
    assert_eq!(added.fg, theme.diff_added);
    assert_eq!(context.fg, theme.diff_context);
}

#[test]
fn transcript_renderer_renders_markdown_tables_tasks_and_inline_marks_without_raw_markers() {
    let renderer = TranscriptRenderer::new(64);
    let lines = renderer.render_markdownish(
        "## Ship list\n\
         - [x] parse **bold** and *italic* with `code`\n\
         - [ ] keep item\n\n\
         | Area | Status |\n\
         | --- | --- |\n\
         | TUI | Ready |\n\n\
         > quote with **weight**",
    );

    assert!(matches!(
        &lines[0],
        TranscriptLine::Heading { level: 2, text } if text == "Ship list"
    ));
    assert!(
        lines.iter().any(|line| {
            matches!(
                line,
                TranscriptLine::ListItem {
                    indent: 0,
                    marker: ListMarker::TaskDone,
                    text,
                }
                    if text == "parse bold and italic with `code`"
            )
        }),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|line| {
            matches!(
                line,
                TranscriptLine::ListItem {
                    indent: 0,
                    marker: ListMarker::TaskOpen,
                    text,
                }
                    if text == "keep item"
            )
        }),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|line| {
            matches!(
                line,
                TranscriptLine::Text { text }
                    if text == "Area | Status"
            )
        }),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|line| {
            matches!(
                line,
                TranscriptLine::Text { text }
                    if text == "TUI  | Ready"
            )
        }),
        "{lines:#?}"
    );
    assert!(
        lines.iter().all(|line| !line.display_text().contains("**")
            && !line.display_text().contains("[x]")
            && !line.display_text().contains("[ ]")
            && !line.display_text().contains("| --- |")),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|line| {
            matches!(
                line,
                TranscriptLine::Quote { text }
                    if text == "quote with weight"
            )
        }),
        "{lines:#?}"
    );
}

#[test]
fn transcript_widget_renders_markdown_without_raw_heading_markers() {
    let transcript = ChatTranscript::from_items([TranscriptItem::assistant(
        "## Neo Project Analysis\n\n### Core Features\n\n- Read files\n> Conclusion",
    )]);

    let lines = render_widget(64, 10, TranscriptWidget::new(&transcript));

    assert!(
        lines
            .iter()
            .any(|line| line.contains("Neo Project Analysis")),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Core Features")),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Read files")),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("Conclusion")),
        "{lines:#?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("## Neo")),
        "{lines:#?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("### Core")),
        "{lines:#?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("> Conclusion")),
        "{lines:#?}"
    );
}

#[test]
fn transcript_renderer_classifies_fenced_diff_blocks_as_diff_lines() {
    let renderer = TranscriptRenderer::new(24);
    let lines = renderer.render_markdownish(
        "```diff\n\
         --- old.txt\n\
         +++ new.txt\n\
         @@\n\
         -before\n\
         +after\n\
         ```",
    );

    assert!(matches!(
        &lines[0],
        TranscriptLine::DiffFileHeader { marker: '-', path } if path == "old.txt"
    ));
    assert!(matches!(
        &lines[1],
        TranscriptLine::DiffFileHeader { marker: '+', path } if path == "new.txt"
    ));
    assert!(matches!(&lines[2], TranscriptLine::DiffHunk { text } if text == "@@"));
    assert!(matches!(
        &lines[3],
        TranscriptLine::DiffRemoved { text } if text == "before"
    ));
    assert!(matches!(
        &lines[4],
        TranscriptLine::DiffAdded { text } if text == "after"
    ));
}

#[test]
fn status_widget_renders_tool_state_without_runtime_details() {
    let statuses = vec![
        ToolStatus::new("read", ToolStatusKind::Running).with_detail("src/lib.rs"),
        ToolStatus::new("test", ToolStatusKind::Failed).with_detail("exit 101"),
    ];

    let lines = render_widget(30, 4, StatusWidget::new(&statuses));

    assert!(lines.iter().any(|line| line.contains("read")));
    assert!(lines.iter().any(|line| line.contains("running")));
    assert!(lines.iter().any(|line| line.contains("test")));
    assert!(lines.iter().any(|line| line.contains("failed")));
}

#[test]
fn transcript_widget_renders_running_tool_marker_in_place() {
    let transcript = ChatTranscript::from_items([TranscriptItem::tool_run(
        "list",
        Some(r#"{"path":"crates/tui/src"}"#.to_owned()),
        None,
        ToolStatusKind::Running,
        neo_tui::ToolRunMetadata::default(),
        neo_tui::ToolPresentationKind::Text,
    )]);

    let lines = render_widget(80, 4, TranscriptWidget::new(&transcript));

    assert!(lines.iter().any(|line| line.contains("● Using list")));
    assert!(lines.iter().any(|line| line.contains("(crates/tui/src)")));
    assert!(!lines.iter().any(|line| line.contains("running")));
}

#[test]
fn transcript_widget_shows_result_chip_when_tool_succeeds() {
    let result = "line1\nline2\nline3\nline4".to_owned();
    let transcript = ChatTranscript::from_items([TranscriptItem::tool_run(
        "Read",
        Some(r#"{"path":"src/lib.rs"}"#.to_owned()),
        Some(result),
        ToolStatusKind::Succeeded,
        neo_tui::ToolRunMetadata::default(),
        neo_tui::ToolPresentationKind::Text,
    )]);

    let lines = render_widget(80, 8, TranscriptWidget::new(&transcript));

    assert!(lines.iter().any(|line| line.contains("✓ Used Read")));
    assert!(lines.iter().any(|line| line.contains("· 4 lines")));
    // Read body is hidden when collapsed (chip conveys line count).
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("... (1 more lines, ctrl+o to expand)")),
        "read body should be hidden when collapsed"
    );
}

#[test]
fn transcript_widget_styles_tool_header_with_dim_key_arg_and_chip() {
    let result = "line1\nline2\nline3\nline4".to_owned();
    let transcript = ChatTranscript::from_items([TranscriptItem::tool_run(
        "Read",
        Some(r#"{"path":"src/lib.rs"}"#.to_owned()),
        Some(result),
        ToolStatusKind::Succeeded,
        neo_tui::ToolRunMetadata::default(),
        neo_tui::ToolPresentationKind::Text,
    )]);

    let buffer = render_widget_buffer(80, 8, TranscriptWidget::new(&transcript));
    let line = buffer.content.chunks(80).next().expect("one line");

    let name_needle = "Read";
    let key_needle = "(src/lib.rs)";
    let chip_needle = " · 4 lines";
    let name_start = find_cells(line, name_needle).expect("tool name in header");
    let key_start = find_cells(line, key_needle).expect("key arg in header");
    let chip_start = find_cells(line, chip_needle).expect("chip in header");

    let muted = Color::Rgb(139, 148, 158);
    let succeeded = Color::Rgb(65, 184, 131);

    for (i, cell) in line
        .iter()
        .enumerate()
        .skip(name_start)
        .take(name_needle.chars().count())
    {
        assert_eq!(cell.fg, succeeded, "tool name cell {i} should be succeeded");
    }
    for (i, cell) in line
        .iter()
        .enumerate()
        .skip(key_start)
        .take(key_needle.chars().count())
    {
        assert_eq!(cell.fg, muted, "key arg cell {i} should be muted");
    }
    for (i, cell) in line
        .iter()
        .enumerate()
        .skip(chip_start)
        .take(chip_needle.chars().count())
    {
        assert_eq!(cell.fg, muted, "chip cell {i} should be muted");
    }
}

#[test]
fn transcript_widget_tints_failed_tool_output_red() {
    let result = "error: something went wrong".to_owned();
    let transcript = ChatTranscript::from_items([TranscriptItem::tool_run(
        "Bash",
        Some(r#"{"command":"cargo test"}"#.to_owned()),
        Some(result),
        ToolStatusKind::Failed,
        neo_tui::ToolRunMetadata {
            exit_code: Some(101),
            ..Default::default()
        },
        neo_tui::ToolPresentationKind::Text,
    )]);

    let buffer = render_widget_buffer(80, 4, TranscriptWidget::new(&transcript));
    let has_red = buffer
        .content
        .chunks(80)
        .any(|line| line.iter().any(|cell| cell.fg == Color::Rgb(248, 81, 73)));
    assert!(has_red);
}

#[test]
fn transcript_widget_collapses_tool_result_to_three_lines() {
    let result = "one\ntwo\nthree\nfour\nfive".to_owned();
    let transcript = ChatTranscript::from_items([TranscriptItem::tool_run(
        "Read",
        Some(r#"{"path":"src/lib.rs"}"#.to_owned()),
        Some(result),
        ToolStatusKind::Succeeded,
        neo_tui::ToolRunMetadata::default(),
        neo_tui::ToolPresentationKind::Text,
    )]);

    let lines = render_widget(64, 7, TranscriptWidget::new(&transcript));

    assert!(lines.iter().any(|line| line.contains("✓ Used Read")));
    assert!(lines.iter().any(|line| line.contains("· 5 lines")));
    // Read body is hidden when collapsed — no content lines, no expand hint.
    assert!(!lines.iter().any(|line| line.contains("one")));
    assert!(!lines.iter().any(|line| line.contains("four")));
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("... (2 more lines, ctrl+o to expand)")),
        "read body should be hidden when collapsed"
    );
}

#[test]
fn prompt_widget_renders_prompt_text_and_cursor_marker() {
    let prompt = PromptState::new("hello").with_cursor(2);

    let lines = render_widget(20, 3, PromptWidget::new(&prompt));

    assert!(lines[0].contains('┌'));
    assert!(lines[1].contains("> he"));
    assert!(lines[1].contains("llo"));
    assert!(lines[1].contains("▏"));
    assert!(lines[2].contains('└'));
}

#[test]
fn prompt_widget_uses_composer_background_across_input_area() {
    let prompt = PromptState::new("ship it").with_cursor(7);

    let buffer = render_widget_buffer(28, 3, PromptWidget::new(&prompt));

    assert!(buffer.content.chunks(28).any(|line| {
        line.iter()
            .map(Cell::symbol)
            .collect::<String>()
            .contains("> ship it")
    }));
    for y in 0..3 {
        assert_eq!(
            buffer.cell((0, y)).expect("left composer cell").bg,
            Color::Rgb(31, 35, 43)
        );
        assert_eq!(
            buffer.cell((27, y)).expect("right composer cell").bg,
            Color::Rgb(31, 35, 43)
        );
    }
}

#[test]
fn approval_modal_renders_request_and_selected_option() {
    let modal = ApprovalModal::new(
        "Run command?",
        "cargo clippy -p neo-tui --all-targets",
        [
            ApprovalOption::new(ApprovalChoice::Approve, "Approve once"),
            ApprovalOption::new(ApprovalChoice::Deny, "Deny"),
        ],
    )
    .with_selected(1);

    let lines = render_widget(42, 8, modal);

    assert!(lines.iter().any(|line| line.contains("Run command?")));
    assert!(lines.iter().any(|line| line.contains("cargo clippy")));
    assert!(lines.iter().any(|line| line.contains("> Deny")));
}

#[test]
fn approval_modal_renders_as_action_panel_with_structured_choice_copy() {
    let modal = ApprovalModal::new(
        "Tool use",
        "shell.run\n$ cargo test -p neo-tui",
        [
            ApprovalOption::new(ApprovalChoice::Approve, "Approve once"),
            ApprovalOption::new(ApprovalChoice::Deny, "Deny"),
            ApprovalOption::new(ApprovalChoice::AlwaysApprove, "Always approve"),
        ],
    );

    let buffer = render_widget_buffer(58, 9, modal);
    let lines = buffer
        .content
        .chunks(58)
        .map(|line| line.iter().map(Cell::symbol).collect::<String>())
        .collect::<Vec<_>>();

    assert!(lines.iter().any(|line| line.contains("Action required")));
    assert!(lines.iter().any(|line| line.contains("Tool use")));
    assert!(lines.iter().any(|line| line.contains("shell.run")));
    assert!(lines.iter().any(|line| line.contains("1. Approve once")));
    assert!(lines.iter().any(|line| line.contains("2. Deny")));
    assert!(lines.iter().any(|line| line.contains("3. Always approve")));
}

#[test]
fn transcript_widget_streams_live_bash_output_into_running_card() {
    let running = TranscriptItem::Tool {
        name: "bash".to_owned(),
        detail: String::new(),
        status: ToolStatusKind::Running,
        tool_run: ToolRunTranscript {
            name: "bash".to_owned(),
            arguments: Some(r#"{"command":"echo live"}"#.to_owned()),
            result: None,
            live_output: vec![
                "line one".to_owned(),
                "line two".to_owned(),
                "line three".to_owned(),
            ],
            status: ToolStatusKind::Running,
            metadata: ToolRunMetadata::default(),
            presentation: ToolPresentationKind::Text,
        },
    };

    let transcript = ChatTranscript::from_items([running]);
    let lines = render_widget(80, 6, TranscriptWidget::new(&transcript));

    assert!(lines.iter().any(|line| line.contains("● Using bash")));
    assert!(lines.iter().any(|line| line.contains("line one")));
    assert!(lines.iter().any(|line| line.contains("line two")));
    assert!(lines.iter().any(|line| line.contains("line three")));

    let finished = TranscriptItem::Tool {
        name: "bash".to_owned(),
        detail: String::new(),
        status: ToolStatusKind::Succeeded,
        tool_run: ToolRunTranscript {
            name: "bash".to_owned(),
            arguments: Some(r#"{"command":"echo live"}"#.to_owned()),
            result: Some("final result".to_owned()),
            live_output: Vec::new(),
            status: ToolStatusKind::Succeeded,
            metadata: ToolRunMetadata::default(),
            presentation: ToolPresentationKind::Text,
        },
    };

    let transcript = ChatTranscript::from_items([finished]);
    let lines = render_widget(80, 4, TranscriptWidget::new(&transcript));

    assert!(lines.iter().any(|line| line.contains("✓ Used bash")));
    assert!(lines.iter().any(|line| line.contains("final result")));
    assert!(!lines.iter().any(|line| line.contains("line one")));
    assert!(!lines.iter().any(|line| line.contains("line two")));
    assert!(!lines.iter().any(|line| line.contains("line three")));
}
