//! Document-coordinate selection integration tests: cross-entry drag with
//! frame-driven auto-scroll materializes the exact document text,
//! double-click selects one Unicode word, keyboard entry selection falls
//! back to the tail before the first render, mouse gestures interact
//! with keyboard selection by the click/drag threshold, and rendered frames
//! paint the selection background on exactly the intersecting document cells.

use crossterm::event::{KeyModifiers, MouseButton};
use neo_tui::NeoTui;
use neo_tui::primitive::{TuiTheme, strip_ansi, visible_width};
use neo_tui::shell::NeoChromeState;
use neo_tui::transcript::{MouseEvent, MouseKind, TranscriptPane};
use std::time::Instant;

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

/// The display-cell ranges of `line` whose background is the selection color.
/// Walks the raw ANSI stream with the same cell math as the painter, so the
/// test asserts the exact painted range instead of a stripped view.
fn selection_bg_ranges(line: &str) -> Vec<(usize, usize)> {
    const SELECTION_BG: &str = "\x1b[100m"; // TuiTheme::default().selection_bg
    const BG_RESET: &str = "\x1b[49m";
    let mut ranges = Vec::new();
    let mut active = false;
    let mut range_start = 0usize;
    let mut cell = 0usize;
    let mut rest = line;
    while !rest.is_empty() {
        let Some(sequence) = take_ansi_sequence(rest) else {
            let character = rest.chars().next().expect("non-empty slice");
            cell += visible_width(&character.to_string());
            rest = &rest[character.len_utf8()..];
            continue;
        };
        if sequence == SELECTION_BG {
            active = true;
            range_start = cell;
        } else if sequence == BG_RESET && active {
            ranges.push((range_start, cell));
            active = false;
        }
        rest = &rest[sequence.len()..];
    }
    if active {
        ranges.push((range_start, cell));
    }
    ranges
}

/// The ANSI escape sequence starting at `s`, or `None` when `s` starts with
/// plain text. Mirrors the parser's accepted sequence shapes.
fn take_ansi_sequence(s: &str) -> Option<&str> {
    if !s.starts_with('\x1b') {
        return None;
    }
    let mut chars = s.chars();
    chars.next();
    match chars.next() {
        Some('[') => {
            let mut end = 2;
            for ch in s[2..].chars() {
                end += ch.len_utf8();
                if ('\x40'..='\x7e').contains(&ch) {
                    return Some(&s[..end]);
                }
            }
            Some(s)
        }
        Some(']' | '_' | 'P' | '^' | 'X') => {
            let mut end = 2;
            let mut chars = s[2..].chars().peekable();
            while let Some(ch) = chars.next() {
                end += ch.len_utf8();
                if ch == '\x07' {
                    return Some(&s[..end]);
                }
                if ch == '\x1b' && chars.peek() == Some(&'\\') {
                    let _ = chars.next();
                    end += 1;
                    return Some(&s[..end]);
                }
            }
            Some(s)
        }
        Some(_) => Some(&s[..2]),
        None => Some(s),
    }
}

#[test]
fn keyboard_selection_without_render_falls_back_to_tail() {
    // No frame was ever rendered, so `body_height` is still zero. Keyboard
    // entry selection must fall back to the pane height and select the tail
    // entry — never anchor at the top of the document (the welcome banner).
    let mut pane = TranscriptPane::new(80, 20);
    pane.push_welcome_banner("neo", "test-session", "gpt-4.1", "/work", "9.9.9", None);
    pane.push_user_message("selected user prompt");
    pane.push_assistant_message("selected assistant reply");

    pane.select_visible_transcript_entry();
    assert_eq!(
        pane.copy_selected_transcript_text().as_deref(),
        Some("Assistant\nselected assistant reply"),
        "unrendered keyboard selection must pick the tail entry, not the banner"
    );

    // Extending upward from the tail covers the preceding entry too.
    pane.extend_transcript_selection_up(1);
    assert_eq!(
        pane.copy_selected_transcript_text().as_deref(),
        Some("You\nselected user prompt\n\nAssistant\nselected assistant reply")
    );
}

