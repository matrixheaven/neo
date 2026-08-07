//! Transcript render/draw behavior (split from `transcript.rs`).

use std::collections::VecDeque;

use neo_agent_core::{AgentEvent, AgentMessage, Content, PendingQuestion, StopReason, ToolResult};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    transcript::{QuestionPromptState, TranscriptEntry, TranscriptPane},
};
use tokio::sync::oneshot;

use super::super::snapshot::compose_tui_frame;
use super::super::*;
use super::*;

#[test]
fn exit_projection_prints_final_answer_status_and_resume_command() {
    let session_id = Some("session_550e8400-e29b-41d4-a716-446655440000");
    assert_eq!(compose_exit_projection(None, None, None, None), "Bye\n");
    assert_eq!(
        compose_exit_projection(session_id, None, None, None),
        "Bye\n\nResume: neo resume session_550e8400-e29b-41d4-a716-446655440000\n"
    );
    let projection = compose_exit_projection(
        session_id,
        Some("done with the task"),
        Some("Tasks: 2 done, 1 in progress, 0 pending"),
        Some("Workflow Demo: running"),
    );
    assert!(projection.contains("done with the task"), "{projection}");
    assert!(projection.contains("Tasks: 2 done"), "{projection}");
    assert!(
        projection.contains("Workflow Demo: running"),
        "{projection}"
    );
    assert!(
        projection.contains("neo resume session_550e8400-e29b-41d4-a716-446655440000"),
        "{projection}"
    );
}

#[test]
fn exit_projection_bounds_long_answers_and_strips_ansi() {
    let long = format!("{} tail", "x".repeat(10_000));
    let projection = compose_exit_projection(None, Some(&format!("\x1b[31m{long}")), None, None);
    assert!(projection.len() <= EXIT_PROJECTION_MAX_BYTES + 1);
    assert!(!projection.contains('\x1b'));
    assert!(projection.contains("… (answer truncated)"));
    // The 10k-char answer is cut at the bound, so the trailing words are gone.
    assert!(!projection.contains("tail"));
}

#[test]
fn exit_recovery_line_carries_session_id_and_stays_bounded() {
    assert_eq!(exit_recovery_line(None), "Session interrupted.\n");
    let line = exit_recovery_line(Some("session_550e8400-e29b-41d4-a716-446655440000"));
    assert!(
        line.contains("neo resume session_550e8400-e29b-41d4-a716-446655440000"),
        "{line}"
    );
    assert!(line.len() < 200, "{line}");
}

#[test]
fn transcript_pane_exposes_live_rows_for_neo_tui_draw() {
    let app = NeoChromeState::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
    );
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 0,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
        workflow_origin: None,
        output_ref: None,
    });

    let lines = compose_tui_frame(&app, &mut transcript, 80, 12).expect("non-zero terminal size");

    let plain: Vec<String> = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect();
    assert!(plain.iter().any(|line| line.contains("Using Bash")));
    assert_eq!(compose_tui_frame(&app, &mut transcript, 0, 12), None);
}

#[tokio::test]
async fn resolving_question_records_answered_terminal_fact_in_transcript() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "session",
        "model",
        test_workspace_root(),
        |_| async { Ok(Vec::new()) },
    );
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

    assert_eq!(
        response_rx
            .try_recv()
            .expect("response should be sent")
            .answers,
        vec!["Left"]
    );
    // The answered question updates its transcript card in place as one
    // terminal fact instead of appending a separate status entry.
    assert!(transcript_entries(&controller).iter().any(|entry| matches!(
        entry,
        TranscriptEntry::QuestionPrompt(data)
            if data.id == "question-1"
                && matches!(
                    &data.state,
                    QuestionPromptState::Answered { answers } if answers == &vec!["Left".to_owned()]
                )
    )));
    assert!(
        !transcript_entries(&controller)
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::Status { .. })),
        "the answered question must not append a duplicate status"
    );
    assert!(controller.render_snapshot().contains("question: answered"));
}

#[test]
fn neo_tui_draw_composes_body_then_chrome_in_one_frame() {
    let mut app = NeoChromeState::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
    );
    app.prompt_mut().text = "next".to_owned();
    app.prompt_mut().cursor = 4;
    let mut transcript = TranscriptPane::new(80, 20);
    transcript.push_banner("Welcome to neo");
    transcript.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 0,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
        workflow_origin: None,
        output_ref: None,
    });

    let lines = compose_tui_frame(&app, &mut transcript, 80, 20)
        .expect("transcript frame composes body + chrome");

    let joined = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    // Banner (finalized) appears in the body before the running tool card,
    // which appears before the prompt chrome.
    let welcome = joined.find("Welcome to neo").expect("welcome in body");
    let tool = joined.find("Using Bash").expect("running tool in body");
    let prompt = joined.find("> next").expect("prompt chrome at tail");
    assert!(welcome < tool, "banner should precede the tool card");
    assert!(tool < prompt, "tool card should precede the prompt chrome");
    // The running tool card is live (● Using), not finalized (● Used).
    assert!(!joined.contains("Used Bash"));
}

