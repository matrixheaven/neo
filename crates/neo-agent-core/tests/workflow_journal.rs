//! Canonical journal envelope and bounded scanner tests.

use neo_agent_core::AgentTokenUsage;
use neo_agent_core::workflow::journal::{
    JournalEnvelope, JournalPayload, JournalPayloadRef, JournalWriter, canonical_input_hash,
    collect_journal, scan_journal, scan_journal_page,
};
use neo_agent_core::workflow::recovery::{
    JournalRecoveryAction, quarantine_tail_path, recover_journal, recovery_quarantine_dir,
};
use neo_agent_core::workflow::{
    UserAnswerPolicy, WorkflowActor, WorkflowArtifactId, WorkflowChildKey, WorkflowChildKind,
    WorkflowChildRef, WorkflowError, WorkflowErrorCode, WorkflowFinalResultMetadata, WorkflowId,
    WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowLimits, WorkflowOutcomeStatus,
    WorkflowState,
};
use serde_json::json;

fn run_id() -> WorkflowId {
    WorkflowId::from_existing("wf_000000000000000000000000000000aa")
}

fn limits() -> WorkflowLimits {
    WorkflowLimits::default()
}

fn outcome_ok_with_usage_and_child() -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        status: WorkflowOutcomeStatus::Completed,
        summary: "child done".to_owned(),
        interruption: None,
        details: json!({}),
        actual_usage: Some(AgentTokenUsage {
            input_tokens: 11,
            output_tokens: 7,
            input_cache_read_tokens: 1,
            input_cache_write_tokens: 2,
        }),
        child_refs: vec![WorkflowChildRef {
            kind: "task".to_owned(),
            id: "child_1".to_owned(),
        }],
    }
}

#[test]
fn journal_round_trips_canonical_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let mut writer = JournalWriter::open(&path, id.clone(), &limits()).unwrap();
    writer
        .append(
            &JournalEnvelope::new(
                0,
                999,
                id.clone(),
                JournalPayload::RunCreated {
                    name: "round-trip".to_owned(),
                    description: None,
                    launch_source: None,
                },
            ),
            &limits(),
        )
        .unwrap();

    let input = json!({"task": "scan", "b": 1, "a": 2});
    let hash = canonical_input_hash(&input);
    let started = JournalEnvelope::new(
        1,
        1_000,
        id.clone(),
        JournalPayload::InvocationStarted {
            invocation_id: "inv_1".to_owned(),
            call_index: 0,
            kind: WorkflowInvocationKind::Delegate,
            canonical_input: Some(input.clone()),
        },
    )
    .with_canonical_input_hash(hash.clone());

    writer.append(&started, &limits()).unwrap();

    let finished = JournalEnvelope::new(
        2,
        1_001,
        id.clone(),
        JournalPayload::InvocationFinished {
            invocation_id: "inv_1".to_owned(),
            outcome: outcome_ok_with_usage_and_child(),
        },
    );
    writer.append(&finished, &limits()).unwrap();

    let envelopes = collect_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .unwrap();
    assert_eq!(envelopes.len(), 3);
    assert_eq!(envelopes[0].seq, 0);
    assert_eq!(envelopes[0].run_id, id);
    assert_eq!(
        envelopes[1].canonical_input_hash.as_deref(),
        Some(hash.as_str())
    );
    assert!(matches!(
        envelopes[1].payload,
        JournalPayload::InvocationStarted {
            ref invocation_id,
            canonical_input: Some(ref value),
            ..
        } if invocation_id == "inv_1" && value == &input
    ));
    assert_eq!(envelopes[2].seq, 2);

    // Reopen continues sequence without full-Vec open retention.
    let writer2 = JournalWriter::open(&path, id, &limits()).unwrap();
    assert_eq!(writer2.next_seq(), 3);
    assert_eq!(writer2.bytes_written(), writer.bytes_written());
}

