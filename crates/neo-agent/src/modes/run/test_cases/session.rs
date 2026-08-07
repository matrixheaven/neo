//! session behavior (moved from `mod.rs`).

use super::*;
use std::{collections::BTreeMap, sync::Arc};

use neo_agent_core::instructions::{InstructionRegistry, InstructionRegistryConfig};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, Content, MessageOrigin, PermissionMode,
    ToolRegistry,
    harness::FakeHarness,
    session::{JsonlSessionReader, JsonlSessionWriter},
};
use neo_ai::{AiStreamEvent, ApiType, StopReason, providers::fake::FakeModelClient};

use super::super::run_prompt_with_runtime;
use super::super::session_mgmt::{
    latest_session_id, session_id_from_path, session_root_from_wire_path,
};
use crate::config::{AppConfig, Defaults, McpConfig, ProviderConfig, RuntimeConfig, TuiConfig};

#[tokio::test]
async fn session_root_from_wire_path_returns_session_directory() {
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

    let wire_path = crate::modes::sessions::create_new_session(&config)
        .await
        .expect("session path is created")
        .wire_path;
    let session_root =
        session_root_from_wire_path(&wire_path).expect("session root from wire path");

    assert_eq!(
        neo_agent_core::session::main_agent_wire_path(&session_root),
        wire_path
    );
    assert_eq!(
        session_root.file_name().and_then(std::ffi::OsStr::to_str),
        session_id_from_path(&wire_path).ok().as_deref()
    );
}

#[test]
fn latest_session_id_ignores_main_wire_directories() {
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
    let bucket_dir = crate::config::workspace_sessions_dir(&config);
    let valid_id = "session_00000000-0000-4000-8000-000000000001";
    let directory_wire_id = "session_00000000-0000-4000-8000-000000000999";
    let valid_wire = neo_agent_core::session::main_agent_wire_path(&bucket_dir.join(valid_id));
    std::fs::create_dir_all(valid_wire.parent().expect("valid wire parent"))
        .expect("create valid wire parent");
    std::fs::write(valid_wire, "{}\n").expect("write valid wire");
    std::fs::create_dir_all(neo_agent_core::session::main_agent_wire_path(
        &bucket_dir.join(directory_wire_id),
    ))
    .expect("create directory wire");

    assert_eq!(
        latest_session_id(&config).expect("latest session"),
        valid_id
    );
}

#[tokio::test]
async fn unchanged_resume_replays_epoch_without_duplicate_message_or_card() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join("AGENTS.md"), "rules v1\n").expect("AGENTS.md");
    let session_dir = temp
        .path()
        .join("session_00000000-0000-4000-8000-000000000504");
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
    let first_fake = FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-1".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    let mut first_config = AgentConfig::for_model(fake_model())
        .with_workspace_root(&workspace)
        .expect("workspace root");
    first_config.instruction_registry = Some(Arc::clone(&registry));
    let first_runtime = super::super::AgentRuntime::new(first_config, Arc::new(first_fake));
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .expect("session writer");
    run_prompt_with_runtime(
        "first prompt".to_owned(),
        AgentContext::new(),
        &mut writer,
        first_runtime,
    )
    .await
    .expect("first turn");
    drop(writer);

    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .expect("replay context");
    let resumed_fake = FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-2".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    let mut resumed_config = AgentConfig::for_model(fake_model())
        .with_workspace_root(&workspace)
        .expect("workspace root");
    resumed_config.instruction_registry = Some(registry);
    let resumed_runtime =
        super::super::AgentRuntime::new(resumed_config, Arc::new(resumed_fake.clone()));
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .expect("append session");

    let turn = run_prompt_with_runtime(
        "unchanged".to_owned(),
        context,
        &mut writer,
        resumed_runtime,
    )
    .await
    .expect("unchanged resumed turn");

    assert!(
        turn.events
            .iter()
            .all(|event| !matches!(event, AgentEvent::InstructionEpoch { .. })),
        "unchanged resume emitted duplicate epoch: {:?}",
        turn.events,
    );
    let requests = resumed_fake.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .messages
            .iter()
            .map(chat_message_text)
            .filter(|text| text.contains("rules v1"))
            .count(),
        1,
        "unchanged resume must replay one instruction snapshot",
    );
}

