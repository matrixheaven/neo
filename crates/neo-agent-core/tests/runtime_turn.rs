use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AgentRuntimeError,
    AgentToolCall, ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest,
    ApprovalResponse, AskUserTool, CompactionSettings, CompactionSummary, Content, PermissionMode,
    PermissionOperation, QueueMode, SessionApprovalKey, SessionApprovalScope, ShellCommandOrigin,
    ShellCommandOutcome, SkillInvocationOutcome, SkillInvocationSource, StopReason, TodoEventData,
    Tool, ToolContext, ToolError, ToolExecutionMode, ToolFuture, ToolRegistry, ToolResult,
    harness::{FakeHarness, fake_model},
    session::{JsonlSessionWriter, main_agent_plans_dir, workspace_sessions_dir},
    skills::SkillStore,
};
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, ChatRequest, ModelCapabilities, ModelClient, ModelSpec,
    ProviderId, ReasoningCapability, ReasoningEffort, ReasoningSelection, ToolSpec,
};
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::mpsc,
    sync::{Notify, oneshot},
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;

async fn collect_turn_events(
    harness: &FakeHarness,
    config: AgentConfig,
    context: &mut AgentContext,
    input: AgentMessage,
) -> Vec<AgentEvent> {
    let runtime = AgentRuntime::new(config, harness.client());
    runtime
        .run_turn(context, input)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed")
}

fn select_action(request: &ApprovalRequest, action: ApprovalAction) -> ApprovalResponse {
    assert!(
        request.options.iter().any(|option| option.action == action),
        "action {action:?} not offered in {:?}",
        request
            .options
            .iter()
            .map(|option| &option.action)
            .collect::<Vec<_>>()
    );
    ApprovalResponse::Selected {
        request_id: request.id.clone(),
        action,
        feedback: None,
    }
}

fn permit_once(request: &ApprovalRequest) -> ApprovalResponse {
    select_action(request, ApprovalAction::PermitOnce)
}

fn reject_action(request: &ApprovalRequest) -> ApprovalResponse {
    select_action(request, ApprovalAction::Reject)
}

fn permit_for_session(request: &ApprovalRequest) -> ApprovalResponse {
    let action = request
        .options
        .iter()
        .find_map(|option| match &option.action {
            ApprovalAction::PermitForSession { .. } => Some(option.action.clone()),
            _ => None,
        })
        .expect("PermitForSession option");
    select_action(request, action)
}

fn permit_for_prefix(request: &ApprovalRequest) -> ApprovalResponse {
    let action = request
        .options
        .iter()
        .find_map(|option| match &option.action {
            ApprovalAction::PermitForPrefix { .. } => Some(option.action.clone()),
            _ => None,
        })
        .expect("PermitForPrefix option");
    select_action(request, action)
}

fn first_offered_action(request: &ApprovalRequest) -> ApprovalResponse {
    let action = request
        .options
        .first()
        .map(|option| option.action.clone())
        .expect("approval options");
    select_action(request, action)
}

fn approve_plan(request: &ApprovalRequest) -> ApprovalResponse {
    let action = request
        .options
        .iter()
        .find_map(|option| match &option.action {
            ApprovalAction::ApprovePlan { .. } => Some(option.action.clone()),
            _ => None,
        })
        .expect("ApprovePlan option");
    select_action(request, action)
}

fn approve_plan_with_label(request: &ApprovalRequest, label: &str) -> ApprovalResponse {
    let action = request
        .options
        .iter()
        .find_map(|option| match &option.action {
            ApprovalAction::ApprovePlan {
                selection: Some(selection),
            } if selection.label == label => Some(option.action.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("ApprovePlan selection {label:?}"));
    select_action(request, action)
}

fn reject_plan(request: &ApprovalRequest) -> ApprovalResponse {
    select_action(request, ApprovalAction::RejectPlan)
}

fn start_goal(request: &ApprovalRequest) -> ApprovalResponse {
    select_action(request, ApprovalAction::StartGoal)
}

fn reject_goal(request: &ApprovalRequest) -> ApprovalResponse {
    select_action(request, ApprovalAction::RejectGoal)
}

fn revise_goal_with_feedback(request: &ApprovalRequest, feedback: &str) -> ApprovalResponse {
    assert!(
        request.options.iter().any(|option| {
            matches!(
                option.action,
                ApprovalAction::ReviseGoal {
                    preset_feedback: None
                }
            )
        }),
        "ReviseGoal manual feedback option not offered"
    );
    ApprovalResponse::Selected {
        request_id: request.id.clone(),
        action: ApprovalAction::ReviseGoal {
            preset_feedback: None,
        },
        feedback: Some(feedback.to_owned()),
    }
}

fn approval_request_id(event: &AgentEvent) -> Option<&str> {
    match event {
        AgentEvent::ApprovalRequested { request } => Some(request.id.as_str()),
        _ => None,
    }
}

#[tokio::test]
async fn runtime_streams_one_turn_text_and_updates_context() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "hel".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "lo".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("say hello"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(
        events,
        vec![
            AgentEvent::RunStarted { turn: 1 },
            AgentEvent::MessageAppended {
                message: AgentMessage::user_text("say hello"),
            },
            AgentEvent::ContextWindowUpdated {
                turn: 1,
                used_tokens: 4,
                projected_tokens: Some(4),
                max_tokens: None,
                trigger_tokens: None,
                remaining_tokens: None,
                source: Some(neo_agent_core::ContextWindowSource::MissingModelWindow),
            },
            AgentEvent::TurnStarted { turn: 1 },
            AgentEvent::MessageStarted {
                turn: 1,
                id: "msg_1".to_owned(),
            },
            AgentEvent::TextDelta {
                turn: 1,
                text: "hel".to_owned(),
            },
            AgentEvent::TextDelta {
                turn: 1,
                text: "lo".to_owned(),
            },
            AgentEvent::MessageFinished {
                turn: 1,
                id: "msg_1".to_owned(),
                stop_reason: StopReason::EndTurn,
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::assistant(
                    [Content::text("hello")],
                    Vec::new(),
                    StopReason::EndTurn,
                ),
            },
            AgentEvent::ContextWindowUpdated {
                turn: 1,
                used_tokens: 9,
                projected_tokens: Some(9),
                max_tokens: None,
                trigger_tokens: None,
                remaining_tokens: None,
                source: Some(neo_agent_core::ContextWindowSource::MissingModelWindow),
            },
            AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            },
            AgentEvent::RunFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            },
        ]
    );
    assert_eq!(context.messages()[0], AgentMessage::user_text("say hello"));
    assert_eq!(
        context.messages()[1],
        AgentMessage::assistant([Content::text("hello")], Vec::new(), StopReason::EndTurn)
    );
}

#[tokio::test]
async fn runtime_injects_workspace_context_into_model_request() {
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "ok".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("where am I?"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let request = harness.requests().pop().expect("model request");
    assert!(matches!(
        request.messages.first(),
        Some(neo_ai::ChatMessage::System { content })
            if content.iter().any(|part| matches!(
                part,
                neo_ai::ContentPart::Text { text }
                    if text.contains("Runtime Context")
                        && text.contains("- cwd:")
                        && text.contains(&workspace_root.display().to_string())
                        && text.contains("Do not prefix shell commands with `cd")
            ))
    ));
}

#[tokio::test]
async fn runtime_context_window_estimate_includes_effective_request_messages() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace_root = temp.path().join("workspace");
    std::fs::create_dir(&workspace_root).expect("workspace dir");
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_system_prompt("system prompt that must count toward context")
            .with_workspace_root(workspace_root)
            .expect("workspace root"),
        harness.client(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("short"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ContextWindowUpdated {
                turn: 1,
                used_tokens,
                ..
            } if *used_tokens > 20
        )),
        "context estimate should include system/workspace request messages, not only the user buffer"
    );
}

#[tokio::test]
async fn runtime_context_window_estimate_includes_tool_schemas() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let tool = ToolSpec {
        name: "LargeSchemaTool".to_owned(),
        description: "tool description that must count toward context".repeat(8),
        input_schema: json!({
            "type": "object",
            "properties": {
                "payload": {
                    "type": "string",
                    "description": "schema description that must count toward context".repeat(16),
                },
            },
            "required": ["payload"],
            "additionalProperties": false,
        }),
    };
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_tools(vec![tool]),
        harness.client(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("x"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let used_tokens = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ContextWindowUpdated { used_tokens, .. } => Some(*used_tokens),
            _ => None,
        })
        .expect("context window update");

    assert!(
        used_tokens > 100,
        "context estimate should include tool name, description, and input schema; got {used_tokens}"
    );
}

#[tokio::test]
async fn runtime_emits_provider_token_usage() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "hello".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: Some(neo_ai::TokenUsage {
                input_tokens: 123,
                output_tokens: 45,
                input_cache_read_tokens: 100,
                input_cache_write_tokens: 7,
            }),
        },
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("say hello"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.contains(&AgentEvent::TokenUsage {
        turn: 1,
        usage: neo_agent_core::AgentTokenUsage {
            input_tokens: 123,
            output_tokens: 45,
            input_cache_read_tokens: 100,
            input_cache_write_tokens: 7,
        },
    }));
}

#[tokio::test]
async fn goal_mode_authoring_injects_exit_goal_mode_guidance() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_goal_mode_authoring(true),
        harness.client(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("draft goal"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let request = harness.requests().pop().expect("model request");
    assert!(request.messages.iter().any(|message| matches!(
        message,
        neo_ai::ChatMessage::User { content }
            if content.iter().any(|part| matches!(
                part,
                neo_ai::ContentPart::Text { text }
                    if text.contains("<system-reminder>")
                        && text.contains("Goal mode is active")
                        && text.contains("ExitGoalMode")
                        && text.contains("Do not start a durable goal directly")
            ))
    )));
}

#[tokio::test]
async fn runtime_yields_model_events_before_model_stream_finishes() {
    let harness = DelayedHarness::new(vec![
        DelayedStep::Event(AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::TextDelta {
            text: "early".to_owned(),
        }),
        DelayedStep::Delay(Duration::from_secs(5)),
        DelayedStep::Event(AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        }),
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("stream"));

    assert_eq!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("run start should stream before delayed message end")
            .expect("run start event")
            .expect("run start should be ok"),
        AgentEvent::RunStarted { turn: 1 }
    );
    assert_eq!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("prompt append should stream before delayed message end")
            .expect("prompt append event")
            .expect("prompt append should be ok"),
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("stream"),
        }
    );
    assert_eq!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("context tokens should stream before delayed message end")
            .expect("context tokens event")
            .expect("context tokens should be ok"),
        AgentEvent::ContextWindowUpdated {
            turn: 1,
            used_tokens: 3,
            projected_tokens: Some(3),
            max_tokens: None,
            trigger_tokens: None,
            remaining_tokens: None,
            source: Some(neo_agent_core::ContextWindowSource::MissingModelWindow),
        }
    );
    assert_eq!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("turn start should stream before delayed message end")
            .expect("turn start event")
            .expect("turn start should be ok"),
        AgentEvent::TurnStarted { turn: 1 }
    );
    assert_eq!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("message start should stream before delayed message end")
            .expect("message start event")
            .expect("message start should be ok"),
        AgentEvent::MessageStarted {
            turn: 1,
            id: "msg_1".to_owned(),
        }
    );
    assert_eq!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("text delta should stream before delayed message end")
            .expect("text delta event")
            .expect("text delta should be ok"),
        AgentEvent::TextDelta {
            turn: 1,
            text: "early".to_owned(),
        }
    );
}

#[tokio::test]
async fn runtime_cancels_in_flight_model_stream_and_emits_cancelled_barriers() {
    let harness = DelayedHarness::new(vec![
        DelayedStep::Event(AiStreamEvent::MessageStart {
            id: "msg_cancel".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::TextDelta {
            text: "partial".to_owned(),
        }),
        DelayedStep::Delay(Duration::from_secs(5)),
        DelayedStep::Event(AiStreamEvent::TextDelta {
            text: "late".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        }),
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();

    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("cancel stream"),
        cancel.clone(),
    );

    let mut events = Vec::new();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("event before cancellation")
    {
        let event = event.expect("event should be ok");
        let should_cancel = matches!(event, AgentEvent::TextDelta { .. });
        events.push(event);
        if should_cancel {
            cancel.cancel();
            break;
        }
    }
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled barriers should arrive promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::MessageFinished { id, .. } if id == "msg_cancel"
    )));
    assert!(events.contains(&AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: StopReason::Cancelled,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { text, .. } if text == "late"
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::MessageAppended {
            message: AgentMessage::Assistant { .. },
        }
    )));
    assert!(!context.messages().iter().any(|message| {
        matches!(message, AgentMessage::Assistant { .. }) || message.text().contains("partial")
    }));
}

#[tokio::test]
async fn runtime_records_tool_calls_and_sends_tool_specs_to_model() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_2".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "Read".to_owned(),
        },
        AiStreamEvent::ToolCallArgsDelta {
            id: "tool_1".to_owned(),
            json_fragment: r#"{"path":"README.md"}"#.to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            raw_arguments: json!({ "path": "README.md" }).to_string(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]);
    let tool = ToolSpec {
        name: "Read".to_owned(),
        description: "read file".to_owned(),
        input_schema: json!({ "type": "object" }),
    };
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_tools(vec![tool.clone()]),
        harness.client(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("read README"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.contains(&AgentEvent::ToolCallFinished {
        turn: 1,
        tool_call: AgentToolCall {
            id: "tool_1".into(),
            name: "Read".into(),
            raw_arguments: json!({ "path": "README.md" }).to_string().into(),
        },
    }));
    assert_eq!(
        context.messages()[1],
        AgentMessage::assistant(
            Vec::new(),
            vec![AgentToolCall {
                id: "tool_1".into(),
                name: "Read".into(),
                raw_arguments: json!({ "path": "README.md" }).to_string().into(),
            }],
            StopReason::ToolUse,
        )
    );
    assert_eq!(harness.requests()[0].tools, vec![tool]);
}

async fn assert_runtime_rejects_unsupported_capability(
    config: AgentConfig,
    harness: &FakeHarness,
    message: AgentMessage,
    expected_substring: &str,
    expectation: &str,
) {
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();
    let error = runtime
        .run_turn(&mut context, message)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect_err(expectation);

    assert!(matches!(
        error,
        AgentRuntimeError::Model(AiError::Configuration { message: _ })
    ));
    assert!(
        error.to_string().contains(expected_substring),
        "expected {expected_substring:?}, got {error}"
    );
    assert!(
        harness.requests().is_empty(),
        "request should not reach provider"
    );
}

#[tokio::test]
async fn runtime_rejects_tools_when_model_lacks_tools_before_request() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let tool = ToolSpec::string_arg("Read", "read file", "path", "file path");
    let config = AgentConfig::for_model(model_with_capabilities(ModelCapabilities::chat()))
        .with_tools(vec![tool]);

    assert_runtime_rejects_unsupported_capability(
        config,
        &harness,
        AgentMessage::user_text("read README"),
        "does not support tools",
        "unsupported tools should fail before provider request",
    )
    .await;
}

#[tokio::test]
async fn runtime_rejects_image_content_when_model_lacks_images_before_request() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let config = AgentConfig::for_model(harness.model());

    assert_runtime_rejects_unsupported_capability(
        config,
        &harness,
        AgentMessage::user_content(vec![Content::Image {
            mime_type: "image/png".into(),
            data: neo_agent_core::ImageRef::Url("https://example.test/cat.png".into()),
        }]),
        "does not support image input",
        "unsupported images should fail before provider request",
    )
    .await;
}

#[tokio::test]
async fn runtime_rejects_reasoning_selection_when_model_lacks_reasoning_before_request() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let mut config = AgentConfig::for_model(harness.model());
    config.reasoning = ReasoningSelection::Effort {
        effort: ReasoningEffort::low(),
    };

    assert_runtime_rejects_unsupported_capability(
        config,
        &harness,
        AgentMessage::user_text("think lightly"),
        "does not support reasoning",
        "unsupported reasoning should fail before provider request",
    )
    .await;
}

#[tokio::test]
async fn runtime_rejects_unsupported_reasoning_selection_before_request() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let mut config = AgentConfig::for_model(model_with_capabilities(ModelCapabilities {
        reasoning: ReasoningCapability::Effort {
            values: vec![ReasoningEffort::high()],
            disable_supported: true,
        },
        ..ModelCapabilities::tool_chat()
    }));
    config.reasoning = ReasoningSelection::BudgetTokens {
        budget_tokens: 8192,
    };
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();

    let error = runtime
        .run_turn(&mut context, AgentMessage::user_text("think with a budget"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect_err("unsupported reasoning selection should fail before provider request");
    let message = error.to_string();

    assert!(matches!(
        error,
        AgentRuntimeError::Model(AiError::Configuration { message: _ })
    ));
    assert!(
        message.contains("model capability-test/capability-test-model"),
        "error should identify the active provider/model: {message}"
    );
    assert!(
        message.contains("BudgetTokens"),
        "error should include the unsupported selection: {message}"
    );
    assert!(
        message.contains("Effort"),
        "error should include the model reasoning capability: {message}"
    );
    assert!(
        harness.requests().is_empty(),
        "request should not reach provider"
    );
}

#[tokio::test]
async fn runtime_passes_reasoning_selection_into_chat_request_options() {
    let harness = FakeHarness::from_events([AiStreamEvent::MessageEnd {
        stop_reason: neo_ai::StopReason::EndTurn,
        usage: None,
    }]);
    let mut config = AgentConfig::for_model(model_with_capabilities(ModelCapabilities {
        reasoning: ReasoningCapability::Effort {
            values: vec![ReasoningEffort::try_from("UltraMax").expect("custom effort")],
            disable_supported: true,
        },
        ..ModelCapabilities::tool_chat()
    }));
    config.reasoning = ReasoningSelection::Effort {
        effort: ReasoningEffort::try_from("UltraMax").expect("custom effort"),
    };
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("think lightly"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(
        harness.requests()[0].options.reasoning,
        ReasoningSelection::Effort {
            effort: ReasoningEffort::try_from("UltraMax").expect("custom effort"),
        }
    );
}

#[tokio::test]
async fn runtime_streams_thinking_events_and_persists_thinking_content() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_thinking".to_owned(),
        },
        AiStreamEvent::ThinkingStart {
            id: "thinking_1".to_owned(),
        },
        AiStreamEvent::ThinkingDelta {
            text: "Checked ".to_owned(),
        },
        AiStreamEvent::ThinkingDelta {
            text: "the plan.".to_owned(),
        },
        AiStreamEvent::ThinkingEnd {
            signature: Some("sig-1".into()),
            redacted: false,
        },
        AiStreamEvent::TextDelta {
            text: "final answer".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("think"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.contains(&AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking_1".to_owned(),
    }));
    assert!(events.contains(&AgentEvent::ThinkingDelta {
        turn: 1,
        text: "Checked ".to_owned(),
    }));
    assert!(events.contains(&AgentEvent::ThinkingDelta {
        turn: 1,
        text: "the plan.".to_owned(),
    }));
    assert!(events.contains(&AgentEvent::ThinkingFinished {
        turn: 1,
        signature: Some("sig-1".into()),
        redacted: false,
    }));
    assert_eq!(
        context.messages()[1],
        AgentMessage::assistant(
            [
                Content::thinking("Checked the plan.", Some("sig-1".into()), false),
                Content::text("final answer"),
            ],
            Vec::new(),
            StopReason::EndTurn,
        )
    );
}

#[tokio::test]
async fn runtime_preserves_multiple_thinking_parts_and_text_order() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_multi_thinking".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "intro ".to_owned(),
        },
        AiStreamEvent::ThinkingStart {
            id: "thinking_1".to_owned(),
        },
        AiStreamEvent::ThinkingDelta {
            text: "first thought".to_owned(),
        },
        AiStreamEvent::ThinkingEnd {
            signature: Some("sig-1".into()),
            redacted: false,
        },
        AiStreamEvent::ThinkingStart {
            id: "thinking_2".to_owned(),
        },
        AiStreamEvent::ThinkingDelta {
            text: "second thought".to_owned(),
        },
        AiStreamEvent::ThinkingEnd {
            signature: Some("sig-2".into()),
            redacted: true,
        },
        AiStreamEvent::TextDelta {
            text: "outro".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("think twice"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(
        context.messages()[1],
        AgentMessage::assistant(
            [
                Content::text("intro "),
                Content::thinking("first thought", Some("sig-1".into()), false),
                Content::thinking("second thought", Some("sig-2".into()), true),
                Content::text("outro"),
            ],
            Vec::new(),
            StopReason::EndTurn,
        )
    );
}

#[tokio::test]
async fn runtime_sends_persisted_thinking_content_back_to_model() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_thinking".to_owned(),
            },
            AiStreamEvent::ThinkingStart {
                id: "thinking_1".to_owned(),
            },
            AiStreamEvent::ThinkingDelta {
                text: "local reasoning summary".to_owned(),
            },
            AiStreamEvent::ThinkingEnd {
                signature: Some("sig-1".into()),
                redacted: false,
            },
            AiStreamEvent::TextDelta {
                text: "answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_followup".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "followup".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("think"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("first turn should succeed");
    runtime
        .run_turn(&mut context, AgentMessage::user_text("continue"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("second turn should succeed");

    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    let assistant_message = requests[1]
        .messages
        .iter()
        .find(|message| matches!(message, neo_ai::ChatMessage::Assistant { .. }))
        .expect("previous assistant message should be sent");
    assert_eq!(
        assistant_message,
        &neo_ai::ChatMessage::Assistant {
            content: vec![
                neo_ai::ContentPart::Thinking {
                    text: "local reasoning summary".to_owned(),
                    signature: Some("sig-1".into()),
                    redacted: false,
                },
                neo_ai::ContentPart::Text {
                    text: "answer".to_owned(),
                },
            ],
            tool_calls: Vec::new(),
        }
    );
}

#[tokio::test]
async fn runtime_can_disable_persisted_thinking_replay() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_thinking".to_owned(),
            },
            AiStreamEvent::ThinkingStart {
                id: "thinking_1".to_owned(),
            },
            AiStreamEvent::ThinkingDelta {
                text: "local reasoning summary".to_owned(),
            },
            AiStreamEvent::ThinkingEnd {
                signature: Some("sig-1".into()),
                redacted: false,
            },
            AiStreamEvent::TextDelta {
                text: "answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_followup".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "followup".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.replay_reasoning = false;
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("think"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("first turn should succeed");
    runtime
        .run_turn(&mut context, AgentMessage::user_text("continue"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("second turn should succeed");

    let requests = harness.requests();
    let assistant_message = requests[1]
        .messages
        .iter()
        .find(|message| matches!(message, neo_ai::ChatMessage::Assistant { .. }))
        .expect("previous assistant message should be sent");
    assert_eq!(
        assistant_message,
        &neo_ai::ChatMessage::Assistant {
            content: vec![neo_ai::ContentPart::Text {
                text: "answer".to_owned(),
            }],
            tool_calls: Vec::new(),
        }
    );
    assert!(!requests[1].options.replay_reasoning);
}

#[tokio::test]
async fn runtime_compaction_estimate_ignores_unsent_thinking_content() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_after_thinking".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "kept".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(32, 1)),
        harness.client(),
    );
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::assistant(
        [
            Content::thinking("x".repeat(4_000), None, false),
            Content::text("short text"),
        ],
        Vec::new(),
        StopReason::EndTurn,
    ));

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("next"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. })),
        "thinking content is not sent back to the provider and should not trigger compaction"
    );
}

