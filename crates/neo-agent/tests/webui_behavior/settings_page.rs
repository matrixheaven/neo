//! Settings page product boundary: the snapshot carries config overview
//! without any credential material, and default-model / MCP mutations persist
//! to config.toml and are reflected in the returned snapshot.

use serde_json::{Value, json};

use super::http as web_http;
use super::provider::{Step, openai_response_sse};
use super::session_env::{create_session, start_env_with_config, wait_for_phase};

const EXTRA_CONFIG: &str = r#"
[[mcp.servers]]
id = "fixture-mcp"
enabled = false
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem"]
"#;

#[tokio::test]
async fn settings_snapshot_hides_credentials_and_persists_updates() {
    let (test_env, provider) = start_env_with_config(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_response_sse("resp-1", "answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
        EXTRA_CONFIG,
    )
    .await;
    let _provider = provider;
    let (session_id, _turn_id, _) = create_session(&test_env, "settings message").await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;
    let port = test_env.webui.port;
    let cookie = test_env.cookie.clone();

    // Snapshot: full overview, never a credential.
    let response = web_http::get(port, &cookie, "/api/settings").await;
    assert_eq!(response.status, 200, "settings: {}", response.body);
    assert!(
        !response.body.contains("test-key"),
        "credential values must never cross the web boundary"
    );
    let parsed: Value = serde_json::from_str(&response.body).expect("settings json");
    assert_eq!(
        parsed["default_model"], "gpt-4.1",
        "bare alias from config.toml"
    );
    assert_eq!(parsed["permission_mode"], "ask");
    let mock = parsed["providers"]
        .as_array()
        .expect("providers")
        .iter()
        .find(|provider| provider["id"] == "mock")
        .expect("mock provider");
    assert_eq!(
        mock["has_api_key"], true,
        "env-ref key counts as configured"
    );
    assert!(
        mock.get("api_key").is_none(),
        "snapshot never carries the key"
    );
    let mcp = parsed["mcp_servers"]
        .as_array()
        .expect("mcp servers")
        .iter()
        .find(|server| server["id"] == "fixture-mcp")
        .expect("fixture mcp");
    assert_eq!(
        mcp["enabled"], false,
        "fixture starts disabled so it never spawns"
    );
    assert_eq!(mcp["transport"], "stdio");

    // Default-model update persists and is reflected.
    let patch = web_http::patch_json(
        port,
        &cookie,
        "/api/settings/default-model",
        &json!({ "alias": "mock/gpt-4.1" }),
    )
    .await;
    assert_eq!(patch.status, 200, "default model: {}", patch.body);
    let parsed: Value = serde_json::from_str(&patch.body).expect("settings json");
    assert_eq!(parsed["default_model"], "mock/gpt-4.1");

    // MCP enable/disable persists and is reflected (no new session starts,
    // so flipping the flag never spawns the fixture process).
    let patch = web_http::patch_json(
        port,
        &cookie,
        "/api/settings/mcp/fixture-mcp",
        &json!({ "enabled": true }),
    )
    .await;
    assert_eq!(patch.status, 200, "mcp enable: {}", patch.body);
    let parsed: Value = serde_json::from_str(&patch.body).expect("settings json");
    assert_eq!(parsed["mcp_servers"][0]["enabled"], true);
    let config_text =
        std::fs::read_to_string(test_env._home.path().join("config.toml")).expect("config.toml");
    assert!(
        config_text.contains("enabled = true"),
        "mcp enabled state persisted to config.toml"
    );
    let patch = web_http::patch_json(
        port,
        &cookie,
        "/api/settings/mcp/fixture-mcp",
        &json!({ "enabled": false }),
    )
    .await;
    assert_eq!(patch.status, 200, "mcp disable: {}", patch.body);
    let config_text =
        std::fs::read_to_string(test_env._home.path().join("config.toml")).expect("config.toml");
    assert!(
        config_text.contains("enabled = false"),
        "mcp disabled state persisted to config.toml"
    );
    assert!(
        config_text.contains("api_key_env"),
        "config rewrite preserves the provider credential reference"
    );
}

#[tokio::test]
async fn settings_mutations_persist_and_hide_secrets() {
    let (test_env, provider) = start_env_with_config(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_response_sse("resp-1", "answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
        "",
    )
    .await;
    let _provider = provider;
    let (session_id, _turn_id, _) = create_session(&test_env, "settings message").await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;
    let port = test_env.webui.port;
    let cookie = test_env.cookie.clone();
    let config_path = test_env._home.path().join("config.toml");

    // Snapshot carries appearance + default_reasoning, never secrets.
    let response = web_http::get(port, &cookie, "/api/settings").await;
    assert_eq!(response.status, 200, "settings: {}", response.body);
    let parsed: Value = serde_json::from_str(&response.body).expect("settings json");
    assert_eq!(parsed["appearance"]["theme"], "system");
    assert_eq!(parsed["appearance"]["ui_font_size"], 14);
    assert!(
        parsed["default_reasoning"].is_object(),
        "default reasoning present"
    );

    // Permission mode persists and reflects.
    let patch = web_http::patch_json(
        port,
        &cookie,
        "/api/settings/permission-mode",
        &json!({ "mode": "auto" }),
    )
    .await;
    assert_eq!(patch.status, 200, "permission-mode: {}", patch.body);
    let parsed: Value = serde_json::from_str(&patch.body).expect("settings json");
    assert_eq!(parsed["permission_mode"], "auto");
    let config_text = std::fs::read_to_string(&config_path).expect("config.toml");
    assert!(
        config_text.contains("permission_mode = \"auto\""),
        "permission mode persisted to config.toml"
    );

    // Appearance persists and reflects.
    let patch = web_http::patch_json(
        port,
        &cookie,
        "/api/settings/appearance",
        &json!({
            "appearance": {
                "theme": "dark",
                "ui_font_size": 15,
                "code_font_size": 13,
                "code_theme": "auto",
                "show_line_numbers": false,
                "word_wrap": true
            }
        }),
    )
    .await;
    assert_eq!(patch.status, 200, "appearance: {}", patch.body);
    let parsed: Value = serde_json::from_str(&patch.body).expect("settings json");
    assert_eq!(parsed["appearance"]["theme"], "dark");
    assert_eq!(parsed["appearance"]["ui_font_size"], 15);
    let config_text = std::fs::read_to_string(&config_path).expect("config.toml");
    assert!(
        config_text.contains("[webui]"),
        "appearance persisted to config.toml"
    );

    // Provider persists and never echoes the key.
    let post = web_http::post_json(
        port,
        &cookie,
        "/api/settings/providers",
        &json!({
            "id": "custom",
            "display_name": "Custom",
            "provider_type": "openai",
            "base_url": "https://api.example.com/v1",
            "api_key_env": "CUSTOM_API_KEY"
        }),
    )
    .await;
    assert_eq!(post.status, 200, "add provider: {}", post.body);
    assert!(
        !post.body.contains("CUSTOM_API_KEY"),
        "provider env key never echoed"
    );
    let parsed: Value = serde_json::from_str(&post.body).expect("settings json");
    let custom = parsed["providers"]
        .as_array()
        .expect("providers")
        .iter()
        .find(|provider| provider["id"] == "custom")
        .expect("custom provider");
    assert_eq!(
        custom["has_api_key"], true,
        "env-ref key counts as configured"
    );

    // Model persists under the new provider.
    let post = web_http::post_json(
        port,
        &cookie,
        "/api/settings/models",
        &json!({
            "alias": "custom/model-x",
            "provider": "custom",
            "model": "model-x",
            "max_context_tokens": 100000,
            "max_output_tokens": 128000,
            "capabilities": ["streaming", "tools"],
            "reasoning": { "type": "none" }
        }),
    )
    .await;
    assert_eq!(post.status, 200, "add model: {}", post.body);
    let parsed: Value = serde_json::from_str(&post.body).expect("settings json");
    assert!(
        parsed["models"]
            .as_array()
            .expect("models")
            .iter()
            .any(|model| model["alias"] == "custom/model-x"),
        "new model reflected in snapshot"
    );

    // MCP http with headers: values never cross the boundary.
    let post = web_http::post_json(
        port,
        &cookie,
        "/api/settings/mcp",
        &json!({
            "id": "remote",
            "transport": "http",
            "enabled": true,
            "url": "http://127.0.0.1:8000/mcp",
            "headers": { "Authorization": "Bearer super-secret" }
        }),
    )
    .await;
    assert_eq!(post.status, 200, "add mcp: {}", post.body);
    assert!(
        !post.body.contains("super-secret"),
        "header value must never cross the web boundary"
    );
    let parsed: Value = serde_json::from_str(&post.body).expect("settings json");
    let remote = parsed["mcp_servers"]
        .as_array()
        .expect("mcp servers")
        .iter()
        .find(|server| server["id"] == "remote")
        .expect("remote mcp");
    assert!(
        remote["header_keys"]
            .as_array()
            .expect("header keys")
            .iter()
            .any(|key| key == "Authorization"),
        "header keys surfaced, values never"
    );
}
