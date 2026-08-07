//! Interactive event-loop dispatch behavior (split from `input.rs`).

use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::PathBuf,
};

use neo_agent_core::{
    AgentEvent, AgentMessage, ApprovalAction, ApprovalResponse, Content, PendingQuestion,
    StopReason,
};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    shell::{ChromeMode, OverlayKind},
    transcript::{MouseKind, TranscriptEntry},
};
use tokio::sync::oneshot;

use super::super::*;
use super::*;
use crate::config::{ModelConfig, ProviderConfig};

#[tokio::test]
async fn event_loop_slash_resume_and_sessions_open_local_session_picker() {
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
            session_items: vec![test_session_summary(
                "alpha",
                "Alpha",
                test_workspace_root(),
                "root",
            )],
            session_error: None,
            model_items: Vec::new(),
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    for command in ["/resume", "/sessions"] {
        controller.type_text(command);
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("session picker command runs locally");

        assert!(matches!(
            controller
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::SessionPicker(_))
        ));
        assert!(controller.chrome().prompt().text.is_empty());
        assert!(requests.lock().expect("recorded requests").is_empty());
        let _ = controller.tui.chrome_mut().close_focused_overlay();
    }
}

#[test]
fn event_loop_slash_tree_absent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let completions = prompt_completions(temp.path(), "/", None, true).expect("slash completions");
    assert!(
        !completions.iter().any(|item| item.value == "/tree"),
        "/tree should not appear in slash completion items"
    );
}

#[tokio::test]
async fn event_loop_opens_command_palette_and_runs_local_model_command() {
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![PickerItem::new(
                "anthropic/claude-sonnet",
                "anthropic/claude-sonnet",
                Some("messages"),
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

    controller.local_config = Some(test_config_with_models(
        &test_workspace_root(),
        test_workspace_root().join(".neo/sessions"),
        BTreeMap::from([(
            "anthropic/claude-sonnet".to_owned(),
            ModelConfig {
                provider: "anthropic".to_owned(),
                model: "claude-sonnet".to_owned(),
                display_name: Some("messages".into()),
                ..ModelConfig::default()
            },
        )]),
    ));
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    let Some(OverlayKind::CommandPalette(palette)) = controller
        .chrome()
        .focused_overlay()
        .map(|overlay| &overlay.kind)
    else {
        panic!("expected command palette overlay");
    };
    assert_eq!(palette.selected_command().expect("command").id, "sessions");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("moves to model command");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("command runs");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::TabbedModelSelector(_))
    ));
}

#[tokio::test]
async fn event_loop_confirms_approval_choice_to_running_turn() {
    let responses = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_responses = std::sync::Arc::clone(&responses);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let captured_responses = std::sync::Arc::clone(&captured_responses);
        Box::pin(async move {
            let request = ordinary_tool_request("tool-1", "Write", "approved.txt", None);
            channels.send_event(AgentEvent::ApprovalRequested {
                request: request.clone(),
            });
            let (response_tx, response_rx) = oneshot::channel();
            channels
                .approvals
                .send(crate::modes::run::PendingApproval {
                    request,
                    response_tx,
                })
                .expect("approval waiter sent");
            let response = response_rx.await.expect("approval response");
            captured_responses
                .lock()
                .expect("responses lock")
                .push(response);
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "approved".to_owned(),
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

    controller.type_text("write file");
    controller
        .run_terminal_loop(
            |_app| Ok(()),
            OptionalScriptedEvents {
                events: VecDeque::from([
                    Some(InputEvent::Submit),
                    None,
                    Some(InputEvent::Action(KeybindingAction::SelectConfirm)),
                    None,
                    Some(InputEvent::Interrupt),
                    Some(InputEvent::Interrupt),
                ]),
            },
        )
        .await
        .expect("approval loop completes");

    let captured = responses.lock().expect("responses lock");
    assert_eq!(captured.len(), 1);
    assert!(matches!(
        &captured[0],
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitOnce,
            ..
        }
    ));
    assert!(!controller.chrome().approval_is_pending());
    assert!(controller.render_snapshot().contains("approved"));
}

#[tokio::test]
async fn event_loop_shows_and_resolves_pending_question_from_running_turn() {
    let (mut controller, answers) = controller_with_pending_math_question();
    let frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_frames = std::sync::Arc::clone(&frames);

    controller.type_text("ask me");
    controller
        .run_terminal_loop(
            move |app| {
                captured_frames.lock().expect("frames lock").push(
                    if app.question_dialog_is_focused() {
                        "question-focused".to_owned()
                    } else {
                        "no-question".to_owned()
                    },
                );
                Ok(())
            },
            OptionalScriptedEvents {
                events: VecDeque::from([
                    Some(InputEvent::Submit),
                    None,
                    Some(InputEvent::Action(KeybindingAction::SelectConfirm)),
                    Some(InputEvent::Action(KeybindingAction::InputTab)),
                    Some(InputEvent::Action(KeybindingAction::SelectConfirm)),
                    None,
                    Some(InputEvent::Interrupt),
                    Some(InputEvent::Interrupt),
                ]),
            },
        )
        .await
        .expect("question loop completes");

    assert_eq!(*answers.lock().expect("answers lock"), vec!["2"]);
    assert!(
        frames
            .lock()
            .expect("frames lock")
            .iter()
            .any(|frame| frame == "question-focused"),
        "the pending question must own the live focus while the turn runs"
    );
    assert!(controller.chrome().focused_overlay().is_none());
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("question: answered"),
        "the answered question commits as one terminal transcript fact:\n{snapshot}"
    );
    assert!(snapshot.contains("answered"));
}