#[tokio::test]
async fn runtime_can_compact_again_after_context_grows_past_threshold() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        // Compaction summary call for the first compaction
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_compact_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "## Current Focus\nFirst compaction.".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        // Compaction summary call for the second compaction
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_compact_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "## Current Focus\nSecond compaction.".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "third answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(4, 1)),
        harness.client(),
    );
    let mut context = AgentContext::new();
    let mut compactions = Vec::new();

    for prompt in [
        "first long prompt that seeds compaction",
        "second long prompt that triggers compaction",
        "third long prompt that should trigger compaction again",
    ] {
        let events = runtime
            .run_turn(&mut context, AgentMessage::user_text(prompt))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("turn should succeed");
        compactions.extend(events.into_iter().filter_map(|event| match event {
            AgentEvent::CompactionApplied { summary } => Some(summary),
            _ => None,
        }));
    }

    assert_eq!(
        compactions.len(),
        2,
        "context should compact again after later turns grow past the threshold. Messages: {:?}",
        context.messages().len()
    );
    // After the second compaction, the context should contain:
    // 1. The injected compaction summary system message
    // 2. The third user prompt
    // 3. The third assistant response
    assert_eq!(context.messages().len(), 3);
    assert!(matches!(
        context.messages().first(),
        Some(AgentMessage::System { .. })
    ));
    assert_eq!(context.compaction_summary(), compactions.last());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn runtime_emits_compaction_lifecycle_events_before_applying_summary() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        // Compaction summary call (no tools, returns structured summary text)
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_compact".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "## Current Focus\nWorking on compaction test.".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(4, 1)),
        harness.client(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("first long prompt that seeds compaction"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("first turn should succeed");

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("second long prompt that triggers compaction"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("second turn should succeed");

    let lifecycle = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::CompactionStarted {
                reason,
                tokens_before,
                message_count,
            } => Some(format!("start:{reason:?}:{tokens_before}:{message_count}")),
            AgentEvent::CompactionProgress { phase, percent } => {
                Some(format!("progress:{phase:?}:{percent}"))
            }
            AgentEvent::CompactionApplied { summary } => Some(format!(
                "applied:{}:{}",
                summary.first_kept_message_index, summary.tokens_before
            )),
            _ => None,
        })
        .collect::<Vec<_>>();

    // Verify the lifecycle starts at 0%, goes through the visible phases, and
    // finishes smoothly at 100% instead of jumping from ~80% to done.
    assert_eq!(lifecycle.first(), Some(&"start:Threshold:29:3".to_owned()));
    assert!(lifecycle.contains(&"progress:Estimating:0".to_owned()));
    assert!(lifecycle.contains(&"progress:SelectingBoundary:15".to_owned()));
    assert!(lifecycle.contains(&"progress:Summarizing:15".to_owned()));
    assert!(
        lifecycle
            .iter()
            .any(|e| e.starts_with("progress:Summarizing:") && e != "progress:Summarizing:15"),
        "Summarizing should make progress beyond its starting percent: {lifecycle:?}"
    );
    assert_eq!(
        lifecycle.iter().rfind(|e| e.starts_with("progress:")),
        Some(&"progress:Applying:100".to_owned()),
        "last progress should reach 100%: {lifecycle:?}"
    );
    assert!(lifecycle.contains(&"applied:2:29".to_owned()));

    // Percentages must be non-decreasing across CompactionProgress events.
    let percents: Vec<u8> = lifecycle
        .iter()
        .filter_map(|e| {
            e.strip_prefix("progress:").and_then(|rest| {
                let parts: Vec<&str> = rest.split(':').collect();
                parts.last().and_then(|p| p.parse().ok())
            })
        })
        .collect();
    assert!(
        percents.windows(2).all(|w| w[0] <= w[1]),
        "progress percents should be monotonic: {percents:?}"
    );
}

#[tokio::test]
async fn runtime_context_window_events_share_budget_snapshot() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history ".repeat(4_000)));
    let mut config = AgentConfig::for_model(harness.model())
        .with_system_prompt("system ".repeat(1_000))
        .with_compaction(CompactionSettings::new(usize::MAX, 4));
    config.model.capabilities.max_context_tokens = Some(200_000);

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("continue"),
    )
    .await;

    let update = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ContextWindowUpdated {
                used_tokens,
                projected_tokens,
                trigger_tokens,
                ..
            } => Some((*used_tokens, *projected_tokens, *trigger_tokens)),
            _ => None,
        })
        .expect("context update");
    assert!(update.0 > 0);
    assert!(update.1.is_some());
    assert!(update.2.is_some());
}

#[tokio::test]
async fn runtime_compacts_before_model_call_when_resume_exceeds_window() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "summary".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "resumed".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history ".repeat(40_000)));
    context.append_message(AgentMessage::assistant(
        [Content::text("previous answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let mut config =
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(1, 1));
    config.model.capabilities.max_context_tokens = Some(32_000);

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("continue"),
    )
    .await;

    let compaction = events
        .iter()
        .position(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
        .expect("compaction");
    let assistant = events
        .iter()
        .rposition(|event| {
            matches!(
                event,
                AgentEvent::MessageAppended {
                    message: AgentMessage::Assistant { .. }
                }
            )
        })
        .expect("assistant");
    assert!(compaction < assistant);
}

#[tokio::test]
async fn runtime_overflow_records_observed_window_and_retries_once() {
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::ContextOverflow {
            message: "too many tokens".to_owned(),
        })],
        vec![
            Ok(AiStreamEvent::MessageStart {
                id: "summary".to_owned(),
            }),
            Ok(AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
        vec![Err(AiError::RateLimit {
            message: "retry compacted request".to_owned(),
            retry_after: Some(Duration::ZERO),
        })],
        vec![
            Ok(AiStreamEvent::MessageStart {
                id: "retry".to_owned(),
            }),
            Ok(AiStreamEvent::TextDelta {
                text: "recovered".to_owned(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history"));
    context.append_message(AgentMessage::assistant(
        [Content::text("old answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let mut config = AgentConfig::for_model(harness.model())
        .with_system_prompt("system ".repeat(4_000))
        .with_compaction(CompactionSettings::new(usize::MAX, 1));
    config.max_retries = 1;
    config.model.capabilities.max_context_tokens = Some(200_000);

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("continue"),
    )
    .await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        serde_json::to_value(&requests[2]).expect("serialize compacted request"),
        serde_json::to_value(&requests[3]).expect("serialize retried compacted request")
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
    );
    let observed_max = events.iter().find_map(|event| match event {
        AgentEvent::ContextWindowUpdated {
            max_tokens: Some(max_tokens),
            source: Some(neo_agent_core::ContextWindowSource::ObservedOverflow),
            ..
        } => Some(*max_tokens),
        _ => None,
    });
    assert!(observed_max.is_some_and(|max| max > 1_000));
    assert!(events.contains(&AgentEvent::RetrySucceeded {
        turn: 1,
        retries_used: 1,
    }));
}

#[tokio::test]
async fn retry_lifecycle_survives_context_overflow_recovery() {
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::Transport {
            message: "connection reset".to_owned(),
        })],
        vec![Err(AiError::ContextOverflow {
            message: "too many tokens".to_owned(),
        })],
        vec![
            Ok(AiStreamEvent::MessageStart {
                id: "summary".to_owned(),
            }),
            Ok(AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
        vec![
            Ok(AiStreamEvent::MessageStart {
                id: "recovered".to_owned(),
            }),
            Ok(AiStreamEvent::TextDelta {
                text: "recovered".to_owned(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history"));
    context.append_message(AgentMessage::assistant(
        [Content::text("old answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let mut config = AgentConfig::for_model(harness.model())
        .with_system_prompt("system ".repeat(4_000))
        .with_compaction(CompactionSettings::new(usize::MAX, 1));
    config.max_retries = 1;
    config.model.capabilities.max_context_tokens = Some(200_000);

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("continue"),
    )
    .await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        serde_json::to_value(&requests[0]).expect("serialize initial request"),
        serde_json::to_value(&requests[1]).expect("serialize ordinary retry")
    );
    let lifecycle = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RetryScheduled { retry: 1, .. } => Some("retry_scheduled"),
            AgentEvent::RetryStarted { retry: 1, .. } => Some("retry_started"),
            AgentEvent::CompactionApplied { .. } => Some("compaction_applied"),
            AgentEvent::RetryResumed { retry: 1, .. } => Some("retry_resumed"),
            AgentEvent::RetrySucceeded {
                retries_used: 1, ..
            } => Some("retry_succeeded"),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lifecycle,
        vec![
            "retry_scheduled",
            "retry_started",
            "compaction_applied",
            "retry_resumed",
            "retry_succeeded",
        ]
    );
}

#[tokio::test]
async fn runtime_does_not_compact_mid_parallel_tool_group() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "a".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "a".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "b".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "b".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "c".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "c".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "summary".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "after tools".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = runtime_with_large_tool(&harness);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("use tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let last_tool_result = events
        .iter()
        .rposition(|event| {
            matches!(
                event,
                AgentEvent::MessageAppended {
                    message: AgentMessage::ToolResult { .. }
                }
            )
        })
        .expect("tool result");
    let first_compaction = events
        .iter()
        .position(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
        .expect("compaction");
    assert!(first_compaction > last_tool_result);
}

#[tokio::test]
async fn runtime_compacts_after_parallel_tool_group_before_followup() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "a".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "a".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "b".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "b".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "c".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "c".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "summary".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "after compaction".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = runtime_with_large_tool(&harness);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("use tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let compaction = events
        .iter()
        .position(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
        .expect("compaction");
    let second_assistant = events
        .iter()
        .rposition(|event| {
            matches!(
                event,
                AgentEvent::MessageAppended {
                    message: AgentMessage::Assistant { .. }
                }
            )
        })
        .expect("assistant");
    assert!(compaction < second_assistant);
}

#[tokio::test]
async fn runtime_compaction_keeps_valid_tool_result_boundaries() {
    let harness = FakeHarness::from_turns([
        // Compaction summary call
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_compact".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "## Current Focus\nInspecting files.".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        // Actual turn response
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_after_compaction".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "after compaction".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(1, 3)),
        harness.client(),
    );
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("inspect"));
    context.append_message(AgentMessage::assistant(
        [],
        [
            AgentToolCall {
                id: "tool_1".into(),
                name: "Read".into(),
                raw_arguments: json!({ "path": "a.rs" }).to_string().into(),
            },
            AgentToolCall {
                id: "tool_2".into(),
                name: "List".into(),
                raw_arguments: json!({ "path": "src" }).to_string().into(),
            },
        ],
        StopReason::ToolUse,
    ));
    context.append_message(AgentMessage::tool_result(
        "tool_1",
        "Read",
        [Content::text("large content")],
        false,
    ));
    context.append_message(AgentMessage::tool_result(
        "tool_2",
        "List",
        [Content::text("file list")],
        false,
    ));

    runtime
        .run_turn(&mut context, AgentMessage::user_text("continue"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let request = harness.requests().pop().expect("model request");
    assert!(
        !matches!(
            request.messages.first(),
            Some(neo_ai::ChatMessage::ToolResult { .. })
        ),
        "compaction must not keep orphaned tool results at the start of replay"
    );
    // The first message is now either the compaction summary system message or
    // the user prompt — never an orphaned tool result.
    assert!(matches!(
        request.messages.first(),
        Some(neo_ai::ChatMessage::System { .. } | neo_ai::ChatMessage::User { .. })
    ));
}

#[tokio::test]
async fn runtime_external_cancellation_before_model_emits_cancelled_barriers() {
    let harness = FakeHarness::from_events([]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    cancel.cancel();

    let events = runtime
        .run_turn_with_cancel(
            &mut context,
            AgentMessage::user_text("already cancelled"),
            cancel,
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("cancel event");

    assert_eq!(
        events,
        vec![
            AgentEvent::RunStarted { turn: 1 },
            AgentEvent::MessageAppended {
                message: AgentMessage::user_text("already cancelled"),
            },
            AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            },
            AgentEvent::ContextWindowUpdated {
                turn: 1,
                used_tokens: 6,
                projected_tokens: Some(6),
                max_tokens: None,
                trigger_tokens: None,
                remaining_tokens: None,
                source: Some(neo_agent_core::ContextWindowSource::MissingModelWindow),
            },
            AgentEvent::RunFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            },
        ]
    );
    assert!(harness.requests().is_empty());
}

#[tokio::test]
async fn runtime_resumed_cancelled_turn_accepts_followup_prompt() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_after_resume".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "resumed".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client());
    let mut context = AgentContext::from_replay(
        [
            AgentEvent::RunStarted { turn: 1 },
            AgentEvent::MessageAppended {
                message: AgentMessage::user_text("cancel this turn"),
            },
            AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            },
            AgentEvent::RunFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            },
        ]
        .iter(),
    );

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("continue after resume"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("follow-up turn should run after replayed cancellation");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { text, .. } if text == "resumed"
    )));
    assert!(events.contains(&AgentEvent::TurnFinished {
        turn: 2,
        stop_reason: StopReason::EndTurn,
    }));
    assert_eq!(harness.requests().len(), 1);
}

#[tokio::test]
async fn runtime_executes_tool_call_and_continues_until_end_turn() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "neo" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "tool said: neo".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call echo"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(events.contains(&AgentEvent::ToolCallFinished {
        turn: 1,
        tool_call: AgentToolCall {
            id: "tool_1".into(),
            name: "echo".into(),
            raw_arguments: json!({ "text": "neo" }).to_string().into(),
        },
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result("tool_1", "echo", vec![Content::text("neo")], false)
    );
    assert_eq!(
        context.messages()[3],
        AgentMessage::assistant(
            vec![Content::text("tool said: neo")],
            Vec::new(),
            StopReason::EndTurn
        )
    );
    assert_eq!(harness.requests().len(), 2);
    assert!(matches!(
        harness.requests()[1].messages.last(),
        Some(neo_ai::ChatMessage::ToolResult { tool_call_id, .. }) if tool_call_id == "tool_1"
    ));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::RunFinished { .. }))
            .count(),
        1
    );
    assert!(events.contains(&AgentEvent::MessageFinished {
        turn: 1,
        id: "msg_1".to_owned(),
        stop_reason: StopReason::ToolUse,
    }));
    assert!(events.contains(&AgentEvent::MessageFinished {
        turn: 2,
        id: "msg_2".to_owned(),
        stop_reason: StopReason::EndTurn,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 2,
            stop_reason: StopReason::EndTurn,
        })
    );
}

#[tokio::test]
async fn runtime_emits_todo_update_only_for_writes() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_write".to_owned(),
                name: "TodoList".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_write".to_owned(),
                raw_arguments: json!({
                    "todos": [{ "title": "Read code", "status": "in_progress" }]
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_read".to_owned(),
                name: "TodoList".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_read".to_owned(),
                raw_arguments: json!({}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let config = AgentConfig::for_model(harness.model());
    let tools = ToolRegistry::with_builtin_tools_and_todos(Arc::clone(&config.todos));
    let runtime = AgentRuntime::with_tools(config, harness.client(), tools);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("track todos"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("todo tool loop should succeed");

    let todo_events = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TodoUpdated { todos, .. } => Some(todos),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(todo_events.len(), 1);
    assert_eq!(todo_events[0][0].title, "Read code");
    assert_eq!(todo_events[0][0].status, "in_progress");
    assert_eq!(context.todos(), todo_events[0].as_slice());
    assert!(
        context.messages().iter().any(|message| {
            matches!(
                message,
                AgentMessage::ToolResult { tool_call_id, content, .. }
                    if tool_call_id.as_ref() == "tool_read"
                        && content.iter().any(|part| matches!(part, Content::Text { text } if text.contains("[in_progress] Read code")))
            )
        }),
        "read-mode tool result should include current todos"
    );
}

#[tokio::test]
async fn runtime_emits_empty_todo_update_for_clear() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_clear".to_owned(),
                name: "TodoList".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_clear".to_owned(),
                raw_arguments: json!({ "todos": [] }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "cleared".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let config = AgentConfig::for_model(harness.model());
    config
        .todos
        .lock()
        .expect("todo state")
        .push(TodoEventData {
            title: "Old".to_owned(),
            status: "done".to_owned(),
        });
    let tools = ToolRegistry::with_builtin_tools_and_todos(Arc::clone(&config.todos));
    let runtime = AgentRuntime::with_tools(config, harness.client(), tools);
    let mut context = AgentContext::from_replay(
        [AgentEvent::TodoUpdated {
            turn: 0,
            todos: vec![TodoEventData {
                title: "Old".to_owned(),
                status: "done".to_owned(),
            }],
        }]
        .iter(),
    );

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("clear todos"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("todo clear should succeed");

    assert!(events.iter().any(|event| {
        matches!(event, AgentEvent::TodoUpdated { todos, .. } if todos.is_empty())
    }));
    assert!(context.todos().is_empty());
}

#[tokio::test]
async fn stream_first_event_timeout_retries_same_request() {
    let harness = DelayedHarness::from_turns([
        vec![DelayedStep::Delay(Duration::from_secs(2))],
        vec![
            DelayedStep::Event(AiStreamEvent::MessageStart {
                id: "retry".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::TextDelta {
                text: "complete".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.first_event_timeout_secs = 1;
    config.stream_idle_timeout_secs = 0;
    config.max_retries = 1;
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("retry silence"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("retry should succeed");

    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        serde_json::to_value(&requests[0]).expect("serialize first request"),
        serde_json::to_value(&requests[1]).expect("serialize retry request")
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled {
            retry: 1,
            error_code,
            message,
            ..
        } if error_code == "provider.transport_error"
            && message.contains("first model stream event")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::RetrySucceeded {
            retries_used: 1,
            ..
        }
    )));
}

#[tokio::test]
async fn stream_idle_timeout_retries_and_discards_partial_attempt() {
    let harness = DelayedHarness::from_turns([
        vec![
            DelayedStep::Event(AiStreamEvent::MessageStart {
                id: "discarded".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::TextDelta {
                text: "discarded partial".to_owned(),
            }),
            DelayedStep::Delay(Duration::from_secs(2)),
        ],
        vec![
            DelayedStep::Event(AiStreamEvent::MessageStart {
                id: "winning".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::TextDelta {
                text: "winning answer".to_owned(),
            }),
            DelayedStep::Event(AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.first_event_timeout_secs = 0;
    config.stream_idle_timeout_secs = 1;
    config.max_retries = 1;
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("retry idle stream"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("retry should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled {
            error_code,
            message,
            ..
        } if error_code == "provider.transport_error"
            && message.contains("model stream idle for 1s")
    )));
    let appended = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended {
                message: message @ AgentMessage::Assistant { .. },
            } => Some(message.text()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(appended, ["winning answer"]);
    assert!(!context.messages().iter().any(|message| {
        matches!(message, AgentMessage::Assistant { .. })
            && message.text().contains("discarded partial")
    }));
    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        serde_json::to_value(&requests[0]).expect("serialize first request"),
        serde_json::to_value(&requests[1]).expect("serialize retry request")
    );
}

#[tokio::test]
async fn stream_timeout_zero_waits_until_cancelled() {
    let harness = DelayedHarness::new(vec![DelayedStep::Delay(Duration::from_secs(5))]);
    let mut config = AgentConfig::for_model(harness.model());
    config.first_event_timeout_secs = 0;
    config.stream_idle_timeout_secs = 0;
    config.max_retries = 0;
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("cancel silent stream"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("turn should start promptly")
            .expect("turn stream should remain open")
            .expect("turn event should be ok");
        let turn_started = matches!(event, AgentEvent::TurnStarted { .. });
        events.push(event);
        if turn_started {
            break;
        }
    }
    assert!(
        timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err(),
        "zero timeouts must leave the pending model stream silent"
    );

    cancel.cancel();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("silent stream cancellation should not stall")
    {
        events.push(event.expect("cancelled stream should remain in-band"));
    }
    drop(stream);

    assert!(events.contains(&AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: StopReason::Cancelled,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled { .. }
            | AgentEvent::RetryStarted { .. }
            | AgentEvent::RetryExhausted { .. }
    )));
}

#[tokio::test]
async fn stream_retries_transport_error() {
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::Transport {
            message: "eof".into(),
        })],
        vec![
            Ok(AiStreamEvent::MessageStart { id: "b".into() }),
            Ok(AiStreamEvent::TextDelta {
                text: "complete".into(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 1;
    let mut context = AgentContext::new();

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("retry"),
    )
    .await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        serde_json::to_value(&requests[0]).expect("serialize first request"),
        serde_json::to_value(&requests[1]).expect("serialize replayed request")
    );

    let lifecycle = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TurnStarted { .. } => Some("turn_started"),
            AgentEvent::RetryScheduled { .. } => Some("retry_scheduled"),
            AgentEvent::RetryStarted { .. } => Some("retry_started"),
            AgentEvent::RetryResumed { .. } => Some("retry_resumed"),
            AgentEvent::RetrySucceeded { .. } => Some("retry_succeeded"),
            AgentEvent::TurnFinished { .. } => Some("turn_finished"),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        lifecycle,
        [
            "turn_started",
            "retry_scheduled",
            "retry_started",
            "retry_resumed",
            "retry_succeeded",
            "turn_finished",
        ]
    );
    let resumed = events
        .iter()
        .position(|event| matches!(event, AgentEvent::RetryResumed { .. }))
        .expect("retry should resume on its first valid event");
    assert!(matches!(
        events.get(resumed + 1),
        Some(AgentEvent::MessageStarted { id, .. }) if id == "b"
    ));

    let scheduled = events
        .iter()
        .find(|event| matches!(event, AgentEvent::RetryScheduled { .. }))
        .expect("retry should be scheduled");
    assert!(matches!(
        scheduled,
        AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 1,
            delay_ms: 500..=625,
            error_code,
            message,
        } if error_code == "provider.transport_error" && message == "transport error: eof"
    ));
}

#[tokio::test]
async fn retry_does_not_append_failed_attempt() {
    let harness = FakeHarness::from_result_turns([
        vec![
            Ok(AiStreamEvent::MessageStart { id: "a".into() }),
            Ok(AiStreamEvent::TextDelta {
                text: "partial".into(),
            }),
        ],
        vec![
            Ok(AiStreamEvent::MessageStart { id: "b".into() }),
            Ok(AiStreamEvent::TextDelta {
                text: "complete".into(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 1;
    let mut context = AgentContext::new();

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("retry partial"),
    )
    .await;

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TextDelta { text, .. } if text == "partial"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled {
            error_code,
            message,
            ..
        } if error_code == "provider.transport_error"
            && message == "transport error: model stream ended before MessageEnd"
    )));
    let appended = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended {
                message: message @ AgentMessage::Assistant { .. },
            } => Some(message.text()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(appended, ["complete"]);
    assert!(
        !context
            .messages()
            .iter()
            .any(|message| matches!(message, AgentMessage::Assistant { .. })
                && message.text().contains("partial"))
    );
}

#[tokio::test]
async fn retry_budget_zero_emits_exhausted_error() {
    let harness = FakeHarness::from_result_turns([vec![Err(AiError::Transport {
        message: "provider failed".into(),
    })]]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 0;
    let mut context = AgentContext::new();

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("fail"),
    )
    .await;

    assert!(events.contains(&AgentEvent::Error {
        turn: 1,
        message: "transport error: provider failed".to_owned(),
        code: Some("provider.transport_error".to_owned()),
        retry_after: None,
    }));
    assert!(events.contains(&AgentEvent::RetryExhausted {
        turn: 1,
        retries_used: 0,
        error_code: "provider.transport_error".to_owned(),
        message: "transport error: provider failed".to_owned(),
    }));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled { .. } | AgentEvent::RetryStarted { .. }
    )));
    assert!(events.contains(&AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: StopReason::Error,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Error,
        })
    );
    assert_eq!(harness.requests().len(), 1);
}

#[tokio::test]
async fn retry_exhaustion_reports_final_error() {
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::RateLimit {
            message: "busy".into(),
            retry_after: Some(Duration::ZERO),
        })],
        vec![Err(AiError::Server {
            status: 503,
            message: "still busy".into(),
            retry_after: Some(Duration::ZERO),
        })],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 1;
    let mut context = AgentContext::new();

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("retry once"),
    )
    .await;

    assert!(events.contains(&AgentEvent::RetryExhausted {
        turn: 1,
        retries_used: 1,
        error_code: "provider.server_error".to_owned(),
        message: "server error (503): still busy".to_owned(),
    }));
    assert!(events.contains(&AgentEvent::Error {
        turn: 1,
        message: "server error (503): still busy".to_owned(),
        code: Some("provider.server_error".to_owned()),
        retry_after: Some(0),
    }));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::RetryResumed { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::RetrySucceeded { .. }))
    );
    assert_eq!(harness.requests().len(), 2);
}

#[tokio::test]
async fn retry_does_not_retry_protocol_failure() {
    let harness = FakeHarness::from_result_turns([vec![Err(AiError::Protocol {
        message: "invalid frame".into(),
    })]]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 5;
    let mut context = AgentContext::new();

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("broken protocol"),
    )
    .await;

    assert!(events.contains(&AgentEvent::Error {
        turn: 1,
        message: "protocol error: invalid frame".to_owned(),
        code: Some("provider.protocol_error".to_owned()),
        retry_after: None,
    }));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryScheduled { .. }
            | AgentEvent::RetryStarted { .. }
            | AgentEvent::RetryResumed { .. }
            | AgentEvent::RetrySucceeded { .. }
            | AgentEvent::RetryExhausted { .. }
    )));
    assert_eq!(harness.requests().len(), 1);
}

#[tokio::test]
async fn retry_backoff_is_cancellable() {
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::Transport {
            message: "eof".into(),
        })],
        vec![Ok(AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        })],
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.max_retries = 1;
    let runtime = AgentRuntime::new(config, harness.client());
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("cancel retry"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    while let Some(event) = timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("retry lifecycle should not stall")
    {
        let event = event.expect("cancelled retry should remain in-band");
        let scheduled = matches!(event, AgentEvent::RetryScheduled { .. });
        events.push(event);
        if scheduled {
            cancel.cancel();
        }
    }
    drop(stream);

    assert!(events.contains(&AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: StopReason::Cancelled,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::RetryStarted { .. } | AgentEvent::RetryResumed { .. }
    )));
    assert_eq!(harness.requests().len(), 1);
}

#[tokio::test]
async fn runtime_stops_on_tool_use_with_empty_tool_calls() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_empty_tools".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_should_not_run".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "followup".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        ToolRegistry::new(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("try a tool"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("empty tool use should fail closed in-band");

    assert!(events.contains(&AgentEvent::Error {
        turn: 1,
        message: "Provider reported tool calls but emitted no structured tool calls".to_owned(),
        code: None,
        retry_after: None,
    }));
    assert!(events.contains(&AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: StopReason::Error,
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Error,
        })
    );
    assert_eq!(harness.requests().len(), 1);
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::MessageStarted { id, .. } if id == "msg_should_not_run"
    )));
}

#[tokio::test]
async fn runtime_returns_tool_errors_to_model_for_retry_instead_of_aborting() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "fallible".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "bad": true }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "retry noted".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(FallibleTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call fallible"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool error should be returned to the model");

    assert_eq!(harness.requests().len(), 2);
    assert!(events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionFinished {
                id,
                result: ToolResult { is_error: true, content, .. },
                ..
            } if id == "tool_1" && content.contains("invalid input for fallible")
        )
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "fallible",
            vec![Content::text("invalid input for fallible: expected text")],
            true
        )
    );
    assert_eq!(
        context.messages()[3],
        AgentMessage::assistant(
            vec![Content::text("retry noted")],
            Vec::new(),
            StopReason::EndTurn
        )
    );
    assert!(matches!(
        harness.requests()[1].messages.last(),
        Some(neo_ai::ChatMessage::ToolResult {
            tool_call_id,
            is_error: true,
            ..
        }) if tool_call_id == "tool_1"
    ));
}

