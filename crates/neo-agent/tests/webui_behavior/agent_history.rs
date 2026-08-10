//! Child-agent history lazy-load: `GET .../agents/{agent_id}/history`
//! replays the child's own wire through the web projection (workspace-relative
//! paths, synthetic sequences from 1) without touching the session history,
//! and unknown, malformed or cross-session ids are rejected.

use serde_json::{Value, json};

use super::http;
use super::provider::{Step, openai_response_sse};
use super::session_env::{TestEnv, create_session, start_env, wait_for_phase};

/// The session's own directory on disk, resolved through the global index
/// (the same resolution the service uses).
fn session_dir(test_env: &TestEnv, session_id: &str) -> std::path::PathBuf {
    let index = neo_agent_core::session::SessionIndex::new(test_env._home.path());
    index
        .find(session_id)
        .expect("index lookup")
        .expect("indexed session")
        .session_dir
        .join(session_id)
}

fn seed_child_wire(session_dir: &std::path::Path, agent_id: &str, cwd: &std::path::Path) {
    let wire_dir = session_dir.join("agents").join(agent_id);
    std::fs::create_dir_all(&wire_dir).expect("child wire dir");
    let events = json!([
        {"MessageAppended": {"message": {"User": {"content": [{"Text": {"text": "child question"}}]}}}},
        {"ShellCommandStarted": {
            "turn": 0,
            "id": "cmd-1",
            "command": "ls",
            "cwd": cwd,
            "origin": "ModelBashTool"
        }},
        {"MessageAppended": {"message": {"Assistant": {
            "content": [{"Text": {"text": "child answer"}}],
            "tool_calls": [],
            "stop_reason": "EndTurn"
        }}}}
    ]);
    let mut wire = String::new();
    for event in events.as_array().expect("events") {
        wire.push_str(&event.to_string());
        wire.push('\n');
    }
    std::fs::write(wire_dir.join("wire.jsonl"), wire).expect("child wire");
}

async fn agent_history(test_env: &TestEnv, session_id: &str, agent_id: &str) -> http::HttpResult {
    http::get(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/agents/{agent_id}/history"),
    )
    .await
}

/// The child agent's wire replays through the web projection: synthetic
/// contiguous sequences from 1, workspace-relative path metadata, the
/// session's own history untouched.
#[tokio::test]
async fn agent_history_replays_child_wire_with_web_projection() {
    let project = tempfile::tempdir().expect("project tempdir");
    // The service works with the kernel-resolved (canonicalized) cwd; the
    // tempdir path itself may go through a symlink (/var → /private/var).
    let child_cwd = project
        .path()
        .canonicalize()
        .expect("canonical project path")
        .join("src");
    let (test_env, _provider) = start_env(
        project,
        vec![
            Step::Respond(openai_response_sse("resp-1", "parent answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, _turn_id, _) = create_session(&test_env, "parent message").await;
    let parent_snapshot = wait_for_phase(&test_env, &session_id, "idle").await;
    let parent_history_len = parent_snapshot["history"]
        .as_array()
        .expect("parent history")
        .len();

    seed_child_wire(&session_dir(&test_env, &session_id), "child-1", &child_cwd);

    let response = agent_history(&test_env, &session_id, "child-1").await;
    assert_eq!(response.status, 200, "agent history: {}", response.body);
    let body: Value = serde_json::from_str(&response.body).expect("agent history json");
    assert_eq!(body["agent_id"], "child-1");
    let history = body["history"].as_array().expect("history");
    assert_eq!(history.len(), 3);
    assert_eq!(body["watermark"], 3);
    let sequences: Vec<u64> = history
        .iter()
        .map(|entry| entry["sequence"].as_u64().expect("sequence"))
        .collect();
    assert_eq!(sequences, vec![1, 2, 3], "synthetic sequences from 1");
    let serialized = serde_json::to_string(history).expect("history json");
    assert!(
        serialized.contains("child question") && serialized.contains("child answer"),
        "child transcript replays: {serialized}"
    );
    assert!(
        serialized.contains("\"cwd\":\"src\""),
        "path metadata is workspace-relative: {serialized}"
    );
    assert!(
        !serialized.contains(&child_cwd.to_string_lossy().to_string()),
        "no absolute path leaves the service: {serialized}"
    );

    // The child replay never enters the session's own history.
    let parent_after = super::session_env::snapshot(&test_env, &session_id).await;
    assert_eq!(
        parent_after["history"]
            .as_array()
            .expect("parent history")
            .len(),
        parent_history_len,
        "session history unchanged by child replay"
    );
}

/// Unknown sessions, unknown or malformed agent ids and ids that only exist
/// under a different session all resolve to `not_found` — the child wire
/// lookup is always scoped to the addressed session's own directory.
#[tokio::test]
async fn agent_history_rejects_unknown_or_cross_session_ids() {
    let (test_env, _provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_response_sse("resp-1", "answer a")),
            Step::Respond(openai_response_sse("resp-title-a", "Title")),
            Step::Respond(openai_response_sse("resp-2", "answer b")),
            Step::Respond(openai_response_sse("resp-title-b", "Title")),
        ],
    )
    .await;
    let (session_a, _turn_a, _) = create_session(&test_env, "message a").await;
    let _ = wait_for_phase(&test_env, &session_a, "idle").await;
    let (session_b, _turn_b, _) = create_session(&test_env, "message b").await;
    let _ = wait_for_phase(&test_env, &session_b, "idle").await;
    seed_child_wire(
        &session_dir(&test_env, &session_b),
        "child-b",
        std::path::Path::new("."),
    );

    // A child that exists only under session B is not visible through A.
    let cross_session = agent_history(&test_env, &session_a, "child-b").await;
    assert_eq!(
        cross_session.status, 404,
        "cross-session: {}",
        cross_session.body
    );

    // Unknown agent ids under a real session.
    let unknown_agent = agent_history(&test_env, &session_a, "child-1").await;
    assert_eq!(
        unknown_agent.status, 404,
        "unknown agent: {}",
        unknown_agent.body
    );

    // Malformed agent ids (path traversal) are rejected before any lookup.
    let traversal = agent_history(&test_env, &session_a, "..").await;
    assert_eq!(traversal.status, 404, "traversal: {}", traversal.body);

    // Unknown sessions.
    let unknown_session = agent_history(
        &test_env,
        "session_00000000-0000-4000-8000-000000000001",
        "child-b",
    )
    .await;
    assert_eq!(
        unknown_session.status, 404,
        "unknown session: {}",
        unknown_session.body
    );

    // The real child under session B still loads.
    let own = agent_history(&test_env, &session_b, "child-b").await;
    assert_eq!(own.status, 200, "own child: {}", own.body);
}
