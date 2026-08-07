//! Interactive keybinding behavior (split from `input.rs`).

use std::collections::VecDeque;

use neo_agent_core::{
    AgentEvent, ApprovalAction, ApprovalResponse, MessageOrigin, PermissionMode,
    ShellCommandOrigin, ToolResult,
};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    shell::{ChromeMode, CommandPaletteState, CommandSpec, Overlay, OverlayKind},
    transcript::TranscriptEntry,
};

use super::super::*;
use super::*;

#[tokio::test]
async fn ctrl_o_renders_before_queued_tool_finish() {
    struct ScriptedEvents(VecDeque<InputEvent>);

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.0
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }
    }

    let (finish_queued_tx, finish_queued_rx) = tokio::sync::oneshot::channel();
    let finish_queued_tx = Arc::new(std::sync::Mutex::new(Some(finish_queued_tx)));
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let finish_queued_tx = Arc::clone(&finish_queued_tx);
        Box::pin(async move {
            channels.send_event(AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: "write-1".to_owned(),
                name: "Write".to_owned(),
                result: ToolResult::ok("write complete"),
                workflow_origin: None,
                output_ref: None,
            });
            let sender = finish_queued_tx.lock().expect("finish sender lock").take();
            if let Some(sender) = sender {
                let _ = sender.send(());
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
    let content = (1..=12)
        .map(|line| format!("live-line-{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "write-1".to_owned(),
            name: "Write".to_owned(),
            arguments: serde_json::json!({
                "path": "artifact.txt",
                "content": content,
            }),
            workflow_origin: None,
            output_ref: None,
        });
    controller.start_turn_with_prompt_origin(Vec::new(), MessageOrigin::User);
    finish_queued_rx.await.expect("finish event queued");

    let mut rendered = Vec::new();
    controller
        .run_terminal_loop_with_suspend(
            |tui, _| {
                let frame = tui.render_terminal_frame_at(80, 24, Instant::now());
                let text = frame
                    .lines
                    .iter()
                    .map(|line| neo_tui::primitive::strip_ansi(line))
                    .collect::<Vec<_>>()
                    .join("\n");
                rendered.push(text);
                Ok(frame.next_animation_deadline)
            },
            || Ok(()),
            |_| Ok(()),
            ScriptedEvents(VecDeque::from([
                InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")),
                InputEvent::Interrupt,
                InputEvent::Interrupt,
                InputEvent::Interrupt,
            ])),
        )
        .await
        .expect("event loop exits");

    let first_after_ctrl_o = rendered.get(1).expect("frame after ctrl-o");
    assert!(first_after_ctrl_o.contains("Using Write"));
    assert!(first_after_ctrl_o.contains("1 files · unverified intent"));
    assert!(first_after_ctrl_o.contains("artifact.txt"));
    assert!(!first_after_ctrl_o.contains("Used Write"));
    assert!(controller.transcript().tool_output_expanded());
}

#[tokio::test]
async fn event_loop_dispatches_editor_keybinding_actions_to_prompt_edits() {
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

    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: vec![test_session_summary(
                "alpha",
                "Alpha",
                test_workspace_root(),
                "session",
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
    controller.set_clipboard_writer(Arc::new(|_text| Box::pin(async { Ok(()) })));

    for character in "hello brave world".chars() {
        controller
            .handle_input_event(InputEvent::Insert(character))
            .await
            .expect("insert succeeds");
    }

    let mut last_prompt_text = String::new();
    let mut last_prompt_cursor = 0usize;

    controller
        .run_terminal_loop(
            |app| {
                let prompt = app.prompt();
                if !prompt.text.is_empty() {
                    last_prompt_text = prompt.text.clone();
                    last_prompt_cursor = prompt.cursor;
                }
                Ok(())
            },
            FakeEvents {
                events: vec![
                    InputEvent::Action(KeybindingAction::InputCopy),
                    InputEvent::Action(KeybindingAction::EditorCursorWordLeft),
                    InputEvent::Action(KeybindingAction::EditorDeleteWordBackward),
                    InputEvent::Action(KeybindingAction::EditorDeleteToLineEnd),
                    InputEvent::Action(KeybindingAction::EditorYank),
                    InputEvent::Action(KeybindingAction::EditorUndo),
                    InputEvent::Action(KeybindingAction::EditorUndo),
                    InputEvent::Action(KeybindingAction::InputTab),
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]
                .into_iter(),
            },
        )
        .await
        .expect("event loop succeeds");

    assert_eq!(controller.chrome().copy_buffer(), Some("hello brave world"));
    assert_eq!(last_prompt_text, "hello \tworld");
    assert_eq!(last_prompt_cursor, 7);
}

#[tokio::test]
async fn event_loop_default_ctrl_c_clears_prompt_instead_of_copying() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_clipboard_writer(Arc::new(|_text| Box::pin(async { Ok(()) })));

    controller.type_text("copy through keybinding");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("clear keybinding handled");

    assert_eq!(controller.chrome().copy_buffer(), None);
    assert_eq!(controller.chrome().prompt().text, "");
}

#[tokio::test]
async fn event_loop_ctrl_c_prefers_selected_transcript_region() {
    let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = std::sync::Arc::clone(&copied);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_clipboard_writer(Arc::new(move |text| {
        let recorded = Arc::clone(&recorded);
        Box::pin(async move {
            recorded.lock().expect("record clipboard text").push(text);
            Ok(())
        })
    }));
    controller
        .transcript_mut()
        .push_user_message("selected user prompt");
    controller
        .transcript_mut()
        .push_assistant_message("selected assistant reply");
    controller.type_text("prompt text stays out of clipboard");

    controller
        .transcript_mut()
        .select_visible_transcript_entry();
    controller
        .handle_input_event(InputEvent::Action(
            KeybindingAction::TranscriptSelectionExtendUp,
        ))
        .await
        .expect("selection extends");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("copy action succeeds");
    wait_for_clipboard_idle(&mut controller).await;

    assert_eq!(
        copied.lock().expect("clipboard writes").as_slice(),
        ["You\nselected user prompt\n\nAssistant\nselected assistant reply"]
    );
    assert_eq!(controller.chrome().copy_buffer(), None);
    assert_eq!(
        controller.chrome().prompt().text,
        "prompt text stays out of clipboard"
    );
}

#[tokio::test]
async fn event_loop_ctrl_c_cancels_overlay_without_copying_prompt() {
    let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = std::sync::Arc::clone(&copied);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: vec![test_session_summary(
                "alpha",
                "Alpha",
                test_workspace_root(),
                "session",
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
    controller.set_clipboard_writer(Arc::new(move |text| {
        let recorded = Arc::clone(&recorded);
        Box::pin(async move {
            recorded.lock().expect("record clipboard text").push(text);
            Ok(())
        })
    }));

    controller.type_text("do not copy while overlay is focused");
    controller.open_session_picker();
    assert!(controller.chrome().focused_overlay().is_some());

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("overlay cancel succeeds");

    assert!(controller.chrome().focused_overlay().is_none());
    assert_eq!(controller.chrome().copy_buffer(), None);
    assert!(copied.lock().expect("clipboard writes").is_empty());
}

#[tokio::test]
async fn event_loop_ctrl_c_clears_prompt_before_confirming_exit() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("draft prompt");
    let should_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("ctrl-c handles prompt clear");

    assert!(!should_exit);
    assert_eq!(controller.chrome().prompt().text, "");
    assert_eq!(
        controller.chrome().exit_confirmation_label(),
        Some("Press Ctrl-C again to exit")
    );
    assert!(!transcript_has_status(
        &controller,
        "Press Ctrl-C again to exit"
    ));
}

#[tokio::test]
async fn event_loop_ctrl_c_requires_second_press_to_exit_when_prompt_is_empty() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    let first = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("first ctrl-c prompts");
    let second = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("second ctrl-c exits");

    assert!(!first);
    assert!(second);
}

#[tokio::test]
async fn event_loop_ctrl_c_key_cancels_active_turn_instead_of_confirming_exit() {
    let captured_token = Arc::new(std::sync::Mutex::new(None));
    let observed_token = Arc::clone(&captured_token);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_token = Arc::clone(&observed_token);
        *observed_token.lock().expect("token lock") = Some(channels.cancel_token.clone());
        Box::pin(async move {
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "started".to_owned(),
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
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("prompt submits");

    assert!(controller.active_turn.is_some());

    let should_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("ctrl-c cancels active turn");

    let token = captured_token
        .lock()
        .expect("token lock")
        .clone()
        .expect("turn token captured");
    assert!(!should_exit);
    assert!(token.is_cancelled());
    assert_eq!(controller.chrome().exit_confirmation_label(), None);
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    assert!(controller.active_turn.is_none());
}

#[tokio::test]
async fn event_loop_ctrl_c_clears_stale_working_state_before_exit_confirmation() {
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
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "ask".to_owned(),
            name: "AskUserQuestion".to_owned(),
            arguments: serde_json::json!({ "questions": [] }),
            workflow_origin: None,
            output_ref: None,
        });
    assert!(controller.chrome().working_label().is_some());

    let should_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("ctrl-c clears stale working state");

    assert!(!should_exit);
    assert!(controller.chrome().working_label().is_none());
    assert_eq!(controller.chrome().exit_confirmation_label(), None);

    controller
        .handle_input_event(InputEvent::Insert('o'))
        .await
        .expect("typing after stale interrupt succeeds");
    controller
        .handle_input_event(InputEvent::Insert('k'))
        .await
        .expect("typing after stale interrupt succeeds");
    assert_eq!(controller.chrome().prompt().text, "ok");
}

#[tokio::test]
async fn event_loop_ctrl_d_deletes_forward_until_prompt_is_empty_then_confirms_exit() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("ab");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorLineStart))
        .await
        .expect("move cursor to start");
    let delete = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
        .await
        .expect("ctrl-d deletes while prompt has text");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
        .await
        .expect("ctrl-d deletes final char");
    let first_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
        .await
        .expect("first empty ctrl-d prompts");
    let second_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
        .await
        .expect("second empty ctrl-d exits");

    assert!(!delete);
    assert_eq!(controller.chrome().prompt().text, "");
    assert!(!first_exit);
    assert!(second_exit);
    assert_eq!(controller.chrome().exit_confirmation_label(), None);
    assert!(!transcript_has_status(
        &controller,
        "Press Ctrl-D again to exit"
    ));
}