#[tokio::test]
async fn question_dialog_consumes_keyboard_before_prompt_editing() {
    let mut controller = controller_with_keyboard_routing_question();

    controller
        .handle_input_event(InputEvent::Insert('2'))
        .await
        .expect("number shortcut selects a question option");
    assert_eq!(controller.chrome().prompt().text, "draft");
    {
        let state = controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused");
        assert_eq!(state.active_tab, 1);
        assert!(state.questions[0].selected[1]);
    }

    controller
        .handle_input_event(InputEvent::Insert('a'))
        .await
        .expect("letters are consumed by the question dialog");
    assert_eq!(controller.chrome().prompt().text, "draft");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorRight))
        .await
        .expect("right arrow action switches to submit");
    assert_eq!(controller.chrome().prompt().text, "draft");
    assert!(
        controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .on_submit_tab()
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorLeft))
        .await
        .expect("left arrow action switches back to the question");
    assert_eq!(controller.chrome().prompt().text, "draft");
    assert_eq!(
        controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .active_tab,
        1
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab switches to submit instead of editing the prompt");
    assert_eq!(controller.chrome().prompt().text, "draft");
    assert!(
        controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .on_submit_tab()
    );
}

#[tokio::test]
async fn question_dialog_prioritizes_real_keybindings_before_prompt_editing() {
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
        questions: vec![neo_agent_core::QuestionEventData {
            question: "Pick one".to_owned(),
            header: Some("Single".into()),
            body: None,
            options: vec![
                neo_agent_core::QuestionOptionData {
                    label: "First".to_owned(),
                    description: None,
                },
                neo_agent_core::QuestionOptionData {
                    label: "Second".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        response_tx,
        workflow_origin: None,
    });

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("down selects Other");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("down selects Other");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("enter starts Other editing");
    controller
        .handle_input_event(InputEvent::Insert('x'))
        .await
        .expect("typed text goes to Other");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("backspace").expect("valid key")))
        .await
        .expect("backspace edits Other text");
    {
        let state = controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused");
        assert_eq!(state.questions[0].other_text, "");
    }
    assert_eq!(controller.chrome().prompt().text, "draft");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("right").expect("valid key")))
        .await
        .expect("right edits Other instead of switching tabs");
    assert!(
        !controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .on_submit_tab()
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("tab").expect("valid key")))
        .await
        .expect("tab switches to submit");
    assert!(
        controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .on_submit_tab()
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("left").expect("valid key")))
        .await
        .expect("left switches back to question");
    assert_eq!(
        controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .active_tab,
        0
    );
}

#[tokio::test]
async fn event_loop_interrupt_drains_cancelled_barriers_before_exit() {
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
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let finished_tx = StdArc::new(std::sync::Mutex::new(Some(finished_tx)));
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_token = StdArc::clone(&observed_token);
        let finished_tx = StdArc::clone(&finished_tx);
        Box::pin(async move {
            *observed_token.lock().expect("token lock") = Some(channels.cancel_token.clone());
            channels.send_event(AgentEvent::MessageStarted {
                phase: neo_ai::MessagePhase::Unknown,
                turn: 1,
                id: "assistant-1".to_owned(),
            });
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "started".to_owned(),
            });
            channels.cancel_token.cancelled().await;
            channels.send_event(AgentEvent::MessageFinished {
                phase: neo_ai::MessagePhase::Unknown,
                turn: 1,
                id: "assistant-1".to_owned(),
                stop_reason: StopReason::Cancelled,
            });
            channels.send_event(AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            });
            channels.send_event(AgentEvent::RunFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            });
            if let Some(finished_tx) = finished_tx.lock().expect("finished lock").take() {
                let _ = finished_tx.send(());
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

    controller.type_text("cancel me");
    controller
        .run_terminal_loop(
            |_app| Ok(()),
            ScriptedEvents {
                events: VecDeque::from([
                    Some(InputEvent::Submit),
                    None,
                    Some(InputEvent::Interrupt),
                ]),
            },
        )
        .await
        .expect("interrupt exits terminal loop after draining cancellation");

    tokio::time::timeout(Duration::from_secs(1), finished_rx)
        .await
        .expect("turn driver should finish after cancellation")
        .expect("finished sender should not be dropped before sending");
    let token = captured_token
        .lock()
        .expect("token lock")
        .clone()
        .expect("turn token captured");
    assert!(token.is_cancelled());
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    assert!(controller.active_turn.is_none());
}

