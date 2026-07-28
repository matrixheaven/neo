//! V2 workflow identity, state, and transactional lifecycle tests (Tasks 1 + 4).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use neo_agent_core::workflow::journal::{
    JournalEnvelope, JournalPayload, JournalV2Writer, collect_journal_v2, read_run_metadata,
    run_dir,
};
use neo_agent_core::workflow::{
    WORKFLOW_NAME_MAX_LEN, WorkflowActor, WorkflowArtifactId, WorkflowCheckpoint, WorkflowError,
    WorkflowErrorCode, WorkflowFinalResultMetadata, WorkflowHumanHandle, WorkflowInvocationId,
    WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowLaunchRequest, WorkflowLimits,
    WorkflowName, WorkflowOutcomeStatus, WorkflowPhase, WorkflowRequestId, WorkflowRevision,
    WorkflowRunId, WorkflowRuntime, WorkflowSourceOrigin, WorkflowState, validate_portable_name,
};

#[test]
fn workflow_v2_identity_rejects_invalid_names() {
    // Valid portable names.
    let max_valid = "a".repeat(WORKFLOW_NAME_MAX_LEN);
    for name in ["a", "review", "review-2", "phase_1", max_valid.as_str()] {
        WorkflowName::parse(name).unwrap_or_else(|e| panic!("expected ok for {name:?}: {e}"));
        WorkflowHumanHandle::parse(name)
            .unwrap_or_else(|e| panic!("expected handle ok for {name:?}: {e}"));
        validate_portable_name(name, "workflow name").unwrap();
    }

    // Invalid: empty, uppercase, unicode, leading separator, too long, illegal chars.
    let too_long = "a".repeat(WORKFLOW_NAME_MAX_LEN + 1);
    let invalid = [
        "",
        "Review",
        "review.2",
        "-leading",
        "_leading",
        "has space",
        "emoji-😀",
        too_long.as_str(),
        "slash/name",
        "dot.name",
    ];
    for name in invalid {
        let err = WorkflowName::parse(name).expect_err("must reject");
        assert_eq!(err.code(), WorkflowErrorCode::InvalidInput, "name={name:?}");
        let err = WorkflowHumanHandle::parse(name).expect_err("handle must reject");
        assert_eq!(
            err.code(),
            WorkflowErrorCode::InvalidInput,
            "handle={name:?}"
        );
    }

    // Run ID: UUID machine key for V2; opaque V1 strings stay readable via from_existing.
    let generated = WorkflowRunId::generate();
    assert!(
        generated.as_str().starts_with("wf_"),
        "generated id should use wf_ prefix"
    );
    WorkflowRunId::parse_v2(generated.as_str()).expect("generated id parses");
    WorkflowRunId::parse_v2("00000000-0000-4000-8000-000000000001").expect("hyphen UUID");
    WorkflowRunId::parse_v2("00000000000040008000000000000001").expect("simple hex");
    let bad = WorkflowRunId::parse_v2("not-a-uuid").expect_err("reject garbage");
    assert_eq!(bad.code(), WorkflowErrorCode::InvalidInput);
    let v1 = WorkflowRunId::from_existing("run_legacy_opaque");
    assert_eq!(v1.as_str(), "run_legacy_opaque");

    // Revision must be lowercase sha-256 hex.
    let rev = WorkflowRevision::from_bytes(b"neo-workflow");
    assert_eq!(rev.as_str().len(), 64);
    WorkflowRevision::parse(rev.as_str()).unwrap();
    let bad_rev = WorkflowRevision::parse("not-hex").expect_err("reject");
    assert_eq!(bad_rev.code(), WorkflowErrorCode::InvalidInput);
    let upper = "A".repeat(64);
    assert!(WorkflowRevision::parse(&upper).is_err());

    // Other identity wrappers construct and display.
    let inv = WorkflowInvocationId::generate();
    assert!(inv.as_str().starts_with("inv_"));
    let req = WorkflowRequestId::generate();
    assert!(req.as_str().starts_with("req_"));
    let art = WorkflowArtifactId::new(generated.clone(), rev.as_str()).unwrap();
    assert_eq!(art.as_content_sha256(), rev.as_str());
    let ckpt = WorkflowCheckpoint::new(generated, 3, rev.as_str()).unwrap();
    assert_eq!(ckpt.sequence, 3);

    // V2 states and transitions.
    assert!(!WorkflowState::Queued.is_terminal());
    assert!(!WorkflowState::AwaitingUser.is_terminal());
    assert!(WorkflowState::Completed.is_terminal());
    assert!(WorkflowState::Queued.rehydrates_as_paused_host_exit());
    assert!(WorkflowState::Running.rehydrates_as_paused_host_exit());
    assert!(!WorkflowState::AwaitingUser.rehydrates_as_paused_host_exit());
    assert!(!WorkflowState::AwaitingUser.allows_ordinary_resume());
    assert!(WorkflowState::Paused.allows_ordinary_resume());

    assert!(WorkflowState::Queued.can_transition_to(WorkflowState::Running));
    assert!(WorkflowState::Running.can_transition_to(WorkflowState::AwaitingUser));
    assert!(WorkflowState::AwaitingUser.can_transition_to(WorkflowState::Queued));
    assert!(!WorkflowState::AwaitingUser.can_transition_to(WorkflowState::Running));
    assert!(!WorkflowState::Completed.can_transition_to(WorkflowState::Running));
    assert_eq!(WorkflowState::AwaitingUser.as_str(), "awaiting_user");
    assert_eq!(WorkflowState::Queued.as_str(), "queued");

    // Stable error codes are not message-parsed.
    let coded = WorkflowError::coded(WorkflowErrorCode::LineageMismatch, "prefix diverged");
    assert_eq!(coded.code(), WorkflowErrorCode::LineageMismatch);
    assert_eq!(
        WorkflowErrorCode::LineageMismatch.as_str(),
        "lineage_mismatch"
    );
    assert_eq!(
        WorkflowError::InvalidInput("x".into()).code(),
        WorkflowErrorCode::InvalidInput
    );

    // Source origin labels are stable.
    assert_eq!(WorkflowSourceOrigin::Builtin.as_str(), "builtin");
    assert_eq!(WorkflowSourceOrigin::Project.as_str(), "project");
}

