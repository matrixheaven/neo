use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use neo_tui::{
    ApprovalChoice, ApprovalModal, ApprovalOption, ChatTranscript, InputEvent, PromptState,
    PromptWidget, StatusWidget, ToolStatus, ToolStatusKind, TranscriptItem, TranscriptWidget,
    wrap_width,
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