#[tokio::test]
async fn changed_source_after_resume_appends_replacement_before_provider_call() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let agents_path = workspace.join("AGENTS.md");
    std::fs::write(&agents_path, "rules v1\n").expect("AGENTS.md v1");
    let session_dir = temp
        .path()
        .join("session_00000000-0000-4000-8000-000000000503");
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
    let first_fake = FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-1".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    let mut first_config = AgentConfig::for_model(fake_model())
        .with_workspace_root(&workspace)
        .expect("workspace root");
    first_config.instruction_registry = Some(Arc::clone(&registry));
    let first_runtime = super::super::AgentRuntime::new(first_config, Arc::new(first_fake));
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .expect("session writer");
    run_prompt_with_runtime(
        "first prompt".to_owned(),
        AgentContext::new(),
        &mut writer,
        first_runtime,
    )
    .await
    .expect("first turn");
    drop(writer);

    std::fs::write(&agents_path, "rules v2\n").expect("AGENTS.md v2");
    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .expect("replay updated context");
    let resumed_fake = FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-3".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    let mut resumed_config = AgentConfig::for_model(fake_model())
        .with_workspace_root(&workspace)
        .expect("workspace root");
    resumed_config.instruction_registry = Some(Arc::clone(&registry));
    let resumed_runtime =
        super::super::AgentRuntime::new(resumed_config, Arc::new(resumed_fake.clone()));
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .expect("append session");

    let turn =
        run_prompt_with_runtime("continue".to_owned(), context, &mut writer, resumed_runtime)
            .await
            .expect("resumed turn");

    let updated = turn
        .events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::InstructionEpoch { epoch }
                    if epoch.outcome
                        == neo_agent_core::instructions::InstructionEpochOutcome::Updated
            )
        })
        .expect("updated instruction epoch");
    let user = turn
        .events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageAppended {
                    message: AgentMessage::User { .. }
                }
            )
        })
        .expect("resumed user event");
    assert!(
        updated < user,
        "replacement must precede user: {:?}",
        turn.events
    );
    let requests = resumed_fake.requests();
    assert_eq!(requests.len(), 1);
    let request_text = requests[0]
        .messages
        .iter()
        .map(chat_message_text)
        .collect::<Vec<_>>();
    let v2 = request_text
        .iter()
        .position(|text| text.contains("rules v2"))
        .expect("updated rules in first resumed request");
    let prompt = request_text
        .iter()
        .position(|text| text == "continue")
        .expect("resumed prompt");
    assert!(v2 < prompt, "request order: {request_text:?}");
    drop(writer);

    std::fs::remove_file(&agents_path).expect("remove AGENTS.md");
    let context = JsonlSessionReader::replay_context(&session_path)
        .await
        .expect("replay removed context");
    let removed_fake = FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-4".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    let mut removed_config = AgentConfig::for_model(fake_model())
        .with_workspace_root(&workspace)
        .expect("workspace root");
    removed_config.instruction_registry = Some(registry);
    let removed_runtime =
        super::super::AgentRuntime::new(removed_config, Arc::new(removed_fake.clone()));
    let mut writer = JsonlSessionWriter::open_append(&session_path)
        .await
        .expect("append removed session");

    let turn = run_prompt_with_runtime(
        "after removal".to_owned(),
        context,
        &mut writer,
        removed_runtime,
    )
    .await
    .expect("removed resumed turn");

    let removed = turn
        .events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::InstructionEpoch { epoch }
                    if epoch.outcome
                        == neo_agent_core::instructions::InstructionEpochOutcome::Removed
            )
        })
        .expect("removed instruction epoch");
    let user = turn
        .events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageAppended {
                    message: AgentMessage::User { .. }
                }
            )
        })
        .expect("removed resume user event");
    assert!(
        removed < user,
        "removal must precede user: {:?}",
        turn.events
    );
    let requests = removed_fake.requests();
    assert_eq!(requests.len(), 1);
    let request_text = requests[0]
        .messages
        .iter()
        .map(chat_message_text)
        .collect::<Vec<_>>();
    let v1 = request_text
        .iter()
        .position(|text| text.contains("rules v1"))
        .expect("historical v1 instruction snapshot");
    let v2 = request_text
        .iter()
        .position(|text| text.contains("rules v2"))
        .expect("historical v2 instruction snapshot");
    let empty_authority = request_text
        .iter()
        .rposition(|text| {
            text.contains("<instruction_active_state") && !text.contains("<active_instruction")
        })
        .expect("removed authority snapshot");
    let prompt = request_text
        .iter()
        .position(|text| text == "after removal")
        .expect("removed resume prompt");
    assert!(
        v1 < v2 && v2 < empty_authority && empty_authority < prompt,
        "append-only authority order: {request_text:?}",
    );
}

