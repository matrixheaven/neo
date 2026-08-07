//! Run-mode workflow context behavior (split from `context.rs`).

use super::*;
use std::sync::Arc;

use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, Content, PermissionMode,
    SteerInputHandle, ToolRegistry,
    harness::FakeHarness,
    session::{JsonlSessionReader, JsonlSessionWriter},
};
use neo_ai::{
    AiStreamEvent, ApiKind, ApiType, ChatMessage, ModelCapabilities, ModelSpec, ProviderId,
    StopReason, providers::fake::FakeModelClient,
};
use tokio_util::sync::CancellationToken;

use super::super::{TurnChannels, TurnRequest, run_prompt_streaming, run_prompt_with_runtime};
use crate::config::{ModelConfig, ProviderConfig};

#[tokio::test]
async fn oversized_workflow_catalog_starts_no_provider_call() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    config.default_provider = "test-provider".to_owned();
    config.default_model = "test-model".to_owned();
    config.providers.insert(
        "test-provider".to_owned(),
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::OpenAiResponse),
            base_url: Some("https://example.test/v1".to_owned()),
            api_key: Some("test-key".to_owned()),
            api_key_env: None,
        },
    );
    config.models.insert(
        "test-model".to_owned(),
        ModelConfig {
            provider: "test-provider".to_owned(),
            model: "test-model".to_owned(),
            max_context_tokens: Some(128),
            max_output_tokens: Some(32),
            capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
            ..ModelConfig::default()
        },
    );

    let (events, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let (approvals, _approval_rx) = tokio::sync::mpsc::unbounded_channel();
    let (session_ids, _session_id_rx) = tokio::sync::mpsc::unbounded_channel();
    let (questions, _question_rx) = tokio::sync::mpsc::unbounded_channel();
    let request = TurnRequest::new(
        vec![Content::text("/workflow use the catalog")],
        None,
        None,
        neo_ai::ReasoningSelection::Off,
    )
    .with_workflow_context("catalog ".repeat(100_000));
    let channels = TurnChannels {
        events,
        approvals,
        session_ids,
        cancel_token: CancellationToken::new(),
        questions,
        steer_input: SteerInputHandle::new(),
    };

    let error = match run_prompt_streaming(request, channels, &config).await {
        Ok(_) => panic!("oversized workflow context must fail before session creation"),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        crate::modes::interactive::workflow_slash::WORKFLOW_CONTEXT_TOO_LARGE
    );
    assert!(
        !crate::config::workspace_sessions_dir(&config).exists(),
        "capacity rejection must not leave an empty session"
    );
}

#[tokio::test]
async fn oversized_workflow_context_does_not_persist_user_message() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_path = temp.path().join("session.jsonl");
    let fake = FakeModelClient::default();
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(ModelSpec {
            provider: ProviderId("test-provider".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::Local,
            capabilities: ModelCapabilities::tool_chat()
                .with_max_context_tokens(100_000)
                .with_max_output_tokens(4_096),
        })
        .with_turn_injection("workflow catalog ".repeat(400_000)),
        Arc::new(fake.clone()),
    );
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .expect("session writer");

    let error = match run_prompt_with_runtime(
        "/workflow:oversized run this workflow".to_owned(),
        AgentContext::new(),
        &mut writer,
        runtime,
    )
    .await
    {
        Ok(_) => panic!("oversized workflow context must fail"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        crate::modes::interactive::workflow_slash::WORKFLOW_CONTEXT_TOO_LARGE
    );
    writer.flush().await.expect("flush rejected turn");
    drop(writer);

    let messages = JsonlSessionReader::replay_messages(&session_path)
        .await
        .expect("replay messages");
    assert!(messages.iter().all(|message| {
        !matches!(
            message,
            AgentMessage::User { content, .. }
                if content == &vec![Content::text("/workflow:oversized run this workflow")]
        )
    }));
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn workflow_turn_injection_is_tail_only_and_persisted() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_path = temp.path().join("session.jsonl");
    let fake = FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-workflow".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(fake_model()).with_turn_injection("complete workflow guidance"),
        Arc::new(fake.clone()),
    );
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .expect("session writer");

    run_prompt_with_runtime(
        "/workflow:demo Research battery recycling".to_owned(),
        AgentContext::new(),
        &mut writer,
        runtime,
    )
    .await
    .expect("workflow turn");

    let request = fake.requests().remove(0);
    assert!(matches!(
        request.messages.last(),
        Some(ChatMessage::User { .. })
    ));
    assert_eq!(
        chat_message_text(request.messages.last().expect("workflow injection")),
        "<workflow_turn_context applies_to=\"next_model_request_only\">\ncomplete workflow guidance\n</workflow_turn_context>"
    );
    let messages = JsonlSessionReader::replay_messages(&session_path)
        .await
        .expect("replay messages");
    assert!(messages.iter().any(|message| {
        matches!(
            message,
            AgentMessage::User { content, .. }
                if content == &vec![Content::text("/workflow:demo Research battery recycling")]
        )
    }));
    assert!(messages.iter().any(|message| {
        message.is_injection_variant("workflow_turn_context")
            && message.text().contains("complete workflow guidance")
    }));
}

