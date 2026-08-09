//! Session-runtime product boundary tests: real `neo webui --no-open`
//! subprocess (PTY), real HTTP + WebSocket, programmable mock provider.
//! Every test uses random ports, an isolated `NEO_HOME`, an isolated project
//! directory, readiness polls instead of fixed sleeps, and kills the child on
//! drop. Test names are condition-plus-observable-result.

use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

use super::http;
use super::provider::{Provider, Step, openai_response_sse, openai_tool_call_sse};
use super::pty::{NeoWebUi, spawn_webui};
use super::ws;

/// Shared environment for one test: isolated project + NEO_HOME, running
/// service, claimed cookie.
struct TestEnv {
    _project: TempDir,
    _home: TempDir,
    webui: NeoWebUi,
    cookie: String,
}

/// Start the provider, write the mock config into the isolated NEO_HOME,
/// spawn `neo webui --no-open` under a PTY and claim the one-time token.
async fn start_env(project: TempDir, steps: Vec<Step>) -> (TestEnv, Provider) {
    let home = tempfile::tempdir().expect("home tempdir");
    let provider = Provider::start(steps);
    let config = format!(
        r#"
default_provider = "mock"
default_model = "gpt-4.1"

[providers.mock]
type = "openai_response"
base_url = "{url}"
api_key_env = "OPENAI_API_KEY"

[models."mock/gpt-4.1"]
provider = "mock"
model = "gpt-4.1"
capabilities = ["streaming", "tools"]
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

async fn create_session(test_env: &TestEnv, message: &str) -> (String, String, Value) {
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

async fn snapshot(test_env: &TestEnv, session_id: &str) -> Value {
    let response = http::get(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/snapshot"),
    )
    .await;
    assert_eq!(response.status, 200, "snapshot: {}", response.body);
    serde_json::from_str(&response.body).expect("snapshot json")
}

async fn wait_for_phase(test_env: &TestEnv, session_id: &str, phase: &str) -> Value {
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

/// POST /api/sessions returns 201 with session_id and turn_id as soon as the
/// runtime reports its session id — the model request is held open and the
/// HTTP reply must not wait for the turn to complete.
#[tokio::test]
async fn first_message_returns_after_session_id_without_waiting_for_turn_completion() {
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::HoldThenRespond(openai_response_sse("resp-1", "held answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, turn_id, created) = create_session(&test_env, "first message").await;
    assert!(!session_id.is_empty());
    assert!(!turn_id.is_empty());
    assert_eq!(created["state"]["phase"], "starting");
    assert_eq!(created["state"]["current_turn_id"], turn_id);

    // The model request is still held: the session must not be idle yet.
    provider.wait_for_requests(1).await;
    let running = snapshot(&test_env, &session_id).await;
    assert_ne!(running["session"]["phase"], "idle");

    // Release the barrier: the turn completes, the title generation follows,
    // and the session reaches idle — all without any web involvement.
    provider.release_next().await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;
    provider.wait_for_requests(2).await;
    assert_eq!(provider.requests().len(), 2, "main turn + title generation");
}

/// Two sessions run in parallel; cancelling one never cancels the other.
#[tokio::test]
async fn different_sessions_run_concurrently_without_cross_cancellation() {
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::HoldThenRespond(openai_response_sse("resp-a", "A answer")),
            Step::Respond(openai_response_sse("resp-b", "B answer")),
            Step::Respond(openai_response_sse("resp-b-title", "Title B")),
            // A's cancelled turn still runs the post-turn title generation.
            Step::Respond(openai_response_sse("resp-a-title", "Title A")),
        ],
    )
    .await;
    let (session_a, turn_a, _) = create_session(&test_env, "session A message").await;
    let (session_b, turn_b, _) = create_session(&test_env, "session B message").await;
    // Let B fully complete first so the provider's remaining steps line up.
    let _ = wait_for_phase(&test_env, &session_b, "idle").await;

    // Cancel session A while its model request is held; session B already
    // completed and is untouched by the cancellation.
    let cancel_body = json!({ "turn_id": turn_a });
    let cancel_response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_a}/cancel"),
        &cancel_body,
    )
    .await;
    assert_eq!(
        cancel_response.status, 202,
        "cancel A: {}",
        cancel_response.body
    );

    let a_snapshot = wait_for_phase(&test_env, &session_a, "cancelled").await;
    assert_eq!(a_snapshot["session"]["current_turn_id"], Value::Null);
    let b_snapshot = snapshot(&test_env, &session_b).await;
    assert_eq!(
        b_snapshot["session"]["phase"], "idle",
        "B is not affected by A's cancel"
    );
    let _ = turn_b;

    let requests = provider.requests();
    assert!(
        requests
            .iter()
            .any(|request| request.body.contains("session B message")),
        "session B's turn reached the provider: {requests:?}"
    );
    assert!(
        requests
            .iter()
            .any(|request| request.body.contains("session A message")),
        "session A's turn reached the provider: {requests:?}"
    );
}

/// A follow-up input on a running session queues (202) and the next model
/// request carries the message — no parallel turn is started.
#[tokio::test]
async fn second_regular_input_queues_on_its_running_session() {
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::HoldThenRespond(openai_response_sse("resp-1", "first answer")),
            Step::Respond(openai_response_sse("resp-2", "second answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, turn_id, _) = create_session(&test_env, "first message").await;
    provider.wait_for_requests(1).await;

    let input_body = json!({
        "turn_id": turn_id,
        "delivery": "follow_up",
        "message": "second message"
    });
    let input_response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/input"),
        &input_body,
    )
    .await;
    assert_eq!(
        input_response.status, 202,
        "follow-up queued: {}",
        input_response.body
    );

    provider.release_next().await;
    provider.wait_for_requests(3).await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;

    let requests = provider.requests();
    assert_eq!(
        requests.len(),
        3,
        "main turn, follow-up turn, title: {requests:?}"
    );
    assert!(
        requests[1].body.contains("second message"),
        "the follow-up input reached the next model request: {}",
        requests[1].body
    );
}

/// Dropping the web subscription removes only the observer; the background
/// turn keeps running and completes.
#[tokio::test]
async fn dropping_the_web_subscription_does_not_cancel_the_background_turn() {
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::HoldThenRespond(openai_response_sse("resp-1", "background answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, _turn_id, _) = create_session(&test_env, "background work").await;
    provider.wait_for_requests(1).await;

    let (watch, first) =
        ws::connect_watch(test_env.webui.port, &test_env.cookie, &session_id, None).await;
    assert_eq!(first["type"], "session_snapshot", "snapshot delivered");
    assert_eq!(first["snapshot"]["session_id"], session_id);
    assert!(
        first["snapshot"].get("history").is_some(),
        "first payload is a snapshot"
    );
    drop(watch); // close the web long connection

    provider.release_next().await;
    let final_snapshot = wait_for_phase(&test_env, &session_id, "idle").await;
    assert_eq!(final_snapshot["session"]["phase"], "idle");
    assert!(
        serde_json::to_string(&final_snapshot["history"])
            .expect("history")
            .contains("background answer"),
        "the completed turn text is in the canonical history: {}",
        serde_json::to_string(&final_snapshot["history"]).expect("history json")
    );
}

/// Stale turn ids, cross-session controls and foreign output references are
/// rejected with the stable 409/404 codes; idle sessions answer
/// `no_active_turn`.
#[tokio::test]
async fn stale_or_cross_session_controls_are_rejected() {
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::HoldThenRespond(openai_response_sse("resp-1", "answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, turn_id, _) = create_session(&test_env, "controlled message").await;
    provider.wait_for_requests(1).await;

    // While running: stale turn ids and foreign pending-item ids.
    let input_body = json!({ "turn_id": "turn_bogus", "delivery": "follow_up", "message": "hi" });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/input"),
        &input_body,
    )
    .await;
    assert_eq!(response.status, 409, "stale input: {}", response.body);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).expect("code")["code"],
        "stale_turn"
    );

    let cancel_body = json!({ "turn_id": "turn_bogus" });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/cancel"),
        &cancel_body,
    )
    .await;
    assert_eq!(response.status, 409, "stale cancel: {}", response.body);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).expect("code")["code"],
        "stale_turn"
    );

    let approval_body = json!({
        "turn_id": "turn_bogus",
        "request_id": "approval_1",
        "action": { "kind": "permit_once" },
        "feedback": null,
    });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/approval"),
        &approval_body,
    )
    .await;
    assert_eq!(response.status, 409, "stale approval: {}", response.body);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).expect("code")["code"],
        "stale_control"
    );

    let question_body = json!({
        "turn_id": "turn_bogus",
        "question_id": "question_1",
        "answer": { "selections": ["Yes"] },
    });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/question"),
        &question_body,
    )
    .await;
    assert_eq!(response.status, 409, "stale question: {}", response.body);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).expect("code")["code"],
        "stale_control"
    );

    // Unknown sessions never leak existence.
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        "/api/sessions/00000000-0000-4000-8000-000000000099/input",
        &input_body,
    )
    .await;
    assert_eq!(
        response.status, 404,
        "unknown session input: {}",
        response.body
    );
    let response = http::get(
        test_env.webui.port,
        &test_env.cookie,
        "/api/sessions/00000000-0000-4000-8000-000000000099/snapshot",
    )
    .await;
    assert_eq!(
        response.status, 404,
        "unknown session snapshot: {}",
        response.body
    );

    // Foreign / path-form output references are rejected without leaking.
    let response = http::get(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/tool-output/not-a-ref?start_line=0&max_lines=10"),
    )
    .await;
    assert_eq!(
        response.status, 404,
        "path-form output ref: {}",
        response.body
    );
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).expect("code")["code"],
        "output_not_in_session"
    );

    // Complete the turn, then verify idle semantics for the real turn id.
    provider.release_next().await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;
    let input_body = json!({ "turn_id": turn_id, "delivery": "follow_up", "message": "late" });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/input"),
        &input_body,
    )
    .await;
    assert_eq!(response.status, 409, "idle input: {}", response.body);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).expect("code")["code"],
        "no_active_turn"
    );
    let cancel_body = json!({ "turn_id": turn_id });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/cancel"),
        &cancel_body,
    )
    .await;
    assert_eq!(response.status, 409, "idle cancel: {}", response.body);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).expect("code")["code"],
        "stale_turn"
    );
    let approval_body = json!({
        "turn_id": turn_id,
        "request_id": "approval_1",
        "action": { "kind": "permit_once" },
        "feedback": null,
    });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/approval"),
        &approval_body,
    )
    .await;
    assert_eq!(response.status, 409, "late approval: {}", response.body);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).expect("code")["code"],
        "stale_control"
    );
}

