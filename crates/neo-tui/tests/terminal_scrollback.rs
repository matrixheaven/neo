use neo_agent_core::{AgentEvent, ToolResult};
use neo_tui::NeoTui;
use neo_tui::primitive::strip_ansi;
use neo_tui::screen_output::{InlineTerminal, TerminalFrame};
use neo_tui::shell::NeoChromeState;
use neo_tui::transcript::{
    FinalizedBlock, TranscriptBrowserState, TranscriptPane, TranscriptTerminalUpdate,
};

#[test]
fn semantic_block_spacing_survives_history_live_partition_and_ack_boundaries() {
    let mut screen = vt100::Parser::new(24, 80, 128);
    let mut inline = InlineTerminal::for_test(80, 24);
    let mut pane = TranscriptPane::new(80, 24);
    pane.set_live_chrome_height(0);
    let mut output = Vec::new();

    pane.push_banner("spacing-banner");
    pane.push_user_message("spacing-user");
    pane.apply_agent_event(AgentEvent::ThinkingStarted {
        turn: 1,
        id: "spacing-thinking-id".to_owned(),
    });
    pane.apply_agent_event(AgentEvent::ThinkingDelta {
        turn: 1,
        text: "spacing-thinking".to_owned(),
    });
    let update = render_update(&mut inline, &mut screen, &mut pane, &mut output);
    let banner_tail = block_tail_containing(&update.history, "spacing-banner");
    assert_blank_rows_between(&mut screen, &banner_tail, "spacing-user", 1);
    assert_blank_rows_between(&mut screen, "spacing-user", "thinking...", 1);
    pane.acknowledge_history(&update.history);

    pane.apply_agent_event(AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "spacing-tool-id".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "spacing-tool-command" }),
    });
    let update = render_update(&mut inline, &mut screen, &mut pane, &mut output);
    assert_blank_rows_between(&mut screen, "spacing-user", "spacing-thinking", 1);
    assert_blank_rows_between(&mut screen, "spacing-thinking", "spacing-tool-command", 1);
    pane.acknowledge_history(&update.history);

    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "spacing-tool-id".to_owned(),
        name: "Bash".to_owned(),
        result: ToolResult::ok("spacing-tool-result"),
    });
    pane.start_assistant_message();
    pane.append_assistant_delta("spacing-assistant-stable\n\nspacing-assistant-live");
    let update = render_update(&mut inline, &mut screen, &mut pane, &mut output);
    let tool_tail = block_tail_containing(&update.history, "spacing-tool-command");
    assert_blank_rows_between(&mut screen, "spacing-thinking", "spacing-tool-command", 1);
    assert_blank_rows_between(&mut screen, &tool_tail, "spacing-assistant-stable", 1);
    assert_blank_rows_between(
        &mut screen,
        "spacing-assistant-stable",
        "spacing-assistant-live",
        0,
    );
    pane.acknowledge_history(&update.history);

    pane.append_assistant_delta(" complete\n\nspacing-assistant-next");
    render_update(&mut inline, &mut screen, &mut pane, &mut output);
    assert_blank_rows_between(
        &mut screen,
        "spacing-assistant-stable",
        "spacing-assistant-live complete",
        0,
    );
    assert_blank_rows_between(
        &mut screen,
        "spacing-assistant-live complete",
        "spacing-assistant-next",
        0,
    );
}