#[tokio::test]
async fn runtime_cancels_in_flight_tool_execution_and_finishes_run() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "never".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(NeverTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("call never"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("tool start should arrive promptly")
            .expect("event before cancellation")
            .expect("event should be ok");
        let should_cancel = matches!(
            event,
            AgentEvent::ToolExecutionStarted { ref id, .. } if id == "tool_1"
        );
        events.push(event);
        if should_cancel {
            cancel.cancel();
            break;
        }
    }
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled tool run should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "never".to_owned(),
        result: ToolResult::error("tool execution cancelled"),
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert_eq!(context.messages().len(), 2);
}

#[tokio::test]
async fn runtime_cancels_while_waiting_for_async_before_tool_hook() {
    let harness = echo_tool_harness("should not execute");
    let (hook_wait_sender, hook_wait_receiver) = oneshot::channel::<()>();
    let hook_wait_receiver = Arc::new(Mutex::new(Some(hook_wait_receiver)));
    let hook_started = Arc::new(Notify::new());
    let hook_started_for_hook = Arc::clone(&hook_started);
    let hook_wait_receiver_for_hook = Arc::clone(&hook_wait_receiver);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo)
            .with_async_before_tool_call(move |_call, _cancel| {
                let started = hook_started_for_hook.clone();
                let receiver = hook_wait_receiver_for_hook.clone();
                async move {
                    started.notify_one();
                    let wait = receiver
                        .lock()
                        .expect("receiver lock poisoned")
                        .take()
                        .expect("hook wait receiver should be present");
                    let _ = wait.await;
                    None
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    // The before-hook runs during batch authorization, before any
    // ToolExecutionStarted: cancel as soon as the hook starts, then drain.
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("call echo"),
        cancel.clone(),
    );
    let mut events = Vec::new();
    timeout(Duration::from_millis(250), hook_started.notified())
        .await
        .expect("async hook should start promptly");
    cancel.cancel();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled async hook should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);
    drop(hook_wait_sender);

    assert_async_hook_cancelled_cleanly(&events, &context);
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
}

#[tokio::test]
async fn runtime_cancels_while_waiting_for_async_after_tool_hook() {
    let harness = echo_tool_harness("executed");
    let (hook_wait_sender, hook_wait_receiver) = oneshot::channel::<()>();
    let hook_wait_receiver = Arc::new(Mutex::new(Some(hook_wait_receiver)));
    let hook_started = Arc::new(Notify::new());
    let hook_started_for_hook = Arc::clone(&hook_started);
    let hook_wait_receiver_for_hook = Arc::clone(&hook_wait_receiver);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo)
            .with_async_after_tool_call(move |_call, result, _cancel| {
                let started = hook_started_for_hook.clone();
                let receiver = hook_wait_receiver_for_hook.clone();
                async move {
                    started.notify_one();
                    let wait = receiver
                        .lock()
                        .expect("receiver lock poisoned")
                        .take()
                        .expect("hook wait receiver should be present");
                    let _ = wait.await;
                    result
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let events =
        cancel_after_async_tool_hook_starts(&runtime, &mut context, cancel, &hook_started).await;
    drop(hook_wait_sender);

    assert_async_hook_cancelled_cleanly(&events, &context);
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["executed".to_owned()]
    );
}

async fn cancel_after_async_tool_hook_starts(
    runtime: &AgentRuntime,
    context: &mut AgentContext,
    cancel: CancellationToken,
    hook_started: &Notify,
) -> Vec<AgentEvent> {
    let mut stream = runtime.run_turn_with_cancel(
        context,
        AgentMessage::user_text("call echo"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("tool start should arrive promptly")
            .expect("event before cancellation")
            .expect("event should be ok");
        let should_cancel = matches!(
            event,
            AgentEvent::ToolExecutionStarted { ref id, .. } if id == "tool_1"
        );
        events.push(event);
        if should_cancel {
            break;
        }
    }
    timeout(Duration::from_millis(250), hook_started.notified())
        .await
        .expect("async hook should start promptly");
    cancel.cancel();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled async hook should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    events
}

fn assert_async_hook_cancelled_cleanly(events: &[AgentEvent], context: &AgentContext) {
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("tool execution cancelled"),
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert_eq!(context.messages().len(), 2);
}

#[tokio::test]
async fn runtime_parallel_cancellation_finishes_all_started_tool_wrappers() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "fast" }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "never".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    tools.register(NeverTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();
    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("call parallel tools"),
        cancel.clone(),
    );
    let mut events = Vec::new();

    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("tool starts should arrive promptly")
            .expect("event before cancellation")
            .expect("event should be ok");
        let should_cancel = matches!(
            event,
            AgentEvent::ToolExecutionStarted { ref id, .. } if id == "tool_2"
        );
        events.push(event);
        if should_cancel {
            cancel.cancel();
            break;
        }
    }
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled parallel tool run should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("fast"),
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_2".to_owned(),
        name: "never".to_owned(),
        result: ToolResult::error("tool execution cancelled"),
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert_eq!(context.messages().len(), 2);
}

#[tokio::test]
async fn runtime_parallel_cancellation_does_not_start_later_tool_calls() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "first" }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "never".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let cancel = CancellationToken::new();
    let cancel_from_hook = cancel.clone();
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    tools.register(NeverTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_before_tool_call(move |call| {
                if call.id.as_ref() == "tool_1" {
                    cancel_from_hook.cancel();
                    Some(ToolResult::ok("first"))
                } else {
                    None
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn_with_cancel(
            &mut context,
            AgentMessage::user_text("call parallel tools"),
            cancel,
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("first"),
    }));
    assert!(!events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionStarted { id, .. } if id == "tool_2"
        )
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert_eq!(context.messages().len(), 2);
}

#[tokio::test]
async fn runtime_drains_queued_steering_before_followups() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "third".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_queue_modes(QueueMode::OneAtATime, QueueMode::All),
        harness.client(),
    );
    let mut context = AgentContext::new();
    context.queue_steering_message(AgentMessage::user_text("steer one"));
    context.queue_steering_message(AgentMessage::user_text("steer two"));
    context.queue_follow_up_message(AgentMessage::user_text("follow"));

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("queued run should succeed");

    let appended = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended { message } => Some(message.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        appended,
        vec![
            AgentMessage::user_text("start"),
            AgentMessage::user_text("steer one"),
            AgentMessage::assistant([Content::text("first")], Vec::new(), StopReason::EndTurn),
            AgentMessage::user_text("steer two"),
            AgentMessage::assistant([Content::text("second")], Vec::new(), StopReason::EndTurn),
            AgentMessage::user_text("follow"),
            AgentMessage::assistant([Content::text("third")], Vec::new(), StopReason::EndTurn),
        ]
    );
    assert_eq!(context.pending_steering_len(), 0);
    assert_eq!(context.pending_follow_up_len(), 0);
    assert_eq!(harness.requests().len(), 3);
    assert!(matches!(
        harness.requests()[0].messages.last(),
        Some(neo_ai::ChatMessage::User { content }) if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text == "steer one"
        )
    ));
    assert!(matches!(
        harness.requests()[1].messages.last(),
        Some(neo_ai::ChatMessage::User { content }) if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text == "steer two"
        )
    ));
    assert!(matches!(
        events.last(),
        Some(AgentEvent::RunFinished {
            turn: 3,
            stop_reason: StopReason::EndTurn,
        })
    ));
}

#[tokio::test]
async fn runtime_applies_context_append_transform_before_model_request() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "trimmed".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_context_append_transform(|messages| {
            vec![AgentMessage::system_reminder(format!(
                "append-only transform saw {} messages",
                messages.len()
            ))]
        }),
        harness.client(),
    );
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("drop"));

    runtime
        .run_turn(&mut context, AgentMessage::user_text("keep"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(harness.requests()[0].messages.len(), 3);
    assert!(matches!(
        &harness.requests()[0].messages[0],
        neo_ai::ChatMessage::User { content } if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text == "drop"
        )
    ));
    assert!(matches!(
        &harness.requests()[0].messages[1],
        neo_ai::ChatMessage::User { content } if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text == "keep"
        )
    ));
    assert!(matches!(
        &harness.requests()[0].messages[2],
        neo_ai::ChatMessage::User { content } if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text.contains("append-only transform saw")
        )
    ));
    assert_eq!(context.messages()[0], AgentMessage::user_text("drop"));
    assert_eq!(context.messages()[1], AgentMessage::user_text("keep"));
}

#[tokio::test]
async fn runtime_emits_tool_execution_events_and_honors_block_and_terminate_hooks() {
    let harness = blocking_then_terminating_tool_harness();
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo)
            .with_before_tool_call(|call| {
                let args: serde_json::Value =
                    serde_json::from_str(&call.raw_arguments).unwrap_or_default();
                if args.get("text").and_then(serde_json::Value::as_str) == Some("blocked") {
                    Some(ToolResult::error("blocked by policy").terminate())
                } else {
                    None
                }
            })
            .with_after_tool_call(|call, mut result| {
                let args: serde_json::Value =
                    serde_json::from_str(&call.raw_arguments).unwrap_or_default();
                if args.get("text").and_then(serde_json::Value::as_str) == Some("stop") {
                    result = result.terminate();
                }
                result
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["stop".to_owned()]
    );
    // Authorization runs before execution starts: a call blocked by the
    // before-hook finishes without ever emitting ToolExecutionStarted.
    assert!(
        !events.contains(&AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "tool_1".to_owned(),
            name: "echo".to_owned(),
            arguments: json!({ "text": "blocked" }),
        }),
        "a hook-blocked call never starts execution"
    );
    assert!(events.contains(&AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool_2".to_owned(),
        name: "echo".to_owned(),
        arguments: json!({ "text": "stop" }),
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("blocked by policy").terminate(),
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_2".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("stop").terminate(),
    }));
    assert_eq!(harness.requests().len(), 1);
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("blocked by policy")],
            true
        )
    );
    assert_eq!(
        context.messages()[3],
        AgentMessage::tool_result("tool_2", "echo", vec![Content::text("stop")], false)
    );
}

#[tokio::test]
async fn runtime_emits_approval_request_for_ask_permission_and_skips_tool_execution() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "needs approval" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Ask),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tool"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("approval required for tool: echo"),
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("approval required for tool: echo")],
            true
        )
    );
}

fn assert_tool_was_executed(executed: &[String], should_execute: bool) {
    let was_executed = !executed.is_empty();
    assert_eq!(
        was_executed, should_execute,
        "expected should_execute={should_execute}, executed list: {executed:?}"
    );
}

fn echo_tool_session_scope(workspace: impl Into<String>) -> SessionApprovalScope {
    SessionApprovalScope {
        keys: vec![SessionApprovalKey::Tool {
            workspace: workspace.into(),
            name: "echo".to_owned(),
        }],
        label: "Approve this tool for this session".to_owned(),
        detail: "Tool: echo".to_owned(),
    }
}

fn echo_tool_options(workspace: impl Into<String>) -> Vec<ApprovalOption> {
    let scope = echo_tool_session_scope(workspace);
    vec![
        ApprovalOption {
            label: "Approve once".to_owned(),
            description: None,
            action: ApprovalAction::PermitOnce,
        },
        ApprovalOption {
            label: scope.label.clone(),
            description: Some(scope.detail.clone()),
            action: ApprovalAction::PermitForSession { scope },
        },
        ApprovalOption {
            label: "Reject".to_owned(),
            description: None,
            action: ApprovalAction::Reject,
        },
    ]
}

fn echo_tool_approval_request(id: &str, workspace: impl Into<String>) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::Tool,
        presentation: ApprovalPresentation::Tool {
            title: "Run tool?".to_owned(),
            details: vec!["tool: echo".to_owned()],
        },
        options: echo_tool_options(workspace),
    }
}

#[tokio::test]
async fn runtime_executes_ask_permission_tool_after_approval_hook_allows_it() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "approved" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::Tool);
                permit_once(request)
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tool"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("approved tool loop should succeed");

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["approved".to_owned()]
    );
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), true);
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("approved"),
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result("tool_1", "echo", vec![Content::text("approved")], false)
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn live_permission_switch_to_auto_skips_approval_for_later_tool_calls() {
    // One turn with two model ToolUse round-trips. The first echo requires
    // approval in Ask mode. The approval handler switches the shared live mode
    // to Auto while granting this first call; the second echo must therefore
    // run WITHOUT a second ApprovalRequested event.
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "first" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "second" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let live_mode = Arc::new(std::sync::RwLock::new(PermissionMode::Ask));
    let live_for_handler = Arc::clone(&live_mode);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_live_permission_mode(Arc::clone(&live_mode))
            .with_approval_handler(move |request| {
                // Flip the live mode to Auto before returning so the second tool
                // call is prepared under Auto and must not request approval again.
                if let Ok(mut mode) = live_for_handler.write() {
                    *mode = PermissionMode::Auto;
                }
                permit_once(request)
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("live-switch tool loop should succeed");

    let first_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { request } if request.id == "tool_1"
        )
    });
    let second_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { request } if request.id == "tool_2"
        )
    });
    assert!(
        first_approval,
        "first call should request approval under Ask"
    );
    assert!(
        !second_approval,
        "second call should NOT request approval after live switch to Auto"
    );
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["first".to_owned(), "second".to_owned()]
    );
    assert_eq!(*live_mode.read().unwrap(), PermissionMode::Auto);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn live_permission_switch_to_ask_requests_approval_for_later_tool_calls() {
    // Inverse of the above: start Auto (no approval), flip live mode to Ask
    // mid-turn via the async after-tool hook, and the second generic tool call
    // must request approval.
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "first" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "second" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let live_mode = Arc::new(std::sync::RwLock::new(PermissionMode::Auto));
    let live_for_hook = Arc::clone(&live_mode);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Auto)
            .with_live_permission_mode(Arc::clone(&live_mode))
            .with_async_after_tool_call(move |_call, result, _cancel| {
                let live = Arc::clone(&live_for_hook);
                async move {
                    if let Ok(mut mode) = live.write() {
                        *mode = PermissionMode::Ask;
                    }
                    result
                }
            })
            .with_approval_handler(|request| permit_once(request)),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("live-switch tool loop should succeed");

    let first_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { request } if request.id == "tool_1"
        )
    });
    let second_approval = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { request } if request.id == "tool_2"
        )
    });
    assert!(
        !first_approval,
        "first call should NOT request approval under Auto"
    );
    assert!(
        second_approval,
        "second call should request approval after live switch to Ask"
    );
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["first".to_owned(), "second".to_owned()]
    );
}

#[tokio::test]
async fn runtime_skips_ask_permission_tool_after_approval_hook_denies_it() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "denied" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::Tool);
                reject_action(request)
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tool"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("denied tool loop should succeed");

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), false);
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("approval denied for tool: echo"),
    }));
}

struct AsyncEchoRuntime {
    runtime: AgentRuntime,
    executed: Arc<Mutex<Vec<String>>>,
    decision_sender: oneshot::Sender<ApprovalResponse>,
    observed_requests: Arc<Mutex<Vec<ApprovalRequest>>>,
}

fn echo_tool_harness(text: &str) -> FakeHarness {
    FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": text }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ])
}

fn async_echo_runtime(harness: &FakeHarness) -> AsyncEchoRuntime {
    let executed = Arc::new(Mutex::new(Vec::new()));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let (decision_sender, decision_receiver) = oneshot::channel();
    let decision_receiver = Arc::new(Mutex::new(Some(decision_receiver)));
    let observed_requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_async_approval_handler({
                let decision_receiver = Arc::clone(&decision_receiver);
                let observed_requests = Arc::clone(&observed_requests);
                move |request| {
                    observed_requests
                        .lock()
                        .expect("observed requests lock poisoned")
                        .push(request.clone());
                    let decision_receiver = take_decision_receiver(&decision_receiver);
                    async move {
                        decision_receiver
                            .await
                            .expect("approval decision should be sent")
                    }
                }
            }),
        harness.client(),
        tools,
    );

    AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender,
        observed_requests,
    }
}

fn take_decision_receiver(
    receiver: &Arc<Mutex<Option<oneshot::Receiver<ApprovalResponse>>>>,
) -> oneshot::Receiver<ApprovalResponse> {
    receiver
        .lock()
        .expect("decision receiver lock poisoned")
        .take()
        .expect("single approval decision receiver")
}

async fn collect_until_approval<S>(stream: &mut S, events: &mut Vec<AgentEvent>)
where
    S: futures::Stream<Item = Result<AgentEvent, AgentRuntimeError>> + Unpin,
{
    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("event before approval request")
            .expect("stream should not end before approval request")
            .expect("event should be ok");
        let approval_requested = matches!(event, AgentEvent::ApprovalRequested { .. });
        events.push(event);
        if approval_requested {
            break;
        }
    }
}

async fn assert_waits_for_approval_decision<S>(stream: &mut S, action: &str)
where
    S: futures::Stream<Item = Result<AgentEvent, AgentRuntimeError>> + Unpin,
{
    assert!(
        timeout(Duration::from_millis(50), stream.next())
            .await
            .is_err(),
        "runtime should wait for the async approval decision before {action}"
    );
}

/// Set the permission mode on a config while keeping the static snapshot and
/// the live runtime state in sync. Direct field mutation of
/// `config.permission_mode` no longer affects permission evaluation because
/// `permission_preparation_for_mode` reads the shared live handle.
fn set_config_permission_mode(config: &mut AgentConfig, mode: PermissionMode) {
    config.permission_mode = mode;
    if let Ok(mut live) = config.live_permission_mode.write() {
        *live = mode;
    }
}

fn final_done_turn() -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: "msg_2".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]
}

#[tokio::test]
async fn runtime_executes_ask_permission_tool_after_async_approval_wait_allows_it() {
    let harness = echo_tool_harness("async approved");
    let AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender,
        observed_requests,
    } = async_echo_runtime(&harness);
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("call tool"));
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert_eq!(
        *observed_requests
            .lock()
            .expect("observed requests lock poisoned"),
        vec![echo_tool_approval_request("tool_1", "")]
    );
    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_waits_for_approval_decision(&mut stream, "executing").await;

    decision_sender
        .send(ApprovalResponse::Selected {
            request_id: "tool_1".to_owned(),
            action: ApprovalAction::PermitOnce,
            feedback: None,
        })
        .expect("send allow decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["async approved".to_owned()]
    );
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), true);
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::ok("async approved"),
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("async approved")],
            false
        )
    );
}

#[tokio::test]
async fn runtime_skips_ask_permission_tool_after_async_approval_wait_denies_it() {
    let harness = echo_tool_harness("async denied");
    let AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender,
        ..
    } = async_echo_runtime(&harness);
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("call tool"));
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), false);
    assert_waits_for_approval_decision(&mut stream, "denying").await;

    decision_sender
        .send(ApprovalResponse::Selected {
            request_id: "tool_1".to_owned(),
            action: ApprovalAction::Reject,
            feedback: None,
        })
        .expect("send deny decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(executed.lock().expect("executed lock poisoned").is_empty());
    assert_tool_was_executed(&executed.lock().expect("lock poisoned"), false);
    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("approval denied for tool: echo"),
    }));
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("approval denied for tool: echo")],
            true
        )
    );
}

#[tokio::test]
async fn runtime_cancels_while_waiting_for_async_approval_decision() {
    let harness = echo_tool_harness("async cancelled");
    let AsyncEchoRuntime {
        runtime,
        executed,
        decision_sender: _decision_sender,
        ..
    } = async_echo_runtime(&harness);
    let mut context = AgentContext::new();
    let cancel = CancellationToken::new();

    let mut stream = runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text("call approval-gated tool"),
        cancel.clone(),
    );
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_1", ""),
    }));
    assert!(executed.lock().expect("executed lock poisoned").is_empty());

    cancel.cancel();
    while let Some(event) = timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("cancelled approval wait should finish promptly")
    {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "echo".to_owned(),
        result: ToolResult::error("tool execution cancelled"),
    }));
    assert_eq!(
        events.last(),
        Some(&AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        })
    );
    assert_eq!(context.messages().len(), 2);
    assert!(executed.lock().expect("executed lock poisoned").is_empty());
}

#[tokio::test]
async fn parallel_mode_serializes_ask_approval_batches() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = parallel_write_and_glob_harness();
    let (decision_sender, decision_receiver) = oneshot::channel();
    let decision_receiver = Arc::new(Mutex::new(Some(decision_receiver)));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace config")
            .with_async_approval_handler({
                let decision_receiver = Arc::clone(&decision_receiver);
                move |request| {
                    assert_eq!(request.operation, PermissionOperation::FileWrite);
                    let decision_receiver = take_decision_receiver(&decision_receiver);
                    async move {
                        decision_receiver
                            .await
                            .expect("approval decision should be sent")
                    }
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("call tools"));
    let mut events = Vec::new();
    collect_until_approval(&mut stream, &mut events).await;

    assert!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .is_err(),
        "later tools in an approval-gated batch must wait for the active approval"
    );
    assert!(
        !workspace.path().join("approved.txt").exists(),
        "approval-gated write should still be pending"
    );

    decision_sender
        .send(ApprovalResponse::Selected {
            request_id: "tool_1".to_owned(),
            action: ApprovalAction::PermitOnce,
            feedback: None,
        })
        .expect("send allow decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event should be ok"));
    }
    drop(stream);

    assert_eq!(
        std::fs::read_to_string(workspace.path().join("approved.txt")).expect("written file"),
        "ok"
    );
    assert!(matches!(
        &context.messages()[2],
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
        } if tool_call_id.as_ref() == "tool_1"
            && tool_name.as_ref() == "Write"
            && content
                .iter()
                .any(|part| matches!(part, Content::Text { text } if text.contains("approved.txt")))
            && !is_error
    ));
    assert!(matches!(
        &context.messages()[3],
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
        } if tool_call_id.as_ref() == "tool_2"
            && tool_name.as_ref() == "Glob"
            && content
                .iter()
                .any(|part| matches!(part, Content::Text { text } if text.contains("Found")))
            && !is_error
    ));
}

fn parallel_write_and_glob_harness() -> FakeHarness {
    FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "path": "approved.txt", "content": "ok" }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "Glob".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "pattern": "*" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ])
}

#[tokio::test]
async fn runtime_approval_handler_allows_file_write_tool_permission() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "path": "approved.txt", "content": "ok" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace config")
            .with_approval_handler(|request| {
                assert_eq!(request.operation, PermissionOperation::FileWrite);
                permit_once(request)
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("write file"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("approved write should succeed");

    // Write now derives a reusable FileWrite scope (Layer 1). Use matches!
    // because the workspace path is dynamic (tempdir).
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalRequested { request }
            if request.id == "tool_1"
                && request.operation == PermissionOperation::FileWrite
                && request.options.iter().any(|option| matches!(
                    &option.action,
                    ApprovalAction::PermitForSession { scope }
                        if scope.label == "Approve writes to this file for this session"
                            && scope.keys.len() == 1
                ))
    )));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("approved.txt")).expect("written file"),
        "ok"
    );
}

#[tokio::test]
async fn runtime_emits_shell_lifecycle_for_bash_tool() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "command": "printf shell-ok" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let workspace = tempfile::tempdir().expect("tempdir");
    let workspace_root = workspace.path().canonicalize().expect("canonicalize");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(&workspace_root)
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run shell"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("shell tool should succeed");

    assert!(events.contains(&AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "tool_1".to_owned(),
        command: "printf shell-ok".to_owned(),
        cwd: workspace_root,
        origin: ShellCommandOrigin::ModelBashTool,
    }));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandFinished {
            turn: 1,
            id,
            exit_code: Some(0),
            stdout,
            stderr,
            ..
        } if id == "tool_1" && stdout.contains("shell-ok") && stderr.is_empty()
    )));
}