#[tokio::test]
async fn workflow_runtime_dispatches_saved_run_from_model_tool_call() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let session_dir = temp.path().join("session");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&session_dir).expect("session directory");
    let source = "return { ok = true }\n";
    let source_sha = neo_agent_core::workflow::source_sha256_hex(source.as_bytes());
    let manifest = format!(
        r#"
name = "demo"
display_name = "Demo"
description = "runtime workflow fixture"
source_sha256 = "{source_sha}"

[[phases]]
id = "run"
description = "run"

[output_schema]
type = "object"
"#
    );
    let definitions = neo_agent_core::workflow::WorkflowDefinitionRegistry::new(
        neo_agent_core::workflow::WorkflowDefinitionRegistryConfig {
            neo_home: temp.path().join("neo_home"),
            workspace: workspace.clone(),
            project_trusted: true,
            limits: neo_agent_core::workflow::WorkflowLimits::default(),
            builtins: vec![neo_agent_core::workflow::BuiltinWorkflowDefinition {
                name: "demo".to_owned(),
                manifest_bytes: manifest.into_bytes(),
                source_bytes: source.as_bytes().to_vec(),
            }],
        },
    );
    let workflow_runtime = neo_agent_core::workflow::WorkflowRuntime::new(
        neo_agent_core::workflow::WorkflowLimits::default(),
    );
    workflow_runtime
        .bind_runner(|_handle, _metadata, _session_dir| async { Ok(()) })
        .expect("bind workflow runner");
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "workflow-call".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "workflow-call-1".to_owned(),
                name: "Workflow".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "workflow-call-1".to_owned(),
                raw_arguments: serde_json::json!({
                    "action": "run_saved",
                    "name": "demo",
                    "args": {}
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "workflow-answer".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(&workspace)
        .expect("workspace root")
        .with_session_directory(&session_dir)
        .with_permission_mode(PermissionMode::Yolo)
        .with_turn_injection("workflow slash guidance")
        .with_workflow_runtime(workflow_runtime.clone())
        .with_workflow_definitions(definitions.clone());
    let tools = ToolRegistry::with_builtin_tools();
    let runtime = AgentRuntime::with_tools(config, harness.client(), tools);
    let session_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
    std::fs::create_dir_all(session_path.parent().expect("session wire parent"))
        .expect("session wire directory");
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .expect("session writer");

    let turn = run_prompt_with_runtime(
        "/workflow:demo run it".to_owned(),
        AgentContext::new(),
        &mut writer,
        runtime,
    )
    .await
    .expect("model workflow turn");

    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        chat_message_text(requests[0].messages.last().expect("workflow injection")),
        "<workflow_turn_context applies_to=\"next_model_request_only\">\nworkflow slash guidance\n</workflow_turn_context>"
    );
    assert!(requests[0].tools.iter().any(|tool| tool.name == "Workflow"));
    assert_eq!(requests[1].tools, requests[0].tools);
    assert_eq!(
        requests[0].messages,
        requests[1].messages[..requests[0].messages.len()]
    );
    let workflow_result = turn.events.iter().find_map(|event| match event {
        AgentEvent::ToolExecutionFinished { name, result, .. } if name == "Workflow" => {
            Some(result)
        }
        _ => None,
    });
    let workflow_result = workflow_result.expect("Workflow tool result");
    assert!(!workflow_result.is_error, "{workflow_result:?}");
    assert_eq!(
        workflow_result.details.as_ref().expect("workflow details")["task"]["kind"],
        "workflow"
    );
    assert_eq!(turn.assistant_text, "done");
}

#[tokio::test]
async fn idle_workflow_event_is_flushed_before_persisted_envelope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path());
    let session_id = "session_00000000-0000-4000-8000-000000000777";
    let session_directory = crate::config::workspace_sessions_dir(&config).join(session_id);
    tokio::fs::create_dir_all(&session_directory)
        .await
        .expect("session directory");
    let wire_path = neo_agent_core::session::main_agent_wire_path(&session_directory);
    tokio::fs::create_dir_all(wire_path.parent().expect("wire parent"))
        .await
        .expect("wire directory");
    let mut seed = JsonlSessionWriter::create(&wire_path)
        .await
        .expect("session writer");
    seed.flush().await.expect("seed flush");
    drop(seed);
    let event = AgentEvent::ToolExecutionStarted {
        turn: 9,
        id: "workflow-idle".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "cargo --version"}),
        workflow_origin: None,
        output_ref: None,
    };
    let (ingress, events) = tokio::sync::mpsc::unbounded_channel();
    let (persisted, mut deliveries) = tokio::sync::mpsc::unbounded_channel();
    let worker = tokio::spawn(super::super::persist_session_workflow_events(
        config, events, persisted,
    ));

    ingress
        .send(super::super::SessionWorkflowEvent {
            session_id: session_id.to_owned(),
            generation: 4,
            event: event.clone(),
        })
        .expect("idle event");
    let delivery = tokio::time::timeout(std::time::Duration::from_secs(5), deliveries.recv())
        .await
        .expect("persisted delivery timeout")
        .expect("persisted delivery");
    assert!(matches!(
        delivery,
        super::super::PersistedSessionWorkflowEvent::Event(ref envelope)
            if envelope.generation == 4
    ));
    let stored = JsonlSessionReader::read_all(&wire_path)
        .await
        .expect("persisted session");
    assert!(
        stored.contains(&event),
        "delivery must follow durable flush"
    );

    let writer = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        JsonlSessionWriter::open_append(&wire_path),
    )
    .await
    .expect("workflow ingress must release the session lock after each event")
    .expect("open session after workflow event");
    drop(writer);

    drop(ingress);
    worker.await.expect("idle persistence worker");
}
