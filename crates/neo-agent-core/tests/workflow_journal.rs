use neo_agent_core::workflow::{
    JournalRecord, JournalWriter, WorkflowActor, WorkflowId, WorkflowInvocationKind,
    WorkflowInvocationOutcome, WorkflowLimits, WorkflowOutcomeStatus, WorkflowPhase,
    WorkflowRunMetadata, WorkflowState, canonical_input_hash, find_incomplete_invocations,
    journal_path, read_journal, read_run_metadata, run_dir, write_run_metadata,
};
use serde_json::json;

fn test_limits() -> WorkflowLimits {
    WorkflowLimits::default()
}

fn sample_metadata(run_id: &str) -> WorkflowRunMetadata {
    WorkflowRunMetadata {
        run_id: WorkflowId(run_id.to_owned()),
        parent_run_id: None,
        name: "test-workflow".to_owned(),
        description: "A test workflow".to_owned(),
        phases: vec![WorkflowPhase {
            id: "inspect".to_owned(),
            description: "Inspect things".to_owned(),
        }],
        script: "neo.phase('inspect')".to_owned(),
        script_sha256: "abc123".to_owned(),
        args: json!({}),
        launch_source: "/workflow".to_owned(),
        journal_format_version: 1,
    }
}

fn state_changed(seq: u64, prev: WorkflowState, new: WorkflowState) -> JournalRecord {
    JournalRecord::StateChanged {
        seq,
        timestamp_ms: 1000 + seq,
        previous: prev,
        new,
        reason: "test".to_owned(),
        actor: WorkflowActor::Runtime,
    }
}

fn invocation_started(seq: u64, id: &str, call_index: u64) -> JournalRecord {
    JournalRecord::InvocationStarted {
        seq,
        timestamp_ms: 1000 + seq,
        invocation_id: id.to_owned(),
        call_index,
        kind: WorkflowInvocationKind::Delegate,
        canonical_input: json!({"task": "test"}),
        canonical_input_hash: canonical_input_hash(&json!({"task": "test"})),
    }
}

fn invocation_finished(seq: u64, id: &str, ok: bool) -> JournalRecord {
    JournalRecord::InvocationFinished {
        seq,
        timestamp_ms: 1000 + seq,
        invocation_id: id.to_owned(),
        outcome: WorkflowInvocationOutcome {
            ok,
            status: if ok {
                WorkflowOutcomeStatus::Completed
            } else {
                WorkflowOutcomeStatus::Failed
            },
            summary: "done".to_owned(),
            details: json!({}),
            actual_usage: None,
            child_refs: vec![],
        },
    }
}

#[test]
fn journal_writes_and_reads_append_only_records() {
    let dir = tempfile::tempdir().unwrap();
    let jpath = dir.path().join("journal.jsonl");
    let limits = test_limits();

    let mut writer = JournalWriter::open(&jpath).unwrap();
    writer
        .append(
            &state_changed(0, WorkflowState::Running, WorkflowState::Running),
            &limits,
        )
        .unwrap();
    writer
        .append(&invocation_started(1, "inv_1", 0), &limits)
        .unwrap();
    writer
        .append(&invocation_finished(2, "inv_1", true), &limits)
        .unwrap();

    let records = read_journal(&jpath).unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].seq(), 0);
    assert_eq!(records[1].seq(), 1);
    assert_eq!(records[2].seq(), 2);

    // Reopen and continue appending
    let mut writer2 = JournalWriter::open(&jpath).unwrap();
    assert_eq!(writer2.next_seq(), 3);
    writer2
        .append(
            &state_changed(3, WorkflowState::Running, WorkflowState::Completed),
            &limits,
        )
        .unwrap();

    let records2 = read_journal(&jpath).unwrap();
    assert_eq!(records2.len(), 4);
}

