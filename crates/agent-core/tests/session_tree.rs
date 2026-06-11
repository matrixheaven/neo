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
                children: vec![child.id.clone()],
            },
            SessionRecord {
                id: child.id,
                name: Some("Investigate parser".to_owned()),
                summary: None,
                parent_id: Some("alpha".to_owned()),
                summary_record: None,
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