#[tokio::test]
async fn runtime_does_not_replay_partial_tool_arguments_to_followup_request() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: r#"{"command":"printf shell-ok","cwd":"#.to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let workspace = tempfile::tempdir().expect("tempdir");
    let workspace_root = workspace.path().canonicalize().expect("canonicalize");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(&workspace_root)
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("run shell"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("shell tool should succeed");

    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    let assistant = requests[1]
        .messages
        .iter()
        .find_map(|message| match message {
            neo_ai::ChatMessage::Assistant { tool_calls, .. } => tool_calls.first(),
            _ => None,
        })
        .expect("assistant tool call replayed");
    assert_eq!(
        assistant.raw_arguments, r#"{"command":"printf shell-ok"}"#,
        "follow-up request must not replay partial JSON tool arguments"
    );
}

#[tokio::test]
async fn runtime_emits_shell_finished_when_model_bash_times_out() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "Bash".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            raw_arguments: json!({
                "command": "printf before-timeout; sleep 5",
                "timeout_secs": 1
            })
            .to_string(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]]);
    let workspace = tempfile::tempdir().expect("tempdir");
    let workspace_root = workspace.path().canonicalize().expect("canonicalize");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(&workspace_root)
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run shell timeout"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should finish with tool error");

    assert!(events.contains(&AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "tool_1".to_owned(),
        command: "printf before-timeout; sleep 5".to_owned(),
        cwd: workspace_root,
        origin: ShellCommandOrigin::ModelBashTool,
    }));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandFinished {
            turn: 1,
            id,
            origin: ShellCommandOrigin::ModelBashTool,
            outcome: ShellCommandOutcome::TimedOut,
            ..
        } if id == "tool_1"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            result,
            ..
        } if id == "tool_1"
            && result.is_error
            && result
                .details
                .as_ref()
                .and_then(|details| details["outcome"].as_str())
                == Some("timed_out")
    )));
}

#[tokio::test]
async fn runtime_marks_model_background_bash_as_backgrounded_shell_event() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "tool_1".to_owned(),
            name: "Bash".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "tool_1".to_owned(),
            raw_arguments: json!({
                "command": "sleep 5",
                "run_in_background": true,
                "description": "sleep in background"
            })
            .to_string(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]]);
    let workspace = tempfile::tempdir().expect("tempdir");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("run background shell"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should finish with background task");

    let task_id = events.iter().find_map(|event| match event {
        AgentEvent::ToolExecutionFinished { result, .. } => result
            .details
            .as_ref()
            .and_then(|details| details["task_id"].as_str())
            .map(str::to_owned),
        _ => None,
    });
    let task_id = task_id.expect("background task id");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandFinished {
            id,
            exit_code: None,
            outcome: ShellCommandOutcome::Backgrounded { task_id: event_task_id },
            ..
        } if id == "tool_1" && event_task_id.as_ref() == task_id.as_str()
    )));
    let _ = runtime
        .config()
        .background_tasks
        .stop(&task_id, "test cleanup", 1024)
        .await;
}

#[tokio::test]
async fn runtime_events_and_session_jsonl_do_not_leak_capped_bash_output() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "command": "printf keep; printf '%s%s%s%s' runtime -bash -leak -tail",
                    "max_output_bytes": 4
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run capped shell"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("shell tool should succeed");
    let event_json = persist_events_to_jsonl_and_read_back(&events).await;

    assert!(!event_json.contains("runtime-bash-leak-tail"));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ShellCommandFinished {
            stdout,
            truncated: true,
            ..
        } if stdout.len() <= 4
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            result,
            ..
        } if result
            .details
            .as_ref()
            .and_then(|details| details["stdout"].as_str())
            .is_some_and(|stdout| stdout.len() <= 4)
    )));
}

#[tokio::test]
async fn runtime_emits_terminal_lifecycle_events_for_terminal_tool() {
    let model = Arc::new(TerminalLifecycleModel::default());
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(fake_model())
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Sequential),
        model,
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("open terminal"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal tool turn should succeed");

    let handle = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::TerminalSessionStarted {
                handle,
                command,
                cols,
                rows,
                cwd,
                ..
            } if command == "bash --noprofile --norc"
                && *cols == 44
                && *rows == 9
                && cwd == &workspace_root =>
            {
                Some(handle.clone())
            }
            _ => None,
        })
        .expect("terminal start event should expose handle and PTY metadata");
    assert!(!handle.is_empty());

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TerminalSessionOutput {
            handle: event_handle,
            output,
            ..
        } if event_handle == &handle && output.contains("terminal-event-ok")
    )));

    let finished = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::TerminalSessionFinished {
                handle: event_handle,
                status,
                ..
            } if event_handle == &handle && status == "cancelled"
        )
    });
    assert!(
        finished,
        "terminal stop should emit a provider-neutral finished event"
    );
}

#[tokio::test]
async fn runtime_streams_terminal_prompt_updates_before_read() {
    let prompt = "Stage this hunk [y,n,q,a,d,j,J,g,/,s,e,p,?]?";
    let model = Arc::new(TerminalStreamingModel::default());
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(fake_model()).with_permission_mode(PermissionMode::Yolo),
        model,
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("open terminal prompt"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal turn should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionUpdate {
                name,
                partial_result,
                ..
            } if name == "Terminal" && partial_result.content.contains(prompt)
        )),
        "expected a Terminal streaming update carrying the prompt before read returned; events: {events:?}"
    );
}

#[tokio::test]
async fn runtime_events_and_session_jsonl_do_not_leak_capped_terminal_output() {
    let model = Arc::new(CappedTerminalOutputModel::default());
    let workspace = tempfile::tempdir().expect("workspace");
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(fake_model())
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Sequential),
        model,
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("read capped terminal"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal tool turn should succeed");
    let event_json = persist_events_to_jsonl_and_read_back(&events).await;

    assert!(!event_json.contains("terminal-runtime-leak-tail"));
    assert!(
        events.iter().any(|event| matches!(
        event,
        AgentEvent::TerminalSessionOutput {
            output,
            truncated: true,
            ..
        } if output.len() <= 4
        )),
        "events should include capped terminal output: {event_json}"
    );
    assert!(
        events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            name,
            result,
            ..
        } if name == "Terminal"
            && result
                .details
                .as_ref()
                .and_then(|details| details["output"].as_str())
                .is_some_and(|output| output.len() <= 4)
        )),
        "events should include capped terminal ToolExecutionFinished: {event_json}"
    );
}

async fn persist_events_to_jsonl_and_read_back(events: &[AgentEvent]) -> String {
    let temp = tempfile::tempdir().expect("session dir");
    let path = temp.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session writer");
    for event in events {
        writer
            .append_event(event)
            .await
            .expect("append session event");
    }
    writer.flush().await.expect("flush session writer");
    std::fs::read_to_string(path).expect("read session jsonl")
}

fn blocking_then_terminating_tool_harness() -> FakeHarness {
    FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "blocked" }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "stop" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "should not run".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ])
}

#[tokio::test]
async fn runtime_parallel_tool_mode_finishes_by_completion_but_appends_in_source_order() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "sleep_echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "slow", "delay_ms": 40 }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "sleep_echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "fast", "delay_ms": 0 }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(SleepEchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    let execution_end_ids = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionFinished { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(execution_end_ids, vec!["tool_2", "tool_1"]);

    let appended_tool_ids = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended {
                message: AgentMessage::ToolResult { tool_call_id, .. },
            } => Some(tool_call_id.as_ref()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(appended_tool_ids, vec!["tool_1", "tool_2"]);
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result("tool_1", "sleep_echo", vec![Content::text("slow")], false)
    );
    assert_eq!(
        context.messages()[3],
        AgentMessage::tool_result("tool_2", "sleep_echo", vec![Content::text("fast")], false)
    );
}

#[tokio::test]
async fn parallel_mode_serializes_non_background_ask_user_question() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "AskUserQuestion".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "questions": [{
                        "question": "Continue?",
                        "options": [
                            { "label": "Yes" },
                            { "label": "No" }
                        ]
                    }]
                })
                .to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "should wait" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let (question_tx, mut question_rx) = mpsc::unbounded_channel();
    let mut tools = ToolRegistry::new();
    tools.register(neo_agent_core::AskUserTool::new(question_tx));
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let mut stream = runtime.run_turn(&mut context, AgentMessage::user_text("ask and echo"));
    let pending = timeout(Duration::from_millis(250), question_rx.recv())
        .await
        .expect("question should be requested before other tools run")
        .expect("question should be pending");
    assert!(
        executed.lock().expect("executed lock poisoned").is_empty(),
        "non-dialog tools must wait while AskUserQuestion is waiting for the user"
    );

    pending
        .response_tx
        .send(neo_agent_core::QuestionResponse {
            answers: vec!["Yes".to_owned()],
        })
        .expect("send question response");
    while let Some(event) = stream.next().await {
        event.expect("event should be ok");
    }
    drop(stream);

    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["should wait".to_owned()]
    );
}

#[tokio::test]
async fn ask_mode_ask_user_question_dispatches_without_approval() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "AskUserQuestion".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "questions": [{
                        "question": "Which language?",
                        "options": [
                            { "label": "Rust" },
                            { "label": "TypeScript" }
                        ]
                    }]
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let (question_tx, mut question_rx) = mpsc::unbounded_channel();
    let mut tools = ToolRegistry::new();
    tools.register(neo_agent_core::AskUserTool::new(question_tx));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Ask),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let stream = runtime.run_turn(&mut context, AgentMessage::user_text("ask user"));
    let pending = timeout(Duration::from_millis(250), question_rx.recv())
        .await
        .expect("ask mode should dispatch AskUserQuestion to the host")
        .expect("question should be pending");
    assert_eq!(pending.questions[0].question, "Which language?");

    pending
        .response_tx
        .send(neo_agent_core::QuestionResponse {
            answers: vec!["Rust".to_owned()],
        })
        .expect("send question response");
    let events = stream
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AgentEvent::ApprovalRequested { .. })),
        "AskUserQuestion must not be wrapped in the approval dialog"
    );
}

#[tokio::test]
async fn ask_mode_skill_tool_runs_without_approval() {
    let skills_dir = tempfile::tempdir().expect("skills dir");
    std::fs::write(
        skills_dir.path().join("SKILL.md"),
        r"---
name: review
description: Review the current change.
---
Review the current change carefully.
",
    )
    .expect("write skill");
    let skill_store = SkillStore::load(&[], &[skills_dir.path().to_path_buf()], Vec::new())
        .expect("load skill store");

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Skill".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({"skill": "review"}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools_and_skills(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Ask),
        harness.client(),
        ToolRegistry::new(),
        skill_store,
    );
    let mut context = AgentContext::new();

    let events = timeout(
        Duration::from_secs(2),
        runtime
            .run_turn(&mut context, AgentMessage::user_text("use review skill"))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("skill turn should finish")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("skill tool should run");

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AgentEvent::ApprovalRequested { .. })),
        "Skill must not be wrapped in the approval dialog"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished { name, result, .. }
                if name == "Skill"
                    && !result.is_error
                    && result.content.contains("Review the current change carefully.")
        )),
        "Skill should execute successfully; events: {events:#?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::SkillInvocation {
                names,
                source: SkillInvocationSource::Auto,
                outcome: SkillInvocationOutcome::Activated,
                body,
            } if names == &["review".to_owned()] && body.is_empty()
        )),
        "Skill should emit one semantic activation event; events: {events:#?}"
    );
}

#[tokio::test]
async fn automatic_missing_skill_emits_failed_skill_invocation() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Skill".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({"skill": "missing"}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools_and_skills(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        ToolRegistry::new(),
        SkillStore::default(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("use missing skill"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("missing skill result should return to the model");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::SkillInvocation {
                names,
                source: SkillInvocationSource::Auto,
                outcome: SkillInvocationOutcome::Failed,
                body,
            } if names == &["missing".to_owned()]
                && body.contains("skill `missing` is not available")
        )),
        "missing Skill should emit a semantic failure event; events: {events:#?}"
    );
}

#[tokio::test]
async fn enter_plan_mode_continues_model_loop_after_mode_switch() {
    let home = tempfile::tempdir().expect("home dir");
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "EnterPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({}).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "continuing plan".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_home_dir(home.path())
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = timeout(
        Duration::from_secs(2),
        runtime
            .run_turn(&mut context, AgentMessage::user_text("make a plan"))
            .collect::<Vec<_>>(),
    )
    .await
    .expect("plan-mode turn should finish")
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("turn should continue after entering plan mode");

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::PlanModeEntered { .. })),
        "EnterPlanMode should still emit the plan-mode side effect"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::TextDelta { text, .. } if text == "continuing plan"
        )),
        "the model loop should continue after EnterPlanMode"
    );
    assert_eq!(
        harness.requests().len(),
        2,
        "EnterPlanMode should not stop the agent loop"
    );
}

#[tokio::test]
async fn runtime_yolo_mode_auto_approves_custom_tool() {
    let harness = echo_tool_harness("yolo approved");
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call echo"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, AgentEvent::ApprovalRequested { .. })),
        "yolo mode should not request approvals"
    );
    assert_eq!(
        context.messages()[2],
        AgentMessage::tool_result(
            "tool_1",
            "echo",
            vec![Content::text("yolo approved")],
            false
        )
    );
}

#[tokio::test]
async fn runtime_auto_mode_denies_ask_user_question() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "AskUserQuestion".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "questions": [{
                        "question": "Continue?",
                        "options": [{ "label": "Yes" }, { "label": "No" }]
                    }]
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let (question_tx, mut question_rx) = mpsc::unbounded_channel();
    let mut tools = ToolRegistry::new();
    tools.register(AskUserTool::new(question_tx));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Auto),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("ask user"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(events.contains(&AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        name: "AskUserQuestion".to_owned(),
        result: ToolResult::error(
            "AskUserQuestion is disabled while auto permission mode is active"
        ),
    }));
    assert!(
        question_rx.try_recv().is_err(),
        "no question should be dispatched in auto mode"
    );
}

#[tokio::test]
async fn runtime_ask_mode_read_runs_and_custom_tool_asks() {
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_key = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace")
        .display()
        .to_string();
    std::fs::write(workspace.path().join("file.txt"), "hello").expect("seed file");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Read".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "path": "file.txt" }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "needs approval" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let mut tools = ToolRegistry::with_builtin_tools();
    tools.register(EchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("read and call"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished {
                id,
                name,
                result,
                ..
            } if id == "tool_1" && name == "Read" && result.content.contains("hello")
        )),
        "Read should run without approval in ask mode"
    );
    assert!(events.contains(&AgentEvent::ApprovalRequested {
        request: echo_tool_approval_request("tool_2", workspace_key),
    }));
}

#[tokio::test]
async fn runtime_session_approval_persists_for_same_tool() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "text": "first" }).to_string(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "text": "second" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let executed = Arc::new(Mutex::new(Vec::new()));
    let approval_count = Arc::new(Mutex::new(0));
    let mut tools = ToolRegistry::new();
    tools.register(RecordingEchoTool {
        executed: Arc::clone(&executed),
    });
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_approval_handler({
                let count = Arc::clone(&approval_count);
                move |request| {
                    *count.lock().expect("count lock poisoned") += 1;
                    permit_for_session(request)
                }
            }),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("call echo twice"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("tool loop should succeed");

    assert_eq!(
        *approval_count.lock().expect("count lock poisoned"),
        1,
        "AllowForSession should approve the same named tool for the rest of the session"
    );
    assert_eq!(
        *executed.lock().expect("executed lock poisoned"),
        vec!["first".to_owned(), "second".to_owned()]
    );
}

#[tokio::test]
async fn runtime_ask_mode_reviews_exit_plan_mode_with_non_empty_plan() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "do the thing").expect("write plan");
    }

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready to execute" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        approve_plan(request)
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ApprovalRequested { request }
            if request.id == "tool_1"
                && request.operation == PermissionOperation::PlanTransition
                && matches!(
                    request.presentation,
                    ApprovalPresentation::Plan { .. }
                )
                && matches!(
                    request.options.first().map(|option| &option.action),
                    Some(ApprovalAction::ApprovePlan { selection: None })
                )
                && request.options.iter().any(|option| {
                    matches!(option.action, ApprovalAction::RejectPlan)
                })
                && !request.options.iter().any(|option| {
                    matches!(option.action, ApprovalAction::PermitForSession { .. })
                })
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PlanModeExited { turn, .. } if *turn == 1
    )));
}

/// Regression: after an approved `ExitPlanMode`, the agent loop must continue
/// into the next turn so the model can execute the approved plan. Previously
/// `ExitPlanMode` set `terminate=true` and `continues_after_terminating_batch`
/// only matched `EnterPlanMode`, so the run ended and the user had to send
/// another prompt to resume. kimi-code's `ExitPlanMode` does not stop the turn.
#[tokio::test]
async fn exit_plan_mode_continues_loop_after_approval() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "execute the plan").expect("write plan");
    }

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready to execute" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "starting work".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        approve_plan(request)
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::PlanModeExited { turn, .. } if *turn == 1
        )),
        "ExitPlanMode should still flip plan mode off"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, AgentEvent::TextDelta { text, .. } if text == "starting work")
        ),
        "the model loop should continue after an approved ExitPlanMode"
    );
    assert_eq!(
        harness.requests().len(),
        2,
        "an approved ExitPlanMode must not stop the agent loop"
    );
}

/// Regression: when the user approves a specific model-supplied plan-review
/// option, the runtime prefixes the `ExitPlanMode` tool result with
/// "Selected approach: <label>" so the model executes only that branch. The
/// selection is carried as typed `ApprovalAction::ApprovePlan` metadata — not
/// a global side map.
#[tokio::test]
async fn exit_plan_mode_plan_selection_label_prefixes_tool_result() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "ship feature X").expect("write plan");
    }

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "plan_summary": "Two approaches available",
                    "options": [
                        {"label": "Option A", "description": "fast"},
                        {"label": "Option B", "description": "safe"}
                    ]
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "running option a".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        approve_plan_with_label(request, "Option A")
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let _events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve option A"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // The selected-approach prefix must reach the next model turn. The harness
    // records every ChatRequest, so turn 2's messages must contain the prefix
    // in the ExitPlanMode tool result that was appended to the context.
    let requests = harness.requests();
    assert_eq!(
        requests.len(),
        2,
        "an approved ExitPlanMode should continue into a second model turn"
    );
    let turn2 = &requests[1];
    let turn2_text = serde_json::to_string(turn2).unwrap_or_default();
    assert!(
        turn2_text.contains("Selected approach: Option A"),
        "turn 2 request must carry the selected-approach prefix; got: {turn2_text}"
    );
    assert!(
        turn2_text.contains("Execute ONLY the selected approach"),
        "turn 2 request must carry the execute-only instruction"
    );
}

/// Regression: `ExitGoalMode` starts the durable goal and the run ends
/// cleanly. Unlike `ExitPlanMode`, the loop must NOT continue inline here —
/// goal continuation (`goal_continuation_messages`) drives subsequent turns on
/// the next `run_agent_turn` entry by design, and continuing inline would
/// re-feed the continuation message every turn and spin. This test pins the
/// boundary: the goal is started, the run finishes without a second model turn
/// from this entry, and the goal is resumable.
#[tokio::test]
async fn exit_goal_mode_starts_goal_and_ends_run_without_spinning() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitGoalMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "objective": "Ship goal mode",
                    "completion_criterion": "Goal tests pass",
                    "phases": ["Draft", "Implement", "Audit"],
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::GoalTransition);
        start_goal(request)
    });
    let goal_manager = Arc::new(
        neo_agent_core::goal::GoalManager::load(home.path().to_path_buf())
            .await
            .expect("goal manager"),
    );
    let mut registry = ToolRegistry::with_builtin_tools();
    registry.register_goal_tools(Arc::clone(&goal_manager));
    let runtime = AgentRuntime::with_tools(config, harness.client(), registry)
        .with_goal_manager(&goal_manager);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve goal"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        events.contains(&AgentEvent::GoalStarted {
            turn: 1,
            objective: "Ship goal mode".to_owned(),
        }),
        "ExitGoalMode should start the durable goal"
    );
    // The terminating batch must not continue inline: the goal is durable and
    // is resumed on the next run_agent_turn entry. Exactly one model request
    // means we did not spin on goal-continuation.
    assert_eq!(
        harness.requests().len(),
        1,
        "ExitGoalMode must end the run without spinning on goal continuation"
    );
    let active = goal_manager.active().expect("active goal");
    assert_eq!(active.phases, ["Draft", "Implement", "Audit"]);
}

/// Generic plan approval (no model-supplied alternatives) must offer
/// `ApprovePlan { selection: None }` — never a fabricated selection named
/// "Approve" — and must not write `plan_selected_label` into tool details.
#[tokio::test]
async fn exit_plan_mode_generic_approval_has_no_selected_approach() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    setup_active_plan(&mut config, &home, &workspace, "generic plan body");
    set_config_permission_mode(&mut config, PermissionMode::Ask);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready to execute" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert!(matches!(
            request.options.first().map(|option| &option.action),
            Some(ApprovalAction::ApprovePlan { selection: None })
        ));
        approve_plan(request)
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let plan_request = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequested { request }
                if request.operation == PermissionOperation::PlanTransition =>
            {
                Some(request)
            }
            _ => None,
        })
        .expect("plan approval request");
    assert!(matches!(
        plan_request.options.first().map(|option| &option.action),
        Some(ApprovalAction::ApprovePlan { selection: None })
    ));
    let finished = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { name, result, .. } if name == "ExitPlanMode" => {
                Some(result)
            }
            _ => None,
        })
        .expect("ExitPlanMode finished");
    assert!(
        !finished.content.contains("Selected approach:"),
        "generic approve must not fabricate a selected approach"
    );
    if let Some(details) = finished.details.as_ref() {
        assert!(
            details.get("plan_selected_label").is_none(),
            "generic approve must not set plan_selected_label"
        );
    }
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PlanModeExited { turn, .. } if *turn == 1
    )));
}

/// Typed plan selection reaches the ExitPlanMode tool result details without
/// a side map keyed by tool id.
#[tokio::test]
async fn exit_plan_mode_typed_selection_reaches_tool_result() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    setup_active_plan(&mut config, &home, &workspace, "choose carefully");
    set_config_permission_mode(&mut config, PermissionMode::Ask);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "plan_summary": "Two approaches",
                    "options": [
                        {"label": "Fast path", "description": "ship sooner"},
                        {"label": "Safe path", "description": "more checks"}
                    ]
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config =
        config.with_approval_handler(|request| approve_plan_with_label(request, "Safe path"));
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("pick safe"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // Re-emitted finished event after decoration carries selection details.
    let selected_plan_result = events
        .iter()
        .rev()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { name, result, .. }
                if name == "ExitPlanMode" && !result.is_error =>
            {
                Some(result)
            }
            _ => None,
        })
        .expect("selected ExitPlanMode result");
    assert!(
        selected_plan_result
            .content
            .contains("Selected approach: Safe path"),
        "content prefix missing: {}",
        selected_plan_result.content
    );
    assert!(matches!(
        selected_plan_result.details.as_ref(),
        Some(details) if details["plan_selected_label"] == "Safe path"
    ));
}

