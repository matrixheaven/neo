//! Interactive workflow behavior (moved from `tests.rs`).

use std::fs;

use super::super::*;
use super::*;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, ApprovalCancelReason,
    ApprovalResolution, Content, PermissionMode, StopReason, ToolRegistry, harness::FakeHarness,
};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    shell::OverlayKind,
    transcript::{ApprovalDisplayState, TranscriptEntry},
};

#[test]
fn persisted_workflow_events_apply_only_to_matching_session_generation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_active_session_id(SESSION_A.to_owned());
    let first_generation_a = controller.workflow_event_generation;
    controller.set_active_session_id(SESSION_B.to_owned());
    controller.set_active_session_id(SESSION_A.to_owned());
    let current_generation_a = controller.workflow_event_generation;
    let (persisted, workflow_events) = tokio::sync::mpsc::unbounded_channel();
    controller.workflow_events = workflow_events;
    let error_event = |message: &str| AgentEvent::Error {
        turn: 3,
        message: message.to_owned(),
        code: None,
        retry_after: None,
    };
    persisted
        .send(crate::modes::run::PersistedSessionWorkflowEvent::Event(
            Box::new(crate::modes::run::SessionWorkflowEvent {
                session_id: SESSION_A.to_owned(),
                generation: first_generation_a,
                event: error_event("stale session A generation must stay hidden"),
            }),
        ))
        .expect("session A delivery");
    persisted
        .send(crate::modes::run::PersistedSessionWorkflowEvent::Event(
            Box::new(crate::modes::run::SessionWorkflowEvent {
                session_id: SESSION_A.to_owned(),
                generation: current_generation_a,
                event: error_event("current session A generation is visible"),
            }),
        ))
        .expect("session B delivery");
    let entries_before = controller.tui.transcript().transcript().entries().len();

    controller.drain_workflow_events();

    let entries_after = controller.tui.transcript().transcript().entries().len();
    assert_eq!(entries_after, entries_before + 1);
}

#[tokio::test]
async fn workflow_event_routes_are_retained_across_switch_and_released_on_exit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);

    controller.set_active_session_id(SESSION_A.to_owned());
    controller.set_active_session_id(SESSION_B.to_owned());
    let generation_b = controller.workflow_event_generation;
    controller.set_active_session_id(SESSION_A.to_owned());
    assert_eq!(controller.workflow_event_routes.len(), 2);
    assert_eq!(controller.workflow_approval_routes.len(), 2);
    assert!(controller.workflow_event_generation > generation_b);

    controller.finalize_terminal_exit();

    assert!(controller.workflow_event_routes.is_empty());
    assert!(controller.workflow_approval_routes.is_empty());
    assert!(controller.workflow_event_ingress.is_none());
}

#[tokio::test]
async fn workflow_stop_before_approval_delivery_drain_removes_closed_responder() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    controller.set_active_session_id(SESSION_A.to_owned());
    let (handle, invocation, journal_path) =
        spawn_workflow_approval_invocation(&config, SESSION_A).await;

    tokio::time::timeout(Duration::from_secs(5), async {
        while controller.workflow_approvals.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approval delivery reaches controller queue");
    handle
        .stop(neo_agent_core::workflow::WorkflowActor::Human)
        .await
        .expect("stop workflow");
    let outcome = invocation
        .await
        .expect("invocation task")
        .expect("workflow invocation");
    assert_eq!(
        outcome.status,
        neo_agent_core::workflow::WorkflowOutcomeStatus::Cancelled
    );

    controller.drain_workflow_approvals();

    assert_eq!(
        handle.snapshot().await.state,
        neo_agent_core::workflow::WorkflowState::Cancelled
    );
    assert!(controller.pending_approvals.is_empty());
    assert!(controller.workflow_approval_backlog.is_empty());
    assert!(!controller.chrome().approval_is_pending());
    assert_cancelled_workflow_invocation_journal(&journal_path);
}

#[tokio::test]
async fn workflow_stop_after_modal_registration_clears_chrome_and_resolves_transcript() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    controller.set_active_session_id(SESSION_A.to_owned());
    let (handle, invocation, journal_path) =
        spawn_workflow_approval_invocation(&config, SESSION_A).await;

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            controller.drain_workflow_approvals();
            if controller.pending_approvals.len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approval modal is registered");
    let request_id = controller
        .pending_approvals
        .keys()
        .next()
        .expect("pending approval")
        .clone();
    assert!(controller.chrome().approval_is_pending());
    assert!(matches!(
        controller
            .tui
            .transcript()
            .transcript()
            .approval(&request_id)
            .expect("approval transcript")
            .state,
        ApprovalDisplayState::Pending
    ));

    handle
        .stop(neo_agent_core::workflow::WorkflowActor::Human)
        .await
        .expect("stop workflow");
    let outcome = invocation
        .await
        .expect("invocation task")
        .expect("workflow invocation");
    assert_eq!(
        outcome.status,
        neo_agent_core::workflow::WorkflowOutcomeStatus::Cancelled
    );
    assert_eq!(
        controller.drain_workflow_approvals(),
        FrameRequest::Immediate
    );

    assert_eq!(
        handle.snapshot().await.state,
        neo_agent_core::workflow::WorkflowState::Cancelled
    );
    assert!(!controller.pending_approvals.contains_key(&request_id));
    assert!(!controller.chrome().approval_is_pending());
    assert!(matches!(
        controller
            .tui
            .transcript()
            .transcript()
            .approval(&request_id)
            .expect("resolved approval transcript")
            .state,
        ApprovalDisplayState::Resolved(ApprovalResolution::Cancelled {
            reason: ApprovalCancelReason::Interrupt,
        })
    ));
    assert_cancelled_workflow_invocation_journal(&journal_path);
}

