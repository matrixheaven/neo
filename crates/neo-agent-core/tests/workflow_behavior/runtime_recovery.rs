use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use neo_agent_core::workflow::journal::{JournalEnvelope, JournalPayload, JournalWriter};
use neo_agent_core::workflow::{
    WorkflowActor, WorkflowChildKey, WorkflowChildKind, WorkflowFinalResultMetadata,
    WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowLimits, WorkflowOutcomeStatus,
    WorkflowRuntime, WorkflowState, canonical_input_hash, journal_path,
};

use super::runtime_lifecycle::{
    collect_journal, completed, create_run, create_running_run, wait_for_state,
};

#[tokio::test]
async fn manually_paused_run_rehydrates_without_host_exit_notification() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;
    handle.pause(WorkflowActor::Human).await.unwrap();
    assert_eq!(
        handle.snapshot().await.terminal_reason.as_deref(),
        Some("pause")
    );
    drop(handle);
    drop(runtime);

    let recovered = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = recovered.rehydrate(dir.path()).await.unwrap().remove(0);
    let snapshot = handle.snapshot().await;
    assert_eq!(snapshot.state, WorkflowState::Paused);
    assert_eq!(snapshot.terminal_reason.as_deref(), Some("pause"));
    assert!(
        recovered
            .notification_queue()
            .pending_for_session(dir.path())
            .is_empty()
    );
}

#[tokio::test]
async fn rehydration_keeps_verify_messages_out_of_latest_log_summary() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_running_run(&runtime, dir.path()).await;
    handle
        .invoke(
            0,
            WorkflowInvocationKind::Log,
            serde_json::json!({"message": "durable log"}),
            false,
            |_| async {
                WorkflowInvocationOutcome {
                    details: serde_json::json!({"message": "durable log"}),
                    ..completed("log recorded")
                }
            },
        )
        .await
        .unwrap();
    handle
        .invoke(
            1,
            WorkflowInvocationKind::Verify,
            serde_json::json!({"condition": true, "message": "verification passed"}),
            false,
            |_| async {
                WorkflowInvocationOutcome {
                    details: serde_json::json!({"message": "verification passed"}),
                    ..completed("verification passed")
                }
            },
        )
        .await
        .unwrap();
    handle.pause(WorkflowActor::Human).await.unwrap();
    drop(handle);
    drop(runtime);

    let recovered = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = recovered.rehydrate(dir.path()).await.unwrap().remove(0);

    assert_eq!(
        handle.snapshot().await.latest_log_summary.as_deref(),
        Some("durable log")
    );
}