#[tokio::test]
async fn event_loop_opens_session_picker_and_continues_selected_transcript() {
    let (mut controller, requests) = session_picker_continuation_controller();

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
        .await
        .expect("session picker opens");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::SessionPicker(_))
    ));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("session loads");

    assert_eq!(controller.chrome().session_label(), SESSION_A);
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(
        &controller,
        "branch summary: Local branch summary"
    ));
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "hello")
    }));
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(entry, TranscriptEntry::AssistantMessage { content } if content == "hi back")
    }));

    controller.type_text("continue");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("continued prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("continued turn completes");
    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, vec![Content::text("continue")]);
    assert_eq!(requests[0].session_id.as_deref(), Some(SESSION_A));
    assert_eq!(requests[0].model, None);
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(entry, TranscriptEntry::AssistantMessage { content } if content == "continued")
    }));
}

#[tokio::test]
async fn event_loop_keeps_new_session_active_for_followup_prompt() {
    let workspace_root = test_workspace_root();
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let captured_requests = std::sync::Arc::clone(&captured_requests);
        Box::pin(async move {
            captured_requests
                .lock()
                .expect("record request")
                .push(request);
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
            Ok(TurnOutcome::session(SESSION_NEW))
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "new",
        "openai/gpt-4.1",
        workspace_root.clone(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );
    controller.local_config = Some(test_config(
        &workspace_root,
        workspace_root.join(".neo/sessions"),
    ));

    controller.type_text("read project");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("first prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("first turn completes");

    controller.type_text("continue");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("followup prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("followup turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].prompt, vec![Content::text("read project")]);
    assert_eq!(requests[0].session_id, None);
    assert_eq!(requests[1].prompt, vec![Content::text("continue")]);
    assert_eq!(requests[1].session_id.as_deref(), Some(SESSION_NEW));
    let first_registry = requests[0]
        .instruction_registry
        .as_ref()
        .expect("first turn registry");
    let followup_registry = requests[1]
        .instruction_registry
        .as_ref()
        .expect("followup turn registry");
    assert!(Arc::ptr_eq(first_registry, followup_registry));
    assert_eq!(controller.chrome().session_label(), SESSION_NEW);
}

#[tokio::test]
async fn event_loop_keeps_started_session_active_after_failed_turn() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let captured_requests = std::sync::Arc::clone(&captured_requests);
        Box::pin(async move {
            let request_index = {
                let mut requests = captured_requests.lock().expect("record request");
                requests.push(request);
                requests.len()
            };
            if request_index == 1 {
                channels
                    .session_ids
                    .send(SESSION_NEW.to_owned())
                    .expect("session id sent");
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "started".to_owned(),
                });
                anyhow::bail!("provider stream error after tool execution");
            }
            channels.send_event(AgentEvent::MessageStarted {
                phase: neo_ai::MessagePhase::Unknown,
                turn: 2,
                id: "assistant-2".to_owned(),
            });
            channels.send_event(AgentEvent::TextDelta {
                turn: 2,
                text: "continued".to_owned(),
            });
            channels.send_event(AgentEvent::MessageFinished {
                phase: neo_ai::MessagePhase::Unknown,
                turn: 2,
                id: "assistant-2".to_owned(),
                stop_reason: StopReason::EndTurn,
            });
            channels.send_event(AgentEvent::TurnFinished {
                turn: 2,
                stop_reason: StopReason::EndTurn,
            });
            Ok(TurnOutcome::session(SESSION_NEW))
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );

    controller.type_text("read project");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("first prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("failed first turn is drained");

    assert_eq!(controller.chrome().session_label(), SESSION_NEW);
    assert!(
        controller
            .render_snapshot()
            .contains("provider stream error")
    );

    controller.type_text("continue");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("followup prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("followup turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].prompt, vec![Content::text("read project")]);
    assert_eq!(requests[0].session_id, None);
    assert_eq!(requests[1].prompt, vec![Content::text("continue")]);
    assert_eq!(requests[1].session_id.as_deref(), Some(SESSION_NEW));
}

