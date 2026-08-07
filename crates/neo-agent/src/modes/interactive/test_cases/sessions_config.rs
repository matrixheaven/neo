//! Session controller/config/model behavior (split from `sessions.rs`).

use std::{collections::BTreeMap, fs, path::PathBuf};

use neo_agent_core::{AgentEvent, Content, PermissionMode, StopReason, ToolResult};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    shell::{ChromeMode, OverlayKind},
};
use tokio::sync::oneshot;

use super::super::catalog_fetch::{
    CatalogFetchCompletion, CatalogFetchSource, PendingCatalogRefresh,
};
use super::super::*;
use super::*;
use crate::config::ModelConfig;

#[tokio::test]
async fn controller_snapshot_uses_transcript_tool_card_rendering() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move {
            Ok(vec![
                AgentEvent::ToolExecutionStarted {
                    turn: 1,
                    id: "tool-1".to_owned(),
                    name: "Read".to_owned(),
                    arguments: serde_json::json!({ "path": "README.md" }),
                    workflow_origin: None,
                    output_ref: None,
                },
                AgentEvent::ToolExecutionFinished {
                    turn: 1,
                    id: "tool-1".to_owned(),
                    name: "Read".to_owned(),
                    result: ToolResult::ok("line one\nline two"),
                    workflow_origin: None,
                    output_ref: None,
                },
            ])
        },
    );

    controller.type_text("inspect");
    let snapshot = controller.submit_prompt().await.expect("prompt succeeds");

    assert!(
        snapshot.contains("● Used Read (README.md)"),
        "transcript snapshot should include finalized tool card, got:\n{snapshot}"
    );
    assert!(snapshot.contains("> "));
}

#[tokio::test]
async fn controller_submits_prompt_reduces_turn_events_and_renders_snapshot() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |request| async move {
            assert_eq!(request.prompt, vec![Content::text("hello neo")]);
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
                    text: "Hello".to_owned(),
                },
                AgentEvent::TextDelta {
                    turn: 1,
                    text: ", Neo".to_owned(),
                },
                AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                },
            ])
        },
    );

    controller.type_text("hello neo");
    let snapshot = controller.submit_prompt().await.expect("turn succeeds");

    assert!(snapshot.contains("Welcome to neo"));
    assert!(snapshot.contains("test-session"));
    assert!(snapshot.contains("openai/gpt-4.1"));
    // The user prompt and assistant reply appear in the rendered frame.
    assert!(snapshot.contains("hello neo"));
    assert!(snapshot.contains("Hello, Neo"));
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
}

#[tokio::test]
async fn provider_refresh_completion_reloads_config_and_keeps_dialog_open() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join(".neo/config.toml");
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("create config dir");
    fs::write(
        &config_path,
        r#"
default_model = "openai/old"
default_provider = "openai"

[providers.openai]
type = "openai_response"
api_key = "keep-me"

[models."openai/old"]
provider = "openai"
model = "old"
"#,
    )
    .expect("write config");
    let config = crate::config::AppConfig::load(crate::config::ConfigOverrides {
        config_path: Some(config_path.clone()),
        project_dir: Some(temp.path().to_path_buf()),
        ..crate::config::ConfigOverrides::default()
    })
    .expect("load config");
    let mut controller = controller_for_config(&config);
    controller.open_provider_picker();

    let entry: neo_ai::catalog::CatalogEntry = serde_json::from_value(serde_json::json!({
        "id": "openai",
        "name": "OpenAI",
        "api": "https://api.openai.test/v1",
        "type": "openai_response",
        "models": {
            "gpt-new": {
                "id": "gpt-new",
                "name": "GPT New",
                "tool_call": true,
                "reasoning": false
            }
        }
    }))
    .expect("catalog entry");
    let catalog = BTreeMap::from([("openai".to_owned(), entry)]);
    let handle = tokio::spawn(async move { Ok(catalog) });
    while !handle.is_finished() {
        tokio::task::yield_now().await;
    }
    controller.pending_catalog_fetch = Some(PendingCatalogFetch {
        source: CatalogFetchSource::Known,
        handle,
        completion: CatalogFetchCompletion::Refresh(PendingCatalogRefresh {
            provider_id: "openai".to_owned(),
            config_path: config_path.clone(),
        }),
    });
    controller
        .tui
        .chrome_mut()
        .set_custom_working_label(Some("Refreshing provider openai...".to_owned()));

    assert!(controller.poll_pending_catalog_fetch().await);

    let written = fs::read_to_string(&config_path).expect("read refreshed config");
    assert!(written.contains("api_key = \"keep-me\""), "{written}");
    assert!(written.contains("[models.\"openai/gpt-new\"]"));
    assert!(!written.contains("[models.\"openai/old\"]"));
    assert_eq!(
        controller
            .local_config
            .as_ref()
            .expect("reloaded config")
            .default_model,
        "openai/gpt-new"
    );
    assert_eq!(
        controller
            .active_model
            .as_ref()
            .map(|model| model.alias.as_str()),
        Some("openai/gpt-new")
    );
    assert_eq!(controller.chrome().model_label(), "openai/gpt-new");
    assert_eq!(
        controller.current_reasoning,
        neo_ai::ReasoningSelection::Off
    );
    assert!(transcript_has_status(
        &controller,
        "refreshed provider 'openai' with 1 model"
    ));
    assert!(controller.chrome().working_label().is_none());
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::ProviderManager(_))
    ));

    let (_hold_sender, receiver) = oneshot::channel::<()>();
    controller.pending_catalog_fetch = Some(PendingCatalogFetch {
        source: CatalogFetchSource::Known,
        handle: tokio::spawn(async move {
            let _ = receiver.await;
            Ok(BTreeMap::new())
        }),
        completion: CatalogFetchCompletion::Browse,
    });
    controller.start_provider_model_refresh("openai".to_owned());
    assert!(matches!(
        controller
            .pending_catalog_fetch
            .as_ref()
            .map(|pending| &pending.completion),
        Some(CatalogFetchCompletion::Browse)
    ));
    assert!(transcript_has_status(
        &controller,
        "Provider refresh is already running"
    ));
    controller.abort_pending_catalog_fetch();
}

