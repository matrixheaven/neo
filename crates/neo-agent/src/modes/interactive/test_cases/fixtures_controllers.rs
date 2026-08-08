//! Interactive test fixtures: `InteractiveController` builders and follow-up/steer
//! assertions (moved from `mod.rs`).

use neo_agent_core::StopReason;
use neo_tui::transcript::TranscriptEntry;

use super::super::*;
use super::fixtures_config::*;
use super::fixtures_sessions::*;
use super::fixtures_transcript::*;

pub fn controller_with_pending_math_question() -> (
    InteractiveController,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let answers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_answers = std::sync::Arc::clone(&answers);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let captured_answers = std::sync::Arc::clone(&captured_answers);
        Box::pin(async move {
            let (response_tx, response_rx) = oneshot::channel();
            channels
                .questions
                .send(PendingQuestion {
                    id: "question-1".to_owned(),
                    questions: vec![neo_agent_core::QuestionEventData {
                        question: "1 + 1 = ?".to_owned(),
                        header: Some("Math".into()),
                        body: None,
                        options: vec![
                            neo_agent_core::QuestionOptionData {
                                label: "2".to_owned(),
                                description: Some("Correct".into()),
                            },
                            neo_agent_core::QuestionOptionData {
                                label: "3".to_owned(),
                                description: Some("Too high".into()),
                            },
                        ],
                        multi_select: false,
                    }],
                    response_tx,
                    workflow_origin: None,
                })
                .expect("question sent");
            let response = response_rx.await.expect("question response");
            captured_answers
                .lock()
                .expect("answers lock")
                .extend(response.answers);
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "answered".to_owned(),
            });
            channels.send_event(AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            });
            Ok(TurnOutcome::default())
        })
    });
    let controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );
    (controller, answers)
}

pub fn controller_with_keyboard_routing_question() -> InteractiveController {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("draft");
    let (response_tx, _response_rx) = oneshot::channel();
    controller.register_pending_question(PendingQuestion {
        id: "question-1".to_owned(),
        questions: vec![
            neo_agent_core::QuestionEventData {
                question: "2 + 2 = ?".to_owned(),
                header: Some("Single".into()),
                body: None,
                options: vec![
                    neo_agent_core::QuestionOptionData {
                        label: "3".to_owned(),
                        description: None,
                    },
                    neo_agent_core::QuestionOptionData {
                        label: "4".to_owned(),
                        description: None,
                    },
                ],
                multi_select: false,
            },
            neo_agent_core::QuestionEventData {
                question: "Pick primes".to_owned(),
                header: Some("Multi".into()),
                body: None,
                options: vec![
                    neo_agent_core::QuestionOptionData {
                        label: "2".to_owned(),
                        description: None,
                    },
                    neo_agent_core::QuestionOptionData {
                        label: "4".to_owned(),
                        description: None,
                    },
                ],
                multi_select: true,
            },
        ],
        response_tx,
        workflow_origin: None,
    });
    controller
}

pub fn session_picker_continuation_controller() -> (
    InteractiveController,
    std::sync::Arc<std::sync::Mutex<Vec<TurnRequest>>>,
) {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let controller = InteractiveController::new_with_event_driver(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        phase: neo_ai::MessagePhase::Unknown,
                        turn: 2,
                        id: "assistant-2".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 2,
                        text: "continued".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 2,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            }
        },
        PickerCatalogs {
            session_items: vec![test_session_summary(
                SESSION_A,
                "Alpha session",
                test_workspace_root(),
                "branch summary",
            )],
            session_error: None,
            model_items: Vec::new(),
        },
        |session_id| async move {
            assert_eq!(session_id, SESSION_A);
            Ok(LoadedSessionTranscript::new(
                SESSION_A,
                ["branch summary: Local branch summary".to_owned()],
                [
                    AgentMessage::user_text("hello"),
                    AgentMessage::assistant(
                        [Content::text("hi back")],
                        Vec::new(),
                        StopReason::EndTurn,
                    ),
                ],
            ))
        },
    );
    (controller, requests)
}

