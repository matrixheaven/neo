use neo_tui::transcript::{TranscriptController, TranscriptEntry};

#[test]
fn finalized_banner_and_user_messages_commit_into_scrollback() {
    let mut controller = TranscriptController::new();

    controller.push(TranscriptEntry::banner("Welcome to neo"));
    controller.push(TranscriptEntry::user("hello"));

    let committed = controller.drain_finalized_rows(80);

    assert!(
        committed
            .iter()
            .any(|row| row == &neo_tui::core::Line::raw("Welcome to neo"))
    );
    assert!(
        committed
            .iter()
            .any(|row| row == &neo_tui::core::Line::raw("hello"))
    );
    assert!(controller.live_entries().is_empty());
}

#[test]
fn live_tool_rows_stay_out_of_committed_scrollback() {
    let mut controller = TranscriptController::new();

    controller.push(TranscriptEntry::tool_call_running(
        "Read",
        "crates/neo-tui/src/app.rs",
    ));

    let committed = controller.drain_finalized_rows(80);

    assert!(committed.is_empty());
    assert!(!controller.live_entries().is_empty());
    assert!(
        controller
            .render_live_rows(80)
            .iter()
            .any(|row| row == &neo_tui::core::Line::raw("● Using Read (crates/neo-tui/src/app.rs)"))
    );
}

#[test]
fn finalized_prefix_stops_before_live_entry() {
    let mut controller = TranscriptController::new();

    controller.push(TranscriptEntry::banner("Welcome to neo"));
    controller.push(TranscriptEntry::assistant_live("working"));
    controller.push(TranscriptEntry::user("queued after live"));

    let committed = controller.drain_finalized_rows(80);

    assert_eq!(committed, vec![neo_tui::core::Line::raw("Welcome to neo")]);
    assert_eq!(controller.live_entries().len(), 2);
    assert_eq!(
        controller.render_live_rows(80),
        vec![
            neo_tui::core::Line::raw("working"),
            neo_tui::core::Line::raw("You"),
            neo_tui::core::Line::raw("queued after live")
        ]
    );
}

#[test]
fn finished_tool_rows_commit_once_they_are_prefix_finalized() {
    let mut controller = TranscriptController::new();

    controller.push(TranscriptEntry::tool_call_finished(
        "Read",
        "crates/neo-tui/src/app.rs",
    ));

    let first_commit = controller.drain_finalized_rows(80);
    let second_commit = controller.drain_finalized_rows(80);

    assert_eq!(
        first_commit,
        vec![neo_tui::core::Line::raw(
            "✓ Used Read (crates/neo-tui/src/app.rs)"
        )]
    );
    assert!(second_commit.is_empty());
    assert!(controller.live_entries().is_empty());
}