fn launch_request() -> WorkflowLaunchRequest {
    WorkflowLaunchRequest {
        name: "review".to_owned(),
        description: "test run".to_owned(),
        phases: vec![WorkflowPhase {
            id: "review".to_owned(),
            description: "review".to_owned(),
        }],
        script: "return { ok = true }".to_owned(),
        args: serde_json::json!({}),
        launch_source: "/workflow review".to_owned(),
        parent_run_id: None,
        output_schema: None,
        display_name: None,
        input_schema: None,
        definition_origin: None,
        inline_unsaved: false,
    }
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

#[tokio::test]
async fn v2_create_is_durable_and_queued_before_registration() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::default();

    let handle = runtime
        .create_run(dir.path(), launch_request())
        .await
        .expect("create");

    let snapshot = handle.snapshot().await;
    assert_eq!(snapshot.state, WorkflowState::Queued);
    assert!(!snapshot.recovery_failure);

    let meta = read_run_metadata(&run_dir(dir.path(), &handle.run_id)).unwrap();
    assert_eq!(meta.run_id, handle.run_id);
    assert_eq!(
        meta.journal_format_version, 3,
        "new runs use V3 journal format"
    );
    assert_eq!(meta.name, "review");

    let journal_path = run_dir(dir.path(), &handle.run_id).join("journal.jsonl");
    let envelopes = collect_journal_v2(&journal_path, Some(&handle.run_id)).unwrap();
    assert_eq!(envelopes.len(), 1);
    assert!(matches!(
        envelopes[0].payload,
        JournalPayload::RunCreated { .. }
    ));
    match &envelopes[0].payload {
        JournalPayload::RunCreated {
            name,
            description,
            launch_source,
        } => {
            assert_eq!(name, "review");
            assert_eq!(description.as_deref(), Some("test run"));
            assert_eq!(launch_source.as_deref(), Some("/workflow review"));
        }
        _ => unreachable!(),
    }

    // Worker must not auto-start; registration is the caller's responsibility
    // and create_run returns while still Queued with worker inactive.
    let err = runtime
        .start_worker(&handle.run_id)
        .await
        .expect_err("start_worker without runner must fail before activation");
    assert!(
        err.to_string().contains("runner is not bound"),
        "unexpected: {err}"
    );
    assert_eq!(handle.snapshot().await.state, WorkflowState::Queued);

    // Rollback only never-started Queued runs.
    runtime
        .rollback_created_run(&handle.run_id)
        .await
        .expect("rollback unstarted");
    assert!(!run_dir(dir.path(), &handle.run_id).exists());
}

