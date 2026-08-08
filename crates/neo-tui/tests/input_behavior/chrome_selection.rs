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
use neo_tui::primitive::{Color, TuiTheme, bg_to_ansi, strip_ansi};
use neo_tui::shell::{NeoChromeState, OverlayKind};
use neo_tui::tasks_browser::{
    TaskBrowserItem, TaskBrowserKind, TaskBrowserSnapshot, TaskBrowserState, TaskBrowserStatus,
};
use neo_tui::transcript::{
    LONG_PRESS_DELAY, MouseEvent, MouseKind, TranscriptPane, frame_content_width,
    prompt_body_width, slice_text_by_cells,
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

#[test]
fn prompt_gesture_releases_outside_prompt_without_switching_owner() {
    let mut tui = new_tui();
    tui.chrome_mut().prompt_mut().set_text("hello world");
    tui.transcript_mut().push_status("alpha");
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (prompt_row, hello_col) = locate(&frame, "hello");
    assert!(prompt_row > 0, "the status body sits above the prompt box");
    let footer_row = frame.len() - 1;
    assert!(footer_row > prompt_row, "the footer sits below the prompt");

    // Press inside the prompt and drag down past the box into the footer:
    // the endpoint clamps to the last visible character, the owner never
    // switches to the frame surface, and the release closes the gesture.
    tui.handle_mouse_event(mouse(MouseKind::Press, cell(hello_col), cell(prompt_row)));
    tui.handle_mouse_event(mouse(
        MouseKind::Drag,
        cell(hello_col + 5),
        cell(footer_row),
    ));
    tui.handle_mouse_event(mouse(
        MouseKind::Release,
        cell(hello_col + 5),
        cell(footer_row),
    ));
    assert_eq!(
        tui.chrome().prompt().selection_text().as_deref(),
        Some("hello world"),
        "the endpoint clamps to the last visible character below the prompt"
    );
    assert!(
        !tui.transcript().has_transcript_selection(),
        "the crossing drag must not activate the transcript owner"
    );
    assert!(
        tui.frame_selection_text().is_none(),
        "the crossing drag must not activate the frame owner"
    );

    // The gesture is closed: hover motion after the release never extends.
    tui.handle_mouse_event(mouse(
        MouseKind::Drag,
        cell(hello_col + 8),
        cell(footer_row),
    ));
    assert_eq!(
        tui.chrome().prompt().selection_text().as_deref(),
        Some("hello world"),
        "motion after release must not extend the prompt selection"
    );

    // Press at the end of the text and drag up into the transcript body:
    // the endpoint clamps to the first visible character.
    tui.handle_mouse_event(mouse(
        MouseKind::Press,
        cell(hello_col + 11),
        cell(prompt_row),
    ));
    tui.handle_mouse_event(mouse(MouseKind::Drag, cell(3), cell(0)));
    tui.handle_mouse_event(mouse(MouseKind::Release, cell(3), cell(0)));
    assert_eq!(
        tui.chrome().prompt().selection_text().as_deref(),
        Some("hello world"),
        "the endpoint clamps to the first visible character above the prompt"
    );
}

#[test]
fn prompt_drag_below_scrolled_box_clamps_to_last_visible_row() {
    // A scrolled multi-line prompt: dragging below the box must clamp the
    // endpoint to the last visible character, never into the text hidden
    // behind the scroll window.
    let mut tui = new_tui();
    let text = (0..30)
        .map(|index| format!("row-{index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let body_width = prompt_body_width(frame_content_width(80));
    tui.chrome_mut().prompt_mut().set_text(&text);
    // Move the caret to line 15: the scroll window shows lines 8..15.
    tui.chrome_mut()
        .prompt_mut()
        .move_cursor_to(15 * 7, body_width);
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (prompt_row, row_col) = locate(&frame, "row-08");
    let footer_row = frame.len() - 1;
    assert!(footer_row > prompt_row, "the footer sits below the prompt");

    // Press on the first visible line and drag down past the box: the
    // selection covers exactly the visible window.
    tui.handle_mouse_event(mouse(MouseKind::Press, cell(row_col), cell(prompt_row)));
    tui.handle_mouse_event(mouse(MouseKind::Drag, cell(10), cell(footer_row)));
    tui.handle_mouse_event(mouse(MouseKind::Release, cell(10), cell(footer_row)));
    assert_eq!(
        tui.chrome().prompt().selection_text().as_deref(),
        Some("row-08\nrow-09\nrow-10\nrow-11\nrow-12\nrow-13\nrow-14\nrow-15"),
        "the endpoint clamps to the last visible row, not the text end"
    );
}

#[test]
fn selection_before_first_frame_is_ignored() {
    let mut tui = new_tui();
    tui.chrome_mut().prompt_mut().set_text("type here");
    tui.transcript_mut().push_status("alpha");
    let cursor_before = tui.chrome().prompt().cursor;
    // No frame was ever rendered, so there is no layout to route against:
    // selection events must be ignored rather than guessed at a region, and
    // no owner may open or select anything.
    tui.handle_mouse_event(mouse(MouseKind::Press, 1, 0));
    tui.handle_mouse_event(mouse(MouseKind::Drag, 10, 4));
    tui.handle_mouse_event(mouse(MouseKind::Release, 10, 4));
    tui.handle_mouse_event(mouse(MouseKind::Press, 3, cell(5)));
    tui.handle_mouse_event(mouse(MouseKind::Release, 3, cell(5)));
    tui.handle_mouse_event(right_mouse(MouseKind::Press, 10, cell(5)));
    assert!(
        !tui.has_any_selection(),
        "pre-frame selection events must not select anything"
    );
    assert!(
        !tui.transcript().has_transcript_selection(),
        "pre-frame events must not open a transcript selection"
    );
    assert_eq!(tui.frame_selection_text(), None);
    assert_eq!(
        tui.take_pending_copy(),
        None,
        "pre-frame right-click must not copy any region"
    );
    assert_eq!(
        tui.chrome().prompt().cursor,
        cursor_before,
        "no caret move without a rendered frame"
    );
    assert_eq!(tui.chrome().prompt().selection_range(), None);
}

#[test]
fn frame_selection_excludes_borders_and_decoration_rows() {
    // (a) Todo panel: a block drag from the horizontal border row down to
    // the last todo row copies the items without the border, and the pure
    // border row leaves no blank line behind.
    let mut tui = tui_with_todos();
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (border_row, _) = locate(&frame, "\u{2500}");
    let (row2, col2) = locate(&frame, "second item");
    let end_col = col2 + "second item".len();
    drag_select(&mut tui, border_row, 1, row2, end_col);
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(col2), cell(row2)));
    let copied = tui.take_pending_copy().expect("todo block copy");
    assert!(copied.contains("first item"), "{copied}");
    assert!(
        copied
            .chars()
            .all(|ch| !('\u{2500}'..='\u{257F}').contains(&ch)),
        "box-drawing border characters must never be copied: {copied:?}"
    );
    assert!(
        !copied.lines().any(|line| line.trim().is_empty()),
        "a pure border row must not leave a blank line: {copied:?}"
    );

    // (b) Task browser: dragging across a pane content row from the leading
    // `│` to the trailing `│` highlights only the content cells and copies
    // without the column separators.
    let mut tui = new_tui();
    tui.chrome_mut()
        .push_task_browser_overlay(TaskBrowserState::new());
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (row, _) = locate(&frame, "No tasks.");
    let plain = strip_ansi(&frame[row]);
    let lead_col = locate_in(&frame[row], "\u{2502}");
    let trail_byte = plain.rfind('\u{2502}').expect("trailing pane border");
    let trail_col = plain[..trail_byte].chars().count();
    drag_select(&mut tui, row, lead_col, row, trail_col);
    assert!(tui.has_any_selection(), "pane content rows are selectable");

    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let painted = &frame[row];
    let first_bg = painted
        .find(&selection_bg())
        .expect("the content cells are highlighted");
    assert_eq!(
        strip_ansi(&painted[..first_bg]),
        " \u{2502} ",
        "the leading border cell must not be highlighted: {painted:?}"
    );
    let last_bg = painted
        .rfind(&selection_bg())
        .expect("the content cells are highlighted");
    let after = strip_ansi(&painted[last_bg + selection_bg().len()..]);
    assert!(after.contains("No tasks."), "{after:?}");
    assert!(
        after.ends_with(" \u{2502}"),
        "the trailing border cell must not be highlighted: {after:?}"
    );

    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(lead_col), cell(row)));
    let copied = tui.take_pending_copy().expect("pane row copy");
    assert!(copied.contains("No tasks."), "{copied}");
    assert!(
        !copied.contains('\u{2502}'),
        "column separators must never be copied: {copied:?}"
    );
}

