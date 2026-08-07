use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use neo_agent_core::runtime::WorkflowDispatchResolver;
use neo_agent_core::workflow::journal::{
    JournalEnvelope, JournalPayload, collect_journal as collect_journal_with_limit,
};
use neo_agent_core::workflow::{
    WorkflowActor, WorkflowExecutionOrigin, WorkflowHandle, WorkflowInvocationKind,
    WorkflowInvocationOutcome, WorkflowLaunchRequest, WorkflowLimits, WorkflowOutcomeStatus,
    WorkflowPhase, WorkflowRuntime, WorkflowState, journal_path,
};
use tokio::sync::Notify;

pub(crate) fn collect_journal(
    path: &Path,
    expected_run_id: Option<&neo_agent_core::workflow::WorkflowId>,
) -> Result<Vec<JournalEnvelope>, neo_agent_core::workflow::WorkflowError> {
    collect_journal_with_limit(
        path,
        expected_run_id,
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
}

pub(crate) fn launch_request() -> WorkflowLaunchRequest {
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
        output_schema: None,
        display_name: None,
        input_schema: None,
        definition_origin: None,
        inline_unsaved: false,
    }
}

pub(crate) async fn create_run(runtime: &WorkflowRuntime, session_dir: &Path) -> WorkflowHandle {
    runtime
        .create_run(session_dir, launch_request())
        .await
        .expect("create run")
}

pub(crate) async fn create_running_run(
    runtime: &WorkflowRuntime,
    session_dir: &Path,
) -> WorkflowHandle {
    let handle = create_run(runtime, session_dir).await;
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("enter running for direct invoke");
    handle
}

pub(crate) fn completed(summary: &str) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        status: WorkflowOutcomeStatus::Completed,
        summary: summary.to_owned(),
        interruption: None,
        details: serde_json::json!({}),
        actual_usage: None,
        child_refs: Vec::new(),
    }
}

pub(crate) async fn wait_for_state(handle: &WorkflowHandle, expected: WorkflowState) {
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
async fn resume_without_session_dispatch_returns_to_readable_paused_state() {
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
                    let envelopes = collect_journal(&run_dir.join("journal.jsonl"), None)?;
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
        collect_journal(&journal_path(dir.path(), &handle.run_id), None)
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

    let records = collect_journal(&journal_path(dir.path(), &handle.run_id), None).unwrap();
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
async fn workflow_worker_panic_finishes_invocation_before_failed_state() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let binding_runtime = runtime.clone();
    let in_effect = Arc::new(Notify::new());
    runtime
        .bind_runner({
            let in_effect = Arc::clone(&in_effect);
            let binding_runtime = binding_runtime.clone();
            move |handle, _metadata, _session_dir| {
                let in_effect = Arc::clone(&in_effect);
                let binding_runtime = binding_runtime.clone();
                async move {
                    let run_id = handle.run_id.clone();
                    handle
                        .invoke(
                            0,
                            WorkflowInvocationKind::Delegate,
                            serde_json::json!({"task": "boom"}),
                            true,
                            move |context| {
                                let in_effect = Arc::clone(&in_effect);
                                let binding_runtime = binding_runtime.clone();
                                let origin = WorkflowExecutionOrigin {
                                    run_id,
                                    human_handle: None,
                                    definition_name: "test-run".to_owned(),
                                    definition_revision: None,
                                    phase_id: None,
                                    invocation_id: Some(context.invocation_id),
                                    swarm_item_id: None,
                                };
                                async move {
                                    binding_runtime
                                        .bind_direct_delegate_agent(
                                            &origin,
                                            &neo_agent_core::multi_agent::AgentId::from_existing(
                                                "agent_worker_panic",
                                            ),
                                        )
                                        .await
                                        .expect("bind direct agent");
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

    let records = collect_journal(&path, None).unwrap();
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
            assert!(!outcome.is_completed());
            assert_eq!(outcome.status, WorkflowOutcomeStatus::Interrupted);
            assert_eq!(
                outcome.details.get("reason").and_then(|v| v.as_str()),
                Some("worker_panicked")
            );
        }
        other => panic!("expected InvocationFinished, got {other:?}"),
    }
    assert!(records.iter().any(|record| matches!(
        &record.payload,
        JournalPayload::ChildFinished {
            agent_id: Some(agent_id),
            status: WorkflowOutcomeStatus::Interrupted,
            ..
        } if agent_id == "agent_worker_panic"
    )));

    // No open invocation remains after panic supervision.
    assert!(!records.iter().any(|record| matches!(&record.payload, JournalPayload::InvocationStarted { invocation_id, .. }
            if !records.iter().any(|finish| matches!(&finish.payload, JournalPayload::InvocationFinished {
                    invocation_id: finished_id,
                    ..
                } if finished_id == invocation_id
            ))
    )));
}
