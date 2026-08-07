//! Run-mode context behavior (moved from `mod.rs`).

use super::*;
use std::{collections::BTreeMap, sync::Arc};

use neo_agent_core::instructions::{InstructionRegistry, InstructionRegistryConfig};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, ApprovalAction, ApprovalResponse,
    CompactionSettings, Content, MessageOrigin, PermissionMode, QueueMode, SteerInputHandle,
    ToolExecutionMode,
    session::{JsonlSessionReader, JsonlSessionWriter},
};
use neo_ai::{
    AiStreamEvent, ApiKind, ApiType, ModelCapabilities, ModelSpec, ProviderId, StopReason,
    providers::fake::FakeModelClient,
};
use tokio_util::sync::CancellationToken;

use super::super::runtime::agent_config_for_app;
use super::super::{
    PendingApproval, TurnChannels, TurnRequest, run_prompt_with_runtime, runtime_for_config,
    user_message,
};
use crate::config::{
    AppConfig, Defaults, McpConfig, ModelConfig, ProviderConfig, RuntimeCompactionConfig,
    RuntimeConfig, RuntimeRetryConfig, TuiConfig,
};

#[test]
fn agent_config_for_app_applies_runtime_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = AppConfig {
        default_model: "test-model".to_owned(),
        default_provider: "openai".to_owned(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir: temp.path().join(".neo/sessions"),
        permission_mode: PermissionMode::default(),
        live_permission_mode: std::sync::Arc::new(
            std::sync::RwLock::new(PermissionMode::default()),
        ),
        workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
        defaults: Defaults {
            mode: "events".to_owned(),
        },
        runtime: RuntimeConfig {
            temperature: Some(0.35),
            max_tokens: Some(512),
            reasoning: neo_ai::ReasoningSelection::Effort {
                effort: neo_ai::ReasoningEffort::high(),
            },
            replay_reasoning: true,
            steering_queue_mode: QueueMode::OneAtATime,
            follow_up_queue_mode: QueueMode::OneAtATime,
            tool_execution_mode: ToolExecutionMode::Sequential,
            retry: RuntimeRetryConfig {
                max_retries: 100,
                first_event_timeout_secs: 7,
                stream_idle_timeout_secs: 11,
            },
            compaction: Some(RuntimeCompactionConfig {
                enabled: true,
                max_estimated_tokens: 16_000,
                keep_recent_messages: 24,
                trigger_ratio: 0.85,
                reserved_context_tokens: 50_000,
                max_recent_messages: 4,
                micro_enabled: false,
                micro_keep_recent: 20,
                snip_enabled: false,
                snip_min_tokens: 1_000,
                snip_keep_recent: 16,
                snip_trigger_ratio: 0.6,
                max_rounds: 5,
                max_retry_attempts: 5,
            }),
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
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir: temp.path().to_path_buf(),
        config_path: temp.path().join(".neo/config.toml"),
        config_file_exists: true,
    };
    let model = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "test-model".to_owned(),
        api: ApiKind::OpenAiResponse,
        capabilities: ModelCapabilities::tool_chat(),
    };

    let agent_config = agent_config_for_app(model, &config, None, None).expect("agent config");

    assert_eq!(agent_config.temperature, Some(0.35));
    assert_eq!(agent_config.max_tokens, Some(512));
    assert_eq!(agent_config.max_retries, 100);
    assert_eq!(agent_config.first_event_timeout_secs, 7);
    assert_eq!(agent_config.stream_idle_timeout_secs, 11);
    assert_eq!(
        agent_config.reasoning,
        neo_ai::ReasoningSelection::Effort {
            effort: neo_ai::ReasoningEffort::high(),
        }
    );
    assert_eq!(agent_config.steering_queue_mode, QueueMode::OneAtATime);
    assert_eq!(agent_config.follow_up_queue_mode, QueueMode::OneAtATime);
    assert_eq!(
        agent_config.tool_execution_mode,
        ToolExecutionMode::Sequential
    );
    assert_eq!(
        agent_config.compaction,
        Some(CompactionSettings {
            enabled: true,
            max_estimated_tokens: 16_000,
            keep_recent_messages: 24,
            trigger_ratio: 0.85,
            reserved_context_tokens: 50_000,
            max_recent_messages: 4,
            micro_enabled: false,
            micro_keep_recent: 20,
            snip_enabled: false,
            snip_min_tokens: 1_000,
            snip_keep_recent: 16,
            snip_trigger_ratio: 0.6,
            max_rounds: 5,
            max_retry_attempts: 5,
        })
    );
    assert!(agent_config.workspace_root.is_some());
    assert!(
        agent_config.instruction_registry.is_some(),
        "production agent config must enable path-scoped AGENTS instructions"
    );
}

