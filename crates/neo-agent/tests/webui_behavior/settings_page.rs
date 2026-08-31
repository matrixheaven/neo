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