#[test]
fn journal_scan_rejects_sequence_hash_and_run_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let other = WorkflowId::from_existing("wf_000000000000000000000000000000bb");
    let limits = limits();

    // Sequence gap.
    {
        let mut writer = JournalWriter::open(&path, id.clone(), &limits).unwrap();
        let created = JournalEnvelope::new(
            0,
            1_000,
            id.clone(),
            JournalPayload::RunCreated {
                name: "sequence-test".to_owned(),
                description: None,
                launch_source: None,
            },
        );
        writer.append(&created, &limits).unwrap();
        let ok = JournalEnvelope::new(
            1,
            1_001,
            id.clone(),
            JournalPayload::StateChanged {
                previous: WorkflowState::Queued,
                new: WorkflowState::Running,
                reason: "launch".to_owned(),
                actor: WorkflowActor::Runtime,
            },
        );
        writer.append(&ok, &limits).unwrap();
        let gap = JournalEnvelope::new(
            3,
            1_003,
            id.clone(),
            JournalPayload::StateChanged {
                previous: WorkflowState::Running,
                new: WorkflowState::Paused,
                reason: "gap".to_owned(),
                actor: WorkflowActor::Runtime,
            },
        );
        let err = writer.append(&gap, &limits).unwrap_err();
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert!(err.to_string().contains("sequence gap"), "{err}");
    }

    // Hash mismatch on disk is fail-closed by scanner.
    {
        let path = dir.path().join("bad_hash.jsonl");
        let input = json!({"task": "x"});
        let created = JournalEnvelope::new(
            0,
            999,
            id.clone(),
            JournalPayload::RunCreated {
                name: "bad-hash".to_owned(),
                description: None,
                launch_source: None,
            },
        );
        let mut bad = JournalEnvelope::new(
            1,
            1_000,
            id.clone(),
            JournalPayload::InvocationStarted {
                invocation_id: "inv_bad".to_owned(),
                call_index: 0,
                kind: WorkflowInvocationKind::Delegate,
                canonical_input: Some(input),
            },
        )
        .with_canonical_input_hash("not-a-real-hash");
        let line = serde_json::to_string(&bad).unwrap();
        let created_line = serde_json::to_string(&created).unwrap();
        std::fs::write(&path, format!("{created_line}\n{line}\n")).unwrap();
        let err = scan_journal(
            &path,
            Some(&id),
            limits.journal_record_bytes,
            limits.journal_total_bytes,
        )
        .unwrap_err();
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert!(
            err.to_string().contains("canonical input hash mismatch"),
            "{err}"
        );
        // Mutate after construction to keep compiler quiet about unused mut in some paths.
        bad.canonical_input_hash = Some("still-bad".into());
        let _ = bad;
    }

    // Run ID mismatch.
    {
        let path = dir.path().join("bad_run.jsonl");
        let rec = JournalEnvelope::new(
            0,
            1_000,
            other.clone(),
            JournalPayload::RunCreated {
                name: "demo".to_owned(),
                description: None,
                launch_source: Some("/workflow".to_owned()),
            },
        );
        let line = serde_json::to_string(&rec).unwrap();
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let err = scan_journal(
            &path,
            Some(&id),
            limits.journal_record_bytes,
            limits.journal_total_bytes,
        )
        .unwrap_err();
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert!(err.to_string().contains("run id mismatch"), "{err}");
    }

    // Unknown envelope fields fail closed.
    {
        let path = dir.path().join("bad_field.jsonl");
        let mut value = serde_json::to_value(JournalEnvelope::new(
            0,
            1_000,
            id.clone(),
            JournalPayload::RunCreated {
                name: "demo".to_owned(),
                description: None,
                launch_source: None,
            },
        ))
        .unwrap();
        value["unsupported_field"] = json!(true);
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&value).unwrap()),
        )
        .unwrap();
        let err = scan_journal(
            &path,
            Some(&id),
            limits.journal_record_bytes,
            limits.journal_total_bytes,
        )
        .unwrap_err();
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert!(err.to_string().contains("malformed"), "{err}");
    }

    // Unknown payload kind fails closed.
    {
        let path = dir.path().join("bad_kind.jsonl");
        let line = format!(
            r#"{{"seq":0,"timestamp_ms":1,"run_id":"{}","payload":{{"type":"not_a_real_kind"}}}}"#,
            id.as_str()
        );
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let err = scan_journal(
            &path,
            Some(&id),
            limits.journal_record_bytes,
            limits.journal_total_bytes,
        )
        .unwrap_err();
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
    }
}