#[tokio::test]
async fn pending_mcp_probe_is_not_reported_as_connected_with_zero_tools() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let project_dir = test_workspace_root();
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));
    controller.pending_mcp_probe = Some(PendingMcpProbe {
        server_id: "linear".to_owned(),
        handle: tokio::spawn(async {
            Ok(neo_agent_core::McpServerSnapshot {
                id: "linear".to_owned(),
                transport: "http".to_owned(),
                status: neo_agent_core::McpServerStatus::Pending,
                tool_count: 0,
                tool_names: Vec::new(),
                resource_count: None,
                error: None,
                reconnect_attempt: 0,
                next_retry_ms: None,
            })
        }),
    });
    while !controller
        .pending_mcp_probe
        .as_ref()
        .expect("pending probe")
        .handle
        .is_finished()
    {
        tokio::task::yield_now().await;
    }

    assert!(controller.poll_pending_mcp_probe().await);
    assert!(transcript_has_status(
        &controller,
        "MCP server \"linear\" still connecting (http)"
    ));
    assert!(!transcript_has_status(&controller, "connected (0 tools)"));
}

#[tokio::test]
async fn controller_for_config_applies_tui_keybinding_overrides() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions");
    let mut config = test_config(temp.path(), sessions_dir);
    config
        .tui
        .keybindings
        .insert("tui.command.open".to_owned(), vec!["ctrl+g".to_owned()]);
    let mut controller = controller_for_config(&config);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+g").expect("valid key")))
        .await
        .expect("configured keybinding runs");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::CommandPalette(_))
    ));
}

#[test]
fn verbose_startup_mentions_local_keybinding_overrides() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config
        .tui
        .keybindings
        .insert("tui.input.submit".to_owned(), vec!["ctrl+j".to_owned()]);

    let mut controller = controller_for_config(&config);
    controller.apply_startup_options(
        &config,
        InteractiveOptions {
            verbose_startup: true,
        },
    );
    assert!(transcript_has_status(
        &controller,
        "keybindings: 1 override"
    ));
}

#[test]
fn model_picker_catalog_for_config_applies_cli_models_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.model_scope = vec!["sonnet".to_owned()];

    let catalog = model_picker_catalog_for_config(&config);

    assert_eq!(catalog.error, None);
    assert!(!catalog.items.is_empty());
    assert!(
        catalog
            .items
            .iter()
            .all(|item| item.value.contains("sonnet"))
    );
    assert!(
        catalog
            .items
            .iter()
            .all(|item| !item.value.contains("openai/gpt-4.1"))
    );
}

#[test]
fn controller_for_config_exposes_default_model_context_window() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));

    let controller = controller_for_config(&config);

    assert_eq!(
        controller.chrome().context_window(),
        Some(ContextWindow::new(1_047_576))
    );
}

