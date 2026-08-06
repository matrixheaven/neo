//! Focused interactive tests for document-coordinate selection: mouse events
//! reach the transcript selection through the controller, Shift-modified
//! drags stay uninterpreted, Task Browser input priority is preserved, and
//! pending approvals/questions keep keyboard ownership while left-button
//! selection drags keep reaching the transcript.
//!
//! Kept outside `tests.rs` per the fullscreen transcript plan so the growing
//! controller test file does not absorb more surface.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use neo_agent_core::{
    ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResponse,
    PendingQuestion, PermissionOperation, QuestionEventData, QuestionOptionData,
};
use tokio::sync::oneshot;

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

fn right_button_event(kind: MouseKind, column: u16, row: u16) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        button: crossterm::event::MouseButton::Right,
        column,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    })
}

/// A shell approval with two options, ready for `register_pending_approval`.
fn pending_shell_approval(
    id: &str,
    command: &str,
) -> (
    crate::modes::run::PendingApproval,
    oneshot::Receiver<ApprovalResponse>,
) {
    let (response_tx, response_rx) = oneshot::channel();
    (
        crate::modes::run::PendingApproval {
            request: ApprovalRequest {
                turn: 1,
                id: id.to_owned(),
                operation: PermissionOperation::Shell,
                presentation: ApprovalPresentation::Command {
                    title: "Run this command?".to_owned(),
                    command: command.to_owned(),
                    cwd: None,
                },
                options: vec![
                    ApprovalOption {
                        label: "Approve once".to_owned(),
                        description: None,
                        action: ApprovalAction::PermitOnce,
                    },
                    ApprovalOption {
                        label: "Reject".to_owned(),
                        description: None,
                        action: ApprovalAction::Reject,
                    },
                ],
                workflow_origin: None,
            },
            response_tx,
        },
        response_rx,
    )
}

/// A controller with eight status rows and a rendered 6-row tail view.
fn selection_controller() -> InteractiveController {
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
    let _ = controller.transcript_mut().render_visible_slice(80, 6);
    controller
}

/// Drag across the whole visible body. The tail view shows the pending
/// approval/question card, so the materialized text covers the card's
/// visible rows; the caller asserts on the card's stable labels.
async fn drag_visible_tail(controller: &mut InteractiveController) {
    // Re-render so a pending approval/question card joins the layout; the
    // tail-following view then shows that card.
    let _ = controller.transcript_mut().render_visible_slice(80, 6);
    for event in [
        mouse_event(MouseKind::Press, 1, 0, crossterm::event::KeyModifiers::NONE),
        mouse_event(MouseKind::Drag, 4, 5, crossterm::event::KeyModifiers::NONE),
        mouse_event(
            MouseKind::Release,
            4,
            5,
            crossterm::event::KeyModifiers::NONE,
        ),
    ] {
        controller
            .handle_input_event(event)
            .await
            .expect("mouse event routed");
    }
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

#[tokio::test]
async fn mouse_selection_works_while_approval_owns_keyboard() {
    let mut controller = selection_controller();

    let (pending, mut response_rx) = pending_shell_approval("approval-1", "sudo --version");
    assert!(controller.register_pending_approval(pending));
    assert!(controller.chrome().approval_is_pending());

    // Left-button selection events keep reaching the transcript selection
    // while the approval owns keyboard selection and submission. The tail
    // view shows the pending approval card, so the drag selects its rows.
    drag_visible_tail(&mut controller).await;
    let selected = controller
        .transcript_mut()
        .copy_selected_transcript_text()
        .expect("drag built a transcript selection");
    assert!(
        selected.contains("Approve once"),
        "approval must not swallow transcript selection drags: {selected:?}"
    );

    // The mouse gesture never moved the approval's keyboard selection.
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|value| value.1),
        Some(0),
        "approval keyboard selection stays on the first option"
    );

    // Keyboard still owns the approval: arrows move the selection and
    // confirm submits it.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("arrow down reaches the approval");
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|value| value.1),
        Some(1)
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm submits the approval");
    assert!(
        !controller.chrome().approval_is_pending(),
        "keyboard submit resolves the approval"
    );
    assert!(
        response_rx.try_recv().is_ok(),
        "the approval response reaches the runtime"
    );
}

