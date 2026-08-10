//! Cross-workspace aggregation: the workspace snapshot groups sessions by
//! their workspace label (never an absolute path), and `watch_session` loads
//! a session that belongs to another workspace with its own recorded
//! workspace, matching CLI cross-directory resume semantics.

use std::time::Duration;

use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use super::provider::{Step, openai_response_sse};
use super::session_env::{create_session, start_env, wait_for_phase};
use super::ws;

/// One foreign workspace with one persisted session: a workspace directory
/// (its basename is the group label), a session bucket with metadata and a
/// main-agent wire, and a global-index entry pointing at both.
struct ForeignWorkspace {
    workdir: tempfile::TempDir,
    session_id: String,
}

fn seed_foreign_workspace(home: &std::path::Path, label_dir: &str) -> ForeignWorkspace {
    let workdir = tempfile::Builder::new()
        .prefix(&format!("{label_dir}-"))
        .tempdir()
        .expect("foreign workdir");
    let session_id = "session_11111111-1111-4111-8111-111111111111".to_owned();
    let bucket = home
        .join("sessions")
        .join(format!("wd_{label_dir}_testbucket"));
    let session_dir = bucket.join(&session_id);
    let wire_dir = session_dir.join("agents").join("main");
    std::fs::create_dir_all(&wire_dir).expect("foreign wire dir");
    std::fs::write(
        wire_dir.join("wire.jsonl"),
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"foreign question\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"foreign answer\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n",
        ),
    )
    .expect("foreign wire");
    let metadata = json!({
        "sessions": {
            session_id.clone(): {
                "title": "foreign session",
                "updated_at": "2026-08-10T08:00:00+00:00",
                "pinned": false,
                "archived": false
            }
        }
    });
    std::fs::write(bucket.join("sessions.metadata.json"), metadata.to_string())
        .expect("foreign metadata");
    let index = neo_agent_core::session::SessionIndex::new(home);
    index
        .append(&neo_agent_core::session::SessionIndexEntry {
            session_id: session_id.clone(),
            session_dir: bucket,
            workdir: workdir.path().to_path_buf(),
        })
        .expect("index append");
    ForeignWorkspace {
        workdir,
        session_id,
    }
}

/// Connect the long connection, subscribe the workspace summary layer and
/// return the workspace snapshot message.
async fn workspace_snapshot(port: u16, cookie: &str) -> (ws::Watch, Value) {
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
    use futures::{SinkExt, StreamExt};
    let (mut write, read) = socket.split();
    write
        .send(Message::Text(
            json!({ "type": "watch_workspace" }).to_string(),
        ))
        .await
        .expect("send watch_workspace");
    let mut watch = ws::Watch { read, write };
    let first = watch.next_json().await;
    (watch, first)
}

/// The workspace snapshot groups sessions by workspace label — current
/// workspace first, foreign workspaces by label — and no field of the wire
/// ever carries an absolute workspace path.
#[tokio::test]
async fn cross_workspace_listing_groups_by_label_without_absolute_paths() {
    let project = tempfile::tempdir().expect("project tempdir");
    let project_path = project.path().to_path_buf();
    let project_label = project_path
        .file_name()
        .expect("project basename")
        .to_string_lossy()
        .into_owned();
    let (test_env, provider) = start_env(
        project,
        vec![
            Step::Respond(openai_response_sse("resp-1", "current answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, _turn_id, _) = create_session(&test_env, "current workspace message").await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;
    let foreign = seed_foreign_workspace(test_env._home.path(), "playground");
    let foreign_path = foreign.workdir.path().to_path_buf();

    let (_watch, snapshot) = workspace_snapshot(test_env.webui.port, &test_env.cookie).await;
    assert_eq!(snapshot["type"], "workspace_snapshot");
    let serialized = serde_json::to_string(&snapshot).expect("snapshot json");
    for leaked in [
        project_path.to_string_lossy().as_ref(),
        foreign_path.to_string_lossy().as_ref(),
    ] {
        assert!(
            !serialized.contains(leaked),
            "workspace wire never carries an absolute path ({leaked}): {serialized}"
        );
    }
    let workspaces = snapshot["workspaces"].as_array().expect("workspaces");
    let current = workspaces
        .iter()
        .find(|group| group["current"] == true)
        .expect("current group");
    assert_eq!(current["label"], project_label);
    let current_sessions = current["sessions"].as_array().expect("current sessions");
    assert!(
        current_sessions
            .iter()
            .any(|summary| summary["session_id"] == session_id
                && summary["workspace_label"] == project_label),
        "the current session is grouped under its workspace label: {current_sessions:?}"
    );
    let other = workspaces
        .iter()
        .find(|group| group["current"] == false)
        .expect("foreign group");
    let foreign_label = foreign_path
        .file_name()
        .expect("foreign basename")
        .to_string_lossy()
        .into_owned();
    assert_eq!(other["label"], foreign_label);
    let other_sessions = other["sessions"].as_array().expect("foreign sessions");
    assert!(
        other_sessions
            .iter()
            .any(|summary| summary["session_id"] == foreign.session_id
                && summary["workspace_label"] == foreign_label
                && summary["title"] == "foreign session"),
        "the foreign session is grouped under its own workspace label: {other_sessions:?}"
    );
    let _ = provider;
}

/// `watch_session` on a session that belongs to another workspace loads it
/// from its indexed bucket (summary read, snapshot replay) — the same
/// resolution the CLI cross-directory resume uses — and the projected
/// transcript still leaks no absolute path.
#[tokio::test]
async fn watch_session_loads_session_from_another_workspace() {
    let (test_env, provider) =
        start_env(tempfile::tempdir().expect("project tempdir"), vec![]).await;
    let foreign = seed_foreign_workspace(test_env._home.path(), "playground");

    let (watch, first) = ws::connect_watch(
        test_env.webui.port,
        &test_env.cookie,
        &foreign.session_id,
        None,
    )
    .await;
    assert_eq!(
        first["type"], "session_snapshot",
        "a foreign-workspace session delivers a full snapshot"
    );
    let snapshot = &first["snapshot"];
    assert_eq!(snapshot["session_id"], foreign.session_id);
    let history = snapshot["history"].as_array().expect("history");
    let serialized = serde_json::to_string(history).expect("history json");
    assert!(
        serialized.contains("foreign question") && serialized.contains("foreign answer"),
        "the foreign session's canonical history replays: {serialized}"
    );
    assert!(
        !serialized.contains(foreign.workdir.path().to_string_lossy().as_ref()),
        "the foreign snapshot never carries the workspace path: {serialized}"
    );
    assert_eq!(snapshot["metadata"]["title"], "foreign session");
    drop(watch);
    let _ = provider;
    let _ = &test_env;
}
