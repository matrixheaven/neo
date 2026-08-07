//! Interactive transcript behavior (moved from `tests.rs`).

use neo_agent_core::{AgentEvent, AgentMessage, PendingQuestion};
use neo_tui::{input::InputEvent, transcript::TranscriptEntry};
use tokio::sync::oneshot;

use super::super::*;
use super::*;

#[tokio::test]
async fn completed_turn_drains_event_backlog_before_removal() {
    const BACKLOG: usize = 513;

    let run_turn: TurnDriver = Arc::new(|_request, channels| {
        Box::pin(async move {
            for _ in 0..BACKLOG {
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "x".to_owned(),
                });
            }
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

    controller.type_text("stream");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start turn");
    for _ in 0..20 {
        if controller
            .active_turn
            .as_ref()
            .is_some_and(|turn| turn.task.is_finished())
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    controller
        .drain_active_turn()
        .await
        .expect("drain completed turn");

    let received = transcript_entries(&controller)
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::AssistantMessage { content } => Some(content.len()),
            _ => None,
        })
        .expect("streaming assistant entry");
    assert_eq!(received, BACKLOG);
    assert!(controller.active_turn.is_none());
}

#[tokio::test]
async fn completed_turn_drains_pending_approval_and_question_channels() {
    let approval_receiver = Arc::new(std::sync::Mutex::new(None));
    let question_receiver = Arc::new(std::sync::Mutex::new(None));
    let approval_receiver_for_turn = Arc::clone(&approval_receiver);
    let question_receiver_for_turn = Arc::clone(&question_receiver);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let approval_receiver = Arc::clone(&approval_receiver_for_turn);
        let question_receiver = Arc::clone(&question_receiver_for_turn);
        Box::pin(async move {
            let request = ordinary_tool_request("terminal-approval", "Write", "done.txt", None);
            let (approval_tx, approval_rx) = oneshot::channel();
            channels
                .approvals
                .send(crate::modes::run::PendingApproval {
                    request,
                    response_tx: approval_tx,
                })
                .expect("approval sent");
            *approval_receiver.lock().expect("approval receiver") = Some(approval_rx);

            let (question_tx, question_rx) = oneshot::channel();
            channels
                .questions
                .send(PendingQuestion {
                    id: "terminal-question".to_owned(),
                    questions: vec![neo_agent_core::QuestionEventData {
                        question: "Continue?".to_owned(),
                        header: None,
                        body: None,
                        options: Vec::new(),
                        multi_select: false,
                    }],
                    response_tx: question_tx,
                    workflow_origin: None,
                })
                .expect("question sent");
            *question_receiver.lock().expect("question receiver") = Some(question_rx);
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

    controller.type_text("finish");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start turn");
    for _ in 0..20 {
        if controller
            .active_turn
            .as_ref()
            .is_some_and(|turn| turn.task.is_finished())
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    controller
        .drain_active_turn()
        .await
        .expect("drain completed turn");

    assert!(
        controller
            .pending_approvals
            .contains_key("terminal-approval")
    );
    assert!(
        controller
            .pending_questions
            .contains_key("terminal-question")
    );
    assert!(controller.active_turn.is_none());
}

#[tokio::test]
async fn queued_follow_up_message_appended_renders_user_transcript_entry() {
    let mut controller = running_turn_controller().await;

    controller.type_text("queued transcript content");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter while busy enqueues");

    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::FollowUp,
        count: 1,
    });
    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_text("queued transcript content"),
    });

    assert!(
        transcript_entries(&controller).iter().any(
            |entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "queued transcript content")
        ),
        "queued follow-up should be rendered as a user prompt when it is appended"
    );

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn appended_user_prompt_renders_single_transcript_entry() {
    let mut controller = running_turn_controller().await;

    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_text("long running"),
    });

    let matching_entries = transcript_entries(&controller)
        .iter()
        .filter(
            |entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "long running"),
        )
        .count();
    assert_eq!(
        matching_entries, 1,
        "runtime ack for a locally rendered prompt should not duplicate it"
    );

    controller.cancel_active_turn().await.expect("cancel turn");
}
