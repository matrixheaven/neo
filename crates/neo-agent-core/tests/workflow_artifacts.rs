//! Immutable artifacts and canonical final-result persistence (Task 7).

use std::sync::Arc;
use std::time::Duration;

use neo_agent_core::AgentTokenUsage;
use neo_agent_core::workflow::journal::{
    JournalEnvelope, JournalPayload, JournalV2Writer, collect_journal_v2,
};
use neo_agent_core::workflow::{
    ArtifactKind, ArtifactStore, ArtifactValue, FINAL_RESULT_LOGICAL_NAME, FinalResultBody,
    WorkflowErrorCode, WorkflowId, WorkflowInvocationKind, WorkflowInvocationOutcome,
    WorkflowLaunchRequest, WorkflowLimits, WorkflowOutcomeStatus, WorkflowPhase, WorkflowRuntime,
    WorkflowState, artifacts_dir,
};
use sha2::{Digest, Sha256};

fn launch_request(name: &str) -> WorkflowLaunchRequest {
    WorkflowLaunchRequest {
        name: name.to_owned(),
        description: "artifact test".to_owned(),
        phases: vec![WorkflowPhase {
            id: "work".to_owned(),
            description: "work".to_owned(),
        }],
        script: "return { ok = true }".to_owned(),
        args: serde_json::json!({}),
        launch_source: "/workflow".to_owned(),
        parent_run_id: None,
        output_schema: None,
        display_name: None,
        input_schema: None,
        definition_origin: None,
        inline_unsaved: false,
    }
}

fn limits_small_inline() -> WorkflowLimits {
    WorkflowLimits {
        task_output_page_bytes: 64,
        artifact_record_bytes: 1024 * 1024,
        ..WorkflowLimits::default()
    }
}

async fn wait_running(handle: &neo_agent_core::workflow::WorkflowHandle) {
    for _ in 0..200 {
        if handle.snapshot().await.state == WorkflowState::Running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run did not become Running");
}

async fn wait_terminal(handle: &neo_agent_core::workflow::WorkflowHandle) {
    for _ in 0..400 {
        if handle.snapshot().await.state.is_terminal() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run did not become terminal");
}

#[tokio::test]
async fn artifact_is_visible_only_after_durable_commit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = WorkflowRuntime::default();
    let gate = Arc::new(tokio::sync::Notify::new());
    let gate_run = Arc::clone(&gate);
    runtime
        .bind_runner(move |handle, _meta, _session| {
            let gate = Arc::clone(&gate_run);
            async move {
                gate.notified().await;
                handle
                    .persist_canonical_final_result(serde_json::json!({"ok": true}), None)
                    .await?;
                Ok(())
            }
        })
        .expect("bind runner");

    let handle = runtime
        .create_run(dir.path(), launch_request("artifact-vis"))
        .await
        .expect("create");
    runtime
        .start_worker(&handle.run_id)
        .await
        .expect("start worker");
    wait_running(&handle).await;

    let run_dir = neo_agent_core::workflow::run_dir(dir.path(), &handle.run_id);
    let store = ArtifactStore::open(&run_dir, handle.run_id.clone()).expect("open store");
    let staged = store
        .stage(
            &WorkflowLimits::default(),
            "plan",
            ArtifactKind::Text,
            &ArtifactValue::Text("research plan body".to_owned()),
            None,
        )
        .expect("stage bytes");

    // Bytes durable on disk, but not journal-visible yet.
    let path = artifacts_dir(&run_dir).join(&staged.sha256);
    assert!(path.is_file(), "staged content-addressed file must exist");
    assert!(
        handle.list_artifacts().await.expect("list").is_empty(),
        "artifact must not be visible before ArtifactCommitted"
    );
    let missing = handle
        .get_artifact(&staged.artifact_id)
        .await
        .expect_err("get before commit");
    assert_eq!(missing.code(), WorkflowErrorCode::ArtifactMissing);

    // Durable commit: stage (idempotent content) + journal + mark visible.
    let committed = handle
        .commit_artifact(
            "plan",
            ArtifactKind::Text,
            ArtifactValue::Text("research plan body".to_owned()),
            None,
        )
        .await
        .expect("commit");
    assert_eq!(committed.sha256, staged.sha256);
    assert_eq!(committed.logical_name, "plan");
    assert_eq!(committed.version, 1);

    let listed = handle.list_artifacts().await.expect("list after commit");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].artifact_id, committed.artifact_id);

    let content = handle
        .get_artifact(&committed.artifact_id)
        .await
        .expect("get after commit");
    assert_eq!(content.bytes, b"research plan body");
    assert_eq!(content.metadata.version, 1);

    // Journal ends with ArtifactCommitted after file write.
    let journal_path = run_dir.join("journal.jsonl");
    let envelopes = collect_journal_v2(&journal_path, Some(&handle.run_id)).expect("journal");
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.payload,
            JournalPayload::ArtifactCommitted {
                sha256,
                logical_name: Some(name),
                ..
            } if sha256 == &committed.sha256 && name == "plan"
        )),
        "ArtifactCommitted must be durable in the journal"
    );

    // Path escape via logical name must fail closed.
    let bad_name = handle
        .commit_artifact(
            "../escape",
            ArtifactKind::Text,
            ArtifactValue::Text("nope".to_owned()),
            None,
        )
        .await
        .expect_err("path-like logical name rejected");
    assert_eq!(bad_name.code(), WorkflowErrorCode::InvalidInput);

    gate.notify_one();
    wait_terminal(&handle).await;
}