#[test]
fn history_commit_does_not_leave_ghost_live_rows_above_terminal_bottom() {
    let mut screen = vt100::Parser::new(12, 80, 128);
    screen.process(b"shell-before-neo\r\n");
    let mut inline = InlineTerminal::for_test(80, 12);
    render_and_process(
        &mut inline,
        &mut screen,
        &TerminalFrame::new(
            Vec::new(),
            vec![
                "old-live-row-0".to_owned(),
                "old-live-row-1".to_owned(),
                "old-composer".to_owned(),
            ],
            None,
        ),
        &mut Vec::new(),
    );

    let mut pane = TranscriptPane::new(80, 12);
    pane.push_status("new-committed-history");
    let update = pane.render_terminal_update(80, 12);
    render_and_process(
        &mut inline,
        &mut screen,
        &TerminalFrame::new(
            update.history,
            vec!["new-live-row".to_owned(), "new-composer".to_owned()],
            None,
        ),
        &mut Vec::new(),
    );

    let visible = visible_rows(&screen);
    assert!(
        visible.iter().all(|row| !row.contains("old-live-row")),
        "obsolete live rows remained after history commit: {visible:#?}"
    );
    assert_eq!(
        visible
            .iter()
            .filter(|row| row.contains("new-live-row") || row.contains("new-composer"))
            .count(),
        2,
        "current live surface must appear exactly once: {visible:#?}"
    );
    let history_row = visible
        .iter()
        .position(|row| row.contains("new-committed-history"))
        .expect("committed history remains visible");
    let live_row = visible
        .iter()
        .position(|row| row.contains("new-live-row"))
        .expect("live row remains visible");
    let composer_row = visible
        .iter()
        .position(|row| row.contains("new-composer"))
        .expect("composer remains visible");
    assert!(
        history_row < live_row && live_row < composer_row,
        "history, live content, and composer must remain ordered: {visible:#?}"
    );
    assert!(
        visible[composer_row + 1..]
            .iter()
            .all(|row| row.trim().is_empty()),
        "old content must not survive below the composer: {visible:#?}"
    );
}

#[test]
fn suspend_resume_preserves_committed_history() {
    let mut screen = vt100::Parser::new(12, 80, 128);
    for row in 0..16 {
        screen.process(format!("shell-suspend-row-{row:02}\r\n").as_bytes());
    }

    let mut pane = TranscriptPane::new(80, 12);
    pane.push_status("committed-suspend-sentinel");
    let update = pane.render_terminal_update(80, 12);
    let live = vec![
        "live-suspend-row-0".to_owned(),
        "live-suspend-row-1".to_owned(),
    ];
    let frame = TerminalFrame::new(update.history, live.clone(), None);
    let mut inline = InlineTerminal::for_test(80, 12);
    let mut initial = Vec::new();
    inline
        .render_to(&mut initial, &frame)
        .expect("initial terminal frame");
    screen.process(&initial);

    let mut suspend = Vec::new();
    inline
        .suspend_prepare(&mut suspend)
        .expect("prepare terminal for suspend");
    assert!(!suspend.windows(4).any(|bytes| bytes == b"\x1b[2J"));
    assert!(!suspend.windows(4).any(|bytes| bytes == b"\x1b[3J"));
    screen.process(&suspend);

    let suspended_rows = all_terminal_rows(&mut screen);
    assert!(
        suspended_rows
            .iter()
            .any(|row| row.contains("committed-suspend-sentinel"))
    );
    assert!(
        suspended_rows
            .iter()
            .all(|row| !row.contains("live-suspend-row"))
    );

    screen.process(b"shell-during-suspend-sentinel\r\n");
    inline.resume().expect("resume terminal modes");
    let resumed_frame = TerminalFrame::new(Vec::new(), live, None);
    let mut resumed = Vec::new();
    inline
        .render_to(&mut resumed, &resumed_frame)
        .expect("redraw resumed live surface");
    let resumed_text = String::from_utf8(resumed.clone()).expect("ANSI output is UTF-8");
    assert!(resumed_text.contains("live-suspend-row-0"));
    assert!(resumed_text.contains("live-suspend-row-1"));
    assert!(!resumed_text.contains("committed-suspend-sentinel"));
    screen.process(&resumed);

    let retained = all_terminal_rows(&mut screen);
    assert!(
        retained
            .iter()
            .any(|row| row.contains("committed-suspend-sentinel"))
    );
    assert!(
        retained
            .iter()
            .any(|row| row.contains("shell-during-suspend-sentinel"))
    );
}