#[test]
fn mouse_click_preserves_keyboard_selection_but_confirmed_drag_replaces() {
    let mut pane = pane_with_status_rows(8);
    pane.render_visible_slice(80, 6);
    // Keyboard-select the tail entry.
    pane.select_visible_transcript_entry();
    let keyboard_text = pane
        .copy_selected_transcript_text()
        .expect("keyboard selection materializes");
    assert_eq!(keyboard_text, "Status\nrow-7");

    // A plain click does NOT destroy the keyboard selection: single clicks
    // stay inert and the copy still returns the keyboard entries. The click
    // sits 6 cells from the next press so the double-click window cannot
    // misclassify it.
    pane.handle_mouse_event(mouse(MouseKind::Press, 9, 5), 5, 8);
    pane.handle_mouse_event(mouse(MouseKind::Release, 9, 5), 5, 8);
    assert_eq!(
        pane.copy_selected_transcript_text().as_deref(),
        Some(keyboard_text.as_str()),
        "a click must keep the keyboard selection"
    );

    // A confirmed drag (movement past the threshold) replaces the keyboard
    // selection with the mouse gesture.
    pane.handle_mouse_event(mouse(MouseKind::Press, 3, 5), 5, 2);
    pane.handle_mouse_event(mouse(MouseKind::Drag, 4, 1), 1, 3);
    pane.handle_mouse_event(mouse(MouseKind::Release, 4, 1), 1, 3);
    assert_eq!(
        pane.copy_selected_transcript_text().as_deref(),
        Some("row-\n\nrow-6\n\nw-7"),
        "a confirmed drag must supersede the keyboard selection"
    );

    // The replaced selection is a mouse selection again: a plain click now
    // clears it (again kept 6 cells from the previous press).
    pane.handle_mouse_event(mouse(MouseKind::Press, 9, 5), 5, 8);
    pane.handle_mouse_event(mouse(MouseKind::Release, 9, 5), 5, 8);
    assert!(pane.copy_selected_transcript_text().is_none());
}