/// A StartTurn on an idle session starts a fresh turn (202 with a turn id),
/// a follow-up input queues on it (202) and reaches the next model request,
/// and a second StartTurn while the turn runs is rejected with 409. After
/// the turn completes, the idle snapshot's history is rebuilt from the
/// canonical JSONL (the completed turn released its projection).
#[tokio::test]
async fn start_turn_on_idle_session_accepts_follow_up_and_rejects_a_second_start() {
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_response_sse("resp-1", "first answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
            // StartTurn carries the submitted user message; the first model
            // request of that turn carries it as user text.
            Step::HoldThenRespond(openai_response_sse("resp-2", "second turn answer")),
            Step::Respond(openai_response_sse("resp-3", "follow-up answer")),
        ],
    )
    .await;
    let (session_id, _first_turn, _) = create_session(&test_env, "first message").await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;

    let start_body = json!({ "message": "second message" });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/turns"),
        &start_body,
    )
    .await;
    assert_eq!(response.status, 202, "start turn: {}", response.body);
    let parsed: Value = serde_json::from_str(&response.body).expect("start turn json");
    let turn_id = parsed["turn_id"].as_str().expect("turn id").to_owned();
    assert_eq!(parsed["session_id"], session_id);
    assert_eq!(parsed["state"]["phase"], "starting");
    assert_eq!(parsed["state"]["current_turn_id"], turn_id);

    // The model request carrying the submitted user message arrives and is
    // held open, so the turn is deterministically still running for the
    // follow-up and the second StartTurn below.
    provider.wait_for_requests(3).await;

    let input_body = json!({
        "turn_id": turn_id,
        "delivery": "follow_up",
        "message": "follow-up text"
    });
    let input_response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/input"),
        &input_body,
    )
    .await;
    assert_eq!(
        input_response.status, 202,
        "follow-up queued: {}",
        input_response.body
    );

    // A second StartTurn while the turn runs is rejected (session busy or
    // turn transition, per the phase machine).
    let second = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/turns"),
        &start_body,
    )
    .await;
    assert_eq!(second.status, 409, "second start: {}", second.body);
    let code = serde_json::from_str::<Value>(&second.body).expect("code")["code"]
        .as_str()
        .expect("code string")
        .to_owned();
    assert!(
        matches!(code.as_str(), "session_busy" | "turn_transition"),
        "busy code: {code}"
    );

    // Release the held turn: the follow-up becomes the next model request
    // and the session completes into idle.
    provider.release_next().await;
    let final_snapshot = wait_for_phase(&test_env, &session_id, "idle").await;
    assert_eq!(final_snapshot["session"]["current_turn_id"], Value::Null);
    let requests = provider.requests();
    assert!(
        requests
            .iter()
            .any(|request| request.body.contains("second message")),
        "the StartTurn message reached a model request: {requests:?}"
    );
    assert!(
        requests
            .iter()
            .any(|request| request.body.contains("follow-up text")),
        "the follow-up input reached a model request: {requests:?}"
    );

    // The idle snapshot's history is rebuilt from the canonical JSONL after
    // the completed turn released its projection: the whole conversation
    // must still be there with contiguous sequences.
    let history = final_snapshot["history"].as_array().expect("history array");
    assert!(!history.is_empty(), "rebuilt history is present");
    let serialized = serde_json::to_string(&history).expect("history json");
    assert!(
        serialized.contains("first message") && serialized.contains("follow-up answer"),
        "rebuilt history is complete: {serialized}"
    );
    let sequences: Vec<u64> = history
        .iter()
        .map(|entry| entry["sequence"].as_u64().expect("sequence"))
        .collect();
    assert!(
        sequences.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "rebuilt history sequences are contiguous: {sequences:?}"
    );
    let watermark = final_snapshot["watermark"].as_u64().expect("watermark");
    assert!(
        watermark >= *sequences.last().expect("last sequence"),
        "watermark covers the rebuilt history"
    );
}

