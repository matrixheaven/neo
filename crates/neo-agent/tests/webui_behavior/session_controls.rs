//! Session control plane: approvals and questions surface in the snapshot and
//! resolve through the HTTP control routes; stale turn ids, stale request or
//! question identifiers and unknown-session controls are rejected with stable
//! 409/404 codes without consuming the live pending control.

use serde_json::{Value, json};

use super::http;
use super::provider::{Step, openai_response_sse, openai_tool_call_sse};
use super::session_env::{create_session, snapshot, start_env, wait_for_pending, wait_for_phase};

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
                &json!({ "path": marker_path, "content": "marker-file-content" }).to_string(),
            )),
            Step::Respond(openai_response_sse("resp-2", "tool result seen")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, turn_id, _) = create_session(&test_env, "read the marker file").await;

    let pending = wait_for_pending(&test_env, &session_id, "pending_approval").await;
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
                &json!({
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

    let pending = wait_for_pending(&test_env, &session_id, "pending_questions").await;
    let Some(pending_questions) = pending else {
        let current = snapshot(&test_env, &session_id).await;
        panic!(
            "question never appeared; requests: {:?}; snapshot: {}",
            provider.requests(),
            current
        );
    };
    let pending = &pending_questions[0];
    assert_eq!(pending["turn_id"], turn_id);
    let question_id = pending["id"].as_str().expect("question id").to_owned();
    let session_snapshot = snapshot(&test_env, &session_id).await;
    assert_eq!(session_snapshot["session"]["waiting_question"], true);

    let wrong_answer_count = json!({
        "turn_id": turn_id,
        "question_id": question_id,
        "answer": { "selections": ["Yes", "No"], "text": null },
    });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/question"),
        &wrong_answer_count,
    )
    .await;
    assert_eq!(
        response.status, 400,
        "wrong answer count: {}",
        response.body
    );
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).expect("code")["code"],
        "invalid_request"
    );
    assert_eq!(
        snapshot(&test_env, &session_id).await["pending_questions"]
            .as_array()
            .map(Vec::len),
        Some(1),
        "the invalid answer did not consume the pending question"
    );

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

/// A stale request or question id on the correct turn is rejected with 409
/// `stale_control` and must not consume the live pending control: the real
/// identifier still resolves afterwards.
#[tokio::test]
async fn stale_request_or_question_id_leaves_the_pending_control_resolvable() {
    let project = tempfile::tempdir().expect("project tempdir");
    let marker = project.path().join("stale-marker.txt");
    let marker_path = marker.display().to_string();
    let (test_env, provider) = start_env(
        project,
        vec![
            Step::Respond(openai_tool_call_sse(
                "resp-1",
                "AskUserQuestion",
                "call-1",
                &json!({
                    "questions": [
                        {
                            "question": "Proceed?",
                            "options": [ { "label": "Yes" }, { "label": "No" } ]
                        }
                    ],
                    "background": false,
                })
                .to_string(),
            )),
            Step::Respond(openai_tool_call_sse(
                "resp-2",
                "Write",
                "call-2",
                &json!({ "path": marker_path, "content": "stale-marker-content" }).to_string(),
            )),
            Step::Respond(openai_response_sse("resp-3", "both resolved")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, turn_id, _) = create_session(&test_env, "question then approval").await;

    // Pending question: a wrong question id on the right turn is stale.
    let pending = wait_for_pending(&test_env, &session_id, "pending_questions").await;
    let Some(pending_questions) = pending else {
        panic!(
            "question never appeared; requests: {:?}",
            provider.requests()
        );
    };
    let pending = &pending_questions[0];
    let question_id = pending["id"].as_str().expect("question id").to_owned();

    let stale_question = json!({
        "turn_id": turn_id,
        "question_id": "question_bogus",
        "answer": { "selections": ["Yes"], "text": null },
    });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/question"),
        &stale_question,
    )
    .await;
    assert_eq!(response.status, 409, "stale question id: {}", response.body);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).expect("code")["code"],
        "stale_control"
    );
    let current = snapshot(&test_env, &session_id).await;
    assert_eq!(
        current["session"]["waiting_question"], true,
        "the stale attempt did not consume the pending question"
    );

    let resolve_question = json!({
        "turn_id": turn_id,
        "question_id": question_id,
        "answer": { "selections": ["Yes"], "text": null },
    });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/question"),
        &resolve_question,
    )
    .await;
    assert_eq!(
        response.status, 204,
        "the real question id still resolves: {}",
        response.body
    );

    // Pending approval: a wrong request id on the right turn is stale.
    let pending = wait_for_pending(&test_env, &session_id, "pending_approval").await;
    let Some(pending) = pending else {
        panic!(
            "approval never appeared; requests: {:?}",
            provider.requests()
        );
    };
    let request_id = pending["request_id"]
        .as_str()
        .expect("request id")
        .to_owned();

    let stale_approval = json!({
        "turn_id": turn_id,
        "request_id": "approval_bogus",
        "action": { "kind": "permit_once" },
        "feedback": null,
    });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/approval"),
        &stale_approval,
    )
    .await;
    assert_eq!(response.status, 409, "stale request id: {}", response.body);
    assert_eq!(
        serde_json::from_str::<Value>(&response.body).expect("code")["code"],
        "stale_control"
    );
    let current = snapshot(&test_env, &session_id).await;
    assert_eq!(
        current["session"]["waiting_approval"], true,
        "the stale attempt did not consume the pending approval"
    );

    let resolve_approval = json!({
        "turn_id": turn_id,
        "request_id": request_id,
        "action": { "kind": "permit_once" },
        "feedback": null,
    });
    let response = http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/approval"),
        &resolve_approval,
    )
    .await;
    assert_eq!(
        response.status, 204,
        "the real request id still resolves: {}",
        response.body
    );

    provider.wait_for_requests(4).await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;
    assert!(
        marker.exists(),
        "the approved Write executed after the stale attempt was rejected"
    );
}
