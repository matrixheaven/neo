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

/// Drag from the top of the tail view down to `needle`'s row. The full
/// frame render gives mouse routing a layout whose transcript body shows
/// the pending approval/question card, so the materialized text covers the
/// card's option rows; the caller asserts on the card's stable labels.
async fn drag_visible_tail(controller: &mut InteractiveController, needle: &str) {
    // Re-render so a pending approval/question card joins the layout; the
    // tail-following view then shows that card.
    let frame = controller
        .tui
        .render_terminal_frame_at(80, 24, Instant::now())
        .lines;
    let (row, _) = locate_in_frame(&frame, needle);
    for event in [
        mouse_event(MouseKind::Press, 1, 0, crossterm::event::KeyModifiers::NONE),
        mouse_event(
            MouseKind::Drag,
            60,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
        mouse_event(
            MouseKind::Release,
            60,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
    ] {
        controller
            .handle_input_event(event)
            .await
            .expect("mouse event routed");
    }
}

/// A task-browser item whose title renders in the list pane.
fn task_item(id: &str, title: &str) -> neo_tui::tasks_browser::TaskBrowserItem {
    neo_tui::tasks_browser::TaskBrowserItem {
        id: id.to_owned(),
        kind: neo_tui::tasks_browser::TaskBrowserKind::Question,
        status: neo_tui::tasks_browser::TaskBrowserStatus::Waiting,
        title: title.to_owned(),
        description: String::new(),
        elapsed: String::new(),
        detail_lines: Vec::new(),
        preview_lines: Vec::new(),
        can_stop: false,
        human_handle: None,
        list_cursor: None,
        workflow: None,
    }
}

/// A task browser pre-loaded with three tasks.
fn task_browser_with_items() -> neo_tui::tasks_browser::TaskBrowserState {
    let mut state = neo_tui::tasks_browser::TaskBrowserState::new();
    state.apply_snapshot(&neo_tui::tasks_browser::TaskBrowserSnapshot::new(vec![
        task_item("task-1", "first task"),
        task_item("task-2", "second task"),
        task_item("task-3", "third task"),
    ]));
    state
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
    // Establish the tail-following layout: the full frame render gives
    // mouse routing a layout whose transcript body shows every status row.
    let frame = controller
        .tui
        .render_terminal_frame_at(80, 24, Instant::now())
        .lines;

    // A plain left-button drag across card boundaries selects and
    // materializes the exact document text.
    let (row5, _) = locate_in_frame(&frame, "row-5");
    let (row6, _) = locate_in_frame(&frame, "row-6");
    for event in [
        mouse_event(
            MouseKind::Press,
            1,
            row5 as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
        mouse_event(
            MouseKind::Drag,
            7,
            row6 as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
        mouse_event(
            MouseKind::Release,
            7,
            row6 as u16,
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

    // Task Browser keeps keyboard and wheel priority. Selection events are
    // pre-routed to the TUI before overlay input, so a drag over the task
    // rows selects the frame surface; the browser still receives the same
    // events afterwards (its left-press row navigation) and keeps focus.
    controller
        .tui
        .chrome_mut()
        .push_task_browser_overlay(task_browser_with_items());
    assert!(controller.chrome().task_browser_state().is_some());
    let frame = controller.tui.render_terminal_frame(120, 24).lines;
    let (row, col) = locate_in_frame(&frame, "first task");
    let (row2, col2) = locate_in_frame(&frame, "third task");
    let end_col = col2 + "third task".len();
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
            .expect("browser mouse event handled");
    }
    let frame_selection = controller
        .tui
        .frame_selection_text()
        .expect("drag over task rows built a frame selection");
    assert!(
        frame_selection.contains("first task") && frame_selection.contains("third task"),
        "the frame surface captures the task rows: {frame_selection:?}"
    );

    // Keyboard still owns the browser: SelectDown moves the task selection.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("arrow down reaches the browser");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .selected_task_id(),
        Some("task-2"),
        "keyboard selection moves inside the browser"
    );

    // Wheel over the task list still moves the browser selection.
    controller
        .handle_input_event(mouse_event(
            MouseKind::ScrollDown,
            col as u16,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ))
        .await
        .expect("wheel reaches the browser");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .selected_task_id(),
        Some("task-3"),
        "wheel selection moves inside the browser"
    );
    assert!(
        controller.chrome().task_browser_state().is_some(),
        "task browser keeps focus through mouse events"
    );
    assert!(
        !controller.transcript().has_transcript_selection(),
        "keyboard and wheel must not touch the transcript selection"
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
    drag_visible_tail(&mut controller, "Reject").await;
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
    drag_visible_tail(&mut controller, "Right").await;
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
    // A full frame render gives mouse routing a layout whose transcript
    // body shows every status row.
    let frame = controller
        .tui
        .render_terminal_frame_at(80, 24, Instant::now())
        .lines;
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
    let (row5, _) = locate_in_frame(&frame, "row-5");
    let (row6, _) = locate_in_frame(&frame, "row-6");
    for event in [
        mouse_event(
            MouseKind::Press,
            1,
            row5 as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
        mouse_event(
            MouseKind::Drag,
            7,
            row6 as u16,
            crossterm::event::KeyModifiers::NONE,
        ),
        mouse_event(
            MouseKind::Release,
            7,
            row6 as u16,
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
async fn selection_routes_before_task_browser_and_rich_dialog_without_stealing_input() {
    // (a) Task browser: a drag over the task rows reaches the frame
    // selection first, while keyboard and wheel keep going to the browser.
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
        .push_task_browser_overlay(task_browser_with_items());
    let frame = controller.tui.render_terminal_frame(120, 24).lines;
    let (row, col) = locate_in_frame(&frame, "first task");
    let (row2, col2) = locate_in_frame(&frame, "third task");
    let end_col = col2 + "third task".len();
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
            .expect("browser drag handled");
    }
    assert!(
        controller.tui.has_any_selection(),
        "selection events pre-route before the task browser"
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("keyboard reaches the browser");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .selected_task_id(),
        Some("task-2")
    );
    controller
        .handle_input_event(mouse_event(
            MouseKind::ScrollDown,
            col as u16,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ))
        .await
        .expect("wheel reaches the browser");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .selected_task_id(),
        Some("task-3"),
        "the wheel still moves the browser selection"
    );

    // (b) Rich dialog (choice picker): a drag over the dialog text reaches
    // the frame selection, while plain keys and the wheel still go through
    // the dialog's own input path.
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let theme = controller.tui.chrome().theme();
    controller
        .tui
        .chrome_mut()
        .open_choice_picker(neo_tui::dialogs::ChoicePickerOptions {
            title: "Pick one".to_owned(),
            items: vec![
                neo_tui::dialogs::ChoiceItem::new("option-1", "option-1"),
                neo_tui::dialogs::ChoiceItem::new("option-2", "option-2"),
                neo_tui::dialogs::ChoiceItem::new("option-3", "option-3"),
            ],
            initial_id: None,
            theme,
            page_size: 0,
            current_id: None,
        });
    let frame = controller.tui.render_terminal_frame(80, 24).lines;
    let (row, col) = locate_in_frame(&frame, "option-1");
    let (row2, col2) = locate_in_frame(&frame, "option-3");
    let end_col = col2 + "option-3".len();
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
            .expect("picker drag handled");
    }
    let frame_selection = controller
        .tui
        .frame_selection_text()
        .expect("drag over the picker built a frame selection");
    assert!(
        frame_selection.contains("option-1") && frame_selection.contains("option-3"),
        "the frame surface captures the picker rows: {frame_selection:?}"
    );

    // Plain keys still enter the picker: SelectDown moves its selection.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("keyboard reaches the picker");
    let frame = controller.tui.render_terminal_frame(80, 24).lines;
    let (marker_row, _) = locate_in_frame(&frame, "▸");
    let (option2_row, _) = locate_in_frame(&frame, "option-2");
    assert_eq!(
        marker_row, option2_row,
        "the picker selection follows the keyboard"
    );

    // The wheel still scrolls the picker along its own path.
    controller
        .handle_input_event(mouse_event(
            MouseKind::ScrollDown,
            col as u16,
            row as u16,
            crossterm::event::KeyModifiers::NONE,
        ))
        .await
        .expect("wheel reaches the picker");
    let frame = controller.tui.render_terminal_frame(80, 24).lines;
    let (marker_row, _) = locate_in_frame(&frame, "▸");
    let (option3_row, _) = locate_in_frame(&frame, "option-3");
    assert_eq!(
        marker_row, option3_row,
        "the wheel moves the picker selection"
    );
    assert!(
        controller.chrome().focused_overlay().is_some(),
        "the picker stays open through mouse events"
    );
}

#[tokio::test]
async fn right_click_copies_current_frame_selection() {
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
            .expect("frame drag handled");
    }
    controller
        .handle_input_event(right_button_event(MouseKind::Press, col as u16, row as u16))
        .await
        .expect("right click handled");
    wait_for_clipboard_write(&mut controller).await;
    let copied = recorded.lock().expect("clipboard writes");
    assert_eq!(copied.len(), 1, "{copied:?}");
    assert!(
        copied[0].contains("first item") && copied[0].contains("second item"),
        "right-click copies the current frame selection: {:?}",
        copied
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
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("ctrl+c handled");
    wait_for_clipboard_write(&mut controller).await;
    assert_eq!(
        recorded.lock().expect("clipboard writes").as_slice(),
        ["hello"],
        "ctrl+c copies the prompt selection, not the whole text"
    );
    assert_eq!(
        controller.chrome().exit_confirmation_label(),
        None,
        "ctrl+c with a selection must not arm the exit confirmation"
    );
    assert_eq!(
        controller.tui.chrome().prompt().text,
        "hello world",
        "ctrl+c with a selection must not clear the prompt"
    );
}

#[tokio::test]
async fn ctrl_c_copies_frame_selection_from_todo_rows() {
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
            .expect("frame drag handled");
    }
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("ctrl+c handled");
    wait_for_clipboard_write(&mut controller).await;
    let copied = recorded.lock().expect("clipboard writes");
    assert_eq!(copied.len(), 1, "{copied:?}");
    assert!(
        copied[0].contains("first item") && copied[0].contains("second item"),
        "ctrl+c copies the frame selection: {:?}",
        copied
    );
    assert_eq!(
        controller.chrome().exit_confirmation_label(),
        None,
        "ctrl+c with a frame selection must not arm the exit confirmation"
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
