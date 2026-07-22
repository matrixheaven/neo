use neo_agent_core::workflow::{
    JournalRecord, JournalWriter, WorkflowActor, WorkflowId, WorkflowInvocationKind,
    WorkflowInvocationOutcome, WorkflowLaunchRequest, WorkflowLimits, WorkflowOutcomeStatus,
    WorkflowPhase, WorkflowRuntime, WorkflowState, canonical_input_hash, find_incomplete_invocations,
    journal_path, read_journal,
};

fn test_limits() -> WorkflowLimits {
    WorkflowLimits::default()
}

async fn create_test_run(runtime: &WorkflowRuntime, session_dir: &std::path::Path) -> neo_agent_core::workflow::WorkflowHandle {
    runtime
        .create_run(
            session_dir,
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
            },
        )
        .await
        .expect("create run")
}

#[tokio::test]
async fn workflow_returns_running_before_worker_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(test_limits());

    let handle = create_test_run(&runtime, dir.path()).await;
    let snap = handle.snapshot().await;
    assert_eq!(snap.state, WorkflowState::Running);
    assert_eq!(snap.name, "test-run");
    assert!(!snap.run_id.0.is_empty());

    // run.json exists
    let rdir = neo_agent_core::workflow::run_dir(dir.path(), &handle.run_id);
    assert!(rdir.join("run.json").exists());

    // journal exists with initial state record
    let jpath = journal_path(dir.path(), &handle.run_id);
    assert!(jpath.exists());
    let records = read_journal(&jpath).unwrap();
    assert_eq!(records.len(), 1);
    assert!(matches!(records[0], JournalRecord::StateChanged { .. }));
}

#[tokio::test]
async fn incomplete_invocation_is_never_reexecuted() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(test_limits());

    let handle = create_test_run(&runtime, dir.path()).await;

    // Manually write an incomplete invocation into the journal
    let jpath = journal_path(dir.path(), &handle.run_id);
    let mut writer = JournalWriter::open(&jpath).unwrap();
    let limits = test_limits();
    writer
        .append(
            &JournalRecord::InvocationStarted {
                seq: writer.next_seq(),
                timestamp_ms: 2000,
                invocation_id: "inv_incomplete".to_owned(),
                call_index: 0,
                kind: WorkflowInvocationKind::Delegate,
                canonical_input: serde_json::json!({"task": "audit"}),
                canonical_input_hash: canonical_input_hash(&serde_json::json!({"task": "audit"})),
            },
            &limits,
        )
        .unwrap();

    let records = read_journal(&jpath).unwrap();
    let incomplete = find_incomplete_invocations(&records);
    assert!(!incomplete.is_empty());

    // Rehydrate should finish incomplete invocations as interrupted
    let runtime2 = WorkflowRuntime::new(test_limits());
    let handles = runtime2.rehydrate(dir.path()).await.unwrap();
    assert_eq!(handles.len(), 1);

    let recovered_records = read_journal(&jpath).unwrap();
    let has_finish = recovered_records.iter().any(|r| {
        matches!(r, JournalRecord::InvocationFinished {
            invocation_id,
            outcome: WorkflowInvocationOutcome {
                status: WorkflowOutcomeStatus::Interrupted,
                ..
            },
            ..
        } if invocation_id == "inv_incomplete")
    });
    assert!(has_finish);
}

#[tokio::test]
async fn host_exit_rehydrates_running_run_as_paused() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(test_limits());

    let handle = create_test_run(&runtime, dir.path()).await;
    let run_id = handle.run_id.clone();

    // Drop the runtime (simulating host exit) and create a new one
    drop(handle);
    drop(runtime);

    let runtime2 = WorkflowRuntime::new(test_limits());
    let handles = runtime2.rehydrate(dir.path()).await.unwrap();
    assert_eq!(handles.len(), 1);
    let snap = handles[0].snapshot().await;
    assert_eq!(snap.state, WorkflowState::Paused);
    assert_eq!(snap.terminal_reason.as_deref(), Some("host_exit"));
    assert_eq!(snap.run_id.0, run_id.0);
}

#[tokio::test]
async fn pause_and_resume_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(test_limits());

    let handle = create_test_run(&runtime, dir.path()).await;

    // Force state to running -> paused manually (normally via worker boundary)
    runtime
        .transition_state(&handle.run_id, WorkflowState::Paused, "test_pause", WorkflowActor::Human)
        .await
        .unwrap();

    let snap = handle.snapshot().await;
    assert_eq!(snap.state, WorkflowState::Paused);

    handle.resume(WorkflowActor::Human).await.unwrap();
    let snap2 = handle.snapshot().await;
    assert_eq!(snap2.state, WorkflowState::Running);
}