/// RejectGoal and ReviseGoal must not create a durable goal and must leave
/// goal authoring pending (no GoalStarted).
#[tokio::test]
async fn exit_goal_mode_reject_and_revise_create_no_goal() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");

    let goal_payload = json!({
        "objective": "Ship goal mode",
        "completion_criterion": "Goal tests pass",
        "phases": ["Draft", "Implement"],
    });

    // --- RejectGoal ---
    {
        let mut config = AgentConfig::for_model(fake_model());
        config.home_dir = Some(home.path().to_path_buf());
        config.workspace_root = Some(workspace_root.clone());
        set_config_permission_mode(&mut config, PermissionMode::Ask);
        let harness = FakeHarness::from_turns([
            vec![
                AiStreamEvent::MessageStart {
                    id: "msg_1".to_owned(),
                },
                AiStreamEvent::ToolCallStart {
                    id: "tool_1".to_owned(),
                    name: "ExitGoalMode".to_owned(),
                },
                AiStreamEvent::ToolCallEnd {
                    id: "tool_1".to_owned(),
                    raw_arguments: goal_payload.to_string(),
                },
                AiStreamEvent::MessageEnd {
                    stop_reason: neo_ai::StopReason::ToolUse,
                    usage: None,
                },
            ],
            final_done_turn(),
        ]);
        let config = config.with_approval_handler(|request| {
            assert!(matches!(
                request.options.as_slice(),
                [
                    ApprovalOption {
                        action: ApprovalAction::StartGoal,
                        ..
                    },
                    ApprovalOption {
                        action: ApprovalAction::RejectGoal,
                        ..
                    },
                    ApprovalOption {
                        action: ApprovalAction::ReviseGoal { .. },
                        ..
                    },
                ]
            ));
            reject_goal(request)
        });
        let goal_manager = Arc::new(
            neo_agent_core::goal::GoalManager::load(home.path().join("reject"))
                .await
                .expect("goal manager"),
        );
        let mut registry = ToolRegistry::with_builtin_tools();
        registry.register_goal_tools(Arc::clone(&goal_manager));
        let runtime = AgentRuntime::with_tools(config, harness.client(), registry)
            .with_goal_manager(&goal_manager);
        let mut context = AgentContext::new();

        let events = runtime
            .run_turn(&mut context, AgentMessage::user_text("reject goal"))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("turn should succeed");

        let goal_request = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ApprovalRequested { request }
                    if request.operation == PermissionOperation::GoalTransition =>
                {
                    Some(request)
                }
                _ => None,
            })
            .expect("goal approval request");
        assert!(matches!(
            goal_request.options.as_slice(),
            [
                ApprovalOption {
                    action: ApprovalAction::StartGoal,
                    ..
                },
                ApprovalOption {
                    action: ApprovalAction::RejectGoal,
                    ..
                },
                ApprovalOption {
                    action: ApprovalAction::ReviseGoal { .. },
                    ..
                },
            ]
        ));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::GoalStarted { .. })),
            "RejectGoal must not start a goal"
        );
        assert!(
            goal_manager.active().is_none(),
            "no active goal after reject"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished {
                name,
                result,
                ..
            } if name == "ExitGoalMode" && result.content.contains("approval denied")
        )));
    }

    // --- ReviseGoal ---
    {
        let mut config = AgentConfig::for_model(fake_model());
        config.home_dir = Some(home.path().to_path_buf());
        config.workspace_root = Some(workspace_root);
        set_config_permission_mode(&mut config, PermissionMode::Ask);
        let harness = FakeHarness::from_turns([
            vec![
                AiStreamEvent::MessageStart {
                    id: "msg_1".to_owned(),
                },
                AiStreamEvent::ToolCallStart {
                    id: "tool_1".to_owned(),
                    name: "ExitGoalMode".to_owned(),
                },
                AiStreamEvent::ToolCallEnd {
                    id: "tool_1".to_owned(),
                    raw_arguments: goal_payload.to_string(),
                },
                AiStreamEvent::MessageEnd {
                    stop_reason: neo_ai::StopReason::ToolUse,
                    usage: None,
                },
            ],
            final_done_turn(),
        ]);
        let config = config.with_approval_handler(|request| {
            revise_goal_with_feedback(request, "add a validation phase")
        });
        let goal_manager = Arc::new(
            neo_agent_core::goal::GoalManager::load(home.path().join("revise"))
                .await
                .expect("goal manager"),
        );
        let mut registry = ToolRegistry::with_builtin_tools();
        registry.register_goal_tools(Arc::clone(&goal_manager));
        let runtime = AgentRuntime::with_tools(config, harness.client(), registry)
            .with_goal_manager(&goal_manager);
        let mut context = AgentContext::new();

        let events = runtime
            .run_turn(&mut context, AgentMessage::user_text("revise goal"))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("turn should succeed");

        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::GoalStarted { .. })),
            "ReviseGoal must not start a goal"
        );
        assert!(
            goal_manager.active().is_none(),
            "no active goal after revise"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished {
                name,
                result,
                ..
            } if name == "ExitGoalMode"
                && !result.is_error
                && result.content.contains("User requested revisions")
                && result.content.contains("add a validation phase")
        )));
    }
}

/// Regression: even if a session-scoped approval (`AllowForSession`) is returned
/// for an `ExitPlanMode` review, the tool name must NOT be cached in
/// `session_approvals`. Plan/goal transitions are one-shot — caching the name
/// would silently auto-approve every future exit review for the rest of the
/// session. The decision is treated as `AllowOnce` for these operations.
#[tokio::test]
async fn runtime_allow_for_session_does_not_cache_exit_plan_mode() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready to execute" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);

    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "do the thing").expect("write plan");
    }
    let session_approvals = Arc::clone(&config.session_approvals);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        // Pretend the (now-removed) "Approve for this session" option was chosen.
        // Session option is not offered for plan/goal transitions.
        first_offered_action(request)
    });

    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // The decisive assertion: no approval key must be cached, otherwise every
    // future exit-plan review would be silently auto-approved for the session.
    let cached = session_approvals.lock().expect("session approvals lock");
    assert!(
        cached.is_empty(),
        "ExitPlanMode must not cache any session approval key (got {cached:?}); \
         AllowForSession must be treated as AllowOnce for plan/goal transitions"
    );
}

#[tokio::test]
async fn runtime_ask_mode_reviews_exit_goal_mode_and_emits_goal_started() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(
        workspace
            .path()
            .canonicalize()
            .expect("canonical workspace"),
    );
    set_config_permission_mode(&mut config, PermissionMode::Ask);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitGoalMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "objective": "Ship goal mode",
                    "completion_criterion": "Goal tests pass",
                    "phases": ["Draft", "Implement", "Audit"],
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::GoalTransition);
        start_goal(request)
    });
    let goal_manager = Arc::new(
        neo_agent_core::goal::GoalManager::load(home.path().to_path_buf())
            .await
            .expect("goal manager"),
    );
    let mut registry = ToolRegistry::with_builtin_tools();
    registry.register_goal_tools(Arc::clone(&goal_manager));
    let runtime = AgentRuntime::with_tools(config, harness.client(), registry)
        .with_goal_manager(&goal_manager);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve goal"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalRequested { request }
            if request.id == "tool_1"
                && request.operation == PermissionOperation::GoalTransition
                && matches!(
                    request.presentation,
                    ApprovalPresentation::Goal { .. }
                )
                && matches!(
                    request.options.as_slice(),
                    [
                        ApprovalOption { action: ApprovalAction::StartGoal, .. },
                        ApprovalOption { action: ApprovalAction::RejectGoal, .. },
                        ApprovalOption { action: ApprovalAction::ReviseGoal { .. }, .. },
                    ]
                )
    )));
    assert!(events.contains(&AgentEvent::GoalStarted {
        turn: 1,
        objective: "Ship goal mode".to_owned(),
    }));
    let active = goal_manager.active().expect("active goal");
    assert_eq!(active.phases, ["Draft", "Implement", "Audit"]);
}

#[tokio::test]
async fn runtime_ask_mode_exit_plan_mode_reject_keeps_plan_active() {
    // RejectPlan must deny with "approval denied" and leave plan mode active.
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Ask);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "do the thing").expect("write plan");
    }
    let plan_mode = Arc::clone(&config.plan_mode);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready to execute" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(move |request| {
        if request.operation == PermissionOperation::PlanTransition {
            reject_plan(request)
        } else {
            permit_once(request)
        }
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("revise plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::PlanModeExited { .. })),
        "plan mode should remain active after RejectPlan"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1"
            && name == "ExitPlanMode"
            && result.content.contains("approval denied")
    )));
    assert!(plan_mode.read().expect("plan mode lock").is_active());
}

#[tokio::test]
async fn runtime_plan_mode_guard_denies_write_outside_plan_file() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Yolo);
    {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let _data = pm.enter(&plans_dir, true).expect("enter plan mode");
    }
    let plan_mode = Arc::clone(&config.plan_mode);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "path": "other.txt", "content": "x" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("write while planning"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Write" && result.is_error && result.content.contains("plan mode")
    )));
    assert!(
        plan_mode.read().expect("plan mode lock").is_active(),
        "plan mode should stay active after a blocked write"
    );
}

#[tokio::test]
async fn runtime_plan_mode_allows_writing_active_plan_file_outside_workspace() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Yolo);
    let plan_path = {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        pm.enter(&plans_dir, true).expect("enter plan mode").path
    };

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "path": plan_path,
                    "content": "# Plan\n\nUse Write, not Bash."
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("write plan file"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Write" && !result.is_error
    )));
    assert_eq!(
        std::fs::read_to_string(&plan_path).expect("read plan"),
        "# Plan\n\nUse Write, not Bash."
    );
}

/// Regression: `Edit` (which resolves via `resolve_workspace_path`, not
/// `resolve_parent_for_write`) must also be able to reach the active plan file
/// under the `NEO_HOME` sessions bucket while plan mode is active.
#[tokio::test]
async fn runtime_plan_mode_allows_editing_active_plan_file_outside_workspace() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    let mut config = AgentConfig::for_model(fake_model());
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    set_config_permission_mode(&mut config, PermissionMode::Yolo);
    let plan_path = {
        let mut pm = config.plan_mode.write().expect("plan mode lock");
        let data = pm.enter(&plans_dir, true).expect("enter plan mode");
        std::fs::write(&data.path, "# Plan\n\nDraft.").expect("seed plan");
        data.path
    };

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Edit".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({
                    "path": plan_path,
                    "old": "Draft.",
                    "new": "Finalized."
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("edit plan file"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Edit" && !result.is_error
    )));
    assert_eq!(
        std::fs::read_to_string(&plan_path).expect("read plan"),
        "# Plan\n\nFinalized."
    );
}

fn setup_active_plan(
    config: &mut AgentConfig,
    home: &tempfile::TempDir,
    workspace: &tempfile::TempDir,
    content: &str,
) {
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let plans_dir = main_agent_plans_dir(&workspace_sessions_dir(
        &home.path().join("sessions"),
        &workspace_root,
    ));
    config.home_dir = Some(home.path().to_path_buf());
    config.workspace_root = Some(workspace_root);
    let mut pm = config.plan_mode.write().expect("plan mode lock");
    let data = pm.enter(&plans_dir, true).expect("enter plan mode");
    std::fs::write(&data.path, content).expect("write plan");
}

#[tokio::test]
async fn ask_mode_asks_for_bash() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "command": "mkdir test_dir" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run bash"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // `mkdir test_dir` is NOT a known-safe command (mkdir isn't in the safe
    // list), so it must prompt. Use matches! because the scope carries a
    // dynamic workspace path.
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalRequested { request }
            if request.id == "tool_1"
                && request.operation == PermissionOperation::Shell
                && matches!(
                    &request.presentation,
                    ApprovalPresentation::Command { command, .. }
                        if command == "mkdir test_dir"
                )
                && request.options.iter().any(|option| {
                    matches!(option.action, ApprovalAction::PermitForSession { .. })
                })
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionFinished {
            id,
            name,
            result,
            ..
        } if id == "tool_1" && name == "Bash" && result.content.contains("approval required")
    )));
}

#[tokio::test]
async fn auto_mode_approves_bash_without_approval() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "command": "printf auto-ok" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Auto)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run bash"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. })),
        "auto mode should not request bash approval"
    );
    assert!(events.contains(&AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "tool_1".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "auto-ok".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome: ShellCommandOutcome::Completed,
    }));
}

#[tokio::test]
async fn yolo_mode_approves_write_without_approval() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Write".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "path": "yolo.txt", "content": "yolo" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("write file"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. })),
        "yolo mode should not request write approval"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("yolo.txt")).expect("written file"),
        "yolo"
    );
}

#[tokio::test]
async fn auto_exit_plan_mode_does_not_request_review() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    setup_active_plan(&mut config, &home, &workspace, "do the thing");
    set_config_permission_mode(&mut config, PermissionMode::Auto);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. })),
        "auto mode should not review ExitPlanMode"
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PlanModeExited { turn, .. } if *turn == 1
    )));
}

#[tokio::test]
async fn yolo_exit_plan_mode_with_non_empty_plan_requests_review() {
    let home = tempfile::tempdir().expect("home");
    let workspace = tempfile::tempdir().expect("workspace");
    let mut config = AgentConfig::for_model(fake_model());
    setup_active_plan(&mut config, &home, &workspace, "do the thing");
    set_config_permission_mode(&mut config, PermissionMode::Yolo);

    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "ExitPlanMode".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "plan_summary": "Ready" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = config.with_approval_handler(|request| {
        assert_eq!(request.operation, PermissionOperation::PlanTransition);
        approve_plan(request)
    });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve plan"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ApprovalRequested { request }
            if request.id == "tool_1"
                && request.operation == PermissionOperation::PlanTransition
                && matches!(
                    request.options.first().map(|option| &option.action),
                    Some(ApprovalAction::ApprovePlan { selection: None })
                )
                && !request.options.iter().any(|option| {
                    matches!(option.action, ApprovalAction::PermitForSession { .. })
                })
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PlanModeExited { turn, .. } if *turn == 1
    )));
}

struct EchoTool;

fn runtime_with_large_tool(harness: &FakeHarness) -> AgentRuntime {
    let mut registry = ToolRegistry::new();
    registry.register(LargeTool);
    let config = AgentConfig::for_model(harness.model())
        .with_tool_execution_mode(ToolExecutionMode::Parallel)
        .with_compaction(CompactionSettings::new(1, 1));
    AgentRuntime::with_tools(config, harness.client(), registry)
}

struct LargeTool;

impl Tool for LargeTool {
    fn name(&self) -> &'static str {
        "LargeTool"
    }

    fn description(&self) -> &'static str {
        "Returns a large payload."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async { Ok(ToolResult::ok("tool output ".repeat(20_000))) })
    }
}

impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Echo text."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
            "required": ["text"]
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            Ok(ToolResult::ok(
                input
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            ))
        })
    }
}

struct RecordingEchoTool {
    executed: Arc<Mutex<Vec<String>>>,
}

impl Tool for RecordingEchoTool {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn description(&self) -> &'static str {
        "Record and echo text."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
            "required": ["text"]
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let text = input
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            self.executed
                .lock()
                .expect("executed lock poisoned")
                .push(text.clone());
            Ok(ToolResult::ok(text))
        })
    }
}

struct FallibleTool;

impl Tool for FallibleTool {
    fn name(&self) -> &'static str {
        "fallible"
    }

    fn description(&self) -> &'static str {
        "Always returns a tool-layer error."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
            "required": ["text"]
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async {
            Err(ToolError::InvalidInput {
                tool: "fallible".to_owned(),
                message: "expected text".to_owned(),
            })
        })
    }
}

struct NeverTool;

impl Tool for NeverTool {
    fn name(&self) -> &'static str {
        "never"
    }

    fn description(&self) -> &'static str {
        "Never completes."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(std::future::pending())
    }
}

struct SleepEchoTool;

impl Tool for SleepEchoTool {
    fn name(&self) -> &'static str {
        "sleep_echo"
    }

    fn description(&self) -> &'static str {
        "Sleep and echo text."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" },
                "delay_ms": { "type": "integer" }
            },
            "required": ["text", "delay_ms"]
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let text = input
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let delay_ms = input
                .get("delay_ms")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default();
            if delay_ms > 0 {
                let mut pending_once = true;
                futures::future::poll_fn(move |cx| {
                    if pending_once {
                        pending_once = false;
                        cx.waker().wake_by_ref();
                        std::task::Poll::Pending
                    } else {
                        std::task::Poll::Ready(())
                    }
                })
                .await;
            }
            Ok(ToolResult::ok(text))
        })
    }
}

#[derive(Default)]
struct TerminalLifecycleModel {
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for TerminalLifecycleModel {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        let next = terminal_lifecycle_events_for_request(&request);
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        futures::stream::iter(next.into_iter().map(Ok)).boxed()
    }
}

#[derive(Default)]
struct CappedTerminalOutputModel {
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for CappedTerminalOutputModel {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        let next = capped_terminal_output_events_for_request(&request);
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        futures::stream::iter(next.into_iter().map(Ok)).boxed()
    }
}

#[derive(Default)]
struct TerminalStreamingModel {
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for TerminalStreamingModel {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request.clone());
        let next = terminal_streaming_events_for_request(&request);
        futures::stream::iter(next.into_iter().map(Ok)).boxed()
    }
}

fn terminal_streaming_events_for_request(request: &ChatRequest) -> Vec<AiStreamEvent> {
    const PROMPT: &str = "Stage this hunk [y,n,q,a,d,j,J,g,/,s,e,p,?]?";
    let tool_results = request
        .messages
        .iter()
        .filter_map(tool_result_text)
        .collect::<Vec<_>>();
    let turn_index = tool_results.len() + 1;
    let handle = tool_results
        .iter()
        .find_map(|content| terminal_handle(content));
    let last = tool_results.last().map(String::as_str).unwrap_or_default();

    match handle {
        None => terminal_tool_turn(
            turn_index,
            "tool_1",
            json!({
                "mode": "start",
                "command": format!(
                    "python3 - <<'PY'\nimport sys, time\nsys.stdout.write('{PROMPT} ')\nsys.stdout.flush()\ntime.sleep(1)\nPY"
                ),
                "cols": 100,
                "rows": 24
            }),
        ),
        Some(handle) if !last.contains("status: cancelled") && !last.contains("output:") => {
            terminal_tool_turn(
                turn_index,
                "tool_2",
                json!({
                    "mode": "read",
                    "handle": handle,
                    "max_output_bytes": 1024
                }),
            )
        }
        _ => end_turn_done(turn_index),
    }
}

fn capped_terminal_output_events_for_request(request: &ChatRequest) -> Vec<AiStreamEvent> {
    let tool_results = request
        .messages
        .iter()
        .filter_map(tool_result_text)
        .collect::<Vec<_>>();
    let turn_index = tool_results.len() + 1;
    let handle = tool_results
        .iter()
        .find_map(|content| terminal_handle(content));
    let last = tool_results.last().map(String::as_str).unwrap_or_default();
    match handle {
        None => terminal_tool_turn(
            turn_index,
            "tool_start",
            json!({
                "mode": "start",
                "command": "printf term; printf '%s%s%s%s' inal -runtime -leak -tail; sleep 1"
            }),
        ),
        Some(_) if last.contains("status: cancelled") => vec![
            AiStreamEvent::MessageStart {
                id: format!("msg_{turn_index}"),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        Some(handle) if last.contains("truncated: true") => terminal_tool_turn(
            turn_index,
            "tool_stop",
            json!({
                "mode": "stop",
                "handle": handle,
                "max_output_bytes": 4
            }),
        ),
        Some(_) if last.contains("status: running") => bash_tool_turn(
            turn_index,
            "tool_wait",
            json!({
                "command": "sleep 0.05; printf waited"
            }),
        ),
        Some(handle) if !last.contains("status: cancelled") => terminal_tool_turn(
            turn_index,
            "tool_read",
            json!({
                "mode": "read",
                "handle": handle,
                "max_output_bytes": 4
            }),
        ),
        _ => end_turn_done(turn_index),
    }
}

#[allow(clippy::needless_pass_by_value)]
fn bash_tool_turn(
    turn_index: usize,
    tool_id: &str,
    arguments: serde_json::Value,
) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: format!("msg_{turn_index}"),
        },
        AiStreamEvent::ToolCallStart {
            id: tool_id.to_owned(),
            name: "Bash".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: tool_id.to_owned(),
            raw_arguments: arguments.to_string(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]
}

fn terminal_lifecycle_events_for_request(request: &ChatRequest) -> Vec<AiStreamEvent> {
    let tool_results = request
        .messages
        .iter()
        .filter_map(tool_result_text)
        .collect::<Vec<_>>();
    let turn_index = tool_results.len() + 1;
    match tool_results
        .last()
        .and_then(|content| terminal_handle(content))
    {
        None => terminal_tool_turn(
            turn_index,
            "tool_start",
            json!({
                "mode": "start",
                "command": "bash --noprofile --norc",
                "cols": 44,
                "rows": 9
            }),
        ),
        Some(_)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("status: cancelled")) =>
        {
            end_turn_done(turn_index)
        }
        Some(handle)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("terminal-event-ok")) =>
        {
            terminal_tool_turn(
                turn_index,
                "tool_stop",
                json!({
                    "mode": "stop",
                    "handle": handle
                }),
            )
        }
        Some(handle)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("written: true")) =>
        {
            terminal_tool_turn(
                turn_index,
                "tool_read",
                json!({
                    "mode": "read",
                    "handle": handle,
                    "max_output_bytes": 4096
                }),
            )
        }
        Some(handle)
            if tool_results
                .last()
                .is_some_and(|content| content.contains("status: running")) =>
        {
            terminal_tool_turn(
                turn_index,
                "tool_write",
                json!({
                    "mode": "write",
                    "handle": handle,
                    "input": "printf terminal-event-ok\\n\n"
                }),
            )
        }
        _ => end_turn_done(turn_index),
    }
}

fn end_turn_done(turn_index: usize) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: format!("msg_{turn_index}"),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]
}

#[allow(clippy::needless_pass_by_value)]
fn terminal_tool_turn(
    turn_index: usize,
    tool_id: &str,
    arguments: serde_json::Value,
) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            id: format!("msg_{turn_index}"),
        },
        AiStreamEvent::ToolCallStart {
            id: tool_id.to_owned(),
            name: "Terminal".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: tool_id.to_owned(),
            raw_arguments: arguments.to_string(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::ToolUse,
            usage: None,
        },
    ]
}

fn tool_result_text(message: &neo_ai::ChatMessage) -> Option<String> {
    match message {
        neo_ai::ChatMessage::ToolResult { content, .. } => Some(
            content
                .iter()
                .filter_map(|part| match part {
                    neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => None,
    }
}

fn terminal_handle(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("handle: "))
        .map(str::trim)
        .filter(|handle| !handle.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Clone)]
struct DelayedHarness {
    model: ModelSpec,
    client: Arc<DelayedModelClient>,
}

fn model_with_capabilities(capabilities: ModelCapabilities) -> ModelSpec {
    ModelSpec {
        provider: ProviderId("capability-test".to_owned()),
        model: "capability-test-model".to_owned(),
        api: ApiKind::Local,
        capabilities,
    }
}

impl DelayedHarness {
    fn new(steps: Vec<DelayedStep>) -> Self {
        Self::from_turns([steps])
    }

    fn from_turns(turns: impl IntoIterator<Item = Vec<DelayedStep>>) -> Self {
        Self {
            model: ModelSpec {
                provider: ProviderId("delayed".to_owned()),
                model: "delayed-agent-model".to_owned(),
                api: ApiKind::Local,
                capabilities: ModelCapabilities {
                    streaming: true,
                    tools: true,
                    images: false,
                    reasoning: ReasoningCapability::None,
                    embeddings: false,
                    max_context_tokens: None,
                    max_output_tokens: None,
                },
            },
            client: Arc::new(DelayedModelClient {
                steps: Mutex::new(turns.into_iter().collect()),
                requests: Mutex::new(Vec::new()),
            }),
        }
    }

    fn model(&self) -> ModelSpec {
        self.model.clone()
    }

    fn client(&self) -> Arc<dyn ModelClient> {
        self.client.clone()
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.client
            .requests
            .lock()
            .expect("request lock poisoned")
            .clone()
    }
}

#[derive(Clone)]
enum DelayedStep {
    Event(AiStreamEvent),
    Delay(Duration),
}

struct DelayedModelClient {
    steps: Mutex<VecDeque<Vec<DelayedStep>>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ModelClient for DelayedModelClient {
    fn stream_chat(
        &self,
        request: ChatRequest,
    ) -> futures::stream::BoxStream<'static, Result<AiStreamEvent, AiError>> {
        self.requests
            .lock()
            .expect("request lock poisoned")
            .push(request);
        let steps = self
            .steps
            .lock()
            .expect("steps lock poisoned")
            .pop_front()
            .unwrap_or_default();
        futures::stream::unfold(steps.into_iter(), |mut steps| async move {
            loop {
                match steps.next()? {
                    DelayedStep::Event(event) => return Some((Ok(event), steps)),
                    DelayedStep::Delay(duration) => sleep(duration).await,
                }
            }
        })
        .boxed()
    }
}

#[tokio::test]
async fn runtime_drains_live_steer_input_at_step_boundary() {
    let harness = FakeHarness::from_turns([vec![
        AiStreamEvent::MessageStart {
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "first".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::SteerNow(
        AgentMessage::user_text("live steer"),
    ));
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_queue_modes(QueueMode::OneAtATime, QueueMode::All),
        harness.client(),
    )
    .with_steer_input(steer_input.clone());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("live-steer run should succeed");

    // The runtime must emit a SteeringQueued event when it drains the live
    // steer input, then inject the steer message before the second model call.
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::SteeringQueued { message }
            if message == &AgentMessage::user_text("live steer")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::QueueDrained { kind, count: 1 } if *kind == neo_agent_core::QueueKind::Steering
    )));
    // The steer text should appear as an appended user message before "second".
    let appended = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended { message } => Some(message.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(appended.contains(&AgentMessage::user_text("live steer")));
    // The handle is drained after the turn.
    assert_eq!(steer_input.pending(), 0);
}

#[tokio::test]
async fn runtime_drains_live_follow_up_input_as_new_turn() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::FollowUp(
        AgentMessage::user_text("queued follow"),
    ));
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_queue_modes(QueueMode::OneAtATime, QueueMode::All),
        harness.client(),
    )
    .with_steer_input(steer_input.clone());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("live-follow-up run should succeed");

    // A FollowUpQueued event must be emitted, and the follow-up must start a
    // fresh model turn after the first one ends (FIFO).
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::FollowUpQueued { message }
            if message == &AgentMessage::user_text("queued follow")
    )));
    assert_eq!(
        harness.requests().len(),
        2,
        "follow-up should trigger a second model call"
    );
    assert_eq!(steer_input.pending(), 0);
}

