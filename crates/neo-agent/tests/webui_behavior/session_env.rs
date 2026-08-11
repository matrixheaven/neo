//! Shared fixtures for the web session product-boundary behavior modules:
//! isolated project + NEO_HOME, a running `neo webui --no-open` service and a
//! claimed cookie. Every test uses random ports, readiness polls instead of
//! fixed sleeps, and kills the child on drop. Test names are
//! condition-plus-observable-result.

use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

use super::http;
use super::provider::{Provider, Step};
use super::pty::{NeoWebUi, spawn_webui};

/// Shared environment for one test: isolated project + NEO_HOME, running
/// service, claimed cookie.
pub(crate) struct TestEnv {
    pub(crate) _project: TempDir,
    pub(crate) _home: TempDir,
    pub(crate) webui: NeoWebUi,
    pub(crate) cookie: String,
}

/// Start the provider, write the mock config into the isolated NEO_HOME,
/// spawn `neo webui --no-open` under a PTY and claim the one-time token.
pub(crate) async fn start_env(project: TempDir, steps: Vec<Step>) -> (TestEnv, Provider) {
    start_env_with_config(project, steps, "").await
}

/// [`start_env`] with extra lines appended to the isolated `config.toml`.
pub(crate) async fn start_env_with_config(
    project: TempDir,
    steps: Vec<Step>,
    extra_config: &str,
) -> (TestEnv, Provider) {
    start_env_inner(project, steps, extra_config, "\"streaming\", \"tools\"").await
}

/// [`start_env`] with the mock model's capability list replaced (media
/// capability lanes).
pub(crate) async fn start_env_with_capabilities(
    project: TempDir,
    steps: Vec<Step>,
    capabilities: &str,
) -> (TestEnv, Provider) {
    start_env_inner(project, steps, "", capabilities).await
}

async fn start_env_inner(
    project: TempDir,
    steps: Vec<Step>,
    extra_config: &str,
    capabilities: &str,
) -> (TestEnv, Provider) {
    let home = tempfile::tempdir().expect("home tempdir");
    let provider = Provider::start(steps);
    let config = format!(
        r#"
default_provider = "mock"
default_model = "gpt-4.1"
{extra_config}

[providers.mock]
type = "openai_response"
base_url = "{url}"
api_key_env = "OPENAI_API_KEY"

[models."mock/gpt-4.1"]
provider = "mock"
model = "gpt-4.1"
capabilities = [{capabilities}]
"#,
        url = provider.url
    );
    std::fs::write(home.path().join("config.toml"), config).expect("write home config");
    let webui = spawn_webui(project.path(), home.path(), Duration::from_secs(30));
    // Readiness probe: the address prints before the accept loop starts, so
    // wait until the port actually accepts a connection.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::net::TcpStream::connect(("127.0.0.1", webui.port)).await {
            Ok(_) => break,
            Err(_) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => {
                let lines = webui.captured.lock().expect("captured lock");
                panic!(
                    "web service never accepted connections: {error}; child output:\n{}",
                    lines.join("\n")
                );
            }
        }
    }
    let cookie = match http::claim_token(webui.port, &webui.token).await {
        Ok(cookie) => cookie,
        Err(error) => {
            let lines = webui.captured.lock().expect("captured lock");
            panic!("claim failed: {error}; child output:\n{}", lines.join("\n"));
        }
    };
    (
        TestEnv {
            _project: project,
            _home: home,
            webui,
            cookie,
        },
        provider,
    )
}

pub(crate) async fn create_session(test_env: &TestEnv, message: &str) -> (String, String, Value) {
    let body = json!({ "message": message, "composer": null });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        "/api/sessions",
        &body,
    )
    .await;
    if response.status != 201 {
        let lines = test_env.webui.captured.lock().expect("captured lock");
        let status = test_env.webui.wait_status();
        panic!(
            "create session failed: {}; child exit: {status:?}; child output:\n{}",
            response.body,
            lines.join("\n")
        );
    }
    let parsed: Value = serde_json::from_str(&response.body).expect("create session json");
    let session_id = parsed["session_id"]
        .as_str()
        .expect("session id")
        .to_owned();
    let turn_id = parsed["turn_id"].as_str().expect("turn id").to_owned();
    (session_id, turn_id, parsed)
}

pub(crate) async fn snapshot(test_env: &TestEnv, session_id: &str) -> Value {
    let response = http::get(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/snapshot"),
    )
    .await;
    assert_eq!(response.status, 200, "snapshot: {}", response.body);
    serde_json::from_str(&response.body).expect("snapshot json")
}

pub(crate) async fn wait_for_phase(test_env: &TestEnv, session_id: &str, phase: &str) -> Value {
    let port = test_env.webui.port;
    let cookie = test_env.cookie.clone();
    let path = format!("/api/sessions/{session_id}/snapshot");
    http::poll_until_async(
        || async {
            let response = http::get(port, &cookie, &path).await;
            if response.status != 200 {
                return None;
            }
            let parsed: Value = serde_json::from_str(&response.body).ok()?;
            let current = parsed["session"]["phase"].as_str().unwrap_or_default();
            (current == phase).then_some(parsed)
        },
        Duration::from_secs(30),
        &format!("phase {phase} for {session_id}"),
    )
    .await
    .unwrap_or_else(|| panic!("timed out waiting for phase {phase} for {session_id}"))
}

/// Poll the snapshot until the top-level pending-control field is present and
/// non-empty, returning its value.
pub(crate) async fn wait_for_pending(
    test_env: &TestEnv,
    session_id: &str,
    field: &str,
) -> Option<Value> {
    let port = test_env.webui.port;
    let cookie = test_env.cookie.clone();
    let path = format!("/api/sessions/{session_id}/snapshot");
    http::poll_until_async(
        || async {
            let response = http::get(port, &cookie, &path).await;
            let parsed: Value = serde_json::from_str(&response.body).ok()?;
            parsed.get(field).cloned().filter(|value| match value {
                Value::Null => false,
                Value::Array(items) => !items.is_empty(),
                _ => true,
            })
        },
        Duration::from_secs(30),
        field,
    )
    .await
}