#[test]
fn frame_selection_skips_prompt_rows_when_crossing() {
    let mut tui = tui_with_todos();
    tui.chrome_mut().prompt_mut().set_text("input-box-text");
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (todo_row, todo_col) = locate(&frame, "first item");
    let (prompt_row, prompt_col) = locate(&frame, "input-box-text");

    // Drag from the todo panel down into the prompt box: the crossing drag
    // stays with the frame owner, but prompt rows are excluded from both
    // the highlight and the materialized copy. The endpoint lands well
    // inside the prompt text so the rectangular selection still covers the
    // whole first todo row.
    drag_select(&mut tui, todo_row, todo_col, prompt_row, prompt_col + 12);
    assert!(tui.has_any_selection(), "the crossing drag selects");

    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (painted_todo, _) = locate(&frame, "first item");
    assert!(
        frame[painted_todo].contains(&selection_bg()),
        "todo rows stay highlighted"
    );
    let (prompt_row, _) = locate(&frame, "input-box-text");
    assert!(
        !frame[prompt_row].contains(&selection_bg()),
        "prompt rows must never be highlighted"
    );

    tui.handle_mouse_event(right_mouse(
        MouseKind::Press,
        cell(todo_col),
        cell(todo_row),
    ));
    let copied = tui.take_pending_copy().expect("crossing copy");
    assert!(copied.contains("first item"), "{copied}");
    assert!(
        !copied.contains("input-box-text"),
        "the prompt input must never be copied: {copied:?}"
    );
}

