//! Mouse text selection over the chrome widgets (prompt input box and Todo
//! panel): clicks place the caret without selecting, drags select and
//! highlight, right-clicks request a copy, and region switches clear the
//! transcript selection.

use crossterm::event::{KeyModifiers, MouseButton};
use neo_tui::NeoTui;
use neo_tui::primitive::{TuiTheme, strip_ansi};
use neo_tui::shell::NeoChromeState;
use neo_tui::transcript::{MouseEvent, MouseKind, TranscriptPane};
use neo_tui::widgets::todo_panel::{TodoDisplayItem, TodoDisplayStatus};
use std::time::Instant;

fn new_tui() -> NeoTui {
    NeoTui::new(
        NeoChromeState::new("neo", "s1", "m1", "/tmp/ws"),
        TranscriptPane::new(80, 24),
    )
}
fn mouse(kind: MouseKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        button: MouseButton::Left,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}
fn right_mouse(kind: MouseKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        button: MouseButton::Right,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}
fn cell(value: usize) -> u16 {
    u16::try_from(value).expect("fits u16")
}
fn locate(frame: &[String], needle: &str) -> (usize, usize) {
    for (row, line) in frame.iter().enumerate() {
        let stripped = strip_ansi(line);
        if let Some(byte_col) = stripped.find(needle) {
            return (row, stripped[..byte_col].chars().count());
        }
    }
    panic!("needle {needle:?} not found in frame: {frame:?}");
}

#[test]
fn prompt_click_places_caret_and_drag_selects_and_highlights() {
    let mut tui = new_tui();
    tui.chrome_mut().prompt_mut().set_text("hello world");
    let frame = tui.render_terminal_frame_at(80, 24, Instant::now()).lines;
    let (row, hello_col) = locate(&frame, "hello");

    // A plain click on the first character places the caret without
    // selecting.
    tui.handle_mouse_event(mouse(MouseKind::Press, cell(hello_col), cell(row)));
    tui.handle_mouse_event(mouse(MouseKind::Release, cell(hello_col), cell(row)));
    assert_eq!(tui.chrome().prompt().cursor, 0, "click places the caret");
    assert_eq!(tui.chrome().prompt().selection_range(), None);

    // A drag across "hello" selects exactly those characters.
    let end_col = hello_col + "hello".len();
    tui.handle_mouse_event(mouse(MouseKind::Press, cell(hello_col), cell(row)));
    tui.handle_mouse_event(mouse(MouseKind::Drag, cell(end_col), cell(row)));
    tui.handle_mouse_event(mouse(MouseKind::Release, cell(end_col), cell(row)));
    assert_eq!(
        tui.chrome().prompt().selection_text().as_deref(),
        Some("hello"),
        "the drag selects the dragged characters"
    );

    // The rendered frame paints the selection background on the content.
    let frame = tui.render_terminal_frame_at(80, 24, Instant::now()).lines;
    let selection_bg = neo_tui::primitive::bg_to_ansi(TuiTheme::default().selection_bg);
    assert!(
        frame.iter().any(|line| line.contains(&selection_bg)),
        "the selection must be painted on the prompt content"
    );
}

#[test]
fn prompt_right_click_copies_selection_or_whole_text() {
    let mut tui = new_tui();
    tui.chrome_mut().prompt_mut().set_text("hello world");
    let frame = tui.render_terminal_frame_at(80, 24, Instant::now()).lines;
    let (row, hello_col) = locate(&frame, "hello");

    // Right-click with a selection copies the selected range.
    tui.handle_mouse_event(mouse(MouseKind::Press, cell(hello_col), cell(row)));
    tui.handle_mouse_event(mouse(MouseKind::Drag, cell(hello_col + 5), cell(row)));
    tui.handle_mouse_event(mouse(MouseKind::Release, cell(hello_col + 5), cell(row)));
    tui.handle_mouse_event(right_mouse(MouseKind::Press, 10, cell(row)));
    assert_eq!(
        tui.take_pending_copy().as_deref(),
        Some("hello"),
        "right-click copies the prompt selection"
    );
    assert_eq!(
        tui.chrome().prompt().selection_range(),
        None,
        "right-click copy collapses the selection like the transcript"
    );

    // Right-click without a selection copies the whole prompt text.
    tui.handle_mouse_event(mouse(MouseKind::Press, 12, cell(row)));
    tui.handle_mouse_event(mouse(MouseKind::Release, 12, cell(row)));
    assert_eq!(tui.chrome().prompt().selection_range(), None);
    tui.handle_mouse_event(right_mouse(MouseKind::Press, 10, cell(row)));
    assert_eq!(
        tui.take_pending_copy().as_deref(),
        Some("hello world"),
        "right-click without a selection copies the whole prompt"
    );
}

#[test]
fn todo_drag_selects_and_right_click_copies() {
    let mut tui = new_tui();
    tui.chrome_mut().set_todo_items(vec![
        TodoDisplayItem::new("first item", TodoDisplayStatus::Pending),
        TodoDisplayItem::new("second item", TodoDisplayStatus::Done),
    ]);
    let frame = tui.render_terminal_frame_at(80, 24, Instant::now()).lines;
    let (row, col) = locate(&frame, "first item");

    // Drag from the first item title across to the end of the second item
    // title (the column swing crosses the movement threshold).
    let (row2, col2) = locate(&frame, "second item");
    let end_col = col2 + "second item".len();
    tui.handle_mouse_event(mouse(MouseKind::Press, cell(col), cell(row)));
    tui.handle_mouse_event(mouse(MouseKind::Drag, cell(end_col), cell(row2)));
    tui.handle_mouse_event(mouse(MouseKind::Release, cell(end_col), cell(row2)));
    let selected = tui
        .chrome()
        .copy_todo_selection_text()
        .expect("todo selection materialized");
    assert!(selected.contains("first item"), "{selected}");
    assert!(selected.contains("second item"), "{selected}");

    // Right-click over the Todo panel copies the materialized selection and
    // collapses it.
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(col), cell(row)));
    assert_eq!(
        tui.take_pending_copy().as_deref(),
        Some(selected.as_str()),
        "right-click over the Todo panel copies its selection"
    );
    assert_eq!(
        tui.chrome().todo_selection(),
        None,
        "right-click copy collapses the todo selection"
    );
}

#[test]
fn region_switch_clears_transcript_selection() {
    let mut tui = new_tui();
    tui.chrome_mut().prompt_mut().set_text("type here");
    tui.transcript_mut().push_status("alpha");
    tui.transcript_mut().push_status("omega");
    let frame = tui.render_terminal_frame_at(80, 24, Instant::now()).lines;
    let (prompt_row, _) = locate(&frame, "type here");

    // A body drag creates a transcript selection.
    tui.handle_mouse_event(mouse(MouseKind::Press, 1, 0));
    tui.handle_mouse_event(mouse(MouseKind::Drag, 10, 0));
    tui.handle_mouse_event(mouse(MouseKind::Release, 10, 0));
    assert!(tui.transcript().has_transcript_selection());

    // Pressing in the prompt clears it.
    tui.handle_mouse_event(mouse(MouseKind::Press, 3, cell(prompt_row)));
    assert!(
        !tui.transcript().has_transcript_selection(),
        "a prompt press clears the transcript selection"
    );
}
