use super::*;

#[tokio::test]
async fn persisted_terminal_elapsed_rehydrates_from_durable_timestamps() {
    let tasks = tempfile::tempdir().expect("tasks");
    tokio::fs::write(
        tasks.path().join("bash-elapsed.status.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "task_id": "bash-elapsed",
            "started_at_ms": 100,
            "finished_at_ms": 650,
            "exit": {
                "status": "completed",
                "exit_code": 0,
                "signal": null,
                "resource_limit": null,
                "omitted_output_bytes": 0,
                "omitted_log_bytes": 0
            },
            "cleanup_errors": []
        }))
        .expect("serialize status"),
    )
    .await
    .expect("write status");
    let manager = BackgroundTaskManager::new().with_persistence_dir(tasks.path().to_path_buf());

    let snapshot = manager
        .snapshot("bash-elapsed")
        .await
        .expect("rehydrated snapshot");
    assert_eq!(snapshot.elapsed, Duration::from_millis(550));
    assert_eq!(
        manager.list_metadata(false).await[0].elapsed,
        Duration::from_millis(550)
    );
}

#[tokio::test]
async fn resume_converges_stale_running_guard_without_claiming_status_file() {
    let tasks = tempfile::tempdir().expect("tasks");
    let task_id = "bash-stale";
    let running = tasks.path().join(format!("{task_id}.running.json"));
    let final_status = tasks.path().join(format!("{task_id}.status.json"));
    tokio::fs::write(
        &running,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "task_id": task_id,
            "guardian_pid": 1,
            "started_at_ms": 1
        }))
        .expect("serialize running marker"),
    )
    .await
    .expect("write running marker");
    let manager = BackgroundTaskManager::new().with_persistence_dir(tasks.path().to_path_buf());

    let snapshot = manager.snapshot(task_id).await.expect("restore stale task");

    assert_eq!(snapshot.status, BackgroundTaskStatus::ParentExited);
    assert!(!final_status.exists());
}

#[tokio::test]
async fn persisted_recovery_rejects_untrusted_task_identity() {
    let task_id = "bash-target";
    for suffix in ["status", "running"] {
        for (schema_version, persisted_task_id) in [(2, task_id), (1, "bash-other")] {
            let tasks = tempfile::tempdir().expect("tasks");
            let record = if suffix == "status" {
                json!({
                    "schema_version": schema_version,
                    "task_id": persisted_task_id,
                    "started_at_ms": 1,
                    "finished_at_ms": 2,
                    "exit": {
                        "status": "completed",
                        "exit_code": 0,
                        "signal": null,
                        "resource_limit": null,
                        "omitted_output_bytes": 0,
                        "omitted_log_bytes": 0
                    },
                    "cleanup_errors": []
                })
            } else {
                json!({
                    "schema_version": schema_version,
                    "task_id": persisted_task_id,
                    "guardian_pid": 1,
                    "started_at_ms": 1
                })
            };
            tokio::fs::write(
                tasks.path().join(format!("{task_id}.{suffix}.json")),
                serde_json::to_vec(&record).expect("serialize record"),
            )
            .await
            .expect("write persisted record");
            let manager =
                BackgroundTaskManager::new().with_persistence_dir(tasks.path().to_path_buf());

            let Err(error) = manager.snapshot(task_id).await else {
                panic!("invalid persisted identity must not restore the target task")
            };

            assert!(matches!(
                &error,
                ToolError::Io(error) if error.kind() == std::io::ErrorKind::InvalidData
            ));
            assert!(error.to_string().contains("recover background task"));
        }
    }
}

#[tokio::test]
async fn persisted_recovery_rejects_unsafe_task_id_before_path_resolution() {
    let tasks = tempfile::tempdir().expect("tasks");
    let manager = BackgroundTaskManager::new().with_persistence_dir(tasks.path().to_path_buf());

    for task_id in ["../escape", r"..\escape", "C:escape", "bash:stream"] {
        let Err(error) = manager.persisted_snapshot(task_id, true).await else {
            panic!("unsafe task id must be rejected")
        };

        assert!(matches!(
            &error,
            ToolError::Io(error) if error.kind() == std::io::ErrorKind::InvalidData
        ));
    }
}

