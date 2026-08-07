//! Follow-up steer behavior (split from `input.rs`).

use neo_agent_core::{AgentEvent, AgentMessage, Content, StopReason};
use neo_tui::{
    input::{InputEvent, KeyId},
    transcript::TranscriptEntry,
};

use super::super::*;
use super::*;

#[tokio::test]
async fn active_turn_enter_enqueues_follow_up_instead_of_rejecting() {
    let captured_steer = Arc::new(std::sync::Mutex::new(
        neo_agent_core::SteerInputHandle::new(),
    ));
    let observed_steer = Arc::clone(&captured_steer);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_steer = Arc::clone(&observed_steer);
        *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
        Box::pin(async move {
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "working".to_owned(),
            });
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
    assert!(controller.active_turn.is_some(), "turn should be active");

    // While the turn is running, typing + Enter must enqueue (not reject).
    controller.type_text("queued follow up");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter while busy enqueues");

    let steer_handle = captured_steer.lock().expect("steer lock").clone();
    assert_eq!(
        steer_handle.pending(),
        1,
        "follow-up should be pushed into the steer input handle"
    );
    // Composer should be cleared after queuing.
    assert_eq!(controller.chrome().prompt().text, "");
    assert!(
        controller.active_turn.is_some(),
        "turn must still be running after enqueue"
    );
}

#[tokio::test]
async fn enter_after_live_input_closes_becomes_next_follow_up() {
    let mut controller = controller_with_closed_active_input().await;
    controller.type_text("next prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("closed turn should preserve follow-up");

    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["next prompt"]
    );
    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn steer_after_live_input_closes_becomes_next_follow_up() {
    let mut controller = controller_with_closed_active_input().await;
    controller.type_text("late steer");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("closed turn should preserve steer");

    assert!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .is_empty()
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["late steer"]
    );
    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn active_turn_enter_updates_pending_preview_immediately() {
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

    controller.type_text("queued follow up");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter while busy enqueues");

    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued follow up"],
        "queued follow-up should appear above the composer immediately"
    );

    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_text("queued follow up"),
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued follow up"],
        "runtime queue ack must not duplicate the local preview"
    );
    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::FollowUp,
        count: 1,
    });
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty(),
        "one runtime drain should clear one queued preview item"
    );
}

#[tokio::test]
async fn idle_submit_renders_user_prompt_immediately_without_duplicate_runtime_append() {
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn: Arc::new(|_request, channels| {
                Box::pin(async move {
                    channels.cancel_token.cancelled().await;
                    Ok(TurnOutcome::default())
                })
            }),
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );

    controller.type_text("wait for runtime append");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit starts turn");

    let matching_entries = transcript_entries(&controller)
        .iter()
        .filter(|entry| {
            matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "wait for runtime append")
        })
        .count();
    assert_eq!(
        matching_entries, 1,
        "normal submits should render the user prompt immediately"
    );

    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_text("wait for runtime append"),
    });

    let matching_entries = transcript_entries(&controller)
        .iter()
        .filter(|entry| {
            matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "wait for runtime append")
        })
        .count();
    assert_eq!(
        matching_entries, 1,
        "runtime append should render the user prompt exactly once"
    );

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn active_turn_ctrl_s_steers_running_turn() {
    let captured_steer = Arc::new(std::sync::Mutex::new(
        neo_agent_core::SteerInputHandle::new(),
    ));
    let observed_steer = Arc::clone(&captured_steer);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_steer = Arc::clone(&observed_steer);
        *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
        Box::pin(async move {
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "working".to_owned(),
            });
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
    assert!(controller.active_turn.is_some());

    // Ctrl+S while busy should steer the running turn.
    controller.type_text("steer this");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("ctrl+s steers");

    let steer_handle = captured_steer.lock().expect("steer lock").clone();
    assert_eq!(steer_handle.pending(), 1, "steer should be pushed");
    // Composer cleared after steering.
    assert_eq!(controller.chrome().prompt().text, "");
}

#[tokio::test]
async fn active_turn_ctrl_s_updates_pending_preview_before_transcript_append() {
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

    controller.type_text("steer this");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("ctrl+s steers");

    assert!(
        !transcript_entries(&controller).iter().any(
            |entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "steer this")
        ),
        "Ctrl+S should wait for MessageAppended before rendering the steered user prompt"
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["steer this"],
        "steer should appear above the composer immediately"
    );
    controller.apply_turn_event(AgentEvent::SteeringQueued {
        message: AgentMessage::user_text("steer this"),
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["steer this"],
        "runtime steer ack must not duplicate the local preview"
    );
    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::Steering,
        count: 1,
    });
    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_text("steer this"),
    });
    assert!(
        transcript_entries(&controller).iter().any(
            |entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "steer this")
        ),
        "steered user prompt should render when the runtime appends it"
    );
}

