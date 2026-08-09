//! Reconnect behavior: resume cursors continue from the snapshot watermark
//! and a wrong stream id falls back to a full snapshot; owned tool-output
//! ranges still read after the idle projection rebuild; a retried provider
//! attempt leaves no failed-attempt text in the reconnected snapshot.

use std::time::Duration;

use serde_json::{Value, json};

use super::http;
use super::provider::{Step, openai_response_sse, openai_tool_call_sse, sse_response};
use super::session_env::{
    create_session, snapshot, start_env, start_env_with_config, wait_for_phase,
};
use super::ws;

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

/// A completed turn releases its projection; the opaque tool-output reference
/// learned from the rebuilt idle snapshot must still read by range, because
/// ownership is rebuilt from the canonical history rather than retained from
/// the live turn.
#[tokio::test]
async fn owned_tool_output_range_reads_after_idle_projection_rebuild() {
    let (test_env, provider) = start_env_with_config(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_tool_call_sse(
                "resp-1",
                "Bash",
                "call-1",
                &json!({ "command": "printf 'alpha\\nbravo\\ncharlie\\n'" }).to_string(),
            )),
            Step::Respond(openai_response_sse("resp-2", "output seen")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
        "permission_mode = \"yolo\"\n",
    )
    .await;
    let (session_id, _turn_id, _) = create_session(&test_env, "run the marker command").await;

    // Wait for idle through the workspace list only: summaries read transport
    // state without rebuilding the projection, so the completed turn's
    // release is still in effect when the snapshot below triggers the rebuild
    // from the canonical JSONL.
    let port = test_env.webui.port;
    let cookie = test_env.cookie.clone();
    let listed = http::poll_until_async(
        || async {
            let response = http::get(port, &cookie, "/api/sessions?scope=active").await;
            let parsed: Value = serde_json::from_str(&response.body).ok()?;
            let item = parsed["items"]
                .as_array()?
                .iter()
                .find(|item| item["session_id"] == session_id)?;
            (item["state"] == "idle").then_some(true)
        },
        Duration::from_secs(30),
        "idle summary state",
    )
    .await;
    assert_eq!(listed, Some(true), "session reached idle");
    provider.wait_for_requests(3).await;

    // First projection access after the release: the rebuilt history carries
    // the opaque output reference minted from the canonical event.
    let idle_snapshot = snapshot(&test_env, &session_id).await;
    let history = idle_snapshot["history"].as_array().expect("history array");
    let output = history
        .iter()
        .find_map(|entry| entry.get("output").filter(|value| !value.is_null()))
        .unwrap_or_else(|| {
            panic!(
                "no output reference in rebuilt history: {}",
                serde_json::to_string(history).expect("history json")
            )
        });
    let output_id = output["id"].as_str().expect("output id").to_owned();
    assert!(
        output["line_count"].as_u64().expect("line count") >= 3,
        "the finished command captured at least three lines: {output}"
    );
    assert_eq!(output["complete"], true, "finished capture is complete");

    // Range reads against the rebuilt ownership set.
    let full = http::get(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/tool-output/{output_id}?start_line=0&max_lines=1000"),
    )
    .await;
    assert_eq!(full.status, 200, "full range: {}", full.body);
    let full: Value = serde_json::from_str(&full.body).expect("full range json");
    assert_eq!(full["start_line"], 0);
    assert_eq!(full["reached_end"], true, "one page covers the capture");
    let full_text = full["text"].as_str().expect("full text").to_owned();
    for marker in ["alpha", "bravo", "charlie"] {
        assert!(
            full_text.contains(marker),
            "captured output contains {marker}: {full_text}"
        );
    }

    let head = http::get(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/tool-output/{output_id}?start_line=0&max_lines=1"),
    )
    .await;
    assert_eq!(head.status, 200, "head range: {}", head.body);
    let head: Value = serde_json::from_str(&head.body).expect("head range json");
    assert_eq!(head["reached_end"], false, "more lines follow the first");
    assert_eq!(head["next_line"], 1);
    let head_text = head["text"].as_str().expect("head text").to_owned();

    let rest = http::get(
        test_env.webui.port,
        &test_env.cookie,
        &format!("/api/sessions/{session_id}/tool-output/{output_id}?start_line=1&max_lines=1000"),
    )
    .await;
    assert_eq!(rest.status, 200, "rest range: {}", rest.body);
    let rest: Value = serde_json::from_str(&rest.body).expect("rest range json");
    assert_eq!(rest["start_line"], 1);
    assert_eq!(rest["reached_end"], true);
    let rest_text = rest["text"].as_str().expect("rest text").to_owned();
    assert_eq!(
        full_text,
        format!("{head_text}{rest_text}"),
        "offset range resumes exactly where the first page stopped"
    );
}

/// A provider attempt whose stream dies mid-text is retried; the reconnected
/// snapshot (rebuilt from the canonical JSONL) carries the retry boundary but
/// never the failed attempt's partial text.
#[tokio::test]
async fn retried_provider_session_reconnects_without_failed_attempt_text() {
    // A complete HTTP response whose SSE stream ends without a terminal event:
    // the client surfaces a retryable transport error after the text delta.
    let truncated = sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-attempt-1" } }),
        json!({ "type": "response.output_text.delta", "delta": "dropped partial attempt text" }),
    ]);
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(truncated),
            Step::Respond(openai_response_sse("resp-2", "recovered answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let (session_id, _turn_id, _) = create_session(&test_env, "retry me").await;
    // Failed attempt, successful retry, title generation.
    provider.wait_for_requests(3).await;
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;
    let requests = provider.requests();
    assert!(
        requests[0].body.contains("retry me") && requests[1].body.contains("retry me"),
        "the retry re-issued the same turn request: {requests:?}"
    );

    // Reconnect with a fresh cursor: the snapshot is the product boundary a
    // browser sees after a retry happened while it was away.
    let (watch, first) =
        ws::connect_watch(test_env.webui.port, &test_env.cookie, &session_id, None).await;
    assert_eq!(first["type"], "session_snapshot", "reconnect snapshot");
    let history = first["snapshot"]["history"]
        .as_array()
        .expect("history array");
    let serialized = serde_json::to_string(history).expect("history json");
    assert!(
        serialized.contains("RetryScheduled"),
        "the retry boundary is represented in the rebuilt history: {serialized}"
    );
    assert!(
        serialized.contains("recovered answer"),
        "the successful attempt's text is in the rebuilt history: {serialized}"
    );
    assert!(
        !serialized.contains("dropped partial attempt text"),
        "the failed attempt's partial text was withdrawn at the retry boundary: {serialized}"
    );
    drop(watch);
}