#[tokio::test]
async fn non_blocking_persisted_reads_do_not_wait_for_stale_tasks() {
    let tasks = tempfile::tempdir().expect("tasks");
    for task_id in ["bash-stale-one", "bash-stale-two"] {
        tokio::fs::write(
            tasks.path().join(format!("{task_id}.running.json")),
            serde_json::to_vec(&json!({
                "schema_version": 1,
                "task_id": task_id,
                "guardian_pid": 1,
                "started_at_ms": 1
            }))
            .expect("serialize running marker"),
        )
        .await
        .expect("write running marker");
    }
    let manager = BackgroundTaskManager::new().with_persistence_dir(tasks.path().to_path_buf());

    let (output, snapshots) = tokio::time::timeout(Duration::from_secs(1), async {
        let output = manager
            .output("bash-stale-one", false, Duration::from_secs(30), 1024)
            .await
            .expect("read stale task output");
        (output, manager.list(false, 10).await)
    })
    .await
    .expect("non-blocking reads must not poll for final status");

    assert_eq!(output.details.as_ref().unwrap()["status"], "parent_exited");
    assert_eq!(snapshots.len(), 2);
    assert!(
        snapshots
            .iter()
            .all(|snapshot| snapshot.status == BackgroundTaskStatus::ParentExited)
    );

    let task_id = "bash-disappeared";
    let running_path = tasks.path().join(format!("{task_id}.running.json"));
    tokio::fs::write(
        &running_path,
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "task_id": task_id,
            "guardian_pid": 1,
            "started_at_ms": 1
        }))
        .expect("serialize running marker"),
    )
    .await
    .expect("write running marker");
    assert!(
        BackgroundTaskManager::read_persisted_running(task_id, &running_path)
            .await
            .expect("first running read")
    );
    tokio::fs::remove_file(&running_path)
        .await
        .expect("remove running marker between reads");
    let state = BackgroundTaskManager::read_after_first_running(
        tasks.path(),
        task_id,
        &tasks.path().join(format!("{task_id}.status.json")),
        &running_path,
        true,
    )
    .await
    .expect("second persisted lookup");
    assert!(matches!(state, PersistedTaskFiles::Missing));
}

#[test]
fn disappeared_persisted_identity_is_not_reported_as_parent_exited() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let tasks = tempfile::tempdir().expect("tasks");
        let task_id = "bash-race";
        let running = tasks.path().join(format!("{task_id}.running.json"));
        std::fs::write(
            &running,
            serde_json::to_vec(&json!({
                "schema_version": 1,
                "task_id": task_id,
                "guardian_pid": 1,
                "started_at_ms": 1
            }))
            .expect("serialize running marker"),
        )
        .expect("write running marker");

        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let release =
            std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let blocker_release = release.clone();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).expect("signal blocker");
            let (lock, condvar) = &*blocker_release;
            let mut released = lock.lock().expect("lock release");
            while !*released {
                released = condvar.wait(released).expect("wait release");
            }
        });
        started_rx.recv().expect("blocking worker started");

        let manager = BackgroundTaskManager::new().with_persistence_dir(tasks.path().to_path_buf());
        let mut recovery = Box::pin(manager.persisted_snapshot(task_id, true));
        assert!(matches!(
            futures::poll!(&mut recovery),
            std::task::Poll::Pending
        ));
        std::fs::remove_file(running).expect("remove running marker");
        let (lock, condvar) = &*release;
        *lock.lock().expect("lock release") = true;
        condvar.notify_one();
        blocker.await.expect("blocking worker");

        assert!(
            recovery
                .await
                .expect("recover disappeared marker")
                .is_none()
        );
    });
}