#[test]
fn leave_clears_obsolete_live_and_places_cursor_below_final_output() {
    let mut screen = vt100::Parser::new(12, 80, 128);
    for row in 0..16 {
        screen.process(format!("shell-exit-row-{row:02}\r\n").as_bytes());
    }

    let mut pane = TranscriptPane::new(80, 12);
    pane.push_status("committed-before-exit-sentinel");
    let first_update = pane.render_terminal_update(80, 12);
    let obsolete_live = vec![
        "obsolete-live-row-0".to_owned(),
        "obsolete-live-row-1".to_owned(),
    ];
    let first_frame = TerminalFrame::new(first_update.history, obsolete_live.clone(), None);
    let mut inline = InlineTerminal::for_test(80, 12);
    let mut initial = Vec::new();
    inline
        .render_to(&mut initial, &first_frame)
        .expect("initial exit frame");
    screen.process(&initial);
    pane.acknowledge_history(&first_frame.history);

    pane.push_status("final-exit-output-sentinel");
    let final_update = pane.render_terminal_update(80, 12);
    let final_frame = TerminalFrame::new(final_update.history, obsolete_live, None);
    let mut final_render = Vec::new();
    inline
        .render_to(&mut final_render, &final_frame)
        .expect("commit final output");
    screen.process(&final_render);

    let mut leave = Vec::new();
    inline.leave(&mut leave).expect("leave inline terminal");
    assert!(leave.windows(6).any(|bytes| bytes == b"\x1b[?25h"));
    assert!(!leave.windows(4).any(|bytes| bytes == b"\x1b[2J"));
    assert!(!leave.windows(4).any(|bytes| bytes == b"\x1b[3J"));
    screen.process(&leave);

    let retained = all_terminal_rows(&mut screen);
    assert!(retained.iter().any(|row| row.contains("shell-exit-row-00")));
    assert!(
        retained
            .iter()
            .any(|row| row.contains("committed-before-exit-sentinel"))
    );
    assert!(
        retained
            .iter()
            .any(|row| row.contains("final-exit-output-sentinel"))
    );
    assert!(
        retained
            .iter()
            .all(|row| !row.contains("obsolete-live-row"))
    );

    let visible = visible_rows(&screen);
    let final_row = visible
        .iter()
        .position(|row| row.contains("final-exit-output-sentinel"))
        .expect("final output remains visible");
    assert!(usize::from(screen.screen().cursor_position().0) > final_row);
}

#[test]
fn shell_and_committed_history_survive_live_updates_resize_and_exit() {
    let mut screen = vt100::Parser::new(12, 80, 4096);
    let shell_rows = (0..40)
        .map(|row| format!("shell-lifecycle-row-{row:02}"))
        .collect::<Vec<_>>();
    for row in &shell_rows {
        screen.process(format!("{row}\r\n").as_bytes());
    }

    let committed_rows = (0..30)
        .map(|row| format!("committed-lifecycle-row-{row:02}"))
        .collect::<Vec<_>>();
    let mut pane = TranscriptPane::new(80, 12);
    for row in &committed_rows {
        pane.push_status(row);
    }
    let committed_update = pane.render_terminal_update(80, 12);
    let committed_frame = TerminalFrame::new(committed_update.history, Vec::new(), None);
    let mut inline = InlineTerminal::for_test(80, 12);
    let mut output = Vec::new();
    render_and_process(&mut inline, &mut screen, &committed_frame, &mut output);
    assert_terminal_contains(&mut screen, "committed-lifecycle-row-29", "initial commit");
    pane.acknowledge_history(&committed_frame.history);

    pump_live_frames(&mut inline, &mut screen, 200, &mut output);
    assert_terminal_contains(&mut screen, "committed-lifecycle-row-29", "200 live frames");

    resize_and_render(
        &mut screen,
        &mut inline,
        &mut output,
        8,
        50,
        "lifecycle-live-after-resize-50",
        2,
    );
    assert_terminal_contains(&mut screen, "committed-lifecycle-row-29", "50x8 resize");

    resize_and_render(
        &mut screen,
        &mut inline,
        &mut output,
        20,
        100,
        "lifecycle-live-after-resize-100",
        3,
    );
    assert_terminal_contains(&mut screen, "committed-lifecycle-row-29", "100x20 resize");

    pane.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "final-lifecycle-tool".to_owned(),
        name: "Bash".to_owned(),
    });
    pane.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "final-lifecycle-tool".to_owned(),
        json_fragment: r#"{"command":"final-tool-card-sentinel"}"#.to_owned(),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "final-lifecycle-tool".to_owned(),
        name: "Bash".to_owned(),
        result: ToolResult::ok("final-tool-result-sentinel"),
    });
    let final_update = pane.render_terminal_update(100, 20);
    render_and_process(
        &mut inline,
        &mut screen,
        &TerminalFrame::new(
            final_update.history,
            vec!["obsolete-lifecycle-live".to_owned()],
            None,
        ),
        &mut output,
    );
    assert_terminal_contains(
        &mut screen,
        "committed-lifecycle-row-29",
        "final tool commit",
    );

    let mut leave = Vec::new();
    inline.leave(&mut leave).expect("leave inline terminal");
    screen.process(&leave);
    output.extend_from_slice(&leave);
    assert_terminal_contains(&mut screen, "committed-lifecycle-row-29", "terminal leave");

    let output_text = String::from_utf8(output).expect("ANSI output is UTF-8");
    assert!(!output_text.contains("\x1b[2J"));
    assert!(!output_text.contains("\x1b[3J"));
    assert!(output_text.contains("\x1b[?25h"));

    assert_lifecycle_retained(&mut screen, &shell_rows, &committed_rows);
}