#[test]
fn journal_record_families_preserve_terminal_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let mut writer = JournalWriter::open(&path, id.clone(), &limits()).unwrap();
    let limits = limits();
    let mut seq = 0u64;
    let mut ts = 1_000u64;
    let mut append =
        |payload: JournalPayload, hash: Option<String>, refs: Vec<JournalPayloadRef>| {
            let mut env = JournalEnvelope::new(seq, ts, id.clone(), payload);
            if let Some(h) = hash {
                env = env.with_canonical_input_hash(h);
            }
            if !refs.is_empty() {
                env = env.with_payload_refs(refs);
            }
            writer.append(&env, &limits).unwrap();
            seq += 1;
            ts += 1;
        };

    append(
        JournalPayload::RunCreated {
            name: "review".to_owned(),
            description: Some("demo".to_owned()),
            launch_source: Some("/workflow review".to_owned()),
        },
        None,
        vec![],
    );
    append(
        JournalPayload::StateChanged {
            previous: WorkflowState::Queued,
            new: WorkflowState::Running,
            reason: "worker".to_owned(),
            actor: WorkflowActor::Runtime,
        },
        None,
        vec![],
    );

    let large_input = json!({"blob": "x".repeat(64)});
    let input_hash = canonical_input_hash(&large_input);
    let details_sha = "a".repeat(64);
    let artifact_id = WorkflowArtifactId::new(id.clone(), details_sha.clone()).unwrap();
    let details_ref = JournalPayloadRef {
        role: "details".to_owned(),
        artifact_id: artifact_id.clone(),
        sha256: details_sha.clone(),
        byte_len: 4096,
        media_type: Some("application/json".to_owned()),
        logical_name: Some("invocation-details".to_owned()),
    };

    // Large details move to payload_refs; canonical input stays hash-linked.
    append(
        JournalPayload::InvocationStarted {
            invocation_id: "inv_1".to_owned(),
            call_index: 0,
            kind: WorkflowInvocationKind::Swarm,
            canonical_input: Some(large_input),
        },
        Some(input_hash),
        vec![details_ref.clone()],
    );

    let finish_outcome = outcome_ok_with_usage_and_child();
    append(
        JournalPayload::ChildQueued {
            child_key: WorkflowChildKey::SwarmItem {
                swarm_id: "swarm_1".to_owned(),
                item_id: "item_1".to_owned(),
            },
            child_kind: WorkflowChildKind::SwarmItem,
            invocation_id: "inv_1".to_owned(),
            phase_id: Some("review".to_owned()),
            title: Some("review item".to_owned()),
            role: Some("reviewer".to_owned()),
        },
        None,
        vec![],
    );
    append(
        JournalPayload::ChildStarted {
            child_key: WorkflowChildKey::SwarmItem {
                swarm_id: "swarm_1".to_owned(),
                item_id: "item_1".to_owned(),
            },
            agent_id: Some("agent-1".to_owned()),
        },
        None,
        vec![],
    );
    append(
        JournalPayload::ChildFinished {
            child_key: WorkflowChildKey::SwarmItem {
                swarm_id: "swarm_1".to_owned(),
                item_id: "item_1".to_owned(),
            },
            agent_id: Some("agent-1".to_owned()),
            status: finish_outcome.status,
            summary: finish_outcome.summary.clone(),
            actual_usage: finish_outcome.actual_usage,
            error: None,
        },
        None,
        vec![],
    );
    append(
        JournalPayload::InvocationFinished {
            invocation_id: "inv_1".to_owned(),
            outcome: finish_outcome.clone(),
        },
        None,
        // Verbose details ref only — usage/child_refs remain on outcome.
        vec![details_ref],
    );

    append(
        JournalPayload::SchemaRepairStarted {
            repair_id: "repair_1".to_owned(),
            invocation_id: "inv_1".to_owned(),
        },
        None,
        vec![],
    );
    append(
        JournalPayload::SchemaRepairFinished {
            repair_id: "repair_1".to_owned(),
            ok: true,
            summary: "repaired".to_owned(),
        },
        None,
        vec![],
    );

    append(
        JournalPayload::UserInputRequested {
            request_id: "req_1".to_owned(),
            prompt: "continue?".to_owned(),
            answer_schema: json!({"type": "boolean"}),
            default: Some(json!(true)),
            title: Some("Confirm".to_owned()),
            answer_policy: UserAnswerPolicy::Human,
        },
        None,
        vec![],
    );
    append(
        JournalPayload::UserInputAnswered {
            request_id: "req_1".to_owned(),
            answer: Some(json!({"a": "yes"})),
        },
        None,
        vec![],
    );

    append(
        JournalPayload::ArtifactCommitted {
            artifact_id: artifact_id.clone(),
            sha256: details_sha,
            byte_len: 4096,
            media_type: Some("application/json".to_owned()),
            logical_name: Some("invocation-details".to_owned()),
        },
        None,
        vec![],
    );

    append(
        JournalPayload::UsageRecorded {
            usage: AgentTokenUsage {
                input_tokens: 3,
                output_tokens: 4,
                input_cache_read_tokens: 0,
                input_cache_write_tokens: 0,
            },
            invocation_id: Some("inv_1".to_owned()),
        },
        None,
        vec![],
    );

    append(
        JournalPayload::ProvenanceRecorded {
            human_handle: Some("review-1".to_owned()),
            definition_name: Some("review".to_owned()),
            definition_revision: Some("c".repeat(64)),
            phase_id: Some("analysis".to_owned()),
            invocation_id: Some("inv_1".to_owned()),
            swarm_item_id: Some("item_1".to_owned()),
        },
        None,
        vec![],
    );

    append(
        JournalPayload::RecoveryActionApplied {
            action: "reconcile_incomplete".to_owned(),
            detail: Some(json!({"invocation_id": "inv_none"})),
            quarantine_sha256: None,
            removed_bytes: None,
        },
        None,
        vec![],
    );

    append(
        JournalPayload::FinalResultRecorded {
            metadata: WorkflowFinalResultMetadata {
                value: Some(json!({"ok": true})),
                artifact_id: None,
                schema_revision: None,
            },
        },
        None,
        vec![],
    );

    append(
        JournalPayload::StateChanged {
            previous: WorkflowState::Running,
            new: WorkflowState::Completed,
            reason: "done".to_owned(),
            actor: WorkflowActor::Runtime,
        },
        None,
        vec![],
    );

    let index = scan_journal(
        &path,
        Some(&id),
        limits.journal_record_bytes,
        limits.journal_total_bytes,
    )
    .unwrap();
    assert_eq!(index.next_seq, seq);
    assert_eq!(index.final_result_seq, Some(seq - 2));
    assert_eq!(index.terminal_state, Some(WorkflowState::Completed));
    assert_eq!(index.terminal_reason.as_deref(), Some("done"));
    assert!(!index.has_incomplete_invocations());
    assert!(!index.has_incomplete_children());

    let envelopes = collect_journal(
        &path,
        Some(&id),
        limits.journal_record_bytes,
        limits.journal_total_bytes,
    )
    .unwrap();
    // Every required family appears at least once.
    let mut kinds = std::collections::HashSet::new();
    for env in &envelopes {
        let tag = match &env.payload {
            JournalPayload::RunCreated { .. } => "run_created",
            JournalPayload::StateChanged { .. } => "state_changed",
            JournalPayload::InvocationStarted { .. } => "invocation_started",
            JournalPayload::InvocationFinished { .. } => "invocation_finished",
            JournalPayload::SchemaRepairStarted { .. } => "schema_repair_started",
            JournalPayload::SchemaRepairFinished { .. } => "schema_repair_finished",
            JournalPayload::UserInputRequested { .. } => "user_input_requested",
            JournalPayload::UserInputAnswered { .. } => "user_input_answered",
            JournalPayload::ArtifactCommitted { .. } => "artifact_committed",
            JournalPayload::FinalResultRecorded { .. } => "final_result_recorded",
            JournalPayload::RecoveryActionApplied { .. } => "recovery_action_applied",
            JournalPayload::UsageRecorded { .. } => "usage_recorded",
            JournalPayload::ProvenanceRecorded { .. } => "provenance_recorded",
            JournalPayload::ChildQueued { .. } => "child_queued",
            JournalPayload::ChildStarted { .. } => "child_started",
            JournalPayload::ChildFinished { .. } => "child_finished",
        };
        kinds.insert(tag);
    }
    for required in [
        "run_created",
        "state_changed",
        "invocation_started",
        "invocation_finished",
        "child_queued",
        "child_started",
        "child_finished",
        "schema_repair_started",
        "schema_repair_finished",
        "user_input_requested",
        "user_input_answered",
        "artifact_committed",
        "final_result_recorded",
        "recovery_action_applied",
        "usage_recorded",
        "provenance_recorded",
    ] {
        assert!(kinds.contains(required), "missing family {required}");
    }

    // Terminal metadata remains inline even when verbose details are referenced.
    let finished = envelopes
        .iter()
        .find_map(|e| match &e.payload {
            JournalPayload::InvocationFinished { outcome, .. } => Some(outcome),
            _ => None,
        })
        .expect("invocation finished");
    assert!(finished.actual_usage.is_some(), "usage must stay inline");
    assert_eq!(finished.child_refs.len(), 1, "child refs must stay inline");
    assert_eq!(finished.status, WorkflowOutcomeStatus::Completed);
    assert!(!finished.summary.is_empty());

    let usage = envelopes
        .iter()
        .find_map(|e| match &e.payload {
            JournalPayload::UsageRecorded { usage, .. } => Some(usage),
            _ => None,
        })
        .expect("usage recorded");
    assert_eq!(usage.input_tokens, 3);
    assert_eq!(usage.output_tokens, 4);

    // Bounded page does not require full retention API for consumers.
    let page = scan_journal_page(
        &path,
        Some(&id),
        limits.journal_record_bytes,
        limits.journal_total_bytes,
        0,
        3,
        1024 * 1024,
    )
    .unwrap();
    assert_eq!(page.envelopes.len(), 3);
    assert!(page.has_more);
    assert_eq!(page.first_seq, Some(0));
    assert_eq!(page.last_seq, Some(2));
}

