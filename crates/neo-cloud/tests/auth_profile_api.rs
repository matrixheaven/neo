use std::{net::TcpListener, path::Path};

use neo_agent_core::{AgentEvent, AgentMessage};
use neo_cloud::{CloudServer, Store};
use neo_cloud_protocol::{
    BootstrapRequest, BootstrapResponse, CloudCommandCatalogResponse, CloudContinueSessionRequest,
    CloudContinueSessionResponse, CloudCreateShareRequest, CloudImportSessionRequest,
    CloudImportSessionResponse, CloudProfile, CloudSessionListResponse, CloudSessionPayload,
    CloudSessionTreeResponse, CloudShareListResponse, CloudSharePayload, CloudShareRecordResponse,
    CloudUpdateBranchRequest, CloudUpdateBranchResponse, DeviceTokenLoginRequest,
    ProfilePullResponse, ProfilePushRequest, ProfileStatusResponse, SettingsPullResponse,
    SettingsPushRequest,
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
    let access_token = "neo_at_0123456789abcdef0123456789abcdef";
    let device_token = "neo_dt_0123456789abcdef0123456789abcdef";
    let local_path = temp.path().join("alpha.jsonl");
    let config_path = temp.path().join("nested/config.toml");
    let config_path_text = config_path.to_string_lossy().into_owned();
    let jsonl = format!(
        "{}\n",
        serde_json::to_string(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text(format!(
                "keep hosted session but redact {secret}, {access_token}, {device_token}, {}, and {}",
                local_path.display(),
                config_path.display()
            )),
        })
        .expect("event json")
    );
    let leak_values = [
        secret,
        access_token,
        device_token,
        local_path.to_str().expect("local path"),
        config_path_text.as_str(),
    ];

    let imported = import_cloud_session(&client, &server.base_url, &token, jsonl).await;
    assert!(imported.record.id.starts_with("cs_"));
    assert_eq!(imported.record.local_session_id.as_deref(), Some("alpha"));
    assert_eq!(imported.record.message_count, 1);

    assert_listed_session_payload_is_sanitized(
        &client,
        &server.base_url,
        &token,
        &imported,
        &leak_values,
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
    assert_no_leaks(&share.html, &leak_values);

    assert_public_share_artifacts(&client, &server.base_url, &share.record.id, &leak_values).await;
}

#[tokio::test]
async fn self_hosted_collaboration_apis_expose_tree_metadata_records_continuation_settings_and_catalog()
 {
    let temp = TempDir::new().expect("tempdir");
    let store = Store::open(&temp.path().join("neo-cloud.sqlite"))
        .await
        .expect("open store");
    let server = spawn_server(store);
    let client = reqwest::Client::new();
    let token = bootstrap_access_token(&client, &server.base_url).await;
    let secret = "sk-test-secret-token";
    let absolute_path = temp.path().join("branch.jsonl");
    let absolute_path_text = absolute_path.to_string_lossy().into_owned();
    let leak_values = [secret, absolute_path_text.as_str()];
    let jsonl = format!(
        "{}\n",
        serde_json::to_string(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text(format!(
                "continue without leaking {secret} or {}",
                absolute_path.display()
            )),
        })
        .expect("event json")
    );

    let imported = import_cloud_session(&client, &server.base_url, &token, jsonl).await;
    let continued = update_branch_and_continue_session(
        &client,
        &server.base_url,
        &token,
        &imported,
        secret,
        &absolute_path,
        &leak_values,
    )
    .await;
    assert_cloud_session_tree(&client, &server.base_url, &token, &imported, &continued).await;
    assert_cloud_share_records(
        &client,
        &server.base_url,
        &token,
        &continued.record.id,
        &leak_values,
    )
    .await;
    assert_settings_sync(&client, &server.base_url, &token).await;
    assert_cloud_command_catalog(&client, &server.base_url, &token).await;
}

async fn update_branch_and_continue_session(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    imported: &CloudImportSessionResponse,
    secret: &str,
    absolute_path: &Path,
    leak_values: &[&str],
) -> CloudContinueSessionResponse {
    let branch_name = format!("Feature branch {secret} {}", absolute_path.display());
    let branch_summary = format!("Summary hides {secret} {}", absolute_path.display());
    let updated = client
        .put(format!(
            "{base_url}/v1/sessions/{}/branch",
            imported.record.id
        ))
        .bearer_auth(token)
        .json(&CloudUpdateBranchRequest {
            name: Some(branch_name),
            summary: Some(branch_summary),
            remote_parent_id: None,
        })
        .send()
        .await
        .expect("branch update response")
        .error_for_status()
        .expect("branch update success")
        .json::<CloudUpdateBranchResponse>()
        .await
        .expect("branch update json");
    let updated_json = serde_json::to_string(&updated).expect("updated json");
    assert_no_leaks(&updated_json, leak_values);
    assert_eq!(
        updated.record.summary.as_deref(),
        Some("Summary hides [REDACTED] [REDACTED_PATH]")
    );

    let continued = client
        .post(format!(
            "{base_url}/v1/sessions/{}/continue",
            imported.record.id
        ))
        .bearer_auth(token)
        .json(&CloudContinueSessionRequest {
            local_session_id: Some("remote-child".to_owned()),
            name: Some(format!("Child {secret} {}", absolute_path.display())),
        })
        .send()
        .await
        .expect("continue response")
        .error_for_status()
        .expect("continue success")
        .json::<CloudContinueSessionResponse>()
        .await
        .expect("continue json");
    assert_eq!(
        continued.record.remote_parent_id.as_deref(),
        Some(imported.record.id.as_str())
    );
    assert_eq!(
        continued.record.local_session_id.as_deref(),
        Some("remote-child")
    );
    assert_eq!(continued.messages.len(), 1);
    let continued_json = serde_json::to_string(&continued).expect("continued json");
    assert_no_leaks(&continued_json, leak_values);
    continued
}

