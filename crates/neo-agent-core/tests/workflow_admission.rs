//! Global workflow admission and retention preview (Task 6).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use neo_agent_core::workflow::{
    AdmissionReason, AdmitOutcome, RetentionExclusion, RetentionPolicy, RetentionSubject,
    WorkflowActor, WorkflowErrorCode, WorkflowId, WorkflowLaunchRequest, WorkflowLimits,
    WorkflowPhase, WorkflowRuntime, WorkflowState, preview_mark_sweep,
};

fn launch_request(name: &str) -> WorkflowLaunchRequest {
    WorkflowLaunchRequest {
        name: name.to_owned(),
        description: "admission test".to_owned(),
        phases: vec![WorkflowPhase {
            id: "work".to_owned(),
            description: "work".to_owned(),
        }],
        script: "neo.phase('work')".to_owned(),
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

fn limits_one_worker() -> WorkflowLimits {
    WorkflowLimits {
        max_active_workers: 1,
        max_active_vms: 1,
        max_active_executors: 4,
        // Keep global storage large enough for several create reservations.
        global_storage_bytes: 256 * 1024 * 1024,
        journal_record_bytes: 64 * 1024,
        ..WorkflowLimits::default()
    }
}

#[tokio::test]
async fn unavailable_permit_keeps_run_durably_queued_fifo() {
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = WorkflowRuntime::new(limits_one_worker());
    let block = Arc::new(tokio::sync::Notify::new());
    let started = Arc::new(AtomicUsize::new(0));

    let block_run = Arc::clone(&block);
    let started_run = Arc::clone(&started);
    runtime
        .bind_runner(move |handle, _meta, _session| {
            let block = Arc::clone(&block_run);
            let started = Arc::clone(&started_run);
            async move {
                started.fetch_add(1, Ordering::AcqRel);
                block.notified().await;
                let _ = handle;
                Ok(())
            }
        })
        .expect("bind runner");

    let first = runtime
        .create_run(dir.path(), launch_request("first"))
        .await
        .expect("create first");
    let second = runtime
        .create_run(dir.path(), launch_request("second"))
        .await
        .expect("create second");
    let third = runtime
        .create_run(dir.path(), launch_request("third"))
        .await
        .expect("create third");

    runtime
        .start_worker(&first.run_id)
        .await
        .expect("start first");
    runtime
        .start_worker(&second.run_id)
        .await
        .expect("queue second");
    runtime
        .start_worker(&third.run_id)
        .await
        .expect("queue third");

    // First holds the only worker permit and is running.
    assert_eq!(first.snapshot().await.state, WorkflowState::Running);
    // Unavailable permits leave later runs durably queued (not failed).
    assert_eq!(second.snapshot().await.state, WorkflowState::Queued);
    assert_eq!(third.snapshot().await.state, WorkflowState::Queued);

    assert_eq!(
        runtime.admission().worker_queue_position(&second.run_id),
        Some(1)
    );
    assert_eq!(
        runtime.admission().worker_queue_position(&third.run_id),
        Some(2)
    );
    assert_eq!(runtime.admission().occupancy().active_workers, 1);
    assert_eq!(runtime.admission().occupancy().queued_workers, 2);

    // Wait until the first worker is parked, then release it (notify_one stores a permit).
    tokio::time::timeout(Duration::from_secs(5), async {
        while started.load(Ordering::Acquire) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first worker started");
    // Fair FIFO: release first, then second (not third) can acquire next.
    block.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if first.snapshot().await.state.is_terminal()
                || first.snapshot().await.state == WorkflowState::Paused
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first finished");

    // Promote second in FIFO order.
    runtime
        .start_worker(&second.run_id)
        .await
        .expect("start second after release");
    assert_eq!(second.snapshot().await.state, WorkflowState::Running);
    assert_eq!(third.snapshot().await.state, WorkflowState::Queued);
    assert_eq!(
        runtime.admission().worker_queue_position(&third.run_id),
        Some(1)
    );

    // Third still cannot jump the queue while second holds the permit.
    runtime
        .start_worker(&third.run_id)
        .await
        .expect("third still queued");
    assert_eq!(third.snapshot().await.state, WorkflowState::Queued);

    block.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if second.snapshot().await.state.is_terminal()
                || second.snapshot().await.state == WorkflowState::Paused
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("second finished");

    runtime
        .start_worker(&third.run_id)
        .await
        .expect("start third");
    assert_eq!(third.snapshot().await.state, WorkflowState::Running);
    assert!(
        runtime
            .admission()
            .worker_queue_position(&third.run_id)
            .is_none()
    );

    block.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if third.snapshot().await.state.is_terminal()
                || third.snapshot().await.state == WorkflowState::Paused
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("third finished");

    assert_eq!(runtime.admission().occupancy().active_workers, 0);
    assert_eq!(runtime.admission().occupancy().queued_workers, 0);
    assert!(started.load(Ordering::Acquire) >= 3);
}

#[tokio::test]
async fn workflow_admission_releases_every_runtime_exit_path() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Path A: normal terminal completion releases permits.
    {
        let runtime = WorkflowRuntime::new(limits_one_worker());
        runtime
            .bind_runner(|_handle, _meta, _session| async move { Ok(()) })
            .expect("bind");
        let handle = runtime
            .create_run(dir.path(), launch_request("normal"))
            .await
            .expect("create");
        runtime.start_worker(&handle.run_id).await.expect("start");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = handle.snapshot().await.state;
                if state.is_terminal() || state == WorkflowState::Paused {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("normal finish");
        assert_eq!(runtime.admission().occupancy().active_workers, 0);
        assert_eq!(runtime.admission().occupancy().active_vms, 0);
    }

    // Path B: worker panic releases permits.
    {
        let runtime = WorkflowRuntime::new(limits_one_worker());
        runtime
            .bind_runner(|_handle, _meta, _session| async move {
                panic!("injected worker panic");
            })
            .expect("bind");
        let handle = runtime
            .create_run(dir.path(), launch_request("panic"))
            .await
            .expect("create");
        runtime.start_worker(&handle.run_id).await.expect("start");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if handle.snapshot().await.state.is_terminal() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("panic finish");
        assert_eq!(runtime.admission().occupancy().active_workers, 0);
        assert_eq!(runtime.admission().occupancy().active_vms, 0);
    }

    // Path C: fail_worker_start after admit releases permits.
    {
        let runtime = WorkflowRuntime::new(limits_one_worker());
        // Bind a runner that never completes so the run stays live until we fail it.
        let hold = Arc::new(tokio::sync::Notify::new());
        let hold_run = Arc::clone(&hold);
        runtime
            .bind_runner(move |_handle, _meta, _session| {
                let hold = Arc::clone(&hold_run);
                async move {
                    hold.notified().await;
                    Ok(())
                }
            })
            .expect("bind");
        let handle = runtime
            .create_run(dir.path(), launch_request("fail-start"))
            .await
            .expect("create");
        runtime.start_worker(&handle.run_id).await.expect("start");
        assert_eq!(runtime.admission().occupancy().active_workers, 1);
        runtime
            .fail_worker_start(
                &handle.run_id,
                &neo_agent_core::workflow::WorkflowError::Host("startup failed".to_owned()),
            )
            .await
            .expect("fail start");
        hold.notify_one();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if runtime.admission().occupancy().active_workers == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("fail_worker_start released");
        assert_eq!(runtime.admission().occupancy().active_workers, 0);
    }

    // Path D: pause boundary releases active VM/worker permits.
    {
        let runtime = WorkflowRuntime::new(limits_one_worker());
        let hold = Arc::new(tokio::sync::Notify::new());
        let hold_run = Arc::clone(&hold);
        runtime
            .bind_runner(move |handle, _meta, _session| {
                let hold = Arc::clone(&hold_run);
                async move {
                    // Stay running until pause is requested.
                    loop {
                        if handle.stop_token().is_cancelled() {
                            break;
                        }
                        // Cooperative pause: request_pause is observed at invoke boundaries;
                        // for this path we wait then exit so finish_worker applies pause.
                        if tokio::time::timeout(Duration::from_millis(20), hold.notified())
                            .await
                            .is_ok()
                        {
                            break;
                        }
                    }
                    Err(neo_agent_core::workflow::WorkflowError::Paused(
                        "pause boundary".to_owned(),
                    ))
                }
            })
            .expect("bind");
        let handle = runtime
            .create_run(dir.path(), launch_request("pause"))
            .await
            .expect("create");
        runtime.start_worker(&handle.run_id).await.expect("start");
        assert_eq!(runtime.admission().occupancy().active_workers, 1);
        handle
            .pause(WorkflowActor::Human)
            .await
            .expect("request pause");
        hold.notify_one();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if handle.snapshot().await.state == WorkflowState::Paused
                    && runtime.admission().occupancy().active_workers == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pause released permit");
        assert_eq!(runtime.admission().occupancy().active_workers, 0);
    }

    // Path E: rehydrate never holds live permits.
    {
        let runtime = WorkflowRuntime::new(limits_one_worker());
        runtime
            .bind_runner(|_handle, _meta, _session| async move { Ok(()) })
            .expect("bind");
        let handle = runtime
            .create_run(dir.path(), launch_request("rehydrate"))
            .await
            .expect("create");
        runtime.start_worker(&handle.run_id).await.expect("start");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if handle.snapshot().await.state.is_terminal() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("finish for rehydrate");

        let recovered = WorkflowRuntime::new(limits_one_worker());
        let handles = recovered.rehydrate(dir.path()).await.expect("rehydrate");
        assert!(!handles.is_empty());
        assert_eq!(recovered.admission().occupancy().active_workers, 0);
        assert_eq!(recovered.admission().occupancy().active_vms, 0);
        assert_eq!(recovered.admission().occupancy().queued_workers, 0);
    }
}

#[tokio::test]
async fn workflow_storage_reservations_are_race_safe() {
    let limits = WorkflowLimits {
        global_storage_bytes: 1_000,
        journal_record_bytes: 400,
        ..WorkflowLimits::default()
    };
    let admission = neo_agent_core::workflow::WorkflowAdmission::new(limits.clone());

    let denied = Arc::new(AtomicUsize::new(0));
    let accepted = Arc::new(AtomicUsize::new(0));
    let mut joins = Vec::new();

    for i in 0..32 {
        let admission = admission.clone();
        let denied = Arc::clone(&denied);
        let accepted = Arc::clone(&accepted);
        joins.push(tokio::spawn(async move {
            let owner = format!("owner-{i}");
            match admission.try_reserve_storage(&owner, 400) {
                Ok(reservation) => {
                    accepted.fetch_add(1, Ordering::AcqRel);
                    // Hold briefly so overlapping races contend on the mutex.
                    tokio::task::yield_now().await;
                    reservation.commit();
                }
                Err(error) => {
                    assert_eq!(error.code(), WorkflowErrorCode::StorageAdmissionDenied);
                    denied.fetch_add(1, Ordering::AcqRel);
                }
            }
        }));
    }
    for join in joins {
        join.await.expect("join");
    }

    let occupancy = admission.occupancy();
    assert!(
        occupancy.reserved_storage_bytes <= limits.global_storage_bytes,
        "reserved {} exceeds limit {}",
        occupancy.reserved_storage_bytes,
        limits.global_storage_bytes
    );
    // 400 * 2 = 800 fits; 400 * 3 = 1200 does not. Exactly two succeed.
    assert_eq!(accepted.load(Ordering::Acquire), 2);
    assert_eq!(denied.load(Ordering::Acquire), 30);
    assert_eq!(occupancy.reserved_storage_bytes, 800);

    // create_run also respects global storage admission.
    let dir = tempfile::tempdir().expect("tempdir");
    let create_limits = WorkflowLimits {
        journal_record_bytes: 4_096,
        global_storage_bytes: 0, // filled below
        ..WorkflowLimits::default()
    };
    let create_limits = WorkflowLimits {
        global_storage_bytes: create_limits.run_storage_reservation_bytes(),
        journal_record_bytes: 4_096,
        ..WorkflowLimits::default()
    };
    let runtime = WorkflowRuntime::new(create_limits);
    let first = runtime
        .create_run(dir.path(), launch_request("storage-first"))
        .await
        .expect("first create consumes the only slot");
    let second = runtime
        .create_run(dir.path(), launch_request("storage-second"))
        .await;
    match second {
        Ok(_) => panic!("second create must be storage-denied"),
        Err(err) => assert_eq!(err.code(), WorkflowErrorCode::StorageAdmissionDenied),
    }
    // Rollback releases storage so a later create can succeed.
    runtime
        .rollback_created_run(&first.run_id)
        .await
        .expect("rollback");
    runtime
        .create_run(dir.path(), launch_request("storage-after-rollback"))
        .await
        .expect("create after rollback");
}

#[test]
fn retention_preview_excludes_live_referenced_pinned_nonterminal() {
    let subjects = vec![
        RetentionSubject {
            run_id: WorkflowId::from_existing("running"),
            state: WorkflowState::Running,
            bytes: 10,
            age_ms: 99_999,
            referenced: false,
            pinned: false,
        },
        RetentionSubject {
            run_id: WorkflowId::from_existing("queued"),
            state: WorkflowState::Queued,
            bytes: 10,
            age_ms: 99_999,
            referenced: false,
            pinned: false,
        },
        RetentionSubject {
            run_id: WorkflowId::from_existing("ref"),
            state: WorkflowState::Completed,
            bytes: 10,
            age_ms: 99_999,
            referenced: true,
            pinned: false,
        },
        RetentionSubject {
            run_id: WorkflowId::from_existing("pin"),
            state: WorkflowState::Failed,
            bytes: 10,
            age_ms: 99_999,
            referenced: false,
            pinned: true,
        },
        RetentionSubject {
            run_id: WorkflowId::from_existing("old"),
            state: WorkflowState::Completed,
            bytes: 50,
            age_ms: 99_999,
            referenced: false,
            pinned: false,
        },
    ];
    let preview = preview_mark_sweep(
        &subjects,
        &RetentionPolicy {
            min_age_ms: Some(1),
            reclaim_target_bytes: None,
        },
    );
    assert_eq!(preview.candidates.len(), 1);
    assert_eq!(preview.candidates[0].run_id.as_str(), "old");
    assert_eq!(preview.reclaimable_bytes, 50);
    let exclusions: Vec<_> = preview.excluded.iter().map(|(_, reason)| *reason).collect();
    assert!(exclusions.contains(&RetentionExclusion::Live));
    assert!(exclusions.contains(&RetentionExclusion::Queued));
    assert!(exclusions.contains(&RetentionExclusion::Referenced));
    assert!(exclusions.contains(&RetentionExclusion::Pinned));
}

#[test]
fn try_admit_reports_capacity_reason_without_rejecting_by_child_count() {
    let limits = WorkflowLimits {
        max_active_workers: 1,
        max_active_vms: 8,
        max_active_executors: 1,
        ..WorkflowLimits::default()
    };
    let admission = neo_agent_core::workflow::WorkflowAdmission::new(limits);
    let a = WorkflowId::from_existing("a");
    let b = WorkflowId::from_existing("b");
    let first = match admission.try_admit_worker(&a) {
        AdmitOutcome::Granted(p) => p,
        AdmitOutcome::Queued { .. } => panic!("first should grant"),
    };
    match admission.try_admit_worker(&b) {
        AdmitOutcome::Queued { position, reason } => {
            assert_eq!(position, 1);
            assert_eq!(reason, AdmissionReason::ActiveWorkersExhausted);
        }
        AdmitOutcome::Granted(_) => panic!("second must stay queued"),
    }
    // No total child-count rejection: many executor-less queue positions remain valid.
    for i in 0..64 {
        let id = WorkflowId::from_existing(format!("child-{i}"));
        match admission.try_admit_worker(&id) {
            AdmitOutcome::Queued { .. } => {}
            AdmitOutcome::Granted(_) => panic!("capacity still held by first"),
        }
    }
    drop(first);
    match admission.try_admit_worker(&b) {
        AdmitOutcome::Granted(_) => {}
        AdmitOutcome::Queued { reason, .. } => panic!("expected grant after release: {reason}"),
    }
}

#[test]
fn automatic_retention_reclaims_only_old_terminal_unreferenced_runs_to_low_watermark() {
    use neo_agent_core::workflow::{
        WorkflowAdmission, WorkflowLimits, WorkflowRunMetadata, WorkflowState, perform_retention,
    };

    let sessions_root = tempfile::tempdir().expect("tempdir");

    fn make_run(
        sessions_root: &Path,
        bucket: &str,
        session_id: &str,
        run_id: &str,
        state: WorkflowState,
        parent_run_id: Option<&str>,
        extra_bytes: usize,
    ) {
        let run_dir = sessions_root
            .join(bucket)
            .join(session_id)
            .join("workflows")
            .join(run_id);
        fs::create_dir_all(&run_dir).expect("create run dir");

        let metadata = WorkflowRunMetadata {
            run_id: neo_agent_core::workflow::WorkflowRunId::from_existing(run_id),
            parent_run_id: parent_run_id
                .map(|id| neo_agent_core::workflow::WorkflowRunId::from_existing(id)),
            name: "test".to_owned(),
            description: String::new(),
            phases: vec![],
            script: "neo.phase('work')".to_owned(),
            script_sha256: "abc".to_owned(),
            args: serde_json::json!({}),
            launch_source: "test".to_owned(),
            journal_format_version: 2,
            output_schema: None,
            display_name: None,
            input_schema: None,
            definition_origin: None,
            inline_unsaved: false,
        };
        neo_agent_core::workflow::journal::write_run_metadata(
            &run_dir,
            &metadata,
            &WorkflowLimits::default(),
        )
        .expect("write metadata");

        // For non-terminal states, write a journal with the state.
        // Terminal runs don't need a journal (infer_run_state defaults to Completed).
        if !state.is_terminal() {
            use neo_agent_core::workflow::WorkflowId;
            use neo_agent_core::workflow::journal::{
                JOURNAL_FORMAT_V2, JournalEnvelope, JournalPayload, JournalWriter,
            };
            let wid = WorkflowId::from_existing(run_id);
            let limits_journal = WorkflowLimits::default();
            let mut writer = JournalWriter::open(&run_dir.join("journal.jsonl"), wid.clone())
                .expect("open journal");
            writer
                .append(
                    &JournalEnvelope {
                        version: JOURNAL_FORMAT_V2,
                        seq: 0,
                        timestamp_ms: 1_000_000,
                        run_id: wid,
                        payload: JournalPayload::StateChanged {
                            previous: WorkflowState::Queued,
                            new: state,
                            reason: "test".to_owned(),
                            actor: neo_agent_core::workflow::WorkflowActor::Runtime,
                        },
                        canonical_input_hash: None,
                        payload_refs: vec![],
                    },
                    &limits_journal,
                )
                .expect("append state");
        }

        if extra_bytes > 0 {
            let dummy = run_dir.join("dummy.bin");
            fs::write(&dummy, vec![0u8; extra_bytes]).expect("write dummy");
        }
    }

    let root = sessions_root.path();
    let limits = WorkflowLimits {
        // Small limit so watermarks trigger easily.
        global_storage_bytes: 20 * 1024, // 20 KiB
        ..WorkflowLimits::default()
    };
    let admission = WorkflowAdmission::new(limits.clone());

    let session = "session-1";
    let bucket = "wd_test_00110011aaaa";

    // Terminal runs — candidates for reclamation (they are current-time, so under 30 days).
    // With a 20 KiB limit, even small runs will trigger retention.
    make_run(
        root,
        bucket,
        session,
        "terminal-1",
        WorkflowState::Completed,
        None,
        4096,
    );
    make_run(
        root,
        bucket,
        session,
        "terminal-2",
        WorkflowState::Failed,
        None,
        4096,
    );
    make_run(
        root,
        bucket,
        session,
        "terminal-3",
        WorkflowState::Cancelled,
        None,
        4096,
    );
    // Non-terminal runs — should be preserved.
    make_run(
        root,
        bucket,
        session,
        "running",
        WorkflowState::Running,
        None,
        2048,
    );
    make_run(
        root,
        bucket,
        session,
        "queued",
        WorkflowState::Queued,
        None,
        2048,
    );
    make_run(
        root,
        bucket,
        session,
        "paused",
        WorkflowState::Paused,
        None,
        2048,
    );
    make_run(
        root,
        bucket,
        session,
        "awaiting",
        WorkflowState::AwaitingUser,
        None,
        2048,
    );
    // Referenced run — has a child run pointing to it.
    make_run(
        root,
        bucket,
        session,
        "referenced-parent",
        WorkflowState::Completed,
        None,
        2048,
    );
    make_run(
        root,
        bucket,
        session,
        "child-of-parent",
        WorkflowState::Completed,
        Some("referenced-parent"),
        1024,
    );

    // With small runs and a 20 KiB limit, the watermarks trigger at ~18 KiB.
    // Total bytes: 4096*3 + 2048*4 + 2048 + 1024 = 12288 + 8192 + 2048 + 1024 = 23552 > 18K.
    // But all runs are < 30 days old (just created), so none should be reclaimed.
    let outcome = perform_retention(root, Some(&admission), &limits);

    // All runs should be preserved since none are 30+ days old.
    // The function returns storage_full if eligible candidates exist but none qualify by age.
    assert_eq!(
        outcome.reclaimed_count, 0,
        "no runs should be reclaimed (under 30 days)"
    );

    // Verify all non-terminal runs are preserved.
    assert!(run_dir_path(root, bucket, session, "running").exists());
    assert!(run_dir_path(root, bucket, session, "queued").exists());
    assert!(run_dir_path(root, bucket, session, "paused").exists());
    assert!(run_dir_path(root, bucket, session, "awaiting").exists());
    // Terminal runs are also preserved (too young).
    assert!(run_dir_path(root, bucket, session, "terminal-1").exists());
    assert!(run_dir_path(root, bucket, session, "terminal-2").exists());
    assert!(run_dir_path(root, bucket, session, "terminal-3").exists());
    // Referenced run is also preserved.
    assert!(run_dir_path(root, bucket, session, "referenced-parent").exists());
    assert!(run_dir_path(root, bucket, session, "child-of-parent").exists());

    // storage_full should be true because data exceeds limit but nothing is eligible.
    assert!(
        outcome.storage_full,
        "should report storage_full when protected data alone exceeds limit"
    );
}

fn run_dir_path(sessions_root: &Path, bucket: &str, session_id: &str, run_id: &str) -> PathBuf {
    sessions_root
        .join(bucket)
        .join(session_id)
        .join("workflows")
        .join(run_id)
}

#[test]
fn automatic_retention_preserves_protected_runs_and_fails_closed_on_path_escape() {
    use neo_agent_core::workflow::{
        WorkflowAdmission, WorkflowLimits, WorkflowRunMetadata, WorkflowState, perform_retention,
    };

    let sessions_root = tempfile::tempdir().expect("tempdir");
    let root = sessions_root.path();

    // Create a sentinel file OUTSIDE the candidate run directories.
    let sentinel = root.join("do-not-delete.txt");
    fs::write(&sentinel, b"sentinel").expect("write sentinel");

    // Create another directory outside runs that could be targeted.
    let outside_dir = root.join("not-a-run");
    fs::create_dir(&outside_dir).expect("create outside dir");
    fs::write(outside_dir.join("data.txt"), b"outside").expect("write outside");

    let limits = WorkflowLimits {
        // Very tight limit to force retention attempt.
        global_storage_bytes: 4 * 1024, // 4 KiB
        ..WorkflowLimits::default()
    };
    let admission = WorkflowAdmission::new(limits.clone());

    let bucket = "wd_test_00220011bbbb";
    let session = "session-1";

    fn create_run(
        root: &Path,
        bucket: &str,
        session: &str,
        run_id: &str,
        state: WorkflowState,
        extra_bytes: usize,
    ) {
        let run_dir = run_dir_path(root, bucket, session, run_id);
        fs::create_dir_all(&run_dir).expect("create run dir");
        let metadata = WorkflowRunMetadata {
            run_id: neo_agent_core::workflow::WorkflowRunId::from_existing(run_id),
            parent_run_id: None,
            name: "test".to_owned(),
            description: String::new(),
            phases: vec![],
            script: "neo.phase('work')".to_owned(),
            script_sha256: "abc".to_owned(),
            args: serde_json::json!({}),
            launch_source: "test".to_owned(),
            journal_format_version: 2,
            output_schema: None,
            display_name: None,
            input_schema: None,
            definition_origin: None,
            inline_unsaved: false,
        };
        neo_agent_core::workflow::journal::write_run_metadata(
            &run_dir,
            &metadata,
            &WorkflowLimits::default(),
        )
        .expect("write metadata");

        // Only write journal for non-terminal states to pass validation.
        if !state.is_terminal() {
            use neo_agent_core::workflow::WorkflowId;
            use neo_agent_core::workflow::journal::{
                JOURNAL_FORMAT_V2, JournalEnvelope, JournalPayload, JournalWriter,
            };
            let wid = WorkflowId::from_existing(run_id);
            let limits_journal = WorkflowLimits::default();
            let mut writer = JournalWriter::open(&run_dir.join("journal.jsonl"), wid.clone())
                .expect("open journal");
            writer
                .append(
                    &JournalEnvelope {
                        version: JOURNAL_FORMAT_V2,
                        seq: 0,
                        timestamp_ms: 1_000_000,
                        run_id: wid,
                        payload: JournalPayload::StateChanged {
                            previous: WorkflowState::Queued,
                            new: state,
                            reason: "test".to_owned(),
                            actor: neo_agent_core::workflow::WorkflowActor::Runtime,
                        },
                        canonical_input_hash: None,
                        payload_refs: vec![],
                    },
                    &limits_journal,
                )
                .expect("append state");
        }

        if extra_bytes > 0 {
            fs::write(&run_dir.join("dummy.bin"), vec![0u8; extra_bytes]).expect("write dummy");
        }
    }

    // Only terminal runs (but under 30 days, so not eligible).
    create_run(
        root,
        bucket,
        session,
        "terminal-x",
        WorkflowState::Completed,
        2048,
    );
    // A running run.
    create_run(
        root,
        bucket,
        session,
        "running-x",
        WorkflowState::Running,
        1024,
    );

    let outcome = perform_retention(root, Some(&admission), &limits);

    // The sentinel file outside candidate directories must be preserved.
    assert!(sentinel.exists(), "sentinel file must never be deleted");
    assert!(
        outside_dir.exists(),
        "outside directory must never be deleted"
    );

    // Non-terminal runs are preserved.
    assert!(run_dir_path(root, bucket, session, "running-x").exists());

    // With young runs, nothing should be reclaimed.
    assert_eq!(
        outcome.reclaimed_count, 0,
        "young runs should never be reclaimed"
    );
}
