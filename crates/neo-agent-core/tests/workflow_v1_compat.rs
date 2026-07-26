//! Read-only V1 workflow fixture decode.
//!
//! Fixtures under `tests/fixtures/workflow_v1/` are byte-for-byte captures from
//! deterministic test construction of the pre-V2 metadata/journal format.
//! They must remain decodable without a V1 writer path.

use neo_agent_core::workflow::{
    WorkflowId, WorkflowRunMetadata, WorkflowState, find_incomplete_invocations, read_journal,
    read_run_metadata,
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
