use neo_agent_core::workflow::journal::{
    JournalEnvelope, JournalPayload, JournalWriter, collect_journal, scan_journal,
    scan_journal_page,
};
use neo_agent_core::workflow::recovery::{
    JournalRecoveryAction, quarantine_tail_path, recover_journal, recovery_quarantine_dir,
};
use neo_agent_core::workflow::{
    WorkflowActor, WorkflowError, WorkflowErrorCode, WorkflowId, WorkflowInvocationKind,
    WorkflowLimits, WorkflowState,
};
use serde_json::json;

fn run_id() -> WorkflowId {
    WorkflowId::from_existing("wf_000000000000000000000000000000aa")
}

fn limits() -> WorkflowLimits {
    WorkflowLimits::default()
}
fn write_valid_prefix(path: &std::path::Path, id: &WorkflowId) -> String {
    let env = JournalEnvelope::new(
        0,
        1_000,
        id.clone(),
        JournalPayload::RunCreated {
            name: "demo".to_owned(),
            description: None,
            launch_source: Some("/workflow".to_owned()),
        },
    );
    let line = serde_json::to_string(&env).unwrap();
    std::fs::write(path, format!("{line}\n")).unwrap();
    line
}

#[test]
fn journal_recovery_normalizes_valid_unterminated_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let line = write_valid_prefix(&path, &id);

    // Second valid envelope without terminating newline (torn after JSON, before \n).
    let second = JournalEnvelope::new(
        1,
        1_001,
        id.clone(),
        JournalPayload::StateChanged {
            previous: WorkflowState::Queued,
            new: WorkflowState::Running,
            reason: "launch".to_owned(),
            actor: WorkflowActor::Runtime,
        },
    );
    let second_line = serde_json::to_string(&second).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(second_line.as_bytes());
    // Explicitly no trailing newline.
    assert!(!bytes.ends_with(b"\n") || bytes.ends_with(second_line.as_bytes()));
    // Ensure file does not end with newline after append of second_line alone.
    std::fs::write(&path, &bytes).unwrap();
    let on_disk = std::fs::read(&path).unwrap();
    assert!(
        !on_disk.ends_with(b"\n"),
        "fixture must end without newline"
    );
    assert!(on_disk.starts_with(format!("{line}\n").as_bytes()));

    let report = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("normalize recovery");
    assert!(matches!(
        report.action,
        JournalRecoveryAction::NormalizedUnterminated { seq: 1 }
    ));
    let recovered = std::fs::read(&path).unwrap();
    assert!(
        recovered.ends_with(b"\n"),
        "normalized journal must end with newline"
    );
    assert_eq!(
        &recovered[..recovered.len() - 1],
        on_disk.as_slice(),
        "normalize must only append a newline"
    );

    let envelopes = collect_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .unwrap();
    assert_eq!(envelopes.len(), 2);
    assert_eq!(envelopes[1].seq, 1);
    assert_eq!(report.index.next_seq, 2);
}

#[test]
fn journal_recovery_enforces_record_limit_and_inferred_run_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let first = write_valid_prefix(&path, &id);
    let encoded_bytes = u64::try_from(first.len() + 1).unwrap();

    let report = recover_journal(&path, None, encoded_bytes, limits().journal_total_bytes)
        .expect("exact limit");
    assert_eq!(report.index.run_id.as_ref(), Some(&id));

    let before = std::fs::read(&path).unwrap();
    let scan_error = scan_journal(&path, None, encoded_bytes - 1, limits().journal_total_bytes)
        .expect_err("canonical scanner must enforce the same record limit");
    assert_eq!(scan_error.code(), WorkflowErrorCode::JournalCorrupt);
    let err = recover_journal(&path, None, encoded_bytes - 1, limits().journal_total_bytes)
        .expect_err("record over configured limit");
    assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let other = WorkflowId::from_existing("wf_000000000000000000000000000000bb");
    let second = JournalEnvelope::new(
        1,
        1_001,
        other,
        JournalPayload::StateChanged {
            previous: WorkflowState::Queued,
            new: WorkflowState::Running,
            reason: "launch".to_owned(),
            actor: WorkflowActor::Runtime,
        },
    );
    let second_line = serde_json::to_string(&second).unwrap();
    let mut mixed = before;
    mixed.extend_from_slice(second_line.as_bytes());
    std::fs::write(&path, &mixed).unwrap();

    let err = recover_journal(&path, None, 16 * 1024, limits().journal_total_bytes)
        .expect_err("inferred run id must remain stable");
    assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
    assert!(err.to_string().contains("run id mismatch"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), mixed);
}

