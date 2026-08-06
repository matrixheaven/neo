use neo_agent_core::session::{
    SessionAgentKind, SessionAgentRecord, SessionState, SessionStateStore, agent_tasks_dir,
    agent_wire_path, agents_dir, main_agent_wire_path, relative_agent_record_dir,
    session_state_path,
};

#[test]
fn session_layout_paths_are_agent_scoped() {
    let session_dir = std::path::Path::new("/tmp/neo-session");

    assert_eq!(
        session_state_path(session_dir),
        session_dir.join("state.json")
    );
    assert_eq!(agents_dir(session_dir), session_dir.join("agents"));
    assert_eq!(
        main_agent_wire_path(session_dir),
        session_dir.join("agents").join("main").join("wire.jsonl")
    );
    assert_eq!(
        agent_wire_path(session_dir, "agent_abc"),
        session_dir
            .join("agents")
            .join("agent_abc")
            .join("wire.jsonl")
    );
    assert_eq!(
        agent_tasks_dir(session_dir, "agent_abc"),
        session_dir.join("agents").join("agent_abc").join("tasks")
    );
}

#[tokio::test]
async fn session_state_store_round_trips_main_and_subagent_records() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionStateStore::new(temp.path());
    let mut state = SessionState::new();
    state.ensure_main_agent();
    state.upsert_agent(SessionAgentRecord {
        kind: SessionAgentKind::Sub,
        record_dir: relative_agent_record_dir("agent_abc"),
        parent_agent_id: Some("main".to_owned()),
        role: Some("coder".to_owned()),
        swarm_id: Some("swarm_1".to_owned()),
        swarm_item: Some("crate-a".to_owned()),
    });

    store.write(&state).expect("write state");
    let loaded = store.read().await.expect("read state");

    assert_eq!(loaded.schema_version, 1);
    assert_eq!(
        loaded.agents.get("main").expect("main").record_dir,
        relative_agent_record_dir("main")
    );
    assert_eq!(
        loaded
            .agents
            .get("agent_abc")
            .expect("child")
            .parent_agent_id
            .as_deref(),
        Some("main")
    );
}

#[tokio::test]
async fn session_state_store_reads_missing_state_with_main_agent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionStateStore::new(temp.path());

    let loaded = store.read().await.expect("read default state");

    assert_eq!(loaded.schema_version, 1);
    let main = loaded.agents.get("main").expect("main");
    assert_eq!(main.record_dir, relative_agent_record_dir("main"));
    assert_eq!(main.parent_agent_id, None);
    assert_eq!(main.role, None);
    assert_eq!(main.swarm_id, None);
    assert_eq!(main.swarm_item, None);
}

#[tokio::test]
async fn session_state_store_adds_missing_main_agent_when_reading_existing_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = SessionStateStore::new(temp.path());
    store
        .write(&SessionState::new())
        .expect("write state without main");

    let loaded = store.read().await.expect("read state");

    assert_eq!(loaded.schema_version, 1);
    assert_eq!(
        loaded.agents.get("main").expect("main").record_dir,
        relative_agent_record_dir("main")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn session_state_write_rejects_symlinked_state_file() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let store = SessionStateStore::new(temp.path());
    let state_path = store.path();
    let outside_state = outside.path().join("session_state.json");
    std::fs::create_dir_all(state_path.parent().expect("state parent")).unwrap();
    std::fs::write(&outside_state, "outside").unwrap();
    std::os::unix::fs::symlink(&outside_state, &state_path).unwrap();

    let error = store
        .write(&SessionState::new())
        .expect_err("session state write should reject symlinked target");

    assert!(
        error.to_string().contains("symlink"),
        "error should name symlink risk: {error}"
    );
    assert_eq!(std::fs::read_to_string(&outside_state).unwrap(), "outside");
}