/// The user message submitted with a StartTurn on an idle session is
/// persisted: after the turn completes, the rebuilt snapshot history carries
/// it exactly once as a `MessageAppended` user event — the only user-bubble
/// source on the web wire.
#[tokio::test]
async fn idle_session_turn_persists_the_submitted_user_message() {
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_response_sse("resp-1", "first answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
            Step::Respond(openai_response_sse("resp-2", "second answer")),
        ],
    )
    .await;
    let (session_id, _first_turn, _) = create_session(&test_env, "first message").await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;

    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/turns"),
        &json!({ "message": "persist me" }),
    )
    .await;
    assert_eq!(response.status, 202, "start turn: {}", response.body);
    provider.wait_for_requests(3).await;
    let final_snapshot = wait_for_phase(&test_env, &session_id, "idle").await;

    let history = final_snapshot["history"].as_array().expect("history array");
    let user_bubbles: Vec<&Value> = history
        .iter()
        .filter(|entry| {
            let text = entry.pointer("/event/MessageAppended/message/User/content/0/Text/text");
            text.and_then(Value::as_str) == Some("persist me")
        })
        .collect();
    assert_eq!(
        user_bubbles.len(),
        1,
        "the submitted user message appears exactly once as MessageAppended: {}",
        serde_json::to_string(&history).expect("history json")
    );
    // No other history entry carries the user text outside MessageAppended.
    let foreign = history
        .iter()
        .filter(|entry| {
            entry["event"].get("MessageAppended").is_none()
                && serde_json::to_string(entry)
                    .expect("entry json")
                    .contains("persist me")
        })
        .count();
    assert_eq!(foreign, 0, "no duplicate user-bubble source");
}

