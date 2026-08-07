//! Session MCP-manager behavior (split from `sessions.rs`).

use std::fs;

use super::super::snapshot::compose_tui_frame;
use super::super::*;
use super::*;
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    shell::OverlayKind,
};

#[tokio::test]
async fn mcp_manager_auth_action_shows_status_on_oauth_failure() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let temp = tempfile::tempdir().expect("temp dir");
    let project_dir = temp.path().to_path_buf();
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    config.mcp.servers.push(crate::config::McpServerConfig {
        id: "example".to_owned(),
        enabled: true,
        transport: crate::config::McpTransport::Http,
        command: None,
        url: Some("https://example.com/mcp".into()),
        args: Vec::new(),
        env: std::collections::BTreeMap::new(),
        headers: std::collections::BTreeMap::new(),
        cwd: None,
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        startup_timeout_ms: None,
        tool_timeout_ms: None,
    });
    controller.local_config = Some(config);
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open /mcp");
    controller
        .handle_input_event(InputEvent::Insert('O'))
        .await
        .expect("auth key");
    assert!(transcript_has_status(&controller, "OAuth flow failed"));
}

#[tokio::test]
async fn mcp_manager_auth_action_ignored_for_stdio_server() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let temp = tempfile::tempdir().expect("temp dir");
    let project_dir = temp.path().to_path_buf();
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    config.mcp.servers.push(crate::config::McpServerConfig {
        id: "fs".to_owned(),
        enabled: true,
        transport: crate::config::McpTransport::Stdio,
        command: Some("mcp-server".into()),
        url: None,
        args: Vec::new(),
        env: std::collections::BTreeMap::new(),
        headers: std::collections::BTreeMap::new(),
        cwd: None,
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        startup_timeout_ms: None,
        tool_timeout_ms: None,
    });
    controller.local_config = Some(config);
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open /mcp");
    controller
        .handle_input_event(InputEvent::Insert('O'))
        .await
        .expect("auth key");
    assert!(!transcript_has_status(
        &controller,
        "No OAuth provider configured"
    ));
}

#[tokio::test]
async fn mcp_add_transport_opens_form() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let project_dir = test_workspace_root();
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    // Open the MCP manager.
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash command handled");
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::McpManager(_))
        ),
        "MCP manager should be focused"
    );

    // Press 'A' to add a server.
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("add key handled");
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::ChoicePicker(_))
        ),
        "transport choice picker should be focused"
    );

    // Press Enter to select the first transport (real TUI sends Key("enter")).
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("select handled");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("selecting a transport should open the next overlay");
    assert!(
        matches!(overlay.kind, OverlayKind::McpAddForm(_)),
        "expected MCP add form overlay after selecting transport, got {:?}",
        overlay.kind
    );

    // The form must actually be rendered in a single composed frame,
    // and the title should reflect the selected transport so the user
    // knows which transport-specific params are being collected.
    let mut transcript = controller.tui.transcript().clone();
    let lines =
        compose_tui_frame(controller.chrome(), &mut transcript, 80, 24).expect("frame composes");
    let joined = lines.join("\n");
    assert!(
        joined.contains("Add Local stdio MCP Server"),
        "rendered frame should contain contextual form title: {joined}"
    );
    assert!(
        joined.contains("▸ Name:")
            && joined.contains("Program:")
            && joined.contains("Arguments (JSON string per line):")
            && joined.contains("Env:"),
        "rendered frame should show stdio fields: {joined}"
    );
}

#[tokio::test]
async fn mcp_add_form_stdio_submits_to_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    // Open manager, start add, select stdio.
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("start add");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("select stdio");
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::McpAddForm(_))
        ),
        "form should be focused"
    );

    // Fill Name, Program, Arguments, and Env.
    controller
        .handle_input_event(InputEvent::Paste("fs".to_owned()))
        .await
        .expect("type name");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to command");
    controller
        .handle_input_event(InputEvent::Paste("npx".to_owned()))
        .await
        .expect("type program");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to arguments");
    controller
        .handle_input_event(InputEvent::Paste(
            "\"-y\"\n\"@server/filesystem\"\n\"/repo\"".to_owned(),
        ))
        .await
        .expect("type arguments");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to env");
    controller
        .handle_input_event(InputEvent::Paste("KEY=value".to_owned()))
        .await
        .expect("type env");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit form");

    // The MCP manager overlay should be reopened after a successful add.
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::McpManager(_))
        ),
        "MCP manager should be reopened after submit"
    );

    let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
        .expect("read saved config");
    let servers = config.mcp.expect("mcp section").servers;
    assert_eq!(servers.len(), 1, "expected one saved MCP server");
    assert_eq!(servers[0].id, "fs");
    assert_eq!(servers[0].transport, crate::config::McpTransport::Stdio);
    assert_eq!(
        servers[0].command,
        Some("npx".into()),
        "command is parsed into program"
    );
    assert_eq!(
        servers[0].args,
        vec![
            "-y".to_owned(),
            "@server/filesystem".to_owned(),
            "/repo".to_owned()
        ]
    );
    assert_eq!(
        servers[0].env.get("KEY"),
        Some(&"value".to_owned()),
        "env key is parsed"
    );
    assert!(servers[0].enabled);
}