#[tokio::test]
async fn event_loop_ctrl_d_cancels_active_shell_without_starting_queued_commands() {
    struct ScriptedEvents(VecDeque<InputEvent>);

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.0
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }
    }

    let commands = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let observed_commands = Arc::clone(&commands);
    let cancel_token = Arc::new(std::sync::Mutex::new(None::<CancellationToken>));
    let observed_cancel_token = Arc::clone(&cancel_token);
    let cancellation_observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let driver_cancellation_observed = Arc::clone(&cancellation_observed);
    let driver_started = Arc::new(tokio::sync::Notify::new());
    let observed_driver_started = Arc::clone(&driver_started);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_shell_driver(Arc::new(move |request| {
        let observed_commands = Arc::clone(&observed_commands);
        let observed_cancel_token = Arc::clone(&observed_cancel_token);
        let driver_cancellation_observed = Arc::clone(&driver_cancellation_observed);
        let observed_driver_started = Arc::clone(&observed_driver_started);
        Box::pin(async move {
            observed_commands
                .lock()
                .expect("command lock")
                .push(request.command.clone());
            *observed_cancel_token.lock().expect("cancel token lock") =
                Some(request.cancel_token.clone());
            // Emit ShellCommandStarted so the transcript records a ShellRun
            // entry. Production ShellDrivers emit this themselves; the test
            // driver must simulate it.
            let _ = request.event_tx.send(AgentEvent::ShellCommandStarted {
                turn: 0,
                id: request.id.clone(),
                command: request.command.clone(),
                cwd: std::path::PathBuf::new(),
                origin: ShellCommandOrigin::UserShellMode,
            });
            observed_driver_started.notify_one();
            request.cancel_token.cancelled().await;
            driver_cancellation_observed.store(true, std::sync::atomic::Ordering::SeqCst);
            let mut result = completed_shell_result("");
            result.exit_code = None;
            result.outcome = neo_agent_core::ShellCommandOutcome::Cancelled;
            Ok(result)
        })
    }));

    controller.type_text("!");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter shell mode");
    controller.type_text("one");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start first shell command");
    controller.type_text("two");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("queue second shell command");
    tokio::time::timeout(Duration::from_secs(1), driver_started.notified())
        .await
        .expect("shell driver starts");

    controller
        .run_terminal_loop_with_suspend(
            |_, _| Ok(None),
            || Ok(()),
            |_| Ok(()),
            ScriptedEvents(VecDeque::from([
                InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")),
                InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")),
            ])),
        )
        .await
        .expect("event loop exits");

    assert!(
        cancel_token
            .lock()
            .expect("cancel token lock")
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled),
        "terminal exit must cancel the active shell"
    );
    assert!(cancellation_observed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(commands.lock().expect("command lock").as_slice(), ["one"]);
    assert!(controller.chrome().pending_input().is_empty());
    assert!(controller.active_shell_command.is_none());
    assert!(!controller.chrome().shell_running());
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::ShellRun { component }
                if component.command() == "one"
                    && component.finalization()
                        == neo_tui::primitive::Finalization::Finalized
        )
    }));
}

