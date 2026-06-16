use neo_tui::app::TuiTheme;
use neo_tui::transcript::{TranscriptController, TranscriptEntry};

#[test]
fn finalized_banner_and_user_messages_commit_into_scrollback() {
    let mut controller = TranscriptController::new();
    let theme = TuiTheme::default();

    controller.push(TranscriptEntry::banner("Welcome to neo"));
    controller.push(TranscriptEntry::user("hello"));

    let committed = controller.drain_finalized_rows(80, &theme);

    assert!(
        committed
            .iter()
            .any(|row| row.text().contains("Welcome to neo"))
    );
    assert!(
        committed
            .iter()
            .any(|row| row.text().contains("✨") && row.text().contains("hello"))
    );
    assert!(controller.live_entries().is_empty());
}

#[test]
fn live_tool_rows_stay_out_of_committed_scrollback() {
    let mut controller = TranscriptController::new();
    let theme = TuiTheme::default();

    controller.push(TranscriptEntry::tool_call_running(
        "Read",
        "crates/neo-tui/src/app.rs",
    ));

    let committed = controller.drain_finalized_rows(80, &theme);

    assert!(committed.is_empty());
    assert!(!controller.live_entries().is_empty());
    assert!(
        controller
            .render_live_rows(80, &theme)
            .iter()
            .any(|row| row.text() == "● Using Read (crates/neo-tui/src/app.rs)")
    );
}

#[test]
fn finalized_prefix_stops_before_live_entry() {
    let mut controller = TranscriptController::new();
    let theme = TuiTheme::default();

    controller.push(TranscriptEntry::banner("Welcome to neo"));
    controller.push(TranscriptEntry::assistant_live("working"));
    controller.push(TranscriptEntry::user("queued after live"));

    let committed = controller.drain_finalized_rows(80, &theme);

    // Banner renders as a multi-line rounded box; verify it's committed and
    // contains the title, rather than asserting an exact line count.
    assert!(!committed.is_empty());
    assert!(
        committed
            .iter()
            .any(|row| row.text().contains("Welcome to neo"))
    );
    assert_eq!(controller.live_entries().len(), 2);
    let live = controller
        .render_live_rows(80, &theme)
        .into_iter()
        .map(|row| row.text())
        .collect::<Vec<_>>();
    // Live assistant text (no bullet yet) + bullet-led user message.
    assert_eq!(
        live,
        vec!["working", "✨ queued after live"],
        "live region: {live:?}"
    );
}

#[test]
fn finished_tool_rows_commit_once_they_are_prefix_finalized() {
    let mut controller = TranscriptController::new();
    let theme = TuiTheme::default();

    controller.push(TranscriptEntry::tool_call_finished(
        "Read",
        "crates/neo-tui/src/app.rs",
    ));

    let first_commit = controller.drain_finalized_rows(80, &theme);
    let second_commit = controller.drain_finalized_rows(80, &theme);

    assert_eq!(first_commit.len(), 1);
    assert_eq!(
        first_commit[0].text(),
        "● Used Read (crates/neo-tui/src/app.rs)"
    );
    assert!(second_commit.is_empty());
    assert!(controller.live_entries().is_empty());
}
