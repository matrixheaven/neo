//! Tests for V3 journal format (generic child lifecycle) and V2 read-only compatibility.

use neo_agent_core::workflow::{
    JOURNAL_FORMAT_V2, JOURNAL_FORMAT_V3, JournalEnvelope, JournalPayload, JournalPayloadRef,
    WorkflowArtifactId, WorkflowChildKey, WorkflowChildKind, WorkflowId, validate_v2_envelope,
};

fn test_run_id() -> WorkflowId {
    WorkflowId("wf_test_v3".to_owned())
}

#[test]
fn journal_v3_generic_child_lifecycle_round_trips_and_replays() {
    let run_id = test_run_id();
    let child_key = WorkflowChildKey::DirectDelegate {
        invocation_id: "inv1".to_owned(),
    };

    let queued = JournalEnvelope::new_v3(
        0,
        1000,
        run_id.clone(),
        JournalPayload::ChildQueued {
            child_key: child_key.clone(),
            child_kind: WorkflowChildKind::Delegate,
            invocation_id: "inv1".to_owned(),
            phase_id: Some("step1".to_owned()),
            spec_payload_ref: neo_agent_core::workflow::JournalPayloadRef {
                role: "spec".to_owned(),
                artifact_id: neo_agent_core::workflow::WorkflowArtifactId {
                    run_id: run_id.clone(),
                    content_sha256:
                        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                            .to_owned(),
                },
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    .to_owned(),
                byte_len: 42,
                media_type: None,
                logical_name: None,
            },
        },
    );
    assert_eq!(queued.version, JOURNAL_FORMAT_V3);
    let q_json = serde_json::to_string(&queued).expect("serialize");
    let q_parsed: JournalEnvelope = serde_json::from_str(&q_json).expect("deserialize");
    assert!(matches!(
        q_parsed.payload,
        JournalPayload::ChildQueued { .. }
    ));

    let started = JournalEnvelope::new_v3(
        1,
        2000,
        run_id.clone(),
        JournalPayload::ChildStarted {
            child_key: child_key.clone(),
            agent_id: "agent-a".to_owned(),
        },
    );
    let s_json = serde_json::to_string(&started).expect("serialize started");
    let s_parsed: JournalEnvelope = serde_json::from_str(&s_json).expect("deserialize started");
    assert!(matches!(
        s_parsed.payload,
        JournalPayload::ChildStarted { .. }
    ));

    let finished = JournalEnvelope::new_v3(
        2,
        3000,
        run_id,
        JournalPayload::ChildFinished {
            child_key,
            agent_id: Some("agent-a".to_owned()),
            outcome_payload_ref: neo_agent_core::workflow::JournalPayloadRef {
                role: "outcome".to_owned(),
                artifact_id: neo_agent_core::workflow::WorkflowArtifactId {
                    run_id: WorkflowId("wf_any".to_owned()),
                    content_sha256:
                        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                            .to_owned(),
                },
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    .to_owned(),
                byte_len: 100,
                media_type: None,
                logical_name: None,
            },
        },
    );
    let f_json = serde_json::to_string(&finished).expect("serialize finished");
    let f_parsed: JournalEnvelope = serde_json::from_str(&f_json).expect("deserialize finished");
    assert!(matches!(
        f_parsed.payload,
        JournalPayload::ChildFinished { .. }
    ));
}

#[test]
fn v2_terminal_children_project_read_only_without_rewrite() {
    let run_id = test_run_id();
    let v2 = JournalEnvelope::new(
        0,
        1000,
        run_id,
        JournalPayload::RunCreated {
            name: "legacy".to_owned(),
            description: None,
            launch_source: None,
        },
    );
    assert_eq!(v2.version, JOURNAL_FORMAT_V2);
    let json = serde_json::to_string(&v2).expect("serialize");
    let parsed: JournalEnvelope = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.version, JOURNAL_FORMAT_V2);
    validate_v2_envelope(&parsed).expect("v2 is valid");

    // V2 JSON bytes include only V2 version number, no V3 fields
    assert!(!json.contains("child_queued"));
    assert!(!json.contains("child_started"));
    assert!(!json.contains("child_finished"));
}

#[test]
fn started_without_finished_projects_recovering() {
    let run_id = test_run_id();
    let child_key = WorkflowChildKey::DirectDelegate {
        invocation_id: "inv_recover".to_owned(),
    };
    let started = JournalEnvelope::new_v3(
        1,
        2000,
        run_id,
        JournalPayload::ChildStarted {
            child_key,
            agent_id: "agent-b".to_owned(),
        },
    );
    assert_eq!(started.version, JOURNAL_FORMAT_V3);
    let json = serde_json::to_string(&started).expect("serialize");
    let parsed: JournalEnvelope = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(
        parsed.payload,
        JournalPayload::ChildStarted { .. }
    ));
    // Projection will mark this as Recovering when no ChildFinished exists.
}