#[tokio::test]
async fn nested_scope_import_and_over_budget_warning_replan_without_breaking_turn() {
    const LOADED: &str = "NESTED-IMPORT-LOADED-7a31";
    const IGNORED: &str = "ROOT-BUNDLE-IGNORED-8c42";
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let nested = workspace.join("nested");
    std::fs::create_dir_all(&nested).expect("nested workspace");
    std::fs::write(
        workspace.join("AGENTS.md"),
        format!("{IGNORED}\n{}", "large root rules ".repeat(6_000)),
    )
    .expect("root AGENTS.md");
    std::fs::write(nested.join("AGENTS.md"), "@./imported.md\n").expect("nested AGENTS.md");
    std::fs::write(nested.join("imported.md"), format!("{LOADED}\n")).expect("nested import");
    std::fs::write(nested.join("data.txt"), "nested data\n").expect("nested data");
    let session_dir = temp
        .path()
        .join("session_00000000-0000-4000-8000-000000000505");
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
    let read_arguments = serde_json::json!({ "path": "nested/data.txt" }).to_string();
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "msg-1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "call-1".to_owned(),
                name: "Read".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "call-1".to_owned(),
                raw_arguments: read_arguments.clone(),
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
                id: "msg-2".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "call-2".to_owned(),
                name: "Read".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "call-2".to_owned(),
                raw_arguments: read_arguments,
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
                id: "msg-3".to_owned(),
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
    let mut model = harness.model();
    model.capabilities.max_context_tokens = Some(32_768);
    let mut config = AgentConfig::for_model(model)
        .with_workspace_root(&workspace)
        .expect("workspace root");
    config.max_tokens = Some(1_024);
    config.instruction_registry = Some(registry);
    let runtime = super::super::AgentRuntime::with_tools(
        config,
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut writer = JsonlSessionWriter::create(&session_path)
        .await
        .expect("session writer");

    let turn = run_prompt_with_runtime(
        "read nested data".to_owned(),
        AgentContext::new(),
        &mut writer,
        runtime,
    )
    .await
    .expect("nested over-budget turn");

    let requests = harness.requests();
    assert_eq!(
        turn.assistant_text,
        "done",
        "events: {:?}; provider requests: {}",
        turn.events,
        requests.len(),
    );
    let epoch = turn
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::InstructionEpoch { epoch }
                if epoch.deferred_tool_ids == ["call-1".to_owned()] =>
            {
                Some(epoch)
            }
            _ => None,
        })
        .expect("nested partially-loaded epoch");
    assert_eq!(
        epoch.outcome,
        neo_agent_core::instructions::InstructionEpochOutcome::PartiallyLoaded,
    );
    let canonical_workspace = workspace.canonicalize().expect("canonical workspace");
    let canonical_nested = nested.canonicalize().expect("canonical nested");
    assert!(
        epoch
            .selected_bundles
            .iter()
            .any(|bundle| bundle.display_path == canonical_nested
                && bundle.import_count == 1
                && bundle
                    .import_paths
                    .iter()
                    .any(|path| path.ends_with("imported.md"))),
        "loaded nested import metadata: {epoch:?}",
    );
    assert!(
        epoch
            .ignored_bundles
            .iter()
            .any(|bundle| bundle.display_path == canonical_workspace),
        "ignored root metadata: {epoch:?}",
    );
    let authority = epoch.model_content.as_deref().expect("nested authority");
    assert!(
        authority.contains(LOADED),
        "loaded import missing: {authority}"
    );
    assert!(
        authority.contains("over budget"),
        "warning missing: {authority}"
    );
    assert!(
        !authority.contains(IGNORED),
        "ignored body leaked: {authority}"
    );

    let deferred = turn.events.iter().find_map(|event| match event {
        AgentEvent::ToolExecutionFinished { id, result, .. } if id == "call-1" => Some(result),
        _ => None,
    });
    let deferred = deferred.expect("deferred tool result");
    assert!(!deferred.is_error);
    assert_eq!(
        deferred.details.as_ref().expect("deferred details")["status"],
        "deferred",
    );
    let retried = turn.events.iter().find_map(|event| match event {
        AgentEvent::ToolExecutionFinished { id, result, .. } if id == "call-2" => Some(result),
        _ => None,
    });
    assert!(retried.is_some_and(|result| !result.is_error));
    assert_eq!(requests.len(), 3);
    let after_defer = requests[1]
        .messages
        .iter()
        .map(chat_message_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(after_defer.contains(LOADED));
    assert!(!after_defer.contains(IGNORED));
    assert!(after_defer.contains("Tool call deferred"));
}