#[test]
fn incomplete_invocation_is_detected_without_reexecution() {
    let dir = tempfile::tempdir().unwrap();
    let jpath = dir.path().join("journal.jsonl");
    let limits = test_limits();

    let mut writer = JournalWriter::open(&jpath).unwrap();
    writer
        .append(
            &state_changed(0, WorkflowState::Running, WorkflowState::Running),
            &limits,
        )
        .unwrap();
    writer
        .append(&invocation_started(1, "inv_1", 0), &limits)
        .unwrap();
    writer
        .append(&invocation_finished(2, "inv_1", true), &limits)
        .unwrap();
    // inv_2 started but never finished (host exit)
    writer
        .append(&invocation_started(3, "inv_2", 1), &limits)
        .unwrap();

    let records = read_journal(&jpath).unwrap();
    let incomplete = find_incomplete_invocations(&records);
    assert_eq!(incomplete.len(), 1);
    assert_eq!(incomplete[0].invocation_id, "inv_2");
    assert_eq!(incomplete[0].call_index, 1);
    assert_eq!(incomplete[0].kind, WorkflowInvocationKind::Delegate);
}

#[test]
fn canonical_input_hash_is_stable_across_key_order() {
    let a = json!({"b": 1, "a": 2, "c": {"z": true, "y": false}});
    let b = json!({"a": 2, "c": {"y": false, "z": true}, "b": 1});
    assert_eq!(canonical_input_hash(&a), canonical_input_hash(&b));

    let c = json!({"a": 3, "b": 1});
    assert_ne!(canonical_input_hash(&a), canonical_input_hash(&c));
}

#[test]
fn journal_rejects_malformed_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let jpath = dir.path().join("journal.jsonl");

    // Write records with a sequence gap
    let r0 = serde_json::to_string(&state_changed(
        0,
        WorkflowState::Running,
        WorkflowState::Running,
    ))
    .unwrap();
    let r2 = serde_json::to_string(&state_changed(
        2,
        WorkflowState::Running,
        WorkflowState::Completed,
    ))
    .unwrap();
    std::fs::write(&jpath, format!("{r0}\n{r2}\n")).unwrap();

    let err = read_journal(&jpath).unwrap_err();
    assert!(err.to_string().contains("sequence gap"));
}

#[test]
fn journal_rejects_oversized_record() {
    let dir = tempfile::tempdir().unwrap();
    let jpath = dir.path().join("journal.jsonl");

    let limits = WorkflowLimits {
        journal_record_bytes: 100, // tiny limit for test
        ..WorkflowLimits::default()
    };

    let mut writer = JournalWriter::open(&jpath).unwrap();
    let big_record = JournalRecord::InvocationStarted {
        seq: 0,
        timestamp_ms: 1000,
        invocation_id: "inv_big".to_owned(),
        call_index: 0,
        kind: WorkflowInvocationKind::Delegate,
        canonical_input: json!({"task": "x".repeat(200)}),
        canonical_input_hash: "hash".to_owned(),
    };

    let err = writer.append(&big_record, &limits).unwrap_err();
    assert!(err.to_string().contains("exceeds limit"));
}

#[test]
fn journal_reservation_prevents_exceeding_total() {
    let dir = tempfile::tempdir().unwrap();
    let jpath = dir.path().join("journal.jsonl");

    let limits = WorkflowLimits {
        journal_total_bytes: 500, // tiny total
        journal_record_bytes: 16 * 1024 * 1024,
        ..WorkflowLimits::default()
    };

    let mut writer = JournalWriter::open(&jpath).unwrap();
    // The reservation check should fail because the total is too small
    assert!(!writer.has_reservation_for_invocation(&limits));
}

#[test]
fn run_metadata_round_trips_through_pathbuf_directory() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = dir.path();
    let run_id = WorkflowId("run_abc123".to_owned());
    let limits = test_limits();

    let rdir = run_dir(session_dir, &run_id);
    let metadata = sample_metadata("run_abc123");

    write_run_metadata(&rdir, &metadata, &limits).unwrap();
    let loaded = read_run_metadata(&rdir).unwrap();
    assert_eq!(loaded, metadata);

    let jpath = journal_path(session_dir, &run_id);
    assert!(jpath.ends_with("workflows/run_abc123/journal.jsonl"));
}

#[test]
fn run_metadata_rejects_oversized_json() {
    let dir = tempfile::tempdir().unwrap();
    let rdir = dir.path().join("run");

    let limits = WorkflowLimits {
        journal_record_bytes: 100, // tiny
        ..WorkflowLimits::default()
    };

    let metadata = sample_metadata("run_big");
    let err = write_run_metadata(&rdir, &metadata, &limits).unwrap_err();
    assert!(err.to_string().contains("exceeds"));
}
