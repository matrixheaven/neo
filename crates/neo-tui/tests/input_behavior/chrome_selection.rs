//! Mouse text selection over the chrome widgets: the prompt input box keeps
//! its caret/drag gesture, and every other frame row (todo panel, footer,
//! rich dialogs, full-screen overlays) is selectable through the frame
//! selection — clicks place nothing, drags select and highlight, right-clicks
//! copy the visible rows and collapse the selection, and region switches
//! clear the transcript selection.

use crossterm::event::{KeyModifiers, MouseButton};
use neo_tui::NeoTui;
use neo_tui::dialogs::ApiKeyInputOptions;
use neo_tui::input::InputEvent;
use neo_tui::primitive::text_layout::visible_width;
use neo_tui::primitive::{TuiTheme, bg_to_ansi, strip_ansi};
use neo_tui::shell::{NeoChromeState, OverlayKind};
use neo_tui::tasks_browser::TaskBrowserState;
use neo_tui::transcript::{
    LONG_PRESS_DELAY, MouseEvent, MouseKind, TranscriptPane, slice_text_by_cells,
};
use neo_tui::widgets::todo_panel::{TodoDisplayItem, TodoDisplayStatus};
use std::time::{Duration, Instant};

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
fn locate_in(line: &str, needle: &str) -> usize {
    let stripped = strip_ansi(line);
    let byte_col = stripped
        .find(needle)
        .unwrap_or_else(|| panic!("needle {needle:?} not found in {stripped:?}"));
    stripped[..byte_col].chars().count()
}
fn selection_bg() -> String {
    bg_to_ansi(TuiTheme::default().selection_bg)
}
/// A tui with a two-item todo panel rendered.
fn tui_with_todos() -> NeoTui {
    let mut tui = new_tui();
    tui.chrome_mut().set_todo_items(vec![
        TodoDisplayItem::new("first item", TodoDisplayStatus::Pending),
        TodoDisplayItem::new("second item", TodoDisplayStatus::Done),
    ]);
    tui
}
/// Drag from `(start_row, start_col)` to `(end_row, end_col)` and release.
fn drag_select(
    tui: &mut NeoTui,
    start_row: usize,
    start_col: usize,
    end_row: usize,
    end_col: usize,
) {
    tui.handle_mouse_event(mouse(MouseKind::Press, cell(start_col), cell(start_row)));
    tui.handle_mouse_event(mouse(MouseKind::Drag, cell(end_col), cell(end_row)));
    tui.handle_mouse_event(mouse(MouseKind::Release, cell(end_col), cell(end_row)));
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
    assert!(
        frame.iter().any(|line| line.contains(&selection_bg())),
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
    let mut tui = tui_with_todos();
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (row, col) = locate(&frame, "first item");
    let (row2, col2) = locate(&frame, "second item");
    let end_col = col2 + "second item".len();

    // Drag from the first item title across to the end of the second item
    // title (the column swing crosses the movement threshold).
    drag_select(&mut tui, row, col, row2, end_col);

    // The rendered frame paints the selection background on the selected
    // frame rows (the todo panel), not just the footer hint.
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (painted_row, _) = locate(&frame, "first item");
    assert!(
        frame[painted_row].contains(&selection_bg()),
        "the selected todo rows must be painted: {:?}",
        frame[painted_row]
    );

    // Right-click over the Todo panel copies the materialized frame
    // selection and collapses it.
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(col), cell(row)));
    let copied = tui
        .take_pending_copy()
        .expect("frame selection materialized at right-click");
    assert!(copied.contains("first item"), "{copied}");
    assert!(copied.contains("second item"), "{copied}");
    assert!(
        !tui.has_any_selection(),
        "right-click collapses the selection"
    );
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (cleared_row, _) = locate(&frame, "first item");
    assert!(
        !frame[cleared_row].contains(&selection_bg()),
        "the collapsed selection must not be painted"
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

#[test]
fn frame_selection_covers_normal_and_overlay_frames() {
    // Normal frame: the todo panel rows route through the final-frame path.
    let mut tui = tui_with_todos();
    let frame = tui.render_terminal_frame_at(80, 24, Instant::now()).lines;
    let (row, col) = locate(&frame, "first item");
    let end_col = col + "first item".len();
    drag_select(&mut tui, row, col, row, end_col);
    assert!(tui.has_any_selection(), "todo panel rows are selectable");
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(col), cell(row)));
    let copied = tui.take_pending_copy().expect("normal frame copy");
    assert!(copied.contains("first item"), "{copied}");

    // Full-screen overlay: the task browser owns the whole frame, its rows
    // are all frame rows, and the same final-frame path applies.
    let mut tui = new_tui();
    tui.chrome_mut()
        .push_task_browser_overlay(TaskBrowserState::new());
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (row, col) = locate(&frame, "No tasks.");
    let end_col = col + "No tasks.".len() - 1;
    drag_select(&mut tui, row, col, row, end_col);
    assert!(
        tui.has_any_selection(),
        "full-screen overlay rows are selectable"
    );
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (painted_row, _) = locate(&frame, "No tasks.");
    assert!(
        frame[painted_row].contains(&selection_bg()),
        "the selected overlay row must be painted"
    );
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(col), cell(row)));
    assert_eq!(
        tui.take_pending_copy().as_deref(),
        Some("No tasks."),
        "the overlay copy materializes from the visible rows"
    );

    // Theme manager: the other full-screen overlay surface follows the same
    // final-frame path for its frame rows.
    let mut tui = new_tui();
    tui.chrome_mut().open_theme_manager(Vec::new());
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (row, col) = locate(&frame, "No themes installed.");
    let end_col = col + "No themes installed.".len() - 1;
    drag_select(&mut tui, row, col, row, end_col);
    assert!(tui.has_any_selection(), "theme manager rows are selectable");
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (painted_row, _) = locate(&frame, "No themes installed.");
    assert!(
        frame[painted_row].contains(&selection_bg()),
        "the selected theme manager row must be painted"
    );
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(col), cell(row)));
    assert_eq!(
        tui.take_pending_copy().as_deref(),
        Some("No themes installed."),
        "the theme manager copy materializes from the visible rows"
    );
}

