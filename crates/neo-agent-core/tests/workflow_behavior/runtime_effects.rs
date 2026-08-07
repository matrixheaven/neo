use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use neo_agent_core::AgentTokenUsage;
use neo_agent_core::workflow::journal::JournalPayload;
use neo_agent_core::workflow::{
    WorkflowActor, WorkflowChildKey, WorkflowChildRef, WorkflowInterruptionReason,
    WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowLimits, WorkflowOutcomeStatus,
    WorkflowRuntime, WorkflowState, journal_path,
};

use super::runtime_lifecycle::{
    collect_journal, completed, create_run, create_running_run, wait_for_state,
};

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

#[tokio::test]
async fn invocation_status_uses_execution_state() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_running_run(&runtime, dir.path()).await;

    // Business data such as `verified = false` is completed execution data and
    // must not count as a host failure.
    let outcome = handle
        .invoke(
            0,
            WorkflowInvocationKind::Verify,
            serde_json::json!({"condition": false, "message": "evidence incomplete"}),
            false,
            |_| async {
                WorkflowInvocationOutcome {
                    status: WorkflowOutcomeStatus::Completed,
                    summary: "verification failed".to_owned(),
                    interruption: None,
                    details: serde_json::json!({
                        "message": "evidence incomplete",
                        "verified": false,
                    }),
                    actual_usage: None,
                    child_refs: Vec::new(),
                }
            },
        )
        .await
        .unwrap();
    assert!(outcome.is_completed());
    assert_eq!(outcome.details["verified"], serde_json::json!(false));
    assert_eq!(handle.snapshot().await.failure_count, 0);

    // A real execution failure still increments the host failure count and
    // stays terminal.
    let failed = handle
        .invoke(
            1,
            WorkflowInvocationKind::Delegate,
            serde_json::json!({"task": "boom"}),
            true,
            |_| async {
                WorkflowInvocationOutcome {
                    status: WorkflowOutcomeStatus::Failed,
                    summary: "provider error".to_owned(),
                    interruption: None,
                    details: serde_json::json!({"error": "provider error"}),
                    actual_usage: None,
                    child_refs: Vec::new(),
                }
            },
        )
        .await
        .unwrap();
    assert!(!failed.is_completed());
    assert_eq!(failed.status, WorkflowOutcomeStatus::Failed);
    assert_eq!(handle.snapshot().await.failure_count, 1);
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
    let records = collect_journal(&journal_path(dir.path(), &handle.run_id), None).unwrap();
    let bounded_outcome = records
        .iter()
        .find_map(|record| match &record.payload {
            JournalPayload::InvocationFinished { outcome, .. }
                if outcome.status == WorkflowOutcomeStatus::ResourceLimited =>
            {
                Some(outcome)
            }
            _ => None,
        })
        .expect("bounded invocation outcome");
    assert_eq!(
        bounded_outcome
            .actual_usage
            .expect("journal usage")
            .output_tokens,
        23
    );
    assert_eq!(
        bounded_outcome.child_refs,
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
    let _recovered_output = recovered[0].output().await.unwrap();
    let recovered_records =
        collect_journal(&journal_path(dir.path(), &handle.run_id), None).unwrap();
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
    assert_eq!(recovered_outcome.child_refs, bounded_outcome.child_refs);
}

#[tokio::test]
async fn invoke_persists_start_before_effect_and_finish_after_effect() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let handle = create_running_run(&runtime, dir.path()).await;
    let path = journal_path(dir.path(), &handle.run_id);
    let observed_queued_projection = Arc::new(AtomicBool::new(false));
    let effect_handle = handle.clone();

    let outcome = handle
        .invoke(
            0,
            WorkflowInvocationKind::Delegate,
            serde_json::json!({"task": "audit"}),
            true,
            {
                let path = path.clone();
                let observed_queued_projection = Arc::clone(&observed_queued_projection);
                move |invocation| async move {
                    let durable = collect_journal(&path, None).unwrap();
                    let queued_sequence = durable.last().and_then(|envelope| {
                        matches!(
                            &envelope.payload,
                            JournalPayload::ChildQueued {
                                child_key: WorkflowChildKey::DirectDelegate { invocation_id },
                                ..
                            } if invocation_id == &invocation.invocation_id
                        )
                        .then_some(envelope.seq)
                    });
                    observed_queued_projection.store(
                        effect_handle.snapshot().await.projection_sequence == queued_sequence,
                        Ordering::Release,
                    );
                    completed_with_usage(3, 2)
                }
            },
        )
        .await
        .unwrap();

    assert!(outcome.is_completed());
    assert!(observed_queued_projection.load(Ordering::Acquire));
    let records = collect_journal(&path, None).unwrap();
    assert!(
        records
            .iter()
            .any(|r| matches!(r.payload, JournalPayload::InvocationStarted { .. })),
        "missing InvocationStarted: {records:?}"
    );
    assert!(
        records
            .iter()
            .any(|r| matches!(r.payload, JournalPayload::ChildQueued { .. })),
        "missing ChildQueued: {records:?}"
    );
    let child_finished = records
        .iter()
        .position(|r| matches!(r.payload, JournalPayload::ChildFinished { .. }))
        .expect("missing ChildFinished");
    let invocation_finished = records
        .iter()
        .position(|r| matches!(r.payload, JournalPayload::InvocationFinished { .. }))
        .expect("missing InvocationFinished");
    assert!(child_finished < invocation_finished);
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
    assert!(collect_journal(&path, None).unwrap().iter().any(
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
        !collect_journal(&journal_path(dir.path(), &handle.run_id), None)
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
        collect_journal(&journal_path(dir.path(), &handle.run_id), None)
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
    // canonical requires a durable final_result for Completed; this runner only
    // exercises pause/resume occupancy, so the worker exits Failed.
    wait_for_state(&handle, WorkflowState::Failed).await;
    assert_eq!(handle.run_id, run_id);
    assert_eq!(worker_starts.load(Ordering::Acquire), 2);
    assert_eq!(effects.load(Ordering::Acquire), 1);
}