#[tokio::test]
async fn prepare_existing_streaming_turn_uses_session_root_for_main_wire_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path());
    let session_id = "session_00000000-0000-4000-8000-000000000502";
    let session_dir = crate::config::workspace_sessions_dir(&config).join(session_id);
    let session_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
    tokio::fs::create_dir_all(session_path.parent().expect("wire parent"))
        .await
        .expect("create wire dir");
    let mut seed = JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session");
    seed.append_event(&AgentEvent::MessageAppended {
        message: AgentMessage::user_text("hello"),
    })
    .await
    .expect("append user");
    seed.flush().await.expect("flush seed");
    drop(seed);

    let prepared = super::super::prepare_existing_streaming_turn(
        session_id,
        &[Content::text("continue")],
        MessageOrigin::User,
        None,
        &config,
        None,
        None,
    )
    .await
    .expect("prepare existing streaming turn");

    assert_eq!(prepared.session_directory, session_dir);
}

#[tokio::test]
async fn workflow_recovery_persists_only_newer_projection() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path());
    let session_id = "session_00000000-0000-4000-8000-000000000778";
    let session_directory = crate::config::workspace_sessions_dir(&config).join(session_id);
    tokio::fs::create_dir_all(&session_directory)
        .await
        .expect("session directory");

    let started = {
        let seed_runtime = neo_agent_core::workflow::WorkflowRuntime::new(
            neo_agent_core::workflow::WorkflowLimits::default(),
        );
        let handle = seed_runtime
            .create_run(
                &session_directory,
                neo_agent_core::workflow::WorkflowLaunchRequest {
                    name: "recover projection".to_owned(),
                    description: "test recovery projection ordering".to_owned(),
                    phases: vec![neo_agent_core::workflow::WorkflowPhase {
                        id: "work".to_owned(),
                        description: "work".to_owned(),
                    }],
                    script: "neo.phase('work')".to_owned(),
                    args: serde_json::json!({}),
                    launch_source: "test".to_owned(),
                    output_schema: None,
                    display_name: None,
                    input_schema: None,
                    definition_origin: None,
                    inline_unsaved: false,
                },
            )
            .await
            .expect("seed workflow");
        handle.snapshot().await
    };
    assert_eq!(started.projection_sequence, Some(0));

    let wire_path = neo_agent_core::session::main_agent_wire_path(&session_directory);
    tokio::fs::create_dir_all(wire_path.parent().expect("wire parent"))
        .await
        .expect("wire directory");
    let started_event = AgentEvent::WorkflowStarted {
        turn: 7,
        workflow: started,
    };
    let mut writer = JsonlSessionWriter::create(&wire_path)
        .await
        .expect("session writer");
    writer
        .append_event(&started_event)
        .await
        .expect("historical projection");
    writer.flush().await.expect("historical projection flush");
    drop(writer);

    let replayed = JsonlSessionReader::read_all(&wire_path)
        .await
        .expect("historical session");
    let recovered = super::super::rehydrate_session_workflows(
        &config,
        session_id,
        &session_directory,
        &replayed,
    )
    .await
    .expect("recover workflow projection");
    assert_eq!(recovered.len(), 1);
    let AgentEvent::WorkflowUpdated { turn, workflow } = &recovered[0] else {
        panic!("host-exit recovery is a paused update")
    };
    assert_eq!(*turn, 7);
    assert_eq!(
        workflow.state,
        neo_agent_core::workflow::WorkflowState::Paused
    );
    assert_eq!(workflow.projection_sequence, Some(1));
    assert_eq!(workflow.terminal_reason.as_deref(), Some("host_exit"));
    assert!(
        config
            .background_tasks
            .workflow_handle(&workflow.id.0)
            .await
            .is_some(),
        "recovery registers the canonical handle before projection"
    );

    let stored = JsonlSessionReader::read_all(&wire_path)
        .await
        .expect("recovered session");
    assert_eq!(
        stored
            .iter()
            .filter(|event| matches!(
                event,
                AgentEvent::WorkflowStarted { .. }
                    | AgentEvent::WorkflowUpdated { .. }
                    | AgentEvent::WorkflowFinished { .. }
            ))
            .count(),
        2
    );
    let duplicate =
        super::super::rehydrate_session_workflows(&config, session_id, &session_directory, &stored)
            .await
            .expect("idempotent workflow recovery");
    assert!(duplicate.is_empty());
    let stored_again = JsonlSessionReader::read_all(&wire_path)
        .await
        .expect("idempotent recovered session");
    assert_eq!(
        stored_again, stored,
        "equal durable sequence is not appended"
    );
}