#[tokio::test]
async fn runtime_drains_multiple_live_follow_ups_all_by_default() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "third".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::FollowUp(
        AgentMessage::user_text("queued one"),
    ));
    steer_input.push(neo_agent_core::ActiveTurnInput::FollowUp(
        AgentMessage::user_text("queued two"),
    ));
    let runtime = AgentRuntime::new(AgentConfig::for_model(harness.model()), harness.client())
        .with_steer_input(steer_input.clone());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("multi-follow-up run should succeed");

    assert_eq!(
        harness.requests().len(),
        2,
        "default follow-up queue mode should drain all queued follow-ups into the next model turn"
    );
    let drained_counts = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::QueueDrained {
                kind: neo_agent_core::QueueKind::FollowUp,
                count,
            } => Some(*count),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        drained_counts,
        vec![2],
        "default follow-up queue mode should preserve FIFO order while draining all pending items"
    );
    let appended_users = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended {
                message: AgentMessage::User { content, .. },
            } => Some(
                content
                    .iter()
                    .filter_map(|part| match part {
                        Content::Text { text } => Some(text.as_ref()),
                        Content::Image { .. } | Content::Thinking { .. } => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(appended_users, vec!["start", "queued one", "queued two"]);
    assert_eq!(steer_input.pending(), 0);
}

#[tokio::test]
async fn runtime_drains_multiple_live_follow_ups_one_turn_at_a_time_when_configured() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "third".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::FollowUp(
        AgentMessage::user_text("queued one"),
    ));
    steer_input.push(neo_agent_core::ActiveTurnInput::FollowUp(
        AgentMessage::user_text("queued two"),
    ));
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_queue_modes(QueueMode::All, QueueMode::OneAtATime),
        harness.client(),
    )
    .with_steer_input(steer_input.clone());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("configured one-at-a-time follow-up run should succeed");

    assert_eq!(
        harness.requests().len(),
        3,
        "configured OneAtATime mode should keep each queued follow-up in its own turn"
    );
    let drained_counts = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::QueueDrained {
                kind: neo_agent_core::QueueKind::FollowUp,
                count,
            } => Some(*count),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        drained_counts,
        vec![1, 1],
        "configured OneAtATime mode should drain follow-ups FIFO one item at a time"
    );
    assert_eq!(steer_input.pending(), 0);
}

#[tokio::test]
async fn runtime_reclassifies_promoted_follow_up_as_steer_without_running_follow_up() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::PromoteFollowUpToSteer);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_queue_modes(QueueMode::OneAtATime, QueueMode::All),
        harness.client(),
    )
    .with_steer_input(steer_input.clone());
    let mut context = AgentContext::new();
    context.queue_follow_up_message(AgentMessage::user_text("queued follow"));

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("promoted follow-up run should succeed");

    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::FollowUpQueued { message }
            if message == &AgentMessage::user_text("queued follow")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::QueueDrained { kind, count: 1 }
            if *kind == neo_agent_core::QueueKind::FollowUp
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::SteeringQueued { message }
            if message == &AgentMessage::user_text("queued follow")
    )));
    assert_eq!(
        harness.requests().len(),
        1,
        "promoted follow-up should run once as a steer, not again as a follow-up"
    );
    assert!(matches!(
        harness.requests()[0].messages.last(),
        Some(neo_ai::ChatMessage::User { content }) if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text == "queued follow"
        )
    ));
    assert_eq!(context.pending_follow_up_len(), 0);
    assert_eq!(context.pending_steering_len(), 0);
    assert_eq!(steer_input.pending(), 0);
}

#[tokio::test]
async fn runtime_dequeues_follow_up_for_edit_without_running_it() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let steer_input = neo_agent_core::SteerInputHandle::new();
    steer_input.push(neo_agent_core::ActiveTurnInput::DequeueFollowUpForEdit);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_queue_modes(QueueMode::OneAtATime, QueueMode::All),
        harness.client(),
    )
    .with_steer_input(steer_input.clone());
    let mut context = AgentContext::new();
    context.queue_follow_up_message(AgentMessage::user_text("queued follow"));

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("start"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("dequeued follow-up run should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::QueueDrained { kind, count: 1 }
            if *kind == neo_agent_core::QueueKind::FollowUp
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::SteeringQueued { message }
            if message == &AgentMessage::user_text("queued follow")
    )));
    assert_eq!(
        harness.requests().len(),
        1,
        "dequeued follow-up should not run as an automatic follow-up turn"
    );
    assert_eq!(context.pending_follow_up_len(), 0);
    assert_eq!(steer_input.pending(), 0);
}

// ---------------------------------------------------------------------------
// NEO-30 Layer 1/2/3: approval key scoping, prefix rules, safety classification
// ---------------------------------------------------------------------------

fn count_approval_requests(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, AgentEvent::ApprovalRequested { .. }))
        .count()
}

fn first_approval_request(events: &[AgentEvent]) -> &ApprovalRequest {
    events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequested { request } => Some(request),
            _ => None,
        })
        .expect("expected ApprovalRequested")
}

async fn collect_approval_request_for_tool(
    name: &str,
    raw_arguments: serde_json::Value,
    workspace: &std::path::Path,
) -> ApprovalRequest {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: name.to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: raw_arguments.to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace)
            .expect("workspace root")
            .with_approval_handler(|request| permit_once(request)),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("approve me"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
    first_approval_request(&events).clone()
}

#[tokio::test]
async fn approval_requests_only_offer_runtime_supported_actions() {
    let workspace = tempfile::tempdir().expect("workspace");

    let background = collect_approval_request_for_tool(
        "Bash",
        json!({ "command": "sleep 1", "run_in_background": true }),
        workspace.path(),
    )
    .await;
    assert_eq!(
        background
            .options
            .iter()
            .map(|option| &option.action)
            .collect::<Vec<_>>(),
        vec![&ApprovalAction::PermitOnce, &ApprovalAction::Reject],
    );
    assert!(matches!(
        background.presentation,
        ApprovalPresentation::Command { .. }
    ));

    let foreground = collect_approval_request_for_tool(
        "Bash",
        json!({ "command": "python script.py" }),
        workspace.path(),
    )
    .await;
    assert!(matches!(
        foreground.options.as_slice(),
        [
            ApprovalOption {
                action: ApprovalAction::PermitOnce,
                ..
            },
            ApprovalOption {
                action: ApprovalAction::PermitForSession { .. },
                ..
            },
            ApprovalOption {
                action: ApprovalAction::PermitForPrefix { .. },
                ..
            },
            ApprovalOption {
                action: ApprovalAction::Reject,
                ..
            },
        ]
    ));
    assert!(matches!(
        foreground.presentation,
        ApprovalPresentation::Command { .. }
    ));

    let write = collect_approval_request_for_tool(
        "Write",
        json!({ "path": "approved.txt", "content": "ok" }),
        workspace.path(),
    )
    .await;
    assert!(matches!(
        write.options.as_slice(),
        [
            ApprovalOption {
                action: ApprovalAction::PermitOnce,
                ..
            },
            ApprovalOption {
                action: ApprovalAction::PermitForSession { .. },
                ..
            },
            ApprovalOption {
                action: ApprovalAction::Reject,
                ..
            },
        ]
    ));
    assert!(matches!(
        write.presentation,
        ApprovalPresentation::Tool { .. }
    ));
}

#[tokio::test]
async fn layer1_bash_session_approval_exact_command_only() {
    // Approving `git status` must NOT cover `git log`. Core regression test.
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "command": "git status" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_2".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_2".to_owned(),
                raw_arguments: json!({ "command": "python script.py" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let approval_count = Arc::new(Mutex::new(0));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_approval_handler({
                let count = Arc::clone(&approval_count);
                move |request| {
                    *count.lock().expect("count lock poisoned") += 1;
                    permit_for_session(request)
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("git status then git log"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
    // `git status` is auto-approved by Layer 3 (safe git subcommand), so only
    // `git log --oneline -20` reaches the handler. This proves the safe-command
    // path + that different commands don't share approval.
    assert!(
        count_approval_requests(&events) <= 1,
        "git status (safe) auto-approves; git log is a different command and must not inherit"
    );
}

#[tokio::test]
async fn allow_for_session_does_not_persist_prefix_rule() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "command": "python script.py" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = AgentConfig::for_model(harness.model())
        .with_permission_mode(PermissionMode::Ask)
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_approval_handler(|request| permit_for_session(request));
    let prefix_store = Arc::clone(&config.prefix_approval_rules);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("python script"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalRequested { request }
            if request.options.iter().any(|option| matches!(
                &option.action,
                ApprovalAction::PermitForPrefix { rule }
                    if rule.prefix == vec!["python".to_owned()]
            ))
    )));
    assert!(
        prefix_store
            .lock()
            .expect("prefix store")
            .prefix_rules
            .is_empty(),
        "AllowForSession must not persist prefix approval rules"
    );
}

#[tokio::test]
async fn allow_for_prefix_persists_prefix_rule() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "command": "python script.py" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let config = AgentConfig::for_model(harness.model())
        .with_permission_mode(PermissionMode::Ask)
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_approval_handler(|request| permit_for_prefix(request));
    let prefix_store = Arc::clone(&config.prefix_approval_rules);
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("python script"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(
        prefix_store
            .lock()
            .expect("prefix store")
            .prefix_rules
            .iter()
            .map(|rule| rule.prefix.clone())
            .collect::<Vec<_>>(),
        vec![vec!["python".to_owned()]]
    );
}

#[tokio::test]
async fn layer3_safe_command_auto_approved() {
    // `cat README.md` is a known-safe command — it should not prompt at all.
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "command": "cat README.md" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let approval_count = Arc::new(Mutex::new(0));
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_approval_handler({
                let count = Arc::clone(&approval_count);
                move |request| {
                    *count.lock().expect("count lock poisoned") += 1;
                    permit_once(request)
                }
            }),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("cat readme"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
    assert_eq!(
        count_approval_requests(&events),
        0,
        "known-safe commands like `cat` must be auto-approved without prompt"
    );
}

#[tokio::test]
async fn layer3_dangerous_command_forces_prompt_no_scope() {
    // `rm -rf /tmp/x` is dangerous — it must prompt and offer NO session scope.
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "Bash".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: json!({ "command": "rm -rf /tmp/x" }).to_string(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        final_done_turn(),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Ask)
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_approval_handler(|request| permit_once(request)),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("rm"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");
    assert_eq!(
        count_approval_requests(&events),
        1,
        "dangerous commands must prompt"
    );
    // The approval event must offer NO session option (so it can't be cached).
    let has_scope = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ApprovalRequested { request }
                if request.options.iter().any(|option| {
                    matches!(option.action, ApprovalAction::PermitForSession { .. })
                })
        )
    });
    assert!(
        !has_scope,
        "dangerous commands must not offer a reusable session scope"
    );
}

#[tokio::test]
async fn runtime_invalid_tool_arguments_return_model_visible_error() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: r#"{"text":"neo"#.into(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "retrying".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call echo"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // 1. No ToolExecutionStarted — execution never begins for invalid args.
    assert!(
        !events.iter().any(
            |event| matches!(event, AgentEvent::ToolExecutionStarted { name, .. } if name == "echo")
        ),
        "invalid tool arguments must not start execution"
    );

    // 2. A ToolExecutionFinished with an error result is emitted.
    let error_event = events.iter().find(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionFinished { name, result, .. }
                if name == "echo" && result.is_error
        )
    });
    let error_event = error_event.expect("expected a ToolExecutionFinished error event");
    if let AgentEvent::ToolExecutionFinished { result, .. } = error_event {
        assert!(
            result.content.contains("Tool arguments were invalid JSON"),
            "error content should mention invalid JSON, got: {}",
            result.content
        );
    }

    // 3. The model gets a second turn (error is fed back).
    assert_eq!(harness.requests().len(), 2);

    // 4. The second request's messages end with a ToolResult containing the error.
    let requests = harness.requests();
    let last_message = requests[1].messages.last();
    assert!(
        matches!(
            last_message,
            Some(neo_ai::ChatMessage::ToolResult { content, is_error, .. })
                if *is_error
                    && content.iter().any(|part| matches!(part,
                        neo_ai::ContentPart::Text { text } if text.contains("invalid JSON")
                    ))
        ),
        "second request should end with an error ToolResult"
    );
}

// ---------------------------------------------------------------------------
// Path-scoped instruction context bridge: prefix stability, dynamic budget,
// compaction exclusion, and exact rehydration.
// ---------------------------------------------------------------------------

use neo_agent_core::InstructionContextBridge;
use neo_agent_core::instructions::{
    InstructionEpochData, InstructionEpochOutcome, InstructionFailureKind, InstructionFingerprint,
    InstructionPreflightDecision, InstructionReconcileKind, InstructionReconcileRequest,
    InstructionRegistry, InstructionRegistryConfig,
};
use std::path::PathBuf;

struct InstructionFixture {
    _temp: tempfile::TempDir,
    workspace: PathBuf,
    registry: InstructionRegistry,
}

/// A trusted workspace with a root `AGENTS.md` plus optional nested scopes.
fn instruction_fixture(nested: &[(&str, &str)], root_rules: &str) -> InstructionFixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("AGENTS.md"), root_rules).expect("root AGENTS.md");
    for (dir, rules) in nested {
        let nested_dir = workspace.join(dir);
        std::fs::create_dir_all(&nested_dir).expect("nested dir");
        std::fs::write(nested_dir.join("AGENTS.md"), rules).expect("nested AGENTS.md");
    }
    let workspace = workspace.canonicalize().expect("canonical workspace");
    let registry = InstructionRegistry::new(InstructionRegistryConfig {
        primary_workspace: workspace.clone(),
        neo_home: None,
        project_trusted: true,
    })
    .expect("registry");
    InstructionFixture {
        _temp: temp,
        workspace,
        registry,
    }
}

async fn reconcile_defer_epoch(
    fixture: &InstructionFixture,
    config: &AgentConfig,
    context: &AgentContext,
    targets: Vec<PathBuf>,
) -> (InstructionEpochData, InstructionFingerprint) {
    let budget = InstructionContextBridge::budget(config, context);
    let decision = fixture
        .registry
        .reconcile(
            InstructionReconcileRequest {
                agent_id: "main".to_owned(),
                kind: InstructionReconcileKind::ToolPreflight,
                target_directories: targets,
                budget,
                deferred_tool_ids: vec!["call-1".to_owned()],
            },
            context.instruction_state(),
        )
        .await;
    match decision {
        InstructionPreflightDecision::Defer { epoch, fingerprint } => (epoch, fingerprint),
        InstructionPreflightDecision::Proceed { .. } => panic!("expected Defer, got Proceed"),
        InstructionPreflightDecision::Block { epoch, .. } => {
            panic!("expected Defer, got Block: {:?}", epoch.failure)
        }
    }
}

