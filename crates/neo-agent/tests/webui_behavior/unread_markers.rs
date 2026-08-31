//! Unread-marker persistence: clicking a session (the frontend PATCHes
//! `read_session_id`) must clear the workspace snapshot's `unread` flag
//! durably across a service restart. The web service binds a random loopback
//! port every launch, so no browser-local state can survive; the read marker
//! in `webui_projects.json` is the only source of truth.

use std::time::Duration;

use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::provider::{Step, openai_response_sse};
use super::session_env::{TestEnv, create_session, start_env, wait_for_phase};
use super::{http as web_http, pty, ws};

/// Connect the long connection, subscribe the workspace summary layer and
/// return the first workspace snapshot.
async fn workspace_snapshot(port: u16, cookie: &str) -> Value {
    let request = http::Request::builder()
        .uri(format!("ws://127.0.0.1:{port}/api/events"))
        .header("Host", format!("127.0.0.1:{port}"))
        .header("Origin", format!("http://127.0.0.1:{port}"))
        .header("Cookie", cookie)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .expect("ws request");
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .expect("ws connect deadline")
    .expect("ws handshake");
    use futures::{SinkExt as _, StreamExt as _};
    let (mut write, read) = socket.split();
    write
        .send(Message::Text(
            json!({ "type": "watch_workspace" }).to_string(),
        ))
        .await
        .expect("send watch_workspace");
    let mut watch = ws::Watch { read, write };
    for _ in 0..5 {
        let message = watch.next_json().await;
        if message["type"] == "workspace_snapshot" {
            return message;
        }
    }
    panic!("no workspace snapshot delivered");
}

fn group_of<'a>(snapshot: &'a Value, workspace_id: &str) -> &'a Value {
    snapshot["workspaces"]
        .as_array()
        .expect("workspace groups")
        .iter()
        .find(|group| group["id"].as_str() == Some(workspace_id))
        .unwrap_or_else(|| panic!("workspace group {workspace_id} missing"))
}

#[tokio::test]
async fn read_marker_keeps_unread_cleared_after_service_restart() {
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_response_sse("resp-1", "current answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, _turn_id, _) = create_session(&test_env, "unread marker message").await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;
    let TestEnv {
        _project,
        _home,
        webui,
        cookie,
    } = test_env;
    let _provider = provider;
    let port = webui.port;

    // A workspace preference entry is what arms the unread computation (the
    // realistic state after the user pinned/renamed the project).
    let first = workspace_snapshot(port, &cookie).await;
    let workspace_id = first["workspaces"][0]["id"]
        .as_str()
        .expect("workspace id")
        .to_owned();
    let pin = web_http::patch_json(
        port,
        &cookie,
        &format!("/api/workspaces/{workspace_id}"),
        &json!({ "pinned": true }),
    )
    .await;
    assert_eq!(pin.status, 204, "pin workspace: {}", pin.body);

    let armed = workspace_snapshot(port, &cookie).await;
    let session = group_of(&armed, &workspace_id)["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session_id"].as_str() == Some(session_id.as_str()))
        .expect("session in group")
        .clone();
    assert_eq!(
        session["unread"].as_bool(),
        Some(true),
        "unknown session must read as unread before being clicked"
    );
    let updated_at = session["updated_at"]
        .as_str()
        .expect("updated at")
        .to_owned();

    // Clicking the session (frontend behavior) writes the server-side marker.
    let patch = web_http::patch_json(
        port,
        &cookie,
        &format!("/api/workspaces/{workspace_id}"),
        &json!({ "read_session_id": session_id }),
    )
    .await;
    assert_eq!(patch.status, 204, "mark read: {}", patch.body);
    let preferences: Value = serde_json::from_str(
        &std::fs::read_to_string(_home.path().join("webui_projects.json"))
            .expect("webui_projects.json"),
    )
    .expect("preferences json");
    assert_eq!(
        preferences[workspace_id.as_str()]["read_at_by_session"][session_id.as_str()].as_str(),
        Some(updated_at.as_str()),
        "read marker written to webui_projects.json"
    );

    // The read marker must be durable: a fresh service over the same home
    // reports the session as read.
    drop(webui);
    let webui2 = pty::spawn_webui(_project.path(), _home.path(), Duration::from_secs(30));
    let cookie2 = web_http::claim_token(webui2.port, &webui2.token)
        .await
        .expect("re-claim cookie after restart");
    let restarted = workspace_snapshot(webui2.port, &cookie2).await;
    let session2 = group_of(&restarted, &workspace_id)["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session_id"].as_str() == Some(session_id.as_str()))
        .expect("session in group")
        .clone();
    assert_eq!(
        session2["unread"].as_bool().unwrap_or(false),
        false,
        "read marker must survive the restart (updated_at {updated_at}); session: {session2}"
    );
    drop(webui2);
}
