//! Interactive input behavior (moved from `tests.rs`).

use neo_agent_core::{AgentEvent, Content, MessageOrigin, PendingQuestion, StopReason};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    shell::ChromeMode,
    transcript::{MouseKind, TranscriptEntry},
};
use tokio::sync::oneshot;

use super::super::*;
use super::*;

#[tokio::test]
async fn background_question_answer_starts_followup_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("workspace");
    std::fs::create_dir_all(&project_dir).expect("workspace");
    let config = test_config(&project_dir, temp.path().join("sessions"));
    let session_dir = workspace_sessions_dir(&config).join(SESSION_A);
    assert!(config.workflow_runtime.notification_queue().enqueue(
        neo_agent_core::WorkflowNotification::new(
            &session_dir,
            neo_agent_core::workflow::WorkflowId("wf_background_question".to_owned()),
            neo_agent_core::workflow::WorkflowState::Completed,
            "worker completed",
            "wf_background_question",
            "background question test",
        ),
    ));
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "session",
        "model",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests.lock().expect("requests").push(request);
                Ok(Vec::new())
            }
        },
    );
    controller.local_config = Some(config.clone());
    controller.set_active_session_id(SESSION_A.to_owned());
    let (response_tx, mut response_rx) = oneshot::channel();
    controller.register_pending_question(PendingQuestion {
        id: "question-1".to_owned(),
        questions: vec![neo_agent_core::QuestionEventData {
            question: "Pick a side?".to_owned(),
            header: Some("Choice".into()),
            body: None,
            options: vec![
                neo_agent_core::QuestionOptionData {
                    label: "Left".to_owned(),
                    description: None,
                },
                neo_agent_core::QuestionOptionData {
                    label: "Right".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        response_tx,
        workflow_origin: None,
    });

    controller
        .resolve_question("question-1", vec!["Left".to_owned()])
        .await
        .expect("question resolves");
    controller
        .wait_for_active_turn()
        .await
        .expect("followup completes");

    assert_eq!(
        response_rx
            .try_recv()
            .expect("response should be sent")
            .answers,
        vec!["Left"]
    );
    let requests = requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].session_id.as_deref(), Some(SESSION_A));
    assert_eq!(
        requests[0].prompt_origin,
        MessageOrigin::injection("background_question")
    );
    assert!(
        requests[0].prompt[0]
            .as_text()
            .unwrap()
            .contains("Background question `question-1`")
    );
    assert!(
        requests[0].prompt[0]
            .as_text()
            .unwrap()
            .contains("TaskOutput")
    );
    assert_eq!(
        config
            .workflow_runtime
            .notification_queue()
            .pending_for_session(&session_dir)
            .len(),
        1,
        "background-question continuation must not consume workflow notifications"
    );
}

#[tokio::test]
async fn active_turn_event_drain_leaves_backlog_for_input_fairness() {
    const BACKLOG: usize = 513;

    let run_turn: TurnDriver = Arc::new(|_request, channels| {
        Box::pin(async move {
            for _ in 0..BACKLOG {
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "x".to_owned(),
                });
            }
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

    controller.type_text("stream");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start turn");
    tokio::task::yield_now().await;
    controller
        .drain_active_turn()
        .await
        .expect("drain streaming events");

    let received = transcript_entries(&controller)
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::AssistantMessage { content } => Some(content.len()),
            _ => None,
        })
        .expect("streaming assistant entry");
    assert_eq!(
        received,
        super::turn::MAX_TURN_EVENTS_PER_TICK,
        "one tick must leave a bounded backlog so terminal input gets another poll"
    );

    controller
        .cancel_active_turn()
        .await
        .expect("cancel streaming turn");
}

#[tokio::test]
async fn image_prompt_submit_renders_user_transcript_with_attachment() {
    let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02\x00\x00\x00\x90wS\xde"
        .to_vec();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |request| async move {
            assert_eq!(request.prompt.len(), 2);
            assert_eq!(request.prompt[0], Content::text("look "));
            assert!(matches!(request.prompt[1], Content::Image { .. }));
            Ok(vec![AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            }])
        },
    );
    controller.image_attachment_store.add(
        "sha256".to_owned(),
        "image/png".to_owned(),
        1,
        1,
        Some(png),
    );

    controller.type_text("look [image #1 (1x1)]");
    controller.submit_prompt().await.expect("prompt succeeds");

    let image_entry = transcript_entries(&controller)
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::UserMessage { content, images }
                if content == "look [image #1 (1x1)]" =>
            {
                Some(images)
            }
            _ => None,
        })
        .expect("user transcript entry with image placeholder");
    assert_eq!(image_entry.len(), 1);
    assert_eq!(image_entry[0].mime_type, "image/png");
    assert_eq!(image_entry[0].placeholder, "[image #1 (1x1)]");
    assert!(!image_entry[0].payload.is_empty());
}