#[tokio::test]
async fn fresh_process_rehydrated_workflow_can_resume_with_bound_runner() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    config.default_model = "gpt-4.1".to_owned();
    config.providers.insert(
        "openai".to_owned(),
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::OpenAiResponse),
            base_url: Some("https://example.test/v1".to_owned()),
            api_key: Some("test-key".to_owned()),
            api_key_env: None,
        },
    );
    let session_id = "session_00000000-0000-4000-8000-000000000781";
    let session_directory = crate::config::workspace_sessions_dir(&config).join(session_id);
    tokio::fs::create_dir_all(&session_directory)
        .await
        .expect("session directory");

    let seed_runtime = neo_agent_core::workflow::WorkflowRuntime::new(
        neo_agent_core::workflow::WorkflowLimits::default(),
    );
    let seeded = seed_runtime
        .create_run(
            &session_directory,
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "fresh process resume".to_owned(),
                description: "test recovery runner composition".to_owned(),
                phases: vec![neo_agent_core::workflow::WorkflowPhase {
                    id: "work".to_owned(),
                    description: "work".to_owned(),
                }],
                script: "neo.phase('work')".to_owned(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
            },
        )
        .await
        .expect("seed workflow");
    let run_id = seeded.run_id.0.clone();
    drop(seeded);
    drop(seed_runtime);

    let wire_path = neo_agent_core::session::main_agent_wire_path(&session_directory);
    tokio::fs::create_dir_all(wire_path.parent().expect("wire parent"))
        .await
        .expect("wire directory");
    let mut writer = JsonlSessionWriter::create(&wire_path)
        .await
        .expect("empty session writer");
    writer.flush().await.expect("empty session flush");
    drop(writer);

    super::super::rehydrate_session_workflows(&config, session_id, &session_directory, &[])
        .await
        .expect("fresh runtime recovery");
    let handle = config
        .background_tasks
        .workflow_handle(&run_id)
        .await
        .expect("recovered workflow handle");
    assert_eq!(
        handle.snapshot().await.state,
        neo_agent_core::workflow::WorkflowState::Paused
    );

    handle
        .resume(neo_agent_core::workflow::WorkflowActor::Human)
        .await
        .expect("fresh-process recovered workflow can start its runner");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if handle.snapshot().await.state == neo_agent_core::workflow::WorkflowState::Completed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("recovered workflow completes through the prepared dispatch runtime");
    assert_eq!(handle.run_id.0, run_id);
}