#[tokio::test]
async fn automatic_workflow_slash_starts_visible_model_turn_with_complete_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = demo_named_workflow_config(&temp, PermissionMode::Yolo);
    let prompt_dir = config.project_dir.join(".neo/prompts");
    fs::create_dir_all(&prompt_dir).expect("prompt template directory");
    fs::write(
        prompt_dir.join("workflow.md"),
        "This template must not rewrite workflow intent: $ARGUMENTS",
    )
    .expect("prompt template");
    config.prompt_templates = vec!["workflow".to_owned()];
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        move |request| {
            seen_requests.lock().expect("requests lock").push(request);
            async {
                Ok(vec![AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                }])
            }
        },
    );
    controller.local_config = Some(config);

    controller.type_text("/workflow Research this API");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("automatic workflow slash submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    {
        let requests = requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].prompt,
            vec![Content::text("/workflow Research this API")]
        );
        assert!(
            requests[0]
                .workflow_context
                .as_deref()
                .is_some_and(|context| {
                    context.contains("<workflow-catalog complete=\"true\">")
                        && context.contains("demo")
                })
        );
        assert_eq!(
            requests[0].prompt_display_text.as_deref(),
            Some("/workflow Research this API")
        );
        assert_eq!(requests[0].skill_context, None);
    }

    controller.type_text("ordinary follow-up");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("ordinary follow-up submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("follow-up completes");
    let requests_after_follow_up = requests.lock().expect("requests lock");
    assert_eq!(requests_after_follow_up.len(), 2);
    assert_eq!(requests_after_follow_up[1].workflow_context, None);
    assert_eq!(requests_after_follow_up[1].skill_context, None);
}

#[tokio::test]
async fn workflow_capacity_rejection_rolls_back_optimistic_user_message_before_retry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = demo_named_workflow_config(&temp, PermissionMode::Yolo);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async {
            Err::<Vec<AgentEvent>, _>(anyhow::anyhow!(
                crate::modes::interactive::workflow_slash::WORKFLOW_CONTEXT_TOO_LARGE
            ))
        },
    );
    controller.local_config = Some(config);

    controller.type_text("/workflow Research this API");
    for _ in 0..2 {
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("capacity rejection is handled");
        controller
            .wait_for_active_turn()
            .await
            .expect("capacity rejection turn completes");

        assert_eq!(
            controller.chrome().prompt().text,
            "/workflow Research this API"
        );
        assert_eq!(controller.pending_local_user_message_to_suppress, None);
        assert_eq!(controller.pending_workflow_restore_prompt, None);
        assert_eq!(
            transcript_entries(&controller)
                .iter()
                .filter(|entry| {
                    matches!(
                        entry,
                        TranscriptEntry::UserMessage { content, .. }
                            if content == "/workflow Research this API"
                    )
                })
                .count(),
            0
        );
    }
}