pub fn model_picker_submission_controller() -> (
    InteractiveController,
    std::sync::Arc<std::sync::Mutex<Vec<TurnRequest>>>,
) {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "new",
        "anthropic/claude-sonnet-4-5",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        phase: neo_ai::MessagePhase::Unknown,
                        turn: 1,
                        id: "assistant-1".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: "model switched".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            }
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![
                PickerItem::new("openai/gpt-4.1", "openai/gpt-4.1", Some("Responses")),
                PickerItem::new(
                    "anthropic/claude-sonnet-4-5",
                    "anthropic/claude-sonnet-4-5",
                    Some("Messages · ctx 200000"),
                ),
            ],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.local_config = Some(selected_model_local_config());
    (controller, requests)
}

pub fn controller_with_session_for_new_tests() -> (
    InteractiveController,
    std::sync::Arc<std::sync::Mutex<Vec<TurnRequest>>>,
) {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        phase: neo_ai::MessagePhase::Unknown,
                        turn: 1,
                        id: "assistant-1".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: "hi back".to_owned(),
                    },
                    AgentEvent::MessageFinished {
                        phase: neo_ai::MessagePhase::Unknown,
                        turn: 1,
                        id: "assistant-1".to_owned(),
                        stop_reason: StopReason::EndTurn,
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            }
        },
    );
    // Seed an active session id, transcript content, prompt text, and todos
    // so the reset tests can prove all of them are cleared.
    controller.active_session_id = Some(SESSION_A.to_owned());
    controller
        .tui
        .chrome_mut()
        .set_session_label(SESSION_A.to_owned());
    controller
        .transcript_mut()
        .push_user_message("continue the permission refactor");
    controller
        .transcript_mut()
        .push_assistant_message("I found the old policy conversion path...");
    controller
        .tui
        .chrome_mut()
        .set_todo_items(vec![neo_tui::widgets::TodoDisplayItem::new(
            "Step 1",
            neo_tui::widgets::TodoDisplayStatus::Pending,
        )]);
    (controller, requests)
}

pub async fn running_turn_controller() -> InteractiveController {
    let run_turn: TurnDriver = Arc::new(move |_request, _channels| {
        Box::pin(async move {
            // Never complete: holds the turn open for live-slash tests.
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
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(controller.active_turn.is_some(), "turn is running");
    controller
}

// --- NEO-23: cross-session prompt history -----------------------------

/// Build a test controller with a temp-backed prompt history store so tests
/// exercise the real load/append path without touching the user's home.
pub fn controller_with_history_store(
    store: crate::prompt::history::PromptHistoryStore,
) -> InteractiveController {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_prompt_history_store(store);
    controller.load_prompt_history();
    controller
}

pub async fn controller_with_closed_active_input() -> InteractiveController {
    let captured_steer = Arc::new(std::sync::Mutex::new(
        neo_agent_core::SteerInputHandle::new(),
    ));
    let observed_steer = Arc::clone(&captured_steer);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
        Box::pin(async move {
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );
    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt starts turn");
    assert!(
        captured_steer.lock().expect("steer lock").close_if_empty(),
        "test turn should close with no pending input"
    );
    controller
}

pub async fn controller_with_queued_follow_ups()
-> (InteractiveController, neo_agent_core::SteerInputHandle) {
    let captured_steer = Arc::new(std::sync::Mutex::new(
        neo_agent_core::SteerInputHandle::new(),
    ));
    let observed_steer = Arc::clone(&captured_steer);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_steer = Arc::clone(&observed_steer);
        *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
        Box::pin(async move {
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );
    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt starts turn");
    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_text("queued one"),
    });
    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_text("queued two"),
    });
    let steer_handle = captured_steer.lock().expect("steer lock").clone();
    (controller, steer_handle)
}

pub async fn assert_oldest_follow_up_promoted_before_composer(
    controller: &mut InteractiveController,
    steer_handle: &neo_agent_core::SteerInputHandle,
) {
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("first ctrl+s promotes oldest queued follow-up");

    assert_eq!(steer_handle.pending(), 1);
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued two"],
        "one Ctrl+S should promote only the oldest queued follow-up"
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one"]
    );
    assert_eq!(
        controller.chrome().prompt().text,
        "current steer",
        "composer text should wait until queued follow-ups have been promoted"
    );
}

pub async fn assert_remaining_follow_ups_promoted_before_composer(
    controller: &mut InteractiveController,
    steer_handle: &neo_agent_core::SteerInputHandle,
) {
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("second ctrl+s promotes second queued follow-up");
    assert_eq!(steer_handle.pending(), 2);
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one", "queued two"]
    );
    assert_eq!(controller.chrome().prompt().text, "current steer");

    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_text("queued D"),
    });
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("third ctrl+s promotes newly queued follow-up before composer");
    assert_eq!(steer_handle.pending(), 3);
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one", "queued two", "queued D"]
    );
    assert_eq!(controller.chrome().prompt().text, "current steer");
}