#[tokio::test]
async fn replay_uses_matching_prefix_without_repeating_effect_then_starts_live() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_running_run(&runtime, dir.path()).await;
    let effects = Arc::new(AtomicUsize::new(0));
    handle
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            serde_json::json!({"task": "audit"}),
            true,
            {
                let effects = Arc::clone(&effects);
                move |_| async move {
                    effects.fetch_add(1, Ordering::AcqRel);
                    completed("audit")
                }
            },
        )
        .await
        .unwrap();
    drop(handle);
    drop(runtime);

    let recovered = WorkflowRuntime::new(WorkflowLimits::default());
    recovered
        .bind_runner({
            let effects = Arc::clone(&effects);
            move |handle, _metadata, _session_dir| {
                let effects = Arc::clone(&effects);
                async move {
                    handle
                        .invoke(
                            0,
                            WorkflowInvocationKind::Delegate,
                            serde_json::json!({"task": "audit"}),
                            true,
                            {
                                let effects = Arc::clone(&effects);
                                move |_| async move {
                                    effects.fetch_add(10, Ordering::AcqRel);
                                    completed("must replay")
                                }
                            },
                        )
                        .await?;
                    handle
                        .invoke(
                            1,
                            WorkflowInvocationKind::Delegate,
                            serde_json::json!({"task": "build"}),
                            true,
                            {
                                let effects = Arc::clone(&effects);
                                move |_| async move {
                                    effects.fetch_add(1, Ordering::AcqRel);
                                    completed("build")
                                }
                            },
                        )
                        .await?;
                    Ok(())
                }
            }
        })
        .unwrap();
    let recovered_handle = recovered.rehydrate(dir.path()).await.unwrap().remove(0);
    recovered_handle.resume(WorkflowActor::Human).await.unwrap();
    // Runner exits without final_result → Failed under canonical completion rules.
    wait_for_state(&recovered_handle, WorkflowState::Failed).await;
    assert_eq!(effects.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn replay_mismatch_starts_live_effect() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_running_run(&runtime, dir.path()).await;
    handle
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            serde_json::json!({"task": "old"}),
            true,
            |_| async { completed("old") },
        )
        .await
        .unwrap();
    drop(handle);
    drop(runtime);

    let effects = Arc::new(AtomicUsize::new(0));
    let recovered = WorkflowRuntime::new(WorkflowLimits::default());
    recovered
        .bind_runner({
            let effects = Arc::clone(&effects);
            move |handle, _metadata, _session_dir| {
                let effects = Arc::clone(&effects);
                async move {
                    handle
                        .invoke(
                            0,
                            WorkflowInvocationKind::Delegate,
                            serde_json::json!({"task": "edited"}),
                            true,
                            move |_| async move {
                                effects.fetch_add(1, Ordering::AcqRel);
                                completed("edited")
                            },
                        )
                        .await?;
                    Ok(())
                }
            }
        })
        .unwrap();
    let recovered_handle = recovered.rehydrate(dir.path()).await.unwrap().remove(0);
    recovered_handle.resume(WorkflowActor::Human).await.unwrap();
    // Runner exits without final_result → Failed under canonical completion rules.
    wait_for_state(&recovered_handle, WorkflowState::Failed).await;
    assert_eq!(effects.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn incomplete_invocation_is_interrupted_and_never_reexecuted() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_running_run(&runtime, dir.path()).await;
    let path = journal_path(dir.path(), &handle.run_id);
    let input = serde_json::json!({"task": "audit"});
    let existing = collect_journal(&path, None).unwrap();
    let next_seq = existing.last().map_or(0, |e| e.seq + 1);
    let run_id = existing
        .first()
        .map_or_else(|| handle.run_id.clone(), |e| e.run_id.clone());
    let mut writer =
        JournalWriter::open(&path, run_id.clone(), &WorkflowLimits::default()).unwrap();
    let started = JournalEnvelope::new(
        next_seq,
        2,
        run_id.clone(),
        JournalPayload::InvocationStarted {
            invocation_id: "inv_incomplete".to_owned(),
            call_index: 0,
            kind: WorkflowInvocationKind::Swarm,
            canonical_input: Some(input.clone()),
        },
    )
    .with_canonical_input_hash(canonical_input_hash(&input));
    writer.append(&started, &WorkflowLimits::default()).unwrap();
    let child_key = WorkflowChildKey::SwarmItem {
        swarm_id: "swarm_crash".to_owned(),
        item_id: "item_1".to_owned(),
    };
    for payload in [
        JournalPayload::ChildQueued {
            child_key: child_key.clone(),
            child_kind: WorkflowChildKind::SwarmItem,
            invocation_id: "inv_incomplete".to_owned(),
            phase_id: Some("build".to_owned()),
            title: Some("crash recovery".to_owned()),
            role: Some("reviewer".to_owned()),
        },
        JournalPayload::ChildStarted {
            child_key: child_key.clone(),
            agent_id: Some("agent_crash".to_owned()),
        },
    ] {
        let envelope = JournalEnvelope::new(writer.next_seq(), 3, run_id.clone(), payload);
        writer
            .append(&envelope, &WorkflowLimits::default())
            .unwrap();
    }
    drop(handle);
    drop(runtime);

    let effects = Arc::new(AtomicUsize::new(0));
    let recovered = WorkflowRuntime::new(WorkflowLimits::default());
    recovered
        .bind_runner({
            let effects = Arc::clone(&effects);
            move |handle, _metadata, _session_dir| {
                let effects = Arc::clone(&effects);
                async move {
                    let outcome = handle
                        .invoke(
                            0,
                            WorkflowInvocationKind::Swarm,
                            serde_json::json!({"task": "audit"}),
                            true,
                            move |_| async move {
                                effects.fetch_add(1, Ordering::AcqRel);
                                completed("unexpected retry")
                            },
                        )
                        .await?;
                    assert_eq!(outcome.status, WorkflowOutcomeStatus::Interrupted);
                    Ok(())
                }
            }
        })
        .unwrap();
    let recovered_handle = recovered.rehydrate(dir.path()).await.unwrap().remove(0);
    recovered_handle.resume(WorkflowActor::Human).await.unwrap();
    // Runner exits without final_result → Failed under canonical completion rules.
    wait_for_state(&recovered_handle, WorkflowState::Failed).await;
    assert_eq!(effects.load(Ordering::Acquire), 0);
    let recovered_records = collect_journal(&path, Some(&run_id)).unwrap();
    let child_finished = recovered_records
        .iter()
        .position(|record| {
            matches!(
                &record.payload,
                JournalPayload::ChildFinished {
                    child_key: finished_key,
                    agent_id: Some(agent_id),
                    status: WorkflowOutcomeStatus::Interrupted,
                    ..
                } if finished_key == &child_key && agent_id == "agent_crash"
            )
        })
        .expect("open swarm child must be durably interrupted");
    let invocation_finished = recovered_records
        .iter()
        .position(|record| {
            matches!(
                &record.payload,
                JournalPayload::InvocationFinished { invocation_id, .. }
                    if invocation_id == "inv_incomplete"
            )
        })
        .expect("incomplete invocation must be durably interrupted");
    assert!(child_finished < invocation_finished);
}