#[test]
fn frame_selection_preserves_unicode_and_ansi_cells() {
    // ASCII, a wide CJK ideograph, a combining mark, an emoji with a
    // variation/modifier sequence, and a ZWJ family sequence.
    let needle = "a你e\u{301}\u{FE0F}👍🏽👨\u{200D}👩\u{200D}👧";
    let needle_cells = visible_width(needle);
    let mut tui = new_tui();
    tui.chrome_mut().set_todo_items(vec![TodoDisplayItem::new(
        needle,
        TodoDisplayStatus::Pending,
    )]);
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (row, col) = locate(&frame, needle);

    drag_select(&mut tui, row, col, row, col + needle_cells - 1);
    assert!(tui.has_any_selection());

    // The highlight paints the exact cells, keeping every grapheme whole.
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (painted_row, _) = locate(&frame, needle);
    assert!(
        frame[painted_row].contains(&selection_bg()),
        "the selected unicode cells must be painted"
    );

    // The copy equals the plain slice of the same display-cell range: wide
    // characters, combining marks and ZWJ sequences stay whole, and ANSI
    // never leaks into the copy.
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(col), cell(row)));
    let copied = tui.take_pending_copy().expect("unicode copy");
    assert_eq!(copied, needle, "wide and combining graphemes stay whole");
    let plain_row = strip_ansi(&frame[painted_row]);
    assert_eq!(
        copied,
        slice_text_by_cells(&plain_row, col, col + needle_cells),
        "the copy range matches the plain slice"
    );
}

#[test]
fn frame_selection_click_drag_and_long_press_share_thresholds() {
    let mut tui = tui_with_todos();
    let start = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, start).lines;
    let (row, col) = locate(&frame, "first item");

    // A plain click leaves no selection.
    tui.handle_mouse_event(mouse(MouseKind::Press, cell(col), cell(row)));
    tui.handle_mouse_event(mouse(MouseKind::Release, cell(col), cell(row)));
    assert!(!tui.has_any_selection(), "a click never selects");

    // Sub-threshold movement still counts as a click.
    tui.handle_mouse_event(mouse(MouseKind::Press, cell(col), cell(row)));
    tui.handle_mouse_event(mouse(MouseKind::Drag, cell(col + 2), cell(row)));
    tui.handle_mouse_event(mouse(MouseKind::Release, cell(col + 2), cell(row)));
    assert!(
        !tui.has_any_selection(),
        "jitter below the threshold is a click"
    );

    // Movement past the shared threshold confirms a drag selection, and a
    // right-click copies it and collapses it.
    drag_select(&mut tui, row, col, row, col + 10);
    assert!(
        tui.has_any_selection(),
        "movement past the threshold selects"
    );
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(col), cell(row)));
    let copied = tui.take_pending_copy().expect("drag selection copy");
    assert!(copied.contains("first item"), "{copied}");
    assert!(
        !tui.has_any_selection(),
        "right-click consumes the selection it copies"
    );

    // A still-held press activates through the existing frame cadence: the
    // next frame past LONG_PRESS_DELAY confirms the selection with no mouse
    // movement event at all.
    tui.handle_mouse_event(mouse(MouseKind::Press, cell(col), cell(row)));
    let frame = tui
        .render_terminal_frame_at(80, 24, start + LONG_PRESS_DELAY + Duration::from_millis(50))
        .lines;
    assert!(
        tui.has_any_selection(),
        "a held press activates via the frame cadence"
    );
    let (painted_row, _) = locate(&frame, "first item");
    assert!(
        frame[painted_row].contains(&selection_bg()),
        "the long-press highlight is painted"
    );
    tui.handle_mouse_event(mouse(MouseKind::Release, cell(col), cell(row)));
    assert!(
        tui.has_any_selection(),
        "the long-press selection survives its release"
    );
}

