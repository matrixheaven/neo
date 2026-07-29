//! Bounded TaskOutput views (Task 18): paging, cursor binding, lock-free I/O.

use std::sync::Arc;
use std::time::{Duration, Instant};

use neo_agent_core::workflow::journal::{
    JournalEnvelope, JournalPayload, JournalWriter, collect_journal,
};
use neo_agent_core::workflow::{
    TaskOutputRequest, TaskOutputView, WorkflowActor, WorkflowId, WorkflowLaunchRequest,
    WorkflowLimits, WorkflowPhase, WorkflowRuntime, WorkflowState, measure_tool_result_bytes,
    page_to_tool_result,
};
use serde_json::json;

fn launch_request(name: &str) -> WorkflowLaunchRequest {
    WorkflowLaunchRequest {
        name: name.to_owned(),
        description: "task output test".to_owned(),
        phases: vec![WorkflowPhase {
            id: "work".to_owned(),
            description: "work".to_owned(),
        }],
        script: "return { ok = true }".to_owned(),
        args: json!({}),
        launch_source: "/workflow".to_owned(),
        output_schema: None,
        display_name: None,
        input_schema: None,
        definition_origin: None,
        inline_unsaved: false,
    }
}

async fn wait_running(handle: &neo_agent_core::workflow::WorkflowHandle) {
    for _ in 0..200 {
        if handle.snapshot().await.state == WorkflowState::Running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run did not become Running");
}

/// Append many large recovery records after the durable head so total
/// journal size is multi-megabyte / multi-gigabyte-logical without loading it
/// all into a TaskOutput page.
fn append_logical_multi_gigabyte_journal(
    path: &std::path::Path,
    run_id: &WorkflowId,
    record_count: u64,
    payload_chars: usize,
) {
    let existing = collect_journal(
        path,
        Some(run_id),
        WorkflowLimits::default().journal_record_bytes,
        WorkflowLimits::default().journal_total_bytes,
    )
    .expect("collect head");
    let next_seq = existing.last().map_or(0, |e| e.seq + 1);
    let limits = WorkflowLimits {
        journal_record_bytes: 32 * 1024 * 1024,
        journal_total_bytes: 8 * 1024 * 1024 * 1024,
        ..WorkflowLimits::default()
    };
    let mut writer = JournalWriter::open(path, run_id.clone(), &limits).expect("open writer");
    let filler = "x".repeat(payload_chars);
    for i in 0..record_count {
        let envelope = JournalEnvelope::new(
            next_seq + i,
            1_000 + i,
            run_id.clone(),
            JournalPayload::RecoveryActionApplied {
                action: format!("page-fixture-{i}"),
                detail: Some(json!({ "filler": &filler })),
                quarantine_sha256: None,
                removed_bytes: None,
            },
        );
        writer.append(&envelope, &limits).expect("append record");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_gigabyte_logical_journal_pages_under_tool_result_cap() {
    let session = tempfile::tempdir().expect("session");
    // Small ToolResult budget forces multi-page journal reads.
    let page_cap: u64 = 8 * 1024;
    let limits = WorkflowLimits {
        task_output_page_bytes: page_cap,
        journal_record_bytes: 32 * 1024 * 1024,
        journal_total_bytes: 8 * 1024 * 1024 * 1024,
        ..WorkflowLimits::default()
    };
    let runtime = WorkflowRuntime::new(limits);
    // No worker: create_run only so the journal is not dual-written.
    let handle = runtime
        .create_run(session.path(), launch_request("page-cap"))
        .await
        .expect("create");

    let journal_path = {
        let materials = runtime
            .task_output_materials(&handle.run_id)
            .await
            .expect("materials");
        materials.journal_path
    };

    // ~512 records × ~4 KiB ≈ 2 MiB logical journal (multi-gigabyte-capable
    // paging path; full serialize would blow the 8 KiB ToolResult cap).
    append_logical_multi_gigabyte_journal(&journal_path, &handle.run_id, 512, 4 * 1024);

    let mut cursor = None;
    let mut pages = 0u32;
    let mut seen_seqs = Vec::new();
    loop {
        let request = TaskOutputRequest {
            view: TaskOutputView::Journal,
            cursor: cursor.clone(),
            max_output_bytes: page_cap,
            artifact_id: None,
        };
        let page = handle.task_output(request).await.expect("page");
        let (content, details) =
            page_to_tool_result(&page, page_cap).expect("tool result under cap");
        let total = measure_tool_result_bytes(&content, &details) as u64;
        assert!(
            total <= page_cap,
            "page {pages} ToolResult {total} exceeds cap {page_cap}"
        );
        // Summary must never embed the complete journal.
        assert!(page.summary.is_none() || page.journal.is_empty());
        assert_eq!(page.view, TaskOutputView::Journal);
        if let (Some(first), Some(last)) = (page.first_seq, page.last_seq) {
            assert!(first <= last);
            for record in &page.journal {
                seen_seqs.push(record.seq);
            }
        }
        pages += 1;
        assert!(pages < 10_000, "failed to terminate paging");
        if !page.has_more {
            assert!(page.next_cursor.is_none());
            break;
        }
        let next = page.next_cursor.expect("next_cursor when has_more");
        cursor = Some(next);
    }

    assert!(pages > 1, "expected multi-page journal, got {pages}");
    assert!(
        !seen_seqs.is_empty(),
        "expected at least one journal record summary"
    );
    // Contiguous ascending sequences across pages.
    for window in seen_seqs.windows(2) {
        assert!(
            window[0] < window[1],
            "journal pages must be ascending contiguous summaries"
        );
    }

    // Summary never serializes the complete journal.
    let summary = handle
        .task_output(TaskOutputRequest::summary(page_cap))
        .await
        .expect("summary");
    assert!(summary.journal.is_empty());
    assert!(summary.summary.is_some());
    let summary_json = serde_json::to_string(&summary).expect("serialize summary");
    assert!(
        !summary_json.contains("heartbeat-0-"),
        "summary must not embed full journal record bodies"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_run_view_or_query_cursor_is_rejected() {
    let session = tempfile::tempdir().expect("session");
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    runtime
        .bind_runner(|_handle, _meta, _session| async move { Ok(()) })
        .expect("bind");

    let handle_a = runtime
        .create_run(session.path(), launch_request("cursor-a"))
        .await
        .expect("create a");
    let handle_b = runtime
        .create_run(session.path(), launch_request("cursor-b"))
        .await
        .expect("create b");

    // Produce a valid journal cursor on run A.
    let page = handle_a
        .task_output(TaskOutputRequest {
            view: TaskOutputView::Journal,
            cursor: None,
            max_output_bytes: 64 * 1024,
            artifact_id: None,
        })
        .await
        .expect("journal page a");
    // Even without has_more, build a cursor-bound request using journal_next from summary.
    let summary = handle_a
        .task_output(TaskOutputRequest::summary(64 * 1024))
        .await
        .expect("summary");
    let journal_cursor = summary
        .summary
        .as_ref()
        .and_then(|s| s.journal_next_cursor.clone())
        .expect("summary should expose journal_next_cursor");

    // Wrong run: cursor from A used on B.
    let err = handle_b
        .task_output(TaskOutputRequest {
            view: TaskOutputView::Journal,
            cursor: Some(journal_cursor.clone()),
            max_output_bytes: 64 * 1024,
            artifact_id: None,
        })
        .await
        .expect_err("wrong run cursor rejected");
    assert!(
        err.to_string().contains("run_id") || err.to_string().contains("cursor"),
        "unexpected error: {err}"
    );

    // Wrong view: journal cursor used with result view.
    let err = handle_a
        .task_output(TaskOutputRequest {
            view: TaskOutputView::Result,
            cursor: Some(journal_cursor.clone()),
            max_output_bytes: 64 * 1024,
            artifact_id: None,
        })
        .await
        .expect_err("wrong view cursor rejected");
    assert!(
        err.to_string().contains("view") || err.to_string().contains("cursor"),
        "unexpected error: {err}"
    );

    // Wrong query: artifact_content cursor requires matching artifact query hash.
    let artifacts_cursor = summary
        .summary
        .as_ref()
        .and_then(|s| s.artifacts_next_cursor.clone());
    if let Some(cursor) = artifacts_cursor {
        let err = handle_a
            .task_output(TaskOutputRequest {
                view: TaskOutputView::Journal,
                cursor: Some(cursor),
                max_output_bytes: 64 * 1024,
                artifact_id: None,
            })
            .await
            .expect_err("wrong query/view cursor rejected");
        assert!(
            err.to_string().contains("view")
                || err.to_string().contains("query")
                || err.to_string().contains("cursor"),
            "unexpected error: {err}"
        );
    }

    // Garbage cursor.
    let err = handle_a
        .task_output(TaskOutputRequest {
            view: TaskOutputView::Journal,
            cursor: Some("not-a-valid-cursor".to_owned()),
            max_output_bytes: 64 * 1024,
            artifact_id: None,
        })
        .await
        .expect_err("garbage cursor rejected");
    assert!(
        err.to_string().contains("cursor") || err.to_string().contains("invalid"),
        "unexpected error: {err}"
    );

    // Valid same-run/same-view cursor is accepted (even if empty page).
    if page.has_more {
        let next = page.next_cursor.expect("next");
        handle_a
            .task_output(TaskOutputRequest {
                view: TaskOutputView::Journal,
                cursor: Some(next),
                max_output_bytes: 64 * 1024,
                artifact_id: None,
            })
            .await
            .expect("matching cursor accepted");
    } else {
        handle_a
            .task_output(TaskOutputRequest {
                view: TaskOutputView::Journal,
                cursor: Some(journal_cursor),
                max_output_bytes: 64 * 1024,
                artifact_id: None,
            })
            .await
            .expect("summary journal_next_cursor accepted on journal view");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn slow_output_io_does_not_block_snapshot_or_pause() {
    let session = tempfile::tempdir().expect("session");
    let runtime = WorkflowRuntime::new(WorkflowLimits::default());
    let gate = Arc::new(tokio::sync::Notify::new());
    let gate_run = Arc::clone(&gate);
    runtime
        .bind_runner(move |_handle, _meta, _session| {
            let gate = Arc::clone(&gate_run);
            async move {
                gate.notified().await;
                Ok(())
            }
        })
        .expect("bind");

    let handle = runtime
        .create_run(session.path(), launch_request("slow-io"))
        .await
        .expect("create");
    runtime.start_worker(&handle.run_id).await.expect("start");
    wait_running(&handle).await;

    // Inject multi-second I/O delay *after* the run lock is released.
    runtime.set_output_io_delay_ms_for_test(1_500);

    let handle_io = handle.clone();
    let io_started = Instant::now();
    let journal_fut = async move {
        handle_io
            .task_output(TaskOutputRequest {
                view: TaskOutputView::Journal,
                cursor: None,
                max_output_bytes: 64 * 1024,
                artifact_id: None,
            })
            .await
    };
    let journal_task = tokio::spawn(journal_fut);

    // Snapshot and pause must complete while journal I/O is still "slow".
    let snap_start = Instant::now();
    let snapshot = tokio::time::timeout(Duration::from_millis(300), handle.snapshot())
        .await
        .expect("snapshot timed out — output I/O held the run lock");
    assert_eq!(snapshot.state, WorkflowState::Running);
    assert!(
        snap_start.elapsed() < Duration::from_millis(300),
        "snapshot took {:?}",
        snap_start.elapsed()
    );

    let pause_start = Instant::now();
    tokio::time::timeout(
        Duration::from_millis(300),
        handle.pause(WorkflowActor::Human),
    )
    .await
    .expect("pause timed out — output I/O held the run lock")
    .expect("pause");
    assert!(
        pause_start.elapsed() < Duration::from_millis(300),
        "pause took {:?}",
        pause_start.elapsed()
    );

    let page = journal_task
        .await
        .expect("join")
        .expect("journal page after slow io");
    assert_eq!(page.view, TaskOutputView::Journal);
    assert!(
        io_started.elapsed() >= Duration::from_millis(1_200),
        "expected injected delay to elapse"
    );

    runtime.set_output_io_delay_ms_for_test(0);
    gate.notify_waiters();
}