fn write_valid_prefix(path: &std::path::Path, id: &WorkflowId) -> String {
    let env = JournalEnvelope::new(
        0,
        1_000,
        id.clone(),
        JournalPayload::RunCreated {
            name: "demo".to_owned(),
            description: None,
            launch_source: Some("/workflow".to_owned()),
        },
    );
    let line = serde_json::to_string(&env).unwrap();
    std::fs::write(path, format!("{line}\n")).unwrap();
    line
}

#[test]
fn journal_recovery_normalizes_valid_unterminated_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let line = write_valid_prefix(&path, &id);

    // Second valid envelope without terminating newline (torn after JSON, before \n).
    let second = JournalEnvelope::new(
        1,
        1_001,
        id.clone(),
        JournalPayload::StateChanged {
            previous: WorkflowState::Queued,
            new: WorkflowState::Running,
            reason: "launch".to_owned(),
            actor: WorkflowActor::Runtime,
        },
    );
    let second_line = serde_json::to_string(&second).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(second_line.as_bytes());
    // Explicitly no trailing newline.
    assert!(!bytes.ends_with(b"\n") || bytes.ends_with(second_line.as_bytes()));
    // Ensure file does not end with newline after append of second_line alone.
    std::fs::write(&path, &bytes).unwrap();
    let on_disk = std::fs::read(&path).unwrap();
    assert!(
        !on_disk.ends_with(b"\n"),
        "fixture must end without newline"
    );
    assert!(on_disk.starts_with(format!("{line}\n").as_bytes()));

    let report = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("normalize recovery");
    assert!(matches!(
        report.action,
        JournalRecoveryAction::NormalizedUnterminated { seq: 1 }
    ));
    let recovered = std::fs::read(&path).unwrap();
    assert!(
        recovered.ends_with(b"\n"),
        "normalized journal must end with newline"
    );
    assert_eq!(
        &recovered[..recovered.len() - 1],
        on_disk.as_slice(),
        "normalize must only append a newline"
    );

    let envelopes = collect_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .unwrap();
    assert_eq!(envelopes.len(), 2);
    assert_eq!(envelopes[1].seq, 1);
    assert_eq!(report.index.next_seq, 2);
}

