//! Focused interactive tests for document-coordinate selection: mouse events
//! reach the transcript selection through the controller, Shift-modified
//! drags stay uninterpreted, and Task Browser input priority is preserved.
//!
//! Kept outside `tests.rs` per the fullscreen transcript plan so the growing
//! controller test file does not absorb more surface.

use std::path::PathBuf;

use super::*;

fn test_workspace_root() -> PathBuf {
    let dir = std::env::temp_dir().join("neo-selection-test-workspace");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn mouse_event(
    kind: MouseKind,
    column: u16,
    row: u16,
    modifiers: crossterm::event::KeyModifiers,
) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        button: crossterm::event::MouseButton::Left,
        column,
        row,
        modifiers,
    })
}

#[tokio::test]
async fn selection_and_task_browser_preserve_input_priority() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    for index in 0..8 {
        controller
            .transcript_mut()
            .push_status(format!("row-{index}"));
    }
    // Establish the pane body height (6 rows) and the tail-following layout.
    let _ = controller.transcript_mut().render_visible_slice(80, 6);

    // A plain left-button drag across card boundaries selects and
    // materializes the exact document text.
    for event in [
        mouse_event(MouseKind::Press, 1, 1, crossterm::event::KeyModifiers::NONE),
        mouse_event(MouseKind::Drag, 7, 3, crossterm::event::KeyModifiers::NONE),
        mouse_event(
            MouseKind::Release,
            7,
            3,
            crossterm::event::KeyModifiers::NONE,
        ),
    ] {
        controller
            .handle_input_event(event)
            .await
            .expect("mouse event handled");
    }
    let copied = controller
        .transcript_mut()
        .copy_selected_transcript_text()
        .expect("drag produced a selection");
    assert_eq!(copied, "row-5\n\nrow-6");

    // Shift-modified drags are not interpreted by Neo: the existing
    // selection stays untouched.
    let before_shift = controller
        .transcript_mut()
        .copy_selected_transcript_text()
        .expect("selection still active");
    for event in [
        mouse_event(
            MouseKind::Press,
            3,
            2,
            crossterm::event::KeyModifiers::SHIFT,
        ),
        mouse_event(MouseKind::Drag, 4, 4, crossterm::event::KeyModifiers::SHIFT),
        mouse_event(
            MouseKind::Release,
            4,
            4,
            crossterm::event::KeyModifiers::SHIFT,
        ),
    ] {
        controller
            .handle_input_event(event)
            .await
            .expect("shift drag handled");
    }
    assert_eq!(
        controller
            .transcript_mut()
            .copy_selected_transcript_text()
            .expect("selection still active"),
        before_shift
    );

    // Task Browser keeps input priority: while it is open, mouse events are
    // consumed by the browser and never reach the document selection.
    controller
        .tui
        .chrome_mut()
        .push_task_browser_overlay(neo_tui::tasks_browser::TaskBrowserState::new());
    assert!(controller.chrome().task_browser_state().is_some());
    let before_browser = controller
        .transcript_mut()
        .copy_selected_transcript_text()
        .expect("selection still active");
    for event in [
        mouse_event(MouseKind::Press, 1, 1, crossterm::event::KeyModifiers::NONE),
        mouse_event(MouseKind::Drag, 1, 5, crossterm::event::KeyModifiers::NONE),
        mouse_event(
            MouseKind::Release,
            1,
            5,
            crossterm::event::KeyModifiers::NONE,
        ),
    ] {
        controller
            .handle_input_event(event)
            .await
            .expect("browser mouse event handled");
    }
    assert!(
        controller.chrome().task_browser_state().is_some(),
        "task browser keeps focus through mouse events"
    );
    assert_eq!(
        controller
            .transcript_mut()
            .copy_selected_transcript_text()
            .expect("selection still active"),
        before_browser,
        "selection untouched while task browser is open"
    );
}