#[tokio::test]
async fn configured_reasoning_selections_reach_interactive_turn_unchanged() {
    let cases = [
        (
            "low",
            neo_ai::ReasoningSelection::Effort {
                effort: neo_ai::ReasoningEffort::low(),
            },
        ),
        (
            "max",
            neo_ai::ReasoningSelection::Effort {
                effort: neo_ai::ReasoningEffort::max(),
            },
        ),
        (
            "budget",
            neo_ai::ReasoningSelection::BudgetTokens {
                budget_tokens: 12_000,
            },
        ),
    ];

    for (name, expected) in cases {
        let actual = capture_configured_interactive_turn_reasoning(expected.clone()).await;
        assert_eq!(actual, expected, "case {name}");
    }
}

#[test]
fn model_selection_with_thinking_preserves_current_structured_reasoning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let expected = neo_ai::ReasoningSelection::BudgetTokens {
        budget_tokens: 12_000,
    };
    config.runtime.reasoning = expected.clone();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config);
    controller.set_current_reasoning(expected.clone());

    controller.apply_model_selection(&neo_tui::dialogs::ModelSelection {
        alias: "openai/gpt-4.1".to_owned(),
        thinking: true,
        reasoning: expected.clone(),
    });

    assert_eq!(
        controller
            .local_config
            .as_ref()
            .expect("local config")
            .runtime
            .reasoning,
        expected
    );
}

#[test]
fn model_selection_persists_reasoning_and_provider_across_reload() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let config_path = config.config_path.clone();
    config.runtime.reasoning = neo_ai::ReasoningSelection::Off;
    let expected_reasoning = neo_ai::ReasoningSelection::Effort {
        effort: neo_ai::ReasoningEffort::medium(),
    };
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config);
    controller.set_current_reasoning(neo_ai::ReasoningSelection::Off);

    controller.apply_model_selection(&neo_tui::dialogs::ModelSelection {
        alias: "anthropic/claude-sonnet-4".to_owned(),
        thinking: true,
        reasoning: expected_reasoning.clone(),
    });

    assert_eq!(
        controller
            .local_config
            .as_ref()
            .expect("local config")
            .runtime
            .reasoning,
        expected_reasoning
    );

    let reloaded = crate::config::AppConfig::load(crate::config::ConfigOverrides {
        config_path: Some(config_path),
        project_dir: Some(temp.path().to_path_buf()),
        ..crate::config::ConfigOverrides::default()
    })
    .expect("reload config");
    assert_eq!(reloaded.runtime.reasoning, expected_reasoning);
    assert_eq!(reloaded.default_model, "anthropic/claude-sonnet-4");
    assert_eq!(reloaded.default_provider, "anthropic");
}

#[tokio::test]
async fn refresh_config_preserves_live_task_and_multi_agent_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, "").expect("write config");
    let mut config = test_config(temp.path(), temp.path().join("sessions"));
    config.config_path = config_path;
    *config.workspace_policy.write().expect("workspace policy") = Some(
        neo_agent_core::WorkspaceAccessPolicy::new(temp.path()).expect("workspace access policy"),
    );
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    let agent = config
        .multi_agent
        .start_foreground_delegate_for_test("preserve delegate");
    let live_permission_mode = Arc::clone(&config.live_permission_mode);
    let workspace_policy = Arc::clone(&config.workspace_policy);
    let original_shell = config.runtime.shell;
    let original_runtime_root = config.runtime.shell_runtime.runtime_root().to_path_buf();
    fs::write(
        &config.config_path,
        "[runtime.shell]\nmax_active_commands = 3\n",
    )
    .expect("write refreshed config");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.live_permission_mode = Arc::clone(&config.live_permission_mode);
    controller.workspace_policy = Arc::clone(&config.workspace_policy);
    controller.local_config = Some(config);
    controller.set_permission_mode(PermissionMode::Yolo);

    controller.refresh_config();

    let reloaded = controller.local_config.as_ref().expect("reloaded config");
    assert_eq!(reloaded.background_tasks.list(false, 10).await.len(), 1);
    assert!(reloaded.multi_agent.snapshot(&agent.id).is_some());
    assert!(Arc::ptr_eq(
        &reloaded.live_permission_mode,
        &live_permission_mode
    ));
    assert!(Arc::ptr_eq(&reloaded.workspace_policy, &workspace_policy));
    assert_eq!(reloaded.permission_mode, PermissionMode::Yolo);
    // Live config refresh preserves the running ShellRuntime; shell-limit file
    // changes take effect on the next Neo process start.
    assert_eq!(reloaded.runtime.shell, original_shell);
    assert_eq!(
        reloaded.runtime.shell_runtime.runtime_root(),
        original_runtime_root.as_path()
    );
    assert_eq!(
        *reloaded
            .live_permission_mode
            .read()
            .expect("live permission mode"),
        PermissionMode::Yolo
    );
    assert!(reloaded.project_trusted);
    assert_eq!(
        reloaded.project_trust,
        crate::trust::ProjectTrustState::NotRequired
    );
}