#[test]
fn agent_config_for_app_shares_session_workflow_registry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let neo_home = temp.path().join("neo_home");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&neo_home).expect("neo home");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let registry = neo_agent_core::workflow::WorkflowDefinitionRegistry::new(
        neo_agent_core::workflow::WorkflowDefinitionRegistryConfig {
            neo_home,
            workspace: workspace.clone(),
            project_trusted: true,
            limits: neo_agent_core::workflow::WorkflowLimits::default(),
            builtins: Vec::new(),
        },
    );
    registry
        .save(
            neo_agent_core::workflow::WorkflowSaveScope::User,
            &neo_agent_core::workflow::WorkflowSaveRequest {
                display_name: "saved-probe".to_owned(),
                name: "saved-probe".to_owned(),
                description: "registry wiring probe".to_owned(),
                phases: vec![neo_agent_core::workflow::WorkflowPhase {
                    id: "work".to_owned(),
                    description: "work".to_owned(),
                }],
                lua_source: "return { ok = true }".to_owned(),
                input_schema: Some(serde_json::json!({"type": "object"})),
                output_schema: serde_json::json!({"type": "object"}),
            },
            false,
        )
        .expect("save definition");
    let config = AppConfig {
        default_model: "test-model".to_owned(),
        default_provider: "openai".to_owned(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir: temp.path().join("sessions"),
        permission_mode: PermissionMode::default(),
        live_permission_mode: std::sync::Arc::new(
            std::sync::RwLock::new(PermissionMode::default()),
        ),
        workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
        defaults: Defaults {
            mode: "events".to_owned(),
        },
        runtime: RuntimeConfig::default(),
        background_tasks: neo_agent_core::BackgroundTaskManager::new(),
        workflow_runtime: neo_agent_core::workflow::WorkflowRuntime::new(
            neo_agent_core::workflow::WorkflowLimits::default(),
        ),
        workflow_definitions: registry,
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
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir: workspace,
        config_path: temp.path().join("config.toml"),
        config_file_exists: true,
    };
    let model = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "test-model".to_owned(),
        api: ApiKind::OpenAiResponse,
        capabilities: ModelCapabilities::tool_chat(),
    };

    let agent_config = agent_config_for_app(model, &config, None, None).expect("agent config");

    // The production root Workflow tool must receive the session-shared
    // registry: an empty default registry would fail this resolve even
    // though the definition pair exists on disk.
    let resolved = agent_config
        .workflow_definitions
        .resolve("saved-probe")
        .expect("saved definition must be visible through production agent config");
    assert_eq!(resolved.name.as_str(), "saved-probe");
}

#[test]
fn agent_config_for_app_falls_back_to_model_max_output_tokens() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = AppConfig {
        default_model: "test-model".to_owned(),
        default_provider: "openai".to_owned(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir: temp.path().join(".neo/sessions"),
        permission_mode: PermissionMode::default(),
        live_permission_mode: std::sync::Arc::new(
            std::sync::RwLock::new(PermissionMode::default()),
        ),
        workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
        defaults: Defaults {
            mode: "events".to_owned(),
        },
        runtime: RuntimeConfig {
            temperature: None,
            max_tokens: None,
            reasoning: neo_ai::ReasoningSelection::Off,
            replay_reasoning: true,
            steering_queue_mode: QueueMode::OneAtATime,
            follow_up_queue_mode: QueueMode::OneAtATime,
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
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir: temp.path().to_path_buf(),
        config_path: temp.path().join(".neo/config.toml"),
        config_file_exists: true,
    };
    // Model declares max_output_tokens; runtime does not override.
    let model = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "test-model".to_owned(),
        api: ApiKind::OpenAiResponse,
        capabilities: ModelCapabilities::tool_chat().with_max_output_tokens(64_000),
    };

    let agent_config = agent_config_for_app(model, &config, None, None).expect("agent config");

    assert_eq!(agent_config.max_tokens, Some(64_000));
}

#[test]
fn user_message_preserves_injection_origin() {
    let origin = MessageOrigin::injection("init");

    let message = user_message(
        vec![Content::text("<system-reminder>\ninit\n</system-reminder>")],
        origin.clone(),
        None,
    );

    assert!(message.is_injection());
    assert_eq!(
        message,
        AgentMessage::User {
            content: vec![Content::text("<system-reminder>\ninit\n</system-reminder>")],
            display_text: None,
            origin,
        }
    );
}

