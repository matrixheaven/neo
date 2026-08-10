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

/// A child agent's tool event projected through the agents-history endpoint
/// carries an opaque output reference that the panel's read endpoint accepts:
/// ownership extends to persisted child-agent records under the session's own
/// `agents/` directory. Forged ids, traversal ids and cross-session reads all
/// resolve to the same 404.
#[tokio::test]
async fn agent_tool_output_reads_through_panel_ownership() {
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

    // A child agent with a real persisted tool-output artifact.
    let session_dir = session_dir(&test_env, &session_a);
    let store = neo_agent_core::session::ToolOutputStore::new(session_dir.clone());
    store.open("child-1", "task-1").expect("open artifact");
    store
        .append("child-1", "task-1", "alpha\nbravo\ncharlie\n")
        .expect("append artifact");
    let reference = store.finish("child-1", "task-1").expect("finish artifact");
    let wire_dir = session_dir.join("agents").join("child-1");
    std::fs::create_dir_all(&wire_dir).expect("child wire dir");
    let event = json!({"ToolExecutionFinished": {
        "turn": 0,
        "id": "call-1",
        "name": "Bash",
        "result": {
            "content": "alpha\nbravo\ncharlie\n",
            "media": [],
            "is_error": false,
            "details": null,
            "terminate": false
        },
        "output_ref": reference,
    }});
    std::fs::write(wire_dir.join("wire.jsonl"), format!("{event}\n")).expect("child wire");

    // The panel path: history projection mints the opaque id, the read
    // endpoint accepts it even though the child event never entered the main
    // projection's ownership set.
    let history = agent_history(&test_env, &session_a, "child-1").await;
    assert_eq!(history.status, 200, "agent history: {}", history.body);
    let body: Value = serde_json::from_str(&history.body).expect("history json");
    let output_id = body["history"]
        .as_array()
        .expect("history")
        .iter()
        .find_map(|entry| entry.get("output").filter(|value| !value.is_null()))
        .and_then(|output| output["id"].as_str())
        .unwrap_or_else(|| panic!("no output reference in child history: {}", history.body))
        .to_owned();
    let read = http::get(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_a}/tool-output/{output_id}?start_line=0&max_lines=100"),
    )
    .await;
    assert_eq!(read.status, 200, "panel read: {}", read.body);
    let range: Value = serde_json::from_str(&read.body).expect("range json");
    let text = range["text"].as_str().expect("range text");
    for marker in ["alpha", "bravo", "charlie"] {
        assert!(
            text.contains(marker),
            "captured output contains {marker}: {text}"
        );
    }

    // A forged task id under the real child agent: artifact missing → 404.
    let forged_task = encode_ref(&neo_agent_core::session::ToolOutputRef {
        agent_id: "child-1".to_owned(),
        task_id: "task-nope".to_owned(),
        byte_len: 0,
        line_count: 0,
        complete: true,
    });
    let forged = http::get(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_a}/tool-output/{forged_task}?start_line=0&max_lines=100"),
    )
    .await;
    assert_eq!(forged.status, 404, "forged task: {}", forged.body);

    // A traversal agent id never reaches a path join: 404.
    let traversal = encode_ref(&neo_agent_core::session::ToolOutputRef {
        agent_id: "..".to_owned(),
        task_id: "task-1".to_owned(),
        byte_len: 0,
        line_count: 0,
        complete: true,
    });
    let traversal = http::get(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_a}/tool-output/{traversal}?start_line=0&max_lines=100"),
    )
    .await;
    assert_eq!(traversal.status, 404, "traversal: {}", traversal.body);

    // The same child output id through another session's endpoint: 404.
    let cross_session = http::get(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_b}/tool-output/{output_id}?start_line=0&max_lines=100"),
    )
    .await;
    assert_eq!(
        cross_session.status, 404,
        "cross-session: {}",
        cross_session.body
    );
}

/// The wire encoding of an opaque output reference, mirroring the service's
/// own `encode_output_ref` (URL-safe base64 of the typed JSON form).
fn encode_ref(reference: &neo_agent_core::session::ToolOutputRef) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(reference).expect("ref json"))
}
