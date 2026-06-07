use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use neo_tui::{
    ApprovalChoice, ApprovalModal, ApprovalOption, ChatTranscript, InputEvent, KeyId,
    KeybindingAction, KeybindingsManager, PromptEdit, PromptState, PromptWidget, SelectItem,
    SelectListState, StatusWidget, ToolStatus, ToolStatusKind, TranscriptItem, TranscriptView,
    TranscriptWidget, truncate_width, wrap_width,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Cell};

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

    let bottom = view.visible_range(&transcript, 3);
    assert_eq!(bottom, 5..8);

    view.scroll_up(2, &transcript, 3);
    assert_eq!(view.visible_range(&transcript, 3), 3..6);

    view.scroll_down(1, &transcript, 3);
    assert_eq!(view.visible_range(&transcript, 3), 4..7);

    view.follow_bottom();
    assert_eq!(view.visible_range(&transcript, 3), 5..8);
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
fn transcript_widget_renders_roles_tools_and_wraps_content() {
    let transcript = ChatTranscript::from_items([
        TranscriptItem::user("hello world from me"),
        TranscriptItem::assistant("你好世界 and hello"),
        TranscriptItem::tool("shell.run", "cargo test", ToolStatusKind::Succeeded),
    ]);

    let lines = render_widget(18, 9, TranscriptWidget::new(&transcript));

    assert!(lines.iter().any(|line| line.contains("You")));
    assert!(lines.iter().any(|line| line.contains("Assistant")));
    assert!(lines.iter().any(|line| line.contains("shell.run")));
    assert!(lines.iter().any(|line| line.contains("test")));
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
fn prompt_widget_renders_prompt_text_and_cursor_marker() {
    let prompt = PromptState::new("hello").with_cursor(2);

    let lines = render_widget(20, 3, PromptWidget::new(&prompt));

    assert!(lines[0].contains("> he"));
    assert!(lines[0].contains("llo"));
    assert!(lines[0].contains("▏"));
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
