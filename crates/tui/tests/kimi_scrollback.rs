use neo_tui::core::{Line, TerminalRenderer};
use neo_tui::renderer::CursorPos;
use neo_tui::transcript::{TranscriptController, TranscriptEntry};

#[test]
fn finalized_banner_and_user_messages_commit_into_scrollback() {
    let mut controller = TranscriptController::new();
    let mut terminal = TerminalRenderer::new(80, 24);

    controller.push(TranscriptEntry::banner("Welcome to neo"));
    controller.push(TranscriptEntry::user("hello"));

    let committed = controller.drain_finalized_rows(80);
    terminal.commit_rows(&committed);

    assert!(
        terminal
            .committed_rows()
            .iter()
            .any(|row| row == &Line::raw("Welcome to neo"))
    );
    assert!(
        terminal
            .committed_rows()
            .iter()
            .any(|row| row == &Line::raw("hello"))
    );
    assert!(controller.live_entries().is_empty());
}

#[test]
fn live_tool_rows_stay_out_of_committed_scrollback() {
    let mut controller = TranscriptController::new();
    let mut terminal = TerminalRenderer::new(80, 24);

    controller.push(TranscriptEntry::tool_call_running(
        "Read",
        "crates/tui/src/app.rs",
    ));

    let committed = controller.drain_finalized_rows(80);
    terminal.commit_rows(&committed);

    assert!(terminal.committed_rows().is_empty());
    assert!(!controller.live_entries().is_empty());
    assert!(
        controller
            .render_live_rows(80)
            .iter()
            .any(|row| row == &Line::raw("● Using Read (crates/tui/src/app.rs)"))
    );
}

#[test]
fn finalized_prefix_stops_before_live_entry() {
    let mut controller = TranscriptController::new();

    controller.push(TranscriptEntry::banner("Welcome to neo"));
    controller.push(TranscriptEntry::assistant_live("working"));
    controller.push(TranscriptEntry::user("queued after live"));

    let committed = controller.drain_finalized_rows(80);

    assert_eq!(committed, vec![Line::raw("Welcome to neo")]);
    assert_eq!(controller.live_entries().len(), 2);
    assert_eq!(
        controller.render_live_rows(80),
        vec![
            Line::raw("working"),
            Line::raw("You"),
            Line::raw("queued after live")
        ]
    );
}

#[test]
fn finished_tool_rows_commit_once_they_are_prefix_finalized() {
    let mut controller = TranscriptController::new();

    controller.push(TranscriptEntry::tool_call_finished(
        "Read",
        "crates/tui/src/app.rs",
    ));

    let first_commit = controller.drain_finalized_rows(80);
    let second_commit = controller.drain_finalized_rows(80);

    assert_eq!(
        first_commit,
        vec![Line::raw("✓ Used Read (crates/tui/src/app.rs)")]
    );
    assert!(second_commit.is_empty());
    assert!(controller.live_entries().is_empty());
}

#[test]
fn terminal_renderer_builds_commit_buffer_before_live_region_without_clear_screen() {
    let mut renderer = TerminalRenderer::new(80, 24);
    renderer.render_live_region(&[Line::raw("old live")], Some(CursorPos { row: 0, col: 3 }));

    let buffer = renderer.commit_buffer(&[Line::raw("one"), Line::raw("two")]);

    assert!(buffer.contains("one"));
    assert!(buffer.contains("two"));
    assert!(buffer.contains("\r\none\r\ntwo"));
    assert!(!buffer.contains("\x1b[2J"));
    assert!(!buffer.contains("old live"));
    assert_eq!(renderer.live_rows(), &[Line::raw("old live")]);
    assert_eq!(renderer.cursor(), Some(CursorPos { row: 0, col: 3 }));
}

#[test]
fn terminal_renderer_live_buffer_clears_only_previous_live_rows() {
    let mut renderer = TerminalRenderer::new(80, 24);
    renderer.render_live_region(&[Line::raw("old one"), Line::raw("old two")], None);

    let buffer = renderer.live_region_buffer(&[Line::raw("new")], None);

    assert!(buffer.contains("new"));
    assert!(buffer.matches("\x1b[2K").count() >= 2);
    assert!(!buffer.contains("\x1b[2J"));
    assert!(!buffer.contains("old one"));
}