#[test]
fn release_ends_drag_and_hover_motion_never_extends() {
    let mut pane = pane_with_status_rows(8);
    let _ = pane.render_visible_slice(80, 6);

    // A confirmed downward drag selects "row-5" through "row-6".
    pane.handle_mouse_event(mouse(MouseKind::Press, 1, 1), 1, 0);
    pane.handle_mouse_event(mouse(MouseKind::Drag, 7, 3), 3, 6);
    pane.handle_mouse_event(mouse(MouseKind::Release, 7, 3), 3, 6);
    let released = pane.copy_selected_transcript_text();
    assert_eq!(released.as_deref(), Some("row-5\n\nrow-6"));
    let released_highlights: Vec<Vec<(usize, usize)>> = pane
        .render_visible_slice(80, 6)
        .iter()
        .map(|line| selection_bg_ranges(line))
        .collect();
    assert!(
        released_highlights.iter().any(|ranges| !ranges.is_empty()),
        "the released selection must stay highlighted"
    );

    // Any-event terminals keep reporting no-button hover motion as drags
    // after the release; it must never re-arm the drag or move the endpoint.
    pane.handle_mouse_event(mouse(MouseKind::Drag, 9, 0), 0, 8);
    pane.handle_mouse_event(mouse(MouseKind::Drag, 11, 5), 5, 10);
    let after_motion: Vec<Vec<(usize, usize)>> = pane
        .render_visible_slice(80, 6)
        .iter()
        .map(|line| selection_bg_ranges(line))
        .collect();
    assert_eq!(
        after_motion, released_highlights,
        "hover motion after release must not extend the painted selection"
    );
    assert_eq!(
        pane.copy_selected_transcript_text(),
        released,
        "hover motion after release must not change the selection"
    );
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

#[test]
fn rendered_selection_highlights_exact_document_cells() {
    // Three status cards lay out as: ["alpha", sep, "select 你好 world",
    // sep, "omega"] — the separator row belongs to the following card's
    // span, one visible row per document row.
    let mut pane = TranscriptPane::new(80, 20);
    pane.push_status("alpha");
    pane.push_status("select 你好 world");
    pane.push_status("omega");
    let lines = pane.render_visible_slice(80, 6);
    assert_eq!(
        lines
            .iter()
            .map(|line| strip_ansi(line))
            .collect::<Vec<_>>(),
        ["alpha", "", "select 你好 world", "", "omega"]
    );
    let selection_bg = neo_tui::primitive::bg_to_ansi(TuiTheme::default().selection_bg);
    assert_eq!(selection_bg, "\x1b[100m");

    // Same-row drag across cells [1, 5) of "alpha": exactly those cells are
    // painted, with the original style and text preserved, and every other
    // row stays unpainted.
    pane.handle_mouse_event(mouse(MouseKind::Press, 1, 0), 0, 1);
    pane.handle_mouse_event(mouse(MouseKind::Drag, 4, 0), 0, 4);
    pane.handle_mouse_event(mouse(MouseKind::Release, 4, 0), 0, 4);
    let lines = pane.render_visible_slice(80, 6);
    assert_eq!(selection_bg_ranges(&lines[0]), vec![(1, 5)]);
    assert_eq!(strip_ansi(&lines[0]), "alpha");
    assert!(
        lines[0].starts_with("\x1b[38;2;139;148;158m"),
        "the row's original foreground style survives: {:?}",
        lines[0]
    );
    for (index, line) in lines.iter().enumerate() {
        if index != 0 {
            assert!(
                selection_bg_ranges(line).is_empty(),
                "row {index} must stay unpainted: {line:?}"
            );
        }
    }

    // Cross-row, cross-entry drag: "select 你好 world" from cell 0 to the end
    // of the line, the blank separator row between cards selected as a
    // newline but not painted, and "omega" cut on the left of the active cell.
    pane.handle_mouse_event(mouse(MouseKind::Press, 1, 2), 2, 0);
    pane.handle_mouse_event(mouse(MouseKind::Drag, 4, 4), 4, 3);
    pane.handle_mouse_event(mouse(MouseKind::Release, 4, 4), 4, 3);
    let lines = pane.render_visible_slice(80, 6);
    assert_eq!(selection_bg_ranges(&lines[2]), vec![(0, 17)]);
    assert_eq!(selection_bg_ranges(&lines[3]), Vec::<(usize, usize)>::new());
    assert_eq!(selection_bg_ranges(&lines[4]), vec![(0, 4)]);
    assert_eq!(strip_ansi(&lines[2]), "select 你好 world");
    assert_eq!(strip_ansi(&lines[4]), "omega");
    assert!(selection_bg_ranges(&lines[0]).is_empty());
    assert!(selection_bg_ranges(&lines[1]).is_empty());

    // A double-click word selection paints the wide characters whole: 你好
    // spans cells 7..11 and the background covers exactly that range.
    pane.handle_mouse_event(mouse(MouseKind::Press, 9, 2), 2, 8);
    pane.handle_mouse_event(mouse(MouseKind::Release, 9, 2), 2, 8);
    pane.handle_mouse_event(mouse(MouseKind::Press, 9, 2), 2, 8);
    pane.handle_mouse_event(mouse(MouseKind::Release, 9, 2), 2, 8);
    let lines = pane.render_visible_slice(80, 6);
    assert_eq!(selection_bg_ranges(&lines[2]), vec![(7, 11)]);
    assert_eq!(
        pane.copy_selected_transcript_text().as_deref(),
        Some("你好"),
        "the wide-character word selection stays materialized"
    );

    // A keyboard entry selection highlights the whole card the same way.
    pane.select_visible_transcript_entry();
    let lines = pane.render_visible_slice(80, 6);
    assert_eq!(selection_bg_ranges(&lines[4]), vec![(0, 5)]);
    assert!(selection_bg_ranges(&lines[0]).is_empty());
    assert!(selection_bg_ranges(&lines[2]).is_empty());

    // Clearing the selection removes every highlight.
    pane.clear_transcript_selection();
    let lines = pane.render_visible_slice(80, 6);
    for (index, line) in lines.iter().enumerate() {
        assert!(
            selection_bg_ranges(line).is_empty(),
            "row {index} must be unpainted after clear: {line:?}"
        );
    }
}

#[test]
fn active_selection_shows_hint_line_without_covering_body() {
    let mut tui = NeoTui::new(
        NeoChromeState::new("neo", "s1", "m1", "/tmp/ws"),
        TranscriptPane::new(80, 20),
    );
    tui.transcript_mut().push_status("alpha");
    tui.transcript_mut().push_status("omega");

    // Without a selection no hint line exists.
    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());
    assert!(frame.lines.len() <= 12);
    assert!(
        frame
            .lines
            .iter()
            .all(|line| !strip_ansi(line).contains("ctrl+c copy"))
    );

    // A drag over "alpha" activates the selection; the frame keeps the body
    // and appends one hint line naming the copy/clear keybindings.
    tui.handle_mouse_event(MouseEvent {
        kind: MouseKind::Press,
        button: MouseButton::Left,
        column: 1,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    tui.handle_mouse_event(MouseEvent {
        kind: MouseKind::Drag,
        button: MouseButton::Left,
        column: 4,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    tui.handle_mouse_event(MouseEvent {
        kind: MouseKind::Release,
        button: MouseButton::Left,
        column: 4,
        row: 0,
        modifiers: KeyModifiers::NONE,
    });
    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());
    assert!(frame.lines.len() <= 12);
    assert!(
        frame
            .lines
            .iter()
            .any(|line| strip_ansi(line).trim() == "alpha"),
        "the body stays visible above the hint"
    );
    let hint = strip_ansi(frame.lines.last().expect("hint line"));
    assert!(
        hint.contains("selected")
            && hint.contains("ctrl+c copy")
            && hint.contains("ctrl+shift+space clear"),
        "the hint names the real copy/clear keybindings: {hint:?}"
    );

    // Clearing the selection removes the hint line again.
    tui.transcript_mut().clear_transcript_selection();
    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());
    assert!(frame.lines.len() <= 12);
    assert!(
        frame
            .lines
            .iter()
            .all(|line| !strip_ansi(line).contains("ctrl+c copy"))
    );
}
