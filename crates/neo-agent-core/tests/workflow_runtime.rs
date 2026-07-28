use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use neo_agent_core::AgentTokenUsage;
use neo_agent_core::runtime::WorkflowDispatchResolver;
use neo_agent_core::workflow::journal::{
    JournalEnvelope, JournalPayload, JournalV2Writer, collect_journal_v2,
};
use neo_agent_core::workflow::{
    WorkflowActor, WorkflowChildRef, WorkflowHandle, WorkflowInterruptionReason,
    WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowLaunchRequest, WorkflowLimits,
    WorkflowOutcomeStatus, WorkflowPhase, WorkflowRuntime, WorkflowState, canonical_input_hash,
    journal_path,
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
        output_schema: None,launch,
    }
}

async fn create_run(runtime: &WorkflowRuntime, session_dir: &Path) -> WorkflowHandle {
    runtime
        .create_run(session_dir, launch_request())
        .await
        .expect("create run")
}

async fn create_running_run(runtime: &WorkflowRuntime, session_dir: &Path) -> WorkflowHandle {
    let handle = create_run(runtime, session_dir).await;
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("enter running for direct invoke");
    handle
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
async fn oversized_invocation_outcome_terminalizes_without_stranding_running_state() {
    let dir = tempfile::tempdir().unwrap();
    let limits = WorkflowLimits {
        journal_record_bytes: 1_024,
        journal_total_bytes: 128 * 1_024,
        ..WorkflowLimits::default()
    };
    let runtime = WorkflowRuntime::new(limits.clone());
    runtime
        .bind_runner(|handle, _metadata, _session_dir| async move {
            handle
                .invoke(
                    0,
                    WorkflowInvocationKind::Delegate,
                    serde_json::json!({"task": "large result"}),
                    true,
                    |_| async {
                        WorkflowInvocationOutcome {
                            details: serde_json::json!({"output": "x".repeat(4_096)}),
                            actual_usage: Some(AgentTokenUsage {
                                input_tokens: 37,
                                output_tokens: 23,
                                input_cache_read_tokens: 11,
                                input_cache_write_tokens: 7,
                            }),
                            child_refs: vec![
                                WorkflowChildRef {
                                    kind: "agent".to_owned(),
                                    id: "agent_oversized".to_owned(),
                                },
                                WorkflowChildRef {
                                    kind: "task".to_owned(),
                                    id: "task_oversized".to_owned(),
                                },
                            ],
                            ..completed("large result")
                        }
                    },
                )
                .await?;
            Ok(())
        })
        .unwrap();

    let handle = create_run(&runtime, dir.path()).await;
    runtime.start_worker(&handle.run_id).await.unwrap();
    wait_for_state(&handle, WorkflowState::ResourceLimited).await;

    let snapshot = handle.snapshot().await;
    assert!(!snapshot.recovery_failure);
    assert_eq!(
        snapshot.terminal_reason.as_deref(),
        Some("workflow invocation result exceeds journal record limit")
    );
    assert_eq!(
        snapshot.actual_usage.expect("snapshot usage").input_tokens,
        37
    );
    let records = collect_journal_v2(&journal_path(dir.path(), &handle.run_id), None).unwrap();
    let compact_outcome = records
        .iter()
        .find_map(|record| match &record.payload {
            JournalPayload::InvocationFinished { outcome, .. }
                if outcome.status == WorkflowOutcomeStatus::ResourceLimited =>
            {
                Some(outcome)
            }
            _ => None,
        })
        .expect("compact invocation outcome");
    assert_eq!(
        compact_outcome
            .actual_usage
            .expect("journal usage")
            .output_tokens,
        23
    );
    assert_eq!(
        compact_outcome.child_refs,
        vec![
            WorkflowChildRef {
                kind: "agent".to_owned(),
                id: "agent_oversized".to_owned(),
            },
            WorkflowChildRef {
                kind: "task".to_owned(),
                id: "task_oversized".to_owned(),
            },
        ]
    );
    assert!(records.iter().any(|record| matches!(
        &record.payload,
        JournalPayload::StateChanged {
            new: WorkflowState::ResourceLimited,
            ..
        }
    )));

    let output = handle.output().await.unwrap();
    assert_eq!(
        output.actual_usage.expect("live output usage").input_tokens,
        37
    );
    // WorkflowOutput no longer embeds full invocation history (use TaskOutput).
    assert!(output.invocations.is_empty());

    let recovered_runtime = WorkflowRuntime::new(limits);
    let recovered = recovered_runtime.rehydrate(dir.path()).await.unwrap();
    assert_eq!(recovered.len(), 1);
    let recovered_snapshot = recovered[0].snapshot().await;
    assert_eq!(
        recovered_snapshot
            .actual_usage
            .expect("recovered snapshot usage")
            .output_tokens,
        23
    );
    let recovered_output = recovered[0].output().await.unwrap();
    assert!(recovered_output.invocations.is_empty());
    let recovered_records =
        collect_journal_v2(&journal_path(dir.path(), &handle.run_id), None).unwrap();
    let recovered_outcome = recovered_records
        .iter()
        .find_map(|record| match &record.payload {
            JournalPayload::InvocationFinished { outcome, .. }
                if outcome.status == WorkflowOutcomeStatus::ResourceLimited =>
            {
                Some(outcome)
            }
            _ => None,
        })
        .expect("recovered journal invocation");
    assert_eq!(recovered_outcome.child_refs, compact_outcome.child_refs);
}

#[tokio::test]
async fn resume_without_session_dispatch_returns_to_inspectable_paused_state() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    WorkflowDispatchResolver::default()
        .bind_workflow_runtime(&runtime)
        .unwrap();
    let handle = create_run(&runtime, dir.path()).await;
    handle.pause(WorkflowActor::Human).await.unwrap();

    handle.resume(WorkflowActor::Human).await.unwrap();
    wait_for_state(&handle, WorkflowState::Paused).await;

    let snapshot = handle.snapshot().await;
    assert!(!snapshot.recovery_failure);
    assert_eq!(
        snapshot.terminal_reason.as_deref(),
        Some("workflow dispatch is not ready for this session")
    );
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
                    // Durable create + worker_start leave RunCreated and Running transition
                    // before any host-effect invocation.
                    let envelopes = collect_journal_v2(&run_dir.join("journal.jsonl"), None)?;
                    assert!(
                        !envelopes.is_empty(),
                        "expected durable journal head before worker body"
                    );
                    assert!(
                        envelopes.iter().any(|env| {
                            matches!(
                                env.payload,
                                neo_agent_core::workflow::journal::JournalPayload::RunCreated { .. }
                            )
                        }),
                        "RunCreated must be durable before worker body"
                    );
                    started.notify_one();
                    release.notified().await;
                    Ok(())
                }
            }
        })
        .unwrap();

    let handle = create_run(&runtime, dir.path()).await;
    // Durable create leaves the run Queued; workers start only via start_worker.
    assert_eq!(handle.snapshot().await.state, WorkflowState::Queued);
    let run_dir_pre = neo_agent_core::workflow::run_dir(dir.path(), &handle.run_id);
    assert!(run_dir_pre.join("run.json").exists());
    runtime.start_worker(&handle.run_id).await.unwrap();
    started.notified().await;
    assert_eq!(handle.snapshot().await.state, WorkflowState::Running);
    release.notify_one();
    // Fixture runner returns Ok without FinalResultRecorded → Failed(missing_final_result).
    wait_for_state(&handle, WorkflowState::Failed).await;
    let snapshot = handle.snapshot().await;
    assert_eq!(
        snapshot.terminal_reason.as_deref(),
        Some("missing_final_result")
    );
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
        collect_journal_v2(&journal_path(dir.path(), &handle.run_id), None)
            .unwrap()
            .iter()
            .any(|record| matches!(
                &record.payload,
                JournalPayload::StateChanged {
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
    let handle = create_running_run(&runtime, dir.path()).await;
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
                        collect_journal_v2(&path, None)
                            .unwrap()
                            .last()
                            .is_some_and(|env| {
                                matches!(
                                    &env.payload,
                                    JournalPayload::InvocationStarted { invocation_id, .. }
                                        if invocation_id == &invocation.invocation_id
                                )
                            }),
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
    let records = collect_journal_v2(&path, None).unwrap();
    assert!(
        records
            .iter()
            .any(|r| matches!(r.payload, JournalPayload::InvocationStarted { .. })),
        "missing InvocationStarted: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|r| matches!(r.payload, JournalPayload::InvocationFinished { .. })),
        "missing InvocationFinished: {records:?}"
    );
    let output = handle.output().await.unwrap();
    assert_eq!(output.actual_usage.unwrap().input_tokens, 3);
    serde_json::to_value(output).expect("WorkflowOutput serializes");
}

#[tokio::test]
async fn instruction_replan_interruption_durably_pauses_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_running_run(&runtime, dir.path()).await;
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
    assert!(collect_journal_v2(&path, None).unwrap().iter().any(
        |record| matches!(&record.payload, JournalPayload::StateChanged {
                new: WorkflowState::Paused,
                reason,
                actor: WorkflowActor::Runtime,
                ..
            } if reason == "instruction_replan_required"
        )
    ));
}

#[tokio::test]
async fn projected_instruction_reason_without_typed_interruption_does_not_pause() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_running_run(&runtime, dir.path()).await;

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
        !collect_journal_v2(&journal_path(dir.path(), &handle.run_id), None)
            .unwrap()
            .iter()
            .any(|record| matches!(
                &record.payload,
                JournalPayload::StateChanged {
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
    // Runner exits without final_result → Failed under V2 completion rules.
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
    // Runner exits without final_result → Failed under V2 completion rules.
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
    let existing = collect_journal_v2(&path, None).unwrap();
    let next_seq = existing.last().map_or(0, |e| e.seq + 1);
    let run_id = existing
        .first()
        .map_or_else(|| handle.run_id.clone(), |e| e.run_id.clone());
    let mut writer = JournalV2Writer::open(&path, run_id.clone()).unwrap();
    let started = JournalEnvelope::new(
        next_seq,
        2,
        run_id,
        JournalPayload::InvocationStarted {
            invocation_id: "inv_incomplete".to_owned(),
            call_index: 0,
            kind: WorkflowInvocationKind::Delegate,
            canonical_input: Some(input.clone()),
        },
    )
    .with_canonical_input_hash(canonical_input_hash(&input));
    writer.append(&started, &WorkflowLimits::default()).unwrap();
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
    // Runner exits without final_result → Failed under V2 completion rules.
    wait_for_state(&recovered_handle, WorkflowState::Failed).await;
    assert_eq!(effects.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn recovery_resolver_adopts_known_terminal_child_result() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_run(&runtime, dir.path()).await;
    let path = journal_path(dir.path(), &handle.run_id);
    let input = serde_json::json!({"task": "audit"});
    let existing = collect_journal_v2(&path, None).unwrap();
    let next_seq = existing.last().map_or(0, |e| e.seq + 1);
    let run_id = existing
        .first()
        .map_or_else(|| handle.run_id.clone(), |e| e.run_id.clone());
    let mut writer = JournalV2Writer::open(&path, run_id.clone()).unwrap();
    let started = JournalEnvelope::new(
        next_seq,
        2,
        run_id,
        JournalPayload::InvocationStarted {
            invocation_id: "child_7".to_owned(),
            call_index: 0,
            kind: WorkflowInvocationKind::Delegate,
            canonical_input: Some(input.clone()),
        },
    )
    .with_canonical_input_hash(canonical_input_hash(&input));
    writer.append(&started, &WorkflowLimits::default()).unwrap();
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
    assert!(collect_journal_v2(&path, None).unwrap().iter().any(|record| {
        matches!(&record.payload, JournalPayload::InvocationFinished { invocation_id, outcome, .. }
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
        collect_journal_v2(&journal_path(dir.path(), &handle.run_id), None)
            .unwrap()
            .iter()
            .any(|record| {
                matches!(
                    &record.payload,
                    JournalPayload::StateChanged {
                        new: WorkflowState::Paused,
                        actor: WorkflowActor::Human,
                        ..
                    }
                )
            })
    );
    let run_id = handle.run_id.clone();
    handle.resume(WorkflowActor::Human).await.unwrap();
    // V2 requires a durable final_result for Completed; this runner only
    // exercises pause/resume occupancy, so the worker exits Failed.
    wait_for_state(&handle, WorkflowState::Failed).await;
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

    let records = collect_journal_v2(&journal_path(dir.path(), &handle.run_id), None).unwrap();
    let finish = records
        .iter()
        .position(|record| matches!(&record.payload, JournalPayload::InvocationFinished { .. }))
        .unwrap();
    let terminal = records
        .iter()
        .position(|record| {
            matches!(
                &record.payload,
                JournalPayload::StateChanged {
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
async fn rehydrate_isolates_recovery_append_failure() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());

    // Bad sibling: durable incomplete invocation; recovery must append and will fail.
    let bad_handle = create_run(&runtime, dir.path()).await;
    let bad_id = bad_handle.run_id.clone();
    let bad_path = journal_path(dir.path(), &bad_id);
    let input = serde_json::json!({"task": "stuck"});
    let existing = collect_journal_v2(&bad_path, None).unwrap();
    let next_seq = existing.last().map_or(0, |e| e.seq + 1);
    let run_id = existing
        .first()
        .map_or_else(|| bad_id.clone(), |e| e.run_id.clone());
    let mut writer = JournalV2Writer::open(&bad_path, run_id.clone()).unwrap();
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
    let existing = collect_journal_v2(&good_path, None).unwrap();
    let next_seq = existing.last().map_or(0, |e| e.seq + 1);
    let run_id = existing
        .first()
        .map_or_else(|| good_id.clone(), |e| e.run_id.clone());
    let mut good_writer = JournalV2Writer::open(&good_path, run_id.clone()).unwrap();
    let changed = JournalEnvelope::new(
        next_seq,
        2,
        run_id,
        JournalPayload::StateChanged {
            previous: WorkflowState::Running,
            // Cancelled is terminal without requiring final_result_recorded.
            new: WorkflowState::Cancelled,
            reason: "done".to_owned(),
            actor: WorkflowActor::Runtime,
        },
    );
    good_writer
        .append(&changed, &WorkflowLimits::default())
        .unwrap();
    drop(bad_handle);
    drop(good_handle);
    drop(runtime);

    // Force recovery append to hit journal total limit for the bad run only.
    let recovered = WorkflowRuntime::new(WorkflowLimits {
        journal_total_bytes: 1,
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
        !collect_journal_v2(&bad_path, None)
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

#[tokio::test]
async fn workflow_worker_panic_finishes_invocation_before_failed_state() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let in_effect = Arc::new(Notify::new());
    runtime
        .bind_runner({
            let in_effect = Arc::clone(&in_effect);
            move |handle, _metadata, _session_dir| {
                let in_effect = Arc::clone(&in_effect);
                async move {
                    handle
                        .invoke(
                            0,
                            WorkflowInvocationKind::Delegate,
                            serde_json::json!({"task": "boom"}),
                            true,
                            move |_| {
                                let in_effect = Arc::clone(&in_effect);
                                async move {
                                    in_effect.notify_waiters();
                                    panic!("workflow worker test panic");
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
    let path = journal_path(dir.path(), &handle.run_id);
    runtime.start_worker(&handle.run_id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), in_effect.notified())
        .await
        .expect("effect started");
    wait_for_state(&handle, WorkflowState::Failed).await;

    let snapshot = handle.snapshot().await;
    assert!(!snapshot.recovery_failure);
    assert_eq!(snapshot.state, WorkflowState::Failed);
    assert_eq!(snapshot.terminal_reason.as_deref(), Some("worker_panicked"));

    let records = collect_journal_v2(&path, None).unwrap();
    let finished_idx = records
        .iter()
        .position(|record| {
            matches!(
                &record.payload,
                JournalPayload::InvocationFinished {
                    outcome: WorkflowInvocationOutcome {
                        status: WorkflowOutcomeStatus::Interrupted,
                        ..
                    },
                    ..
                }
            )
        })
        .expect("interrupted invocation outcome");
    let failed_idx = records
        .iter()
        .position(|record| {
            matches!(&record.payload, JournalPayload::StateChanged {
                    new: WorkflowState::Failed,
                    reason,
                    ..
                } if reason == "worker_panicked"
            )
        })
        .expect("failed state after worker panic");
    assert!(
        finished_idx < failed_idx,
        "invocation outcome must be durable before workflow terminalization"
    );

    match &records[finished_idx].payload {
        JournalPayload::InvocationFinished { outcome, .. } => {
            assert!(!outcome.ok);
            assert_eq!(outcome.status, WorkflowOutcomeStatus::Interrupted);
            assert_eq!(
                outcome.details.get("reason").and_then(|v| v.as_str()),
                Some("worker_panicked")
            );
        }
        other => panic!("expected InvocationFinished, got {other:?}"),
    }

    // No open invocation remains after panic supervision.
    assert!(!records.iter().any(|record| matches!(&record.payload, JournalPayload::InvocationStarted { invocation_id, .. }
            if !records.iter().any(|finish| matches!(&finish.payload, JournalPayload::InvocationFinished {
                    invocation_id: finished_id,
                    ..
                } if finished_id == invocation_id
            ))
    )));
}