fn chat_request_text(request: &ChatRequest) -> String {
    request
        .messages
        .iter()
        .map(|message| {
            let content = match message {
                neo_ai::ChatMessage::System { content }
                | neo_ai::ChatMessage::User { content }
                | neo_ai::ChatMessage::Assistant { content, .. }
                | neo_ai::ChatMessage::ToolResult { content, .. } => content,
            };
            content
                .iter()
                .filter_map(|part| match part {
                    neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn request_contains_exact_text(request: &ChatRequest, expected: &str) -> bool {
    request_exact_text_count(request, expected) > 0
}

fn request_exact_text_count(request: &ChatRequest, expected: &str) -> usize {
    request
        .messages
        .iter()
        .map(|message| {
            let content = match message {
                neo_ai::ChatMessage::System { content }
                | neo_ai::ChatMessage::User { content }
                | neo_ai::ChatMessage::Assistant { content, .. }
                | neo_ai::ChatMessage::ToolResult { content, .. } => content,
            };
            content
                .iter()
                .filter(
                    |part| matches!(part, neo_ai::ContentPart::Text { text } if text == expected),
                )
                .count()
        })
        .sum()
}

fn end_turn_events(text: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]
}

async fn run_turn_collect(
    runtime: &AgentRuntime,
    context: &mut AgentContext,
    input: &str,
) -> Vec<AgentEvent> {
    runtime
        .run_turn(context, AgentMessage::user_text(input))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed")
}

async fn apply_preflight_baseline(
    fixture: &PreflightFixture,
    config: &AgentConfig,
    context: &mut AgentContext,
) -> String {
    let (epoch, fingerprint) = match fixture
        .registry
        .reconcile(
            InstructionReconcileRequest {
                agent_id: "main".to_owned(),
                kind: InstructionReconcileKind::Baseline,
                target_directories: Vec::new(),
                budget: InstructionContextBridge::budget(config, context),
                deferred_tool_ids: Vec::new(),
            },
            context.instruction_state(),
        )
        .await
    {
        InstructionPreflightDecision::Defer { epoch, fingerprint } => (epoch, fingerprint),
        _ => panic!("expected baseline Defer"),
    };
    let authority = epoch.model_content.clone().expect("baseline authority");
    InstructionContextBridge::apply_epoch(context, &epoch, &fingerprint);
    authority
}

#[tokio::test]
async fn threshold_compaction_rehydrates_exact_authority_before_same_request() {
    let fixture = preflight_fixture(&[], "# exact threshold authority\n");
    let harness = FakeHarness::from_turns([
        end_turn_events("summary output"),
        end_turn_events("continued"),
    ]);
    let baseline_config = preflight_config(&fixture, &harness);
    let mut context = preflight_context(&fixture);
    let authority = apply_preflight_baseline(&fixture, &baseline_config, &mut context).await;
    let mut config = baseline_config.with_compaction(CompactionSettings {
        trigger_ratio: 0.05,
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 1)
    });
    config.model.capabilities.max_context_tokens = Some(200_000);
    context.append_message(AgentMessage::user_text("history ".repeat(40_000)));
    context.append_message(AgentMessage::assistant(
        [Content::text("previous answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));

    let events = run_turn_collect(
        &AgentRuntime::new(config, harness.client()),
        &mut context,
        "go",
    )
    .await;

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
    );
    let requests = harness.requests();
    assert_eq!(requests.len(), 2, "summary then continued request");
    assert!(
        request_contains_exact_text(&requests[1], &authority),
        "the first provider request after threshold compaction must contain the exact authority"
    );
}

#[tokio::test]
async fn history_pressure_compacts_before_baseline_selection_and_user_append() {
    let root_rules = format!("# exact resumed authority\n{}\n", "r".repeat(64_000));
    let fixture = preflight_fixture(&[], &root_rules);
    let harness = FakeHarness::from_turns([
        end_turn_events("summary output"),
        end_turn_events("continued"),
    ]);
    let mut config = preflight_config(&fixture, &harness).with_compaction(CompactionSettings {
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 3)
    });
    config.model.capabilities.max_context_tokens = Some(32_000);
    let mut context = preflight_context(&fixture);
    context.append_message(AgentMessage::user_text(format!(
        "old history {}",
        "x".repeat(40_000)
    )));
    context.append_message(AgentMessage::assistant(
        [Content::text("old answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));

    let events = run_turn_collect(
        &AgentRuntime::new(config, harness.client()),
        &mut context,
        "next prompt",
    )
    .await;

    let compaction_index = event_index(&events, |event| {
        matches!(event, AgentEvent::CompactionApplied { .. })
    })
    .expect("history compaction");
    let epoch_index = event_index(&events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { .. })
    })
    .expect("baseline epoch");
    let user_index = event_index(&events, |event| {
        matches!(
            event,
            AgentEvent::MessageAppended { message }
                if message.text() == "next prompt"
        )
    })
    .expect("new user message");
    assert!(compaction_index < epoch_index && epoch_index < user_index);
    let epoch = instruction_epochs(&events)[0];
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Ready);
    assert!(epoch.ignored_bundles.is_empty());
    let authority = epoch
        .model_content
        .as_deref()
        .expect("full baseline authority");
    assert!(request_contains_exact_text(
        &harness.requests()[1],
        authority
    ));
}

#[tokio::test]
async fn history_pressure_compacts_before_blocked_baseline_notice_and_user_append() {
    let fixture = preflight_fixture(&[], "@./missing.md\n");
    let harness = FakeHarness::from_turns([
        end_turn_events("summary output"),
        end_turn_events("continued"),
    ]);
    let mut config = preflight_config(&fixture, &harness).with_compaction(CompactionSettings {
        trigger_ratio: 0.3,
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 3)
    });
    config.model.capabilities.max_context_tokens = Some(32_000);
    let mut context = preflight_context(&fixture);
    context.append_message(AgentMessage::user_text("old history ".repeat(40_000)));
    context.append_message(AgentMessage::assistant(
        [Content::text("old answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));

    let events = run_turn_collect(
        &AgentRuntime::new(config, harness.client()),
        &mut context,
        "next prompt",
    )
    .await;

    let compaction_index = event_index(&events, |event| {
        matches!(event, AgentEvent::CompactionApplied { .. })
    })
    .expect("history compaction");
    let epoch_index = event_index(&events, |event| {
        matches!(
            event,
            AgentEvent::InstructionEpoch { epoch }
                if epoch.outcome == InstructionEpochOutcome::Blocked
        )
    })
    .expect("Blocked baseline epoch");
    let user_index = event_index(&events, |event| {
        matches!(
            event,
            AgentEvent::MessageAppended { message }
                if message.text() == "next prompt"
        )
    })
    .expect("new user message");
    assert!(compaction_index < epoch_index && epoch_index < user_index);
    let blocked = instruction_epochs(&events)[0]
        .model_content
        .as_deref()
        .expect("Blocked notice");
    assert!(request_contains_exact_text(&harness.requests()[1], blocked));
}

#[tokio::test]
async fn overflow_recovery_rehydrates_exact_authority_before_retry_request() {
    let fixture = preflight_fixture(&[], "# exact overflow authority\n");
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::ContextOverflow {
            message: "too many tokens".to_owned(),
        })],
        end_turn_events("summary output")
            .into_iter()
            .map(Ok)
            .collect::<Vec<_>>(),
        end_turn_events("recovered")
            .into_iter()
            .map(Ok)
            .collect::<Vec<_>>(),
    ]);
    let mut config = preflight_config(&fixture, &harness)
        .with_compaction(CompactionSettings::new(usize::MAX, 1));
    config.model.capabilities.max_context_tokens = Some(200_000);
    let mut context = preflight_context(&fixture);
    let authority = apply_preflight_baseline(&fixture, &config, &mut context).await;
    context.append_message(AgentMessage::user_text("old history"));
    context.append_message(AgentMessage::assistant(
        [Content::text("old answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));

    let events = run_turn_collect(
        &AgentRuntime::new(config, harness.client()),
        &mut context,
        "go",
    )
    .await;

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
    );
    let requests = harness.requests();
    assert_eq!(requests.len(), 3, "initial, summary, retry");
    assert!(
        request_contains_exact_text(&requests[2], &authority),
        "the overflow retry request must contain the exact authority"
    );
}

#[tokio::test]
async fn retained_blocked_notice_does_not_replace_compacted_authority() {
    let fixture = preflight_fixture(&[], "# exact prior authority\n");
    let harness = FakeHarness::from_turns([end_turn_events("continued")]);
    let config = preflight_config(&fixture, &harness);
    let mut context = preflight_context(&fixture);
    let authority = apply_preflight_baseline(&fixture, &config, &mut context).await;
    context.append_message(AgentMessage::user_text("ordinary history"));
    std::fs::write(fixture.workspace.join("AGENTS.md"), "@./missing.md\n")
        .expect("break root bundle");
    let (blocked, blocked_fingerprint) = match fixture
        .registry
        .reconcile(
            InstructionReconcileRequest {
                agent_id: "main".to_owned(),
                kind: InstructionReconcileKind::ToolPreflight,
                target_directories: vec![fixture.workspace.clone()],
                budget: InstructionContextBridge::budget(&config, &context),
                deferred_tool_ids: vec!["blocked-call".to_owned()],
            },
            context.instruction_state(),
        )
        .await
    {
        InstructionPreflightDecision::Block { epoch, fingerprint } => (epoch, fingerprint),
        _ => panic!("expected Block"),
    };
    let blocked_notice = blocked.model_content.clone().expect("blocked notice");
    InstructionContextBridge::apply_epoch(&mut context, &blocked, &blocked_fingerprint);
    let blocked_index = context.messages().len() - 1;
    context.apply_compaction(CompactionSummary {
        summary: "summary".to_owned(),
        tokens_before: 100,
        tokens_after: 10,
        first_kept_message_index: blocked_index,
    });

    run_turn_collect(
        &AgentRuntime::new(config, harness.client()),
        &mut context,
        "continue",
    )
    .await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        request_exact_text_count(&requests[0], &authority),
        1,
        "the complete authority snapshot must be present exactly once"
    );
    assert_eq!(
        request_exact_text_count(&requests[0], &blocked_notice),
        1,
        "rehydration must preserve exactly one current Blocked notice"
    );
}

async fn run_manual_compaction_collect(runtime: &AgentRuntime, context: &mut AgentContext) {
    let events = runtime
        .run_manual_compaction_turn(context)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("compaction turn should succeed");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. })),
        "expected a CompactionApplied event"
    );
}

#[tokio::test]
async fn adjacent_requests_keep_the_complete_previous_message_prefix() {
    let fixture = instruction_fixture(&[("nested", "nested rules\n")], "root rules\n");
    let harness =
        FakeHarness::from_turns([end_turn_events("reply one"), end_turn_events("reply two")]);
    let tool = ToolSpec {
        name: "Read".to_owned(),
        description: "read a file".to_owned(),
        input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    };
    let config = AgentConfig::for_model(harness.model())
        .with_system_prompt("BASE SYSTEM PROMPT")
        .with_tools(vec![tool])
        .with_workspace_root(&fixture.workspace)
        .expect("workspace root")
        .with_session_directory(
            fixture
                .workspace
                .join("session_00000000-0000-4000-8000-0000000000aa"),
        );
    let runtime = AgentRuntime::new(config.clone(), harness.client());
    let mut context = AgentContext::new();

    run_turn_collect(&runtime, &mut context, "first request").await;

    // Activate the nested scope between the two provider requests.
    let (epoch, fingerprint) = reconcile_defer_epoch(
        &fixture,
        &config,
        &context,
        vec![fixture.workspace.join("nested")],
    )
    .await;
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Activated);
    InstructionContextBridge::apply_epoch(&mut context, &epoch, &fingerprint);

    run_turn_collect(&runtime, &mut context, "second request").await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    let first = &requests[0];
    let second = &requests[1];

    // The complete earlier message sequence is the exact prefix of the next
    // pre-compaction request; the epoch only appends.
    assert!(
        first.messages.len() < second.messages.len(),
        "the epoch and the follow-up exchange must append messages"
    );
    assert_eq!(
        first.messages.as_slice(),
        &second.messages[..first.messages.len()],
        "request N messages must be the exact prefix of request N+1"
    );

    // Stable system prompt bytes, tool ordering, reasoning settings, and the
    // session cache key across scope activation.
    let system_text = |request: &ChatRequest| match request.messages.first() {
        Some(neo_ai::ChatMessage::System { content }) => content
            .iter()
            .filter_map(|part| match part {
                neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>(),
        other => panic!("expected leading system message, got {other:?}"),
    };
    assert_eq!(system_text(first), system_text(second));
    assert_eq!(system_text(first), "BASE SYSTEM PROMPT");
    assert_eq!(first.tools, second.tools);
    assert_eq!(first.options.reasoning, second.options.reasoning);
    assert_eq!(first.options.session_id, second.options.session_id);
    assert_eq!(
        first.options.session_id.as_deref(),
        Some("session_00000000-0000-4000-8000-0000000000aa")
    );
}

#[tokio::test]
async fn compaction_excludes_instruction_bodies_and_rehydrates_exact_bytes() {
    const INSTRUCTION_SENTINEL: &str = "INSTRUCTION-SENTINEL-4f8c2e-rules";
    const ORDINARY_SENTINEL: &str = "ORDINARY-SENTINEL-91bd7a-history";

    let fixture = instruction_fixture(&[], &format!("# rules\n{INSTRUCTION_SENTINEL}\n"));
    let harness = FakeHarness::from_turns([
        end_turn_events("summary output"),
        end_turn_events("continued"),
    ]);
    let mut config = AgentConfig::for_model(harness.model())
        .with_compaction(CompactionSettings::new(usize::MAX, 4));
    config.manual_compact_request = Arc::new(Mutex::new(Some(String::new())));
    let runtime = AgentRuntime::new(config.clone(), harness.client());
    let mut context = AgentContext::new();

    // Pin the workspace baseline epoch carrying the instruction sentinel.
    let (epoch, fingerprint) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![fixture.workspace.clone()]).await;
    let model_content = epoch
        .model_content
        .clone()
        .expect("baseline epoch carries model content");
    assert!(model_content.contains(INSTRUCTION_SENTINEL));
    InstructionContextBridge::apply_epoch(&mut context, &epoch, &fingerprint);

    // Ordinary history with its own sentinel, then a manual compaction.
    context.append_message(AgentMessage::user_text(format!(
        "please remember {ORDINARY_SENTINEL}"
    )));
    context.append_message(AgentMessage::assistant(
        vec![Content::text("noted")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    context.append_message(AgentMessage::user_text("and now something else"));
    context.append_message(AgentMessage::assistant(
        vec![Content::text("done")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    run_manual_compaction_collect(&runtime, &mut context).await;

    // The summary request excludes the instruction body but still summarizes
    // ordinary history.
    let requests = harness.requests();
    assert_eq!(requests.len(), 1);
    let summary_text = chat_request_text(&requests[0]);
    assert!(
        !summary_text.contains(INSTRUCTION_SENTINEL),
        "summary input must exclude pinned instruction bodies: {summary_text}"
    );
    assert!(
        summary_text.contains(ORDINARY_SENTINEL),
        "summary input must keep ordinary history: {summary_text}"
    );

    // Rehydrate the exact current rules from registry state.
    let repinned =
        InstructionContextBridge::rehydrate_after_compaction(&fixture.registry, &mut context)
            .await
            .expect("rehydration succeeds");
    assert!(repinned, "current instruction chain must be re-pinned");

    run_turn_collect(&runtime, &mut context, "continue working").await;

    // The post-compaction request contains the byte-identical instruction
    // content exactly once.
    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    let post_compaction = chat_request_text(&requests[1]);
    assert_eq!(
        post_compaction.matches(INSTRUCTION_SENTINEL).count(),
        1,
        "instruction sentinel must appear exactly once: {post_compaction}"
    );
    let pinned = requests[1]
        .messages
        .iter()
        .map(|message| {
            let parts = match message {
                neo_ai::ChatMessage::System { content }
                | neo_ai::ChatMessage::User { content }
                | neo_ai::ChatMessage::Assistant { content, .. }
                | neo_ai::ChatMessage::ToolResult { content, .. } => content,
            };
            parts
                .iter()
                .filter_map(|part| match part {
                    neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .find(|text| text.contains(INSTRUCTION_SENTINEL))
        .expect("one message carries the rehydrated sentinel");
    assert_eq!(
        pinned, model_content,
        "rehydrated content must be byte-identical to the epoch content"
    );
}

#[tokio::test]
async fn compaction_rehydration_never_admits_previously_ignored_bundle() {
    const ROOT: &str = "ROOT-ADMITTED-41b7";
    const IGNORED: &str = "NESTED-IGNORED-e230";
    let nested_rules = format!("{IGNORED} {}\n", "large ".repeat(2_000));
    let fixture = instruction_fixture(&[("nested", &nested_rules)], &format!("{ROOT}\n"));
    let nested = fixture.workspace.join("nested");
    let request = InstructionReconcileRequest {
        agent_id: "main".to_owned(),
        kind: InstructionReconcileKind::ToolPreflight,
        target_directories: vec![nested],
        budget: neo_agent_core::instructions::InstructionBudget {
            nominal: 65_536,
            actual: 512,
        },
        deferred_tool_ids: vec!["call-1".to_owned()],
    };
    let context = &mut AgentContext::new();
    let (epoch, fingerprint) = match fixture
        .registry
        .reconcile(request, context.instruction_state())
        .await
    {
        InstructionPreflightDecision::Defer { epoch, fingerprint } => (epoch, fingerprint),
        InstructionPreflightDecision::Proceed { .. } => {
            panic!("expected partially loaded epoch, got Proceed")
        }
        InstructionPreflightDecision::Block { epoch, .. } => {
            panic!(
                "expected partially loaded epoch, got Block: {:?}",
                epoch.failure
            )
        }
    };
    assert_eq!(epoch.outcome, InstructionEpochOutcome::PartiallyLoaded);
    assert!(
        epoch
            .model_content
            .as_deref()
            .is_some_and(|body| body.contains(ROOT)),
        "{epoch:?}"
    );
    assert!(
        epoch
            .model_content
            .as_deref()
            .is_some_and(|body| !body.contains(IGNORED))
    );
    InstructionContextBridge::apply_epoch(context, &epoch, &fingerprint);

    InstructionContextBridge::rehydrate_after_compaction(&fixture.registry, context)
        .await
        .expect("rehydration succeeds");

    let pinned = context
        .messages()
        .iter()
        .filter(|message| matches!(message, AgentMessage::Instruction { .. }))
        .map(AgentMessage::text)
        .collect::<String>();
    assert!(
        pinned.contains(ROOT),
        "admitted root must remain pinned: {pinned}"
    );
    assert!(
        !pinned.contains(IGNORED),
        "ignored nested bundle must remain unpinned: {pinned}"
    );
    assert_eq!(
        context.instruction_state().visited_revisions,
        context.instruction_state().visible_revisions,
        "ignored bundles must never enter agent-local visited history"
    );
}

#[tokio::test]
async fn compacted_sibling_scope_reactivates_when_reentered() {
    let fixture = instruction_fixture(
        &[("a", "ALPHA-RULES-a11\n"), ("b", "BETA-RULES-b22\n")],
        "ROOT-RULES-c01\n",
    );
    let scope_a = fixture.workspace.join("a");
    let scope_b = fixture.workspace.join("b");
    let harness = FakeHarness::from_turns([end_turn_events("summary output")]);
    let mut config = AgentConfig::for_model(harness.model())
        .with_compaction(CompactionSettings::new(usize::MAX, 4));
    config.manual_compact_request = Arc::new(Mutex::new(Some(String::new())));
    let runtime = AgentRuntime::new(config.clone(), harness.client());
    let mut context = AgentContext::new();

    // Activate sibling scope A, then sibling scope B.
    let (epoch_a, fingerprint_a) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![scope_a.clone()]).await;
    assert_eq!(epoch_a.outcome, InstructionEpochOutcome::Activated);
    InstructionContextBridge::apply_epoch(&mut context, &epoch_a, &fingerprint_a);
    let (epoch_b, fingerprint_b) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![scope_b.clone()]).await;
    InstructionContextBridge::apply_epoch(&mut context, &epoch_b, &fingerprint_b);
    context.append_message(AgentMessage::user_text("working in b"));

    // Compact while B is current, then rehydrate from registry state.
    run_manual_compaction_collect(&runtime, &mut context).await;
    let repinned =
        InstructionContextBridge::rehydrate_after_compaction(&fixture.registry, &mut context)
            .await
            .expect("rehydration succeeds");
    assert!(repinned);

    // A remains cached but unpinned; the current chain (root + B) is pinned.
    let pinned = context
        .messages()
        .iter()
        .filter(|message| matches!(message, AgentMessage::Instruction { .. }))
        .map(AgentMessage::text)
        .collect::<Vec<_>>()
        .concat();
    assert!(
        !pinned.contains("ALPHA-RULES"),
        "sibling A must stay unpinned"
    );
    assert!(
        pinned.contains("ROOT-RULES"),
        "workspace baseline is rehydrated"
    );
    assert!(
        pinned.contains("BETA-RULES"),
        "current scope B is rehydrated"
    );

    let state = context.instruction_state();
    assert_eq!(state.active_scopes, vec![fixture.workspace.clone()]);
    assert_eq!(state.most_recent_scope.as_deref(), Some(scope_b.as_path()));
    for scope in [&fixture.workspace, &scope_b] {
        assert!(
            state.visible_revisions.contains_key(scope),
            "current authority retained for {}",
            scope.display()
        );
    }
    assert!(!state.visible_revisions.contains_key(&scope_a));
    for scope in [&fixture.workspace, &scope_a, &scope_b] {
        assert!(
            state.visited_revisions.contains_key(scope),
            "visited metadata retained for {}",
            scope.display()
        );
    }

    // Re-entering A emits exactly one Reactivated epoch.
    let (epoch_reentry, fingerprint_reentry) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![scope_a.clone()]).await;
    assert_eq!(epoch_reentry.outcome, InstructionEpochOutcome::Reactivated);
    assert!(
        epoch_reentry
            .model_content
            .as_deref()
            .is_some_and(|content| content.contains("ALPHA-RULES")),
        "re-entry re-pins A's exact content"
    );
    InstructionContextBridge::apply_epoch(&mut context, &epoch_reentry, &fingerprint_reentry);

    // The identical probe afterwards proceeds silently — no second epoch.
    let decision = fixture
        .registry
        .reconcile(
            InstructionReconcileRequest {
                agent_id: "main".to_owned(),
                kind: InstructionReconcileKind::ToolPreflight,
                target_directories: vec![scope_a],
                budget: InstructionContextBridge::budget(&config, &context),
                deferred_tool_ids: vec!["call-1".to_owned()],
            },
            context.instruction_state(),
        )
        .await;
    assert!(
        matches!(decision, InstructionPreflightDecision::Proceed { .. }),
        "an unchanged scope must not emit a second epoch"
    );
}

#[tokio::test]
async fn replayed_compacted_sibling_reactivates_with_fresh_registry() {
    let fixture = instruction_fixture(
        &[("a", "ALPHA-RULES-a11\n"), ("b", "BETA-RULES-b22\n")],
        "ROOT-RULES-c01\n",
    );
    let scope_a = fixture.workspace.join("a");
    let scope_b = fixture.workspace.join("b");
    let harness = FakeHarness::from_turns([end_turn_events("summary output")]);
    let mut config = AgentConfig::for_model(harness.model())
        .with_compaction(CompactionSettings::new(usize::MAX, 4));
    config.manual_compact_request = Arc::new(Mutex::new(Some(String::new())));
    let runtime = AgentRuntime::new(config.clone(), harness.client());

    let mut live = AgentContext::new();
    let (epoch_a, fingerprint_a) =
        reconcile_defer_epoch(&fixture, &config, &live, vec![scope_a.clone()]).await;
    InstructionContextBridge::apply_epoch(&mut live, &epoch_a, &fingerprint_a);
    let (epoch_b, _fingerprint_b) =
        reconcile_defer_epoch(&fixture, &config, &live, vec![scope_b.clone()]).await;

    let replay_events = [
        AgentEvent::InstructionEpoch { epoch: epoch_a },
        AgentEvent::InstructionEpoch {
            epoch: epoch_b.clone(),
        },
    ];
    let mut replayed = AgentContext::from_replay(replay_events.iter());
    for scope in [&fixture.workspace, &scope_a, &scope_b] {
        assert!(
            replayed
                .instruction_state()
                .visited_revisions
                .contains_key(scope),
            "replay retained {}",
            scope.display()
        );
    }

    let fresh_registry = InstructionRegistry::new(InstructionRegistryConfig {
        primary_workspace: fixture.workspace.clone(),
        neo_home: None,
        project_trusted: true,
    })
    .expect("fresh registry");
    fresh_registry.restore_epoch(&epoch_b);
    replayed.append_message(AgentMessage::user_text("working in b after replay"));
    run_manual_compaction_collect(&runtime, &mut replayed).await;
    InstructionContextBridge::rehydrate_after_compaction(&fresh_registry, &mut replayed)
        .await
        .expect("fresh-registry rehydration");

    assert!(
        replayed
            .instruction_state()
            .visited_revisions
            .contains_key(&scope_a),
        "rehydration must preserve replayed agent-local history"
    );
    let decision = fresh_registry
        .reconcile(
            InstructionReconcileRequest {
                agent_id: "main".to_owned(),
                kind: InstructionReconcileKind::ToolPreflight,
                target_directories: vec![scope_a],
                budget: InstructionContextBridge::budget(&config, &replayed),
                deferred_tool_ids: vec!["call-1".to_owned()],
            },
            replayed.instruction_state(),
        )
        .await;
    let InstructionPreflightDecision::Defer { epoch, .. } = decision else {
        panic!("re-entering replayed sibling must defer")
    };
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Reactivated);
}

#[tokio::test]
async fn compacted_current_nested_scope_removal_emits_removed_epoch() {
    let fixture = instruction_fixture(&[("nested", "NESTED-RULES\n")], "ROOT-RULES\n");
    let nested = fixture.workspace.join("nested");
    let config = AgentConfig::for_model(neo_agent_core::harness::fake_model());
    let mut context = AgentContext::new();
    let (epoch, fingerprint) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![nested.clone()]).await;
    InstructionContextBridge::apply_epoch(&mut context, &epoch, &fingerprint);
    InstructionContextBridge::rehydrate_after_compaction(&fixture.registry, &mut context)
        .await
        .expect("rehydration");
    assert_eq!(
        context.instruction_state().most_recent_scope.as_deref(),
        Some(nested.as_path())
    );
    assert!(!context.instruction_state().active_scopes.contains(&nested));
    std::fs::remove_file(nested.join("AGENTS.md")).expect("remove nested AGENTS.md");

    let (removed, fingerprint) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![nested.clone()]).await;

    assert_eq!(removed.outcome, InstructionEpochOutcome::Removed);
    InstructionContextBridge::apply_epoch(&mut context, &removed, &fingerprint);
    assert!(
        !context
            .instruction_state()
            .visited_revisions
            .contains_key(&nested),
        "{removed:?}"
    );
}

// ---------------------------------------------------------------------------
// Path-scoped instruction preflight enforcement: complete-batch deferral,
// typed probes, and event ordering before permission and tool execution.
// ---------------------------------------------------------------------------

struct PreflightFixture {
    _temp: tempfile::TempDir,
    workspace: PathBuf,
    registry: Arc<InstructionRegistry>,
}

/// A trusted workspace with a root `AGENTS.md` plus optional nested scopes,
/// with the registry behind a shared handle for context attachment.
fn preflight_fixture(nested: &[(&str, &str)], root_rules: &str) -> PreflightFixture {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("AGENTS.md"), root_rules).expect("root AGENTS.md");
    for (dir, rules) in nested {
        let nested_dir = workspace.join(dir);
        std::fs::create_dir_all(&nested_dir).expect("nested dir");
        std::fs::write(nested_dir.join("AGENTS.md"), rules).expect("nested AGENTS.md");
    }
    let workspace = workspace.canonicalize().expect("canonical workspace");
    let registry = InstructionRegistry::new(InstructionRegistryConfig {
        primary_workspace: workspace.clone(),
        neo_home: None,
        project_trusted: true,
    })
    .expect("registry");
    PreflightFixture {
        _temp: temp,
        workspace,
        registry: Arc::new(registry),
    }
}

fn preflight_context(fixture: &PreflightFixture) -> AgentContext {
    let mut context = AgentContext::new();
    context.attach_instruction_registry(Arc::clone(&fixture.registry));
    context
}

fn preflight_config(fixture: &PreflightFixture, harness: &FakeHarness) -> AgentConfig {
    let mut config = AgentConfig::for_model(harness.model())
        .with_workspace_root(&fixture.workspace)
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Auto);
    config.instruction_registry = Some(Arc::clone(&fixture.registry));
    config
}

fn preflight_runtime(fixture: &PreflightFixture, harness: &FakeHarness) -> AgentRuntime {
    AgentRuntime::with_tools(
        preflight_config(fixture, harness),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    )
}

/// One assistant turn carrying `calls` as `(id, name, arguments)` triples.
fn tool_call_turn(calls: &[(&str, &str, serde_json::Value)]) -> Vec<AiStreamEvent> {
    let mut events = vec![AiStreamEvent::MessageStart {
        id: "msg_tools".to_owned(),
    }];
    for (id, name, arguments) in calls {
        events.push(AiStreamEvent::ToolCallStart {
            id: (*id).to_owned(),
            name: (*name).to_owned(),
        });
        events.push(AiStreamEvent::ToolCallEnd {
            id: (*id).to_owned(),
            raw_arguments: arguments.to_string(),
        });
    }
    events.push(AiStreamEvent::MessageEnd {
        stop_reason: neo_ai::StopReason::ToolUse,
        usage: None,
    });
    events
}

fn instruction_epochs(events: &[AgentEvent]) -> Vec<&InstructionEpochData> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::InstructionEpoch { epoch } => Some(epoch),
            _ => None,
        })
        .collect()
}

fn finished_tool_results<'a>(events: &'a [AgentEvent], id: &str) -> Vec<&'a ToolResult> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionFinished {
                id: event_id,
                result,
                ..
            } if event_id == id => Some(result),
            _ => None,
        })
        .collect()
}

fn event_index(events: &[AgentEvent], predicate: impl Fn(&AgentEvent) -> bool) -> Option<usize> {
    events.iter().position(predicate)
}

fn has_tool_started(events: &[AgentEvent]) -> bool {
    event_index(events, |event| {
        matches!(event, AgentEvent::ToolExecutionStarted { .. })
    })
    .is_some()
}

#[tokio::test]
async fn first_nested_edit_defers_before_side_effect_and_retried_batch_executes_once() {
    let fixture = preflight_fixture(&[("nested", "nested rules\n")], "root rules\n");
    let target = fixture.workspace.join("nested").join("target.txt");
    std::fs::write(&target, "alpha").expect("target file");
    let edit_arguments = json!({
        "path": target.to_string_lossy(),
        "old": "alpha",
        "new": "beta",
    });
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("call_1", "Edit", edit_arguments.clone())]),
        tool_call_turn(&[("call_2", "Edit", edit_arguments)]),
        end_turn_events("edited"),
    ]);
    let runtime = preflight_runtime(&fixture, &harness);
    let mut context = AgentContext::new();

    let events = run_turn_collect(&runtime, &mut context, "edit the file").await;

    let epochs = instruction_epochs(&events);
    assert_eq!(
        epochs.len(),
        2,
        "baseline plus nested activation epochs: {events:?}"
    );
    assert_eq!(epochs[0].outcome, InstructionEpochOutcome::Ready);
    assert_eq!(epochs[1].outcome, InstructionEpochOutcome::Activated);
    assert_eq!(epochs[1].deferred_tool_ids, vec!["call_1".to_owned()]);

    // The deferred first call produced a provider-valid non-error result and
    // never touched the file; the retried call edited it exactly once.
    let deferred = finished_tool_results(&events, "call_1");
    assert_eq!(deferred.len(), 1, "call_1");
    assert!(!deferred[0].is_error, "deferred result must be non-error");
    let details = deferred[0].details.as_ref().expect("deferred details");
    assert_eq!(details["status"], "deferred");
    assert_eq!(details["reason"], "instruction_epoch");
    assert_eq!(details["side_effect_occurred"], false);
    assert_eq!(details["generation"], json!(epochs[1].generation));

    let retried = finished_tool_results(&events, "call_2");
    assert_eq!(retried.len(), 1, "call_2");
    assert!(
        !retried[0].is_error,
        "the retried edit must succeed exactly once: {}",
        retried[0].content
    );
    assert_eq!(
        std::fs::read_to_string(&target).expect("read target"),
        "beta"
    );

    // The deferred result reaches the provider before the next request.
    let requests = harness.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1].messages.iter().any(|message| matches!(
            message,
            neo_ai::ChatMessage::ToolResult { tool_call_id, is_error, .. }
                if tool_call_id == "call_1" && !is_error
        )),
        "the deferred call must receive a non-error tool result before the next request"
    );

    // Event order proof: the activation epoch precedes every tool start; no
    // permission prompt ever appears before instruction preflight.
    let epoch_index = event_index(&events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { epoch } if epoch.generation == epochs[1].generation)
    })
    .expect("activation epoch index");
    let started_index = event_index(&events, |event| {
        matches!(event, AgentEvent::ToolExecutionStarted { .. })
    });
    assert!(
        started_index.is_some_and(|index| index > epoch_index),
        "no tool may start before the instruction epoch: {events:?}"
    );
    assert!(
        event_index(&events, |event| matches!(
            event,
            AgentEvent::ApprovalRequested { .. }
        ))
        .is_none(),
        "no approval prompt may precede instruction preflight: {events:?}"
    );
}