#[tokio::test]
async fn event_loop_interrupt_cancels_active_turn_token() {
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
                    Some(InputEvent::Interrupt),
                ]),
            },
        )
        .await
        .expect("interrupt exits terminal loop");

    let token = captured_token
        .lock()
        .expect("token lock")
        .clone()
        .expect("turn token captured");
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn event_loop_ctrl_z_reports_suspend_request() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    let should_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+z").expect("valid key")))
        .await
        .expect("ctrl-z is handled");

    assert!(!should_exit);
    assert!(controller.take_suspend_requested());
}

#[tokio::test]
async fn event_loop_ctrl_p_toggles_slash_completion() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+p").expect("valid key")))
        .await
        .expect("ctrl+p opens slash completion");

    assert_eq!(controller.chrome().prompt().text, "");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::PromptCompletion(_))
    ));

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+p").expect("valid key")))
        .await
        .expect("ctrl+p closes slash completion");

    assert_eq!(controller.chrome().prompt().text, "");
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_ctrl_p_toggles_slash_completion_without_editing_existing_prompt() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("hello");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+p").expect("valid key")))
        .await
        .expect("ctrl+p opens slash completion");

    assert_eq!(controller.chrome().prompt().text, "hello");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::PromptCompletion(_))
    ));

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+p").expect("valid key")))
        .await
        .expect("ctrl+p closes slash completion");

    assert_eq!(controller.chrome().prompt().text, "hello");
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn ctrl_o_toggles_primary_document_without_review_surface() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let body = (0..20)
        .map(|index| format!("expanded-line-{index}"))
        .collect::<Vec<_>>()
        .join("\n");
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "tool-1".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({ "path": "README.md" }),
            workflow_origin: None,
            output_ref: None,
        });
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "tool-1".to_owned(),
            name: "Read".to_owned(),
            result: ToolResult::ok(body),
            workflow_origin: None,
            output_ref: None,
        });

    let collapsed = controller.tui.render_terminal_frame(80, 24);
    let collapsed_text = collapsed
        .lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        collapsed_text.contains("20 lines"),
        "collapsed card shows the result chip: {collapsed_text}"
    );
    assert!(
        !collapsed_text.contains("expanded-line-19"),
        "collapsed card must not show the full result: {collapsed_text}"
    );
    assert!(!controller.transcript().tool_output_expanded());

    // Ctrl+O toggles the selected tool inside the primary document. It never
    // opens an overlay or a second transcript surface.
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")))
        .await
        .expect("ctrl-o toggles tool output");
    assert!(controller.transcript().tool_output_expanded());
    assert!(
        controller.chrome().focused_overlay().is_none(),
        "Ctrl+O must not open a review overlay"
    );
    let expanded = controller.tui.render_terminal_frame(80, 24);
    let expanded_text = expanded
        .lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        expanded_text.contains("expanded-line-19"),
        "expanded card shows the full result: {expanded_text}"
    );

    // A second Ctrl+O collapses back inside the primary document.
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")))
        .await
        .expect("ctrl-o collapses tool output");
    assert!(!controller.transcript().tool_output_expanded());
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_ctrl_t_expands_overflowing_todo_panel() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.tui.chrome_mut().set_todo_items(
        (0..7)
            .map(|index| {
                neo_tui::widgets::TodoDisplayItem::new(
                    format!("todo {index}"),
                    neo_tui::widgets::TodoDisplayStatus::Pending,
                )
            })
            .collect(),
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+t").expect("valid key")))
        .await
        .expect("ctrl-t expands overflowing todo panel");

    assert!(controller.chrome().todo_panel_expanded());

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+t").expect("valid key")))
        .await
        .expect("ctrl-t collapses overflowing todo panel");

    assert!(!controller.chrome().todo_panel_expanded());
}

