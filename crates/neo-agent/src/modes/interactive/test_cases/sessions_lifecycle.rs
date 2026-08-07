//! Session new/fork/clear lifecycle behavior (split from `sessions.rs`).

use std::path::PathBuf;

use neo_agent_core::{AgentEvent, AgentMessage, Content, PermissionMode, StopReason};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    screen_output::FullscreenTerminal,
    shell::ChromeMode,
};

use super::super::*;
use super::*;

#[tokio::test]
async fn slash_new_resets_to_unsaved_fresh_session_without_streaming() {
    let (mut controller, _requests) = controller_with_session_for_new_tests();
    controller.tui.chrome_mut().set_context_window(Some(
        ContextWindow::new(1_000_000)
            .with_used_tokens(57_000)
            .with_projected_tokens(Some(61_000)),
    ));

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    assert_eq!(controller.active_session_id(), None);
    assert_eq!(controller.chrome().session_label(), "new");
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("Welcome to neo!"),
        "snapshot shows welcome banner"
    );
    assert!(
        snapshot.contains("Started fresh session"),
        "snapshot shows fresh session status"
    );
    assert!(
        !snapshot.contains("permission refactor"),
        "old transcript content is gone"
    );
    assert!(
        !snapshot.contains("policy conversion"),
        "old assistant content is gone"
    );
    assert!(controller.chrome().prompt().text.is_empty());
    assert!(controller.chrome().todo_items().is_empty());
    assert_eq!(
        controller.chrome().context_window(),
        Some(ContextWindow::new(1_000_000))
    );
}

#[tokio::test]
async fn slash_new_parks_workflow_approval_until_origin_session_reactivated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    controller.set_active_session_id(SESSION_A.to_owned());
    let (pending, mut response_rx) = make_pending_approval(ordinary_shell_request(
        "workflow-before-new",
        "sudo --version",
        None,
        None,
    ));
    controller
        .workflow_approval_ingress
        .send(SessionWorkflowApproval {
            session_id: SESSION_A.to_owned(),
            pending,
        })
        .expect("workflow approval delivery");
    assert_eq!(
        controller.drain_workflow_approvals(),
        FrameRequest::Immediate
    );
    assert!(controller.chrome().approval_is_pending());

    controller.start_new_session_from_slash();

    assert_eq!(controller.active_session_id(), None);
    assert!(controller.pending_approvals.is_empty());
    assert!(!controller.chrome().approval_is_pending());
    assert_eq!(controller.workflow_approval_backlog[SESSION_A].len(), 1);
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    controller.set_active_session_id(SESSION_A.to_owned());

    assert!(
        controller
            .pending_approvals
            .contains_key("workflow-before-new")
    );
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|selection| selection.0),
        Some("workflow-before-new")
    );
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn slash_clear_alias_resets_to_unsaved_fresh_session() {
    let (mut controller, _requests) = controller_with_session_for_new_tests();

    controller.type_text("/clear");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/clear submits");

    assert_eq!(controller.active_session_id(), None);
    assert_eq!(controller.chrome().session_label(), "new");
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("Started fresh session"));
    assert!(!snapshot.contains("permission refactor"));
}

#[tokio::test]
async fn slash_clear_does_not_request_terminal_scrollback_purge() {
    let (mut controller, _requests) = controller_with_session_for_new_tests();
    let mut terminal = FullscreenTerminal::for_test(80, 24);

    let before_clear = controller.tui.render_terminal_frame(80, 24);
    terminal
        .render_to(&mut Vec::new(), &before_clear)
        .expect("render initial terminal frame");

    controller.type_text("/clear");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/clear submits");

    let after_clear = controller.tui.render_terminal_frame(80, 24);
    let mut output = Vec::new();
    terminal
        .render_to(&mut output, &after_clear)
        .expect("render cleared terminal frame");
    let output = String::from_utf8(output).expect("terminal output is UTF-8");

    assert!(!output.contains("\x1b[2J"));
    assert!(!output.contains("\x1b[3J"));
}

#[tokio::test]
async fn slash_new_does_not_enter_streaming_mode() {
    let (mut controller, requests) = controller_with_session_for_new_tests();

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    assert!(requests.lock().expect("recorded requests").is_empty());
}