#[test]
fn workflow_v2_rejects_all_illegal_and_terminal_transitions() {
    let allowed: std::collections::HashSet<_> = WorkflowState::allowed_transitions()
        .iter()
        .copied()
        .collect();

    for &from in WorkflowState::all_states() {
        for &to in WorkflowState::all_states() {
            let legal = allowed.contains(&(from, to));
            assert_eq!(
                from.can_transition_to(to),
                legal,
                "can_transition_to mismatch for {} -> {}",
                from.as_str(),
                to.as_str()
            );
            if from == to {
                assert!(from.require_transition_to(to).is_err());
                continue;
            }
            if legal {
                from.require_transition_to(to).unwrap_or_else(|e| {
                    panic!("expected allow {} -> {}: {e}", from.as_str(), to.as_str())
                });
            } else {
                let err = from.require_transition_to(to).expect_err("must reject");
                assert_eq!(err.code(), WorkflowErrorCode::InvalidOperation);
            }
        }
        if from.is_terminal() {
            assert!(!from.allows_ordinary_resume());
            for &to in WorkflowState::all_states() {
                assert!(
                    !from.can_transition_to(to),
                    "terminal {} must be immutable",
                    from.as_str()
                );
            }
        }
    }

    // AwaitingUser is not an ordinary-resume source and cannot jump to Running.
    assert!(!WorkflowState::AwaitingUser.allows_ordinary_resume());
    assert!(!WorkflowState::AwaitingUser.can_transition_to(WorkflowState::Running));
    assert!(!WorkflowState::AwaitingUser.can_transition_to(WorkflowState::Paused));
    assert!(WorkflowState::AwaitingUser.can_transition_to(WorkflowState::Queued));
}