#[tokio::test]
async fn idle_terminal_polling_does_not_render_repeated_frames() {
    struct IdleThenInterruptEvents {
        remaining_idle_polls: usize,
    }

    impl TerminalEvents for IdleThenInterruptEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            Ok(InputEvent::Interrupt)
        }

        fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
            if self.remaining_idle_polls == 0 {
                return Ok(Some(InputEvent::Interrupt));
            }

            self.remaining_idle_polls -= 1;
            std::thread::sleep(timeout);
            Ok(None)
        }
    }

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    assert!(
        !controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("first interrupt requests confirmation")
    );

    let mut render_count = 0;
    controller
        .run_terminal_loop_with_suspend(
            |tui, _| {
                let _ = tui.render_frame(80, 24);
                render_count += 1;
                Ok(None)
            },
            || Ok(()),
            |_| Ok(()),
            IdleThenInterruptEvents {
                remaining_idle_polls: 3,
            },
        )
        .await
        .expect("event loop exits after idle polls");

    assert_eq!(
        render_count, 1,
        "idle timeout polls must not request frames"
    );
}

#[tokio::test]
async fn animation_deadline_requests_one_follow_up_frame_without_input() {
    struct IdleThenInterruptEvents {
        remaining_idle_polls: usize,
    }

    impl TerminalEvents for IdleThenInterruptEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            Ok(InputEvent::Interrupt)
        }

        fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
            if self.remaining_idle_polls == 0 {
                return Ok(Some(InputEvent::Interrupt));
            }
            self.remaining_idle_polls -= 1;
            std::thread::sleep(timeout);
            Ok(None)
        }
    }

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
        .set_custom_working_label(Some("testing animation".to_owned()));
    assert!(
        !controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("first interrupt requests confirmation")
    );

    let mut render_count = 0;
    controller
        .run_terminal_loop_with_suspend(
            |tui, animation_due| {
                render_count += 1;
                if animation_due {
                    tui.advance_animation_at(Instant::now());
                }
                let deadline = tui
                    .chrome()
                    .working_label()
                    .map(|_| Instant::now() + Duration::from_millis(1));
                Ok(deadline)
            },
            || Ok(()),
            |_| Ok(()),
            IdleThenInterruptEvents {
                remaining_idle_polls: 1,
            },
        )
        .await
        .expect("event loop exits after deadline frame");

    assert_eq!(render_count, 2, "one startup and one deadline frame");
}

#[tokio::test]
async fn cleared_animation_deadline_does_not_render_again_while_idle() {
    struct IdleThenInterruptEvents {
        remaining_idle_polls: usize,
    }

    impl TerminalEvents for IdleThenInterruptEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            Ok(InputEvent::Interrupt)
        }

        fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
            if self.remaining_idle_polls == 0 {
                return Ok(Some(InputEvent::Interrupt));
            }
            self.remaining_idle_polls -= 1;
            std::thread::sleep(timeout);
            Ok(None)
        }
    }

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
        .set_custom_working_label(Some("testing animation".to_owned()));
    assert!(
        !controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("first interrupt requests confirmation")
    );

    let mut render_count = 0;
    let mut animation_render_count = 0;
    controller
        .run_terminal_loop_with_suspend(
            |tui, animation_due| {
                render_count += 1;
                if animation_due {
                    animation_render_count += 1;
                    tui.chrome_mut().set_custom_working_label(None);
                    tui.advance_animation_at(Instant::now());
                }
                let frame = tui.render_terminal_frame_at(80, 24, Instant::now());
                Ok(frame.next_animation_deadline)
            },
            || Ok(()),
            |_| Ok(()),
            IdleThenInterruptEvents {
                remaining_idle_polls: 3,
            },
        )
        .await
        .expect("event loop exits after idle polls");

    assert_eq!(
        animation_render_count, 1,
        "only one deadline frame advances animation"
    );
    assert!(render_count >= 2);
}

#[tokio::test]
async fn empty_async_polls_report_no_visible_change() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    assert!(!controller.poll_pending_catalog_fetch().await);
    assert!(!controller.poll_pending_custom_endpoint_fetch().await);
    assert!(!controller.poll_pending_custom_endpoint_test().await);
    assert!(!controller.poll_pending_mcp_probe().await);
}