#[tokio::test]
async fn event_loop_ctrl_t_is_noop_when_todo_panel_does_not_overflow() {
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
        .set_todo_items(vec![neo_tui::widgets::TodoDisplayItem::new(
            "todo",
            neo_tui::widgets::TodoDisplayStatus::Pending,
        )]);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+t").expect("valid key")))
        .await
        .expect("ctrl-t no-ops without todo overflow");

    assert!(!controller.chrome().todo_panel_expanded());
    assert!(controller.chrome().prompt().text.is_empty());
}

#[tokio::test]
async fn event_loop_dispatches_select_keybinding_actions_to_overlay_primitives() {
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

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (pending, mut response_rx) = make_pending_approval(ordinary_shell_request(
        "approval-1",
        "cargo test",
        None,
        None,
    ));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("selection moves down");
    assert!(matches!(
        controller.chrome().approval_selected_action(),
        Some(ApprovalAction::Reject)
    ));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectUp))
        .await
        .expect("selection moves up");
    assert!(matches!(
        controller.chrome().approval_selected_action(),
        Some(ApprovalAction::PermitOnce)
    ));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("approval confirms");
    assert!(matches!(
        response_rx.try_recv().expect("response ready"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitOnce,
            ..
        }
    ));
    assert!(!controller.chrome().approval_is_pending());

    controller.tui.chrome_mut().push_overlay(Overlay::new(
        "palette",
        OverlayKind::CommandPalette(CommandPaletteState::new((0..10).map(|index| {
            CommandSpec::new(
                format!("command-{index}"),
                format!("Command {index}"),
                None::<String>,
            )
        }))),
    ));
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageDown))
        .await
        .expect("selection pages down");
    let Some(OverlayKind::CommandPalette(palette)) = controller
        .chrome()
        .focused_overlay()
        .map(|overlay| &overlay.kind)
    else {
        panic!("expected command palette overlay");
    };
    assert_eq!(palette.selected_command().expect("command").id, "command-8");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageUp))
        .await
        .expect("selection pages up");
    let Some(OverlayKind::CommandPalette(palette)) = controller
        .chrome()
        .focused_overlay()
        .map(|overlay| &overlay.kind)
    else {
        panic!("expected command palette overlay");
    };
    assert_eq!(palette.selected_command().expect("command").id, "command-0");
    let _ = controller.tui.chrome_mut().close_focused_overlay();

    controller.tui.chrome_mut().push_overlay(Overlay::new(
        "custom",
        OverlayKind::Message("Body".to_owned()),
    ));
    controller
        .run_terminal_loop(
            |_app| Ok(()),
            FakeEvents {
                events: vec![
                    InputEvent::Action(KeybindingAction::SelectCancel),
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]
                .into_iter(),
            },
        )
        .await
        .expect("event loop exits after canceling overlay and receiving cancel again");

    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn shift_tab_cycles_development_mode_without_changing_permission_mode() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(
        controller.chrome().development_mode(),
        DevelopmentMode::Normal
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
        .await
        .expect("cycle to plan");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(
        controller.chrome().development_mode(),
        DevelopmentMode::Plan
    );
    assert!(transcript_has_status(&controller, "Plan Mode On"));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
        .await
        .expect("cycle to goal");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(
        controller.chrome().development_mode(),
        DevelopmentMode::Goal(GoalModeStatus::Pending)
    );
    assert!(transcript_has_status(&controller, "Goal Mode On"));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
        .await
        .expect("cycle to normal");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(
        controller.chrome().development_mode(),
        DevelopmentMode::Normal
    );
    assert!(transcript_has_status(&controller, "Goal Mode Off"));
}

#[tokio::test]
async fn shift_tab_key_uses_development_cycle() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("shift+tab").expect("valid key")))
        .await
        .expect("shift tab cycles");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(
        controller.chrome().development_mode(),
        DevelopmentMode::Plan
    );
}