async fn assert_cloud_session_tree(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    imported: &CloudImportSessionResponse,
    continued: &CloudContinueSessionResponse,
) {
    let tree = client
        .get(format!("{base_url}/v1/sessions/tree"))
        .bearer_auth(token)
        .send()
        .await
        .expect("tree response")
        .error_for_status()
        .expect("tree success")
        .json::<CloudSessionTreeResponse>()
        .await
        .expect("tree json");
    assert_eq!(tree.tree.len(), 2);
    assert_eq!(tree.tree[0].depth, 0);
    assert_eq!(tree.tree[0].record.id, imported.record.id);
    assert_eq!(tree.tree[0].children, vec![continued.record.id.clone()]);
    assert_eq!(tree.tree[1].depth, 1);
    assert_eq!(tree.tree[1].record.id, continued.record.id);
}

async fn assert_cloud_share_records(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    session_id: &str,
    leak_values: &[&str],
) {
    let share = create_public_share(client, base_url, token, session_id).await;
    let session_shares = client
        .get(format!("{base_url}/v1/sessions/{session_id}/shares"))
        .bearer_auth(token)
        .send()
        .await
        .expect("session share records response")
        .error_for_status()
        .expect("session share records success")
        .json::<CloudShareListResponse>()
        .await
        .expect("session share records json");
    assert_eq!(session_shares.shares, vec![share.record.clone()]);

    let share_record = client
        .get(format!("{base_url}/v1/share-records/{}", share.record.id))
        .bearer_auth(token)
        .send()
        .await
        .expect("share record response")
        .error_for_status()
        .expect("share record success")
        .json::<CloudShareRecordResponse>()
        .await
        .expect("share record json");
    assert_eq!(share_record.record, share.record);
    assert_public_share_artifacts(client, base_url, &share.record.id, leak_values).await;
}

async fn assert_settings_sync(client: &reqwest::Client, base_url: &str, token: &str) {
    let settings = CloudProfile {
        default_provider: Some("anthropic".to_owned()),
        default_model: Some("deepseek-v4-pro".to_owned()),
        api_base: Some("https://api.deepseek.com/anthropic".to_owned()),
        api_key_env: Some("DEEPSEEK_API_KEY".to_owned()),
        ..CloudProfile::default()
    };
    let settings_status = client
        .put(format!("{base_url}/v1/settings"))
        .bearer_auth(token)
        .json(&SettingsPushRequest {
            settings: settings.clone(),
        })
        .send()
        .await
        .expect("settings push response")
        .error_for_status()
        .expect("settings push success")
        .json::<ProfileStatusResponse>()
        .await
        .expect("settings status json");
    assert_eq!(settings_status.revision, 1);
    let pulled_settings = client
        .get(format!("{base_url}/v1/settings"))
        .bearer_auth(token)
        .send()
        .await
        .expect("settings pull response")
        .error_for_status()
        .expect("settings pull success")
        .json::<SettingsPullResponse>()
        .await
        .expect("settings pull json");
    assert_eq!(pulled_settings.settings, settings);
}

async fn assert_cloud_command_catalog(client: &reqwest::Client, base_url: &str, token: &str) {
    let catalog = client
        .get(format!("{base_url}/v1/commands"))
        .bearer_auth(token)
        .send()
        .await
        .expect("catalog response")
        .error_for_status()
        .expect("catalog success")
        .json::<CloudCommandCatalogResponse>()
        .await
        .expect("catalog json");
    let command_names = catalog
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect::<Vec<_>>();
    assert!(command_names.contains(&"sessions.tree"));
    assert!(command_names.contains(&"sessions.continue"));
    assert!(command_names.contains(&"settings.pull"));
    assert!(catalog.commands.iter().all(|command| command.available));
    assert!(
        catalog
            .commands
            .iter()
            .all(|command| !command.name.contains("oauth") && !command.name.contains("hosted"))
    );
}

#[tokio::test]
async fn managed_hosted_or_oauth_paths_fail_closed() {
    let temp = TempDir::new().expect("tempdir");
    let store = Store::open(&temp.path().join("neo-cloud.sqlite"))
        .await
        .expect("open store");
    let server = spawn_server(store);

    let status = reqwest::Client::new()
        .post(format!("{}/v1/auth/oauth", server.base_url))
        .send()
        .await
        .expect("oauth response")
        .status();

    assert!(status.is_client_error());
    assert_ne!(status, StatusCode::OK);
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
            summary: None,
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
    leak_values: &[&str],
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
    assert_no_leaks(&fetched_json, leak_values);
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
    leak_values: &[&str],
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
    let public_json_text = serde_json::to_string(&public_json).expect("public share json");
    assert_no_leaks(&public_json_text, leak_values);

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
    assert_no_leaks(&html, leak_values);
}

fn assert_no_leaks(haystack: &str, leak_values: &[&str]) {
    for value in leak_values {
        assert!(
            !haystack.contains(value),
            "sanitized payload leaked sensitive value {value:?} in {haystack}"
        );
    }
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
