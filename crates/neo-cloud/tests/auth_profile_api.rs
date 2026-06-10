use std::{net::TcpListener, path::Path};

use neo_agent_core::{AgentEvent, AgentMessage};
use neo_cloud::{CloudServer, Store};
use neo_cloud_protocol::{
    BootstrapRequest, BootstrapResponse, CloudCreateShareRequest, CloudImportSessionRequest,
    CloudImportSessionResponse, CloudProfile, CloudSessionListResponse, CloudSessionPayload,
    CloudSharePayload, DeviceTokenLoginRequest, ProfilePullResponse, ProfilePushRequest,
    ProfileStatusResponse,
};
use reqwest::StatusCode;
use tempfile::TempDir;

#[tokio::test]
async fn bootstrap_device_token_login_and_profile_sync_persist_in_sqlite() {
    let temp = TempDir::new().expect("tempdir");
    let database_path = temp.path().join("neo-cloud.sqlite");
    let store = Store::open(&database_path).await.expect("open store");
    let server = spawn_server(store);
    let client = reqwest::Client::new();

    let bootstrap = client
        .post(format!("{}/v1/auth/bootstrap", server.base_url))
        .json(&BootstrapRequest {
            device_name: "workstation".to_owned(),
        })
        .send()
        .await
        .expect("bootstrap response")
        .error_for_status()
        .expect("bootstrap success")
        .json::<BootstrapResponse>()
        .await
        .expect("bootstrap json");

    assert_eq!(bootstrap.token_type, "Bearer");
    assert!(!bootstrap.access_token.is_empty());
    assert!(!bootstrap.device_token.is_empty());

    let device_login = client
        .post(format!("{}/v1/auth/device-token", server.base_url))
        .json(&DeviceTokenLoginRequest {
            device_id: bootstrap.device_id.clone(),
            device_token: bootstrap.device_token.clone(),
        })
        .send()
        .await
        .expect("device login response")
        .error_for_status()
        .expect("device login success")
        .json::<BootstrapResponse>()
        .await
        .expect("device login json");
    assert_eq!(device_login.user_id, bootstrap.user_id);
    assert_ne!(device_login.access_token, bootstrap.access_token);

    let profile = CloudProfile {
        default_provider: Some("anthropic".to_owned()),
        default_model: Some("deepseek-v4-pro".to_owned()),
        model_scope: vec!["anthropic/deepseek-*".to_owned()],
        runtime: serde_json::json!({
            "max_tokens": 2048,
            "reasoning_effort": "high",
            "tool_execution_mode": "Sequential"
        }),
        tui: serde_json::json!({
            "keybindings": { "tui.input.submit": ["ctrl+j"] }
        }),
        extensions: vec!["echo".to_owned()],
        ..CloudProfile::default()
    };
    let pushed = client
        .put(format!("{}/v1/profile", server.base_url))
        .bearer_auth(&device_login.access_token)
        .json(&ProfilePushRequest {
            profile: profile.clone(),
        })
        .send()
        .await
        .expect("push response")
        .error_for_status()
        .expect("push success")
        .json::<ProfileStatusResponse>()
        .await
        .expect("push json");
    assert_eq!(pushed.revision, 1);

    drop(server);
    let store = Store::open(&database_path).await.expect("reopen store");
    let server = spawn_server(store);
    let pulled = client
        .get(format!("{}/v1/profile", server.base_url))
        .bearer_auth(&device_login.access_token)
        .send()
        .await
        .expect("pull response")
        .error_for_status()
        .expect("pull success")
        .json::<ProfilePullResponse>()
        .await
        .expect("pull json");

    assert_eq!(pulled.revision, 1);
    assert_eq!(pulled.profile, profile);
}

#[tokio::test]
async fn profile_endpoints_require_a_valid_bearer_token() {
    let temp = TempDir::new().expect("tempdir");
    let store = Store::open(&temp.path().join("neo-cloud.sqlite"))
        .await
        .expect("open store");
    let server = spawn_server(store);

    let status = reqwest::Client::new()
        .get(format!("{}/v1/profile/status", server.base_url))
        .bearer_auth("wrong-token")
        .send()
        .await
        .expect("status response")
        .status();

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn hosted_sessions_share_and_public_artifacts_are_persisted_and_sanitized() {
    let temp = TempDir::new().expect("tempdir");
    let store = Store::open(&temp.path().join("neo-cloud.sqlite"))
        .await
        .expect("open store");
    let server = spawn_server(store);
    let client = reqwest::Client::new();
    let token = bootstrap_access_token(&client, &server.base_url).await;
    let secret = "sk-test-secret-token";
    let local_path = temp.path().join("alpha.jsonl");
    let jsonl = format!(
        "{}\n",
        serde_json::to_string(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text(format!(
                "keep hosted session but redact {secret} and {}",
                local_path.display()
            )),
        })
        .expect("event json")
    );

    let imported = import_cloud_session(&client, &server.base_url, &token, jsonl).await;
    assert!(imported.record.id.starts_with("cs_"));
    assert_eq!(imported.record.local_session_id.as_deref(), Some("alpha"));
    assert_eq!(imported.record.message_count, 1);

    assert_listed_session_payload_is_sanitized(
        &client,
        &server.base_url,
        &token,
        &imported,
        secret,
        temp.path(),
    )
    .await;

    let forked = fork_cloud_session(&client, &server.base_url, &token, &imported.record.id).await;
    assert_eq!(
        forked.record.remote_parent_id.as_deref(),
        Some(imported.record.id.as_str())
    );

    let share = create_public_share(&client, &server.base_url, &token, &imported.record.id).await;
    assert!(share.record.public);
    assert!(share.html.contains("<!doctype html>"));
    assert!(!share.html.contains(secret));

    assert_public_share_artifacts(&client, &server.base_url, &share.record.id, secret).await;
}