#[test]
fn review_surface_transition_preserves_primary_scrollback() {
    let mut terminal = InlineTerminal::for_test(80, 12);
    let normal = TerminalFrame::with_surface(Vec::new(), vec!["normal".into()], None, false, None);
    terminal
        .render_to(&mut Vec::new(), &normal)
        .expect("normal frame");

    let review = TerminalFrame::with_surface(Vec::new(), vec!["review".into()], None, true, None);
    let mut bytes = Vec::new();
    terminal
        .render_to(&mut bytes, &review)
        .expect("review frame");
    terminal
        .render_to(&mut bytes, &normal)
        .expect("normal frame after review");

    let output = String::from_utf8(bytes).expect("ANSI output is UTF-8");
    assert!(output.contains("?1049h"));
    assert!(output.contains("?1049l"));
    assert!(!output.contains("\x1b[2J"));
    assert!(!output.contains("\x1b[3J"));
}

#[test]
fn leaving_review_appends_history_finalized_while_browser_was_open() {
    let mut screen = vt100::Parser::new(12, 80, 128);
    let mut terminal = InlineTerminal::for_test(80, 12);
    let mut pane = TranscriptPane::new(80, 12);

    pane.push_status("history-before-review");
    let initial = pane.render_terminal_update(80, 12);
    let initial_frame = TerminalFrame::new(initial.history, vec!["normal-live".into()], None);
    render_and_process(&mut terminal, &mut screen, &initial_frame, &mut Vec::new());
    pane.acknowledge_history(&initial_frame.history);

    let review =
        TerminalFrame::with_surface(Vec::new(), vec!["review-live".into()], None, true, None);
    render_and_process(&mut terminal, &mut screen, &review, &mut Vec::new());

    pane.push_status("history-finished-during-review");
    let update = pane.render_terminal_update(80, 12);
    let normal = TerminalFrame::with_surface(
        update.history,
        vec!["normal-after-review".into()],
        None,
        false,
        None,
    );
    render_and_process(&mut terminal, &mut screen, &normal, &mut Vec::new());

    let retained = all_terminal_rows(&mut screen);
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("history-finished-during-review"))
            .count(),
        1,
        "new history was lost or replayed after review: {retained:#?}"
    );
}