#[test]
fn model_selection_without_thinking_sets_reasoning_off() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.runtime.reasoning = neo_ai::ReasoningSelection::Effort {
        effort: neo_ai::ReasoningEffort::max(),
    };
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config);
    controller.set_current_reasoning(neo_ai::ReasoningSelection::Effort {
        effort: neo_ai::ReasoningEffort::max(),
    });

    controller.apply_model_selection(&neo_tui::dialogs::ModelSelection {
        alias: "openai/gpt-4.1".to_owned(),
        thinking: false,
        reasoning: neo_ai::ReasoningSelection::Off,
    });

    assert_eq!(
        controller
            .local_config
            .as_ref()
            .expect("local config")
            .runtime
            .reasoning,
        neo_ai::ReasoningSelection::Off
    );
}

#[test]
fn controller_for_config_loads_builtin_skills() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));

    let controller = controller_for_config(&config);

    let skill_store = controller
        .skill_store
        .as_ref()
        .expect("skill store should load");
    assert!(
        skill_store.get("sub-skill").is_some(),
        "builtin sub-skill skill should be loaded"
    );
    assert!(
        skill_store.get("self-evo").is_some(),
        "builtin self-evo skill should be loaded"
    );
}

#[test]
fn model_picker_items_include_parseable_context_window() {
    let item = model_to_picker_item(&neo_ai::ModelSpec {
        provider: neo_ai::ProviderId("test".to_owned()),
        model: "huge".to_owned(),
        api: neo_ai::ApiKind::OpenAiResponse,
        capabilities: neo_ai::ModelCapabilities::tool_chat().with_max_context_tokens(128_000),
    });

    assert!(
        item.description
            .as_deref()
            .is_some_and(|text| text.contains("ctx 128000"))
    );
    assert_eq!(context_window_from_picker_item(&item), Some(128_000));
}

#[tokio::test]
async fn add_provider_picker_includes_custom_endpoint() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(test_config(temp.path(), sessions_dir));

    controller.open_add_provider_picker();

    let visible = controller
        .tui
        .chrome()
        .focused_overlay_lines(80)
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("Known third-party provider"), "{visible}");
    assert!(visible.contains("Custom endpoint"), "{visible}");
    assert!(visible.contains("Custom registry (api.json)"), "{visible}");
    let known = visible
        .find("Known third-party provider")
        .expect("known provider row");
    let custom_endpoint = visible
        .find("Custom endpoint")
        .expect("custom endpoint row");
    let custom_registry = visible
        .find("Custom registry (api.json)")
        .expect("custom registry row");
    assert!(known < custom_endpoint, "{visible}");
    assert!(custom_endpoint < custom_registry, "{visible}");
}

#[tokio::test]
async fn add_provider_custom_endpoint_choice_opens_wizard() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(test_config(temp.path(), sessions_dir));

    controller.open_add_provider_picker();
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("select custom endpoint row");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("open custom endpoint wizard");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::CustomEndpointWizard(_))
    ));
    let visible = controller
        .chrome()
        .focused_overlay_lines(80)
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("Custom Endpoint 1/4"), "{visible}");
}

#[test]
fn instruction_registry_cache_is_scoped_by_session_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);

    controller.set_active_session_id(SESSION_A.to_owned());
    let session_a = controller
        .instruction_registry_for_turn()
        .expect("session A registry")
        .expect("configured controller registry");
    let session_a_again = controller
        .instruction_registry_for_turn()
        .expect("session A registry")
        .expect("configured controller registry");
    assert!(Arc::ptr_eq(&session_a, &session_a_again));

    controller.set_active_session_id(SESSION_B.to_owned());
    let session_b = controller
        .instruction_registry_for_turn()
        .expect("session B registry")
        .expect("configured controller registry");
    assert!(!Arc::ptr_eq(&session_a, &session_b));

    controller.active_session_id = None;
    let new_session = controller
        .instruction_registry_for_turn()
        .expect("new session registry")
        .expect("configured controller registry");
    let another_new_session = controller
        .instruction_registry_for_turn()
        .expect("another new session registry")
        .expect("configured controller registry");
    assert!(!Arc::ptr_eq(&new_session, &another_new_session));

    controller.set_active_session_id(SESSION_A.to_owned());
    let session_a_after_switch = controller
        .instruction_registry_for_turn()
        .expect("session A registry after switch")
        .expect("configured controller registry");
    assert!(Arc::ptr_eq(&session_a, &session_a_after_switch));
}

