use std::fs;
use std::path::Path;

use super::super::{SessionError, SessionMetadataStore, SessionRecord, main_agent_wire_path};

const SESSION_ID: &str = "session_00000000-0000-4000-8000-000000000301";

fn write_session_transcript(dir: &Path, session_id: &str) {
    let session_dir = dir.join(session_id);
    let wire_path = main_agent_wire_path(&session_dir);
    fs::create_dir_all(wire_path.parent().expect("wire parent")).expect("create session dir");
    fs::write(wire_path, "{}\n").expect("write transcript");
}

fn listed_session(store: &SessionMetadataStore, session_id: &str) -> SessionRecord {
    store
        .list()
        .expect("list sessions")
        .into_iter()
        .find(|session| session.id == session_id)
        .expect("session should be listed")
}

#[test]
fn legacy_metadata_defaults_to_unpinned_and_unarchived() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sessions_dir = dir.path();
    write_session_transcript(sessions_dir, SESSION_ID);

    let legacy = format!(
        "{{\n  \"sessions\": {{\n    \"{SESSION_ID}\": {{\n      \"name\": \"legacy session\"\n    }}\n  }}\n}}\n"
    );
    fs::write(sessions_dir.join("sessions.metadata.json"), legacy)
        .expect("write legacy metadata without pinned/archived");

    let record = listed_session(&SessionMetadataStore::new(sessions_dir), SESSION_ID);
    assert!(!record.pinned, "missing pinned must default to false");
    assert!(!record.archived, "missing archived must default to false");
}

#[test]
fn metadata_update_persists_pinned_and_archived_across_store_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sessions_dir = dir.path();
    write_session_transcript(sessions_dir, SESSION_ID);

    let store = SessionMetadataStore::new(sessions_dir);
    let updated = store
        .update_metadata(
            SESSION_ID,
            Some("WebUI title".to_owned()),
            Some(true),
            Some(true),
        )
        .expect("update title, pinned, and archived");
    assert!(updated.pinned, "updated record reports pinned");
    assert!(updated.archived, "updated record reports archived");
    assert_eq!(
        updated.name.as_deref(),
        Some("WebUI title"),
        "user title becomes the display name"
    );

    let reopened = SessionMetadataStore::new(sessions_dir);
    let record = listed_session(&reopened, SESSION_ID);
    assert!(record.pinned, "pinned survives store reopen");
    assert!(record.archived, "archived survives store reopen");
    assert_eq!(
        record.name.as_deref(),
        Some("WebUI title"),
        "user title survives store reopen"
    );
}

#[test]
fn metadata_update_rejects_invalid_session_id_without_creating_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sessions_dir = dir.path();

    let store = SessionMetadataStore::new(sessions_dir);
    let error = store
        .update_metadata("not-a-session-id", None, Some(true), None)
        .expect_err("invalid session id must be rejected");
    assert!(matches!(error, SessionError::InvalidId(_)));

    assert!(
        !sessions_dir.join("sessions.metadata.json").exists(),
        "rejected update must not create a metadata file"
    );
}

#[test]
fn metadata_update_rejects_missing_session_without_creating_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sessions_dir = dir.path();

    // Session directory skeleton without the transcript wire file.
    let wire_path = main_agent_wire_path(&sessions_dir.join(SESSION_ID));
    fs::create_dir_all(wire_path.parent().expect("wire parent")).expect("create session dir");

    let store = SessionMetadataStore::new(sessions_dir);
    let error = store
        .update_metadata(SESSION_ID, None, Some(true), None)
        .expect_err("missing session must be rejected");
    assert!(matches!(error, SessionError::MissingSession(_)));

    assert!(
        !sessions_dir.join("sessions.metadata.json").exists(),
        "rejected update must not create a metadata file"
    );
}
