use neo_agent_core::session::{
    SessionMetadataStore, SessionRecord, SessionSummaryRecord, SessionSummarySource,
};

#[test]
fn session_metadata_lists_existing_jsonl_sessions_with_names_and_children() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("alpha.jsonl"), "{}\n").expect("write alpha");

    let store = SessionMetadataStore::new(dir.path());
    let child = store
        .fork("alpha", Some("Investigate parser".to_owned()))
        .expect("fork alpha");
    store
        .rename("alpha", "Main thread".to_owned())
        .expect("rename alpha");
    store
        .summarize("alpha", "Investigating parser branch".to_owned())
        .expect("summarize alpha");

    let sessions = store.list().expect("list sessions");

    assert_eq!(
        sessions,
        vec![
            SessionRecord {
                id: "alpha".to_owned(),
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
                parent_id: Some("alpha".to_owned()),
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
    std::fs::write(dir.path().join("alpha.jsonl"), "{}\n").expect("write alpha");

    let store = SessionMetadataStore::new(dir.path());
    let summarized = store
        .record_summary(
            "alpha",
            SessionSummaryRecord {
                text: "Investigating parser branch".to_owned(),
                source: SessionSummarySource::ModelGenerated,
                model: Some("openai/gpt-4.1".to_owned()),
                updated_at: Some("125.0Z".to_owned()),
            },
        )
        .expect("summarize alpha");

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
    std::fs::write(dir.path().join("alpha.jsonl"), "{}\n").expect("write alpha");
    std::fs::write(dir.path().join("beta.jsonl"), "{}\n").expect("write beta");

    let store = SessionMetadataStore::new(dir.path());
    store
        .record_activity(
            "alpha",
            Some("/workspace/old".to_owned()),
            Some("older prompt".to_owned()),
            "100".to_owned(),
        )
        .expect("record alpha activity");
    store
        .record_activity(
            "beta",
            Some("/workspace/new".to_owned()),
            Some("newer prompt".to_owned()),
            "200".to_owned(),
        )
        .expect("record beta activity");
    store
        .record_title(
            "beta",
            "Generated resume title".to_owned(),
            Some("test/model".to_owned()),
            "201".to_owned(),
        )
        .expect("record beta title");

    let sessions = store.list_recent().expect("list recent sessions");

    assert_eq!(sessions[0].id, "beta");
    assert_eq!(sessions[0].title.as_deref(), Some("Generated resume title"));
    assert_eq!(sessions[0].title_model.as_deref(), Some("test/model"));
    assert_eq!(sessions[0].title_updated_at.as_deref(), Some("201"));
    assert_eq!(sessions[0].workspace.as_deref(), Some("/workspace/new"));
    assert_eq!(
        sessions[0].last_user_prompt.as_deref(),
        Some("newer prompt")
    );
    assert_eq!(sessions[0].updated_at.as_deref(), Some("200"));
    assert_eq!(sessions[1].id, "alpha");
}

#[test]
fn session_metadata_title_falls_back_to_name_then_last_prompt_then_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("alpha.jsonl"), "{}\n").expect("write alpha");
    std::fs::write(dir.path().join("beta.jsonl"), "{}\n").expect("write beta");
    std::fs::write(dir.path().join("gamma.jsonl"), "{}\n").expect("write gamma");

    let store = SessionMetadataStore::new(dir.path());
    store
        .rename("alpha", "Named session".to_owned())
        .expect("rename alpha");
    store
        .record_activity(
            "beta",
            None,
            Some("Prompt title".to_owned()),
            "100".to_owned(),
        )
        .expect("record beta activity");

    let sessions = store.list().expect("list sessions");

    assert_eq!(sessions[0].title.as_deref(), Some("Named session"));
    assert_eq!(sessions[1].title.as_deref(), Some("Prompt title"));
    assert_eq!(sessions[2].title.as_deref(), Some("gamma"));
}
