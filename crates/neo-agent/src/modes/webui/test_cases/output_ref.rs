//! Opaque output references: stable encoding, ownership built from events,
//! and ownership-checked reads that never leak other sessions' artifacts.

use super::state_fixtures::{test_state, test_state_in};
use super::*;

#[test]
fn output_ref_ownership_is_built_from_events_and_decodes_stably() {
    let relay = Relay::new("test_stream");
    let state = test_state(&relay, "session_1", None);
    let reference = ToolOutputRef {
        agent_id: "main".to_owned(),
        task_id: "task_1".to_owned(),
        byte_len: 12,
        line_count: 3,
        complete: false,
    };
    let encoded = encode_output_ref(&reference).expect("encode");
    {
        let mut guard = state.lock().expect("state lock");
        guard.ingest_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "tool_1".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({}),
            workflow_origin: None,
            output_ref: Some(reference.clone()),
        });
    }
    let guard = state.lock().expect("state lock");
    assert!(guard.output_refs.contains(&encoded));
    assert_eq!(decode_output_ref(&encoded), Some(reference));
    // Path-form and free strings never decode.
    assert_eq!(
        decode_output_ref("/tmp/session/main/tasks/task_1.log"),
        None
    );
    assert_eq!(decode_output_ref("not-base64!"), None);
    {
        use base64::Engine as _;
        let wrong_shape = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::json!({"agent_id": "main", "task_id": "task_1"})
                .to_string()
                .as_bytes(),
        );
        assert_eq!(decode_output_ref(&wrong_shape), None);
    }
}

#[test]
fn opaque_output_reference_reads_only_its_own_session() {
    let relay = Relay::new("test_stream");
    let dir_a = tempfile::tempdir().expect("session A dir");
    let dir_b = tempfile::tempdir().expect("session B dir");
    let state_a = test_state_in(&relay, "session_a", None, dir_a.path().to_path_buf());
    let state_b = test_state_in(&relay, "session_b", None, dir_b.path().to_path_buf());

    let reference = ToolOutputRef {
        agent_id: "main".to_owned(),
        task_id: "task_a".to_owned(),
        byte_len: 11,
        line_count: 2,
        complete: true,
    };
    let store = neo_agent_core::session::ToolOutputStore::new(dir_a.path().to_path_buf());
    store.open("main", "task_a").expect("open artifact");
    store
        .append("main", "task_a", "alpha\nbeta\n")
        .expect("append");

    let encoded = encode_output_ref(&reference).expect("encode");
    {
        let mut guard = state_a.lock().expect("state lock");
        guard.ingest_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "tool_1".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({}),
            workflow_origin: None,
            output_ref: Some(reference.clone()),
        });
    }
    let guard_a = state_a.lock().expect("state lock");
    // The projected history entry carries the opaque metadata; the
    // structured ToolOutputRef never serializes to the web.
    let entry = guard_a.history.last().expect("history entry");
    let output = entry.output.as_ref().expect("opaque output metadata");
    assert_eq!(output.id, encoded);
    assert_eq!(output.byte_len, 11);
    assert_eq!(output.line_count, 2);
    assert!(output.complete);
    let event_json = serde_json::to_string(&entry.event).expect("event serializes");
    assert!(
        !event_json.contains("output_ref"),
        "structured reference never leaves the service: {event_json}"
    );

    // The owning session reads its own range.
    let range = read_owned_tool_output(&guard_a, &encoded, 0, 10).expect("owned read");
    assert!(range.text.contains("alpha"), "own output reads: {range:?}");

    // The same opaque id against another session is output_not_in_session.
    let guard_b = state_b.lock().expect("state lock");
    assert_eq!(
        read_owned_tool_output(&guard_b, &encoded, 0, 10)
            .expect_err("cross-session reference")
            .code,
        WebUiErrorCode::OutputNotInSession
    );
    drop(guard_b);

    // A well-formed reference the session never published is rejected too.
    let forged = encode_output_ref(&ToolOutputRef {
        agent_id: "main".to_owned(),
        task_id: "task_forged".to_owned(),
        byte_len: 0,
        line_count: 0,
        complete: false,
    })
    .expect("forged encodes");
    assert_eq!(
        read_owned_tool_output(&guard_a, &forged, 0, 10)
            .expect_err("forged reference")
            .code,
        WebUiErrorCode::OutputNotInSession
    );
    // Path-form input resolves to the same code without leaking.
    assert_eq!(
        read_owned_tool_output(&guard_a, "/tmp/session/main/tasks/task_a.log", 0, 10)
            .expect_err("path-form reference")
            .code,
        WebUiErrorCode::OutputNotInSession
    );
}
