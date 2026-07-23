use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use neo_agent_core::AgentTokenUsage;
use neo_agent_core::workflow::{
    JournalRecord, JournalWriter, WorkflowActor, WorkflowHandle, WorkflowInterruptionReason,
    WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowLaunchRequest, WorkflowLimits,
    WorkflowOutcomeStatus, WorkflowPhase, WorkflowRuntime, WorkflowState, canonical_input_hash,
    journal_path, read_journal,
};
use tokio::sync::Notify;

fn launch_request() -> WorkflowLaunchRequest {
    WorkflowLaunchRequest {
        name: "test-run".to_owned(),
        description: "test".to_owned(),
        phases: vec![WorkflowPhase {
            id: "build".to_owned(),
            description: "build it".to_owned(),
        }],
        script: "neo.phase('build')".to_owned(),
        args: serde_json::json!({}),
        launch_source: "/workflow".to_owned(),
        parent_run_id: None,
    }
}

async fn create_run(runtime: &WorkflowRuntime, session_dir: &Path) -> WorkflowHandle {
    runtime
        .create_run(session_dir, launch_request())
        .await
        .expect("create run")
}

fn completed(summary: &str) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        ok: true,
        status: WorkflowOutcomeStatus::Completed,
        summary: summary.to_owned(),
        interruption: None,
        details: serde_json::json!({}),
        actual_usage: None,
        child_refs: Vec::new(),
    }
}

fn completed_with_usage(input_tokens: u32, output_tokens: u32) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        actual_usage: Some(AgentTokenUsage {
            input_tokens,
            output_tokens,
            input_cache_read_tokens: 0,
            input_cache_write_tokens: 0,
        }),
        ..completed("used provider")
    }
}

async fn wait_for_state(handle: &WorkflowHandle, expected: WorkflowState) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handle.snapshot().await.state == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("workflow reached expected state");
}

#[tokio::test]
async fn durable_create_waits_for_explicit_worker_start() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let root = dir.path().to_path_buf();
    runtime
        .bind_runner({
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            move |_handle, metadata, _session_dir| {
                let started = Arc::clone(&started);
                let release = Arc::clone(&release);
                let run_dir = neo_agent_core::workflow::run_dir(&root, &metadata.run_id);
                async move {
                    assert!(run_dir.join("run.json").exists());
                    assert_eq!(read_journal(&run_dir.join("journal.jsonl"))?.len(), 1);
                    started.notify_one();
                    release.notified().await;
                    Ok(())
                }
            }
        })
        .unwrap();

    let handle = create_run(&runtime, dir.path()).await;
    assert_eq!(handle.snapshot().await.state, WorkflowState::Running);
    runtime.start_worker(&handle.run_id).await.unwrap();
    started.notified().await;
    assert_eq!(handle.snapshot().await.state, WorkflowState::Running);
    release.notify_one();
    wait_for_state(&handle, WorkflowState::Completed).await;
}

#[tokio::test]
async fn rollback_created_run_removes_only_unstarted_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;
    let run_dir = neo_agent_core::workflow::run_dir(dir.path(), &handle.run_id);
    assert!(run_dir.exists());

    runtime
        .rollback_created_run(&handle.run_id)
        .await
        .expect("rollback unstarted run");

    assert!(!run_dir.exists());
    assert!(runtime.snapshot(&handle.run_id).await.is_err());
}

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
    let handle = create_run(&runtime, dir.path()).await;
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
async fn worker_start_failure_is_durably_terminalized() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;
    let error = runtime
        .start_worker(&handle.run_id)
        .await
        .expect_err("unbound worker must fail");

    runtime
        .fail_worker_start(&handle.run_id, &error)
        .await
        .expect("persist failed startup");

    assert_eq!(handle.snapshot().await.state, WorkflowState::Failed);
    assert!(
        read_journal(&journal_path(dir.path(), &handle.run_id))
            .unwrap()
            .iter()
            .any(|record| matches!(
                record,
                JournalRecord::StateChanged {
                    new: WorkflowState::Failed,
                    ..
                }
            ))
    );
}

