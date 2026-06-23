use neo_agent_core::session::{
    SessionMetadataStore, SessionRecord, SessionSummaryRecord, SessionSummarySource,
};

const SESSION_A: &str = "session_00000000-0000-4000-8000-000000000101";
const SESSION_B: &str = "session_00000000-0000-4000-8000-000000000102";
const SESSION_C: &str = "session_00000000-0000-4000-8000-000000000103";

fn write_session_transcript(dir: &std::path::Path, session_id: &str) {
    let session_dir = dir.join(session_id);
    std::fs::create_dir_all(&session_dir).expect("create session dir");
    std::fs::write(session_dir.join("transcript.jsonl"), "{}\n").expect("write transcript");
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

    assert_eq!(
        sessions,
        vec![
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
            SessionRecord {
                id: child.id,
                name: Some("Investigate parser".to_owned()),
                summary: None,
                parent_id: Some(SESSION_A.to_owned()),
                summary_record: None,
                title: Some("Investigate parser".to_owned()),
                title_model: None,
                title_updated_at: None,
                workspace: None,
                last_user_prompt: None,
                updated_at: None,
                children: Vec::new(),
            },
        ]
    );
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
