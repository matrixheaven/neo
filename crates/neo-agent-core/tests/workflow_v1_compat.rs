//! Read-only V1 workflow fixture decode.
//!
//! Fixtures under `tests/fixtures/workflow_v1/` are byte-for-byte captures from
//! deterministic test construction of the pre-V2 metadata/journal format.
//! They must remain decodable without a V1 writer path.

use neo_agent_core::workflow::{
    WorkflowErrorCode, WorkflowId, WorkflowRunMetadata, WorkflowRuntime, WorkflowState,
    find_incomplete_invocations, read_journal, read_run_metadata,
};
use std::path::{Path, PathBuf};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workflow_v1")
}

fn completed_dir() -> PathBuf {
    fixture_root().join("completed")
}

fn incomplete_dir() -> PathBuf {
    fixture_root().join("incomplete")
}

#[test]
fn v1_fixtures_decode_current_format() {
    let completed = completed_dir();
    let incomplete = incomplete_dir();

    assert!(
        completed.join("run.json").is_file(),
        "completed V1 run.json fixture missing"
    );
    assert!(
        completed.join("journal.jsonl").is_file(),
        "completed V1 journal fixture missing"
    );
    assert!(
        incomplete.join("run.json").is_file(),
        "incomplete V1 run.json fixture missing"
    );
    assert!(
        incomplete.join("journal.jsonl").is_file(),
        "incomplete V1 journal fixture missing"
    );

    let completed_meta = read_run_metadata(&completed).expect("completed V1 metadata decodes");
    let incomplete_meta = read_run_metadata(&incomplete).expect("incomplete V1 metadata decodes");
    assert_eq!(completed_meta, incomplete_meta);
    assert_v1_metadata(&completed_meta);

    let completed_journal =
        read_journal(&completed.join("journal.jsonl")).expect("completed V1 journal decodes");
    assert_eq!(completed_journal.len(), 4);
    assert!(
        find_incomplete_invocations(&completed_journal).is_empty(),
        "completed fixture must have no incomplete invocations"
    );
    assert!(matches!(
        completed_journal.last(),
        Some(neo_agent_core::workflow::JournalRecord::StateChanged {
            new: WorkflowState::Completed,
            ..
        })
    ));

    let incomplete_journal =
        read_journal(&incomplete.join("journal.jsonl")).expect("incomplete V1 journal decodes");
    assert_eq!(incomplete_journal.len(), 2);
    let incomplete_inv = find_incomplete_invocations(&incomplete_journal);
    assert_eq!(incomplete_inv.len(), 1);
    assert_eq!(incomplete_inv[0].invocation_id, "inv_open");

    // Byte stability: re-serializing metadata must not invent V2-only fields.
    let round_trip = serde_json::to_string_pretty(&completed_meta).unwrap() + "\n";
    let on_disk = std::fs::read_to_string(completed.join("run.json")).unwrap();
    assert_eq!(
        on_disk, round_trip,
        "V1 run.json fixture must match current metadata serde exactly"
    );

    assert_fixture_bytes_unchanged(&completed);
    assert_fixture_bytes_unchanged(&incomplete);
}

fn assert_v1_metadata(meta: &WorkflowRunMetadata) {
    assert_eq!(
        meta.run_id,
        WorkflowId("wf_00000000000000000000000000000001".to_owned())
    );
    assert_eq!(meta.journal_format_version, 1);
    assert_eq!(meta.name, "test-workflow");
    assert_eq!(meta.launch_source, "/workflow");
    assert!(meta.parent_run_id.is_none());
    assert_eq!(meta.phases.len(), 1);
    assert_eq!(meta.phases[0].id, "inspect");
}

fn assert_fixture_bytes_unchanged(dir: &Path) {
    // Reading fixtures must not require or create a writer side-effect.
    assert!(
        !dir.join("run.json.tmp").exists(),
        "decode must not leave temp writer artifacts"
    );
    let meta_bytes = std::fs::read(dir.join("run.json")).unwrap();
    let journal_bytes = std::fs::read(dir.join("journal.jsonl")).unwrap();
    assert!(
        !meta_bytes.is_empty() && !journal_bytes.is_empty(),
        "fixtures must be non-empty byte captures"
    );
    // Journal lines are newline-terminated JSONL.
    assert!(
        journal_bytes.ends_with(b"\n"),
        "journal fixture must end with newline"
    );
}

#[tokio::test]
async fn v1_nonterminal_resume_requires_linked_upgrade_without_append() {
    let fixture = incomplete_dir();
    let tmp = tempfile::tempdir().unwrap();
    let session_dir = tmp.path();
    let run_id = "wf_00000000000000000000000000000001";
    let run_dir = session_dir.join("workflows").join(run_id);
    std::fs::create_dir_all(&run_dir).unwrap();

    // Copy fixture bytes into a session workflows layout.
    for name in ["run.json", "journal.jsonl"] {
        std::fs::copy(fixture.join(name), run_dir.join(name)).unwrap();
    }
    let journal_before = std::fs::read(run_dir.join("journal.jsonl")).unwrap();
    let meta_before = std::fs::read(run_dir.join("run.json")).unwrap();

    let runtime = WorkflowRuntime::new(neo_agent_core::workflow::WorkflowLimits::default());
    let handles = runtime.rehydrate(session_dir).await.expect("rehydrate v1");
    assert_eq!(handles.len(), 1);
    let handle = &handles[0];
    assert_eq!(handle.run_id.as_str(), run_id);

    let snapshot = handle.snapshot().await;
    assert_eq!(
        snapshot.state,
        WorkflowState::Paused,
        "nonterminal V1 must project as paused host_exit without journal rewrite"
    );
    assert_eq!(snapshot.terminal_reason.as_deref(), Some("host_exit"));
    assert!(
        !snapshot.recovery_failure,
        "V1 read-only projection is not a recovery failure"
    );

    // Rehydrate must not append interrupted finishes or host_exit records.
    let journal_after_rehydrate = std::fs::read(run_dir.join("journal.jsonl")).unwrap();
    assert_eq!(
        journal_before, journal_after_rehydrate,
        "V1 rehydrate must not append any journal bytes"
    );
    assert_eq!(
        meta_before,
        std::fs::read(run_dir.join("run.json")).unwrap(),
        "V1 run.json must remain byte-stable"
    );

    // Same-ID resume is rejected; linked upgrade is the only writer path.
    let err = handle
        .resume(neo_agent_core::workflow::WorkflowActor::Human)
        .await
        .expect_err("V1 same-ID resume must fail");
    assert_eq!(err.code(), WorkflowErrorCode::InvalidOperation);
    assert!(
        err.to_string().contains("linked_upgrade_required"),
        "error must carry linked_upgrade_required: {err}"
    );

    let journal_after_resume = std::fs::read(run_dir.join("journal.jsonl")).unwrap();
    assert_eq!(
        journal_before, journal_after_resume,
        "failed resume must not append any V1 journal bytes"
    );

    // Incomplete invocation remains durable and un-finished (never relaunched).
    let records = read_journal(&run_dir.join("journal.jsonl")).unwrap();
    let incomplete = find_incomplete_invocations(&records);
    assert_eq!(incomplete.len(), 1);
    assert_eq!(incomplete[0].invocation_id, "inv_open");
}