#[tokio::test]
async fn invoke_persists_start_before_effect_and_finish_after_effect() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;
    let path = journal_path(dir.path(), &handle.run_id);
    let observed_start = Arc::new(AtomicBool::new(false));

    let outcome = handle
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            serde_json::json!({"task": "audit"}),
            true,
            {
                let path = path.clone();
                let observed_start = Arc::clone(&observed_start);
                move |invocation| async move {
                    observed_start.store(
                        matches!(
                            read_journal(&path).unwrap().last(),
                            Some(JournalRecord::InvocationStarted { invocation_id, .. })
                                if invocation_id == &invocation.invocation_id
                        ),
                        Ordering::Release,
                    );
                    completed_with_usage(3, 2)
                }
            },
        )
        .await
        .unwrap();

    assert!(outcome.ok);
    assert!(observed_start.load(Ordering::Acquire));
    let records = read_journal(&path).unwrap();
    assert!(matches!(
        records[1],
        JournalRecord::InvocationStarted { .. }
    ));
    assert!(matches!(
        records[2],
        JournalRecord::InvocationFinished { .. }
    ));
    let output = handle.output().await.unwrap();
    assert_eq!(output.actual_usage.unwrap().input_tokens, 3);
    serde_json::to_value(output).expect("WorkflowOutput serializes");
}

#[tokio::test]
async fn instruction_replan_interruption_durably_pauses_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;
    let path = journal_path(dir.path(), &handle.run_id);

    let outcome = handle
        .invoke(
            0,
            WorkflowInvocationKind::VerifyCommand,
            serde_json::json!({"command": "cargo --version"}),
            false,
            |_| async {
                WorkflowInvocationOutcome {
                    ok: false,
                    status: WorkflowOutcomeStatus::Interrupted,
                    summary: "instructions changed".to_owned(),
                    interruption: Some(WorkflowInterruptionReason::InstructionReplanRequired),
                    details: serde_json::json!({
                        "reason": "instruction_replan_required",
                        "side_effect_occurred": false,
                    }),
                    actual_usage: None,
                    child_refs: Vec::new(),
                }
            },
        )
        .await
        .unwrap();

    assert_eq!(outcome.status, WorkflowOutcomeStatus::Interrupted);
    let snapshot = handle.snapshot().await;
    assert_eq!(snapshot.state, WorkflowState::Paused);
    assert_eq!(
        snapshot.terminal_reason.as_deref(),
        Some("instruction_replan_required")
    );
    assert!(read_journal(&path).unwrap().iter().any(|record| matches!(
        record,
        JournalRecord::StateChanged {
            new: WorkflowState::Paused,
            reason,
            actor: WorkflowActor::Runtime,
            ..
        } if reason == "instruction_replan_required"
    )));
}

#[tokio::test]
async fn projected_instruction_reason_without_typed_interruption_does_not_pause() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;

    handle
        .invoke(
            0,
            WorkflowInvocationKind::VerifyCommand,
            serde_json::json!({"command": "cargo --version"}),
            false,
            |_| async {
                WorkflowInvocationOutcome {
                    ok: false,
                    status: WorkflowOutcomeStatus::Interrupted,
                    summary: "spoofed projection".to_owned(),
                    interruption: None,
                    details: serde_json::json!({
                        "reason": "instruction_replan_required",
                        "side_effect_occurred": false,
                    }),
                    actual_usage: None,
                    child_refs: Vec::new(),
                }
            },
        )
        .await
        .unwrap();

    assert_eq!(handle.snapshot().await.state, WorkflowState::Running);
    assert!(
        !read_journal(&journal_path(dir.path(), &handle.run_id))
            .unwrap()
            .iter()
            .any(|record| matches!(
                record,
                JournalRecord::StateChanged {
                    new: WorkflowState::Paused,
                    ..
                }
            ))
    );
}