#[tokio::test]
async fn event_loop_forks_selected_session_and_continues_child_session() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver_and_forker(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
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
        EventDriverCallbacks {
            run_turn: move |request| {
                let captured_requests = std::sync::Arc::clone(&captured_requests);
                async move {
                    captured_requests
                        .lock()
                        .expect("record request")
                        .push(request);
                    Ok(vec![
                        AgentEvent::MessageStarted {
                            phase: neo_ai::MessagePhase::Unknown,
                            turn: 3,
                            id: "assistant-3".to_owned(),
                        },
                        AgentEvent::TextDelta {
                            turn: 3,
                            text: "continued on fork".to_owned(),
                        },
                        AgentEvent::TurnFinished {
                            turn: 3,
                            stop_reason: StopReason::EndTurn,
                        },
                    ])
                }
            },
            load_session: |_session_id| async move {
                panic!("fork action should not use the plain session loader");
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
                        [
                            AgentMessage::user_text("hello"),
                            AgentMessage::assistant(
                                [Content::text("hi back")],
                                Vec::new(),
                                StopReason::EndTurn,
                            ),
                        ],
                    ),
                ))
            },
        },
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+r").expect("valid key")))
        .await
        .expect("ctrl+r opens session picker");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+n").expect("valid key")))
        .await
        .expect("ctrl+n forks selected session");

    assert_eq!(controller.chrome().session_label(), SESSION_CHILD);
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(
        &controller,
        &format!("fork from session {SESSION_A}")
    ));
    assert!(transcript_has_status(
        &controller,
        &format!("switch to fork session {SESSION_CHILD}")
    ));
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "hello")
    }));

    controller.type_text("continue fork");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("continued prompt submits on fork");
    controller
        .wait_for_active_turn()
        .await
        .expect("continued fork turn completes");
    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, vec![Content::text("continue fork")]);
    assert_eq!(requests[0].session_id.as_deref(), Some(SESSION_CHILD));
    assert_eq!(requests[0].model, None);
}

#[tokio::test]
async fn event_loop_dispatches_mouse_wheel_to_transcript_view() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.keybindings.set_user_bindings([(
        KeybindingAction::EditorCursorUp,
        vec![KeyId::new("k").expect("valid key")],
    )]);
    for index in 0..30 {
        controller
            .transcript_mut()
            .push_status(format!("row-{index}"));
    }
    // Establish the viewport height through a bounded slice render.
    let initial = controller.tui.render_terminal_frame(80, 6).lines;

    controller
        .handle_input_event(wheel_event(MouseKind::ScrollUp))
        .await
        .expect("wheel up scrolls the document toward older rows");
    let wheel_up = controller.tui.render_terminal_frame(80, 6).lines;
    assert_ne!(wheel_up, initial);

    controller
        .handle_input_event(wheel_event(MouseKind::ScrollDown))
        .await
        .expect("wheel down returns the document to newest rows");
    assert_eq!(controller.tui.render_terminal_frame(80, 6).lines, initial);
}

#[tokio::test]
async fn event_loop_opens_model_picker_and_submits_with_selected_model() {
    let (mut controller, requests) = model_picker_submission_controller();

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::ModelPickerOpen))
        .await
        .expect("model picker opens");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("model selection applies");

    assert_eq!(
        controller.chrome().model_label(),
        "anthropic/claude-sonnet-4-5"
    );
    controller.type_text("use selected model");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits with selected model");
    controller
        .wait_for_active_turn()
        .await
        .expect("selected model turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    let selected = requests[0].model.as_ref().expect("selected model");
    assert_eq!(selected.provider, "anthropic");
    assert_eq!(selected.model, "claude-sonnet-4-5");
    assert_eq!(selected.max_context_tokens, Some(200_000));
    assert_eq!(requests[0].session_id, None);
}

