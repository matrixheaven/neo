use neo_tui::shell::NeoChromeState;

#[test]
fn shell_mode_defaults_to_inactive() {
    let app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    assert!(!app.shell_mode_active());
    assert!(!app.shell_running());
}

#[test]
fn enter_and_exit_shell_mode_toggle_state() {
    let mut app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    app.enter_shell_mode();
    assert!(app.shell_mode_active());
    app.exit_shell_mode();
    assert!(!app.shell_mode_active());
}

#[test]
fn shell_running_toggle_controls_working_label() {
    let mut app = NeoChromeState::new("neo", "s1", "model", "/tmp");
    app.set_shell_running(true);
    assert!(app.shell_running());
    assert_eq!(
        app.working_label().as_deref(),
        Some("shell · esc to cancel")
    );
    app.set_shell_running(false);
    assert!(!app.shell_running());
}