#[test]
fn journal_recovery_enforces_record_limit_and_inferred_run_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let first = write_valid_prefix(&path, &id);
    let encoded_bytes = u64::try_from(first.len() + 1).unwrap();

    let report = recover_journal(&path, None, encoded_bytes, limits().journal_total_bytes)
        .expect("exact limit");
    assert_eq!(report.index.run_id.as_ref(), Some(&id));

    let before = std::fs::read(&path).unwrap();
    let scan_error = scan_journal(&path, None, encoded_bytes - 1, limits().journal_total_bytes)
        .expect_err("canonical scanner must enforce the same record limit");
    assert_eq!(scan_error.code(), WorkflowErrorCode::JournalCorrupt);
    let err = recover_journal(&path, None, encoded_bytes - 1, limits().journal_total_bytes)
        .expect_err("record over configured limit");
    assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let other = WorkflowId::from_existing("wf_000000000000000000000000000000bb");
    let second = JournalEnvelope::new(
        1,
        1_001,
        other,
        JournalPayload::StateChanged {
            previous: WorkflowState::Queued,
            new: WorkflowState::Running,
            reason: "launch".to_owned(),
            actor: WorkflowActor::Runtime,
        },
    );
    let second_line = serde_json::to_string(&second).unwrap();
    let mut mixed = before;
    mixed.extend_from_slice(second_line.as_bytes());
    std::fs::write(&path, &mixed).unwrap();

    let err = recover_journal(&path, None, 16 * 1024, limits().journal_total_bytes)
        .expect_err("inferred run id must remain stable");
    assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
    assert!(err.to_string().contains("run id mismatch"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), mixed);
}

#[test]
fn journal_scan_rejects_total_bytes_above_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let first = write_valid_prefix(&path, &id);
    let encoded_bytes = u64::try_from(first.len() + 1).unwrap();

    let error = scan_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        encoded_bytes - 1,
    )
    .expect_err("scanner must stop at the configured total size");
    assert_eq!(error.code(), WorkflowErrorCode::JournalCorrupt);

    let recovery = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        encoded_bytes - 1,
    )
    .expect_err("recovery must enforce the same total size");
    assert_eq!(recovery.code(), WorkflowErrorCode::JournalCorrupt);

    let page = scan_journal_page(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        encoded_bytes - 1,
        0,
        1,
        encoded_bytes,
    )
    .expect_err("paging must enforce the same total size");
    assert_eq!(page.code(), WorkflowErrorCode::JournalCorrupt);
}