#[test]
fn frame_selection_invalidates_only_for_selected_visual_state() {
    // A terminal size change clears the selection.
    let mut tui = tui_with_todos();
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (row, col) = locate(&frame, "first item");
    drag_select(&mut tui, row, col, row, col + 10);
    assert!(tui.has_any_selection());
    let frame = tui.render_terminal_frame_at(100, 30, now).lines;
    assert!(!tui.has_any_selection(), "a resize clears the selection");
    assert!(
        frame.iter().all(|line| !line.contains(&selection_bg())),
        "no selection highlight after the resize"
    );

    // Replacing the surface (a blocking overlay opening) clears it.
    let mut tui = tui_with_todos();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (row, col) = locate(&frame, "first item");
    drag_select(&mut tui, row, col, row, col + 10);
    assert!(tui.has_any_selection());
    tui.chrome_mut()
        .push_task_browser_overlay(TaskBrowserState::new());
    let _ = tui.render_terminal_frame_at(80, 24, now);
    assert!(
        !tui.has_any_selection(),
        "an overlay replacement clears the selection"
    );

    // Changed content on a selected row clears it.
    let mut tui = tui_with_todos();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (row, col) = locate(&frame, "first item");
    drag_select(&mut tui, row, col, row, col + 10);
    assert!(tui.has_any_selection());
    tui.chrome_mut().set_todo_items(vec![
        TodoDisplayItem::new("other one", TodoDisplayStatus::Pending),
        TodoDisplayItem::new("other two", TodoDisplayStatus::Done),
    ]);
    let _ = tui.render_terminal_frame_at(80, 24, now);
    assert!(
        !tui.has_any_selection(),
        "selected-row content changes clear the selection"
    );

    // Unselected rows changing (the prompt content) keep it.
    let mut tui = tui_with_todos();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (row, col) = locate(&frame, "first item");
    drag_select(&mut tui, row, col, row, col + 10);
    assert!(tui.has_any_selection());
    tui.chrome_mut().prompt_mut().set_text("type here");
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    assert!(
        tui.has_any_selection(),
        "unselected rows changing must not clear the selection"
    );
    let (painted_row, _) = locate(&frame, "first item");
    assert!(
        frame[painted_row].contains(&selection_bg()),
        "the surviving selection stays painted"
    );
}

#[test]
fn masked_overlay_selection_exposes_only_rendered_mask() {
    let mut tui = new_tui();
    tui.chrome_mut().open_api_key_input(ApiKeyInputOptions {
        title: "API Key".into(),
        provider_name: "OpenAI".into(),
    });
    let overlay = tui
        .chrome_mut()
        .focused_overlay_mut()
        .expect("api key overlay focused");
    let OverlayKind::ApiKeyInput(state) = &mut overlay.kind else {
        panic!("expected the api key input overlay");
    };
    state.handle_input(&InputEvent::Paste("sk-supersecret".to_owned()));

    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (row, _) = locate(&frame, "API Key:");
    let first_bullet = locate_in(&frame[row], "•");
    let bullets = "•".repeat(14);
    let mask_segment: String = strip_ansi(&frame[row])
        .chars()
        .skip(first_bullet)
        .take(14)
        .collect();
    assert_eq!(mask_segment, bullets, "the dialog renders the masked value");

    // Drag across the mask and release; the copy must contain only the
    // rendered mask, never the stored secret.
    drag_select(&mut tui, row, first_bullet, row, first_bullet + 13);
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (painted_row, _) = locate(&frame, "API Key:");
    assert!(
        frame[painted_row].contains(&selection_bg()),
        "the masked field selection is painted"
    );
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(first_bullet), cell(row)));
    let copied = tui.take_pending_copy().expect("masked copy");
    assert_eq!(copied, bullets, "only the screen mask is copied");
    assert!(
        !copied.contains("sk-supersecret"),
        "the raw secret must never be copied"
    );
}