#[tokio::test]
async fn question_up_down_does_not_recall_prompt_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    store.append(None, "old prompt").expect("seed history");
    let mut controller = controller_with_history_store(store);

    let (response_tx, _response_rx) = oneshot::channel();
    controller.register_pending_question(PendingQuestion {
        id: "question-1".to_owned(),
        questions: vec![neo_agent_core::QuestionEventData {
            question: "Pick one".to_owned(),
            header: Some("Single".into()),
            body: None,
            options: vec![
                neo_agent_core::QuestionOptionData {
                    label: "First".to_owned(),
                    description: None,
                },
                neo_agent_core::QuestionOptionData {
                    label: "Second".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        response_tx,
        workflow_origin: None,
    });

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up moves question selection");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down moves question selection");

    assert_eq!(
        controller.chrome().prompt().text,
        "",
        "question Up/Down must not leak into PromptState"
    );
    drop(dir);
}

#[test]
fn idle_model_and_provider_refreshes_bound_workflow_dispatch_client() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.providers.insert(
        "anthropic".to_owned(),
        ProviderConfig {
            display_name: Some("Anthropic test".to_owned()),
            provider_type: Some(neo_ai::ApiType::Anthropic),
            base_url: None,
            api_key: Some("test-key".to_owned()),
            api_key_env: None,
        },
    );
    config.models.insert(
        "selected-model".to_owned(),
        ModelConfig {
            provider: "anthropic".to_owned(),
            model: "claude-test".to_owned(),
            max_context_tokens: Some(200_000),
            max_output_tokens: Some(8_192),
            capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
            reasoning: neo_ai::ReasoningCapability::default(),
            display_name: Some("Selected model".to_owned()),
        },
    );
    let initial_harness = neo_agent_core::harness::FakeHarness::from_turns([]);
    let initial_client = initial_harness.client();
    let initial_agent_config = neo_agent_core::AgentConfig::for_model(initial_harness.model())
        .with_workspace_root(temp.path())
        .expect("workspace root");
    config
        .workflow_dispatch_resolver
        .refresh(neo_agent_core::runtime::WorkflowDispatchSnapshot {
            config: initial_agent_config,
            model_client: Arc::clone(&initial_client),
            registry: Arc::new(neo_agent_core::ToolRegistry::with_builtin_tools()),
            skills: None,
            process_supervisor: neo_agent_core::ProcessSupervisor::default(),
            context: neo_agent_core::AgentContext::new(),
        })
        .expect("bind initial workflow dispatch");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config);

    controller.apply_model_selection(&neo_tui::dialogs::ModelSelection {
        alias: "selected-model".to_owned(),
        thinking: false,
        reasoning: neo_ai::ReasoningSelection::Off,
    });

    let snapshot = controller
        .local_config
        .as_ref()
        .expect("local config")
        .workflow_dispatch_resolver
        .resolve()
        .expect("updated workflow dispatch");
    assert_eq!(snapshot.config.model.provider.0, "anthropic");
    assert_eq!(snapshot.config.model.model, "claude-test");
    assert!(
        !Arc::ptr_eq(&initial_client, &snapshot.model_client),
        "idle selection must replace the bound workflow client before another tool batch",
    );
    let selected_client = Arc::clone(&snapshot.model_client);

    controller.active_model = None;
    let config_path = controller
        .local_config
        .as_ref()
        .expect("local config")
        .config_path
        .clone();
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("config directory");
    fs::write(
        &config_path,
        r#"
default_model = "provider_refreshed_model"
default_provider = "refreshed-provider"

[providers.refreshed-provider]
type = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"

[models.provider_refreshed_model]
provider = "refreshed-provider"
model = "provider-refreshed-model"
"#,
    )
    .expect("provider config");
    controller.refresh_config();
    assert_eq!(
        controller
            .local_config
            .as_ref()
            .expect("refreshed config")
            .default_provider,
        "refreshed-provider"
    );
    assert_eq!(controller.active_session_id(), None);
    let resolved_model = crate::modes::run::resolve_model(
        controller.local_config.as_ref().expect("refreshed config"),
    )
    .expect("refreshed model resolves");
    assert_eq!(resolved_model.provider.0, "refreshed-provider");
    assert_eq!(resolved_model.model, "provider-refreshed-model");
    crate::modes::run::resolve_model_client(
        controller.local_config.as_ref().expect("refreshed config"),
        &resolved_model,
    )
    .expect("refreshed client resolves");

    let refreshed = controller
        .local_config
        .as_ref()
        .expect("refreshed config")
        .workflow_dispatch_resolver
        .resolve()
        .expect("provider-refreshed workflow dispatch");
    assert_eq!(refreshed.config.model.provider.0, "refreshed-provider");
    assert_eq!(refreshed.config.model.model, "provider-refreshed-model");
    assert!(
        !Arc::ptr_eq(&selected_client, &refreshed.model_client),
        "idle provider refresh must replace the matching workflow client",
    );
}
