//! Interactive sessions behavior (moved from `tests.rs`).

use std::{fs, path::PathBuf};

use clap::Parser as _;
use neo_agent_core::{AgentEvent, AgentMessage};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    shell::OverlayKind,
};

use super::super::*;
use super::*;

#[tokio::test]
async fn session_catalog_and_loader_use_real_local_session_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    // Compute the workspace-scoped bucket directory that the code will use.
    let bucket_dir = workspace_sessions_dir(&test_config(temp.path(), sessions_dir.clone()));
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    write_main_wire(
        &bucket_dir,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi back\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );

    let store = SessionMetadataStore::new(&bucket_dir);
    store
        .rename(SESSION_A, "Alpha Session".to_owned())
        .expect("rename session");
    store
        .summarize(SESSION_A, "Local branch summary".to_owned())
        .expect("summarize session");
    let child = store
        .fork(SESSION_A, Some("Parser branch".into()))
        .expect("fork session");
    store
        .record_activity(
            SESSION_A,
            Some(temp.path().display().to_string()),
            Some("hello".into()),
            "100".to_owned(),
        )
        .expect("record session activity");
    store
        .record_activity(
            &child.id,
            Some(temp.path().display().to_string()),
            Some("child prompt".into()),
            "200".to_owned(),
        )
        .expect("record child activity");

    let config = test_config(temp.path(), sessions_dir);
    let catalog = session_catalog_for_config(&config);
    assert_eq!(catalog.error, None);
    assert_eq!(catalog.items.len(), 2);
    assert_eq!(catalog.items[0].id, child.id);
    assert_eq!(catalog.items[0].title.as_deref(), Some("Parser branch"));
    assert!(
        catalog.items[0]
            .last_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("child prompt"))
    );
    assert_eq!(catalog.items[1].id, SESSION_A);
    assert_eq!(catalog.items[1].title.as_deref(), Some("Alpha Session"));
    assert!(
        catalog.items[1]
            .last_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("hello"))
    );

    let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("load session transcript");
    assert_eq!(loaded.label, SESSION_A);
    assert_eq!(loaded.terminal_title.as_deref(), Some("Alpha Session"));
    assert_eq!(
        loaded.notices,
        vec!["branch summary: Local branch summary".to_owned()]
    );
    assert_eq!(loaded.messages.len(), 2);
    assert!(matches!(
        &loaded.messages[0],
        AgentMessage::User { content, .. } if content[0].as_text() == Some("hello")
    ));
    assert!(matches!(
        &loaded.messages[1],
        AgentMessage::Assistant { content, .. } if content[0].as_text() == Some("hi back")
    ));
}

#[tokio::test]
async fn fork_session_transcript_copies_jsonl_metadata_and_loads_child() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir.clone());
    let bucket_dir = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    write_main_wire(
        &bucket_dir,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi back\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );

    // Seed parent metadata so we can verify it is inherited by the fork.
    SessionMetadataStore::new(&bucket_dir)
        .record_activity(
            SESSION_A,
            Some("/fake/workspace".to_owned()),
            Some("what is neo?".to_owned()),
            "1000.000000000Z".to_owned(),
        )
        .expect("record parent activity");
    SessionMetadataStore::new(&bucket_dir)
        .record_title(
            SESSION_A,
            "Intro to neo".to_owned(),
            Some("test-model".to_owned()),
            "1000.000000000Z".to_owned(),
        )
        .expect("record parent title");

    let forked = fork_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("fork session");

    assert!(forked.session_id.starts_with("session_"));
    assert_eq!(forked.transcript.label, forked.session_id);
    assert!(
        forked.transcript.notices.is_empty(),
        "fork notices are pushed by the controller, not by fork_session_transcript"
    );
    assert_eq!(forked.transcript.messages.len(), 2);
    assert!(
        neo_agent_core::session::main_agent_wire_path(&bucket_dir.join(&forked.session_id))
            .is_file()
    );

    let sessions = SessionMetadataStore::new(&bucket_dir)
        .list()
        .expect("list sessions");
    let parent = sessions
        .iter()
        .find(|session| session.id == SESSION_A)
        .expect("parent listed");
    assert!(parent.children.contains(&forked.session_id));
    let child = sessions
        .iter()
        .find(|session| session.id == forked.session_id)
        .expect("child listed");
    assert_eq!(child.parent_id.as_deref(), Some(SESSION_A));
    // Fork inherits parent title with [fork] prefix.
    assert_eq!(
        child.title.as_deref(),
        Some("[fork] Intro to neo"),
        "child title should be [fork]-prefixed parent title"
    );
    // Fork inherits parent workspace and last_user_prompt.
    assert_eq!(
        child.workspace.as_deref(),
        Some("/fake/workspace"),
        "child inherits parent workspace"
    );
    assert_eq!(
        child.last_user_prompt.as_deref(),
        Some("what is neo?"),
        "child inherits parent last_user_prompt"
    );
    // Fork updated_at is set (not empty / not epoch zero).
    let child_ts = child.updated_at.as_deref().unwrap_or("");
    assert!(
        !child_ts.is_empty() && child_ts != "0" && child_ts != "0.000000000Z",
        "child updated_at should be a real timestamp, got: {child_ts}"
    );
}

