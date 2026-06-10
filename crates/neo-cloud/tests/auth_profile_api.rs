use std::net::TcpListener;

use neo_cloud::{CloudServer, Store};
use neo_cloud_protocol::{
    BootstrapRequest, BootstrapResponse, CloudProfile, DeviceTokenLoginRequest,
    ProfilePullResponse, ProfilePushRequest, ProfileStatusResponse,
};
use reqwest::StatusCode;
use tempfile::TempDir;

#[tokio::test]
async fn bootstrap_device_token_login_and_profile_sync_persist_in_sqlite() {
    let temp = TempDir::new().expect("tempdir");
    let database_path = temp.path().join("neo-cloud.sqlite");
    let store = Store::open(&database_path).await.expect("open store");
    let server = spawn_server(store).await;
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
    let server = spawn_server(store).await;
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
    let server = spawn_server(store).await;

    let status = reqwest::Client::new()
        .get(format!("{}/v1/profile/status", server.base_url))
        .bearer_auth("wrong-token")
        .send()
        .await
        .expect("status response")
        .status();

    assert_eq!(status, StatusCode::UNAUTHORIZED);
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

async fn spawn_server(store: Store) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind server");
    let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
    let server = CloudServer::new(store);
    let handle = tokio::spawn(async move {
        server.serve(listener).await.expect("serve cloud");
    });
    TestServer { base_url, handle }
}