#[test]
fn agent_config_for_app_scales_default_compaction_to_model_context_window() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = AppConfig {
        default_model: "large-context-model".to_owned(),
        default_provider: "anthropic".to_owned(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir: temp.path().join(".neo/sessions"),
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
            tool_execution_mode: ToolExecutionMode::Parallel,
            compaction: Some(RuntimeCompactionConfig {
                enabled: true,
                max_estimated_tokens: 32_000,
                keep_recent_messages: 20,
                trigger_ratio: 0.85,
                reserved_context_tokens: 50_000,
                max_recent_messages: 4,
                micro_enabled: false,
                micro_keep_recent: 20,
                snip_enabled: false,
                snip_min_tokens: 1_000,
                snip_keep_recent: 16,
                snip_trigger_ratio: 0.6,
                max_rounds: 5,
                max_retry_attempts: 5,
            }),
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
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir: temp.path().to_path_buf(),
        config_path: temp.path().join(".neo/config.toml"),
        config_file_exists: true,
    };
    let model = ModelSpec {
        provider: ProviderId("anthropic".to_owned()),
        model: "large-context-model".to_owned(),
        api: ApiKind::AnthropicMessages,
        capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
    };

    let agent_config = agent_config_for_app(model, &config, None, None).expect("agent config");

    assert_eq!(
        agent_config.compaction,
        Some(CompactionSettings {
            enabled: true,
            max_estimated_tokens: 800_000,
            keep_recent_messages: 20,
            trigger_ratio: 0.85,
            reserved_context_tokens: 50_000,
            max_recent_messages: 4,
            micro_enabled: false,
            micro_keep_recent: 20,
            snip_enabled: false,
            snip_min_tokens: 1_000,
            snip_keep_recent: 16,
            snip_trigger_ratio: 0.6,
            max_rounds: 5,
            max_retry_attempts: 5,
        })
    );
}

#[test]
fn agent_config_for_app_keeps_explicit_custom_compaction_threshold() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = AppConfig {
        default_model: "large-context-model".to_owned(),
        default_provider: "anthropic".to_owned(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir: temp.path().join(".neo/sessions"),
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
            tool_execution_mode: ToolExecutionMode::Parallel,
            compaction: Some(RuntimeCompactionConfig {
                enabled: true,
                max_estimated_tokens: 12_000,
                keep_recent_messages: 16,
                trigger_ratio: 0.85,
                reserved_context_tokens: 50_000,
                max_recent_messages: 4,
                micro_enabled: false,
                micro_keep_recent: 20,
                snip_enabled: false,
                snip_min_tokens: 1_000,
                snip_keep_recent: 16,
                snip_trigger_ratio: 0.6,
                max_rounds: 5,
                max_retry_attempts: 5,
            }),
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
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir: temp.path().to_path_buf(),
        config_path: temp.path().join(".neo/config.toml"),
        config_file_exists: true,
    };
    let model = ModelSpec {
        provider: ProviderId("anthropic".to_owned()),
        model: "large-context-model".to_owned(),
        api: ApiKind::AnthropicMessages,
        capabilities: ModelCapabilities::tool_chat().with_max_context_tokens(1_000_000),
    };

    let agent_config = agent_config_for_app(model, &config, None, None).expect("agent config");

    assert_eq!(
        agent_config.compaction,
        Some(CompactionSettings {
            enabled: true,
            max_estimated_tokens: 12_000,
            keep_recent_messages: 16,
            trigger_ratio: 0.85,
            reserved_context_tokens: 50_000,
            max_recent_messages: 4,
            micro_enabled: false,
            micro_keep_recent: 20,
            snip_enabled: false,
            snip_min_tokens: 1_000,
            snip_keep_recent: 16,
            snip_trigger_ratio: 0.6,
            max_rounds: 5,
            max_retry_attempts: 5,
        })
    );
}

#[tokio::test]
async fn agent_config_for_app_async_approval_channel_waits_for_ui_decision() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = AppConfig {
        default_model: "test-model".to_owned(),
        default_provider: "openai".to_owned(),
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir: temp.path().join(".neo/sessions"),
        permission_mode: PermissionMode::default(),
        live_permission_mode: std::sync::Arc::new(
            std::sync::RwLock::new(PermissionMode::default()),
        ),
        workspace_policy: std::sync::Arc::new(std::sync::RwLock::new(None)),
        defaults: Defaults {
            mode: "interactive".to_owned(),
        },
        runtime: RuntimeConfig::default(),
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
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir: temp.path().to_path_buf(),
        config_path: temp.path().join(".neo/config.toml"),
        config_file_exists: true,
    };
    let model = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "test-model".to_owned(),
        api: ApiKind::OpenAiResponse,
        capabilities: ModelCapabilities::tool_chat(),
    };
    let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel();
    let agent_config =
        agent_config_for_app(model, &config, Some(approval_tx), None).expect("agent config");
    let handler = agent_config
        .async_approval_handler
        .expect("async approval handler");

    let response_task = tokio::spawn(handler(sample_tool_approval_request("tool-1")));
    let PendingApproval {
        request,
        response_tx,
    } = approval_rx.recv().await.expect("approval waiter");

    assert_eq!(request.id, "tool-1");
    response_tx
        .send(ApprovalResponse::Selected {
            request_id: request.id,
            action: ApprovalAction::PermitOnce,
            feedback: None,
        })
        .expect("send response");
    assert!(matches!(
        response_task.await.expect("approval task joins"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitOnce,
            feedback: None,
            ..
        }
    ));
}