#[test]
fn neo_tui_draw_replays_finished_tool_before_prompt_chrome() {
    let mut app = NeoChromeState::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
    );
    app.prompt_mut().text = "next".to_owned();
    app.prompt_mut().cursor = 4;
    let loaded = LoadedSessionTranscript::new(
        "alpha",
        Vec::new(),
        [
            AgentMessage::user_text("inspect"),
            AgentMessage::assistant(
                [Content::text("reading")],
                [neo_agent_core::AgentToolCall {
                    id: "tool-1".into(),
                    name: "Read".into(),
                    raw_arguments: r#"{"path":"README.md"}"#.into(),
                }],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result("tool-1", "Read", [Content::text("README contents")], false),
        ],
    );
    let mut transcript = TranscriptPane::new(80, 20);
    transcript.push_banner("Welcome to neo");
    replay_session_into_transcript(&mut transcript, &loaded);

    let lines =
        compose_tui_frame(&app, &mut transcript, 80, 20).expect("transcript frame composes replay");

    // Tool header spans are individually ANSI-colored, so strip codes
    // before substring searching for the committed tool card.
    let plain: Vec<String> = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect();
    let joined = plain.join("\n");
    let welcome = joined.find("Welcome to neo").expect("welcome in body");
    let prompt = joined.find("> next").expect("prompt chrome live row");
    let tool = joined
        .find("Used Read (README.md)")
        .expect("tool committed");
    assert!(welcome < tool);
    assert!(tool < prompt);
    assert!(!joined.contains("Using Read"));
}

#[tokio::test]
async fn thinking_boundaries_render_before_the_completed_turn_is_drained() {
    let run_turn: TurnDriver = Arc::new(|_request, channels| {
        Box::pin(async move {
            for (id, title) in [
                ("summary-1", "Planning initial workspace inspection"),
                ("summary-2", "Evaluating parallel subagent dispatch"),
                ("summary-3", "Checking the final workspace state"),
            ] {
                channels.send_event(AgentEvent::ThinkingStarted {
                    turn: 1,
                    id: id.to_owned(),
                    kind: neo_ai::ThinkingKind::Summary,
                });
                channels.send_event(AgentEvent::ThinkingDelta {
                    turn: 1,
                    text: format!("**{title}**"),
                });
                channels.send_event(AgentEvent::ThinkingFinished {
                    turn: 1,
                    signature: None,
                    redacted: false,
                });
            }
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

    assert_eq!(
        controller.drain_active_turn().await.expect("first drain"),
        FrameRequest::Immediate
    );
    let first = controller
        .tui
        .render_terminal_frame(80, 24)
        .lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        first.contains("Planning initial workspace inspection"),
        "{first}"
    );
    assert!(
        !first.contains("Evaluating parallel subagent dispatch"),
        "{first}"
    );

    assert_eq!(
        controller.drain_active_turn().await.expect("second drain"),
        FrameRequest::Immediate
    );
    let second = controller
        .tui
        .render_terminal_frame(80, 24)
        .lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        second.contains("Evaluating parallel subagent dispatch"),
        "{second}"
    );
    assert!(
        !second.contains("Planning initial workspace inspection"),
        "{second}"
    );

    assert_eq!(
        controller.drain_active_turn().await.expect("third drain"),
        FrameRequest::Immediate
    );
    let third = controller
        .tui
        .render_terminal_frame(80, 24)
        .lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        third.contains("Checking the final workspace state"),
        "{third}"
    );
    assert!(
        !third.contains("Evaluating parallel subagent dispatch"),
        "{third}"
    );

    controller
        .drain_active_turn()
        .await
        .expect("finish turn after final boundary");
    controller
        .drain_active_turn()
        .await
        .expect("remove completed turn after final frame");
    assert!(controller.active_turn.is_none());
}

#[tokio::test]
async fn thinking_boundaries_render_incrementally_through_terminal_loop() {
    struct Events {
        events: VecDeque<Option<InputEvent>>,
    }

    impl TerminalEvents for Events {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.poll_input_event(Duration::ZERO)?
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }

        fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
            Ok(self
                .events
                .pop_front()
                .unwrap_or(Some(InputEvent::Interrupt)))
        }
    }

    let titles = [
        "Planning initial workspace inspection",
        "Evaluating parallel subagent dispatch",
        "Checking the final workspace state",
    ];
    let run_turn = {
        let titles = titles.map(str::to_owned);
        move |_request: TurnRequest| {
            let titles = titles.clone();
            async move {
                let events = titles
                    .into_iter()
                    .enumerate()
                    .flat_map(|(index, title)| {
                        [
                            AgentEvent::ThinkingStarted {
                                turn: 1,
                                id: format!("summary-{}", index + 1),
                                kind: neo_ai::ThinkingKind::Summary,
                            },
                            AgentEvent::ThinkingDelta {
                                turn: 1,
                                text: format!("**{title}**"),
                            },
                            AgentEvent::ThinkingFinished {
                                turn: 1,
                                signature: None,
                                redacted: false,
                            },
                        ]
                    })
                    .chain([AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: StopReason::EndTurn,
                    }])
                    .collect();
                Ok(events)
            }
        }
    };
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
    );
    controller.type_text("stream");

    let rendered = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let rendered_for_callback = Arc::clone(&rendered);
    controller
        .run_terminal_loop_with_suspend(
            move |tui, _| {
                let frame = tui.render_terminal_frame(80, 24);
                let text = frame
                    .lines
                    .into_iter()
                    .map(|line| neo_tui::primitive::strip_ansi(&line))
                    .collect::<Vec<_>>()
                    .join("\n");
                rendered_for_callback
                    .lock()
                    .expect("render lock")
                    .push(text);
                Ok(frame.next_animation_deadline)
            },
            || Ok(()),
            |_| Ok(()),
            Events {
                events: VecDeque::from([
                    Some(InputEvent::Submit),
                    None,
                    None,
                    None,
                    None,
                    Some(InputEvent::Interrupt),
                    Some(InputEvent::Interrupt),
                ]),
            },
        )
        .await
        .expect("event loop should finish");

    let rendered = rendered.lock().expect("render lock");
    let title_frames = titles
        .iter()
        .map(|title| {
            rendered
                .iter()
                .position(|frame| frame.contains(title))
                .unwrap_or_else(|| panic!("missing {title} in frames: {rendered:?}"))
        })
        .collect::<Vec<_>>();
    assert!(title_frames.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(rendered[title_frames[0]].contains(titles[0]));
    assert!(!rendered[title_frames[0]].contains(titles[1]));
    assert!(!rendered[title_frames[1]].contains(titles[2]));
}