/// After the turn completes, the session drains all late control channels,
/// clears the turn id and becomes idle; the snapshot carries the full
/// contiguous history.
#[tokio::test]
async fn turn_completion_drains_late_control_channels_before_becoming_idle() {
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_response_sse("resp-1", "completed answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, turn_id, _) = create_session(&test_env, "completing message").await;
    let final_snapshot = wait_for_phase(&test_env, &session_id, "idle").await;
    let _ = turn_id;

    assert_eq!(final_snapshot["session"]["current_turn_id"], Value::Null);
    assert_eq!(final_snapshot["session"]["waiting_approval"], false);
    assert_eq!(final_snapshot["session"]["waiting_question"], false);
    let history = final_snapshot["history"].as_array().expect("history array");
    assert!(!history.is_empty(), "canonical history is present");
    let sequences: Vec<u64> = history
        .iter()
        .map(|entry| entry["sequence"].as_u64().expect("sequence"))
        .collect();
    let watermark = final_snapshot["watermark"].as_u64().expect("watermark");
    assert!(
        sequences.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "history sequences are contiguous: {sequences:?}"
    );
    assert!(watermark >= *sequences.last().expect("last sequence"));
    let serialized = serde_json::to_string(&history).expect("history json");
    assert!(
        serialized.contains("completing message") && serialized.contains("completed answer"),
        "user message and assistant text are in the projection"
    );

    // Metadata updates (title / pinned / archived) publish and persist.
    let patch_body = json!({ "title": "Pinned completion", "pinned": true, "archived": null });
    let response = http::patch_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}"),
        &patch_body,
    )
    .await;
    assert_eq!(response.status, 200, "metadata patch: {}", response.body);
    let patched: Value = serde_json::from_str(&response.body).expect("patch json");
    assert_eq!(patched["title"], "Pinned completion");
    assert_eq!(patched["pinned"], true);
    let final_snapshot = snapshot(&test_env, &session_id).await;
    assert_eq!(final_snapshot["metadata"]["title"], "Pinned completion");
    assert_eq!(final_snapshot["metadata"]["pinned"], true);
    let list_response = http::get(
        test_env.webui.port,
        &test_env.cookie,
        "/api/sessions?scope=active",
    )
    .await;
    assert_eq!(
        list_response.status, 200,
        "session list: {}",
        list_response.body
    );
    let list: Value = serde_json::from_str(&list_response.body).expect("list json");
    assert!(
        list["items"]
            .as_array()
            .expect("items")
            .iter()
            .any(|item| item["session_id"] == session_id && item["pinned"] == true),
        "pinned session appears in the active list: {}",
        list_response.body
    );
    let _ = provider;
}