#[tokio::test]
async fn mcp_add_form_http_submits_to_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    // Open manager, start add, select HTTP (second item -> one Down + Enter).
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("start add");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to HTTP");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("select http");

    // Fill Name, URL, Bearer Token, and Headers.
    controller
        .handle_input_event(InputEvent::Paste("linear".to_owned()))
        .await
        .expect("type name");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to url");
    controller
        .handle_input_event(InputEvent::Paste("https://example.invalid/mcp".to_owned()))
        .await
        .expect("type url");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to token");
    controller
        .handle_input_event(InputEvent::Paste("secret".to_owned()))
        .await
        .expect("type token");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to headers");
    controller
        .handle_input_event(InputEvent::Paste("X-Custom=foo".to_owned()))
        .await
        .expect("type headers");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit form");

    let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
        .expect("read saved config");
    let servers = config.mcp.expect("mcp section").servers;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].id, "linear");
    assert_eq!(servers[0].transport, crate::config::McpTransport::Http);
    assert_eq!(servers[0].url, Some("https://example.invalid/mcp".into()));
    assert_eq!(
        servers[0].headers.get("Authorization"),
        Some(&"Bearer secret".to_owned()),
        "bearer token is prepended as Authorization header"
    );
    assert_eq!(servers[0].headers.get("X-Custom"), Some(&"foo".to_owned()));
}

#[tokio::test]
async fn mcp_add_form_sse_submits_to_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    // Open manager, start add, select SSE (third item -> two Down + Enter).
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("start add");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to HTTP");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to SSE");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("select sse");

    // Fill Name and URL only; leave optional fields empty.
    controller
        .handle_input_event(InputEvent::Paste("events".to_owned()))
        .await
        .expect("type name");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to url");
    controller
        .handle_input_event(InputEvent::Paste("https://events.invalid/sse".to_owned()))
        .await
        .expect("type url");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit form");

    let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
        .expect("read saved config");
    let servers = config.mcp.expect("mcp section").servers;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].id, "events");
    assert_eq!(servers[0].transport, crate::config::McpTransport::Sse);
    assert_eq!(servers[0].url, Some("https://events.invalid/sse".into()));
    assert!(servers[0].headers.is_empty());
}

#[tokio::test]
async fn mcp_add_form_cancel_returns_to_manager() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("start add");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("select stdio");
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::McpAddForm(_))
        ),
        "form should be focused"
    );

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel form");

    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::McpManager(_))
        ),
        "MCP manager should be reopened after cancel"
    );

    let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
        .expect("read config");
    assert!(
        config.mcp.is_none() || config.mcp.unwrap().servers.is_empty(),
        "no server should be saved on cancel"
    );
}

#[tokio::test]
async fn mcp_startup_queues_prompt_then_starts_it_when_settled() {
    let run_turn: TurnDriver = Arc::new(|_request, channels| {
        Box::pin(async move {
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
    controller.tui.chrome_mut().set_mcp_startup_active(true);

    controller.type_text("queued during MCP startup");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("prompt should queue while MCP starts");

    assert!(
        controller.active_turn.is_none(),
        "MCP startup should defer the prompt instead of creating a turn"
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued during MCP startup"]
    );

    controller.tui.chrome_mut().set_mcp_startup_active(false);
    controller
        .start_next_mcp_startup_prompt()
        .expect("queued prompt should start after MCP settles");

    assert!(
        controller.active_turn.is_some(),
        "settled MCP startup should promote the queued prompt"
    );
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
    );
    controller
        .cancel_active_turn()
        .await
        .expect("cleanup active turn");
}