#[tokio::test]
async fn workflowish_is_not_workflow() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join("sessions"));
    let mut controller = controller_for_config(&config);

    // Strict slash_arg boundary: prefix-only forgery is not a workflow command.
    let handled = controller.handle_slash_command("/workflowish").await;
    assert!(
        !handled,
        "/workflowish must not be consumed as a workflow slash command"
    );

    // Ordinary text that only contains the word is not a command either.
    let handled_text = controller
        .handle_slash_command("please run /workflow for me")
        .await;
    assert!(!handled_text);
    assert!(controller.active_turn.is_none());
}

#[tokio::test]
async fn workflow_intent_slash_no_match_asks_before_authoring() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = demo_named_workflow_config(&temp, PermissionMode::Yolo);
    config.workflow_definitions = neo_agent_core::workflow::WorkflowDefinitionRegistry::empty();
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        move |request| {
            seen_requests.lock().expect("requests lock").push(request);
            async {
                Ok(vec![AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                }])
            }
        },
    );
    controller.local_config = Some(config);

    controller.type_text("/workflow find a matching workflow");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("automatic no-match request submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let requests = requests.lock().expect("requests lock");
    let context = requests[0].workflow_context.as_deref().expect("context");
    assert!(context.contains("If no definition fits, ask whether to create one."));
    assert!(context.contains("<workflow-catalog complete=\"true\">\n</workflow-catalog>"));
    assert_eq!(requests[0].skill_context, None);
}

#[tokio::test]
async fn named_workflow_slash_starts_visible_model_turn_with_full_schema() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("workspace");
    let config = demo_named_workflow_config(&temp, PermissionMode::Yolo);
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        move |request| {
            seen_requests.lock().expect("requests lock").push(request);
            async {
                Ok(vec![AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                }])
            }
        },
    );
    controller.permission_mode = PermissionMode::Yolo;
    *controller
        .live_permission_mode
        .write()
        .expect("permission lock") = PermissionMode::Yolo;
    controller.local_config = Some(config.clone());

    controller.type_text("/workflow:demo Research battery recycling");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("named workflow slash submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("/workflow:demo Research battery recycling")]
    );
    let context = requests[0]
        .workflow_context
        .as_deref()
        .expect("workflow context");
    assert!(context.contains("name=\"demo\""), "{context}");
    assert!(
        context.contains("&quot;required&quot;:[&quot;topic&quot;]"),
        "{context}"
    );
    assert!(!context.contains("lua_source"), "{context}");
    assert_eq!(requests[0].skill_context, None);
}

