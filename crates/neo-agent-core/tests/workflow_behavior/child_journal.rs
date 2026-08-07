use neo_agent_core::AgentTokenUsage;
use neo_agent_core::workflow::journal::scan_journal;
use neo_agent_core::workflow::{
    JournalEnvelope, JournalPayload, JournalWriter, WorkflowChildKey, WorkflowChildKind,
    WorkflowChildState, WorkflowId, WorkflowLimits, WorkflowOutcomeStatus, project_children,
};

#[test]
fn journal_child_lifecycle_validates_and_projects_terminal_facts() {
    let dir = tempfile::tempdir().expect("journal directory");
    let path = dir.path().join("journal.jsonl");
    let run_id = WorkflowId("wf_test_child_journal".to_owned());
    let child_key = WorkflowChildKey::DirectDelegate {
        invocation_id: "inv1".to_owned(),
    };
    let limits = WorkflowLimits::default();
    let mut writer = JournalWriter::open(&path, run_id.clone(), &limits).expect("open journal");
    let created = JournalEnvelope::new(
        writer.next_seq(),
        500,
        run_id.clone(),
        JournalPayload::RunCreated {
            name: "child journal".to_owned(),
            description: None,
            launch_source: Some("test".to_owned()),
        },
    );
    writer
        .append(&created, &limits)
        .expect("append run created");

    for (timestamp_ms, payload) in [
        (
            1_000,
            JournalPayload::ChildQueued {
                child_key: child_key.clone(),
                child_kind: WorkflowChildKind::Delegate,
                invocation_id: "inv1".to_owned(),
                phase_id: Some("review".to_owned()),
                title: Some("Review changes".to_owned()),
                role: Some("reviewer".to_owned()),
            },
        ),
        (
            2_000,
            JournalPayload::ChildStarted {
                child_key: child_key.clone(),
                agent_id: Some("agent-a".to_owned()),
            },
        ),
        (
            3_000,
            JournalPayload::ChildFinished {
                child_key: child_key.clone(),
                agent_id: Some("agent-a".to_owned()),
                status: WorkflowOutcomeStatus::Completed,
                summary: "review complete".to_owned(),
                actual_usage: Some(AgentTokenUsage {
                    input_tokens: 11,
                    output_tokens: 7,
                    input_cache_read_tokens: 1,
                    input_cache_write_tokens: 2,
                }),
                error: None,
            },
        ),
    ] {
        let envelope =
            JournalEnvelope::new(writer.next_seq(), timestamp_ms, run_id.clone(), payload);
        writer.append(&envelope, &limits).expect("append lifecycle");
    }

    let index = scan_journal(
        &path,
        Some(&run_id),
        limits.journal_record_bytes,
        limits.journal_total_bytes,
    )
    .expect("scan journal");
    assert!(!index.has_incomplete_children());
    let projection = project_children(
        &path,
        Some(&run_id),
        limits.journal_record_bytes,
        limits.journal_total_bytes,
    )
    .expect("project children");
    assert!(projection.duplicate_keys.is_empty());
    assert_eq!(projection.rows.len(), 1);
    let row = &projection.rows[0];
    assert_eq!(row.key, child_key);
    assert_eq!(row.phase_id.as_deref(), Some("review"));
    assert_eq!(row.title.as_deref(), Some("Review changes"));
    assert_eq!(row.role.as_deref(), Some("reviewer"));
    assert_eq!(row.agent_id.as_deref(), Some("agent-a"));
    assert_eq!(row.state, WorkflowChildState::Completed);
    assert_eq!(row.terminal_at_ms, Some(3_000));
    assert_eq!(row.terminal_summary.as_deref(), Some("review complete"));
    assert_eq!(
        row.actual_usage
            .as_ref()
            .and_then(|usage| usage.get("input_tokens"))
            .and_then(serde_json::Value::as_u64),
        Some(11)
    );
}

#[test]
fn started_child_without_live_state_projects_as_recovering() {
    let dir = tempfile::tempdir().expect("journal directory");
    let path = dir.path().join("journal.jsonl");
    let run_id = WorkflowId("wf_test_recovering_child".to_owned());
    let child_key = WorkflowChildKey::DirectDelegate {
        invocation_id: "inv1".to_owned(),
    };
    let limits = WorkflowLimits::default();
    let mut writer = JournalWriter::open(&path, run_id.clone(), &limits).expect("open journal");
    let created = JournalEnvelope::new(
        writer.next_seq(),
        500,
        run_id.clone(),
        JournalPayload::RunCreated {
            name: "child journal".to_owned(),
            description: None,
            launch_source: Some("test".to_owned()),
        },
    );
    writer
        .append(&created, &limits)
        .expect("append run created");

    for (timestamp_ms, payload) in [
        (
            1_000,
            JournalPayload::ChildQueued {
                child_key: child_key.clone(),
                child_kind: WorkflowChildKind::Delegate,
                invocation_id: "inv1".to_owned(),
                phase_id: None,
                title: None,
                role: None,
            },
        ),
        (
            2_000,
            JournalPayload::ChildStarted {
                child_key,
                agent_id: Some("agent-a".to_owned()),
            },
        ),
    ] {
        let envelope =
            JournalEnvelope::new(writer.next_seq(), timestamp_ms, run_id.clone(), payload);
        writer.append(&envelope, &limits).expect("append lifecycle");
    }

    let projection = project_children(
        &path,
        Some(&run_id),
        limits.journal_record_bytes,
        limits.journal_total_bytes,
    )
    .expect("project children");
    assert_eq!(projection.rows.len(), 1);
    assert_eq!(projection.rows[0].state, WorkflowChildState::Recovering);
    assert_eq!(projection.rows[0].agent_id.as_deref(), Some("agent-a"));
}