/// Regression: the turn driver must receive the controller's *live*
/// `local_config` (via `TurnRequest.base_config`), not the stale snapshot
/// captured at construction. Without this, a provider added at runtime via
/// `/provider` is written to disk but the next turn fails with
/// "unknown model" because the stale registry is used.
#[tokio::test]
async fn turn_request_carries_live_local_config() {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_config = std::sync::Arc::clone(&captured);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_config = std::sync::Arc::clone(&captured_config);
            async move {
                *captured_config.lock().expect("capture config") = request.base_config;
                Ok(vec![
                    AgentEvent::MessageStarted {
                        phase: neo_ai::MessagePhase::Unknown,
                        turn: 1,
                        id: "m".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: neo_agent_core::StopReason::EndTurn,
                    },
                ])
            }
        },
    );

    // Simulate a runtime config change (e.g. provider added via `/provider`)
    // by setting local_config AFTER the controller was built.
    let live_config = test_config_with_models(
        &test_workspace_root(),
        test_workspace_root().join(".neo/sessions"),
        BTreeMap::from([(
            "minimax-cn-coding-plan/MiniMax-M3".to_owned(),
            ModelConfig {
                provider: "minimax-cn-coding-plan".to_owned(),
                model: "MiniMax-M3".to_owned(),
                ..ModelConfig::default()
            },
        )]),
    );
    controller.local_config = Some(live_config);

    controller.type_text("hello");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let captured = captured.lock().expect("captured").take();
    let config = captured.expect("base_config was forwarded to the driver");
    assert_eq!(config.default_provider, "openai");
    assert!(
        config
            .models
            .contains_key("minimax-cn-coding-plan/MiniMax-M3")
    );
}

#[tokio::test]
async fn turn_request_carries_workspace_policy() {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_policy = std::sync::Arc::clone(&captured);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_policy = std::sync::Arc::clone(&captured_policy);
            async move {
                *captured_policy.lock().expect("capture policy") =
                    Some(std::sync::Arc::clone(&request.workspace_policy));
                Ok(vec![
                    AgentEvent::MessageStarted {
                        phase: neo_ai::MessagePhase::Unknown,
                        turn: 1,
                        id: "m".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: neo_agent_core::StopReason::EndTurn,
                    },
                ])
            }
        },
    );

    let expected_policy = std::sync::Arc::clone(&controller.workspace_policy);
    controller.type_text("hello");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let captured = captured.lock().expect("captured").take();
    let captured_policy = captured.expect("workspace policy was forwarded to the driver");
    assert!(std::sync::Arc::ptr_eq(&captured_policy, &expected_policy));
}

#[test]
fn configured_model_picker_preserves_unqualified_alias() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config_with_models(
        temp.path(),
        temp.path().join(".neo/sessions"),
        BTreeMap::from([(
            "fast".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                max_context_tokens: Some(1_000_000),
                ..ModelConfig::default()
            },
        )]),
    );

    let items = model_picker_items_from_config(&config);
    assert_eq!(items[0].value, "fast");
    let selected =
        SelectedModel::from_alias("fast", Some(&config), &items).expect("alias resolves");
    assert_eq!(selected.alias, "fast");
    assert_eq!(selected.provider, "openai");
    assert_eq!(selected.model, "gpt-4.1");
    assert_eq!(selected.max_context_tokens, Some(1_000_000));
}

#[tokio::test]
async fn controller_loads_workspace_prompt_history_on_startup() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    store
        .append(Some("prior-session"), "earlier prompt")
        .expect("seed earlier");
    store
        .append(Some("prior-session"), "latest prompt")
        .expect("seed latest");

    let mut controller = controller_with_history_store(store);

    // Empty composer: first Up recalls the most recent persisted prompt.
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls latest persisted prompt");
    assert_eq!(controller.chrome().prompt().text, "latest prompt");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls older persisted prompt");
    assert_eq!(controller.chrome().prompt().text, "earlier prompt");
}

#[tokio::test]
async fn refresh_config_preserves_session_theme_override() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    write_test_theme(&project_dir, "solarized.json", "Solarized", "#ff0000");
    write_test_theme(&project_dir, "gruvbox.json", "Gruvbox", "#00ff00");

    let mut controller = theme_controller_with_project(&project_dir);
    controller.type_text("/theme solarized.json");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("apply session override");

    controller.refresh_config();
    assert_eq!(
        controller.chrome().theme().brand,
        neo_tui::primitive::Color::Rgb(255, 0, 0),
        "an unrelated config refresh must not overwrite the session override"
    );
    assert!(
        controller.session_theme_override.is_some(),
        "the override marker must survive a config refresh"
    );
}
