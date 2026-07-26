//! Verified linked-run checkpoints and seed import (Task 8 / design §34).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use neo_agent_core::AgentTokenUsage;
use neo_agent_core::workflow::capability::WorkflowCapability;
use neo_agent_core::workflow::journal::{JournalPayload, collect_journal_v2};
use neo_agent_core::workflow::{
    ArtifactKind, ArtifactValue, WorkflowActor, WorkflowErrorCode, WorkflowHandle,
    WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowLaunchRequest,
    WorkflowOutcomeStatus, WorkflowPhase, WorkflowRuntime, WorkflowState, run_dir,
};

fn launch_request(name: &str) -> WorkflowLaunchRequest {
    WorkflowLaunchRequest {
        name: name.to_owned(),
        description: "lineage test".to_owned(),
        phases: vec![WorkflowPhase {
            id: "work".to_owned(),
            description: "work".to_owned(),
        }],
        script: "return { ok = true }".to_owned(),
        args: serde_json::json!({}),
        launch_source: "/workflow".to_owned(),
        parent_run_id: None,
    }
}

fn completed_with_usage(summary: &str, usage: AgentTokenUsage) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        ok: true,
        status: WorkflowOutcomeStatus::Completed,
        summary: summary.to_owned(),
        interruption: None,
        details: serde_json::json!({}),
        actual_usage: Some(usage),
        child_refs: Vec::new(),
    }
}

async fn wait_state(handle: &WorkflowHandle, want: WorkflowState) {
    for _ in 0..400 {
        if handle.snapshot().await.state == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "run did not reach {want:?}; last={:?}",
        handle.snapshot().await.state
    );
}