#[tokio::test]
async fn workflow_intent_slash_end_to_end_selects_runs_and_persists() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = demo_named_workflow_config(&temp, PermissionMode::Yolo);
    let session_id = "session_00000000-0000-4000-8000-000000000701";
    let session_path =
        crate::modes::sessions::session_path(session_id, &config).expect("session path");
    let run_session_path = session_path.clone();
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let harness = FakeHarness::from_turns([
        vec![
            neo_ai::AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "workflow-call".to_owned(),
            },
            neo_ai::AiStreamEvent::ToolCallStart {
                id: "workflow-call-1".to_owned(),
                name: "Workflow".to_owned(),
            },
            neo_ai::AiStreamEvent::ToolCallEnd {
                id: "workflow-call-1".to_owned(),
                raw_arguments: serde_json::json!({
                    "action": "run_saved",
                    "name": "demo",
                    "args": {"topic": "battery recycling"}
                })
                .to_string(),
            },
            neo_ai::AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            neo_ai::AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "workflow-answer".to_owned(),
            },
            neo_ai::AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            neo_ai::AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let driver_harness = harness.clone();
    let driver_config = config.clone();
    let run_turn = move |request: TurnRequest| {
        let session_path = run_session_path.clone();
        let seen_requests = std::sync::Arc::clone(&seen_requests);
        let harness = driver_harness.clone();
        let config = driver_config.clone();
        async move {
            seen_requests
                .lock()
                .expect("requests lock")
                .push(request.clone());
            std::fs::create_dir_all(session_path.parent().expect("session parent"))
                .expect("session directory");
            let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
                .await
                .expect("session writer");
            let agent_config = AgentConfig::for_model(harness.model())
                .with_workspace_root(&config.project_dir)
                .expect("workspace root")
                .with_session_directory(session_path.parent().expect("session parent"))
                .with_permission_mode(PermissionMode::Yolo)
                .with_turn_injection(request.workflow_context.clone().expect("workflow context"))
                .with_workflow_runtime(config.workflow_runtime.clone())
                .with_workflow_definitions(config.workflow_definitions.clone());
            let runtime = AgentRuntime::with_tools(
                agent_config,
                harness.client(),
                ToolRegistry::with_builtin_tools(),
            );
            let turn = crate::modes::run::run_prompt_with_runtime_message(
                request.prompt.clone(),
                request.prompt_origin.clone(),
                request.prompt_display_text.clone(),
                AgentContext::new(),
                &mut writer,
                runtime,
            )
            .await
            .expect("run workflow turn");
            assert!(turn.events.iter().any(|event| {
                matches!(event, AgentEvent::WorkflowStarted { workflow, .. } if workflow.display_name == "Demo")
            }));
            assert!(turn.events.iter().any(|event| {
                matches!(event, AgentEvent::ToolExecutionFinished { name, result, .. } if name == "Workflow" && !result.is_error)
            }));
            Ok(turn.events)
        }
    };
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "new",
        "openai/gpt-4.1",
        config.project_dir.clone(),
        run_turn,
        PickerCatalogs::default(),
        |_session_id| async { Err(anyhow::anyhow!("session loader unused")) },
    );
    controller.local_config = Some(config.clone());

    controller.type_text("/workflow");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open workflow picker");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("select workflow");
    assert_eq!(controller.chrome().prompt().text, "/workflow:demo ");

    controller.type_text("Research battery recycling");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("submit selected workflow");
    controller
        .wait_for_active_turn()
        .await
        .expect("workflow turn completes");

    {
        let requests = requests.lock().expect("requests lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].prompt,
            vec![Content::text("/workflow:demo Research battery recycling")]
        );
        assert!(requests[0].workflow_context.is_some());
        assert_eq!(requests[0].skill_context, None);
    }

    let model_requests = harness.requests();
    assert_eq!(model_requests.len(), 2);
    let slash_index = model_requests[0]
        .messages
        .iter()
        .position(|message| {
            chat_message_text(message) == "/workflow:demo Research battery recycling"
        })
        .expect("slash user message");
    let injection_index = model_requests[0]
        .messages
        .iter()
        .rposition(|message| chat_message_text(message).contains("<workflow_turn_context"))
        .expect("workflow injection");
    assert!(
        slash_index < injection_index,
        "workflow injection must append after the user message: {:?}",
        model_requests[0].messages
    );
    assert!(
        chat_message_text(
            model_requests[0]
                .messages
                .last()
                .expect("workflow injection"),
        )
        .contains("<workflow_turn_context")
    );
    assert!(
        model_requests[0]
            .messages
            .iter()
            .any(|message| chat_message_text(message).contains("neo-workflow-request"))
    );
    assert!(
        model_requests[0]
            .tools
            .iter()
            .any(|tool| tool.name == "Workflow")
    );
    assert_eq!(model_requests[1].tools, model_requests[0].tools);
    assert_eq!(
        model_requests[0].messages,
        model_requests[1].messages[..model_requests[0].messages.len()]
    );

    let messages = neo_agent_core::session::JsonlSessionReader::replay_messages(&session_path)
        .await
        .expect("replay session");
    assert!(messages.iter().any(|message| {
        matches!(
            message,
            AgentMessage::User { content, .. }
                if content == &vec![Content::text("/workflow:demo Research battery recycling")]
        )
    }));
}

#[tokio::test]
async fn workflow_slash_local_errors_preserve_composer_and_start_nothing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = demo_named_workflow_config(&temp, PermissionMode::Yolo);
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        move |request| {
            seen_requests.lock().expect("requests lock").push(request);
            async { Ok(Vec::<AgentEvent>::new()) }
        },
    );
    controller.local_config = Some(config);

    for prompt in ["/workflow:", "/workflow:demo", "/workflow:missing task"] {
        controller
            .tui
            .chrome_mut()
            .prompt_mut()
            .clear_after_submit();
        controller.type_text(prompt);
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("local workflow error is handled");
        assert_eq!(controller.chrome().prompt().text, prompt);
        assert!(controller.active_turn.is_none());
    }
    assert!(requests.lock().expect("requests lock").is_empty());
}