async fn import_cloud_session(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    jsonl: String,
) -> CloudImportSessionResponse {
    client
        .post(format!("{base_url}/v1/sessions/import"))
        .bearer_auth(token)
        .json(&CloudImportSessionRequest {
            local_session_id: "alpha".to_owned(),
            jsonl,
            name: Some("Main thread".to_owned()),
            remote_parent_id: None,
        })
        .send()
        .await
        .expect("import response")
        .error_for_status()
        .expect("import success")
        .json::<CloudImportSessionResponse>()
        .await
        .expect("import json")
}

async fn assert_listed_session_payload_is_sanitized(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    imported: &CloudImportSessionResponse,
    secret: &str,
    temp_path: &Path,
) {
    let listed = client
        .get(format!("{base_url}/v1/sessions"))
        .bearer_auth(token)
        .send()
        .await
        .expect("list response")
        .error_for_status()
        .expect("list success")
        .json::<CloudSessionListResponse>()
        .await
        .expect("list json");
    assert_eq!(listed.sessions, vec![imported.record.clone()]);

    let fetched = client
        .get(format!("{base_url}/v1/sessions/{}", imported.record.id))
        .bearer_auth(token)
        .send()
        .await
        .expect("get response")
        .error_for_status()
        .expect("get success")
        .json::<CloudSessionPayload>()
        .await
        .expect("get json");
    let fetched_json = serde_json::to_string(&fetched).expect("fetched json");
    assert!(fetched_json.contains("keep hosted session"));
    assert!(!fetched_json.contains(secret));
    assert!(!fetched_json.contains(temp_path.to_str().expect("temp path")));
}

async fn fork_cloud_session(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    session_id: &str,
) -> neo_cloud_protocol::CloudForkSessionResponse {
    client
        .post(format!("{base_url}/v1/sessions/{session_id}/fork"))
        .bearer_auth(token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("fork response")
        .error_for_status()
        .expect("fork success")
        .json::<neo_cloud_protocol::CloudForkSessionResponse>()
        .await
        .expect("fork json")
}

async fn create_public_share(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    session_id: &str,
) -> CloudSharePayload {
    client
        .post(format!("{base_url}/v1/sessions/{session_id}/shares"))
        .bearer_auth(token)
        .json(&CloudCreateShareRequest { public: true })
        .send()
        .await
        .expect("share response")
        .error_for_status()
        .expect("share success")
        .json::<CloudSharePayload>()
        .await
        .expect("share json")
}

async fn assert_public_share_artifacts(
    client: &reqwest::Client,
    base_url: &str,
    share_id: &str,
    secret: &str,
) {
    let public_json = client
        .get(format!("{base_url}/v1/shares/{share_id}"))
        .send()
        .await
        .expect("public json response")
        .error_for_status()
        .expect("public json success")
        .json::<CloudSharePayload>()
        .await
        .expect("public json");
    assert_eq!(public_json.record.id, share_id);

    let html = client
        .get(format!("{base_url}/v1/shares/{share_id}.html"))
        .send()
        .await
        .expect("html response")
        .error_for_status()
        .expect("html success")
        .text()
        .await
        .expect("html text");
    assert!(html.contains("<!doctype html>"));
    assert!(!html.contains(secret));
}

async fn bootstrap_access_token(client: &reqwest::Client, base_url: &str) -> String {
    client
        .post(format!("{base_url}/v1/auth/bootstrap"))
        .json(&BootstrapRequest {
            device_name: "workstation".to_owned(),
        })
        .send()
        .await
        .expect("bootstrap response")
        .error_for_status()
        .expect("bootstrap success")
        .json::<BootstrapResponse>()
        .await
        .expect("bootstrap json")
        .access_token
}

struct TestServer {
    base_url: String,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn spawn_server(store: Store) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind server");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let server = CloudServer::new(store);
    let handle = tokio::spawn(async move {
        server.serve(listener).await.expect("serve cloud");
    });
    TestServer { base_url, handle }
}