#[tokio::test]
async fn one_new_scope_defers_every_call_in_a_parallel_mixed_batch() {
    let fixture = preflight_fixture(&[("nested", "nested rules\n")], "root rules\n");
    std::fs::write(fixture.workspace.join("readme.txt"), "hello").expect("readme");
    let new_file = fixture.workspace.join("nested").join("new.txt");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[
            ("call_read", "Read", json!({ "path": "readme.txt" })),
            (
                "call_write",
                "Write",
                json!({ "path": "nested/new.txt", "content": "created" }),
            ),
            ("call_grep", "Grep", json!({ "pattern": "hello" })),
        ]),
        end_turn_events("done"),
    ]);
    let observed_approvals = Arc::new(Mutex::new(Vec::new()));
    let config = preflight_config(&fixture, &harness)
        .with_permission_mode(PermissionMode::Ask)
        .with_approval_handler({
            let observed_approvals = Arc::clone(&observed_approvals);
            move |request| {
                observed_approvals
                    .lock()
                    .expect("approvals lock")
                    .push(request.clone());
                permit_once(request)
            }
        });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = preflight_context(&fixture);

    let events = run_turn_collect(&runtime, &mut context, "mixed batch").await;

    // Zero tools executed: the write left no file and no tool ever started.
    assert!(
        !new_file.exists(),
        "a deferred write must not create the file"
    );
    assert!(
        !has_tool_started(&events),
        "no tool may start in a deferred batch: {events:?}"
    );
    assert!(
        observed_approvals
            .lock()
            .expect("approvals lock")
            .is_empty(),
        "instruction preflight precedes every permission prompt"
    );
    assert!(
        event_index(&events, |event| matches!(
            event,
            AgentEvent::ApprovalRequested { .. }
        ))
        .is_none(),
        "no approval prompt may appear in a deferred batch: {events:?}"
    );

    // Every call in the batch received a matching non-error deferred result.
    for id in ["call_read", "call_write", "call_grep"] {
        let results = finished_tool_results(&events, id);
        assert_eq!(results.len(), 1, "{id}");
        assert!(
            !results[0].is_error,
            "{id} deferred result must be non-error"
        );
        let details = results[0].details.as_ref().expect("deferred details");
        assert_eq!(details["status"], "deferred", "{id}");
        assert_eq!(details["side_effect_occurred"], false, "{id}");
    }
    let epochs = instruction_epochs(&events);
    assert_eq!(
        epochs.len(),
        2,
        "baseline plus one activation epoch: {events:?}"
    );
    assert_eq!(epochs[1].outcome, InstructionEpochOutcome::Activated);
}

#[tokio::test]
async fn first_read_write_and_nested_cwd_shell_each_defer_before_execution() {
    let cases: Vec<(&str, serde_json::Value, Option<&str>)> = vec![
        ("Read", json!({ "path": "nested/data.txt" }), None),
        (
            "Write",
            json!({ "path": "nested/created.txt", "content": "created" }),
            Some("nested/created.txt"),
        ),
        (
            "Bash",
            json!({ "command": "printf changed > marker.txt", "cwd": "nested" }),
            Some("nested/marker.txt"),
        ),
        (
            "Terminal",
            json!({ "mode": "start", "command": "printf hi", "cwd": "nested" }),
            None,
        ),
    ];
    for (tool, arguments, marker) in cases {
        let fixture = preflight_fixture(&[("nested", "nested rules\n")], "root rules\n");
        std::fs::write(fixture.workspace.join("nested").join("data.txt"), "body")
            .expect("data file");
        let harness = FakeHarness::from_turns([
            tool_call_turn(&[("call_1", tool, arguments)]),
            end_turn_events("done"),
        ]);
        let runtime = preflight_runtime(&fixture, &harness);
        let mut context = preflight_context(&fixture);

        let events = run_turn_collect(&runtime, &mut context, "first contact").await;

        let results = finished_tool_results(&events, "call_1");
        assert_eq!(results.len(), 1, "{tool}");
        assert!(
            !results[0].is_error,
            "{tool} deferred result must be non-error"
        );
        assert_eq!(
            results[0].details.as_ref().expect("deferred details")["status"],
            "deferred",
            "{tool}"
        );
        assert!(
            !has_tool_started(&events),
            "{tool} must not start in a deferred batch: {events:?}"
        );
        assert!(
            event_index(&events, |event| matches!(
                event,
                AgentEvent::ShellCommandStarted { .. }
            ))
            .is_none(),
            "{tool} shell side effect must not start: {events:?}"
        );
        if let Some(marker) = marker {
            assert!(
                !fixture.workspace.join(marker).exists(),
                "{tool} deferred side effect must not exist"
            );
        }
        let epochs = instruction_epochs(&events);
        assert_eq!(epochs.len(), 2, "{tool}: baseline plus activation epoch");
        assert_eq!(
            epochs[1].outcome,
            InstructionEpochOutcome::Activated,
            "{tool}"
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn approval_wait_rechecks_instruction_fingerprint_before_execution() {
    let fixture = preflight_fixture(&[("nested", "nested rules v1\n")], "root rules\n");
    std::fs::write(fixture.workspace.join("nested").join("data.txt"), "body").expect("data");

    // Turn 1: activate the nested scope through a deferred-then-retried Read.
    let first_harness = FakeHarness::from_turns([
        tool_call_turn(&[("read_1", "Read", json!({ "path": "nested/data.txt" }))]),
        tool_call_turn(&[("read_2", "Read", json!({ "path": "nested/data.txt" }))]),
        end_turn_events("read done"),
    ]);
    let first_runtime = preflight_runtime(&fixture, &first_harness);
    let mut context = preflight_context(&fixture);
    let first_events = run_turn_collect(&first_runtime, &mut context, "read the file").await;
    assert_eq!(
        instruction_epochs(&first_events).len(),
        2,
        "baseline plus activation in turn 1: {first_events:?}"
    );

    // Turn 2: the Write needs approval in ask mode; the instruction source
    // changes while the dialog waits, so approval must defer, not execute.
    let new_file = fixture.workspace.join("nested").join("approved.txt");
    let second_harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "write_1",
            "Write",
            json!({ "path": "nested/approved.txt", "content": "approved" }),
        )]),
        end_turn_events("done"),
    ]);
    let (decision_sender, decision_receiver) = oneshot::channel::<ApprovalResponse>();
    let decision_receiver = Arc::new(Mutex::new(Some(decision_receiver)));
    let config = preflight_config(&fixture, &second_harness)
        .with_permission_mode(PermissionMode::Ask)
        .with_async_approval_handler(move |request| {
            let receiver = decision_receiver
                .lock()
                .expect("decision receiver lock")
                .take()
                .expect("single approval decision receiver");
            async move { receiver.await.expect("approval decision") }
        });
    let second_runtime = AgentRuntime::with_tools(
        config,
        second_harness.client(),
        ToolRegistry::with_builtin_tools(),
    );

    let mut stream = second_runtime.run_turn(&mut context, AgentMessage::user_text("write a file"));
    let mut events = Vec::new();
    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("approval dialog should be pending")
            .expect("stream open")
            .expect("event ok");
        let is_approval = matches!(event, AgentEvent::ApprovalRequested { .. });
        events.push(event);
        if is_approval {
            break;
        }
    }
    // The source changes after preflight but before approval completes.
    std::fs::write(
        fixture.workspace.join("nested").join("AGENTS.md"),
        "nested rules v2\n",
    )
    .expect("mutate AGENTS.md");
    decision_sender
        .send(ApprovalResponse::Selected {
            request_id: "write_1".to_owned(),
            action: ApprovalAction::PermitOnce,
            feedback: None,
        })
        .expect("send decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event ok"));
    }

    assert!(
        !new_file.exists(),
        "an approval taken against stale instructions must defer, not execute"
    );
    assert!(
        !has_tool_started(&events),
        "the deferred write must not start: {events:?}"
    );
    let write_results = finished_tool_results(&events, "write_1");
    assert_eq!(write_results.len(), 1, "write_1");
    assert!(
        !write_results[0].is_error,
        "the recheck defer is a non-error result"
    );
    assert_eq!(
        write_results[0].details.as_ref().expect("details")["status"],
        "deferred"
    );
    let epochs = instruction_epochs(&events);
    assert_eq!(
        epochs.len(),
        1,
        "exactly one Updated epoch after the approval: {events:?}"
    );
    assert_eq!(epochs[0].outcome, InstructionEpochOutcome::Updated);
    assert_eq!(epochs[0].deferred_tool_ids, vec!["write_1".to_owned()]);
    // The epoch lands after the approval resolution, before the next request.
    let approval_index = event_index(&events, |event| {
        matches!(event, AgentEvent::ApprovalRequested { .. })
    })
    .expect("approval event");
    let epoch_index = event_index(&events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { .. })
    })
    .expect("epoch event");
    assert!(
        approval_index < epoch_index,
        "preflight approved first, the changed source rechecked to defer: {events:?}"
    );
}

#[tokio::test]
async fn blocked_scope_allows_read_only_diagnosis_but_blocks_mixed_mutation_batch() {
    let fixture = preflight_fixture(
        &[("nested", "@missing-rules.md\nnested body\n")],
        "root rules\n",
    );
    let target = fixture.workspace.join("nested").join("data.txt");
    std::fs::write(&target, "body").expect("data file");
    let edit_arguments = json!({
        "path": target.to_string_lossy(),
        "old": "body",
        "new": "changed",
    });
    let read_arguments = json!({ "path": target.to_string_lossy() });
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("edit_1", "Edit", edit_arguments.clone())]),
        tool_call_turn(&[("read_1", "Read", read_arguments.clone())]),
        tool_call_turn(&[
            ("read_2", "Read", read_arguments),
            ("edit_2", "Edit", edit_arguments),
        ]),
        end_turn_events("done"),
    ]);
    let runtime = preflight_runtime(&fixture, &harness);
    let mut context = preflight_context(&fixture);

    let events = run_turn_collect(&runtime, &mut context, "work in nested").await;

    // Exactly two epochs: the Ready baseline and one Blocked epoch. The same
    // failure is injected once even though three batches touched the scope.
    let epochs = instruction_epochs(&events);
    assert_eq!(
        epochs.len(),
        2,
        "baseline plus one blocked epoch: {events:?}"
    );
    assert_eq!(epochs[1].outcome, InstructionEpochOutcome::Blocked);
    let failure = epochs[1].failure.as_ref().expect("blocked failure");
    assert_eq!(failure.kind, InstructionFailureKind::MissingImport);
    assert_eq!(epochs[1].deferred_tool_ids, vec!["edit_1".to_owned()]);

    // The first Edit blocked with a structured result and no side effect.
    let blocked_first = finished_tool_results(&events, "edit_1");
    assert_eq!(blocked_first.len(), 1, "edit_1");
    assert!(blocked_first[0].is_error, "edit_1 must be blocked");
    let details = blocked_first[0].details.as_ref().expect("blocked details");
    assert_eq!(details["status"], "blocked");
    assert_eq!(details["reason"], "instruction_scope_blocked");
    assert_eq!(details["side_effect_occurred"], false);
    assert_eq!(details["failure"]["kind"], "missing import");

    // Read-only diagnosis proceeds once the failure epoch is visible.
    let read_results = finished_tool_results(&events, "read_1");
    assert_eq!(read_results.len(), 1, "read_1");
    assert!(
        !read_results[0].is_error,
        "read-only diagnosis must proceed: {}",
        read_results[0].content
    );
    assert!(read_results[0].content.contains("body"));

    // A mixed batch afterwards blocks as a whole without a third epoch.
    for id in ["read_2", "edit_2"] {
        let results = finished_tool_results(&events, id);
        assert_eq!(results.len(), 1, "{id}");
        assert!(results[0].is_error, "{id} must block in a mixed batch");
        assert_eq!(
            results[0].details.as_ref().expect("details")["status"],
            "blocked",
            "{id}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&target).expect("read target"),
        "body",
        "no blocked call may produce a side effect"
    );

    // Event order proof: no tool started before the blocked epoch; exactly
    // one tool (the diagnostic read) started after it.
    let epoch_index = event_index(&events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { epoch } if epoch.outcome == InstructionEpochOutcome::Blocked)
    })
    .expect("blocked epoch index");
    let started: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            matches!(event, AgentEvent::ToolExecutionStarted { .. }).then_some(index)
        })
        .collect();
    assert_eq!(
        started.len(),
        1,
        "only the diagnostic read may start: {events:?}"
    );
    assert!(started[0] > epoch_index);
}

#[tokio::test]
async fn baseline_epoch_precedes_first_user_message_for_new_and_legacy_sessions() {
    // New session: empty context establishes a Ready baseline first.
    let fixture = preflight_fixture(&[], "root rules\n");
    let harness = FakeHarness::from_turns([end_turn_events("hello")]);
    let runtime = preflight_runtime(&fixture, &harness);
    let mut context = preflight_context(&fixture);

    let events = run_turn_collect(&runtime, &mut context, "first prompt").await;

    let epochs = instruction_epochs(&events);
    assert_eq!(epochs.len(), 1, "one baseline epoch: {events:?}");
    assert_eq!(epochs[0].outcome, InstructionEpochOutcome::Ready);
    assert!(epochs[0].deferred_tool_ids.is_empty());
    let epoch_index = event_index(&events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { .. })
    })
    .expect("baseline epoch index");
    let user_index = event_index(&events, |event| {
        matches!(
            event,
            AgentEvent::MessageAppended {
                message: AgentMessage::User { .. }
            }
        )
    })
    .expect("user message index");
    assert!(
        epoch_index < user_index,
        "the baseline epoch must precede the first user message: {events:?}"
    );
    let instruction_position = context
        .messages()
        .iter()
        .position(|message| matches!(message, AgentMessage::Instruction { .. }))
        .expect("pinned instruction message");
    let user_position = context
        .messages()
        .iter()
        .position(|message| matches!(message, AgentMessage::User { .. }))
        .expect("user message");
    assert!(
        instruction_position < user_position,
        "the pinned baseline must precede the user message in context"
    );

    // Legacy resume: replayed context without any epoch gets a fresh baseline
    // before the new user message, not a reconstructed legacy behavior.
    let legacy_fixture = preflight_fixture(&[], "root rules\n");
    let legacy_harness = FakeHarness::from_turns([end_turn_events("hi again")]);
    let legacy_runtime = preflight_runtime(&legacy_fixture, &legacy_harness);
    let mut legacy = AgentContext::from_replay(
        [
            AgentEvent::MessageAppended {
                message: AgentMessage::user_text("legacy prompt"),
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::assistant(
                    [Content::text("legacy answer")],
                    Vec::new(),
                    StopReason::EndTurn,
                ),
            },
            AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            },
        ]
        .iter(),
    );
    assert_eq!(legacy.instruction_state().visible_generation, 0);
    let legacy_events = run_turn_collect(&legacy_runtime, &mut legacy, "next prompt").await;

    let legacy_epochs = instruction_epochs(&legacy_events);
    assert_eq!(
        legacy_epochs.len(),
        1,
        "exactly one fresh baseline for the legacy session: {legacy_events:?}"
    );
    assert_eq!(legacy_epochs[0].outcome, InstructionEpochOutcome::Ready);
    let legacy_epoch_index = event_index(&legacy_events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { .. })
    })
    .expect("baseline epoch index");
    let new_user_index = legacy_events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            AgentEvent::MessageAppended {
                message: AgentMessage::User { content, .. },
            } if content
                .iter()
                .filter_map(Content::as_text)
                .collect::<String>()
                == "next prompt" =>
            {
                Some(index)
            }
            _ => None,
        })
        .expect("new user message index");
    assert!(
        legacy_epoch_index < new_user_index,
        "the baseline must precede the new user message on legacy resume: {legacy_events:?}"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn context_pressure_compacts_before_pending_epoch_admission() {
    const ROOT_SENTINEL: &str = "ROOT-SENTINEL-c4d9e1-rules";
    const NESTED_SENTINEL: &str = "NESTED-SENTINEL-77aa10-rules";
    const ORDINARY_SENTINEL: &str = "ORDINARY-SENTINEL-0f3b55-history";

    // Token economics (builtin tool schemas ≈ 12_500 tokens, workspace
    // overhead ≈ 250): in a 32_000-token window the trigger is 25_600. The
    // ~10_000 tokens of ordinary history keep the first request below the
    // trigger (~23_000), but admitting the ~4_000-token nested epoch on top
    // (~27_300) crosses it — so compact-first admission must run. After
    // compaction the request shrinks below the trigger (~17_200 with the
    // epoch), so the epoch is admitted without a second compaction.
    let nested_rules = format!(
        "# nested rules\n{NESTED_SENTINEL}\n{}\n",
        "n".repeat(16_000)
    );
    let fixture = preflight_fixture(
        &[("nested", &nested_rules)],
        &format!("# root rules\n{ROOT_SENTINEL}\n"),
    );
    let target = fixture.workspace.join("nested").join("target.txt");
    std::fs::write(&target, "alpha").expect("target file");
    let edit_arguments = json!({
        "path": target.to_string_lossy(),
        "old": "alpha",
        "new": "beta",
    });
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("call_1", "Edit", edit_arguments.clone())]),
        end_turn_events("summary of earlier work"),
        tool_call_turn(&[("call_2", "Edit", edit_arguments)]),
        end_turn_events("edited"),
    ]);
    let mut config = preflight_config(&fixture, &harness).with_compaction(CompactionSettings {
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 3)
    });
    config.model.capabilities.max_context_tokens = Some(32_000);
    let mut context = preflight_context(&fixture);
    // ~10_000 tokens of ordinary history carrying its own sentinel.
    context.append_message(AgentMessage::user_text(format!(
        "please remember {ORDINARY_SENTINEL} {}",
        "x".repeat(40_000)
    )));
    context.append_message(AgentMessage::assistant(
        [Content::text("noted")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());

    let events = run_turn_collect(&runtime, &mut context, "edit the nested file").await;

    // Two epochs: the Ready baseline, then the nested Activated epoch admitted
    // after compaction.
    let epochs = instruction_epochs(&events);
    assert_eq!(
        epochs.len(),
        2,
        "baseline plus nested activation: {events:?}"
    );
    assert_eq!(epochs[0].outcome, InstructionEpochOutcome::Ready);
    assert_eq!(epochs[1].outcome, InstructionEpochOutcome::Activated);
    assert_eq!(epochs[1].deferred_tool_ids, vec!["call_1".to_owned()]);
    let baseline_model_content = epochs[0]
        .model_content
        .as_deref()
        .expect("baseline epoch carries model content");
    let nested_model_content = epochs[1]
        .model_content
        .as_deref()
        .expect("activated epoch carries model content");
    assert!(nested_model_content.contains(NESTED_SENTINEL));

    // Order proof: compaction ran BEFORE the pending epoch was admitted, and
    // nothing re-compacted afterwards (no inject-then-summarize).
    let compacted_index = event_index(&events, |event| {
        matches!(event, AgentEvent::CompactionApplied { .. })
    })
    .expect("one compaction: {events:?}");
    let epoch_index = event_index(&events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { epoch } if epoch.outcome == InstructionEpochOutcome::Activated)
    })
    .expect("activated epoch index");
    assert!(
        compacted_index < epoch_index,
        "compaction must precede the pending epoch admission: {events:?}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::CompactionStarted { .. }))
            .count(),
        1,
        "no summarize-after-inject: exactly one compaction: {events:?}"
    );

    // The deferred call never executed; the retried call edited exactly once.
    let deferred = finished_tool_results(&events, "call_1");
    assert_eq!(deferred.len(), 1, "call_1");
    assert!(!deferred[0].is_error, "deferred result must be non-error");
    assert_eq!(
        deferred[0].details.as_ref().expect("deferred details")["status"],
        "deferred"
    );
    assert_eq!(
        std::fs::read_to_string(&target).expect("read target"),
        "beta"
    );

    // The compaction summary input contains ordinary history but neither the
    // baseline body nor the pending epoch body (no summarize-after-inject).
    let requests = harness.requests();
    assert_eq!(
        requests.len(),
        4,
        "turn, summary, post-admission, final: {requests:?}"
    );
    let summary_input = chat_request_text(&requests[1]);
    assert!(
        summary_input.contains(ORDINARY_SENTINEL),
        "ordinary history is summarized: {summary_input}"
    );
    assert!(
        !summary_input.contains(NESTED_SENTINEL),
        "pending epoch bytes must never enter the summary input: {summary_input}"
    );
    assert!(
        !summary_input.contains(ROOT_SENTINEL),
        "pinned baseline bodies stay out of the summary input: {summary_input}"
    );

    // Post-compaction rehydration restored the exact baseline bytes BEFORE the
    // pending bundle was admitted: the rehydrated baseline (generation 1) sits
    // ahead of the admitted epoch (generation 2), both byte-identical.
    let pinned: Vec<(u64, String)> = context
        .messages()
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Instruction {
                generation,
                content,
            } => Some((
                *generation,
                content.iter().filter_map(Content::as_text).collect(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        pinned,
        vec![
            (1, baseline_model_content.to_owned()),
            (epochs[1].generation, nested_model_content.to_owned()),
        ],
        "rehydrate-then-admit must preserve instruction bytes exactly"
    );

    // The post-admission provider request carries the nested AGENTS.md body
    // byte-for-byte exactly once.
    let post_admission = chat_request_text(&requests[2]);
    assert_eq!(
        post_admission.matches(NESTED_SENTINEL).count(),
        1,
        "nested instruction bytes appear exactly once: {post_admission}"
    );
    assert!(
        post_admission.contains(&nested_rules),
        "the nested AGENTS.md body is preserved byte-for-byte: {post_admission}"
    );
}

#[tokio::test]
async fn history_pressure_compacts_before_whole_bundle_omission() {
    let nested_rules = format!("# exact nested authority\n{}\n", "n".repeat(96_000));
    let fixture = preflight_fixture(&[("nested", &nested_rules)], "# root authority\n");
    let target = fixture.workspace.join("nested/target.txt");
    std::fs::write(&target, "alpha").expect("target file");
    let edit_arguments = json!({
        "path": target.to_string_lossy(),
        "old": "alpha",
        "new": "beta",
    });
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("call_1", "Edit", edit_arguments.clone())]),
        end_turn_events("summary output"),
        tool_call_turn(&[("call_2", "Edit", edit_arguments)]),
        end_turn_events("edited"),
    ]);
    let mut config = preflight_config(&fixture, &harness).with_compaction(CompactionSettings {
        trigger_ratio: 0.99,
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 3)
    });
    config.model.capabilities.max_context_tokens = Some(32_000);
    let mut context = preflight_context(&fixture);
    apply_preflight_baseline(&fixture, &config, &mut context).await;
    context.append_message(AgentMessage::user_text(format!(
        "ordinary history {}",
        "x".repeat(40_000)
    )));
    context.append_message(AgentMessage::assistant(
        [Content::text("noted")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());

    let events = run_turn_collect(&runtime, &mut context, "edit the nested file").await;

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. })),
        "ordinary history must compact before an applicable bundle is omitted: {events:?}"
    );
    let epoch = instruction_epochs(&events)
        .into_iter()
        .find(|epoch| epoch.outcome == InstructionEpochOutcome::Activated)
        .expect("nested authority activates after fresh admission");
    assert!(epoch.ignored_bundles.is_empty());
    let authority = epoch.model_content.as_deref().expect("nested authority");
    let requests = harness.requests();
    assert!(
        request_contains_exact_text(&requests[2], authority),
        "post-compaction admission must use the fresh byte-exact authority"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionStarted { id, .. } if id == "call_2"
        )),
        "retried edit never started; requests={}, epochs={:?}",
        requests.len(),
        instruction_epochs(&events)
            .iter()
            .map(|epoch| epoch.outcome)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        std::fs::read_to_string(target).expect("target contents"),
        "beta",
        "same-turn replan must execute the retried edit: {events:?}"
    );
}

#[tokio::test]
async fn post_tool_instruction_update_compacts_before_fresh_admission() {
    const UPDATED_SENTINEL: &str = "POST-TOOL-UPDATED-AUTHORITY";
    let fixture = preflight_fixture(&[], "old root rules\n");
    let updated_rules = format!("{UPDATED_SENTINEL}\n{}\n", "r".repeat(120_000));
    let agents_path = fixture.workspace.join("AGENTS.md");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "write_agents",
            "Write",
            json!({
                "path": agents_path.to_string_lossy(),
                "content": updated_rules,
            }),
        )]),
        end_turn_events("summary output"),
        end_turn_events("continued with updated rules"),
    ]);
    let mut config = preflight_config(&fixture, &harness).with_compaction(CompactionSettings {
        trigger_ratio: 0.99,
        reserved_context_tokens: 1_000,
        ..CompactionSettings::new(usize::MAX, 3)
    });
    config.model.capabilities.max_context_tokens = Some(100_000);
    let mut context = preflight_context(&fixture);
    apply_preflight_baseline(&fixture, &config, &mut context).await;
    context.append_message(AgentMessage::user_text(format!(
        "ordinary history {}",
        "h".repeat(160_000)
    )));
    context.append_message(AgentMessage::assistant(
        [Content::text("history acknowledged")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());

    let events = run_turn_collect(&runtime, &mut context, "replace the root instructions").await;

    let compaction_index = event_index(&events, |event| {
        matches!(event, AgentEvent::CompactionApplied { .. })
    })
    .expect("post-tool update must compact ordinary history");
    let updated_index = event_index(&events, |event| {
        matches!(
            event,
            AgentEvent::InstructionEpoch { epoch }
                if epoch.outcome == InstructionEpochOutcome::Updated
                    && epoch.ignored_bundles.is_empty()
                    && epoch.model_content.as_deref().is_some_and(|content| content.contains(UPDATED_SENTINEL))
        )
    })
    .expect("fresh post-compaction Updated epoch");
    assert!(compaction_index < updated_index, "events: {events:#?}");
    assert_eq!(
        harness.requests().len(),
        3,
        "tool request, compaction summary, continued request"
    );
}