#[tokio::test]
async fn recovery_resolver_adopts_known_terminal_child_result() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;
    let path = journal_path(dir.path(), &handle.run_id);
    let input = serde_json::json!({"task": "audit"});
    let existing = collect_journal(&path, None).unwrap();
    let next_seq = existing.last().map_or(0, |e| e.seq + 1);
    let run_id = existing
        .first()
        .map_or_else(|| handle.run_id.clone(), |e| e.run_id.clone());
    let mut writer =
        JournalWriter::open(&path, run_id.clone(), &WorkflowLimits::default()).unwrap();
    let started = JournalEnvelope::new(
        next_seq,
        2,
        run_id.clone(),
        JournalPayload::InvocationStarted {
            invocation_id: "child_7".to_owned(),
            call_index: 0,
            kind: WorkflowInvocationKind::Delegate,
            canonical_input: Some(input.clone()),
        },
    )
    .with_canonical_input_hash(canonical_input_hash(&input));
    writer.append(&started, &WorkflowLimits::default()).unwrap();
    let child_key = WorkflowChildKey::DirectDelegate {
        invocation_id: "child_7".to_owned(),
    };
    for payload in [
        JournalPayload::ChildQueued {
            child_key: child_key.clone(),
            child_kind: WorkflowChildKind::Delegate,
            invocation_id: "child_7".to_owned(),
            phase_id: None,
            title: Some("audit".to_owned()),
            role: None,
        },
        JournalPayload::ChildStarted {
            child_key: child_key.clone(),
            agent_id: Some("agent_7".to_owned()),
        },
    ] {
        let envelope = JournalEnvelope::new(writer.next_seq(), 3, run_id.clone(), payload);
        writer
            .append(&envelope, &WorkflowLimits::default())
            .unwrap();
    }
    drop(handle);
    drop(runtime);

    let recovered = WorkflowRuntime::new(WorkflowLimits::default());
    recovered
        .bind_recovery_resolver(|invocation| async move {
            tokio::task::yield_now().await;
            (invocation.invocation_id == "child_7").then(|| completed("adopted child"))
        })
        .unwrap();
    recovered.rehydrate(dir.path()).await.unwrap();
    let recovered_records = collect_journal(&path, None).unwrap();
    assert!(recovered_records.iter().any(|record| {
        matches!(&record.payload, JournalPayload::InvocationFinished { invocation_id, outcome, .. }
            if invocation_id == "child_7" && outcome.summary == "adopted child")
    }));
    assert!(recovered_records.iter().any(|record| {
        matches!(
            &record.payload,
            JournalPayload::ChildFinished {
                child_key: finished_key,
                agent_id: Some(agent_id),
                status: WorkflowOutcomeStatus::Completed,
                ..
            } if finished_key == &child_key && agent_id == "agent_7"
        )
    }));
}

#[tokio::test]
async fn final_result_recovery_rejects_non_running_durable_state() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_running_run(&runtime, dir.path()).await;
    handle
        .record_final_result(WorkflowFinalResultMetadata {
            value: Some(serde_json::json!({"ok": true})),
            artifact_id: None,
            schema_revision: None,
        })
        .await
        .unwrap();
    handle.pause(WorkflowActor::Human).await.unwrap();
    let path = journal_path(dir.path(), &handle.run_id);
    let before = collect_journal(&path, Some(&handle.run_id)).unwrap();
    drop(handle);
    drop(runtime);

    let recovered = WorkflowRuntime::new(WorkflowLimits::default());
    let recovered_handle = recovered.rehydrate(dir.path()).await.unwrap().remove(0);
    let snapshot = recovered_handle.snapshot().await;
    assert_eq!(snapshot.state, WorkflowState::Failed);
    assert!(
        snapshot.terminal_reason.as_deref().is_some_and(
            |reason| reason.contains("illegal workflow transition paused -> completed")
        ),
        "unexpected recovery failure: {:?}",
        snapshot.terminal_reason
    );
    assert_eq!(
        collect_journal(&path, Some(&recovered_handle.run_id)).unwrap(),
        before
    );
}