#[test]
fn committed_tool_review_does_not_duplicate_native_scrollback() {
    let mut screen = vt100::Parser::new(12, 80, 128);
    let mut inline = InlineTerminal::for_test(80, 12);
    let mut pane = TranscriptPane::new(80, 12);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "review-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "review-committed-tool" }),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "review-tool".to_owned(),
        name: "Read".to_owned(),
        result: ToolResult::ok("review-committed-tool-result"),
    });

    let committed = pane.render_terminal_update(80, 12);
    assert!(!committed.history.is_empty());
    let primary_frame = TerminalFrame::new(
        committed.history.clone(),
        vec!["primary-review-anchor".to_owned()],
        None,
    );
    render_and_process(&mut inline, &mut screen, &primary_frame, &mut Vec::new());
    pane.acknowledge_history(&committed.history);
    let primary_before_review = native_terminal_snapshot(&mut screen);

    let mut browser = TranscriptBrowserState::new(true);
    let expanded_rows = pane.render_browser_rows(&mut browser, 80, 12);
    assert!(
        expanded_rows
            .iter()
            .any(|row| row.contains("review-committed-tool-result")),
        "expanded review must include the committed tool result: {expanded_rows:#?}"
    );
    let expanded = TerminalFrame::with_surface(Vec::new(), expanded_rows, None, true, None);
    assert!(expanded.history.is_empty());
    let mut review_transition_output = Vec::new();
    let entering_start = review_transition_output.len();
    render_and_process(
        &mut inline,
        &mut screen,
        &expanded,
        &mut review_transition_output,
    );
    let entering_end = review_transition_output.len();

    browser.toggle();
    let collapsed_rows = pane.render_browser_rows(&mut browser, 80, 12);
    assert!(
        collapsed_rows
            .iter()
            .all(|row| !row.contains("review-committed-tool-result"))
    );
    let collapsed = TerminalFrame::with_surface(Vec::new(), collapsed_rows, None, true, None);
    assert!(collapsed.history.is_empty());
    render_and_process(
        &mut inline,
        &mut screen,
        &collapsed,
        &mut review_transition_output,
    );

    let after_review =
        TerminalFrame::new(Vec::new(), vec!["primary-review-anchor".to_owned()], None);
    let leaving_start = review_transition_output.len();
    render_and_process(
        &mut inline,
        &mut screen,
        &after_review,
        &mut review_transition_output,
    );
    let leaving_end = review_transition_output.len();
    let primary_after_review = native_terminal_snapshot(&mut screen);

    let entering_transition = &review_transition_output[entering_start..entering_end];
    let leaving_transition = &review_transition_output[leaving_start..leaving_end];
    assert!(
        String::from_utf8_lossy(entering_transition).contains("?1049h"),
        "entering review transition: {entering_transition:?}"
    );
    assert!(
        String::from_utf8_lossy(leaving_transition).contains("?1049l"),
        "leaving review transition: {leaving_transition:?}"
    );
    assert!(
        !review_transition_output
            .windows(b"\x1b[2J".len())
            .any(|window| window == b"\x1b[2J")
    );
    assert!(
        !review_transition_output
            .windows(b"\x1b[3J".len())
            .any(|window| window == b"\x1b[3J")
    );

    assert_eq!(primary_after_review, primary_before_review);
    let primary_after_review_rows = all_terminal_rows(&mut screen);
    assert_eq!(
        primary_after_review_rows
            .iter()
            .filter(|row| row.contains("review-committed-tool"))
            .count(),
        1,
        "committed tool must remain exactly once in native scrollback: {primary_after_review_rows:#?}"
    );
}

#[test]
fn review_acknowledgement_does_not_advance_normal_history_ledger() {
    let chrome = NeoChromeState::new("neo", "session", "model", ".".into());
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "review-ack-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),
    });
    transcript.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "review-ack-tool".to_owned(),
        name: "Read".to_owned(),
        result: ToolResult::ok("contents"),
    });
    let mut tui = NeoTui::new(chrome, transcript);
    let normal = tui.render_terminal_frame(80, 12);
    assert!(!normal.history.is_empty());
    let ledger_before_review = tui.transcript().has_committed_expandable_entries();
    assert!(!ledger_before_review);

    tui.chrome_mut().open_transcript_browser(true);
    let review = tui.render_terminal_frame(80, 12);
    assert!(review.review_surface);
    assert!(review.history.is_empty());
    tui.acknowledge_history(&review);
    assert_eq!(
        tui.transcript().has_committed_expandable_entries(),
        ledger_before_review,
        "review acknowledgement must not advance the normal history ledger"
    );

    tui.acknowledge_history(&normal);
    assert!(
        tui.transcript().has_committed_expandable_entries(),
        "normal acknowledgement must advance the normal history ledger"
    );
}