#[tokio::test]
async fn slash_new_preserves_model_permission_reasoning_and_plan_mode() {
    let (mut controller, _requests) = controller_with_session_for_new_tests();
    // Configure preserved state.
    controller.set_permission_mode(PermissionMode::Yolo);
    controller.set_current_reasoning(neo_ai::ReasoningSelection::On);
    controller.set_plan_mode_from_user(true);

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Yolo);
    assert_eq!(
        controller.current_reasoning,
        neo_ai::ReasoningSelection::On,
        "structured reasoning selection is preserved across /new"
    );
    assert_eq!(
        controller.chrome().reasoning_label(),
        Some("on".to_owned()),
        "reasoning indicator is preserved across /new"
    );
    assert_eq!(controller.chrome().model_label(), "openai/gpt-4.1");
    assert!(
        controller.chrome().is_plan_mode(),
        "user-enabled plan mode is preserved across /new"
    );
}

#[tokio::test]
async fn slash_new_clears_transcript_todos_prompt_and_pending_overlays() {
    let (mut controller, _requests) = controller_with_session_for_new_tests();

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("Welcome to neo!"));
    assert!(
        !snapshot.contains("permission refactor"),
        "old transcript content is cleared"
    );
    assert!(controller.chrome().prompt().text.is_empty());
    assert!(controller.chrome().todo_items().is_empty());
    assert!(controller.active_session_id().is_none());
}

#[tokio::test]
async fn slash_new_preserves_loaded_prompt_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    store.append(Some(SESSION_A), "remembered prompt").unwrap();
    let mut controller = controller_with_history_store(store);
    controller.active_session_id = Some(SESSION_A.to_owned());
    controller
        .tui
        .chrome_mut()
        .set_session_label(SESSION_A.to_owned());

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls history after /new");
    assert_eq!(controller.chrome().prompt().text, "remembered prompt");
}

#[tokio::test]
async fn slash_new_is_blocked_while_turn_is_running_and_preserves_prompt() {
    // Use a driver that blocks forever until cancelled, so the turn stays
    // active while we submit /new.
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let run_turn: TurnDriver = Arc::new(move |request, _channels| {
        let captured_requests = std::sync::Arc::clone(&captured_requests);
        Box::pin(async move {
            captured_requests
                .lock()
                .expect("record request")
                .push(request);
            // Never complete: holds the turn open.
            std::future::pending::<Result<TurnOutcome>>().await
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );
    controller.active_session_id = Some(SESSION_A.to_owned());

    controller.type_text("long running");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt submits");
    // Let the turn task spawn and register itself.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(controller.active_turn.is_some(), "turn is running");

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submit handles blocking");

    assert_eq!(
        controller.active_session_id(),
        Some(SESSION_A),
        "active session id is unchanged when blocked"
    );
    assert!(
        transcript_has_status(
            &controller,
            "Cannot start a new session while a turn is running"
        ),
        "blocked status is shown"
    );
    assert_eq!(
        controller.chrome().prompt().text,
        "/new",
        "blocked /new preserves the command text for retry"
    );

    // Clean up the dangling turn.
    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn slash_new_preserves_old_session_for_resume_picker_and_next_prompt_creates_new_session() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let captured_requests = std::sync::Arc::clone(&captured_requests);
        Box::pin(async move {
            let is_first = {
                let mut requests = captured_requests.lock().expect("record request");
                let is_first = requests.is_empty();
                requests.push(request);
                is_first
            };
            if is_first {
                // First prompt after /new should carry session_id = None and
                // report a brand-new session id.
                channels
                    .session_ids
                    .send(SESSION_NEW.to_owned())
                    .expect("session id sent");
            }
            channels.send_event(AgentEvent::MessageStarted {
                phase: neo_ai::MessagePhase::Unknown,
                turn: 1,
                id: "assistant-1".to_owned(),
            });
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "ok".to_owned(),
            });
            channels.send_event(AgentEvent::MessageFinished {
                phase: neo_ai::MessagePhase::Unknown,
                turn: 1,
                id: "assistant-1".to_owned(),
                stop_reason: StopReason::EndTurn,
            });
            channels.send_event(AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            });
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs {
            session_items: vec![test_session_summary(
                SESSION_A,
                "Alpha",
                test_workspace_root(),
                "permission refactor",
            )],
            session_error: None,
            model_items: Vec::new(),
        },
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );
    controller.active_session_id = Some(SESSION_A.to_owned());

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    assert_eq!(controller.active_session_id(), None);
    assert_eq!(controller.chrome().session_label(), "new");
    // The old session is still advertised in the picker catalog.
    assert!(
        controller
            .session_items
            .iter()
            .any(|item| item.id == SESSION_A),
        "old session remains in the picker catalog"
    );

    // The next real prompt should carry session_id = None so the runtime
    // creates a brand-new JSONL session.
    controller.type_text("hello new session");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("next prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("next turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("hello new session")],
        "next prompt text is forwarded"
    );
    assert_eq!(
        requests[0].session_id, None,
        "next prompt carries no session id so a new session is created"
    );
    assert_eq!(
        controller.chrome().session_label(),
        SESSION_NEW,
        "new session id becomes active"
    );
    assert_eq!(controller.active_session_id(), Some(SESSION_NEW));
}