#[tokio::test]
async fn oversized_final_result_uses_artifact_without_losing_usage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = WorkflowRuntime::new(limits_small_inline());
    let done = Arc::new(tokio::sync::Notify::new());
    let done_run = Arc::clone(&done);

    runtime
        .bind_runner(move |handle, _meta, _session| {
            let done = Arc::clone(&done_run);
            async move {
                let outcome = handle
                    .invoke(
                        0,
                        WorkflowInvocationKind::Delegate,
                        serde_json::json!({"task": "collect"}),
                        true,
                        |_| async {
                            WorkflowInvocationOutcome {
                                ok: true,
                                status: WorkflowOutcomeStatus::Completed,
                                summary: "collected".to_owned(),
                                interruption: None,
                                details: serde_json::json!({}),
                                actual_usage: Some(AgentTokenUsage {
                                    input_tokens: 41,
                                    output_tokens: 17,
                                    input_cache_read_tokens: 3,
                                    input_cache_write_tokens: 2,
                                }),
                                child_refs: vec![neo_agent_core::workflow::WorkflowChildRef {
                                    kind: "task".to_owned(),
                                    id: "task_collect".to_owned(),
                                }],
                            }
                        },
                    )
                    .await
                    .expect("invoke");
                assert!(outcome.ok);
                assert_eq!(outcome.actual_usage.unwrap().input_tokens, 41);

                // Larger than task_output_page_bytes (64) → artifact ref.
                let large = serde_json::json!({
                    "report": "x".repeat(200),
                    "ok": true,
                });
                let final_result = handle
                    .persist_canonical_final_result(large, None)
                    .await
                    .expect("persist final");
                match &final_result.body {
                    FinalResultBody::Artifact {
                        logical_name,
                        byte_len,
                        ..
                    } => {
                        assert_eq!(logical_name, FINAL_RESULT_LOGICAL_NAME);
                        assert!(*byte_len > 64);
                    }
                    FinalResultBody::Inline { .. } => {
                        panic!("oversized final result must use artifact indirection")
                    }
                }
                // Usage stays on the canonical result surface.
                assert_eq!(final_result.actual_usage.map(|u| u.input_tokens), Some(41));
                done.notify_one();
                Ok(())
            }
        })
        .expect("bind");

    let handle = runtime
        .create_run(dir.path(), launch_request("final-oversized"))
        .await
        .expect("create");
    runtime.start_worker(&handle.run_id).await.expect("start");
    done.notified().await;
    wait_terminal(&handle).await;

    let output = handle.output().await.expect("output");
    assert_eq!(output.state, WorkflowState::Completed);
    assert_eq!(output.actual_usage.map(|u| u.input_tokens), Some(41));
    assert_eq!(output.actual_usage.map(|u| u.output_tokens), Some(17));
    assert_eq!(output.terminal_reason.as_deref(), Some("worker completed"));

    let final_result = output.final_result.expect("final result on output");
    match final_result.body {
        FinalResultBody::Artifact {
            artifact_id,
            logical_name,
            ..
        } => {
            assert_eq!(logical_name, FINAL_RESULT_LOGICAL_NAME);
            let content = handle
                .get_artifact(&artifact_id)
                .await
                .expect("load final artifact");
            let value: serde_json::Value =
                serde_json::from_slice(&content.bytes).expect("json artifact");
            assert_eq!(value["ok"], true);
            assert_eq!(value["report"].as_str().unwrap().len(), 200);
        }
        FinalResultBody::Inline { .. } => panic!("expected artifact body"),
    }

    // Reports are separate and must not become a synthetic final result.
    assert!(
        output.reports.is_empty(),
        "final result owner is the top-level return, not reports"
    );

    // ArtifactCommitted precedes FinalResultRecorded in the journal.
    let run_dir = neo_agent_core::workflow::run_dir(dir.path(), &handle.run_id);
    let envelopes =
        collect_journal_v2(&run_dir.join("journal.jsonl"), Some(&handle.run_id)).expect("journal");
    let art_seq = envelopes.iter().find_map(|e| match &e.payload {
        JournalPayload::ArtifactCommitted {
            logical_name: Some(name),
            ..
        } if name == FINAL_RESULT_LOGICAL_NAME => Some(e.seq),
        _ => None,
    });
    let final_seq = envelopes.iter().find_map(|e| match &e.payload {
        JournalPayload::FinalResultRecorded { metadata }
            if metadata.artifact_id.is_some() && metadata.value.is_none() =>
        {
            Some(e.seq)
        }
        _ => None,
    });
    assert!(art_seq.is_some() && final_seq.is_some());
    assert!(
        art_seq.unwrap() < final_seq.unwrap(),
        "ArtifactCommitted must precede FinalResultRecorded"
    );
}

