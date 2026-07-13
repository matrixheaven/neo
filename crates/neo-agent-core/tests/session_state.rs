use neo_agent_core::session::{SessionState, SessionStateStore};

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