#[test]
fn frame_selection_cjk_row_keeps_full_content_and_highlight() {
    // A pane row with wide (CJK) content between the column borders: the
    // content span is measured in display cells, so the whole title is
    // highlighted and copied — never truncated at the wide-char boundary.
    let mut tui = new_tui();
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![TaskBrowserItem {
        id: "task-1".to_owned(),
        kind: TaskBrowserKind::Question,
        status: TaskBrowserStatus::Waiting,
        title: "中文任务".to_owned(),
        description: String::new(),
        elapsed: String::new(),
        detail_lines: Vec::new(),
        preview_lines: Vec::new(),
        can_stop: false,
        human_handle: None,
        list_cursor: None,
        workflow: None,
    }]));
    tui.chrome_mut().push_task_browser_overlay(state);
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (row, _) = locate(&frame, "中文任务");
    let plain = strip_ansi(&frame[row]);
    let lead_col = locate_in(&frame[row], "\u{2502}");
    let trail_byte = plain.rfind('\u{2502}').expect("trailing pane border");
    let trail_col = plain[..trail_byte].chars().count();
    drag_select(&mut tui, row, lead_col, row, trail_col);
    assert!(tui.has_any_selection(), "the CJK pane row is selectable");

    // The highlight covers every CJK cell: the text after the last selection
    // background keeps the full title, and the trailing border stays clean.
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let painted = &frame[row];
    let last_bg = painted
        .rfind(&selection_bg())
        .expect("the CJK content cells are highlighted");
    let after = strip_ansi(&painted[last_bg + selection_bg().len()..]);
    assert!(
        after.contains("中文任务"),
        "the full CJK title must be highlighted: {after:?}"
    );
    assert!(
        after.ends_with(" \u{2502}"),
        "the trailing border cell must not be highlighted: {after:?}"
    );

    // The copy keeps every CJK character whole.
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(lead_col), cell(row)));
    let copied = tui.take_pending_copy().expect("CJK pane row copy");
    assert!(
        copied.contains("中文任务"),
        "the CJK title must copy whole: {copied:?}"
    );
    assert!(
        !copied.contains('\u{2502}'),
        "column separators must never be copied: {copied:?}"
    );
}

