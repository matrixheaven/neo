//! Command-palette and picker behavior (split from `input.rs`).

use std::{collections::BTreeMap, fs};

use neo_agent_core::{AgentEvent, Content};
use neo_tui::{
    input::{InputEvent, KeybindingAction},
    shell::{ChromeMode, OverlayKind},
    transcript::TranscriptEntry,
};

use super::super::snapshot::compose_tui_frame;
use super::super::*;
use super::*;
use crate::config::ModelConfig;

#[tokio::test]
async fn slash_help_opens_help_panel_overlay() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured = std::sync::Arc::clone(&captured);
            async move {
                captured.lock().expect("recorded requests").push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    controller.type_text("/help");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash help command runs locally");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::HelpPanel(_))
    ));
    assert!(controller.chrome().prompt().text.is_empty());
    assert!(requests.lock().expect("recorded requests").is_empty());

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageDown))
        .await
        .expect("scroll help panel");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageUp))
        .await
        .expect("scroll help panel back up");

    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("help · Esc / Enter / q close"),
        "{snapshot}"
    );
    assert!(snapshot.contains("/help"), "{snapshot}");
    assert!(snapshot.contains("/ask"), "{snapshot}");
}

#[tokio::test]
async fn slash_help_panel_includes_dynamic_skill_commands() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.skill_store = Some(skill_store_with_refactor_skill());

    controller.type_text("/help");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash help command runs locally");

    for _ in 0..8 {
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageDown))
            .await
            .expect("scroll help panel");
    }

    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("/skill:refactor"), "{snapshot}");
    assert!(
        snapshot.contains("Refactor with project conventions"),
        "{snapshot}"
    );
}

#[tokio::test]
async fn command_palette_inserts_project_prompt_template_command() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompts_dir = temp.path().join(".neo/prompts");
    fs::create_dir_all(&prompts_dir).expect("create prompts");
    fs::write(
        prompts_dir.join("review.md"),
        "---\ndescription: Review a target\nargument-hint: <path>\n---\nReview $1.\n",
    )
    .expect("write review prompt");

    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
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
    );
    controller.completion_root = temp.path().to_path_buf();

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..32 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "prompt-template.review" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to review command");
    }
    assert_eq!(
        controller
            .chrome()
            .selected_command()
            .expect("review command")
            .id,
        "prompt-template.review"
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("prompt template command inserts invocation");

    assert_eq!(controller.chrome().prompt().text, "/review ");
    assert_eq!(controller.chrome().prompt().cursor, 8);
    assert!(controller.chrome().focused_overlay().is_none());

    controller.type_text("src/lib.rs");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("prompt template command submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("prompt template turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("Review src/lib.rs.")]
    );
}

#[tokio::test]
async fn event_loop_model_picker_action_opens_model_picker() {
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
                "openai/gpt-4.1",
                "openai/gpt-4.1",
                Some("test model"),
            )],
        },
        empty_session_loader,
    );

    controller.local_config = Some(test_config_with_models(
        &test_workspace_root(),
        test_workspace_root().join(".neo/sessions"),
        BTreeMap::from([(
            "openai/gpt-4.1".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                display_name: Some("test model".into()),
                ..ModelConfig::default()
            },
        )]),
    ));
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::ModelPickerOpen))
        .await
        .expect("model picker action opens model picker");

    assert!(
        controller.chrome().tabbed_model_selector_result().is_some()
            || controller.chrome().focused_overlay().is_some()
    );
}

#[tokio::test]
async fn command_palette_add_workspace_opens_workspace_manager_overlay() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let project_dir = test_workspace_root();
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..32 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "add-workspace" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to add workspace command");
    }
    let selected = controller
        .chrome()
        .selected_command()
        .expect("add workspace command");
    assert_eq!(selected.id, "add-workspace");
    assert_eq!(selected.label, "Open workspace access");
    assert_eq!(
        selected.description.as_deref(),
        Some("Manage additional workspace directories")
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("add workspace command runs");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::WorkspaceManager(_))
    ));
}