#[tokio::test]
async fn tall_transcript_keeps_prompt_input_on_normal_screen() {
    let submitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = Arc::clone(&submitted);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let observed = Arc::clone(&observed);
            async move {
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    // A tall live workload (running Bash with a long body) renders as one
    // bounded document slice inside the active fullscreen surface.
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "overflow-tool".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({ "command": "overflow-controller-command" }),
            workflow_origin: None,
            output_ref: None,
        });
    let body = (0..40)
        .map(|index| format!("overflow-controller-sentinel-{index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionUpdate {
            turn: 1,
            id: "overflow-tool".to_owned(),
            name: "Bash".to_owned(),
            partial_result: ToolResult::ok(body),
            workflow_origin: None,
            output_ref: None,
        });

    let frame = controller.tui.render_terminal_frame(40, 8);
    assert!(
        frame.lines.len() <= 8,
        "tall transcript stays bounded in the fullscreen frame"
    );
    assert!(
        frame
            .lines
            .iter()
            .map(|line| neo_tui::primitive::strip_ansi(line))
            .any(|line| line.contains("overflow-controller-sentinel-39")),
        "tail follow shows the newest output row"
    );

    // The prompt stays editable and submittable on the same surface.
    controller
        .handle_input_event(InputEvent::Insert('h'))
        .await
        .expect("prompt remains editable");
    controller
        .handle_input_event(InputEvent::Insert('i'))
        .await
        .expect("prompt remains editable");
    assert_eq!(controller.chrome().prompt().text, "hi");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("submit works");
    controller
        .wait_for_active_turn()
        .await
        .expect("submitted turn completes");
    assert!(submitted.load(std::sync::atomic::Ordering::SeqCst));

    // Ctrl+O toggles tool output inside the primary document; it never opens
    // a second surface.
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")))
        .await
        .expect("ctrl-o toggles tool output");
    assert!(controller.transcript().tool_output_expanded());
    assert!(
        controller.chrome().focused_overlay().is_none(),
        "Ctrl+O must not open an overlay"
    );
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")))
        .await
        .expect("ctrl-o collapses tool output");
    assert!(!controller.transcript().tool_output_expanded());
    assert!(controller.chrome().focused_overlay().is_none());
}

#[test]
fn composed_frame_lines_do_not_exceed_content_width() {
    let app = NeoChromeState::new("neo", "s", "openai/gpt-4.1", "/tmp");
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.push_welcome_banner("neo", "s", "m", "~Workspace/neo", "0.1.0", None);
    let lines = compose_tui_frame(&app, &mut transcript, 80, 12).expect("frame composes");
    let expected = 80usize;
    for (i, line) in lines.iter().enumerate() {
        let w = neo_tui::primitive::visible_width(line);
        assert!(
            w < expected,
            "line {i} reaches terminal autowrap column {expected}: {w}: {line:?}"
        );
    }
}