async fn wait_terminal(handle: &WorkflowHandle) {
    for _ in 0..400 {
        if handle.snapshot().await.state.is_terminal() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run did not become terminal");
}

fn grant_auth() -> (
    WorkflowCapability,
    neo_agent_core::workflow::capability::WorkflowCapabilityReservation,
) {
    let cap = WorkflowCapability::default();
    cap.grant();
    let reservation = cap.reserve().expect("reserve fresh authorization");
    (cap, reservation)
}

/// Build a terminal parent with one completed host call + one committed artifact.
///
/// Uses a dedicated runtime so callers can bind a different runner for the child.
async fn terminal_parent_with_artifact(session: &std::path::Path) -> WorkflowHandle {
    let runtime = WorkflowRuntime::default();
    let gate = Arc::new(tokio::sync::Notify::new());
    let gate_run = Arc::clone(&gate);
    runtime
        .bind_runner(move |handle, _meta, _session| {
            let gate = Arc::clone(&gate_run);
            async move {
                let _ = handle
                    .invoke(
                        0,
                        WorkflowInvocationKind::Delegate,
                        serde_json::json!({"task": "parent-seed"}),
                        true,
                        |_| async {
                            completed_with_usage(
                                "parent done",
                                AgentTokenUsage {
                                    input_tokens: 11,
                                    output_tokens: 7,
                                    input_cache_read_tokens: 1,
                                    input_cache_write_tokens: 0,
                                },
                            )
                        },
                    )
                    .await?;
                handle
                    .commit_artifact(
                        "seed-note",
                        ArtifactKind::Text,
                        ArtifactValue::Text("verified seed artifact body".to_owned()),
                        None,
                    )
                    .await?;
                handle
                    .persist_canonical_final_result(serde_json::json!({"ok": true}), None)
                    .await?;
                gate.notified().await;
                Ok(())
            }
        })
        .expect("bind parent runner");

    let parent = runtime
        .create_run(session, launch_request("parent-lineage"))
        .await
        .expect("create parent");
    runtime
        .start_worker(&parent.run_id)
        .await
        .expect("start parent");
    wait_state(&parent, WorkflowState::Running).await;

    for _ in 0..200 {
        let out = parent.output().await.expect("parent output");
        if out.artifacts.iter().any(|a| a.logical_name == "seed-note") && out.final_result.is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    gate.notify_one();
    wait_terminal(&parent).await;
    assert_eq!(parent.snapshot().await.state, WorkflowState::Completed);
    // `WorkflowHandle` clones the runtime Arc; drop the local binding only.
    drop(runtime);
    parent
}

#[tokio::test]
async fn linked_upgrade_imports_verified_prefix_and_artifacts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let parent = terminal_parent_with_artifact(dir.path()).await;
    let parent_id = parent.run_id.clone();
    // Drop handle so only disk parent remains for the child runtime.
    drop(parent);

    let parent_journal_before =
        std::fs::read(run_dir(dir.path(), &parent_id).join("journal.jsonl")).unwrap();
    let parent_meta_before =
        std::fs::read(run_dir(dir.path(), &parent_id).join("run.json")).unwrap();

    let runtime = WorkflowRuntime::default();
    let (_cap, auth) = grant_auth();
    let child = runtime
        .create_linked_run(
            dir.path(),
            neo_agent_core::workflow::runtime::LinkedRunRequest {
                parent_run_id: parent_id.clone(),
                checkpoint: None,
                link_reason: "v2_upgrade".to_owned(),
                launch: launch_request("child-lineage"),
            },
            Some(auth),
        )
        .await
        .expect("linked create");

    assert_ne!(child.run_id, parent_id);
    assert_eq!(child.snapshot().await.state, WorkflowState::Queued);

    let child_dir = run_dir(dir.path(), &child.run_id);
    let envelopes =
        collect_journal_v2(&child_dir.join("journal.jsonl"), Some(&child.run_id)).expect("journal");

    assert!(
        envelopes
            .iter()
            .any(|e| matches!(e.payload, JournalPayload::LineageSeedImported { .. })),
        "LineageSeedImported must be durable"
    );
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.payload,
            JournalPayload::InvocationStarted {
                kind: WorkflowInvocationKind::Delegate,
                ..
            }
        )),
        "seed must import completed InvocationStarted"
    );
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.payload,
            JournalPayload::InvocationFinished { outcome, .. } if outcome.ok
        )),
        "seed must import completed InvocationFinished"
    );
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.payload,
            JournalPayload::ArtifactCommitted {
                logical_name: Some(name),
                ..
            } if name == "seed-note"
        )),
        "referenced artifacts must be imported by verified hash"
    );

    let output = child.output().await.expect("child output");
    assert!(
        output.actual_usage.is_none(),
        "inherited seed usage must not charge new-run actual_usage: {:?}",
        output.actual_usage
    );
    let inherited = output.inherited_usage.expect("inherited_usage set");
    assert_eq!(inherited.input_tokens, 11);
    assert_eq!(inherited.output_tokens, 7);
    assert!(
        output
            .artifacts
            .iter()
            .any(|a| a.logical_name == "seed-note"),
        "child must list imported artifact metadata"
    );
    let art = output
        .artifacts
        .iter()
        .find(|a| a.logical_name == "seed-note")
        .unwrap();
    let content = child
        .get_artifact(&art.artifact_id)
        .await
        .expect("read imported artifact");
    assert_eq!(content.bytes, b"verified seed artifact body");
    assert_eq!(
        content.metadata.sha256, art.sha256,
        "artifact content hash must match journal"
    );

    assert_eq!(
        parent_journal_before,
        std::fs::read(run_dir(dir.path(), &parent_id).join("journal.jsonl")).unwrap()
    );
    assert_eq!(
        parent_meta_before,
        std::fs::read(run_dir(dir.path(), &parent_id).join("run.json")).unwrap()
    );

    let err = match runtime
        .create_linked_run(
            dir.path(),
            neo_agent_core::workflow::runtime::LinkedRunRequest {
                parent_run_id: parent_id,
                checkpoint: None,
                link_reason: "no-auth".to_owned(),
                launch: launch_request("no-auth-child"),
            },
            None,
        )
        .await
    {
        Ok(_) => panic!("missing authorization must fail"),
        Err(err) => err,
    };
    assert_eq!(err.code(), WorkflowErrorCode::LaunchAuthorizationMissing);
}

