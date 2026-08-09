//! Btw mode behavior (moved from `btw.rs`).

use std::{collections::BTreeMap, sync::Arc};

use neo_agent_core::{
    AgentMessage, PermissionMode, QueueMode, StopReason as AgentStopReason, ToolExecutionMode,
};
use neo_ai::{
    AiStreamEvent, ApiKind, ContentPart, ModelCapabilities, ModelSpec, ProviderId, StopReason,
    providers::fake::FakeModelClient,
};

use super::*;
use crate::config::{AppConfig, Defaults, McpConfig, RuntimeConfig, TuiConfig};
use crate::trust;
use neo_tui::widgets::btw_panel::BtwSidecar;

fn test_config(project_dir: &std::path::Path) -> AppConfig {
    AppConfig {
        default_model: "test-model".to_owned(),
        default_provider: "test-provider".to_owned(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir: project_dir.join(".neo/sessions"),
        permission_mode: PermissionMode::default(),
        live_permission_mode: std::sync::Arc::new(
            std::sync::RwLock::new(PermissionMode::default()),
        ),
        workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
        defaults: Defaults {
            mode: "interactive".to_owned(),
        },
        runtime: RuntimeConfig {
            temperature: None,
            max_tokens: None,
            reasoning: neo_ai::ReasoningSelection::Off,
            replay_reasoning: true,
            steering_queue_mode: QueueMode::All,
            follow_up_queue_mode: QueueMode::All,
            tool_execution_mode: ToolExecutionMode::Sequential,
            compaction: None,
            ..RuntimeConfig::default()
        },
        background_tasks: neo_agent_core::BackgroundTaskManager::new(),
        workflow_runtime: neo_agent_core::workflow::WorkflowRuntime::new(
            neo_agent_core::workflow::WorkflowLimits::default(),
        ),
        workflow_definitions: neo_agent_core::workflow::WorkflowDefinitionRegistry::empty(),
        workflow_dispatch_resolver: neo_agent_core::runtime::WorkflowDispatchResolver::default(),
        multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
        tui: TuiConfig::default(),
        theme: crate::themes::ResolvedTheme::default(),
        theme_resolution: crate::themes::ThemeResolution::Default,
        mcp: McpConfig::default(),
        prompt_templates: Vec::new(),
        system_prompt_file: None,
        extra_skill_dirs: Vec::new(),
        skill_path: Vec::new(),
        project_trusted: true,
        project_trust: trust::ProjectTrustState::NotRequired,
        project_dir: project_dir.to_path_buf(),
        config_path: project_dir.join(".neo/config.toml"),
        config_file_exists: true,
    }
}

fn fake_model() -> ModelSpec {
    ModelSpec {
        provider: ProviderId("test-provider".to_owned()),
        model: "test-model".to_owned(),
        api: ApiKind::Local,
        capabilities: ModelCapabilities::tool_chat(),
    }
}

#[tokio::test]
async fn streams_text_and_finished_events() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path());
    let fake = FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "hello ".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "world".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runner = BtwRunner::new(fake_model(), Arc::new(fake), config, &[]);

    let mut rx = runner.run("hi".to_owned()).await.expect("run");

    let mut texts = Vec::new();
    let mut finished = false;
    while let Some(event) = rx.recv().await {
        match event {
            BtwEvent::Started { .. } => {}
            BtwEvent::TextDelta(text) => texts.push(text),
            BtwEvent::Finished => {
                finished = true;
                break;
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    assert_eq!(texts, vec!["hello ", "world"]);
    assert!(finished);
}

#[tokio::test]
async fn inherited_messages_are_projected_into_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path());
    let fake = FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "ok".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    let inherited = vec![
        AgentMessage::user_text("first"),
        AgentMessage::assistant(
            [neo_agent_core::Content::text("second")],
            Vec::new(),
            AgentStopReason::EndTurn,
        ),
    ];
    let runner = BtwRunner::new(fake_model(), Arc::new(fake.clone()), config, &inherited);

    let mut rx = runner.run("third".to_owned()).await.expect("run");
    while rx.recv().await.is_some() {}

    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let contents: Vec<String> = requests[0].messages.iter().map(chat_message_text).collect();

    // The request must contain the inherited user/assistant exchange, the
    // sidecar reminder, and the new prompt. Extra system prompts from the
    // environment may appear, so we assert on ordering rather than count.
    let first_idx = contents.iter().position(|c| c == "first").expect("first");
    let second_idx = contents.iter().position(|c| c == "second").expect("second");
    let prompt_idx = contents.iter().position(|c| c == "third").expect("third");
    let reminder_idx = contents
        .iter()
        .position(|c| c.contains("side-channel"))
        .expect("sidecar reminder");

    assert!(first_idx < second_idx);
    assert!(second_idx < reminder_idx);
    assert!(reminder_idx < prompt_idx);
}