#[test]
fn child_finish_rejects_started_agent_id_mismatch() {
    let dir = tempfile::tempdir().expect("journal directory");
    let path = dir.path().join("journal.jsonl");
    let run_id = WorkflowId("wf_test_child_agent_mismatch".to_owned());
    let child_key = WorkflowChildKey::DirectDelegate {
        invocation_id: "inv1".to_owned(),
    };
    let limits = WorkflowLimits::default();
    let mut writer = JournalWriter::open(&path, run_id.clone(), &limits).expect("open journal");

    for (timestamp_ms, payload) in [
        (
            500,
            JournalPayload::RunCreated {
                name: "child identity".to_owned(),
                description: None,
                launch_source: Some("test".to_owned()),
            },
        ),
        (
            1_000,
            JournalPayload::ChildQueued {
                child_key: child_key.clone(),
                child_kind: WorkflowChildKind::Delegate,
                invocation_id: "inv1".to_owned(),
                phase_id: None,
                title: None,
                role: None,
            },
        ),
        (
            2_000,
            JournalPayload::ChildStarted {
                child_key: child_key.clone(),
                agent_id: Some("agent-a".to_owned()),
            },
        ),
    ] {
        let envelope =
            JournalEnvelope::new(writer.next_seq(), timestamp_ms, run_id.clone(), payload);
        writer.append(&envelope, &limits).expect("append lifecycle");
    }

    let finish = JournalEnvelope::new(
        writer.next_seq(),
        3_000,
        run_id,
        JournalPayload::ChildFinished {
            child_key,
            agent_id: Some("agent-b".to_owned()),
            status: WorkflowOutcomeStatus::Failed,
            summary: "failed".to_owned(),
            actual_usage: None,
            error: Some("bind failed".to_owned()),
        },
    );
    let error = writer
        .append(&finish, &limits)
        .expect_err("mismatched child agent id must fail closed");

    assert_eq!(
        error.code(),
        neo_agent_core::workflow::WorkflowErrorCode::JournalCorrupt
    );
    assert!(error.to_string().contains("child agent id mismatch"));
}

#[test]
fn prestart_child_finish_can_introduce_agent_id() {
    let dir = tempfile::tempdir().expect("journal directory");
    let path = dir.path().join("journal.jsonl");
    let run_id = WorkflowId("wf_test_prestart_child_finish".to_owned());
    let child_key = WorkflowChildKey::DirectDelegate {
        invocation_id: "inv1".to_owned(),
    };
    let limits = WorkflowLimits::default();
    let mut writer = JournalWriter::open(&path, run_id.clone(), &limits).expect("open journal");

    for (timestamp_ms, payload) in [
        (
            500,
            JournalPayload::RunCreated {
                name: "prestart child failure".to_owned(),
                description: None,
                launch_source: Some("test".to_owned()),
            },
        ),
        (
            1_000,
            JournalPayload::ChildQueued {
                child_key: child_key.clone(),
                child_kind: WorkflowChildKind::Delegate,
                invocation_id: "inv1".to_owned(),
                phase_id: None,
                title: None,
                role: None,
            },
        ),
        (
            2_000,
            JournalPayload::ChildFinished {
                child_key: child_key.clone(),
                agent_id: Some("agent-prestart".to_owned()),
                status: WorkflowOutcomeStatus::Failed,
                summary: "failed before start".to_owned(),
                actual_usage: None,
                error: Some("bind failed".to_owned()),
            },
        ),
    ] {
        let envelope =
            JournalEnvelope::new(writer.next_seq(), timestamp_ms, run_id.clone(), payload);
        writer.append(&envelope, &limits).expect("append lifecycle");
    }

    let index = scan_journal(
        &path,
        Some(&run_id),
        limits.journal_record_bytes,
        limits.journal_total_bytes,
    )
    .expect("scan journal");
    assert_eq!(
        index.child_agent_ids.get(&child_key.display_key()),
        Some(&Some("agent-prestart".to_owned()))
    );
    let projection = project_children(
        &path,
        Some(&run_id),
        limits.journal_record_bytes,
        limits.journal_total_bytes,
    )
    .expect("project children");
    assert_eq!(projection.rows.len(), 1);
    assert_eq!(
        projection.rows[0].agent_id.as_deref(),
        Some("agent-prestart")
    );
    assert_eq!(projection.rows[0].state, WorkflowChildState::Failed);
}