#[tokio::test]
async fn external_effect_is_never_executed_before_durable_start() {
    let dir = tempfile::tempdir().unwrap();
    // Reservation = start + journal_record_bytes + 64 KiB terminal reserve.
    // Create/start fit under 32 KiB; any InvocationStarted reservation does not.
    let limits = WorkflowLimits {
        journal_record_bytes: 16 * 1024,
        journal_total_bytes: 32 * 1024,
        ..WorkflowLimits::default()
    };
    let runtime = WorkflowRuntime::new(limits);
    let effect_ran = Arc::new(AtomicBool::new(false));
    let effect_ran_worker = Arc::clone(&effect_ran);

    runtime
        .bind_runner(move |handle, _metadata, _session_dir| {
            let effect_ran_worker = Arc::clone(&effect_ran_worker);
            async move {
                let result = handle
                    .invoke(
                        0,
                        WorkflowInvocationKind::Log,
                        serde_json::json!({"message": "should not run"}),
                        false,
                        |_| {
                            effect_ran_worker.store(true, Ordering::Release);
                            async { completed("should not run") }
                        },
                    )
                    .await;
                // Reservation/start failure must surface without executing the effect.
                assert!(result.is_err(), "expected start/reservation failure");
                assert!(
                    !effect_ran_worker.load(Ordering::Acquire),
                    "external effect executed before durable InvocationStarted"
                );
                Err(WorkflowError::Failed("reservation denied".to_owned()))
            }
        })
        .unwrap();

    let handle = runtime
        .create_run(dir.path(), launch_request())
        .await
        .expect("create must fit tiny journal");
    assert_eq!(handle.snapshot().await.state, WorkflowState::Queued);

    runtime.start_worker(&handle.run_id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snap = runtime.snapshot(&handle.run_id).await.unwrap();
            if snap.state.is_terminal() {
                return snap;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("workflow reached a terminal state");

    assert!(
        !effect_ran.load(Ordering::Acquire),
        "effect must never run without durable start"
    );

    let journal_path = run_dir(dir.path(), &handle.run_id).join("journal.jsonl");
    let envelopes = collect_journal_v2(&journal_path, Some(&handle.run_id)).unwrap();
    let started = envelopes
        .iter()
        .any(|e| matches!(e.payload, JournalPayload::InvocationStarted { .. }));
    assert!(
        !started,
        "InvocationStarted must not be durable when reservation fails"
    );
}

#[tokio::test]
async fn crash_after_final_result_appends_only_completed_state() {
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path();
    let run_id = WorkflowRunId::generate();
    let run_path = run_dir(session, &run_id);
    std::fs::create_dir_all(&run_path).unwrap();

    // Synthesize a durable prefix that crashed after FinalResultRecorded.
    let meta = neo_agent_core::workflow::WorkflowRunMetadata {
        run_id: run_id.clone(),
        parent_run_id: None,
        name: "review".to_owned(),
        description: "crash recovery".to_owned(),
        phases: Vec::new(),
        script: "return { ok = true }".to_owned(),
        script_sha256: WorkflowRevision::from_bytes(b"return { ok = true }")
            .as_str()
            .to_owned(),
        args: serde_json::json!({}),
        launch_source: "/workflow".to_owned(),
        journal_format_version: 2,
        output_schema: None,
        display_name: None,
        input_schema: None,
        definition_origin: None,
        inline_unsaved: false,
    };
    neo_agent_core::workflow::write_run_metadata(&run_path, &meta, &WorkflowLimits::default())
        .unwrap();

    let journal_path = run_path.join("journal.jsonl");
    let mut writer = JournalV2Writer::open(&journal_path, run_id.clone()).unwrap();
    let limits = WorkflowLimits::default();
    let mut seq = 0u64;
    let mut append = |payload: JournalPayload| {
        let env = JournalEnvelope::new(seq, 1_000 + seq, run_id.clone(), payload);
        writer.append(&env, &limits).unwrap();
        seq += 1;
    };
    append(JournalPayload::RunCreated {
        name: "review".to_owned(),
        description: Some("crash recovery".to_owned()),
        launch_source: Some("/workflow".to_owned()),
    });
    append(JournalPayload::StateChanged {
        previous: WorkflowState::Queued,
        new: WorkflowState::Running,
        reason: "worker_start".to_owned(),
        actor: WorkflowActor::Runtime,
    });
    append(JournalPayload::FinalResultRecorded {
        metadata: WorkflowFinalResultMetadata {
            value: Some(serde_json::json!({"ok": true})),
            artifact_id: None,
            schema_revision: None,
        },
    });
    drop(writer);

    let before = collect_journal_v2(&journal_path, Some(&run_id)).unwrap();
    assert_eq!(before.len(), 3);
    assert!(before.iter().all(|e| {
        !matches!(
            e.payload,
            JournalPayload::StateChanged {
                new: WorkflowState::Completed,
                ..
            }
        )
    }));

    let runtime = WorkflowRuntime::default();
    let handles = runtime.rehydrate(session).await.unwrap();
    assert_eq!(handles.len(), 1);
    let recovered = &handles[0];
    assert_eq!(recovered.run_id, run_id);
    assert_eq!(recovered.snapshot().await.state, WorkflowState::Completed);

    let after = collect_journal_v2(&journal_path, Some(&run_id)).unwrap();
    assert_eq!(
        after.len(),
        4,
        "recovery must append only the missing Completed state"
    );
    match &after[3].payload {
        JournalPayload::StateChanged {
            previous,
            new,
            reason,
            ..
        } => {
            assert_eq!(*previous, WorkflowState::Running);
            assert_eq!(*new, WorkflowState::Completed);
            assert_eq!(reason, "recover_final_result");
        }
        other => panic!("expected Completed transition, got {other:?}"),
    }
    let final_count = after
        .iter()
        .filter(|e| matches!(e.payload, JournalPayload::FinalResultRecorded { .. }))
        .count();
    assert_eq!(final_count, 1, "must not rewrite FinalResultRecorded");

    // Second rehydrate is idempotent: no extra terminal append.
    let runtime2 = WorkflowRuntime::default();
    let handles2 = runtime2.rehydrate(session).await.unwrap();
    assert_eq!(handles2[0].snapshot().await.state, WorkflowState::Completed);
    let after2 = collect_journal_v2(&journal_path, Some(&run_id)).unwrap();
    assert_eq!(after2.len(), 4);
}

#[tokio::test]
async fn ordinary_resume_cannot_bypass_awaiting_user() {
    // Runtime-level guard: only Paused allows ordinary resume.
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::default();
    // Build a rehydrated AwaitingUser projection by writing journal + rehydrate.
    let run_id = WorkflowRunId::generate();
    let run_path = run_dir(dir.path(), &run_id);
    std::fs::create_dir_all(&run_path).unwrap();
    let meta = neo_agent_core::workflow::WorkflowRunMetadata {
        run_id: run_id.clone(),
        parent_run_id: None,
        name: "await".to_owned(),
        description: String::new(),
        phases: Vec::new(),
        script: "return nil".to_owned(),
        script_sha256: WorkflowRevision::from_bytes(b"return nil")
            .as_str()
            .to_owned(),
        args: serde_json::json!({}),
        launch_source: "/workflow".to_owned(),
        journal_format_version: 2,
        output_schema: None,

        display_name: None,
        input_schema: None,
        definition_origin: None,
        inline_unsaved: false,
    };
    neo_agent_core::workflow::write_run_metadata(&run_path, &meta, &WorkflowLimits::default())
        .unwrap();
    let journal_path = run_path.join("journal.jsonl");
    let mut writer = JournalV2Writer::open(&journal_path, run_id.clone()).unwrap();
    let limits = WorkflowLimits::default();
    for (seq, payload) in [
        JournalPayload::RunCreated {
            name: "await".to_owned(),
            description: None,
            launch_source: Some("/workflow".to_owned()),
        },
        JournalPayload::StateChanged {
            previous: WorkflowState::Queued,
            new: WorkflowState::Running,
            reason: "worker_start".to_owned(),
            actor: WorkflowActor::Runtime,
        },
        JournalPayload::UserInputRequested {
            request_id: "req_1".to_owned(),
            prompt: Some(serde_json::json!({"q": "continue?"})),
        },
        JournalPayload::StateChanged {
            previous: WorkflowState::Running,
            new: WorkflowState::AwaitingUser,
            reason: "user_input".to_owned(),
            actor: WorkflowActor::Runtime,
        },
    ]
    .into_iter()
    .enumerate()
    {
        let env = JournalEnvelope::new(seq as u64, 1_000 + seq as u64, run_id.clone(), payload);
        writer.append(&env, &limits).unwrap();
    }
    drop(writer);

    let handles = runtime.rehydrate(dir.path()).await.unwrap();
    assert_eq!(
        handles[0].snapshot().await.state,
        WorkflowState::AwaitingUser
    );
    runtime.bind_runner(|_h, _m, _s| async { Ok(()) }).unwrap();
    let err = handles[0]
        .resume(WorkflowActor::Human)
        .await
        .expect_err("ordinary resume must not bypass awaiting_user");
    assert_eq!(err.code(), WorkflowErrorCode::AwaitingUser);
    assert_eq!(
        handles[0].snapshot().await.state,
        WorkflowState::AwaitingUser
    );
}

#[tokio::test]
async fn worker_panic_clears_active_state_and_releases_resources() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::default();
    let in_effect = Arc::new(tokio::sync::Notify::new());
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

    let handle = runtime
        .create_run(dir.path(), launch_request())
        .await
        .expect("create");
    runtime.start_worker(&handle.run_id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), in_effect.notified())
        .await
        .expect("effect started");

    // Wait until supervision terminalizes the run.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handle.snapshot().await.state == WorkflowState::Failed {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("worker panic terminalized");

    let snapshot = handle.snapshot().await;
    assert_eq!(snapshot.state, WorkflowState::Failed);
    assert_eq!(snapshot.terminal_reason.as_deref(), Some("worker_panicked"));
    assert!(!snapshot.recovery_failure);

    // Active markers and admission occupancy must be cleared.
    // Snapshot does not expose worker_active; occupancy proves permit release.
    let occupancy = runtime.admission().occupancy();
    assert_eq!(
        occupancy.active_workers, 0,
        "worker panic must release admission permits"
    );

    // Journal must finish the open invocation before Failed.
    let journal_path = run_dir(dir.path(), &handle.run_id).join("journal.jsonl");
    let envelopes = collect_journal_v2(&journal_path, Some(&handle.run_id)).unwrap();
    let finished = envelopes.iter().any(|env| {
        matches!(
            &env.payload,
            JournalPayload::InvocationFinished {
                outcome: WorkflowInvocationOutcome {
                    status: WorkflowOutcomeStatus::Interrupted,
                    ..
                },
                ..
            }
        )
    });
    assert!(finished, "panic must durable-finish the open invocation");
}