#[tokio::test]
async fn corrupt_or_missing_artifact_is_typed_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = WorkflowRuntime::default();
    let gate = Arc::new(tokio::sync::Notify::new());
    let gate_run = Arc::clone(&gate);
    runtime
        .bind_runner(move |handle, _meta, _session| {
            let gate = Arc::clone(&gate_run);
            async move {
                gate.notified().await;
                handle
                    .persist_canonical_final_result(serde_json::json!({"ok": true}), None)
                    .await?;
                Ok(())
            }
        })
        .expect("bind");

    let handle = runtime
        .create_run(dir.path(), launch_request("artifact-integrity"))
        .await
        .expect("create");
    runtime.start_worker(&handle.run_id).await.expect("start");
    wait_running(&handle).await;

    let committed = handle
        .commit_artifact(
            "evidence",
            ArtifactKind::Json,
            ArtifactValue::Json(serde_json::json!({"n": 1})),
            None,
        )
        .await
        .expect("commit");

    let run_dir = neo_agent_core::workflow::run_dir(dir.path(), &handle.run_id);
    let artifact_path = artifacts_dir(&run_dir).join(&committed.sha256);

    // Missing file after journal commit → typed missing error (never empty content).
    std::fs::remove_file(&artifact_path).expect("remove artifact bytes");
    let missing = handle
        .get_artifact(&committed.artifact_id)
        .await
        .expect_err("missing");
    assert_eq!(missing.code(), WorkflowErrorCode::ArtifactMissing);

    // Recreate with wrong bytes → typed corrupt error.
    std::fs::write(&artifact_path, b"tampered-not-matching-digest").expect("write corrupt");
    let corrupt = handle
        .get_artifact(&committed.artifact_id)
        .await
        .expect_err("corrupt");
    assert_eq!(corrupt.code(), WorkflowErrorCode::ArtifactCorrupt);

    // Direct store read_range also typed.
    let store = ArtifactStore::open(&run_dir, handle.run_id.clone()).expect("open");
    // Rehydrate membership from journal without trusting FS.
    let envelopes =
        collect_journal_v2(&run_dir.join("journal.jsonl"), Some(&handle.run_id)).expect("scan");
    let mut store = store;
    store
        .rehydrate_from_envelopes(&envelopes)
        .expect("rehydrate membership");
    let range_err = store
        .read_range(&committed.artifact_id, 0, 16)
        .expect_err("range corrupt");
    assert_eq!(range_err.code(), WorkflowErrorCode::ArtifactCorrupt);

    // Completely unknown id.
    let unknown_sha = format!("{:x}", Sha256::digest(b"never-written"));
    let unknown_id =
        neo_agent_core::workflow::WorkflowArtifactId::new(handle.run_id.clone(), unknown_sha)
            .unwrap();
    let unknown = handle.get_artifact(&unknown_id).await.expect_err("unknown");
    assert_eq!(unknown.code(), WorkflowErrorCode::ArtifactMissing);

    gate.notify_one();
    wait_terminal(&handle).await;
}