#[tokio::test]
async fn mouse_selection_works_while_question_owns_keyboard() {
    let mut controller = selection_controller();

    let (response_tx, mut response_rx) = oneshot::channel();
    controller.register_pending_question(PendingQuestion {
        id: "q-1".to_owned(),
        questions: vec![QuestionEventData {
            question: "Pick a side?".to_owned(),
            header: Some("Choice".into()),
            body: None,
            options: vec![
                QuestionOptionData {
                    label: "Left".to_owned(),
                    description: None,
                },
                QuestionOptionData {
                    label: "Right".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        response_tx,
        workflow_origin: None,
    });
    assert!(controller.chrome().question_dialog_is_focused());

    // Left-button selection events keep reaching the transcript while the
    // question dialog owns keyboard input. The tail view shows the pending
    // question card, so the drag selects its option rows.
    drag_visible_tail(&mut controller).await;
    let selected = controller
        .transcript_mut()
        .copy_selected_transcript_text()
        .expect("drag built a transcript selection");
    assert!(
        selected.contains("Left") && selected.contains("Right"),
        "question dialog must not swallow transcript selection drags: {selected:?}"
    );
    assert!(
        controller.chrome().question_dialog_is_focused(),
        "the question dialog keeps focus through mouse events"
    );

    // Keyboard still owns the question: escape cancels it and delivers the
    // response.
    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("escape reaches the question");
    assert!(
        !controller.chrome().question_dialog_is_focused(),
        "escape cancels the question dialog"
    );
    assert!(
        response_rx.try_recv().is_err(),
        "cancelling drops the response channel without a reply"
    );
}

#[tokio::test]
async fn right_click_copies_current_selection_to_clipboard() {
    let mut controller = selection_controller();
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let writer_recorded = Arc::clone(&recorded);
    controller.set_clipboard_writer(Arc::new(move |text| {
        let recorded = Arc::clone(&writer_recorded);
        Box::pin(async move {
            recorded.lock().expect("record clipboard text").push(text);
            Ok(())
        })
    }));

    // A plain left-button drag materializes the selection first.
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
            .expect("mouse drag handled");
    }
    let expected = controller
        .transcript_mut()
        .copy_selected_transcript_text()
        .expect("drag produced a selection");
    assert_eq!(expected, "row-5\n\nrow-6");

    // Right-click copies the selection to the system clipboard.
    controller
        .handle_input_event(right_button_event(MouseKind::Press, 10, 3))
        .await
        .expect("right click handled");
    wait_for_clipboard_write(&mut controller).await;
    assert_eq!(
        recorded.lock().expect("clipboard writes").as_slice(),
        [expected.as_str()],
        "right-click must copy the current selection"
    );

    // A keyboard entry selection is copied by right-click too.
    controller.transcript_mut().clear_transcript_selection();
    controller
        .transcript_mut()
        .select_visible_transcript_entry();
    let keyboard_text = controller
        .transcript_mut()
        .copy_selected_transcript_text()
        .expect("keyboard selection materializes");
    controller
        .handle_input_event(right_button_event(MouseKind::Press, 10, 3))
        .await
        .expect("right click handled");
    wait_for_clipboard_write(&mut controller).await;
    assert_eq!(
        recorded.lock().expect("clipboard writes").as_slice(),
        [expected.as_str(), keyboard_text.as_str()],
        "right-click must copy a keyboard selection too"
    );
}

#[tokio::test]
async fn right_click_without_selection_writes_nothing() {
    let mut controller = selection_controller();
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let writer_recorded = Arc::clone(&recorded);
    controller.set_clipboard_writer(Arc::new(move |text| {
        let recorded = Arc::clone(&writer_recorded);
        Box::pin(async move {
            recorded.lock().expect("record clipboard text").push(text);
            Ok(())
        })
    }));

    controller
        .handle_input_event(right_button_event(MouseKind::Press, 10, 3))
        .await
        .expect("right click handled");
    wait_for_clipboard_write(&mut controller).await;
    assert!(
        recorded.lock().expect("clipboard writes").is_empty(),
        "right-click without a selection must not touch the clipboard"
    );
}

#[tokio::test]
async fn ctrl_space_toggles_transcript_selection() {
    let mut controller = selection_controller();

    // Without a selection, ctrl+space selects the visible entry.
    controller
        .handle_input_event(InputEvent::Action(
            KeybindingAction::TranscriptSelectionStart,
        ))
        .await
        .expect("selection start handled");
    assert!(controller.transcript().has_transcript_selection());

    // With a selection, the same key clears it — this is the path the
    // advertised clear key (ctrl+shift+space) lands on when the terminal
    // cannot send the distinct kitty sequence.
    controller
        .handle_input_event(InputEvent::Action(
            KeybindingAction::TranscriptSelectionStart,
        ))
        .await
        .expect("selection toggle handled");
    assert!(!controller.transcript().has_transcript_selection());

    // A mouse drag selection is toggled off the same way.
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
            .expect("mouse drag handled");
    }
    assert!(controller.transcript().has_transcript_selection());
    controller
        .handle_input_event(InputEvent::Action(
            KeybindingAction::TranscriptSelectionStart,
        ))
        .await
        .expect("selection toggle handled");
    assert!(!controller.transcript().has_transcript_selection());
}

/// Row and display column of `needle` in the rendered TUI frame.
fn locate_in_frame(frame: &[String], needle: &str) -> (usize, usize) {
    for (row, line) in frame.iter().enumerate() {
        let stripped = neo_tui::primitive::strip_ansi(line);
        if let Some(byte_col) = stripped.find(needle) {
            return (row, stripped[..byte_col].chars().count());
        }
    }
    panic!("needle {needle:?} not found in frame: {frame:?}");
}

fn prompt_selection_controller() -> InteractiveController {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller
        .tui
        .chrome_mut()
        .prompt_mut()
        .set_text("hello world");
    // Render a full frame so mouse routing has a layout and the prompt box
    // occupies known screen rows.
    let _ = controller
        .tui
        .render_terminal_frame_at(80, 24, Instant::now());
    controller
}

#[tokio::test]
async fn right_click_in_prompt_copies_prompt_selection() {
    let mut controller = prompt_selection_controller();
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let writer_recorded = Arc::clone(&recorded);
    controller.set_clipboard_writer(Arc::new(move |text| {
        let recorded = Arc::clone(&writer_recorded);
        Box::pin(async move {
            recorded.lock().expect("record clipboard text").push(text);
            Ok(())
        })
    }));

    let frame = controller
        .tui
        .render_terminal_frame_at(80, 24, Instant::now())
        .lines;
    let (row, hello_col) = locate_in_frame(&frame, "hello");

    // Drag across "hello" in the prompt box.
    for event in [
        mouse_event(
            MouseKind::Press,
            hello_col as u16,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
        mouse_event(
            MouseKind::Drag,
            (hello_col + 5) as u16,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
        mouse_event(
            MouseKind::Release,
            (hello_col + 5) as u16,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
    ] {
        controller
            .handle_input_event(event)
            .await
            .expect("prompt drag handled");
    }
    assert_eq!(
        controller.tui.chrome().prompt().selection_text().as_deref(),
        Some("hello")
    );

    // Right-click over the prompt copies the selection to the clipboard.
    controller
        .handle_input_event(right_button_event(
            MouseKind::Press,
            hello_col as u16,
            row as u16,
        ))
        .await
        .expect("right click handled");
    wait_for_clipboard_write(&mut controller).await;
    assert_eq!(
        recorded.lock().expect("clipboard writes").as_slice(),
        ["hello"],
        "right-click in the prompt copies the prompt selection"
    );
}

#[tokio::test]
async fn ctrl_c_prefers_prompt_selection_over_whole_text() {
    let mut controller = prompt_selection_controller();
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let writer_recorded = Arc::clone(&recorded);
    controller.set_clipboard_writer(Arc::new(move |text| {
        let recorded = Arc::clone(&writer_recorded);
        Box::pin(async move {
            recorded.lock().expect("record clipboard text").push(text);
            Ok(())
        })
    }));

    let frame = controller
        .tui
        .render_terminal_frame_at(80, 24, Instant::now())
        .lines;
    let (row, hello_col) = locate_in_frame(&frame, "hello");
    for event in [
        mouse_event(
            MouseKind::Press,
            hello_col as u16,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
        mouse_event(
            MouseKind::Drag,
            (hello_col + 5) as u16,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
        mouse_event(
            MouseKind::Release,
            (hello_col + 5) as u16,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
    ] {
        controller
            .handle_input_event(event)
            .await
            .expect("prompt drag handled");
    }
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputCopy))
        .await
        .expect("copy action handled");
    wait_for_clipboard_write(&mut controller).await;
    assert_eq!(
        recorded.lock().expect("clipboard writes").as_slice(),
        ["hello"],
        "ctrl+c copies the prompt selection, not the whole text"
    );
}

#[tokio::test]
async fn ctrl_c_copies_todo_selection() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.tui.chrome_mut().set_todo_items(vec![
        neo_tui::widgets::todo_panel::TodoDisplayItem::new(
            "first item",
            neo_tui::widgets::todo_panel::TodoDisplayStatus::Pending,
        ),
        neo_tui::widgets::todo_panel::TodoDisplayItem::new(
            "second item",
            neo_tui::widgets::todo_panel::TodoDisplayStatus::Done,
        ),
    ]);
    let _ = controller
        .tui
        .render_terminal_frame_at(80, 24, Instant::now());
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let writer_recorded = Arc::clone(&recorded);
    controller.set_clipboard_writer(Arc::new(move |text| {
        let recorded = Arc::clone(&writer_recorded);
        Box::pin(async move {
            recorded.lock().expect("record clipboard text").push(text);
            Ok(())
        })
    }));

    let frame = controller
        .tui
        .render_terminal_frame_at(80, 24, Instant::now())
        .lines;
    let (row, col) = locate_in_frame(&frame, "first item");
    let (row2, col2) = locate_in_frame(&frame, "second item");
    let end_col = col2 + "second item".len();
    for event in [
        mouse_event(
            MouseKind::Press,
            col as u16,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
        mouse_event(
            MouseKind::Drag,
            end_col as u16,
            row2 as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
        mouse_event(
            MouseKind::Release,
            end_col as u16,
            row2 as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
    ] {
        controller
            .handle_input_event(event)
            .await
            .expect("todo drag handled");
    }
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputCopy))
        .await
        .expect("copy action handled");
    wait_for_clipboard_write(&mut controller).await;
    let copied = recorded.lock().expect("clipboard writes");
    assert_eq!(copied.len(), 1, "{copied:?}");
    assert!(
        copied[0].contains("first item") && copied[0].contains("second item"),
        "ctrl+c copies the todo selection: {:?}",
        copied
    );
}

/// Drain the controller-owned clipboard helper task. The helper runs on the
/// test runtime, so the poll loop must yield for it to make progress.
async fn wait_for_clipboard_write(controller: &mut InteractiveController) {
    while controller.pending_clipboard.is_some() {
        let _ = controller.poll_pending_clipboard().await;
        tokio::task::yield_now().await;
    }
}
