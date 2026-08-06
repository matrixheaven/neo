use neo_agent_core::session::{
    SessionMetadataStore, SessionRecord, SessionSummaryRecord, SessionSummarySource,
};

const SESSION_A: &str = "session_00000000-0000-4000-8000-000000000101";
const SESSION_B: &str = "session_00000000-0000-4000-8000-000000000102";
const SESSION_C: &str = "session_00000000-0000-4000-8000-000000000103";

fn write_session_transcript(dir: &std::path::Path, session_id: &str) {
    let session_dir = dir.join(session_id);
    let wire_path = neo_agent_core::session::main_agent_wire_path(&session_dir);
    std::fs::create_dir_all(wire_path.parent().expect("wire parent")).expect("create session dir");
    std::fs::write(wire_path, "{}\n").expect("write transcript");
}

#[test]
fn session_metadata_lists_sessions_with_main_agent_wire() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path();
    let session_id = "session_00000000-0000-0000-0000-000000000001";
    let dir = sessions_dir.join(session_id);
    let wire_path = neo_agent_core::session::main_agent_wire_path(&dir);
    std::fs::create_dir_all(wire_path.parent().expect("wire parent")).expect("mkdir");
    std::fs::write(
        wire_path,
        "{\"kind\":\"neo.session.metadata\",\"format\":\"neo.session.jsonl\",\"schema_version\":1,\"created_at\":\"0\"}\n",
    )
    .expect("write wire");

    let store = neo_agent_core::session::SessionMetadataStore::new(sessions_dir);
    let sessions = store.list().expect("list");

    assert!(sessions.iter().any(|session| session.id == session_id));
}

#[test]
fn session_metadata_lists_existing_jsonl_sessions_with_names_and_children() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_session_transcript(dir.path(), SESSION_A);

    let store = SessionMetadataStore::new(dir.path());
    let child = store
        .fork(SESSION_A, Some("Investigate parser".to_owned()))
        .expect("fork session");
    store
        .rename(SESSION_A, "Main thread".to_owned())
        .expect("rename session");
    store
        .summarize(SESSION_A, "Investigating parser branch".to_owned())
        .expect("summarize session");

    let sessions = store.list().expect("list sessions");
    assert!(child.id.starts_with("session_"));

    let expected = vec![
        SessionRecord {
            id: SESSION_A.to_owned(),
            name: Some("Main thread".to_owned()),
            summary: Some("Investigating parser branch".to_owned()),
            parent_id: None,
            summary_record: Some(SessionSummaryRecord {
                text: "Investigating parser branch".to_owned(),
                source: SessionSummarySource::LocalExtractive,
                model: None,
                updated_at: None,
            }),
            title: Some("Main thread".to_owned()),
            title_model: None,
            title_updated_at: None,
            workspace: None,
            last_user_prompt: None,
            updated_at: None,
            children: vec![child.id.clone()],
        },
        child,
    ];
    assert_eq!(sessions, expected);
}

#[test]
fn session_metadata_stores_branch_summary_records() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_session_transcript(dir.path(), SESSION_A);

    let store = SessionMetadataStore::new(dir.path());
    let summarized = store
        .record_summary(
            SESSION_A,
            SessionSummaryRecord {
                text: "Investigating parser branch".to_owned(),
                source: SessionSummarySource::ModelGenerated,
                model: Some("openai/gpt-4.1".to_owned()),
                updated_at: Some("125.0Z".to_owned()),
            },
        )
        .expect("summarize session");

    assert_eq!(
        summarized.summary.as_deref(),
        Some("Investigating parser branch")
    );
    assert_eq!(
        summarized.summary_record,
        Some(SessionSummaryRecord {
            text: "Investigating parser branch".to_owned(),
            source: SessionSummarySource::ModelGenerated,
            model: Some("openai/gpt-4.1".to_owned()),
            updated_at: Some("125.0Z".to_owned()),
        })
    );
    assert_eq!(
        store.list().expect("list sessions")[0]
            .summary_record
            .as_ref()
            .map(|summary| summary.source),
        Some(SessionSummarySource::ModelGenerated)
    );
}

#[test]
fn session_metadata_records_activity_title_and_orders_recent_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_session_transcript(dir.path(), SESSION_A);
    write_session_transcript(dir.path(), SESSION_B);

    let store = SessionMetadataStore::new(dir.path());
    store
        .record_activity(
            SESSION_A,
            Some("/workspace/old".to_owned()),
            Some("older prompt".to_owned()),
            "100".to_owned(),
        )
        .expect("record session a activity");
    store
        .record_activity(
            SESSION_B,
            Some("/workspace/new".to_owned()),
            Some("newer prompt".to_owned()),
            "200".to_owned(),
        )
        .expect("record session b activity");
    store
        .record_title(
            SESSION_B,
            "Generated resume title".to_owned(),
            Some("test/model".to_owned()),
            "201".to_owned(),
        )
        .expect("record session b title");

    let sessions = store.list_recent().expect("list recent sessions");

    assert_eq!(sessions[0].id, SESSION_B);
    assert_eq!(sessions[0].title.as_deref(), Some("Generated resume title"));
    assert_eq!(sessions[0].title_model.as_deref(), Some("test/model"));
    assert_eq!(sessions[0].title_updated_at.as_deref(), Some("201"));
    assert_eq!(sessions[0].workspace.as_deref(), Some("/workspace/new"));
    assert_eq!(
        sessions[0].last_user_prompt.as_deref(),
        Some("newer prompt")
    );
    assert_eq!(sessions[0].updated_at.as_deref(), Some("200"));
    assert_eq!(sessions[1].id, SESSION_A);
}