#[tokio::test]
async fn mismatch_stops_before_new_effect() {
    let dir = tempfile::tempdir().expect("tempdir");
    let parent = terminal_parent_with_artifact(dir.path()).await;
    let parent_id = parent.run_id.clone();
    drop(parent);

    let runtime = WorkflowRuntime::default();
    let (_cap, auth) = grant_auth();
    let child = runtime
        .create_linked_run(
            dir.path(),
            neo_agent_core::workflow::runtime::LinkedRunRequest {
                parent_run_id: parent_id,
                checkpoint: None,
                link_reason: "fork".to_owned(),
                launch: launch_request("mismatch-child"),
            },
            Some(auth),
        )
        .await
        .expect("linked create");

    let effect_ran = Arc::new(AtomicBool::new(false));
    let effect_flag = Arc::clone(&effect_ran);
    let mismatch = Arc::new(tokio::sync::Mutex::new(None));
    let mismatch_slot = Arc::clone(&mismatch);

    runtime
        .bind_runner(move |handle, _meta, _session| {
            let effect_flag = Arc::clone(&effect_flag);
            let mismatch_slot = Arc::clone(&mismatch_slot);
            async move {
                let err = handle
                    .invoke(
                        0,
                        WorkflowInvocationKind::Delegate,
                        serde_json::json!({"task": "DIFFERENT-from-seed"}),
                        true,
                        |_| {
                            let effect_flag = Arc::clone(&effect_flag);
                            async move {
                                effect_flag.store(true, Ordering::SeqCst);
                                completed_with_usage(
                                    "should never run",
                                    AgentTokenUsage {
                                        input_tokens: 1,
                                        output_tokens: 1,
                                        input_cache_read_tokens: 0,
                                        input_cache_write_tokens: 0,
                                    },
                                )
                            }
                        },
                    )
                    .await
                    .expect_err("seed mismatch must fail");
                *mismatch_slot.lock().await = Some(err);
                Err(neo_agent_core::workflow::WorkflowError::coded(
                    WorkflowErrorCode::LineageMismatch,
                    "seed mismatch stopped runner",
                ))
            }
        })
        .expect("bind mismatch runner");

    runtime
        .start_worker(&child.run_id)
        .await
        .expect("start child");
    wait_terminal(&child).await;

    assert!(
        !effect_ran.load(Ordering::SeqCst),
        "external effect must not run on lineage seed mismatch"
    );
    let err = mismatch
        .lock()
        .await
        .take()
        .expect("mismatch error captured");
    assert_eq!(err.code(), WorkflowErrorCode::LineageMismatch);

    let envelopes = collect_journal_v2(
        &run_dir(dir.path(), &child.run_id).join("journal.jsonl"),
        Some(&child.run_id),
    )
    .expect("child journal");
    let seed_pairs = neo_agent_core::workflow::runtime::seed_pair_count_from_journal(&envelopes);
    let started = envelopes
        .iter()
        .filter(|e| matches!(e.payload, JournalPayload::InvocationStarted { .. }))
        .count();
    assert_eq!(
        started, seed_pairs,
        "no new InvocationStarted may be appended before mismatch failure"
    );
}

#[tokio::test]
async fn terminal_parent_never_changes_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let parent = terminal_parent_with_artifact(dir.path()).await;
    let parent_id = parent.run_id.clone();
    let parent_dir = run_dir(dir.path(), &parent_id);
    let journal_before = std::fs::read(parent_dir.join("journal.jsonl")).unwrap();
    let meta_before = std::fs::read(parent_dir.join("run.json")).unwrap();
    drop(parent);

    // Load parent projection into a fresh runtime to assert in-memory state stability.
    let runtime = WorkflowRuntime::default();
    let handles = runtime.rehydrate(dir.path()).await.expect("rehydrate");
    let parent = handles
        .into_iter()
        .find(|h| h.run_id == parent_id)
        .expect("parent rehydrated");
    let parent_state = parent.snapshot().await.state;
    assert_eq!(parent_state, WorkflowState::Completed);
    let snap_before = parent.snapshot().await;

    let (_cap, auth) = grant_auth();
    let child = runtime
        .create_linked_run(
            dir.path(),
            neo_agent_core::workflow::runtime::LinkedRunRequest {
                parent_run_id: parent_id.clone(),
                checkpoint: None,
                link_reason: "retry_terminal".to_owned(),
                launch: launch_request("retry-child"),
            },
            Some(auth),
        )
        .await
        .expect("linked create from terminal parent");

    assert_ne!(child.run_id.as_str(), parent_id.as_str());
    let snap_after = parent.snapshot().await;
    assert_eq!(snap_after.state, parent_state);
    assert_eq!(snap_after.state, WorkflowState::Completed);
    assert_eq!(snap_after.terminal_reason, snap_before.terminal_reason);
    assert_eq!(
        journal_before,
        std::fs::read(parent_dir.join("journal.jsonl")).unwrap(),
        "terminal parent journal must remain byte-immutable"
    );
    assert_eq!(
        meta_before,
        std::fs::read(parent_dir.join("run.json")).unwrap(),
        "terminal parent run.json must remain byte-immutable"
    );

    let _err = parent
        .resume(WorkflowActor::Human)
        .await
        .expect_err("terminal resume");
    assert_eq!(
        journal_before,
        std::fs::read(parent_dir.join("journal.jsonl")).unwrap()
    );
}