#[tokio::test]
async fn final_result_is_not_synthesized_from_reports() {
    // Guard: Completed without FinalResultRecorded fails closed (existing recovery);
    // reports alone never become the final result owner.
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = WorkflowRuntime::default();
    runtime
        .bind_runner(|handle, _meta, _session| async move {
            // Emit a report-like intermediate value without recording final result.
            let _ = handle;
            // Missing final result → Failed(missing_final_result), not report synthesis.
            Ok(())
        })
        .expect("bind");

    let handle = runtime
        .create_run(dir.path(), launch_request("no-synth"))
        .await
        .expect("create");
    runtime.start_worker(&handle.run_id).await.expect("start");
    wait_terminal(&handle).await;

    let snap = handle.snapshot().await;
    assert_eq!(snap.state, WorkflowState::Failed);
    assert_eq!(
        snap.terminal_reason.as_deref(),
        Some("missing_final_result")
    );
    let output = handle.output().await.expect("output");
    assert!(output.final_result.is_none());
}

/// Stage-without-journal leaves only an orphan file (not listable).
#[tokio::test]
async fn staged_orphan_is_not_listed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_id = WorkflowId::generate();
    let run_dir = dir.path().join("workflows").join(run_id.as_str());
    std::fs::create_dir_all(&run_dir).unwrap();
    let mut store = ArtifactStore::open(&run_dir, run_id.clone()).unwrap();
    let staged = store
        .stage(
            &WorkflowLimits::default(),
            "orphan",
            ArtifactKind::Text,
            &ArtifactValue::Text("orphan-bytes".into()),
            None,
        )
        .unwrap();
    assert!(store.list_metadata().is_empty());
    assert!(artifacts_dir(&run_dir).join(&staged.sha256).is_file());

    // Journal commit is what makes it visible.
    let journal_path = run_dir.join("journal.jsonl");
    let mut writer = JournalV2Writer::open(&journal_path, run_id.clone()).unwrap();
    let env = JournalEnvelope::new(
        0,
        1,
        run_id.clone(),
        JournalPayload::ArtifactCommitted {
            artifact_id: staged.artifact_id.clone(),
            sha256: staged.sha256.clone(),
            byte_len: staged.byte_len,
            media_type: Some(staged.media_type.clone()),
            logical_name: Some(staged.logical_name.clone()),
        },
    );
    writer
        .append(&env, &WorkflowLimits::default())
        .expect("append");
    drop(writer);

    let envelopes = collect_journal_v2(&journal_path, Some(&run_id)).unwrap();
    store.rehydrate_from_envelopes(&envelopes).unwrap();
    assert_eq!(store.list_metadata().len(), 1);
    let content = store.get(&staged.artifact_id).unwrap();
    assert_eq!(content.bytes, b"orphan-bytes");
}