#[test]
fn frame_selection_cross_column_drag_stays_in_press_column() {
    // Split task browser at 120 columns: a multi-row drag that stays inside
    // the left task column selects a rectangle over the press column, so the
    // split separator and the inspector column stay out of both the
    // highlight and the copied text.
    let mut tui = new_tui();
    let mut state = TaskBrowserState::new();
    let item = |id: &str, title: &str| TaskBrowserItem {
        id: id.to_owned(),
        kind: TaskBrowserKind::Bash,
        status: TaskBrowserStatus::Running,
        title: title.to_owned(),
        description: String::new(),
        elapsed: "00:01".to_owned(),
        detail_lines: vec!["D-right-column-detail".to_owned()],
        preview_lines: vec!["P-right-column-preview".to_owned()],
        can_stop: true,
        human_handle: None,
        list_cursor: None,
        workflow: None,
    };
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![
        item("task-1", "task one"),
        item("task-2", "task two"),
    ]));
    tui.chrome_mut().push_task_browser_overlay(state);
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(120, 24, now).lines;
    let (row1, col1) = locate(&frame, "task one");
    let (row2, col2) = locate(&frame, "task two");
    let end_col = col2 + "task two".len();
    // The drag must stay inside the left column. The anchor row's last `│`
    // is the split separator (the inspector identity rows carry no border).
    let middle_col = {
        let plain = strip_ansi(&frame[row1]);
        let byte = plain.rfind('\u{2502}').expect("split column separator");
        plain[..byte].chars().count()
    };
    assert!(
        end_col < middle_col,
        "the drag must stay inside the left column: end_col={end_col}, middle={middle_col}"
    );
    drag_select(&mut tui, row1, col1, row2, end_col);
    assert!(tui.has_any_selection(), "the left-column drag selects");

    // The highlight covers the left column only: the selection paint is one
    // contiguous run, so its trailing background reset marks where it ends.
    // On every selected row the reset lands inside the left column — the
    // split separator and the inspector text after it stay unpainted.
    let bg_reset = bg_to_ansi(Color::Reset);
    let frame = tui.render_terminal_frame_at(120, 24, now).lines;
    for row in row1..=row2 {
        let painted = &frame[row];
        let reset = painted
            .rfind(&bg_reset)
            .expect("the painted run ends with a background reset");
        let after = strip_ansi(&painted[reset + bg_reset.len()..]);
        assert!(
            after.contains('\u{2502}'),
            "the split separator must not be highlighted: {after:?}"
        );
        assert!(
            after.contains("task one") || after.contains("DETAILS"),
            "the inspector column must not be highlighted: {after:?}"
        );
    }

    // Right-click copy materializes the same rectangle: the left column task
    // texts come along, the inspector column never does.
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(col1), cell(row1)));
    let copied = tui.take_pending_copy().expect("left-column copy");
    assert!(copied.contains("task one"), "{copied}");
    assert!(copied.contains("task two"), "{copied}");
    assert!(
        !copied.contains("D-right-column-detail") && !copied.contains("P-right-column-preview"),
        "the inspector content must never be copied: {copied:?}"
    );
    assert!(
        !copied.contains(" DETAILS ") && !copied.contains("LATEST OUTPUT"),
        "the inspector dividers must never be copied: {copied:?}"
    );
    assert!(
        !copied.contains("running"),
        "the inspector identity must never be copied: {copied:?}"
    );
    assert!(
        copied
            .chars()
            .all(|ch| !('\u{2500}'..='\u{257F}').contains(&ch)),
        "box-drawing borders must never be copied: {copied:?}"
    );
}

#[test]
fn frame_selection_notice_never_overwrites_selected_footer() {
    let mut tui = tui_with_todos();
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let footer_row = frame.len() - 1;
    let footer_before = strip_ansi(&frame[footer_row]);
    assert!(
        !footer_before.contains("selected"),
        "no selection hint before any selection: {footer_before:?}"
    );

    // Selecting the footer itself must not replace it with the hint: the
    // footer keeps its text, stays highlighted, and copies as selected text.
    drag_select(&mut tui, footer_row, 1, footer_row, 12);
    assert!(tui.has_any_selection(), "the footer row is selectable");
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let footer_after = strip_ansi(frame.last().expect("footer"));
    assert_eq!(
        footer_after, footer_before,
        "a selected footer must keep its original text"
    );
    assert!(
        !footer_after.contains("selected"),
        "no hint may cover a selected footer: {footer_after:?}"
    );
    assert!(
        frame.last().expect("footer").contains(&selection_bg()),
        "the selected footer stays highlighted"
    );
    tui.handle_mouse_event(right_mouse(MouseKind::Press, cell(5), cell(footer_row)));
    let copied = tui.take_pending_copy().expect("footer copy");
    assert_eq!(
        copied,
        slice_text_by_cells(&footer_before, 1, 13),
        "the footer copies as ordinary selected text"
    );
    assert!(!copied.contains("selected"), "{copied:?}");

    // Control: a selection elsewhere still writes the hint into the footer.
    let mut tui = tui_with_todos();
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    let (todo_row, todo_col) = locate(&frame, "first item");
    let (row2, col2) = locate(&frame, "second item");
    drag_select(
        &mut tui,
        todo_row,
        todo_col,
        row2,
        col2 + "second item".len(),
    );
    let frame = tui.render_terminal_frame_at(80, 24, now).lines;
    assert!(
        strip_ansi(frame.last().expect("footer")).contains("selected"),
        "the hint still replaces the footer when the selection is elsewhere"
    );
}