#[tokio::test]
async fn follow_up_reuses_sidecar_conversation_context() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path());
    let fake = FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "side answer".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runner = BtwRunner::new(fake_model(), Arc::new(fake.clone()), config, &[]);

    let mut first = runner
        .run("first side question".to_owned())
        .await
        .expect("run");
    while first.recv().await.is_some() {}
    let mut second = runner
        .run("second side question".to_owned())
        .await
        .expect("run");
    while second.recv().await.is_some() {}

    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    let contents: Vec<String> = requests[1].messages.iter().map(chat_message_text).collect();
    let first_prompt_idx = contents
        .iter()
        .position(|content| content == "first side question")
        .expect("first prompt");
    let first_answer_idx = contents
        .iter()
        .position(|content| content == "side answer")
        .expect("first answer");
    let second_prompt_idx = contents
        .iter()
        .position(|content| content == "second side question")
        .expect("second prompt");

    assert!(first_prompt_idx < first_answer_idx);
    assert!(first_answer_idx < second_prompt_idx);
}

#[tokio::test(flavor = "current_thread")]
async fn cancel_emits_cancelled_event() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path());
    // Fake model never emits anything, so cancellation is the only way the
    // stream will end.
    let fake = FakeModelClient::new(vec![]);
    let runner = BtwRunner::new(fake_model(), Arc::new(fake), config, &[]);

    let mut rx = runner.run("wait".to_owned()).await.expect("run");
    runner.cancel();

    let mut saw_cancelled = false;
    while let Some(event) = rx.recv().await {
        if matches!(event, BtwEvent::Cancelled) {
            saw_cancelled = true;
            break;
        }
    }

    assert!(saw_cancelled);
}

#[test]
fn forward_event_maps_error_to_failed() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    assert!(forward_event(
        &tx,
        Ok(AgentEvent::Error {
            turn: 1,
            message: "boom".to_owned(),
            code: None,
            retry_after: None,
        })
    ));
    assert_eq!(rx.try_recv(), Ok(BtwEvent::Failed("boom".to_owned())));
}

#[test]
fn forward_event_maps_tool_error_to_denied() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    assert!(!forward_event(
        &tx,
        Ok(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "t1".to_owned(),
            name: "bash".to_owned(),
            result: neo_agent_core::ToolResult::error("nope"),
            workflow_origin: None,
            output_ref: None,
        })
    ));
    assert_eq!(
        rx.try_recv(),
        Ok(BtwEvent::ToolDenied {
            message: "nope".to_owned()
        })
    );
}