#[tokio::test]
async fn workflow_slash_unknown_name_offers_only_a_unique_suggestion() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = demo_named_workflow_config(&temp, PermissionMode::Yolo);
    let mut controller = controller_for_config(&config);

    controller.type_text("/workflow:dem Research this API");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("unknown workflow name is handled locally");

    assert_eq!(
        controller.chrome().prompt().text,
        "/workflow:dem Research this API"
    );
    assert!(
        controller
            .render_snapshot()
            .contains("Did you mean `demo`?")
    );
    assert!(controller.active_turn.is_none());
}

#[tokio::test]
async fn bare_workflow_slash_opens_picker_and_selection_only_fills_composer() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = demo_named_workflow_config(&temp, PermissionMode::Yolo);
    let mut controller = controller_for_config(&config);

    controller.type_text("/workflow");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("bare /workflow opens picker");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::WorkflowPicker(_))
    ));
    assert!(controller.active_turn.is_none());

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("picker selection is handled");
    assert_eq!(controller.chrome().prompt().text, "/workflow:demo ");
    assert!(controller.active_turn.is_none());
}

#[tokio::test]
async fn workflow_slash_is_rejected_while_busy_without_queueing_as_prose() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { std::future::pending::<Result<Vec<AgentEvent>>>().await },
    );
    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("first prompt starts");
    assert!(controller.active_turn.is_some());

    controller.type_text("/workflow use an existing workflow");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("busy workflow slash is handled");
    assert_eq!(
        controller.chrome().prompt().text,
        "/workflow use an existing workflow"
    );
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
    );
    controller.abort_active_turn();
}