#[test]
fn session_metadata_title_falls_back_to_name_then_last_prompt_then_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_session_transcript(dir.path(), SESSION_A);
    write_session_transcript(dir.path(), SESSION_B);
    write_session_transcript(dir.path(), SESSION_C);

    let store = SessionMetadataStore::new(dir.path());
    store
        .rename(SESSION_A, "Named session".to_owned())
        .expect("rename session a");
    store
        .record_activity(
            SESSION_B,
            None,
            Some("Prompt title".to_owned()),
            "100".to_owned(),
        )
        .expect("record session b activity");

    let sessions = store.list().expect("list sessions");

    assert_eq!(sessions[0].title.as_deref(), Some("Named session"));
    assert_eq!(sessions[1].title.as_deref(), Some("Prompt title"));
    assert_eq!(sessions[2].title.as_deref(), Some(SESSION_C));
}

#[test]
fn session_metadata_ignores_legacy_numeric_session_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("1781719048514.jsonl"), "{}\n").expect("write legacy session");
    write_session_transcript(dir.path(), SESSION_A);

    let store = SessionMetadataStore::new(dir.path());
    let sessions = store.list().expect("list sessions");

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, SESSION_A);
    assert!(store.rename("1781719048514", "Legacy".to_owned()).is_err());
    assert!(
        store
            .record_activity("1781719048514", None, None, "100".to_owned())
            .is_err()
    );
    assert!(store.fork("1781719048514", None).is_err());
}

#[test]
fn failed_session_fork_cleans_up_child_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_session_transcript(dir.path(), SESSION_A);
    std::fs::write(dir.path().join("sessions.metadata.json"), "{").expect("write invalid metadata");

    let store = SessionMetadataStore::new(dir.path());
    store
        .fork(SESSION_A, Some("Broken fork".to_owned()))
        .expect_err("metadata failure should fail fork");

    let session_dirs = std::fs::read_dir(dir.path())
        .expect("read sessions dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("session_"))
        .count();
    assert_eq!(
        session_dirs, 1,
        "failed fork must not leave a child session"
    );
}

#[cfg(unix)]
#[test]
fn session_metadata_write_rejects_symlink_instead_of_following_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    write_session_transcript(dir.path(), SESSION_A);

    let outside_metadata = outside.path().join("sessions.metadata.json");
    std::fs::write(&outside_metadata, "{\"sessions\":{}}\n").expect("write outside metadata");
    std::os::unix::fs::symlink(&outside_metadata, dir.path().join("sessions.metadata.json"))
        .expect("symlink metadata");

    let store = SessionMetadataStore::new(dir.path());
    let error = store
        .rename(SESSION_A, "Main thread".to_owned())
        .expect_err("metadata symlink should be rejected");

    assert!(
        error.to_string().contains("symlink"),
        "error should name symlink risk: {error}"
    );
    assert_eq!(
        std::fs::read_to_string(&outside_metadata).expect("read outside metadata"),
        "{\"sessions\":{}}\n"
    );
}

#[cfg(unix)]
#[test]
fn session_metadata_rejects_symlinked_sessions_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    write_session_transcript(outside.path(), SESSION_A);
    let sessions_link = dir.path().join("sessions");
    std::os::unix::fs::symlink(outside.path(), &sessions_link).expect("symlink sessions dir");

    let store = SessionMetadataStore::new(&sessions_link);
    let error = store
        .rename(SESSION_A, "Main thread".to_owned())
        .expect_err("symlinked sessions dir should be rejected");

    assert!(
        error.to_string().contains("symlink"),
        "error should name symlink risk: {error}"
    );
}

#[cfg(unix)]
#[test]
fn session_fork_rejects_symlinked_artifacts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    write_session_transcript(dir.path(), SESSION_A);
    let outside_blob = outside.path().join("secret.txt");
    std::fs::write(&outside_blob, "external secret").expect("write outside blob");

    let session_dir = dir.path().join(SESSION_A);
    let linked_blob = session_dir.join("linked-secret.txt");
    std::os::unix::fs::symlink(&outside_blob, &linked_blob).expect("symlink blob");

    let store = SessionMetadataStore::new(dir.path());
    let error = store
        .fork(SESSION_A, Some("Fork with link".to_owned()))
        .expect_err("fork should reject symlinked session artifacts");

    assert!(
        error.to_string().contains("symlink"),
        "error should name symlink risk: {error}"
    );
    let session_dirs = std::fs::read_dir(dir.path())
        .expect("read sessions dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("session_"))
        .count();
    assert_eq!(
        session_dirs, 1,
        "failed fork must not leave a child session"
    );
}

#[cfg(unix)]
#[test]
fn session_fork_rejects_symlinked_session_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    write_session_transcript(outside.path(), SESSION_A);
    std::os::unix::fs::symlink(outside.path().join(SESSION_A), dir.path().join(SESSION_A))
        .expect("symlink session root");

    let store = SessionMetadataStore::new(dir.path());
    let error = store
        .fork(SESSION_A, Some("Fork with linked root".to_owned()))
        .expect_err("fork should reject symlinked session root");

    assert!(
        error.to_string().contains("symlink"),
        "error should name symlink risk: {error}"
    );
}