#[test]
fn blocking_dialog_events_request_an_immediate_frame() {
    let question = AgentEvent::QuestionRequested {
        turn: 1,
        id: "question-1".to_owned(),
        questions: Vec::new(),
        workflow_origin: None,
    };
    let text = AgentEvent::TextDelta {
        turn: 1,
        text: "delta".to_owned(),
    };

    assert_eq!(
        InteractiveController::frame_request_for_agent_event(&question),
        FrameRequest::Immediate
    );
    assert_eq!(
        InteractiveController::frame_request_for_agent_event(&text),
        FrameRequest::Coalesced
    );
}

#[tokio::test]
async fn event_loop_types_submits_renders_and_exits_without_a_real_terminal() {
    struct FakeEvents {
        events: std::vec::IntoIter<InputEvent>,
    }

    impl TerminalEvents for FakeEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.events
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected test event"))
        }
    }

    let mut rendered = Vec::new();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |request| async move {
            assert_eq!(request.prompt, vec![Content::text("hi")]);
            assert_eq!(request.session_id, None);
            assert_eq!(request.model, None);
            Ok(vec![
                AgentEvent::MessageStarted {
                    phase: neo_ai::MessagePhase::Unknown,
                    turn: 1,
                    id: "assistant-1".to_owned(),
                },
                AgentEvent::TextDelta {
                    turn: 1,
                    text: "hello from controller".to_owned(),
                },
                AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                },
            ])
        },
    );

    controller
        .run_terminal_loop_with_suspend(
            |tui, _| {
                rendered.push(render_tui_snapshot(tui));
                Ok(None)
            },
            || Ok(()),
            |_| Ok(()),
            FakeEvents {
                events: vec![
                    InputEvent::Insert('h'),
                    InputEvent::Insert('i'),
                    InputEvent::Submit,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]
                .into_iter(),
            },
        )
        .await
        .expect("event loop succeeds");

    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    assert!(rendered.iter().any(|snapshot| snapshot.contains("> hi")));
    assert!(
        rendered
            .last()
            .expect("final render")
            .contains("hello from controller")
    );
}

#[tokio::test]
async fn event_loop_reports_turn_error_and_keeps_running() {
    use std::collections::VecDeque;

    struct ScriptedEvents {
        events: VecDeque<InputEvent>,
    }

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.events
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }
    }

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { anyhow::bail!("provider stream error: http status 400") },
    );
    let mut prompt_snapshots = Vec::new();

    controller.type_text("trigger error");
    controller
        .run_terminal_loop(
            |app| {
                prompt_snapshots.push(app.prompt().text.clone());
                Ok(())
            },
            ScriptedEvents {
                events: VecDeque::from([
                    InputEvent::Submit,
                    InputEvent::Insert('o'),
                    InputEvent::Insert('k'),
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]),
            },
        )
        .await
        .expect("turn error should not exit the interactive loop");

    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("Error: provider stream error: http status 400"));
    assert!(prompt_snapshots.iter().any(|prompt| prompt == "ok"));
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
}

#[tokio::test]
async fn event_loop_inserts_paste_newlines_without_submitting_until_enter() {
    struct FakeEvents {
        events: std::vec::IntoIter<InputEvent>,
    }

    impl TerminalEvents for FakeEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.events
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected test event"))
        }
    }

    let mut rendered = Vec::new();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |request| async move {
            assert_eq!(request.prompt, vec![Content::text("alpha\nbeta")]);
            Ok(vec![AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            }])
        },
    );

    controller
        .run_terminal_loop_with_suspend(
            |tui, _| {
                rendered.push(render_tui_snapshot(tui));
                Ok(None)
            },
            || Ok(()),
            |_| Ok(()),
            FakeEvents {
                events: vec![
                    InputEvent::Paste("alpha\nbeta".to_owned()),
                    InputEvent::Submit,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]
                .into_iter(),
            },
        )
        .await
        .expect("event loop succeeds");

    assert!(rendered.iter().any(|snapshot| snapshot.contains("alpha")));
    assert!(rendered.iter().any(|snapshot| snapshot.contains("beta")));
}