#[tokio::test]
async fn workflow_operator_answers_controls_and_saves_through_canonical_owners() {
    use neo_agent_core::workflow::{
        AwaitUserInput, UserAnswerPolicy, WorkflowDefinitionRegistry,
        WorkflowDefinitionRegistryConfig, WorkflowLimits,
    };

    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join("sessions");
    let project_dir = temp.path().join("workspace");
    let neo_home = temp.path().join("neo-home");
    std::fs::create_dir_all(&project_dir).expect("project dir");
    std::fs::create_dir_all(&neo_home).expect("neo home");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        project_dir.clone(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let mut config = test_config(&project_dir, sessions_dir.clone());
    config.workflow_definitions =
        WorkflowDefinitionRegistry::new(WorkflowDefinitionRegistryConfig {
            neo_home,
            workspace: project_dir.clone(),
            project_trusted: true,
            limits: WorkflowLimits::default(),
            builtins: Vec::new(),
        });
    let runtime = neo_agent_core::workflow::WorkflowRuntime::new(WorkflowLimits::default());
    let handle = runtime
        .create_run(
            &sessions_dir,
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "browser-inline".to_owned(),
                description: "answer and save from tasks".to_owned(),
                phases: vec![neo_agent_core::workflow::WorkflowPhase {
                    id: "work".to_owned(),
                    description: "work".to_owned(),
                }],
                script: "return { ok = true }".to_owned(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": { "ok": { "type": "boolean" } },
                    "required": ["ok"],
                    "additionalProperties": false
                })),
                display_name: Some("Browser inline".to_owned()),
                input_schema: None,
                definition_origin: None,
                inline_unsaved: true,
            },
        )
        .await
        .expect("create workflow");
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");
    let awaiting = handle
        .await_user(
            0,
            AwaitUserInput {
                prompt: "Continue?".to_owned(),
                answer_schema: serde_json::json!({
                    "oneOf": [
                        { "title": "Continue", "type": "boolean" },
                        { "title": "Explain", "type": "string" }
                    ]
                }),
                default: Some(serde_json::json!(true)),
                title: Some("Continue".to_owned()),
                answer_policy: Some(UserAnswerPolicy::Human),
            },
        )
        .await;
    assert!(awaiting.is_err(), "await_user must suspend the workflow");
    let task_id = handle.run_id.0.clone();
    config
        .background_tasks
        .start_workflow(task_id.clone(), "browser inline".to_owned(), handle.clone())
        .await
        .expect("register workflow");
    for index in 0..55 {
        config
            .background_tasks
            .start_question(format!("question-{index}"), format!("Question {index}"))
            .await;
    }
    config
        .workflow_definitions
        .save(
            neo_agent_core::workflow::WorkflowSaveScope::Project,
            &neo_agent_core::workflow::WorkflowSaveRequest {
                name: "browser-saved".to_owned(),
                display_name: "Existing browser workflow".to_owned(),
                description: "existing definition".to_owned(),
                phases: vec![neo_agent_core::workflow::WorkflowPhase {
                    id: "work".to_owned(),
                    description: "work".to_owned(),
                }],
                lua_source: "return { ok = false }".to_owned(),
                input_schema: None,
                output_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "ok": { "type": "boolean" } },
                    "required": ["ok"],
                    "additionalProperties": false
                }),
            },
            false,
        )
        .expect("seed conflicting workflow");
    controller.local_config = Some(config);

    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show workflow");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .and_then(neo_tui::tasks_browser::TaskBrowserState::workflow_item)
            .map(|item| item.id.as_str()),
        Some(task_id.as_str())
    );
    assert!(
        controller
            .chrome()
            .task_browser_state()
            .and_then(neo_tui::tasks_browser::TaskBrowserState::answer_draft)
            .is_some()
    );
    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("dismiss answer");
    assert!(
        controller
            .chrome()
            .task_browser_state()
            .and_then(neo_tui::tasks_browser::TaskBrowserState::answer_draft)
            .is_none()
    );
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("reopen answer");
    assert!(
        controller
            .chrome()
            .task_browser_state()
            .and_then(neo_tui::tasks_browser::TaskBrowserState::answer_draft)
            .is_some()
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("right").expect("right key")))
        .await
        .expect("choose text branch");
    controller
        .handle_input_event(InputEvent::Insert('x'))
        .await
        .expect("edit text branch");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .and_then(|state| state.answer_draft())
            .map(|draft| &draft.value),
        Some(&serde_json::json!("x"))
    );
    controller
        .handle_input_event(InputEvent::MoveLeft)
        .await
        .expect("restore boolean branch");
    controller
        .handle_input_event(InputEvent::Insert(' '))
        .await
        .expect("toggle boolean branch");
    controller
        .handle_input_event(InputEvent::MoveRight)
        .await
        .expect("restore text branch");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .and_then(|state| state.answer_draft())
            .map(|draft| &draft.value),
        Some(&serde_json::json!("x"))
    );
    controller
        .handle_input_event(InputEvent::MoveLeft)
        .await
        .expect("restore edited boolean branch");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit answer");
    assert_eq!(
        handle
            .pending_user_input()
            .await
            .expect("pending input")
            .expect("input record")
            .answer,
        Some(serde_json::json!(false))
    );

    controller
        .handle_input_event(InputEvent::Insert('s'))
        .await
        .expect("open save dialog");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .and_then(neo_tui::tasks_browser::TaskBrowserState::save_draft)
            .map(|draft| draft.name.as_str()),
        Some("browser-inline")
    );
    controller
        .handle_input_event(InputEvent::Paste("browser-saved".to_owned()))
        .await
        .expect("set save name");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("request replacement");
    let replacement = controller
        .chrome()
        .task_browser_state()
        .and_then(neo_tui::tasks_browser::TaskBrowserState::save_draft)
        .and_then(|draft| draft.replacement.as_ref())
        .expect("replacement details");
    assert_eq!(
        replacement.existing_display_name,
        "Existing browser workflow"
    );
    assert_eq!(replacement.new_display_name, "Browser inline");
    assert_eq!(
        replacement.target_location,
        project_dir.join(".neo/workflows").to_string_lossy()
    );
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("replace workflow");
    assert!(
        project_dir
            .join(".neo/workflows/browser-saved.lua")
            .is_file()
    );
    assert_eq!(
        std::fs::read_to_string(project_dir.join(".neo/workflows/browser-saved.lua"))
            .expect("read replaced workflow"),
        "return { ok = true }"
    );
    assert!(
        handle
            .output()
            .await
            .expect("output")
            .metadata
            .inline_unsaved
    );
    assert!(
        !controller
            .chrome()
            .task_browser_state()
            .and_then(neo_tui::tasks_browser::TaskBrowserState::workflow_item)
            .and_then(|item| item.workflow.as_ref())
            .is_some_and(|workflow| workflow.inline_unsaved)
    );

    controller
        .handle_input_event(InputEvent::Insert('x'))
        .await
        .expect("request stop");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .stop_confirmation_task_id(),
        Some(task_id.as_str())
    );
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm stop");
    assert_eq!(
        handle.snapshot().await.state,
        neo_agent_core::workflow::WorkflowState::Cancelled
    );
}