#[test]
fn journal_scan_rejects_total_bytes_above_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let first = write_valid_prefix(&path, &id);
    let encoded_bytes = u64::try_from(first.len() + 1).unwrap();

    let error = scan_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        encoded_bytes - 1,
    )
    .expect_err("scanner must stop at the configured total size");
    assert_eq!(error.code(), WorkflowErrorCode::JournalCorrupt);

    let recovery = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        encoded_bytes - 1,
    )
    .expect_err("recovery must enforce the same total size");
    assert_eq!(recovery.code(), WorkflowErrorCode::JournalCorrupt);

    let page = scan_journal_page(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        encoded_bytes - 1,
        0,
        1,
        encoded_bytes,
    )
    .expect_err("paging must enforce the same total size");
    assert_eq!(page.code(), WorkflowErrorCode::JournalCorrupt);
}

#[test]
fn journal_recovery_quarantines_torn_tail_before_truncate() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path();
    let path = run_dir.join("journal.jsonl");
    let id = run_id();
    let _ = write_valid_prefix(&path, &id);
    let prefix = std::fs::read(&path).unwrap();
    let prefix_len = prefix.len() as u64;

    let torn = b"{\"seq\":1,\"timestamp_ms\":1001,\"run_id\":\"";
    let mut bytes = prefix.clone();
    bytes.extend_from_slice(torn);
    std::fs::write(&path, &bytes).unwrap();
    let original = std::fs::read(&path).unwrap();

    // Quarantine must succeed before truncate.
    let report = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("quarantine recovery");
    let JournalRecoveryAction::TornTailQuarantined {
        quarantine_sha256,
        quarantine_path,
        removed_bytes,
        last_validated_offset,
    } = report.action
    else {
        panic!("expected torn-tail quarantine, got {:?}", report.action);
    };

    assert_eq!(last_validated_offset, prefix_len);
    assert_eq!(removed_bytes, torn.len() as u64);
    assert!(quarantine_path.is_file(), "quarantine file must exist");
    assert_eq!(
        quarantine_path,
        quarantine_tail_path(run_dir, &quarantine_sha256)
    );
    let quarantined = std::fs::read(&quarantine_path).unwrap();
    assert_eq!(quarantined, torn);

    let after = std::fs::read(&path).unwrap();
    // Prefix preserved; recovery record may be appended after truncate.
    assert!(
        after.starts_with(&prefix),
        "valid prefix must survive truncation"
    );
    assert!(
        !after.windows(torn.len()).any(|w| w == torn),
        "torn suffix must not remain in journal"
    );
    assert!(report.recovery_record_appended);

    // Quarantine failure must leave original bytes unchanged.
    let path2 = run_dir.join("journal_fail.jsonl");
    std::fs::write(&path2, &original).unwrap();
    // Make recovery-quarantine a file so directory create fails for this journal's sibling path.
    // recover uses path.parent()/recovery-quarantine — poison that path after first success
    // by replacing the directory with a file for a second journal under a nested run dir.
    let run2 = run_dir.join("run2");
    std::fs::create_dir_all(&run2).unwrap();
    let path3 = run2.join("journal.jsonl");
    std::fs::write(&path3, &original).unwrap();
    // Create a file where the quarantine directory should be.
    let qdir = recovery_quarantine_dir(&run2);
    std::fs::write(&qdir, b"not-a-directory").unwrap();
    let before_fail = std::fs::read(&path3).unwrap();
    let err = recover_journal(
        &path3,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect_err("quarantine must fail");
    assert!(
        err.to_string().contains("quarantine") || err.to_string().contains("directory"),
        "unexpected error: {err}"
    );
    let after_fail = std::fs::read(&path3).unwrap();
    assert_eq!(
        before_fail, after_fail,
        "quarantine failure must leave journal byte-for-byte unchanged"
    );
    let _ = path2;
}