#[tokio::test]
async fn session_picker_ctrl_a_toggles_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");

    let project_a = temp.path().join("project_a");
    let config_a =
        add_indexed_session_fixture(&sessions_dir, &project_a, SESSION_A, "alpha prompt", "200");

    let project_b = temp.path().join("project_b");
    add_indexed_session_fixture(&sessions_dir, &project_b, SESSION_B, "beta prompt", "100");

    let mut controller = controller_for_config(&config_a);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+r").expect("valid key")))
        .await
        .expect("ctrl+r opens session picker");
    let overlay = controller.chrome().focused_overlay().expect("picker open");
    assert!(
        matches!(
            &overlay.kind,
            OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::Workspace
        ),
        "workspace scope on open"
    );
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.to_lowercase().contains("alpha"),
        "workspace scope should show alpha: {snapshot}"
    );
    assert!(
        !snapshot.to_lowercase().contains("beta"),
        "workspace scope should not show beta: {snapshot}"
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+a").expect("valid key")))
        .await
        .expect("ctrl+a toggles scope");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("picker still open");
    assert!(
        matches!(
            &overlay.kind,
            OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::All
        ),
        "all scope after toggle"
    );
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.to_lowercase().contains("alpha"),
        "all scope should show alpha: {snapshot}"
    );
    assert!(
        snapshot.to_lowercase().contains("beta"),
        "all scope should show beta: {snapshot}"
    );
}

#[tokio::test]
async fn session_picker_ctrl_a_empty_target_scope_can_toggle_back() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");

    let project_a = temp.path().join("project_a");
    fs::create_dir_all(&project_a).expect("create project_a");
    let config_a = test_config(&project_a, sessions_dir.clone());
    let bucket_a = workspace_sessions_dir(&config_a);
    fs::create_dir_all(&bucket_a).expect("create bucket_a");
    write_main_wire(
        &bucket_a,
        SESSION_A,
        r#"{"MessageAppended":{"message":{"User":{"content":[{"Text":{"text":"hello"}}]}}}}"#,
    );
    let store_a = SessionMetadataStore::new(&bucket_a);
    store_a
        .record_activity(
            SESSION_A,
            Some(project_a.display().to_string()),
            Some("alpha prompt".into()),
            "200".to_owned(),
        )
        .expect("record alpha");

    let mut controller = controller_for_config(&config_a);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+r").expect("valid key")))
        .await
        .expect("ctrl+r opens session picker");
    let overlay = controller.chrome().focused_overlay().expect("picker open");
    assert!(
        matches!(
            &overlay.kind,
            OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::Workspace
        ),
        "workspace scope on open"
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+a").expect("valid key")))
        .await
        .expect("ctrl+a switches to empty all-sessions scope");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("empty picker stays open");
    assert!(
        matches!(
            &overlay.kind,
            OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::All
        ),
        "all scope remains toggleable when empty"
    );
    assert!(transcript_has_status(
        &controller,
        "No sessions in all sessions. Press Ctrl+A again to switch back to current workspace."
    ));

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+a").expect("valid key")))
        .await
        .expect("ctrl+a toggles back to workspace scope");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("workspace picker opens again");
    assert!(
        matches!(
            &overlay.kind,
            OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::Workspace
        ),
        "workspace scope after toggling back"
    );
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.to_lowercase().contains("alpha"),
        "workspace scope should show alpha after toggling back: {snapshot}"
    );
}