/// An approval-required tool call surfaces in the snapshot and, once resolved,
/// the tool executes and its result returns to the model.
#[tokio::test]
async fn approval_surfaces_in_snapshot_and_resolves_to_execute_the_tool() {
    // The Write target must live inside the workspace (the project dir).
    let project = tempfile::tempdir().expect("project tempdir");
    let marker = project.path().join("marker.txt");
    let marker_path = marker.display().to_string();
    let (test_env, provider) = start_env(
        project,
        vec![
            // `Write` is a mutating tool: in ask mode it must open an approval
            // before the runtime executes it.
            Step::Respond(openai_tool_call_sse(
                "resp-1",
                "Write",
                "call-1",
                &serde_json::json!({ "path": marker_path, "content": "marker-file-content" })
                    .to_string(),
            )),
            Step::Respond(openai_response_sse("resp-2", "tool result seen")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, turn_id, _) = create_session(&test_env, "read the marker file").await;

    let port = test_env.webui.port;
    let cookie = test_env.cookie.clone();
    let path = format!("/api/sessions/{session_id}/snapshot");
    let pending = http::poll_until_async(
        || async {
            let response = http::get(port, &cookie, &path).await;
            let parsed: Value = serde_json::from_str(&response.body).ok()?;
            parsed
                .get("pending_approval")
                .cloned()
                .filter(|value| !value.is_null())
        },
        Duration::from_secs(30),
        "pending approval",
    )
    .await;
    let Some(pending) = pending else {
        let current = snapshot(&test_env, &session_id).await;
        panic!(
            "approval never appeared; requests: {:?}; snapshot: {}",
            provider.requests(),
            current
        );
    };
    assert_eq!(pending["turn_id"], turn_id);
    let request_id = pending["request_id"]
        .as_str()
        .expect("request id")
        .to_owned();
    let session_snapshot = snapshot(&test_env, &session_id).await;
    assert_eq!(session_snapshot["session"]["waiting_approval"], true);

    let approval_body = json!({
        "turn_id": turn_id,
        "request_id": request_id,
        "action": { "kind": "permit_once" },
        "feedback": null,
    });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/approval"),
        &approval_body,
    )
    .await;
    assert_eq!(response.status, 204, "approval resolved: {}", response.body);

    provider.wait_for_requests(3).await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;
    assert!(
        marker.exists(),
        "the approved Write tool actually executed and created the file"
    );
    let requests = provider.requests();
    assert_eq!(
        requests.len(),
        3,
        "tool-call turn, result round trip, title"
    );
    assert!(
        requests[1].body.contains(&marker_path),
        "the executed tool result reached the model: {}",
        requests[1].body
    );
    let final_snapshot = snapshot(&test_env, &session_id).await;
    assert_eq!(final_snapshot["session"]["waiting_approval"], false);
    let serialized = serde_json::to_string(&final_snapshot["history"]).expect("history json");
    assert!(
        serialized.contains("ApprovalRequested") && serialized.contains("ApprovalResolved"),
        "approval lifecycle events are in the projection"
    );
}

/// An `AskUserQuestion` tool call surfaces as a pending question; resolving it
/// lets the turn continue to completion.
#[tokio::test]
async fn question_surfaces_in_snapshot_and_resolves_to_continue_the_turn() {
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_tool_call_sse(
                "resp-1",
                "AskUserQuestion",
                "call-1",
                &serde_json::json!({
                    "questions": [
                        {
                            "question": "Continue?",
                            "options": [ { "label": "Yes" }, { "label": "No" } ]
                        }
                    ],
                    "background": false,
                })
                .to_string(),
            )),
            Step::Respond(openai_response_sse("resp-2", "continuing after answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, turn_id, _) = create_session(&test_env, "ask me something").await;

    let port = test_env.webui.port;
    let cookie = test_env.cookie.clone();
    let path = format!("/api/sessions/{session_id}/snapshot");
    let pending = http::poll_until_async(
        || async {
            let response = http::get(port, &cookie, &path).await;
            let parsed: Value = serde_json::from_str(&response.body).ok()?;
            parsed
                .get("pending_question")
                .cloned()
                .filter(|value| !value.is_null())
        },
        Duration::from_secs(30),
        "pending question",
    )
    .await;
    let Some(pending) = pending else {
        let current = snapshot(&test_env, &session_id).await;
        panic!(
            "question never appeared; requests: {:?}; snapshot: {}",
            provider.requests(),
            current
        );
    };
    assert_eq!(pending["turn_id"], turn_id);
    let question_id = pending["id"].as_str().expect("question id").to_owned();
    let session_snapshot = snapshot(&test_env, &session_id).await;
    assert_eq!(session_snapshot["session"]["waiting_question"], true);

    let question_body = json!({
        "turn_id": turn_id,
        "question_id": question_id,
        "answer": { "selections": ["Yes"], "text": null },
    });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/question"),
        &question_body,
    )
    .await;
    assert_eq!(response.status, 204, "question resolved: {}", response.body);

    provider.wait_for_requests(3).await;
    let final_snapshot = wait_for_phase(&test_env, &session_id, "idle").await;
    assert_eq!(final_snapshot["session"]["waiting_question"], false);
    assert_eq!(final_snapshot["session"]["current_turn_id"], Value::Null);
}

/// A reconnect with a resume cursor continues from the snapshot watermark
/// without gaps; a wrong stream id falls back to a full snapshot.
#[tokio::test]
async fn reconnect_resumes_after_the_snapshot_watermark_or_falls_back_to_snapshot() {
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_response_sse("resp-1", "resumable answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, _turn_id, _) = create_session(&test_env, "resumable message").await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;

    let (_watch, first) =
        ws::connect_watch(test_env.webui.port, &test_env.cookie, &session_id, None).await;
    assert_eq!(
        first["type"], "session_snapshot",
        "fresh watch delivers a snapshot"
    );
    let first = &first["snapshot"];
    assert_eq!(first["session_id"], session_id);
    let stream_id = first["stream_id"].as_str().expect("stream id").to_owned();
    let watermark = first["watermark"].as_u64().expect("watermark");
    let history_len = first["history"].as_array().expect("history").len() as u64;

    // Resume from a cursor behind the watermark: the contiguous cache replay
    // delivers the missing tail (a cursor at the exact watermark with no new
    // events is legitimately silent, so replay only the tail).
    let resume_from = watermark.saturating_sub(2);
    let (_, resumed) = ws::connect_watch(
        test_env.webui.port,
        &test_env.cookie,
        &session_id,
        Some((stream_id.clone(), resume_from)),
    )
    .await;
    let resumed_is_snapshot = resumed["type"] == "session_snapshot";
    if resumed_is_snapshot {
        let snapshot = &resumed["snapshot"];
        assert_eq!(
            snapshot["watermark"].as_u64().expect("watermark"),
            watermark
        );
        assert!(snapshot["history"].as_array().expect("history").len() as u64 >= history_len);
    } else {
        assert!(
            matches!(
                resumed["type"].as_str(),
                Some("session_event" | "session_state" | "session_metadata_changed")
            ),
            "replay carries envelopes: {}",
            resumed
        );
        let sequence = resumed["sequence"].as_u64().expect("sequence");
        assert!(
            sequence > resume_from,
            "replay continues after the resume cursor ({sequence} <= {resume_from})"
        );
    }

    // A wrong stream id always falls back to a full snapshot.
    let (watch, wrong_stream) = ws::connect_watch(
        test_env.webui.port,
        &test_env.cookie,
        &session_id,
        Some(("webui_some_other_stream".to_owned(), watermark)),
    )
    .await;
    assert_eq!(
        wrong_stream["type"], "session_snapshot",
        "wrong stream id forces a snapshot"
    );
    assert_eq!(wrong_stream["snapshot"]["session_id"], session_id);
    drop(watch);
    let _ = provider;
}