#[tokio::test]
async fn event_loop_renders_after_terminal_resize_without_submitting_prompt() {
    struct FakeEvents {
        events: std::vec::IntoIter<InputEvent>,
    }

    impl TerminalEvents for FakeEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.events
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected test event"))
        }
    }

    let mut rendered = Vec::new();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move {
            panic!("resize should not submit a turn");
            #[allow(unreachable_code)]
            Ok(Vec::<AgentEvent>::new())
        },
    );

    controller
        .run_terminal_loop_with_suspend(
            |tui, _| {
                rendered.push(render_tui_snapshot(tui));
                Ok(None)
            },
            || Ok(()),
            |_| Ok(()),
            FakeEvents {
                events: vec![
                    InputEvent::Insert('h'),
                    InputEvent::Resize {
                        columns: 100,
                        rows: 30,
                    },
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]
                .into_iter(),
            },
        )
        .await
        .expect("event loop succeeds");

    assert_eq!(rendered.len(), 4);
    assert!(rendered[1].contains("> h"));
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
}

#[tokio::test]
async fn event_loop_escape_cancels_active_turn() {
    use std::{collections::VecDeque, sync::Arc as StdArc};

    struct ScriptedEvents {
        events: VecDeque<Option<InputEvent>>,
    }

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.poll_input_event(Duration::from_millis(0))?
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }

        fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
            Ok(self
                .events
                .pop_front()
                .unwrap_or(Some(InputEvent::Interrupt)))
        }
    }

    let captured_token = StdArc::new(std::sync::Mutex::new(None));
    let observed_token = StdArc::clone(&captured_token);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_token = StdArc::clone(&observed_token);
        Box::pin(async move {
            *observed_token.lock().expect("token lock") = Some(channels.cancel_token.clone());
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "started".to_owned(),
            });
            channels.send_event(AgentEvent::ToolCallStarted {
                turn: 1,
                id: "pending-tool".to_owned(),
                name: "Read".to_owned(),
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

    controller.type_text("cancel me");
    controller
        .run_terminal_loop(
            |_app| Ok(()),
            ScriptedEvents {
                events: VecDeque::from([
                    Some(InputEvent::Submit),
                    None,
                    // Raw terminal input maps ESC through the active keybindings.
                    Some(InputEvent::Key(KeyId::new("escape").expect("valid key"))),
                    // After cancellation the app is idle; two Interrupts to exit
                    Some(InputEvent::Interrupt),
                    Some(InputEvent::Interrupt),
                ]),
            },
        )
        .await
        .expect("escape cancels turn and loop exits");

    let token = captured_token
        .lock()
        .expect("token lock")
        .clone()
        .expect("turn token captured");
    assert!(token.is_cancelled());
    assert!(controller.active_turn.is_none());
    assert!(transcript_entries(&controller).iter().any(|entry| matches!(
        entry,
        TranscriptEntry::ToolRun { component }
            if component.id() == "pending-tool"
                && component.status() == neo_tui::shell::ToolStatusKind::Cancelled
    )));
}

#[tokio::test]
async fn event_loop_escape_is_noop_when_idle() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("hello");

    // ESC when idle (no overlay, no active turn) should be a no-op
    let should_exit = controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("escape is no-op when idle");

    assert!(!should_exit, "ESC should not exit the app when idle");
    // Prompt text should be preserved (ESC is not clearing it)
    assert_eq!(controller.chrome().prompt().text, "hello");
}

#[tokio::test]
async fn event_loop_at_model_token_submits_as_plain_text() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![PickerItem::new(
                "anthropic/claude-sonnet",
                "anthropic/claude-sonnet",
                Some("Messages"),
            )],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("@anthropic/claude-sonnet explain this file");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("@anthropic/claude-sonnet explain this file")]
    );
    assert_eq!(requests[0].model, None);
}

#[tokio::test]
async fn event_loop_at_model_token_without_prompt_submits_as_plain_text() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![PickerItem::new(
                "anthropic/claude-sonnet",
                "anthropic/claude-sonnet",
                Some("Messages"),
            )],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("@anthropic/claude-sonnet");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits literal model token");
    controller
        .wait_for_active_turn()
        .await
        .expect("literal model token turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("@anthropic/claude-sonnet")]
    );
    assert_eq!(requests[0].model, None);
}

#[tokio::test]
async fn event_loop_dispatches_editor_scroll_actions_to_transcript_view() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    for index in 0..10 {
        controller
            .transcript_mut()
            .push_status(format!("line {index}"));
    }
    // Establish the viewport height through a bounded slice render.
    let _ = controller.transcript_mut().render_visible_slice(80, 2);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorPageUp))
        .await
        .expect("page up scrolls transcript");
    assert!(transcript_view_locked(&controller));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorDown))
        .await
        .expect("cursor down stays on the prompt");
    assert!(
        transcript_view_locked(&controller),
        "cursor down must not scroll the view"
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorPageDown))
        .await
        .expect("page down returns transcript to bottom");
    assert!(!transcript_view_locked(&controller));
}