#[tokio::test]
async fn corrupt_workflow_recovery_persists_terminal_projection_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path());
    let session_id = "session_00000000-0000-4000-8000-000000000779";
    let session_directory = crate::config::workspace_sessions_dir(&config).join(session_id);
    let run_directory = session_directory.join("workflows").join("wf_corrupt");
    tokio::fs::create_dir_all(&run_directory)
        .await
        .expect("corrupt workflow directory");
    tokio::fs::write(run_directory.join("run.json"), b"not-json")
        .await
        .expect("corrupt run metadata");

    let wire_path = neo_agent_core::session::main_agent_wire_path(&session_directory);
    tokio::fs::create_dir_all(wire_path.parent().expect("wire parent"))
        .await
        .expect("wire directory");
    let historical_running: neo_agent_core::workflow::WorkflowSnapshot =
        serde_json::from_value(serde_json::json!({
            "id": "wf_corrupt",
            "title": "Workflow before corruption",
            "state": "running",
            "projection_sequence": 4
        }))
        .expect("historical workflow snapshot");
    let mut writer = JsonlSessionWriter::create(&wire_path)
        .await
        .expect("session writer");
    writer
        .append_event(&AgentEvent::WorkflowStarted {
            turn: 8,
            workflow: historical_running.clone(),
        })
        .await
        .expect("historical running projection");
    writer.flush().await.expect("historical projection flush");
    drop(writer);

    let replayed = JsonlSessionReader::read_all(&wire_path)
        .await
        .expect("historical session");
    let recovered = super::super::rehydrate_session_workflows(
        &config,
        session_id,
        &session_directory,
        &replayed,
    )
    .await
    .expect("recover corrupt workflow projection");
    assert_eq!(recovered.len(), 1);
    let AgentEvent::WorkflowFinished { turn, workflow } = &recovered[0] else {
        panic!("corrupt workflow recovery is terminal")
    };
    assert_eq!(*turn, 8);
    assert_eq!(
        workflow.state,
        neo_agent_core::workflow::WorkflowState::Failed
    );
    assert_eq!(workflow.projection_sequence, None);
    assert!(workflow.recovery_failure);
    assert!(
        workflow
            .terminal_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("corrupt run metadata"))
    );

    let stored = JsonlSessionReader::read_all(&wire_path)
        .await
        .expect("recovered corrupt session");
    assert_eq!(stored.len(), 2);
    let mut writer = JsonlSessionWriter::open_append(&wire_path)
        .await
        .expect("append stale projection");
    writer
        .append_event(&AgentEvent::WorkflowUpdated {
            turn: 8,
            workflow: historical_running,
        })
        .await
        .expect("stale sequenced projection");
    writer.flush().await.expect("stale projection flush");
    drop(writer);
    let stored_with_stale = JsonlSessionReader::read_all(&wire_path)
        .await
        .expect("session with stale projection");
    let duplicate = super::super::rehydrate_session_workflows(
        &config,
        session_id,
        &session_directory,
        &stored_with_stale,
    )
    .await
    .expect("idempotent corrupt workflow recovery");
    assert!(duplicate.is_empty());
    let stored_again = JsonlSessionReader::read_all(&wire_path)
        .await
        .expect("idempotent corrupt session");
    assert_eq!(stored_again, stored_with_stale);
}

#[tokio::test]
async fn empty_workflow_journal_recovery_projects_terminal_failure() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path());
    let session_id = "session_00000000-0000-4000-8000-000000000780";
    let session_directory = crate::config::workspace_sessions_dir(&config).join(session_id);
    let seed_runtime = neo_agent_core::workflow::WorkflowRuntime::new(
        neo_agent_core::workflow::WorkflowLimits::default(),
    );
    let handle = seed_runtime
        .create_run(
            &session_directory,
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "empty journal".to_owned(),
                description: "test empty journal recovery".to_owned(),
                phases: vec![neo_agent_core::workflow::WorkflowPhase {
                    id: "work".to_owned(),
                    description: "work".to_owned(),
                }],
                script: "neo.phase('work')".to_owned(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
            },
        )
        .await
        .expect("seed workflow");
    let historical_running = handle.snapshot().await;
    let run_directory = session_directory
        .join("workflows")
        .join(&historical_running.id.0);
    drop(handle);
    drop(seed_runtime);
    tokio::fs::write(run_directory.join("journal.jsonl"), b"")
        .await
        .expect("empty journal");

    let wire_path = neo_agent_core::session::main_agent_wire_path(&session_directory);
    tokio::fs::create_dir_all(wire_path.parent().expect("wire parent"))
        .await
        .expect("wire directory");
    let mut writer = JsonlSessionWriter::create(&wire_path)
        .await
        .expect("session writer");
    writer
        .append_event(&AgentEvent::WorkflowStarted {
            turn: 9,
            workflow: historical_running,
        })
        .await
        .expect("historical running projection");
    writer.flush().await.expect("historical projection flush");
    drop(writer);

    let replayed = JsonlSessionReader::read_all(&wire_path)
        .await
        .expect("historical session");
    let recovered = super::super::rehydrate_session_workflows(
        &config,
        session_id,
        &session_directory,
        &replayed,
    )
    .await
    .expect("recover empty journal projection");

    assert_eq!(recovered.len(), 1);
    let AgentEvent::WorkflowFinished { workflow, .. } = &recovered[0] else {
        panic!("empty journal recovery is terminal")
    };
    assert_eq!(
        workflow.state,
        neo_agent_core::workflow::WorkflowState::Failed
    );
    assert!(workflow.recovery_failure);
    assert_eq!(workflow.projection_sequence, None);
    assert_eq!(
        workflow.terminal_reason.as_deref(),
        Some("recovery append failed: journal_corrupt: journal is missing run_created")
    );
}