fn chat_message_text(message: &neo_ai::ChatMessage) -> String {
    let content = match message {
        neo_ai::ChatMessage::System { content }
        | neo_ai::ChatMessage::User { content }
        | neo_ai::ChatMessage::Assistant { content, .. }
        | neo_ai::ChatMessage::ToolResult { content, .. } => content,
    };
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            ContentPart::Thinking { .. }
            | ContentPart::Image { .. }
            | ContentPart::Video { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[test]
fn update_state_started_creates_running_turn() {
    let mut state = BtwPanelState::new(BtwSidecar::new("btw-1"));
    update_btw_panel_state(
        &mut state,
        BtwEvent::Started {
            sidecar_id: "btw-1".to_owned(),
            prompt: "hello".to_owned(),
        },
    );
    assert_eq!(state.sidecar.turns.len(), 1);
    assert_eq!(state.sidecar.turns[0].prompt, "hello");
    assert_eq!(state.sidecar.turns[0].phase, BtwPhase::Running);
    assert_eq!(state.sidecar.phase, BtwPhase::Running);
}

#[test]
fn update_state_appends_thinking_and_text_to_last_turn() {
    let mut state = BtwPanelState::new(BtwSidecar::new("btw-1"));
    update_btw_panel_state(
        &mut state,
        BtwEvent::Started {
            sidecar_id: "btw-1".to_owned(),
            prompt: "q".to_owned(),
        },
    );
    update_btw_panel_state(&mut state, BtwEvent::ThinkingDelta("thinking ".to_owned()));
    update_btw_panel_state(&mut state, BtwEvent::TextDelta("answer".to_owned()));

    assert_eq!(state.sidecar.turns[0].thinking, "thinking ");
    assert_eq!(state.sidecar.turns[0].answer, "answer");
}

#[test]
fn update_state_finished_marks_turn_done() {
    let mut state = BtwPanelState::new(BtwSidecar::new("btw-1"));
    update_btw_panel_state(
        &mut state,
        BtwEvent::Started {
            sidecar_id: "btw-1".to_owned(),
            prompt: "q".to_owned(),
        },
    );
    update_btw_panel_state(&mut state, BtwEvent::Finished);
    assert_eq!(state.sidecar.turns[0].phase, BtwPhase::Done);
    assert_eq!(state.sidecar.phase, BtwPhase::Done);
}

#[test]
fn update_state_cancelled_pops_empty_turn() {
    let mut state = BtwPanelState::new(BtwSidecar::new("btw-1"));
    update_btw_panel_state(
        &mut state,
        BtwEvent::Started {
            sidecar_id: "btw-1".to_owned(),
            prompt: "q".to_owned(),
        },
    );
    update_btw_panel_state(&mut state, BtwEvent::Cancelled);
    assert!(state.sidecar.turns.is_empty());
    assert_eq!(state.sidecar.phase, BtwPhase::Cancelled);
}

#[test]
fn update_state_cancelled_keeps_nonempty_turn() {
    let mut state = BtwPanelState::new(BtwSidecar::new("btw-1"));
    update_btw_panel_state(
        &mut state,
        BtwEvent::Started {
            sidecar_id: "btw-1".to_owned(),
            prompt: "q".to_owned(),
        },
    );
    update_btw_panel_state(&mut state, BtwEvent::TextDelta("partial".to_owned()));
    update_btw_panel_state(&mut state, BtwEvent::Cancelled);
    assert_eq!(state.sidecar.turns[0].phase, BtwPhase::Cancelled);
    assert_eq!(state.sidecar.phase, BtwPhase::Cancelled);
}

#[test]
fn update_state_failed_sets_error_and_phase() {
    let mut state = BtwPanelState::new(BtwSidecar::new("btw-1"));
    update_btw_panel_state(
        &mut state,
        BtwEvent::Started {
            sidecar_id: "btw-1".to_owned(),
            prompt: "q".to_owned(),
        },
    );
    update_btw_panel_state(&mut state, BtwEvent::Failed("boom".to_owned()));
    assert_eq!(state.sidecar.turns[0].error, Some("boom".to_owned()));
    assert_eq!(state.sidecar.turns[0].phase, BtwPhase::Failed);
    assert_eq!(state.sidecar.phase, BtwPhase::Failed);
}

#[test]
fn update_state_tool_denied_without_turn_sets_status_message() {
    let mut state = BtwPanelState::new(BtwSidecar::new("btw-1"));
    update_btw_panel_state(
        &mut state,
        BtwEvent::ToolDenied {
            message: "denied".to_owned(),
        },
    );
    assert_eq!(state.status_message, Some("denied".to_owned()));
}