#[tokio::test]
async fn replay_uses_matching_prefix_without_repeating_effect_then_starts_live() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;
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
    wait_for_state(&recovered_handle, WorkflowState::Completed).await;
    assert_eq!(effects.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn replay_mismatch_starts_live_effect() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;
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
    wait_for_state(&recovered_handle, WorkflowState::Completed).await;
    assert_eq!(effects.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn incomplete_invocation_is_interrupted_and_never_reexecuted() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;
    let path = journal_path(dir.path(), &handle.run_id);
    let mut writer = JournalWriter::open(&path).unwrap();
    let input = serde_json::json!({"task": "audit"});
    writer
        .append(
            &JournalRecord::InvocationStarted {
                seq: writer.next_seq(),
                timestamp_ms: 2,
                invocation_id: "inv_incomplete".to_owned(),
                call_index: 0,
                kind: WorkflowInvocationKind::Delegate,
                canonical_input: input.clone(),
                canonical_input_hash: canonical_input_hash(&input),
            },
            &WorkflowLimits::default(),
        )
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
                    let outcome = handle
                        .invoke(
                            0,
                            WorkflowInvocationKind::Delegate,
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
    wait_for_state(&recovered_handle, WorkflowState::Completed).await;
    assert_eq!(effects.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn recovery_resolver_adopts_known_terminal_child_result() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;
    let path = journal_path(dir.path(), &handle.run_id);
    let mut writer = JournalWriter::open(&path).unwrap();
    let input = serde_json::json!({"task": "audit"});
    writer
        .append(
            &JournalRecord::InvocationStarted {
                seq: writer.next_seq(),
                timestamp_ms: 2,
                invocation_id: "child_7".to_owned(),
                call_index: 0,
                kind: WorkflowInvocationKind::Delegate,
                canonical_input: input.clone(),
                canonical_input_hash: canonical_input_hash(&input),
            },
            &WorkflowLimits::default(),
        )
        .unwrap();
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
    assert!(read_journal(&path).unwrap().iter().any(|record| {
        matches!(record, JournalRecord::InvocationFinished { invocation_id, outcome, .. }
            if invocation_id == "child_7" && outcome.summary == "adopted child")
    }));
}

#[tokio::test]
async fn pause_reaches_effect_boundary_and_resume_restarts_same_run() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let worker_starts = Arc::new(AtomicUsize::new(0));
    let effects = Arc::new(AtomicUsize::new(0));
    runtime
        .bind_runner({
            let worker_starts = Arc::clone(&worker_starts);
            let effects = Arc::clone(&effects);
            move |handle, _metadata, _session_dir| {
                let worker_starts = Arc::clone(&worker_starts);
                let effects = Arc::clone(&effects);
                async move {
                    worker_starts.fetch_add(1, Ordering::AcqRel);
                    handle
                        .invoke(
                            0,
                            WorkflowInvocationKind::Delegate,
                            serde_json::json!({"task": "audit"}),
                            true,
                            {
                                let handle = handle.clone();
                                move |_| async move {
                                    effects.fetch_add(1, Ordering::AcqRel);
                                    while !handle.is_pause_requested() {
                                        tokio::task::yield_now().await;
                                    }
                                    completed("boundary reached")
                                }
                            },
                        )
                        .await?;
                    Ok(())
                }
            }
        })
        .unwrap();
    let handle = create_run(&runtime, dir.path()).await;
    runtime.start_worker(&handle.run_id).await.unwrap();
    while effects.load(Ordering::Acquire) == 0 {
        tokio::task::yield_now().await;
    }
    handle.pause(WorkflowActor::Human).await.unwrap();
    wait_for_state(&handle, WorkflowState::Paused).await;
    assert!(
        read_journal(&journal_path(dir.path(), &handle.run_id))
            .unwrap()
            .iter()
            .any(|record| {
                matches!(
                    record,
                    JournalRecord::StateChanged {
                        new: WorkflowState::Paused,
                        actor: WorkflowActor::Human,
                        ..
                    }
                )
            })
    );
    let run_id = handle.run_id.clone();
    handle.resume(WorkflowActor::Human).await.unwrap();
    wait_for_state(&handle, WorkflowState::Completed).await;
    assert_eq!(handle.run_id, run_id);
    assert_eq!(worker_starts.load(Ordering::Acquire), 2);
    assert_eq!(effects.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn stop_cancels_active_effect_and_terminalizes_after_finish_record() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let effect_started = Arc::new(Notify::new());
    let effect_cancelled = Arc::new(Notify::new());
    let allow_settlement = Arc::new(Notify::new());
    let effect_settled = Arc::new(AtomicBool::new(false));
    runtime
        .bind_runner({
            let effect_started = Arc::clone(&effect_started);
            let effect_cancelled = Arc::clone(&effect_cancelled);
            let allow_settlement = Arc::clone(&allow_settlement);
            let effect_settled = Arc::clone(&effect_settled);
            move |handle, _metadata, _session_dir| {
                let effect_started = Arc::clone(&effect_started);
                let effect_cancelled = Arc::clone(&effect_cancelled);
                let allow_settlement = Arc::clone(&allow_settlement);
                let effect_settled = Arc::clone(&effect_settled);
                async move {
                    handle
                        .invoke(
                            0,
                            WorkflowInvocationKind::Delegate,
                            serde_json::json!({"task": "long"}),
                            true,
                            move |invocation| async move {
                                effect_started.notify_one();
                                invocation.cancel_token.cancelled().await;
                                effect_cancelled.notify_one();
                                allow_settlement.notified().await;
                                effect_settled.store(true, Ordering::Release);
                                WorkflowInvocationOutcome {
                                    ok: false,
                                    status: WorkflowOutcomeStatus::Cancelled,
                                    summary: "canonical child cancelled".to_owned(),
                                    interruption: None,
                                    details: serde_json::json!({
                                        "invocation_id": invocation.invocation_id,
                                    }),
                                    actual_usage: None,
                                    child_refs: Vec::new(),
                                }
                            },
                        )
                        .await?;
                    Ok(())
                }
            }
        })
        .unwrap();
    let handle = create_run(&runtime, dir.path()).await;
    runtime.start_worker(&handle.run_id).await.unwrap();
    effect_started.notified().await;
    handle.stop(WorkflowActor::Human).await.unwrap();
    effect_cancelled.notified().await;
    handle.stop(WorkflowActor::Model).await.unwrap();
    allow_settlement.notify_one();
    wait_for_state(&handle, WorkflowState::Cancelled).await;
    assert!(effect_settled.load(Ordering::Acquire));

    let records = read_journal(&journal_path(dir.path(), &handle.run_id)).unwrap();
    let finish = records
        .iter()
        .position(|record| matches!(record, JournalRecord::InvocationFinished { .. }))
        .unwrap();
    let terminal = records
        .iter()
        .position(|record| {
            matches!(
                record,
                JournalRecord::StateChanged {
                    new: WorkflowState::Cancelled,
                    actor: WorkflowActor::Human,
                    ..
                }
            )
        })
        .unwrap();
    assert!(finish < terminal);
}

#[tokio::test]
async fn corrupt_run_is_rehydrated_as_inspectable_failed_handle() {
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
async fn token_cap_uses_actual_usage_and_blocks_only_next_provider_call() {
    let dir = tempfile::tempdir().unwrap();
    let limits = WorkflowLimits {
        token_cap: Some(5),
        ..WorkflowLimits::default()
    };
    let runtime = WorkflowRuntime::new(limits);
    let handle = create_run(&runtime, dir.path()).await;
    let effects = Arc::new(AtomicUsize::new(0));

    handle
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            serde_json::json!({"estimated_tokens": 1_000_000}),
            true,
            {
                let effects = Arc::clone(&effects);
                move |_| async move {
                    effects.fetch_add(1, Ordering::AcqRel);
                    completed_with_usage(3, 2)
                }
            },
        )
        .await
        .unwrap();
    handle
        .invoke(
            1,
            WorkflowInvocationKind::Log,
            serde_json::json!({"message": "local"}),
            false,
            {
                let effects = Arc::clone(&effects);
                move |_| async move {
                    effects.fetch_add(1, Ordering::AcqRel);
                    completed("local")
                }
            },
        )
        .await
        .unwrap();
    let blocked = handle
        .invoke(
            2,
            WorkflowInvocationKind::Delegate,
            serde_json::json!({"task": "blocked"}),
            true,
            {
                let effects = Arc::clone(&effects);
                move |_| async move {
                    effects.fetch_add(100, Ordering::AcqRel);
                    completed("must not run")
                }
            },
        )
        .await
        .unwrap();

    assert_eq!(blocked.status, WorkflowOutcomeStatus::ResourceLimited);
    assert_eq!(effects.load(Ordering::Acquire), 2);
    assert_eq!(
        handle.snapshot().await.state,
        WorkflowState::ResourceLimited
    );
    assert_eq!(
        handle
            .output()
            .await
            .unwrap()
            .actual_usage
            .unwrap()
            .input_tokens,
        3
    );
}