#[tokio::test]
async fn cross_workspace_picker_emits_parseable_product_resume_command() {
    let other_dir = tempfile::tempdir().expect("tempdir");
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: vec![SessionSummary {
                id: SESSION_A.to_owned(),
                title: Some("Alpha session".into()),
                last_prompt: Some("hello".into()),
                work_dir: other_dir.path().to_path_buf(),
                updated_at: String::new(),
                metadata: None,
            }],
            session_error: None,
            model_items: Vec::new(),
        },
        |_session_id| async move {
            panic!("load_session should not be called for a cross-cwd session");
            #[allow(unreachable_code)]
            Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
        },
    );
    controller.set_clipboard_writer(Arc::new(|_text| Box::pin(async { Ok(()) })));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
        .await
        .expect("session picker opens");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("select cross-cwd session");
    wait_for_clipboard_idle(&mut controller).await;

    let expected = format!("neo resume {SESSION_A}");
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(&controller, &expected));
    let parsed = crate::cli::Cli::try_parse_from(expected.split_whitespace())
        .expect("resume command should be parseable by Neo");
    assert!(matches!(
        parsed.command,
        Some(crate::cli::Command::Resume { session_id: Some(id) }) if id == SESSION_A
    ));
}

#[tokio::test]
async fn submitted_prompt_is_persisted_to_workspace_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("prompt-history.jsonl");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));

    let mut controller = controller_with_history_store(store);

    controller.type_text("real prompt from this session");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let persisted = std::fs::read_to_string(&path).expect("history file exists");
    assert!(
        persisted.contains("real prompt from this session"),
        "prompt should be persisted: {persisted}"
    );

    // A fresh controller on the same workspace bucket recalls it.
    let store2 = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    let controller2 = controller_with_history_store(store2);
    assert_eq!(
        controller2
            .chrome()
            .prompt()
            .history_snapshot()
            .last()
            .map(String::as_str),
        Some("real prompt from this session")
    );
    drop(dir);
}

#[tokio::test]
async fn prompt_history_is_shared_across_sessions_in_same_workspace() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store_a = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));

    // Session A submits a prompt.
    let mut controller_a = controller_with_history_store(store_a);
    controller_a.type_text("first from session a");
    controller_a
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("session a submits");
    controller_a
        .wait_for_active_turn()
        .await
        .expect("session a turn completes");

    // Session B starts fresh in the same workspace bucket and recalls A's
    // prompt via Up from an empty composer.
    let store_b = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    let mut controller_b = controller_with_history_store(store_b);
    controller_b
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls cross-session prompt");
    assert_eq!(controller_b.chrome().prompt().text, "first from session a");
    drop(dir);
}

#[tokio::test]
async fn prompt_history_is_isolated_by_workspace_bucket() {
    let dir_one = tempfile::tempdir().expect("temp dir one");
    let dir_two = tempfile::tempdir().expect("temp dir two");

    let store_one =
        crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir_one.path()));
    let mut controller_one = controller_with_history_store(store_one);
    controller_one.type_text("workspace one");
    controller_one
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("workspace one submits");
    controller_one
        .wait_for_active_turn()
        .await
        .expect("workspace one turn completes");

    // A different workspace bucket must not recall workspace one's prompt.
    let store_two =
        crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir_two.path()));
    let controller_two = controller_with_history_store(store_two);
    assert!(
        controller_two
            .chrome()
            .prompt()
            .history_snapshot()
            .is_empty(),
        "history must be isolated per workspace bucket"
    );
    drop(dir_one);
    drop(dir_two);
}