#[test]
fn journal_recovery_of_torn_first_record_leaves_reopenable_empty_journal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let torn = br#"{"seq":0,"timestamp_ms":1000"#;
    std::fs::write(&path, torn).unwrap();

    let report = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("quarantine torn first record");
    assert!(!report.recovery_record_appended);
    assert!(std::fs::read(&path).unwrap().is_empty());
    let JournalRecoveryAction::TornTailQuarantined {
        quarantine_path, ..
    } = report.action
    else {
        panic!("expected torn-tail quarantine");
    };
    assert_eq!(std::fs::read(quarantine_path).unwrap(), torn);

    let retry = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("empty canonical journal must reopen");
    assert_eq!(retry.action, JournalRecoveryAction::None);
    let writer = JournalWriter::open(&path, id, &limits()).expect("empty journal must reopen");
    assert_eq!(writer.next_seq(), 0);
}

#[test]
fn journal_recovery_propagates_recovery_record_failure_without_truncating() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    let id = run_id();
    let _ = write_valid_prefix(&path, &id);
    let prefix = std::fs::read(&path).unwrap();
    let torn = b"{\"seq\":";
    let mut bytes = prefix.clone();
    bytes.extend_from_slice(torn);
    std::fs::write(&path, &bytes).unwrap();
    let original = std::fs::read(&path).unwrap();

    let limit = u64::try_from(prefix.len()).unwrap();
    let error = recover_journal(&path, Some(&id), limit, limits().journal_total_bytes)
        .expect_err("recovery fact over the record limit must fail closed");
    assert!(matches!(
        error,
        WorkflowError::JournalRecordLimitExceeded { limit: actual, .. } if actual == limit
    ));

    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "recovery-record failure must leave the original journal byte-for-byte unchanged"
    );
    let quarantined = std::fs::read_dir(recovery_quarantine_dir(dir.path()))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(quarantined.len(), 1);
    assert_eq!(std::fs::read(&quarantined[0]).unwrap(), torn);

    let report = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("retry must atomically commit prefix and recovery fact");
    assert!(report.recovery_record_appended);
    let recovered = std::fs::read(&path).unwrap();
    assert!(recovered.starts_with(&prefix));
    assert!(
        !recovered.ends_with(torn),
        "the original torn EOF suffix must not survive recovery"
    );

    let records = collect_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|record| {
                matches!(
                    &record.payload,
                    JournalPayload::RecoveryActionApplied { .. }
                )
            })
            .count(),
        1,
        "retry must persist exactly one recovery fact"
    );
    let writer = JournalWriter::open(&path, id, &limits()).expect("restart-safe journal open");
    assert_eq!(writer.index().record_count, records.len() as u64);
}