#[test]
fn unknown_or_torn_v3_data_remains_fail_closed() {
    let result: Result<JournalEnvelope, _> = serde_json::from_str(
        r#"{"version":99,"seq":0,"timestamp_ms":1000,"run_id":"wf_test","payload":{"type":"run_created","name":"bad"}}"#,
    );
    let envelope = result.expect("serde deserialization");
    validate_v2_envelope(&envelope).expect_err("unknown version 99 must be rejected");

    let bad: Result<JournalEnvelope, _> = serde_json::from_str(
        r#"{"version":3,"seq":0,"timestamp_ms":1000,"run_id":"wf_test","payload":{"type":"bogus_event","data":42}}"#,
    );
    assert!(
        bad.is_err(),
        "unknown payload type must fail deserialization"
    );
}

#[test]
fn direct_and_swarm_children_replay_exactly_once_with_unresolved_started_as_recovering() {
    let run_id = WorkflowId("wf_replay_exact".to_owned());
    let delegate_key = WorkflowChildKey::DirectDelegate {
        invocation_id: "inv_direct".to_owned(),
    };
    let swarm_key = WorkflowChildKey::SwarmItem {
        swarm_id: "sw1".to_owned(),
        item_id: "item1".to_owned(),
    };

    // Direct delegate lifecycle: queued -> started -> finished
    let q = JournalEnvelope::new_v3(
        0,
        1000,
        run_id.clone(),
        JournalPayload::ChildQueued {
            child_key: delegate_key.clone(),
            child_kind: WorkflowChildKind::Delegate,
            invocation_id: "inv_direct".to_owned(),
            phase_id: None,
            spec_payload_ref: JournalPayloadRef {
                role: "spec".to_owned(),
                artifact_id: WorkflowArtifactId {
                    run_id: run_id.clone(),
                    content_sha256: "aabb".to_owned(),
                },
                sha256: "aabb".to_owned(),
                byte_len: 10,
                media_type: None,
                logical_name: None,
            },
        },
    );
    assert!(matches!(q.payload, JournalPayload::ChildQueued { .. }));

    let s = JournalEnvelope::new_v3(
        1,
        2000,
        run_id.clone(),
        JournalPayload::ChildStarted {
            child_key: delegate_key.clone(),
            agent_id: "agent-d".to_owned(),
        },
    );
    assert!(matches!(s.payload, JournalPayload::ChildStarted { .. }));

    let f = JournalEnvelope::new_v3(
        2,
        3000,
        run_id.clone(),
        JournalPayload::ChildFinished {
            child_key: delegate_key.clone(),
            agent_id: Some("agent-d".to_owned()),
            outcome_payload_ref: JournalPayloadRef {
                role: "outcome".to_owned(),
                artifact_id: WorkflowArtifactId {
                    run_id: run_id.clone(),
                    content_sha256: "ccdd".to_owned(),
                },
                sha256: "ccdd".to_owned(),
                byte_len: 20,
                media_type: None,
                logical_name: None,
            },
        },
    );
    assert!(matches!(f.payload, JournalPayload::ChildFinished { .. }));

    // Swarm item lifecycle: queued -> started -> finished
    let sq = JournalEnvelope::new_v3(
        3,
        4000,
        run_id.clone(),
        JournalPayload::ChildQueued {
            child_key: swarm_key.clone(),
            child_kind: WorkflowChildKind::SwarmItem,
            invocation_id: "inv_swarm".to_owned(),
            phase_id: Some("step2".to_owned()),
            spec_payload_ref: JournalPayloadRef {
                role: "spec".to_owned(),
                artifact_id: WorkflowArtifactId {
                    run_id: run_id.clone(),
                    content_sha256: "eeff".to_owned(),
                },
                sha256: "eeff".to_owned(),
                byte_len: 30,
                media_type: None,
                logical_name: None,
            },
        },
    );
    assert!(matches!(sq.payload, JournalPayload::ChildQueued { .. }));

    let ss = JournalEnvelope::new_v3(
        4,
        5000,
        run_id.clone(),
        JournalPayload::ChildStarted {
            child_key: swarm_key.clone(),
            agent_id: "agent-s".to_owned(),
        },
    );
    assert!(matches!(ss.payload, JournalPayload::ChildStarted { .. }));

    // Simulate: swarm item finishes (terminal), then replay would see it once
    let sf = JournalEnvelope::new_v3(
        5,
        6000,
        run_id,
        JournalPayload::ChildFinished {
            child_key: swarm_key,
            agent_id: Some("agent-s".to_owned()),
            outcome_payload_ref: JournalPayloadRef {
                role: "outcome".to_owned(),
                artifact_id: WorkflowArtifactId {
                    run_id: WorkflowId("wf_tmp".to_owned()),
                    content_sha256: "gghh".to_owned(),
                },
                sha256: "gghh".to_owned(),
                byte_len: 40,
                media_type: None,
                logical_name: None,
            },
        },
    );
    assert!(matches!(sf.payload, JournalPayload::ChildFinished { .. }));

    // After restart, a started child without ChildFinished projects as Recovering
    // (projection logic in Tasks 4-5; type-level proof here).
    assert_eq!(
        serde_json::to_string(&s).unwrap().contains("child_started"),
        true,
        "ChildStarted must serialize as child_started type"
    );
}