#[tokio::test]
async fn async_approval_handler_returns_revision_feedback_atomically() {
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
            capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
            ..ModelConfig::default()
        },
    );
    let (approval_tx, mut approval_rx) = tokio::sync::mpsc::unbounded_channel();
    let (events, _) = tokio::sync::mpsc::unbounded_channel();
    let (session_ids, _) = tokio::sync::mpsc::unbounded_channel();
    let (questions, _) = tokio::sync::mpsc::unbounded_channel();
    let request = TurnRequest::new(Vec::new(), None, None, neo_ai::ReasoningSelection::Off);
    let channels = TurnChannels {
        events,
        approvals: approval_tx,
        session_ids,
        cancel_token: CancellationToken::new(),
        questions,
        steer_input: SteerInputHandle::new(),
    };
    let runtime = runtime_for_config(&config, None, Some(&request), Some(&channels))
        .await
        .expect("runtime");
    let handler = runtime
        .config()
        .async_approval_handler
        .clone()
        .expect("async approval handler");

    let response_task = tokio::spawn(handler(sample_plan_approval_request("tool-1")));
    let PendingApproval {
        request,
        response_tx,
    } = approval_rx.recv().await.expect("approval waiter");

    assert_eq!(request.id, "tool-1");
    response_tx
        .send(ApprovalResponse::Selected {
            request_id: request.id,
            action: ApprovalAction::RevisePlan {
                preset_feedback: None,
            },
            feedback: Some("tighten the implementation scope".to_owned()),
        })
        .expect("send response");

    match response_task.await.expect("approval task joins") {
        ApprovalResponse::Selected {
            action: ApprovalAction::RevisePlan { .. },
            feedback: Some(feedback),
            ..
        } => assert_eq!(feedback, "tighten the implementation scope"),
        other => panic!("expected revise response, got {other:?}"),
    }
}

#[tokio::test]
async fn startup_builds_one_registry_and_baseline_before_first_provider_request() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join("AGENTS.md"), "root rules\n").expect("AGENTS.md");
    let session_dir = temp
        .path()
        .join("session_00000000-0000-4000-8000-000000000502");
    let session_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
    tokio::fs::create_dir_all(session_path.parent().expect("wire parent"))
        .await
        .expect("wire dir");
    let registry = Arc::new(
        InstructionRegistry::new(InstructionRegistryConfig {
            primary_workspace: workspace.clone(),
            neo_home: None,
            project_trusted: true,
        })
        .expect("registry"),
    );
    let fake = FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "answer".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    let mut config = AgentConfig::for_model(fake_model())
        .with_workspace_root(&workspace)
        .expect("workspace root");
    config.instruction_registry = Some(registry);
    let runtime = super::super::AgentRuntime::new(config, Arc::new(fake.clone()));
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .expect("session writer");

    run_prompt_with_runtime(
        "first prompt".to_owned(),
        AgentContext::new(),
        &mut writer,
        runtime,
    )
    .await
    .expect("run prompt");

    let events = JsonlSessionReader::read_all(&session_path)
        .await
        .expect("read events");
    let epoch = events
        .iter()
        .position(|event| matches!(event, AgentEvent::InstructionEpoch { .. }))
        .expect("instruction epoch");
    let user = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageAppended {
                    message: AgentMessage::User { .. }
                }
            )
        })
        .expect("user event");
    assert!(
        epoch < user,
        "persisted baseline must precede user: {events:?}"
    );
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let request_text = requests[0]
        .messages
        .iter()
        .map(chat_message_text)
        .collect::<Vec<_>>();
    let rules = request_text
        .iter()
        .position(|text| text.contains("root rules"))
        .expect("baseline rules in first provider request");
    let prompt = request_text
        .iter()
        .position(|text| text == "first prompt")
        .expect("first prompt in provider request");
    assert!(rules < prompt, "request order: {request_text:?}");
}