#[tokio::test]
async fn event_loop_submit_restores_transcript_follow_tail() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    // Establish the viewport height through a bounded slice render.
    let _ = controller.transcript_mut().render_visible_slice(80, 6);

    controller
        .handle_input_event(wheel_event(MouseKind::ScrollUp))
        .await
        .expect("wheel up scrolls transcript");
    assert!(transcript_view_locked(&controller));
    assert!(!controller.transcript().document().view().following_tail);

    controller
        .handle_input_event(InputEvent::Insert('h'))
        .await
        .expect("typing works");
    controller
        .handle_input_event(InputEvent::Insert('i'))
        .await
        .expect("typing works");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit restores tail before sending");

    assert!(!transcript_view_locked(&controller));
    assert!(controller.transcript().document().view().following_tail);
}

#[test]
fn idle_workflow_events_do_not_set_the_foreground_working_footer() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_active_session_id(SESSION_A.to_owned());
    let generation = controller.workflow_event_generation;
    let (events, workflow_events) = tokio::sync::mpsc::unbounded_channel();
    controller.workflow_events = workflow_events;

    events
        .send(crate::modes::run::PersistedSessionWorkflowEvent::Event(
            Box::new(crate::modes::run::SessionWorkflowEvent {
                session_id: SESSION_A.to_owned(),
                generation,
                event: AgentEvent::ToolExecutionStarted {
                    turn: 1,
                    id: "background-workflow".to_owned(),
                    name: "Bash".to_owned(),
                    arguments: serde_json::json!({"command": "cargo --version"}),
                    workflow_origin: None,
                    output_ref: None,
                },
            }),
        ))
        .expect("workflow event");

    controller.drain_workflow_events();

    assert!(controller.chrome().working_label().is_none());
}

#[tokio::test]
async fn event_loop_keeps_unknown_at_prefix_as_prompt_text() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![PickerItem::new(
                "anthropic/claude-sonnet",
                "anthropic/claude-sonnet",
                Some("Messages"),
            )],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("@src/main.rs explain this file");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits with file mention");
    controller
        .wait_for_active_turn()
        .await
        .expect("file mention turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("@src/main.rs explain this file")]
    );
    assert_eq!(requests[0].model, None);
}

#[tokio::test]
#[ignore = "controller regression: pending question keeps input while later workflow events arrive"]
async fn pending_question_keeps_input_while_later_workflow_events_arrive() {
    use neo_agent_core::workflow::{WorkflowId, WorkflowSnapshot, WorkflowState};

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (response_tx, response_rx) = oneshot::channel();
    controller.register_pending_question(PendingQuestion {
        id: "question-1".to_owned(),
        questions: vec![neo_agent_core::QuestionEventData {
            question: "Which option?".to_owned(),
            header: Some("Choice".into()),
            body: None,
            options: vec![
                neo_agent_core::QuestionOptionData {
                    label: "Yes".to_owned(),
                    description: None,
                },
                neo_agent_core::QuestionOptionData {
                    label: "No".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        response_tx,
        workflow_origin: None,
    });

    // Later workflow events arrive while the question is pending.
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::WorkflowStarted {
            turn: 1,
            workflow: WorkflowSnapshot {
                id: WorkflowId("wf-later".to_owned()),
                title: "later workflow".to_owned(),
                state: WorkflowState::Running,
                current_phase: Some("work".to_owned()),
                projection_sequence: Some(1),
                recovery_failure: false,
                started_at_ms: Some(1_000),
                updated_at_ms: Some(2_000),
                invocation_count: 1,
                failure_count: 0,
                actual_usage: None,
                latest_log_summary: None,
                latest_report_summary: None,
                terminal_reason: None,
                display_name: "later workflow".to_owned(),
                purpose: "later".to_owned(),
            },
        });

    // The earliest blocking entry is still the question: keys reach its
    // state machine, and the workflow card cannot displace it.
    assert_eq!(
        controller.tui.transcript().earliest_blocking_entry(),
        Some(neo_tui::transcript::BlockingEntryKind::Question(
            "question-1".to_owned()
        ))
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("select moves within the question");
    assert_eq!(
        controller
            .chrome()
            .question_dialog_state()
            .map(|state| state.cursor),
        Some(1)
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("select answer");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("move to submit tab");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("submit question");
    assert_eq!(
        response_rx.await.expect("question response").answers,
        vec!["No".to_owned()]
    );
    // The later workflow card remains and commits once afterwards.
    assert!(
        controller.render_snapshot().contains("later workflow"),
        "workflow card must remain in the transcript"
    );
}