#[test]
fn journal_recovery_quarantines_torn_tail_before_truncate() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path();
    let path = run_dir.join("journal.jsonl");
    let id = run_id();
    let _ = write_valid_prefix(&path, &id);
    let prefix = std::fs::read(&path).unwrap();
    let prefix_len = prefix.len() as u64;

    let torn = b"{\"seq\":1,\"timestamp_ms\":1001,\"run_id\":\"";
    let mut bytes = prefix.clone();
    bytes.extend_from_slice(torn);
    std::fs::write(&path, &bytes).unwrap();
    let original = std::fs::read(&path).unwrap();

    // Quarantine must succeed before truncate.
    let report = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("quarantine recovery");
    let JournalRecoveryAction::TornTailQuarantined {
        quarantine_sha256,
        quarantine_path,
        removed_bytes,
        last_validated_offset,
    } = report.action
    else {
        panic!("expected torn-tail quarantine, got {:?}", report.action);
    };

    assert_eq!(last_validated_offset, prefix_len);
    assert_eq!(removed_bytes, torn.len() as u64);
    assert!(quarantine_path.is_file(), "quarantine file must exist");
    assert_eq!(
        quarantine_path,
        quarantine_tail_path(run_dir, &quarantine_sha256)
    );
    let quarantined = std::fs::read(&quarantine_path).unwrap();
    assert_eq!(quarantined, torn);

    let after = std::fs::read(&path).unwrap();
    // Prefix preserved; recovery record may be appended after truncate.
    assert!(
        after.starts_with(&prefix),
        "valid prefix must survive truncation"
    );
    assert!(
        !after.windows(torn.len()).any(|w| w == torn),
        "torn suffix must not remain in journal"
    );
    assert!(report.recovery_record_appended);

    // Quarantine failure must leave original bytes unchanged.
    let path2 = run_dir.join("journal_fail.jsonl");
    std::fs::write(&path2, &original).unwrap();
    // Make recovery-quarantine a file so directory create fails for this journal's sibling path.
    // recover uses path.parent()/recovery-quarantine — poison that path after first success
    // by replacing the directory with a file for a second journal under a nested run dir.
    let run2 = run_dir.join("run2");
    std::fs::create_dir_all(&run2).unwrap();
    let path3 = run2.join("journal.jsonl");
    std::fs::write(&path3, &original).unwrap();
    // Create a file where the quarantine directory should be.
    let qdir = recovery_quarantine_dir(&run2);
    std::fs::write(&qdir, b"not-a-directory").unwrap();
    let before_fail = std::fs::read(&path3).unwrap();
    let err = recover_journal(
        &path3,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect_err("quarantine must fail");
    assert!(
        err.to_string().contains("quarantine") || err.to_string().contains("directory"),
        "unexpected error: {err}"
    );
    let after_fail = std::fs::read(&path3).unwrap();
    assert_eq!(
        before_fail, after_fail,
        "quarantine failure must leave journal byte-for-byte unchanged"
    );
    let _ = path2;
}

#[test]
fn journal_recovery_of_torn_first_record_leaves_reopenable_empty_journal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let torn = br#"{"seq":0,"timestamp_ms":1000"#;
    std::fs::write(&path, torn).unwrap();

    let report = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("quarantine torn first record");
    assert!(!report.recovery_record_appended);
    assert!(std::fs::read(&path).unwrap().is_empty());
    let JournalRecoveryAction::TornTailQuarantined {
        quarantine_path, ..
    } = report.action
    else {
        panic!("expected torn-tail quarantine");
    };
    assert_eq!(std::fs::read(quarantine_path).unwrap(), torn);

    let retry = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("empty canonical journal must reopen");
    assert_eq!(retry.action, JournalRecoveryAction::None);
    let writer = JournalWriter::open(&path, id, &limits()).expect("empty journal must reopen");
    assert_eq!(writer.next_seq(), 0);
}

#[test]
fn journal_recovery_propagates_recovery_record_failure_without_truncating() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let _ = write_valid_prefix(&path, &id);
    let prefix = std::fs::read(&path).unwrap();
    let torn = b"{\"seq\":";
    let mut bytes = prefix.clone();
    bytes.extend_from_slice(torn);
    std::fs::write(&path, &bytes).unwrap();
    let original = std::fs::read(&path).unwrap();

    let limit = u64::try_from(prefix.len()).unwrap();
    let error = recover_journal(&path, Some(&id), limit, limits().journal_total_bytes)
        .expect_err("recovery fact over the record limit must fail closed");
    assert!(matches!(
        error,
        WorkflowError::JournalRecordLimitExceeded { limit: actual, .. } if actual == limit
    ));

    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "recovery-record failure must leave the original journal byte-for-byte unchanged"
    );
    let quarantined = std::fs::read_dir(recovery_quarantine_dir(dir.path()))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(quarantined.len(), 1);
    assert_eq!(std::fs::read(&quarantined[0]).unwrap(), torn);

    let report = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("retry must atomically commit prefix and recovery fact");
    assert!(report.recovery_record_appended);
    let recovered = std::fs::read(&path).unwrap();
    assert!(recovered.starts_with(&prefix));
    assert!(
        !recovered.ends_with(torn),
        "the original torn EOF suffix must not survive recovery"
    );

    let records = collect_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|record| {
                matches!(
                    &record.payload,
                    JournalPayload::RecoveryActionApplied { .. }
                )
            })
            .count(),
        1,
        "retry must persist exactly one recovery fact"
    );
    let writer = JournalWriter::open(&path, id, &limits()).expect("restart-safe journal open");
    assert_eq!(writer.index().record_count, records.len() as u64);
}