#[tokio::test]
async fn steer_preview_is_cleared_when_turn_is_cancelled() {
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn: Arc::new(|_request, channels| {
                Box::pin(async move {
                    channels.cancel_token.cancelled().await;
                    Ok(TurnOutcome::default())
                })
            }),
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );

    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt starts turn");

    controller.type_text("steer this");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("ctrl+s steers");

    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["steer this"],
        "steer should be visible before cancellation"
    );

    controller
        .cancel_active_turn()
        .await
        .expect("cancel active turn");

    assert!(
        controller.chrome().pending_input().is_empty(),
        "pending input should be cleared after the turn is interrupted"
    );
}

#[tokio::test]
async fn active_turn_ctrl_s_promotes_one_follow_up_per_press_before_current_prompt() {
    let (mut controller, steer_handle) = controller_with_queued_follow_ups().await;
    controller.type_text("current steer");
    assert_oldest_follow_up_promoted_before_composer(&mut controller, &steer_handle).await;
    assert_remaining_follow_ups_promoted_before_composer(&mut controller, &steer_handle).await;
    assert_composer_promoted_after_follow_ups(&mut controller, &steer_handle).await;
    assert_steers_render_after_runtime_append(&mut controller);
}

#[tokio::test]
async fn empty_ctrl_s_promotes_one_follow_up_per_press_without_local_duplication() {
    let (mut controller, steer_handle) = controller_with_queued_follow_ups().await;
    assert_first_empty_follow_up_promotion(&mut controller, &steer_handle).await;
    assert_second_empty_follow_up_promotion(&mut controller, &steer_handle).await;
}

#[tokio::test]
async fn alt_up_dequeues_oldest_follow_up_into_multiline_composer() {
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
    for text in ["AAAA", "BBBB", "CCCC"] {
        controller.apply_turn_event(AgentEvent::FollowUpQueued {
            message: AgentMessage::user_text(text),
        });
    }

    let steer_handle = captured_steer.lock().expect("steer lock").clone();
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("alt+up").expect("valid key")))
        .await
        .expect("first alt+up dequeues oldest queued follow-up");
    assert_eq!(steer_handle.pending(), 1);
    assert_eq!(controller.chrome().prompt().text, "AAAA");
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["BBBB", "CCCC"]
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("alt+up").expect("valid key")))
        .await
        .expect("second alt+up appends next queued follow-up");
    assert_eq!(steer_handle.pending(), 2);
    assert_eq!(controller.chrome().prompt().text, "AAAA\nBBBB");
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["CCCC"]
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("alt+up").expect("valid key")))
        .await
        .expect("third alt+up appends final queued follow-up");
    assert_eq!(steer_handle.pending(), 3);
    assert_eq!(controller.chrome().prompt().text, "AAAA\nBBBB\nCCCC");
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
    );
}

#[tokio::test]
async fn empty_ctrl_s_with_no_queue_reports_noop_status() {
    let mut controller = running_turn_controller().await;

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("empty ctrl+s with no queue is handled");

    assert!(
        transcript_has_status(&controller, "No queued follow-up to steer"),
        "empty Ctrl+S with no queue should be visible feedback"
    );

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn idle_ctrl_s_falls_back_to_normal_submit() {
    let prompt_seen = Arc::new(std::sync::Mutex::new(None));
    let observed_prompt = Arc::clone(&prompt_seen);
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let observed_prompt = Arc::clone(&observed_prompt);
        Box::pin(async move {
            *observed_prompt.lock().expect("prompt lock") = Some(request.prompt.clone());
            channels.send_event(AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            });
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

    controller.type_text("submit via ctrl+s");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("ctrl+s submits when idle");
    controller
        .wait_for_active_turn()
        .await
        .expect("idle ctrl+s turn completes");

    let seen = prompt_seen.lock().expect("prompt lock").clone();
    assert_eq!(
        seen,
        Some(vec![Content::text("submit via ctrl+s")]),
        "idle Ctrl+S should behave like a normal submit"
    );
}
