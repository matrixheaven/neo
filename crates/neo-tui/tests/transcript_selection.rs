//! Document-coordinate selection integration tests: cross-entry drag with
//! frame-driven auto-scroll materializes the exact document text, and
//! double-click selects one Unicode word.

use crossterm::event::{KeyModifiers, MouseButton};
use neo_tui::transcript::{MouseEvent, MouseKind, TranscriptPane};

fn pane_with_status_rows(count: usize) -> TranscriptPane {
    let mut pane = TranscriptPane::new(80, 20);
    for index in 0..count {
        pane.push_status(format!("row-{index}"));
    }
    pane
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

#[test]
fn selection_crosses_entries_autoscrolls_and_materializes_text() {
    let mut pane = pane_with_status_rows(8);
    // Establish the body height (6 rows) and the tail-following layout.
    pane.render_visible_slice(80, 6);

    // A downward drag across the card boundary selects the exact rendered
    // text, including the blank separator row between the cards.
    pane.handle_mouse_event(mouse(MouseKind::Press, 1, 1), 1, 0);
    pane.handle_mouse_event(mouse(MouseKind::Drag, 7, 3), 3, 6);
    pane.handle_mouse_event(mouse(MouseKind::Release, 7, 3), 3, 6);
    assert_eq!(
        pane.copy_selected_transcript_text().as_deref(),
        Some("row-5\n\nrow-6")
    );

    // Dragging above the top edge requests auto-scroll; each rendered frame
    // advances the document one row while the anchor stays fixed.
    pane.handle_mouse_event(mouse(MouseKind::Press, 1, 5), 5, 0);
    pane.handle_mouse_event(mouse(MouseKind::Drag, 6, 0), 0, 5);
    for _ in 0..9 {
        pane.render_visible_slice(80, 6);
    }
    pane.handle_mouse_event(mouse(MouseKind::Release, 6, 0), 0, 5);

    // The drag covered the whole document; materialized text spans every
    // card with its separator rows, exactly as rendered.
    assert_eq!(
        pane.copy_selected_transcript_text().as_deref(),
        Some("row-0\n\nrow-1\n\nrow-2\n\nrow-3\n\nrow-4\n\nrow-5\n\nrow-6\n\nrow-7")
    );

    // Later document updates cannot change the materialized text.
    pane.push_status("row-8");
    assert_eq!(
        pane.copy_selected_transcript_text().as_deref(),
        Some("row-0\n\nrow-1\n\nrow-2\n\nrow-3\n\nrow-4\n\nrow-5\n\nrow-6\n\nrow-7")
    );

    // A plain click clears the selection; Shift-modified drags never touch it.
    pane.handle_mouse_event(mouse(MouseKind::Press, 1, 0), 0, 0);
    pane.handle_mouse_event(mouse(MouseKind::Release, 1, 0), 0, 0);
    assert!(pane.copy_selected_transcript_text().is_none());
    pane.handle_mouse_event(mouse(MouseKind::Press, 5, 0), 0, 4);
    pane.handle_mouse_event(mouse(MouseKind::Drag, 6, 4), 4, 5);
    pane.handle_mouse_event(mouse(MouseKind::Release, 6, 4), 4, 5);
    let selected = pane.copy_selected_transcript_text();
    assert!(selected.is_some());
    let shift_press = MouseEvent {
        modifiers: KeyModifiers::SHIFT,
        ..mouse(MouseKind::Press, 1, 1)
    };
    let shift_drag = MouseEvent {
        modifiers: KeyModifiers::SHIFT,
        ..mouse(MouseKind::Drag, 4, 4)
    };
    let shift_release = MouseEvent {
        modifiers: KeyModifiers::SHIFT,
        ..mouse(MouseKind::Release, 4, 4)
    };
    pane.handle_mouse_event(shift_press, 1, 0);
    pane.handle_mouse_event(shift_drag, 4, 3);
    pane.handle_mouse_event(shift_release, 4, 3);
    assert_eq!(pane.copy_selected_transcript_text(), selected);
}

#[test]
fn upward_drag_materialization_respects_endpoint_cells() {
    let mut pane = pane_with_status_rows(8);
    pane.render_visible_slice(80, 6);

    // Press inside "row-7" at cell 2 and drag upward to "row-5" at cell 3:
    // the materialized range must start at the active cell on the min row
    // and start at the anchor cell on the max row, mirroring the downward
    // case instead of copying both rows whole.
    pane.handle_mouse_event(mouse(MouseKind::Press, 3, 5), 5, 2);
    pane.handle_mouse_event(mouse(MouseKind::Drag, 4, 1), 1, 3);
    pane.handle_mouse_event(mouse(MouseKind::Release, 4, 1), 1, 3);
    assert_eq!(
        pane.copy_selected_transcript_text().as_deref(),
        Some("row-\n\nrow-6\n\nw-7")
    );
}

#[test]
fn double_click_selects_one_unicode_word() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.push_status("select 你好 world");
    pane.render_visible_slice(80, 6);

    // 你 occupies display cells 7..9 (each CJK char is two cells wide).
    let word_cell: usize = 8;
    let column = u16::try_from(word_cell).expect("cell fits u16") + 1;
    pane.handle_mouse_event(mouse(MouseKind::Press, column, 0), 0, word_cell);
    pane.handle_mouse_event(mouse(MouseKind::Release, column, 0), 0, word_cell);
    pane.handle_mouse_event(mouse(MouseKind::Press, column, 0), 0, word_cell);
    pane.handle_mouse_event(mouse(MouseKind::Release, column, 0), 0, word_cell);

    assert_eq!(
        pane.copy_selected_transcript_text().as_deref(),
        Some("你好")
    );

    // A single click on a Latin word does not select; a double-click does.
    let mut pane = TranscriptPane::new(80, 20);
    pane.push_status("select hello world");
    pane.render_visible_slice(80, 6);
    let column = 9;
    pane.handle_mouse_event(mouse(MouseKind::Press, column, 0), 0, 8);
    pane.handle_mouse_event(mouse(MouseKind::Release, column, 0), 0, 8);
    assert!(pane.copy_selected_transcript_text().is_none());
    pane.handle_mouse_event(mouse(MouseKind::Press, column, 0), 0, 8);
    pane.handle_mouse_event(mouse(MouseKind::Release, column, 0), 0, 8);
    assert_eq!(
        pane.copy_selected_transcript_text().as_deref(),
        Some("hello")
    );
}