fn render_and_process(
    inline: &mut InlineTerminal,
    screen: &mut vt100::Parser,
    frame: &TerminalFrame,
    output: &mut Vec<u8>,
) {
    let mut transaction = Vec::new();
    inline
        .render_to(&mut transaction, frame)
        .expect("render terminal transaction");
    screen.process(&transaction);
    output.extend_from_slice(&transaction);
}

fn render_update(
    inline: &mut InlineTerminal,
    screen: &mut vt100::Parser,
    pane: &mut TranscriptPane,
    output: &mut Vec<u8>,
) -> TranscriptTerminalUpdate {
    let update = pane.render_terminal_update(80, 24);
    render_and_process(
        inline,
        screen,
        &TerminalFrame::new(update.history.clone(), update.live.clone(), None),
        output,
    );
    update
}

fn block_tail_containing(history: &[FinalizedBlock], needle: &str) -> String {
    history
        .iter()
        .find(|block| {
            block
                .lines
                .iter()
                .any(|line| strip_ansi(line).contains(needle))
        })
        .and_then(|block| {
            block
                .lines
                .iter()
                .rev()
                .map(|line| strip_ansi(line).trim().to_owned())
                .find(|line| !line.is_empty())
        })
        .unwrap_or_else(|| panic!("no history block containing {needle:?}"))
}

fn pump_live_frames(
    inline: &mut InlineTerminal,
    screen: &mut vt100::Parser,
    count: usize,
    output: &mut Vec<u8>,
) {
    for index in 0..count {
        let live = (0..3)
            .map(|row| format!("lifecycle-live-frame-{index:03}-row-{row}"))
            .collect::<Vec<_>>();
        render_and_process(
            inline,
            screen,
            &TerminalFrame::new(Vec::new(), live, None),
            output,
        );
    }
}

fn resize_and_render(
    screen: &mut vt100::Parser,
    inline: &mut InlineTerminal,
    output: &mut Vec<u8>,
    rows: u16,
    cols: u16,
    live_prefix: &str,
    live_rows: usize,
) {
    resize_vt100(screen, rows, cols);
    inline.resize(cols, rows);
    let live = (0..live_rows)
        .map(|row| format!("{live_prefix}-row-{row}"))
        .collect::<Vec<_>>();
    render_and_process(
        inline,
        screen,
        &TerminalFrame::new(Vec::new(), live, None),
        output,
    );
}

fn assert_lifecycle_retained(
    screen: &mut vt100::Parser,
    shell_rows: &[String],
    committed_rows: &[String],
) {
    let retained = all_terminal_rows(screen);
    assert_rows_once_in_order(&retained, shell_rows);
    assert_sentinels_once_in_order(&retained, committed_rows);
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("final-tool-card-sentinel"))
            .count(),
        1
    );
    // A destructive external resize can make the old live anchor unknowable
    // before Neo receives the resize event. Those rows are terminal-owned at
    // that point; clearing them could erase committed history. The live rows
    // drawn from the final established anchor must still be removed on exit.
    let stale_current_live = retained
        .iter()
        .filter(|row| {
            row.contains("lifecycle-live-after-resize-100")
                || row.contains("obsolete-lifecycle-live")
        })
        .collect::<Vec<_>>();
    assert!(
        stale_current_live.is_empty(),
        "stale rows from the final live anchor: {stale_current_live:?}"
    );
}

fn resize_vt100(terminal: &mut vt100::Parser, rows: u16, cols: u16) {
    let old_rows = terminal.screen().size().0;
    if rows < old_rows {
        terminal.process(format!("\x1b[{}S", old_rows - rows).as_bytes());
    }
    terminal.screen_mut().set_size(rows, cols);
}