#[test]
fn journal_recovery_fails_closed_on_interior_or_newline_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let id = run_id();

    // Newline-terminated invalid JSON at EOF — fail closed, no mutation.
    {
        let path = dir.path().join("newline_bad.jsonl");
        let prefix_line = write_valid_prefix(&path, &id);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(b"{not-json}\n");
        std::fs::write(&path, &bytes).unwrap();
        let before = std::fs::read(&path).unwrap();
        let err = recover_journal(
            &path,
            Some(&id),
            limits().journal_record_bytes,
            limits().journal_total_bytes,
        )
        .expect_err("must fail closed");
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = prefix_line;
    }

    // Interior malformed (bad complete line between good lines).
    {
        let path = dir.path().join("interior_bad.jsonl");
        let env0 = JournalEnvelope::new(
            0,
            1_000,
            id.clone(),
            JournalPayload::RunCreated {
                name: "demo".to_owned(),
                description: None,
                launch_source: None,
            },
        );
        let second = JournalEnvelope::new(
            1,
            1_002,
            id.clone(),
            JournalPayload::StateChanged {
                previous: WorkflowState::Queued,
                new: WorkflowState::Running,
                reason: "x".to_owned(),
                actor: WorkflowActor::Runtime,
            },
        );
        let content = format!(
            "{}\n{{bad}}\n{}\n",
            serde_json::to_string(&env0).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
        std::fs::write(&path, &content).unwrap();
        let before = std::fs::read(&path).unwrap();
        let err = recover_journal(
            &path,
            Some(&id),
            limits().journal_record_bytes,
            limits().journal_total_bytes,
        )
        .expect_err("interior must fail closed");
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    // Sequence mismatch on complete lines — fail closed.
    {
        let path = dir.path().join("seq_gap.jsonl");
        let env0 = JournalEnvelope::new(
            0,
            1_000,
            id.clone(),
            JournalPayload::RunCreated {
                name: "demo".to_owned(),
                description: None,
                launch_source: None,
            },
        );
        let gap = JournalEnvelope::new(
            2,
            1_002,
            id.clone(),
            JournalPayload::StateChanged {
                previous: WorkflowState::Queued,
                new: WorkflowState::Running,
                reason: "gap".to_owned(),
                actor: WorkflowActor::Runtime,
            },
        );
        let content = format!(
            "{}\n{}\n",
            serde_json::to_string(&env0).unwrap(),
            serde_json::to_string(&gap).unwrap()
        );
        std::fs::write(&path, &content).unwrap();
        let before = std::fs::read(&path).unwrap();
        let err = recover_journal(
            &path,
            Some(&id),
            limits().journal_record_bytes,
            limits().journal_total_bytes,
        )
        .expect_err("seq gap must fail closed");
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert!(err.to_string().contains("sequence gap"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    // Run-id mismatch.
    {
        let path = dir.path().join("run_mismatch.jsonl");
        let other = WorkflowId::from_existing("wf_000000000000000000000000000000bb");
        let env = JournalEnvelope::new(
            0,
            1_000,
            other,
            JournalPayload::RunCreated {
                name: "demo".to_owned(),
                description: None,
                launch_source: None,
            },
        );
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&env).unwrap())).unwrap();
        let before = std::fs::read(&path).unwrap();
        let err = recover_journal(
            &path,
            Some(&id),
            limits().journal_record_bytes,
            limits().journal_total_bytes,
        )
        .expect_err("run id must fail closed");
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert!(err.to_string().contains("run id mismatch"), "{err}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    // Hash mismatch on complete line.
    {
        let path = dir.path().join("hash_bad.jsonl");
        let mut env = JournalEnvelope::new(
            0,
            1_000,
            id.clone(),
            JournalPayload::InvocationStarted {
                invocation_id: "inv_1".to_owned(),
                call_index: 0,
                kind: WorkflowInvocationKind::Delegate,
                canonical_input: Some(json!({"task": "x"})),
            },
        )
        .with_canonical_input_hash("0".repeat(64));
        std::fs::write(&path, format!("{}\n", serde_json::to_string(&env).unwrap())).unwrap();
        let before = std::fs::read(&path).unwrap();
        let err = recover_journal(
            &path,
            Some(&id),
            limits().journal_record_bytes,
            limits().journal_total_bytes,
        )
        .expect_err("hash must fail closed");
        assert_eq!(err.code(), WorkflowErrorCode::JournalCorrupt);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        env.canonical_input_hash = Some("1".repeat(64));
        let _ = env;
    }
}

/// Platform journal contract: PathBuf quarantine layout, append durability via
/// sync_all, torn-tail quarantine before truncate, quarantine failure leaves
/// the journal byte-for-byte intact.
///
/// Native evidence target for Task 25 (macOS / Linux / Windows).
#[test]
fn journal_platform_sync_and_quarantine_semantics() {
    use std::path::PathBuf;

    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("workflows").join("platform-run");
    std::fs::create_dir_all(&run_dir).unwrap();
    let path = run_dir.join("journal.jsonl");
    let id = run_id();

    // Open creates parent safely and returns a writer bound to Path (not strings).
    let mut writer = JournalWriter::open(&path, id.clone(), &limits()).expect("open empty journal");
    assert!(path.is_file());
    let created = JournalEnvelope::new(
        0,
        1_000,
        id.clone(),
        JournalPayload::RunCreated {
            name: "platform-sync".to_owned(),
            description: Some("sync proof".to_owned()),
            launch_source: Some("/workflow".to_owned()),
        },
    );
    writer.append(&created, &limits()).expect("append");
    drop(writer);

    // After append+sync_all, a fresh open must see the durable record.
    let reread = collect_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("reread after sync");
    assert_eq!(reread.len(), 1);
    assert_eq!(reread[0].seq, 0);

    // Quarantine paths are PathBuf joins under the run directory.
    let qdir = recovery_quarantine_dir(&run_dir);
    assert_eq!(qdir, PathBuf::from(&run_dir).join("recovery-quarantine"));
    let sample_sha = "a".repeat(64);
    let qpath = quarantine_tail_path(&run_dir, &sample_sha);
    assert_eq!(
        qpath,
        PathBuf::from(&run_dir)
            .join("recovery-quarantine")
            .join(format!("{sample_sha}.tail"))
    );

    // Torn tail: quarantine content-addressed suffix, truncate, keep valid prefix.
    let prefix = std::fs::read(&path).unwrap();
    let prefix_len = prefix.len() as u64;
    let torn = br#"{"seq":1,"timestamp_ms":1001,"run_id":"torn"#;
    let mut bytes = prefix.clone();
    bytes.extend_from_slice(torn);
    std::fs::write(&path, &bytes).unwrap();
    let original_with_torn = std::fs::read(&path).unwrap();

    let report = recover_journal(
        &path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect("torn-tail recovery");
    let JournalRecoveryAction::TornTailQuarantined {
        quarantine_sha256,
        quarantine_path,
        removed_bytes,
        last_validated_offset,
    } = report.action
    else {
        panic!("expected TornTailQuarantined, got {:?}", report.action);
    };
    assert_eq!(last_validated_offset, prefix_len);
    assert_eq!(removed_bytes, torn.len() as u64);
    assert_eq!(
        quarantine_path,
        quarantine_tail_path(&run_dir, &quarantine_sha256)
    );
    assert!(quarantine_path.is_file());
    assert_eq!(std::fs::read(&quarantine_path).unwrap(), torn);

    let after = std::fs::read(&path).unwrap();
    assert!(
        after.starts_with(&prefix),
        "valid prefix must survive on this platform"
    );
    assert!(
        !after.windows(torn.len()).any(|w| w == torn),
        "torn suffix must leave the journal"
    );
    assert!(report.recovery_record_appended);

    // Quarantine failure (parent path is a file) must leave journal untouched.
    let nested = run_dir.join("nested-fail");
    std::fs::create_dir_all(&nested).unwrap();
    let fail_path = nested.join("journal.jsonl");
    std::fs::write(&fail_path, &original_with_torn).unwrap();
    let q_poison = recovery_quarantine_dir(&nested);
    std::fs::write(&q_poison, b"not-a-directory").unwrap();
    let before = std::fs::read(&fail_path).unwrap();
    let err = recover_journal(
        &fail_path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect_err("quarantine must fail");
    assert!(
        err.to_string().to_lowercase().contains("quarantine")
            || err.to_string().to_lowercase().contains("directory")
            || err.to_string().to_lowercase().contains("not a directory")
            || err.to_string().contains("File exists")
            || err.to_string().contains("AlreadyExists"),
        "unexpected quarantine failure wording: {err}"
    );
    assert_eq!(
        std::fs::read(&fail_path).unwrap(),
        before,
        "quarantine failure must leave journal byte-for-byte intact"
    );

    // Interior newline-terminated corruption still fails closed with no mutation.
    let corrupt_path = run_dir.join("interior.jsonl");
    let mut corrupt = prefix.clone();
    corrupt.extend_from_slice(b"{not-json}\n");
    std::fs::write(&corrupt_path, &corrupt).unwrap();
    let before_corrupt = std::fs::read(&corrupt_path).unwrap();
    let cerr = recover_journal(
        &corrupt_path,
        Some(&id),
        limits().journal_record_bytes,
        limits().journal_total_bytes,
    )
    .expect_err("interior corrupt");
    assert_eq!(cerr.code(), WorkflowErrorCode::JournalCorrupt);
    assert_eq!(std::fs::read(&corrupt_path).unwrap(), before_corrupt);
}