#[tokio::test]
async fn corrupt_run_is_rehydrated_as_readable_failed_handle() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("workflows").join("wf_corrupt");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(run_dir.join("run.json"), b"not-json").unwrap();

    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handles = runtime.rehydrate(dir.path()).await.unwrap();
    assert_eq!(handles.len(), 1);
    let snapshot = handles[0].snapshot().await;
    assert_eq!(snapshot.state, WorkflowState::Failed);
    assert!(snapshot.recovery_failure);
    assert!(
        snapshot
            .terminal_reason
            .unwrap()
            .contains("corrupt run metadata")
    );
    let output = handles[0].output().await.unwrap();
    assert_eq!(output.metadata.run_id.0, "wf_corrupt");
    serde_json::to_value(output).unwrap();
}

#[tokio::test]
async fn rehydrate_isolates_recovery_append_failure() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());

    // Bad sibling: durable incomplete invocation; recovery must append and will fail.
    let bad_handle = create_run(&runtime, dir.path()).await;
    let bad_id = bad_handle.run_id.clone();
    let bad_path = journal_path(dir.path(), &bad_id);
    let input = serde_json::json!({"task": "x".repeat(1_024)});
    let existing = collect_journal(&bad_path, None).unwrap();
    let next_seq = existing.last().map_or(0, |e| e.seq + 1);
    let run_id = existing
        .first()
        .map_or_else(|| bad_id.clone(), |e| e.run_id.clone());
    let mut writer =
        JournalWriter::open(&bad_path, run_id.clone(), &WorkflowLimits::default()).unwrap();
    let started = JournalEnvelope::new(
        next_seq,
        2,
        run_id,
        JournalPayload::InvocationStarted {
            invocation_id: "inv_stuck".to_owned(),
            call_index: 0,
            kind: WorkflowInvocationKind::Delegate,
            canonical_input: Some(input.clone()),
        },
    )
    .with_canonical_input_hash(canonical_input_hash(&input));
    writer.append(&started, &WorkflowLimits::default()).unwrap();

    // Healthy sibling: already terminal so rehydrate needs no recovery append.
    let good_handle = create_run(&runtime, dir.path()).await;
    let good_id = good_handle.run_id.clone();
    let good_path = journal_path(dir.path(), &good_id);
    let existing = collect_journal(&good_path, None).unwrap();
    let next_seq = existing.last().map_or(0, |e| e.seq + 1);
    let run_id = existing
        .first()
        .map_or_else(|| good_id.clone(), |e| e.run_id.clone());
    let mut good_writer =
        JournalWriter::open(&good_path, run_id.clone(), &WorkflowLimits::default()).unwrap();
    let changed = JournalEnvelope::new(
        next_seq,
        2,
        run_id,
        JournalPayload::StateChanged {
            previous: WorkflowState::Queued,
            // Cancelled is terminal without requiring final_result_recorded.
            new: WorkflowState::Cancelled,
            reason: "done".to_owned(),
            actor: WorkflowActor::Runtime,
        },
    );
    good_writer
        .append(&changed, &WorkflowLimits::default())
        .unwrap();
    drop(writer);
    drop(good_writer);
    let bad_journal_bytes = std::fs::metadata(&bad_path).unwrap().len();
    assert!(std::fs::metadata(&good_path).unwrap().len() <= bad_journal_bytes);
    drop(bad_handle);
    drop(good_handle);
    drop(runtime);

    // Force recovery append to hit journal total limit for the bad run only.
    let recovered = WorkflowRuntime::new(WorkflowLimits {
        journal_total_bytes: bad_journal_bytes,
        ..WorkflowLimits::default()
    });
    let handles = recovered
        .rehydrate(dir.path())
        .await
        .expect("sibling rehydration continues after run-local recovery failure");
    assert_eq!(handles.len(), 2);

    let mut by_id = std::collections::HashMap::new();
    for handle in &handles {
        by_id.insert(handle.run_id.0.clone(), handle.snapshot().await);
    }

    let failed = by_id.get(&bad_id.0).expect("failed run handle present");
    assert_eq!(failed.state, WorkflowState::Failed);
    assert!(failed.recovery_failure);
    assert!(
        failed
            .terminal_reason
            .as_deref()
            .unwrap_or_default()
            .contains("recovery append failed"),
        "terminal reason was {:?}",
        failed.terminal_reason
    );

    let healthy = by_id
        .get(&good_id.0)
        .expect("healthy sibling handle present");
    assert_eq!(healthy.state, WorkflowState::Cancelled);
    assert!(!healthy.recovery_failure);

    // Recovery must not invent a finish record when the recovery append failed.
    assert!(
        !collect_journal(&bad_path, None)
            .unwrap()
            .iter()
            .any(|record| {
                matches!(&record.payload, JournalPayload::InvocationFinished {
                        invocation_id,
                        ..
                    } if invocation_id == "inv_stuck"
                )
            })
    );
}