#[test]
fn journal_recovery_fails_closed_on_interior_or_newline_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let id = run_id();

    // Newline-terminated invalid JSON at EOF — fail closed, no mutation.
    {
        let path = dir.path().join("newline_bad.jsonl");
        let prefix_line = write_valid_prefix(&path, &id);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(b"{not-json}\n");
        std::fs::write(&path, &bytes).unwrap();
        let before = std::fs::read(&path).unwrap();
        let err = recover_journal(
            &path,
            Some(&id),
            limits().journal_record_bytes,
            limits().journal_total_bytes,
        )
        .expect_err("must fail closed");
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = prefix_line;
    }

    // Interior malformed (bad complete line between good lines).
    {
        let path = dir.path().join("interior_bad.jsonl");
        let env0 = JournalEnvelope::new(
            0,
            1_000,
            id.clone(),
            JournalPayload::RunCreated {
                name: "demo".to_owned(),
                description: None,
                launch_source: None,
            },
        );
        let second = JournalEnvelope::new(
            1,
            1_002,
            id.clone(),
            JournalPayload::StateChanged {
                previous: WorkflowState::Queued,
                new: WorkflowState::Running,
                reason: "x".to_owned(),
                actor: WorkflowActor::Runtime,
            },
        );
        let content = format!(
            "{}\n{{bad}}\n{}\n",
            serde_json::to_string(&env0).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
        std::fs::write(&path, &content).unwrap();
        let before = std::fs::read(&path).unwrap();
        let err = recover_journal(
            &path,
            Some(&id),
            limits().journal_record_bytes,
            limits().journal_total_bytes,
        )
        .expect_err("interior must fail closed");
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    // Sequence mismatch on complete lines — fail closed.
    {
        let path = dir.path().join("seq_gap.jsonl");
        let env0 = JournalEnvelope::new(
            0,
            1_000,
            id.clone(),
            JournalPayload::RunCreated {
                name: "demo".to_owned(),
                description: None,
                launch_source: None,
            },
        );
        let gap = JournalEnvelope::new(
            2,
            1_002,
            id.clone(),
            JournalPayload::StateChanged {
                previous: WorkflowState::Queued,
                new: WorkflowState::Running,
                reason: "gap".to_owned(),
                actor: WorkflowActor::Runtime,
            },
        );
        let content = format!(
            "{}\n{}\n",
            serde_json::to_string(&env0).unwrap(),
            serde_json::to_string(&gap).unwrap()
        );
        std::fs::write(&path, &content).unwrap();
        let before = std::fs::read(&path).unwrap();
        let err = recover_journal(
            &path,
            Some(&id),
            limits().journal_record_bytes,
            limits().journal_total_bytes,
        )
        .expect_err("seq gap must fail closed");
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert!(err.to_string().contains("sequence gap"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    // Run-id mismatch.
    {
        let path = dir.path().join("run_mismatch.jsonl");
        let other = WorkflowId::from_existing("wf_000000000000000000000000000000bb");
        let env = JournalEnvelope::new(
            0,
            1_000,
            other,
            JournalPayload::RunCreated {
                name: "demo".to_owned(),
                description: None,
                launch_source: None,
            },
        );
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&env).unwrap())).unwrap();
        let before = std::fs::read(&path).unwrap();
        let err = recover_journal(
            &path,
            Some(&id),
            limits().journal_record_bytes,
            limits().journal_total_bytes,
        )
        .expect_err("run id must fail closed");
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert!(err.to_string().contains("run id mismatch"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    // Hash mismatch on complete line.
    {
        let path = dir.path().join("hash_bad.jsonl");
        let mut env = JournalEnvelope::new(
            0,
            1_000,
            id.clone(),
            JournalPayload::InvocationStarted {
                invocation_id: "inv_1".to_owned(),
                call_index: 0,
                kind: WorkflowInvocationKind::Delegate,
                canonical_input: Some(json!({"task": "x"})),
            },
        )
        .with_canonical_input_hash("0".repeat(64));
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&env).unwrap())).unwrap();
        let before = std::fs::read(&path).unwrap();
        let err = recover_journal(
            &path,
            Some(&id),
            limits().journal_record_bytes,
            limits().journal_total_bytes,
        )
        .expect_err("hash must fail closed");
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        env.canonical_input_hash = Some("1".repeat(64));
        let _ = env;
    }
}

/// Platform journal contract: PathBuf quarantine layout, append durability via
/// sync_all, torn-tail quarantine before truncate, quarantine failure leaves
/// the journal byte-for-byte intact.
///
/// Native evidence target for Task 25 (macOS / Linux / Windows).
#[test]
fn journal_platform_sync_and_quarantine_semantics() {
    use std::path::PathBuf;

    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("workflows").join("platform-run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let path = run_dir.join("journal.jsonl");
    let id = run_id();

    // Open creates parent safely and returns a writer bound to Path (not strings).
    let mut writer = JournalWriter::open(&path, id.clone(), &limits()).expect("open empty journal");
    assert!(path.is_file());
    let created = JournalEnvelope::new(
        0,
        1_000,
        id.clone(),
        JournalPayload::RunCreated {
            name: "platform-sync".to_owned(),
            description: Some("sync proof".to_owned()),
            launch_source: Some("/workflow".to_owned()),
        },
    );
    writer.append(&created, &limits()).expect("append");
    drop(writer);

    // After append+sync_all, a fresh open must see the durable record.
    let reread = collect_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("reread after sync");
    assert_eq!(reread.len(), 1);
    assert_eq!(reread[0].seq, 0);

    // Quarantine paths are PathBuf joins under the run directory.
    let qdir = recovery_quarantine_dir(&run_dir);
    assert_eq!(qdir, PathBuf::from(&run_dir).join("recovery-quarantine"));
    let sample_sha = "a".repeat(64);
    let qpath = quarantine_tail_path(&run_dir, &sample_sha);
    assert_eq!(
        qpath,
        PathBuf::from(&run_dir)
            .join("recovery-quarantine")
            .join(format!("{sample_sha}.tail"))
    );

    // Torn tail: quarantine content-addressed suffix, truncate, keep valid prefix.
    let prefix = std::fs::read(&path).unwrap();
    let prefix_len = prefix.len() as u64;
    let torn = br#"{"seq":1,"timestamp_ms":1001,"run_id":"torn"#;
    let mut bytes = prefix.clone();
    bytes.extend_from_slice(torn);
    std::fs::write(&path, &bytes).unwrap();
    let original_with_torn = std::fs::read(&path).unwrap();

    let report = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("torn-tail recovery");
    let JournalRecoveryAction::TornTailQuarantined {
        quarantine_sha256,
        quarantine_path,
        removed_bytes,
        last_validated_offset,
    } = report.action
    else {
        panic!("expected TornTailQuarantined, got {:?}", report.action);
    };
    assert_eq!(last_validated_offset, prefix_len);
    assert_eq!(removed_bytes, torn.len() as u64);
    assert_eq!(
        quarantine_path,
        quarantine_tail_path(&run_dir, &quarantine_sha256)
    );
    assert!(quarantine_path.is_file());
    assert_eq!(std::fs::read(&quarantine_path).unwrap(), torn);

    let after = std::fs::read(&path).unwrap();
    assert!(
        after.starts_with(&prefix),
        "valid prefix must survive on this platform"
    );
    assert!(
        !after.windows(torn.len()).any(|w| w == torn),
        "torn suffix must leave the journal"
    );
    assert!(report.recovery_record_appended);

    // Quarantine failure (parent path is a file) must leave journal untouched.
    let nested = run_dir.join("nested-fail");
    std::fs::create_dir_all(&nested).unwrap();
    let fail_path = nested.join("journal.jsonl");
    std::fs::write(&fail_path, &original_with_torn).unwrap();
    let q_poison = recovery_quarantine_dir(&nested);
    std::fs::write(&q_poison, b"not-a-directory").unwrap();
    let before = std::fs::read(&fail_path).unwrap();
    let err = recover_journal(
        &fail_path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect_err("quarantine must fail");
    assert!(
        err.to_string().to_lowercase().contains("quarantine")
            || err.to_string().to_lowercase().contains("directory")
            || err.to_string().to_lowercase().contains("not a directory")
            || err.to_string().contains("File exists")
            || err.to_string().contains("AlreadyExists"),
        "unexpected quarantine failure wording: {err}"
    );
    assert_eq!(
        std::fs::read(&fail_path).unwrap(),
        before,
        "quarantine failure must leave journal byte-for-byte intact"
    );

    // Interior newline-terminated corruption still fails closed with no mutation.
    let corrupt_path = run_dir.join("interior.jsonl");
    let mut corrupt = prefix.clone();
    corrupt.extend_from_slice(b"{not-json}\n");
    std::fs::write(&corrupt_path, &corrupt).unwrap();
    let before_corrupt = std::fs::read(&corrupt_path).unwrap();
    let cerr = recover_journal(
        &corrupt_path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect_err("interior corrupt");
    assert_eq!(cerr.code(), WorkflowErrorCode::JournalCorrupt);
    assert_eq!(std::fs::read(&corrupt_path).unwrap(), before_corrupt);
}