pub async fn assert_composer_promoted_after_follow_ups(
    controller: &mut InteractiveController,
    steer_handle: &neo_agent_core::SteerInputHandle,
) {
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("fourth ctrl+s steers current composer text");
    assert_eq!(steer_handle.pending(), 4);
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one", "queued two", "queued D", "current steer"]
    );
    assert_eq!(controller.chrome().prompt().text, "");
}

pub fn assert_steers_render_after_runtime_append(controller: &mut InteractiveController) {
    let steered_user_messages = transcript_entries(controller)
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::UserMessage { content, .. }
                if matches!(
                    content.as_str(),
                    "queued one" | "queued two" | "queued D" | "current steer"
                ) =>
            {
                Some(content.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        steered_user_messages,
        Vec::<&str>::new(),
        "promoted steers should not render in the transcript before MessageAppended"
    );

    for text in ["queued one", "queued two", "queued D", "current steer"] {
        controller.apply_turn_event(AgentEvent::MessageAppended {
            message: AgentMessage::user_text(text),
        });
    }
    let steered_user_messages = transcript_entries(controller)
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::UserMessage { content, .. }
                if matches!(
                    content.as_str(),
                    "queued one" | "queued two" | "queued D" | "current steer"
                ) =>
            {
                Some(content.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        steered_user_messages,
        vec!["queued one", "queued two", "queued D", "current steer"],
        "promoted steers should render in runtime append order"
    );
}

pub async fn assert_first_empty_follow_up_promotion(
    controller: &mut InteractiveController,
    steer_handle: &neo_agent_core::SteerInputHandle,
) {
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("empty ctrl+s promotes oldest queued follow-up");

    assert_eq!(
        steer_handle.pending(),
        1,
        "one Ctrl+S should enqueue one promotion request"
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued two"],
        "only the oldest follow-up should leave the visible follow-up queue"
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one"],
        "promoted follow-up should appear as a pending steer immediately"
    );

    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::FollowUp,
        count: 1,
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued two"],
        "runtime follow-up drain ack must not affect the next visible queued follow-up"
    );
    controller.apply_turn_event(AgentEvent::SteeringQueued {
        message: AgentMessage::user_text("queued one"),
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one"],
        "runtime steer ack must not duplicate the promoted preview"
    );
}

pub async fn assert_second_empty_follow_up_promotion(
    controller: &mut InteractiveController,
    steer_handle: &neo_agent_core::SteerInputHandle,
) {
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("second empty ctrl+s promotes next queued follow-up");
    assert_eq!(steer_handle.pending(), 2);
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one", "queued two"]
    );

    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::FollowUp,
        count: 1,
    });
    controller.apply_turn_event(AgentEvent::SteeringQueued {
        message: AgentMessage::user_text("queued two"),
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one", "queued two"],
        "runtime steer acks must not duplicate the promoted previews"
    );
    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::Steering,
        count: 2,
    });
    assert!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .is_empty(),
        "one runtime steer drain should clear the promoted preview"
    );
}

// ---------------------------------------------------------------------------
// Theme manager controller adapter (`/theme`, palette, runtime overrides)
// ---------------------------------------------------------------------------

pub fn theme_controller_with_project(project_dir: &Path) -> InteractiveController {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(test_config(project_dir, project_dir.join(".neo/sessions")));
    controller
}

pub fn busy_turn_controller(project_dir: &Path) -> InteractiveController {
    let run_turn: TurnDriver = Arc::new(|_request, channels| {
        Box::pin(async move {
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        project_dir.to_path_buf(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );
    controller.local_config = Some(test_config(project_dir, project_dir.join(".neo/sessions")));
    controller
}