/// Platform artifact contract: content-addressed PathBuf layout, atomic
/// create-new replace semantics, integrity revalidation, symlink/reparse
/// rejection, and path-escape logical names fail closed.
///
/// Native evidence target for Task 25 (macOS / Linux / Windows).
#[test]
fn artifact_replace_and_integrity_are_platform_safe() {
    use std::path::PathBuf;

    let dir = tempfile::tempdir().expect("tempdir");
    let run_id = WorkflowId::generate();
    let run_dir = dir.path().join("workflows").join(run_id.as_str());
    std::fs::create_dir_all(&run_dir).unwrap();

    let store = ArtifactStore::open(&run_dir, run_id.clone()).expect("open store");
    let art_dir = store.artifacts_dir_path().to_path_buf();
    assert_eq!(art_dir, PathBuf::from(&run_dir).join("artifacts"));
    assert!(art_dir.is_dir());

    let payload = b"platform-artifact-body-v1";
    let staged = store
        .stage(
            &WorkflowLimits::default(),
            "platform-evidence",
            ArtifactKind::Text,
            &ArtifactValue::Text(String::from_utf8(payload.to_vec()).unwrap()),
            None,
        )
        .expect("stage");
    let content_path = art_dir.join(&staged.sha256);
    assert!(content_path.is_file(), "content-addressed file must exist");
    assert_eq!(std::fs::read(&content_path).unwrap(), payload);

    // Idempotent re-stage of identical bytes is safe (no clobber of wrong content).
    let again = store
        .stage(
            &WorkflowLimits::default(),
            "platform-evidence",
            ArtifactKind::Text,
            &ArtifactValue::Text(String::from_utf8(payload.to_vec()).unwrap()),
            None,
        )
        .expect("idempotent stage");
    assert_eq!(again.sha256, staged.sha256);
    assert_eq!(std::fs::read(&content_path).unwrap(), payload);

    // Journal membership then integrity-validated read.
    let journal_path = run_dir.join("journal.jsonl");
    let mut writer = JournalV2Writer::open(&journal_path, run_id.clone()).unwrap();
    let env = JournalEnvelope::new(
        0,
        1,
        run_id.clone(),
        JournalPayload::ArtifactCommitted {
            artifact_id: staged.artifact_id.clone(),
            sha256: staged.sha256.clone(),
            byte_len: staged.byte_len,
            media_type: Some(staged.media_type.clone()),
            logical_name: Some(staged.logical_name.clone()),
        },
    );
    writer
        .append(&env, &WorkflowLimits::default())
        .expect("append ArtifactCommitted");
    drop(writer);

    let envelopes = collect_journal_v2(&journal_path, Some(&run_id)).unwrap();
    let mut store = store;
    store.rehydrate_from_envelopes(&envelopes).unwrap();
    let content = store.get(&staged.artifact_id).expect("validated get");
    assert_eq!(content.bytes, payload);

    // Tamper after commit → typed corrupt (never silent wrong bytes).
    std::fs::write(&content_path, b"tampered-platform-bytes").unwrap();
    let corrupt = store.get(&staged.artifact_id).expect_err("tamper");
    assert_eq!(corrupt.code(), WorkflowErrorCode::ArtifactCorrupt);

    // Missing file after commit → typed missing.
    std::fs::remove_file(&content_path).unwrap();
    let missing = store.get(&staged.artifact_id).expect_err("missing");
    assert_eq!(missing.code(), WorkflowErrorCode::ArtifactMissing);

    // Path-like logical names fail closed (no separator-based escape).
    for bad in ["../escape", "a/b", "a\\b", "..", "name/with/slash"] {
        let err = store
            .stage(
                &WorkflowLimits::default(),
                bad,
                ArtifactKind::Text,
                &ArtifactValue::Text("nope".into()),
                None,
            )
            .expect_err("path-like logical name rejected");
        assert_eq!(
            err.code(),
            WorkflowErrorCode::InvalidInput,
            "unexpected code for {bad:?}: {err}"
        );
    }

    // Symlink at content-addressed path is not accepted as a regular artifact.
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let outside = dir.path().join("outside-artifact.bin");
        std::fs::write(&outside, payload).unwrap();
        // Restore a path that matches the digest name but is a symlink.
        let _ = std::fs::remove_file(&content_path);
        symlink(&outside, &content_path).expect("symlink artifact path");
        let link_err = store
            .get(&staged.artifact_id)
            .expect_err("symlink artifact rejected");
        assert_eq!(
            link_err.code(),
            WorkflowErrorCode::ArtifactCorrupt,
            "symlink must be treated as corrupt: {link_err}"
        );
        // Clean for any later assertions.
        let _ = std::fs::remove_file(&content_path);
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_file;
        let outside = dir.path().join("outside-artifact.bin");
        std::fs::write(&outside, payload).unwrap();
        let _ = std::fs::remove_file(&content_path);
        match symlink_file(&outside, &content_path) {
            Ok(()) => {
                let link_err = store
                    .get(&staged.artifact_id)
                    .expect_err("symlink artifact rejected on Windows");
                assert_eq!(link_err.code(), WorkflowErrorCode::ArtifactCorrupt);
                let _ = std::fs::remove_file(&content_path);
            }
            Err(e) => {
                eprintln!(
                    "windows symlink unavailable on this host ({e}); atomic write + integrity verified"
                );
            }
        }
    }

    // Fresh content-addressed write after cleanup remains durable regular file.
    let restored = store
        .stage(
            &WorkflowLimits::default(),
            "platform-evidence",
            ArtifactKind::Text,
            &ArtifactValue::Text(String::from_utf8(payload.to_vec()).unwrap()),
            None,
        )
        .expect("re-stage after cleanup");
    assert_eq!(restored.sha256, staged.sha256);
    assert!(art_dir.join(&restored.sha256).is_file());
    let meta = std::fs::symlink_metadata(art_dir.join(&restored.sha256)).unwrap();
    assert!(meta.is_file(), "restored artifact must be a regular file");
    assert!(!meta.file_type().is_symlink());
}