fn assert_terminal_contains(terminal: &mut vt100::Parser, sentinel: &str, stage: &str) {
    assert!(
        all_terminal_rows(terminal)
            .iter()
            .any(|row| row.contains(sentinel)),
        "missing {sentinel} after {stage}"
    );
}

fn visible_rows(terminal: &vt100::Parser) -> Vec<String> {
    terminal.screen().rows(0, 80).collect()
}

#[derive(Debug, PartialEq, Eq)]
struct NativeTerminalSnapshot {
    size: (u16, u16),
    cursor_position: (u16, u16),
    alternate_screen: bool,
    hide_cursor: bool,
    scrollback_position: usize,
    scrollback_extent: usize,
    formatted_positions: Vec<(usize, Vec<u8>)>,
}

fn native_terminal_snapshot(terminal: &mut vt100::Parser) -> NativeTerminalSnapshot {
    let screen = terminal.screen();
    let size = screen.size();
    let cursor_position = screen.cursor_position();
    let alternate_screen = screen.alternate_screen();
    let hide_cursor = screen.hide_cursor();
    let scrollback_position = screen.scrollback();

    terminal.screen_mut().set_scrollback(usize::MAX);
    let scrollback_extent = terminal.screen().scrollback();
    let mut formatted_positions = Vec::with_capacity(scrollback_extent.saturating_add(1));
    for offset in 0..=scrollback_extent {
        terminal.screen_mut().set_scrollback(offset);
        formatted_positions.push((offset, terminal.screen().state_formatted()));
    }
    terminal.screen_mut().set_scrollback(scrollback_position);

    NativeTerminalSnapshot {
        size,
        cursor_position,
        alternate_screen,
        hide_cursor,
        scrollback_position,
        scrollback_extent,
        formatted_positions,
    }
}

fn all_terminal_rows(terminal: &mut vt100::Parser) -> Vec<String> {
    terminal.screen_mut().set_scrollback(usize::MAX);
    let maximum_scrollback = terminal.screen().scrollback();
    let mut rows = visible_rows(terminal);
    for offset in (0..maximum_scrollback).rev() {
        terminal.screen_mut().set_scrollback(offset);
        rows.push(
            visible_rows(terminal)
                .pop()
                .expect("terminal has visible rows"),
        );
    }
    rows
}

fn assert_blank_rows_between(
    terminal: &mut vt100::Parser,
    before: &str,
    after: &str,
    expected: usize,
) {
    let rows = all_terminal_rows(terminal);
    let before_index = rows
        .iter()
        .position(|row| row.contains(before))
        .unwrap_or_else(|| panic!("missing row containing {before:?}: {rows:#?}"));
    let after_index = rows
        .iter()
        .position(|row| row.contains(after))
        .unwrap_or_else(|| panic!("missing row containing {after:?}: {rows:#?}"));
    assert!(
        before_index < after_index,
        "expected {before:?} before {after:?}: {rows:#?}"
    );
    let between = &rows[before_index + 1..after_index];
    assert!(
        between.iter().all(|row| row.trim().is_empty()),
        "non-blank rows between {before:?} and {after:?}: {between:#?}"
    );
    assert_eq!(
        between.len(),
        expected,
        "blank row count between {before:?} and {after:?}: {rows:#?}"
    );
}

fn assert_rows_once_in_order(actual: &[String], expected: &[String]) {
    let mut previous = None;
    for expected_row in expected {
        let matches = actual
            .iter()
            .enumerate()
            .filter_map(|(index, row)| (row == expected_row).then_some(index))
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "row occurrence count for {expected_row}");
        if let Some(previous) = previous {
            assert!(matches[0] > previous, "row order at {expected_row}");
        }
        previous = Some(matches[0]);
    }
}

fn assert_sentinels_once_in_order(actual: &[String], expected: &[String]) {
    let mut previous = None;
    for expected_row in expected {
        let matches = actual
            .iter()
            .enumerate()
            .filter_map(|(index, row)| row.contains(expected_row).then_some(index))
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "row occurrence count for {expected_row}");
        if let Some(previous) = previous {
            assert!(matches[0] > previous, "row order at {expected_row}");
        }
        previous = Some(matches[0]);
    }
}