#[tokio::test]
async fn command_palette_exports_active_session_to_html() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir.clone());
    let bucket_dir = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    write_main_wire(
        &bucket_dir,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello <script>alert(1)</script>\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"use **bold** safely\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );

    let config = test_config(temp.path(), sessions_dir.clone());
    let mut controller = controller_for_config(&config);
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
        .await
        .expect("session picker opens");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("session loads");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..32 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "session.exportHtml" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to export command");
    }
    assert_eq!(
        controller
            .chrome()
            .selected_command()
            .expect("export command")
            .id,
        "session.exportHtml"
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("export command runs");

    let export_path = neo_agent_core::session::main_agent_wire_path(&bucket_dir.join(SESSION_A))
        .with_extension("html");
    let html = fs::read_to_string(&export_path).expect("read exported html");
    assert!(html.contains(&format!("<title>neo session {SESSION_A}</title>")));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Status { text, .. }
                if text.contains(&format!("Exported session {SESSION_A} to"))
                    && text.contains(&export_path.display().to_string())
        )
    }));
}

#[tokio::test]
async fn slash_picker_commands_do_not_enter_streaming_mode() {
    for command in ["/model", "/provider"] {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text(command);
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .unwrap_or_else(|e| panic!("{command} submit failed: {e}"));
        assert_eq!(
            controller.chrome().mode(),
            ChromeMode::Editing,
            "{command} should keep editing mode"
        );
        assert!(
            controller.chrome().prompt().text.is_empty(),
            "{command} should leave the prompt empty"
        );
    }
}

#[tokio::test]
async fn command_palette_export_html_without_active_session_shows_local_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");
    let config = test_config(temp.path(), sessions_dir);
    let mut controller = controller_for_config(&config);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..32 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "session.exportHtml" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to export command");
    }

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("export command handles missing session locally");

    assert!(transcript_has_status(
        &controller,
        "No active session to export"
    ));
}

#[tokio::test]
async fn slash_model_opens_picker_when_no_config_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.config_file_exists = false;
    config.models.clear();
    config.providers.clear();
    let mut controller = controller_for_config(&config);

    controller.type_text("/model");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/model submits");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::TabbedModelSelector(_))
    ));
}

#[tokio::test]
async fn slash_provider_opens_picker_when_no_config_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.config_file_exists = false;
    config.providers.clear();
    config.models.clear();
    let mut controller = controller_for_config(&config);

    controller.type_text("/provider");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/provider submits");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::ProviderManager(_))
    ));
}

#[tokio::test]
async fn command_palette_new_session_resets_to_fresh_session() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.active_session_id = Some(SESSION_A.to_owned());
    controller
        .tui
        .chrome_mut()
        .set_session_label(SESSION_A.to_owned());
    controller
        .transcript_mut()
        .push_user_message("old session content");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..64 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "session.new" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to next command");
    }
    assert_eq!(
        controller
            .chrome()
            .selected_command()
            .expect("new session command")
            .id,
        "session.new"
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("new session command runs");

    assert_eq!(controller.active_session_id(), None);
    assert_eq!(controller.chrome().session_label(), "new");
    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("Started fresh session"));
    assert!(!snapshot.contains("old session content"));
}

#[tokio::test]
async fn command_palette_new_session_works_before_session_materialization() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    assert_eq!(controller.active_session_id(), None);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..64 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "session.new" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to next command");
    }
    assert_eq!(
        controller
            .chrome()
            .selected_command()
            .expect("new session command")
            .id,
        "session.new"
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("new session command runs");

    assert_eq!(controller.active_session_id(), None);
}

#[tokio::test]
async fn slash_mcp_opens_mcp_manager_overlay() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let project_dir = test_workspace_root();
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash command handled");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("/mcp should open an overlay");
    assert!(
        matches!(overlay.kind, OverlayKind::McpManager(_)),
        "/mcp should open the MCP manager overlay, got {:?}",
        overlay.kind
    );
}

#[tokio::test]
async fn slash_add_workspace_opens_workspace_manager_overlay() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let project_dir = test_workspace_root();
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));
    controller.type_text("/add-workspace");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash command handled");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("/add-workspace should open an overlay");
    assert!(
        matches!(overlay.kind, OverlayKind::WorkspaceManager(_)),
        "/add-workspace should open the workspace manager overlay, got {:?}",
        overlay.kind
    );
}

#[tokio::test]
async fn slash_mcp_renders_mcp_manager_overlay() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let project_dir = test_workspace_root();
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash command handled");
    let mut transcript = controller.tui.transcript().clone();
    let lines =
        compose_tui_frame(controller.chrome(), &mut transcript, 80, 24).expect("frame composes");
    let joined = lines.join("\n");
    assert!(
        joined.contains("MCP Servers"),
        "rendered frame should contain MCP manager title: {joined}"
    );
}