#[test]
fn terminal_renderer_write_commit_writes_and_records_rows() {
    let mut renderer = TerminalRenderer::new(80, 24);
    let mut output = Vec::new();

    renderer
        .write_commit(&mut output, &[Line::raw("committed")])
        .expect("commit writes to buffer");

    assert_eq!(String::from_utf8(output).expect("utf8"), "\r\ncommitted");
    assert_eq!(renderer.committed_rows(), &[Line::raw("committed")]);
}

#[test]
fn terminal_renderer_live_region_starts_cleanly_after_commit_write() {
    let mut renderer = TerminalRenderer::new(80, 24);
    let mut output = Vec::new();
    renderer.render_live_region(&[Line::raw("old live")], None);

    renderer
        .write_commit(&mut output, &[Line::raw("committed")])
        .expect("commit writes to buffer");
    renderer
        .write_live_region(&mut output, &[Line::raw("new live")], None)
        .expect("live region writes to buffer");

    let output = String::from_utf8(output).expect("utf8");
    assert!(
        output.contains("\r\ncommitted\x1b[?2026h\r\n\x1b[2Knew live"),
        "live rows must begin on a fresh clean line after commits: {output:?}"
    );
    assert!(!output.contains("committed\x1b[?2026h\x1b[2Knew live"));
}

#[test]
fn terminal_renderer_resize_preserves_committed_rows_and_reclamps_live_rows() {
    let mut renderer = TerminalRenderer::new(10, 5);
    renderer.commit_rows(&[Line::raw("history")]);
    renderer.render_live_region(
        &[
            Line::raw("1234567890"),
            Line::raw("abcdefghij"),
            Line::raw("klmnopqrst"),
        ],
        Some(CursorPos { row: 2, col: 9 }),
    );

    renderer.resize(6, 2);

    assert_eq!(renderer.dimensions(), (6, 2));
    assert_eq!(renderer.committed_rows(), &[Line::raw("history")]);
    assert_eq!(
        renderer.live_rows(),
        &[Line::raw("abcde…"), Line::raw("klmno…")]
    );
    assert_eq!(renderer.cursor(), Some(CursorPos { row: 1, col: 5 }));
}

#[test]
fn terminal_renderer_serializes_commit_live_commit_live_without_full_redraw() {
    let mut renderer = TerminalRenderer::new(80, 6);
    let mut output = Vec::new();

    renderer
        .write_commit(&mut output, &[Line::raw("Welcome to neo")])
        .expect("initial commit writes");
    renderer
        .write_live_region(&mut output, &[Line::raw("● Using Bash (cargo test)")], None)
        .expect("running live tool writes");
    renderer
        .write_commit(&mut output, &[Line::raw("✓ Used Bash (cargo test)")])
        .expect("finished tool commit writes");
    renderer
        .write_live_region(&mut output, &[Line::raw("> next prompt")], None)
        .expect("prompt live region writes");

    let output = String::from_utf8(output).expect("utf8");
    let welcome = output.find("Welcome to neo").expect("welcome committed");
    let running = output.find("Using Bash").expect("running tool live");
    let finished = output.find("Used Bash").expect("finished tool committed");
    let prompt = output.find("> next prompt").expect("prompt live row");

    assert!(welcome < running);
    assert!(running < finished);
    assert!(finished < prompt);
    assert!(!output.contains("\x1b[2J"));
    assert!(output.contains("\r\n✓ Used Bash (cargo test)\x1b[?2026h\r\n\x1b[2K> next prompt"));
    assert_eq!(
        renderer.committed_rows(),
        &[
            Line::raw("Welcome to neo"),
            Line::raw("✓ Used Bash (cargo test)")
        ]
    );
    assert_eq!(renderer.live_rows(), &[Line::raw("> next prompt")]);
}