#[tokio::test]
async fn stop_cancels_workflow() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(test_limits());

    let handle = create_test_run(&runtime, dir.path()).await;

    handle.stop(WorkflowActor::Human).await.unwrap();
    let snap = handle.snapshot().await;
    assert_eq!(snap.state, WorkflowState::Cancelled);
}

#[tokio::test]
async fn terminal_workflow_has_terminal_child_records() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(test_limits());

    let handle = create_test_run(&runtime, dir.path()).await;

    // Simulate: start + finish a child invocation, then complete
    let jpath = journal_path(dir.path(), &handle.run_id);
    let limits = test_limits();
    let mut writer = JournalWriter::open(&jpath).unwrap();
    let inv_id = "inv_child";

    writer
        .append(
            &JournalRecord::InvocationStarted {
                seq: writer.next_seq(),
                timestamp_ms: 2000,
                invocation_id: inv_id.to_owned(),
                call_index: 0,
                kind: WorkflowInvocationKind::Delegate,
                canonical_input: serde_json::json!({"task": "audit"}),
                canonical_input_hash: canonical_input_hash(&serde_json::json!({"task": "audit"})),
            },
            &limits,
        )
        .unwrap();

    writer
        .append(
            &JournalRecord::InvocationFinished {
                seq: writer.next_seq(),
                timestamp_ms: 3000,
                invocation_id: inv_id.to_owned(),
                outcome: WorkflowInvocationOutcome {
                    ok: true,
                    status: WorkflowOutcomeStatus::Completed,
                    summary: "done".to_owned(),
                    details: serde_json::json!({}),
                    actual_usage: None,
                    child_refs: vec![],
                },
            },
            &limits,
        )
        .unwrap();

    // Now transition to completed
    runtime
        .transition_state(
            &handle.run_id,
            WorkflowState::Completed,
            "all children done",
            WorkflowActor::Runtime,
        )
        .await
        .unwrap();

    let records = read_journal(&jpath).unwrap();
    let incomplete = find_incomplete_invocations(&records);
    // No child should be incomplete when entering Completed
    assert!(incomplete.is_empty(), "incomplete: {incomplete:?}");
}

#[tokio::test]
async fn replay_prefix_matches_existing_records() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(test_limits());
    let handle = create_test_run(&runtime, dir.path()).await;

    let jpath = journal_path(dir.path(), &handle.run_id);
    let limits = test_limits();
    let mut writer = JournalWriter::open(&jpath).unwrap();

    let input = serde_json::json!({"task": "audit"});
    let hash = canonical_input_hash(&input);

    writer
        .append(
            &JournalRecord::InvocationStarted {
                seq: writer.next_seq(),
                timestamp_ms: 2000,
                invocation_id: "inv_1".to_owned(),
                call_index: 0,
                kind: WorkflowInvocationKind::Delegate,
                canonical_input: input.clone(),
                canonical_input_hash: hash.clone(),
            },
            &limits,
        )
        .unwrap();
    writer
        .append(
            &JournalRecord::InvocationFinished {
                seq: writer.next_seq(),
                timestamp_ms: 3000,
                invocation_id: "inv_1".to_owned(),
                outcome: WorkflowInvocationOutcome {
                    ok: true,
                    status: WorkflowOutcomeStatus::Completed,
                    summary: "done".to_owned(),
                    details: serde_json::json!({}),
                    actual_usage: None,
                    child_refs: vec![],
                },
            },
            &limits,
        )
        .unwrap();

    let records = read_journal(&jpath).unwrap();
    let new_calls = vec![
        (0u64, WorkflowInvocationKind::Delegate, input.clone()),
        (1u64, WorkflowInvocationKind::Delegate, serde_json::json!({"task": "build"})),
    ];
    let prefix =
        neo_agent_core::workflow::compute_replay_prefix(&records, &new_calls);
    assert_eq!(prefix.first_live_call_index, 1);
    assert!(!prefix.matched_records.is_empty());
}

#[tokio::test]
async fn snapshot_is_consistent_with_journal_state() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = WorkflowRuntime::new(test_limits());
    let handle = create_test_run(&runtime, dir.path()).await;

    let snap = runtime.snapshot(&handle.run_id).await.unwrap();
    assert_eq!(snap.state, WorkflowState::Running);

    // transition to failed
    runtime
        .transition_state(&handle.run_id, WorkflowState::Failed, "error", WorkflowActor::Runtime)
        .await
        .unwrap();

    let snap2 = runtime.snapshot(&handle.run_id).await.unwrap();
    assert_eq!(snap2.state, WorkflowState::Failed);
    assert!(snap2.terminal_reason.is_some());
}