#[tokio::test]
async fn slash_fork_forks_current_session_and_enters_child() {
    let mut controller = InteractiveController::new_with_event_driver_and_forker(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: Vec::new(),
        },
        EventDriverCallbacks {
            run_turn: move |_request| async move {
                Ok(vec![AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                }])
            },
            load_session: |_session_id| async move {
                panic!("fork should not use the load_session callback");
                #[allow(unreachable_code)]
                Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
            },
            fork_session: |parent_id| async move {
                assert_eq!(parent_id, SESSION_A);
                Ok(ForkedSessionTranscript::new(
                    SESSION_CHILD,
                    LoadedSessionTranscript::new(
                        SESSION_CHILD,
                        [],
                        [AgentMessage::user_text("hello")],
                    ),
                ))
            },
        },
    );
    controller.active_session_id = Some(SESSION_A.to_owned());

    let consumed = controller.handle_slash_command("/fork").await;
    assert!(consumed, "/fork should be consumed as a slash command");

    assert_eq!(
        controller.active_session_id(),
        Some(SESSION_CHILD),
        "active session switched to fork child"
    );
    assert_eq!(controller.chrome().session_label(), SESSION_CHILD);
    assert!(
        transcript_has_status(&controller, &format!("fork from session {SESSION_A}")),
        "transcript shows fork-from notice"
    );
    assert!(
        transcript_has_status(
            &controller,
            &format!("switch to fork session {SESSION_CHILD}")
        ),
        "transcript shows switch-to notice"
    );
}

#[tokio::test]
async fn ctrl_n_forks_current_session_and_enters_child() {
    let mut controller = InteractiveController::new_with_event_driver_and_forker(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: Vec::new(),
        },
        EventDriverCallbacks {
            run_turn: move |_request| async move {
                Ok(vec![AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                }])
            },
            load_session: |_session_id| async move {
                panic!("fork should not use the load_session callback");
                #[allow(unreachable_code)]
                Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
            },
            fork_session: |parent_id| async move {
                assert_eq!(parent_id, SESSION_A);
                Ok(ForkedSessionTranscript::new(
                    SESSION_CHILD,
                    LoadedSessionTranscript::new(
                        SESSION_CHILD,
                        [],
                        [AgentMessage::user_text("hello")],
                    ),
                ))
            },
        },
    );
    controller.active_session_id = Some(SESSION_A.to_owned());

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+n").expect("valid key")))
        .await
        .expect("ctrl+n forks current session");

    assert_eq!(
        controller.active_session_id(),
        Some(SESSION_CHILD),
        "active session switched to fork child"
    );
    assert_eq!(controller.chrome().session_label(), SESSION_CHILD);
    assert!(
        transcript_has_status(&controller, &format!("fork from session {SESSION_A}")),
        "transcript shows fork-from notice"
    );
    assert!(
        transcript_has_status(
            &controller,
            &format!("switch to fork session {SESSION_CHILD}")
        ),
        "transcript shows switch-to notice"
    );
}