#[tokio::test]
async fn rehydrate_starts_no_worker_and_preserves_awaiting_user() {
    let dir = tempfile::tempdir().unwrap();
    let run_id = WorkflowRunId::generate();
    let run_path = run_dir(dir.path(), &run_id);
    std::fs::create_dir_all(&run_path).unwrap();
    let meta = neo_agent_core::workflow::WorkflowRunMetadata {
        run_id: run_id.clone(),
        parent_run_id: None,
        name: "await".to_owned(),
        description: String::new(),
        phases: Vec::new(),
        script: "return nil".to_owned(),
        script_sha256: WorkflowRevision::from_bytes(b"return nil")
            .as_str()
            .to_owned(),
        args: serde_json::json!({}),
        launch_source: "/workflow".to_owned(),
        journal_format_version: 2,
        output_schema: None,

        display_name: None,
        input_schema: None,
        definition_origin: None,
        inline_unsaved: false,
    };
    neo_agent_core::workflow::write_run_metadata(&run_path, &meta, &WorkflowLimits::default())
        .unwrap();
    let journal_path = run_path.join("journal.jsonl");
    let mut writer = JournalV2Writer::open(&journal_path, run_id.clone()).unwrap();
    let limits = WorkflowLimits::default();
    for (seq, payload) in [
        JournalPayload::RunCreated {
            name: "await".to_owned(),
            description: None,
            launch_source: Some("/workflow".to_owned()),
        },
        JournalPayload::StateChanged {
            previous: WorkflowState::Queued,
            new: WorkflowState::Running,
            reason: "worker_start".to_owned(),
            actor: WorkflowActor::Runtime,
        },
        JournalPayload::UserInputRequested {
            request_id: "req_await".to_owned(),
            prompt: Some(serde_json::json!({"q": "continue?"})),
        },
        JournalPayload::StateChanged {
            previous: WorkflowState::Running,
            new: WorkflowState::AwaitingUser,
            reason: "user_input".to_owned(),
            actor: WorkflowActor::Runtime,
        },
    ]
    .into_iter()
    .enumerate()
    {
        let env = JournalEnvelope::new(seq as u64, 1_000 + seq as u64, run_id.clone(), payload);
        writer.append(&env, &limits).unwrap();
    }
    drop(writer);

    let starts = Arc::new(AtomicUsize::new(0));
    let runtime = WorkflowRuntime::default();
    runtime
        .bind_runner({
            let starts = Arc::clone(&starts);
            move |_handle, _metadata, _session_dir| {
                let starts = Arc::clone(&starts);
                async move {
                    starts.fetch_add(1, Ordering::AcqRel);
                    panic!("rehydrate must not start a worker");
                }
            }
        })
        .unwrap();

    let handles = runtime.rehydrate(dir.path()).await.unwrap();
    assert_eq!(handles.len(), 1);
    let snapshot = handles[0].snapshot().await;
    assert_eq!(snapshot.state, WorkflowState::AwaitingUser);
    assert!(!snapshot.recovery_failure);
    assert_eq!(
        starts.load(Ordering::Acquire),
        0,
        "rehydrate starts no worker"
    );
    assert_eq!(
        runtime.admission().occupancy().active_workers,
        0,
        "rehydrate must not take worker permits"
    );

    // AwaitingUser is preserved; ordinary resume still rejected.
    let err = handles[0]
        .resume(WorkflowActor::Human)
        .await
        .expect_err("ordinary resume must not bypass awaiting_user");
    assert_eq!(err.code(), WorkflowErrorCode::AwaitingUser);
    assert_eq!(starts.load(Ordering::Acquire), 0);
    assert_eq!(
        handles[0].snapshot().await.state,
        WorkflowState::AwaitingUser
    );
}
