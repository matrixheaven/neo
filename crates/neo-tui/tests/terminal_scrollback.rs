use neo_agent_core::{AgentEvent, StopReason, ToolResult};
use neo_tui::NeoTui;
use neo_tui::primitive::strip_ansi;
use neo_tui::screen_output::{InlineTerminal, TerminalFrame};
use neo_tui::shell::NeoChromeState;
use neo_tui::transcript::{
    FinalizedBlock, TranscriptBrowserState, TranscriptEntry, TranscriptPane,
    TranscriptTerminalUpdate,
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
    // Use low-level thinking manipulation to avoid triggering
    // live_model_attempt. The test exercises history/live partitioning
    // independently of the full model turn lifecycle.
    pane.transcript_mut().start_thinking();
    pane.transcript_mut()
        .append_thinking_delta("spacing-thinking");
    pane.mark_dirty();
    let update = render_update(&mut inline, &mut screen, &mut pane, &mut output);
    let banner_tail = block_tail_containing(&update.history, "spacing-banner");
    assert_blank_rows_between(&mut screen, &banner_tail, "spacing-user", 1);
    assert_blank_rows_between(&mut screen, "spacing-user", "thinking...", 1);
    pane.acknowledge_history(&update.history);

    pane.transcript_mut().finish_thinking();
    pane.mark_dirty();
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "spacing-tool-id".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "spacing-tool-command" }),

        workflow_origin: None,
    });
    let update = render_update(&mut inline, &mut screen, &mut pane, &mut output);
    assert_blank_rows_between(&mut screen, "spacing-user", "spacing-thinking", 1);
    assert_blank_rows_between(&mut screen, "spacing-thinking", "● Using Bash", 1);
    assert_blank_rows_between(&mut screen, "● Using Bash", "$ spacing-tool-command", 0);
    pane.acknowledge_history(&update.history);

    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "spacing-tool-id".to_owned(),
        name: "Bash".to_owned(),
        result: ToolResult::ok("spacing-tool-result"),

        workflow_origin: None,
    });
    pane.start_assistant_message();
    pane.append_assistant_delta("spacing-assistant-stable\n\nspacing-assistant-live");
    let update = render_update(&mut inline, &mut screen, &mut pane, &mut output);
    let tool_tail = block_tail_containing(&update.history, "spacing-tool-command");
    assert_blank_rows_between(&mut screen, "spacing-thinking", "● Used Bash", 1);
    assert_blank_rows_between(&mut screen, "● Used Bash", "$ spacing-tool-command", 0);
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
fn thinking_keeps_one_blank_row_after_tool_while_streaming_and_complete() {
    let mut screen = vt100::Parser::new(24, 80, 128);
    let mut inline = InlineTerminal::for_test(80, 24);
    let mut pane = TranscriptPane::new(80, 24);
    pane.set_live_chrome_height(0);
    let mut output = Vec::new();

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "thinking-spacing-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": ".tmp/report.md" }),
        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "thinking-spacing-tool".to_owned(),
        name: "Read".to_owned(),
        result: ToolResult::ok("report body"),
        workflow_origin: None,
    });
    let update = render_update(&mut inline, &mut screen, &mut pane, &mut output);
    let tool_tail = block_tail_containing(&update.history, "report body");
    pane.acknowledge_history(&update.history);

    pane.transcript_mut().start_thinking();
    pane.transcript_mut()
        .append_thinking_delta("thinking spacing sentinel");
    pane.mark_dirty();
    render_update(&mut inline, &mut screen, &mut pane, &mut output);
    assert_blank_rows_between(&mut screen, &tool_tail, "thinking...", 1);

    pane.transcript_mut().finish_thinking();
    pane.mark_dirty();
    render_update(&mut inline, &mut screen, &mut pane, &mut output);
    assert_blank_rows_between(&mut screen, &tool_tail, "thinking spacing sentinel", 1);
}

#[test]
fn tool_keeps_one_blank_row_after_stable_content_while_running_and_complete() {
    let mut screen = vt100::Parser::new(24, 80, 128);
    let mut inline = InlineTerminal::for_test(80, 24);
    let mut pane = TranscriptPane::new(80, 24);
    pane.set_live_chrome_height(0);
    let mut output = Vec::new();

    pane.push_status("stable content before tool");
    let update = render_update(&mut inline, &mut screen, &mut pane, &mut output);
    let stable_tail = block_tail_containing(&update.history, "stable content before tool");
    pane.acknowledge_history(&update.history);

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "running-tool-spacing".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "true" }),
        workflow_origin: None,
    });
    render_update(&mut inline, &mut screen, &mut pane, &mut output);
    assert_blank_rows_between(&mut screen, &stable_tail, "● Using Bash", 1);

    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "running-tool-spacing".to_owned(),
        name: "Bash".to_owned(),
        result: ToolResult::ok("done"),
        workflow_origin: None,
    });
    render_update(&mut inline, &mut screen, &mut pane, &mut output);
    assert_blank_rows_between(&mut screen, &stable_tail, "● Used Bash", 1);
}

#[test]
fn history_commit_never_moves_live_chrome_into_native_scrollback() {
    // Seed more than one screen of shell rows so later history commits can
    // push material into native scrollback rather than only the visible page.
    let height = 10u16;
    let width = 80u16;
    let mut screen = vt100::Parser::new(height, width, 512);
    let shell_first = "shell-scrollback-first-sentinel";
    let shell_last = "shell-scrollback-last-sentinel";
    screen.process(format!("{shell_first}\r\n").as_bytes());
    for row in 1..24 {
        screen.process(format!("shell-scrollback-row-{row:02}\r\n").as_bytes());
    }
    // Seed the last shell sentinel so pre-Neo content occupies the bottom of
    // the screen; Neo must not leave mutable chrome beside it in scrollback.
    screen.process(format!("{shell_last}\r\n").as_bytes());

    // After 25 shell lines on a 10-row screen the cursor sits at the bottom.
    let mut inline = InlineTerminal::for_test_with_cursor(width, height, 0, height - 1);
    let mut output = Vec::new();
    // Unique obsolete Todo/composer chrome that must never enter scrollback.
    render_and_process(
        &mut inline,
        &mut screen,
        &TerminalFrame::new(
            Vec::new(),
            vec![
                "obsolete-todo-sentinel".to_owned(),
                "obsolete-todo-body".to_owned(),
                "obsolete-composer-sentinel".to_owned(),
            ],
            None,
        ),
        &mut output,
    );

    // Height resize through both the vt100 harness and InlineTerminal so the
    // absolute geometry generation advances the way a real reflow does.
    resize_vt100(&mut screen, 8, width);
    inline.resize_for_test(width, 8);
    render_and_process(
        &mut inline,
        &mut screen,
        &TerminalFrame::new(
            Vec::new(),
            vec![
                "obsolete-todo-sentinel".to_owned(),
                "obsolete-composer-sentinel".to_owned(),
            ],
            None,
        ),
        &mut output,
    );

    // Commit finalized history one block at a time (realistic ack cadence) so
    // the protected history region must scroll repeatedly without ever moving
    // the mutable live chrome.
    let mut pane = TranscriptPane::new(usize::from(width), 8);
    let committed = (0..12)
        .map(|index| format!("committed-history-sentinel-{index:02}"))
        .collect::<Vec<_>>();
    let current_live = vec![
        "current-todo-sentinel".to_owned(),
        "current-composer-sentinel".to_owned(),
    ];
    for line in &committed {
        pane.push_status(line);
        let update = pane.render_terminal_update(usize::from(width), 8);
        render_and_process(
            &mut inline,
            &mut screen,
            &TerminalFrame::new(update.history.clone(), current_live.clone(), None),
            &mut output,
        );
        pane.acknowledge_history(&update.history);
    }

    let retained = all_terminal_rows(&mut screen);
    let output_text = String::from_utf8_lossy(&output);
    assert!(
        retained.iter().all(|row| {
            !row.contains("obsolete-todo-sentinel") && !row.contains("obsolete-composer-sentinel")
        }),
        "obsolete live chrome entered native scrollback: {retained:#?}"
    );
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("current-composer-sentinel"))
            .count(),
        1,
        "current composer must appear exactly once: {retained:#?}"
    );
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("current-todo-sentinel"))
            .count(),
        1,
        "current todo must appear exactly once: {retained:#?}"
    );
    // Deep pre-Neo shell rows were scrolled by ordinary full-screen newlines
    // during seeding; they must still be present after Neo history commits.
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains(shell_first))
            .count(),
        1,
        "first shell sentinel must remain exactly once: {retained:#?}"
    );
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains(shell_last))
            .count(),
        1,
        "last shell sentinel must remain exactly once: {retained:#?}"
    );
    for line in &committed {
        assert_eq!(
            retained
                .iter()
                .filter(|row| row.contains(line.as_str()))
                .count(),
            1,
            "committed history sentinel must remain exactly once ({line}): {retained:#?}"
        );
    }
    assert!(
        output_text.contains("\x1b[1;") && output_text.contains('r'),
        "protected history insert must set a DECSTBM region: missing in output"
    );
    assert!(
        output_text.contains("\x1b[r"),
        "scroll region must be reset after protected history insert"
    );
    assert!(!output_text.contains("\x1b[2J") && !output_text.contains("\x1b[3J"));
    let first_history = retained
        .iter()
        .position(|row| row.contains(&committed[0]))
        .expect("first committed history present");
    let last_history = retained
        .iter()
        .position(|row| row.contains(committed.last().expect("committed non-empty")))
        .expect("last committed history present on surface");
    let todo_row = retained
        .iter()
        .position(|row| row.contains("current-todo-sentinel"))
        .expect("current todo present");
    let composer_row = retained
        .iter()
        .position(|row| row.contains("current-composer-sentinel"))
        .expect("current composer present");
    assert!(
        first_history < last_history && last_history < todo_row && todo_row < composer_row,
        "history then current live chrome order: {retained:#?}"
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
    let mut inline = InlineTerminal::for_test_with_cursor(80, 12, 0, 11);
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
    let (cursor_row, cursor_col) = screen.screen().cursor_position();
    inline
        .resume(80, 12, cursor_col, cursor_row, 1)
        .expect("resume terminal modes");
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
    let mut inline = InlineTerminal::for_test_with_cursor(80, 12, 0, 11);
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
    assert_eq!(
        usize::from(screen.screen().cursor_position().0),
        final_row + 1,
        "exit cursor must sit directly below finalized output"
    );
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
    let mut inline = InlineTerminal::for_test_with_cursor(80, 12, 0, 11);
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

    pane.apply_agent_event(AgentEvent::MessageStarted {
        turn: 1,
        id: "msg-lifecycle".to_owned(),
    });
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

        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::MessageFinished {
        turn: 1,
        id: "msg-lifecycle".to_owned(),
        stop_reason: StopReason::EndTurn,
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

        workflow_origin: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "review-tool".to_owned(),
        name: "Read".to_owned(),
        result: ToolResult::ok("review-committed-tool-result"),

        workflow_origin: None,
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
    // When collapsed (preview mode), short results still show within RESULT_PREVIEW_LINES.
    assert!(
        collapsed_rows
            .iter()
            .any(|row| row.contains("review-committed-tool-result")),
        "collapsed review should show result preview: {collapsed_rows:#?}"
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
    // Match the tool card header only — the result body also embeds the same
    // path sentinel and must not be double-counted as a duplicate card.
    assert_eq!(
        primary_after_review_rows
            .iter()
            .filter(|row| row.contains("Used Read (review-committed-tool)"))
            .count(),
        1,
        "committed tool must remain exactly once in native scrollback: {primary_after_review_rows:#?}"
    );
}

#[test]
fn native_scrollback_keeps_shell_and_progressive_history_exactly_once() {
    let height = 10u16;
    let width = 80u16;
    let mut screen = vt100::Parser::new(height, width, 512);
    let shell_first = "shell-overflow-first-sentinel";
    let shell_last = "shell-overflow-last-sentinel";
    screen.process(format!("{shell_first}\r\n").as_bytes());
    for row in 1..20 {
        screen.process(format!("shell-overflow-row-{row:02}\r\n").as_bytes());
    }
    screen.process(format!("{shell_last}\r\n").as_bytes());

    let mut inline = InlineTerminal::for_test_with_cursor(width, height, 0, height - 1);
    let mut output = Vec::new();

    let chrome = NeoChromeState::new("neo", "session", "model", ".");
    let mut transcript = TranscriptPane::new(usize::from(width), usize::from(height));
    transcript.push_status("pre-overflow-history-sentinel");
    let mut tui = NeoTui::new(chrome, transcript);

    let primary = tui.render_terminal_frame(usize::from(width), usize::from(height));
    render_and_process(&mut inline, &mut screen, &primary, &mut output);
    tui.acknowledge_history(&primary);

    // A tall live workload stays on the normal screen while the shell launch
    // line and committed history remain in native scrollback.
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "overflow-live".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({ "command": "overflow-live-command" }),

            workflow_origin: None,
        });
    let live_body = (0..30)
        .map(|index| format!("overflow-live-sentinel-{index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionUpdate {
            turn: 1,
            id: "overflow-live".to_owned(),
            name: "Bash".to_owned(),
            partial_result: ToolResult::ok(live_body),

            workflow_origin: None,
        });
    tui.transcript_mut()
        .push_status("deferred-alpha-overflow-sentinel");

    let tall = tui.render_terminal_frame(usize::from(width), usize::from(height));
    assert!(
        !tall.review_surface,
        "tall live transcript must stay on the normal screen"
    );
    assert!(!tall.mouse_capture);
    render_and_process(&mut inline, &mut screen, &tall, &mut output);
    tui.acknowledge_history(&tall);

    // The tool completes; its canonical card and the later status commit once.
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "overflow-live".to_owned(),
            name: "Bash".to_owned(),
            result: ToolResult::ok("overflow-live-finished"),

            workflow_origin: None,
        });
    let released = tui.render_terminal_frame(usize::from(width), usize::from(height));
    assert!(!released.review_surface);
    render_and_process(&mut inline, &mut screen, &released, &mut output);
    tui.acknowledge_history(&released);

    let retained = all_terminal_rows(&mut screen);
    let output_text = String::from_utf8_lossy(&output);

    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains(shell_first))
            .count(),
        1,
        "shell first must remain once: {retained:#?}"
    );
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains(shell_last))
            .count(),
        1,
        "shell last must remain once: {retained:#?}"
    );
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("pre-overflow-history-sentinel"))
            .count(),
        1,
        "pre-overflow history must remain once: {retained:#?}"
    );
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("deferred-alpha-overflow-sentinel"))
            .count(),
        1,
        "later stable history must append exactly once: {retained:#?}"
    );
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("overflow-live-command"))
            .count(),
        1,
        "the canonical tool card must commit exactly once: {retained:#?}"
    );
    // Live tool body rows must not leak into native scrollback as duplicates.
    for index in 0..30 {
        let needle = format!("overflow-live-sentinel-{index:02}");
        let count = retained.iter().filter(|row| row.contains(&needle)).count();
        assert!(
            count <= 1,
            "live sentinel duplicated after commit ({needle} x{count}): {retained:#?}"
        );
    }

    assert_eq!(
        output_text.matches("?1049h").count(),
        0,
        "ordinary conversation must never enter the alternate screen: {output_text}"
    );
    assert_eq!(
        output_text.matches("?1049l").count(),
        0,
        "ordinary conversation must never leave the alternate screen: {output_text}"
    );
    assert!(!output_text.contains("\x1b[2J") && !output_text.contains("\x1b[3J"));

    let pre = retained
        .iter()
        .position(|row| row.contains("pre-overflow-history-sentinel"))
        .expect("pre-overflow history present");
    let deferred = retained
        .iter()
        .position(|row| row.contains("deferred-alpha-overflow-sentinel"))
        .expect("deferred history present");
    let tool = retained
        .iter()
        .position(|row| row.contains("overflow-live-command"))
        .expect("tool card present");
    assert!(
        pre < deferred && deferred < tool,
        "history order broken: {retained:#?}"
    );
}

#[test]
fn review_acknowledgement_does_not_advance_normal_history_ledger() {
    let chrome = NeoChromeState::new("neo", "session", "model", ".");
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "review-ack-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),

        workflow_origin: None,
    });
    transcript.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "review-ack-tool".to_owned(),
        name: "Read".to_owned(),
        result: ToolResult::ok("contents"),

        workflow_origin: None,
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
    inline.resize_for_test(cols, rows);
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

/// Tall Delegate, workflow, and approval content stays on the normal screen
/// with terminal-owned scrolling: no alternate-screen enter, no mouse
/// capture, stable facts exactly once, and no duplicate final card.
#[test]
fn delegate_workflow_approval_live_content_stays_on_normal_screen_without_capture() {
    use neo_agent_core::multi_agent::{
        AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
        AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentToolActivityPhase, DelegateContext,
    };
    use neo_agent_core::workflow::{WorkflowId, WorkflowSnapshot, WorkflowState};
    use neo_agent_core::{
        ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
        PermissionOperation,
    };

    let height = 12u16;
    let width = 80u16;
    let mut screen = vt100::Parser::new(height, width, 512);
    screen.process(b"shell-delegate-launch-line\r\n");
    let mut inline = InlineTerminal::for_test_with_cursor(width, height, 0, height - 1);
    let mut output = Vec::new();

    let chrome = NeoChromeState::new("neo", "session", "model", ".");
    let mut transcript = TranscriptPane::new(usize::from(width), usize::from(height));
    transcript.push_status("pre-live-sentinel");
    let mut tui = NeoTui::new(chrome, transcript);
    let primary = tui.render_terminal_frame(usize::from(width), usize::from(height));
    render_and_process(&mut inline, &mut screen, &primary, &mut output);
    tui.acknowledge_history(&primary);

    // A running Delegate with one completed tool, a running workflow, and a
    // pending approval arrive while the turn is live.
    let done_tool = AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "read-1".to_owned(),
            name: "Read".to_owned(),
            summary: Some("one.rs".to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
            files: Vec::new(),
        },
    };
    let agent = AgentSnapshot {
        id: AgentId::from_suffix_for_test("agent-a"),
        display_name: AgentDisplayName::new("agent-a"),
        path: AgentPath::root_child(&AgentDisplayName::new("agent-a")),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: "delegate task".to_owned(),
        task_title: "delegate task".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: false,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 1,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: std::time::Duration::ZERO,
        latest_text: None,
        activity: vec![done_tool],
        prior_messages: Vec::new(),
        outcome: None,
    };
    tui.transcript_mut()
        .transcript_mut()
        .upsert_delegate(1, agent);
    tui.transcript_mut()
        .transcript_mut()
        .upsert_workflow(WorkflowSnapshot {
            id: WorkflowId("wf-1".to_owned()),
            title: "delegate workflow".to_owned(),
            state: WorkflowState::Running,
            current_phase: Some("verify".to_owned()),
            projection_sequence: Some(1),
            recovery_failure: false,
            started_at_ms: Some(1_000),
            updated_at_ms: Some(2_000),
            invocation_count: 1,
            failure_count: 0,
            actual_usage: None,
            latest_log_summary: None,
            latest_report_summary: None,
            terminal_reason: None,
            display_name: "delegate workflow".to_owned(),
            purpose: "test".to_owned(),
        });
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::ApprovalRequested {
            request: ApprovalRequest {
                turn: 1,
                id: "approval-1".to_owned(),
                operation: PermissionOperation::Shell,
                presentation: ApprovalPresentation::Tool {
                    title: "Run tests?".to_owned(),
                    details: vec!["cargo test".to_owned()],
                },
                options: vec![ApprovalOption {
                    action: ApprovalAction::PermitOnce,
                    label: "Allow once".to_owned(),
                    description: None,
                }],
                workflow_origin: None,
            },
        });

    let live = tui.render_terminal_frame(usize::from(width), usize::from(height));
    assert!(
        !live.review_surface,
        "delegate/workflow/approval content must stay on the normal screen"
    );
    assert!(!live.mouse_capture, "no automatic mouse capture");
    render_and_process(&mut inline, &mut screen, &live, &mut output);
    tui.acknowledge_history(&live);

    // Complete the delegate and workflow and resolve the approval.
    let mut completed = tui
        .transcript_mut()
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::Delegate { component } => Some(component.snapshot().clone()),
            _ => None,
        })
        .expect("delegate snapshot");
    completed.state = AgentLifecycleState::Completed;
    completed.terminal_at_ms = Some(3);
    completed.updated_at_ms = 3;
    completed.outcome = Some(neo_agent_core::multi_agent::AgentTerminalOutcome {
        summary: "delegate done".to_owned(),
        is_error: false,
    });
    tui.transcript_mut()
        .transcript_mut()
        .upsert_delegate(1, completed);
    let finished_workflow = WorkflowSnapshot {
        id: WorkflowId("wf-1".to_owned()),
        title: "delegate workflow".to_owned(),
        state: WorkflowState::Completed,
        current_phase: Some("verify".to_owned()),
        projection_sequence: Some(9),
        recovery_failure: false,
        started_at_ms: Some(1_000),
        updated_at_ms: Some(9_000),
        invocation_count: 1,
        failure_count: 0,
        actual_usage: None,
        latest_log_summary: None,
        latest_report_summary: None,
        terminal_reason: Some("workflow completed".to_owned()),
        display_name: "delegate workflow".to_owned(),
        purpose: "test".to_owned(),
    };
    tui.transcript_mut()
        .transcript_mut()
        .upsert_workflow(finished_workflow);
    tui.transcript_mut().resolve_approval(
        "approval-1",
        &ApprovalResolution::Selected {
            action: ApprovalAction::PermitOnce,
            label: "Allow once".to_owned(),
            feedback: None,
        },
    );

    let final_frame = tui.render_terminal_frame(usize::from(width), usize::from(height));
    assert!(!final_frame.review_surface);
    render_and_process(&mut inline, &mut screen, &final_frame, &mut output);
    tui.acknowledge_history(&final_frame);

    let retained = all_terminal_rows(&mut screen);
    let output_text = String::from_utf8_lossy(&output);

    // Ordinary conversation never enters the alternate screen and never
    // captures the mouse.
    assert_eq!(
        output_text.matches("?1049h").count(),
        0,
        "no automatic alternate-screen enter: {output_text}"
    );
    assert!(
        !output_text.contains("?1000h") && !output_text.contains("?1002h"),
        "no automatic mouse capture: {output_text}"
    );

    // The shell launch line stays in native scrollback.
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("shell-delegate-launch-line"))
            .count(),
        1,
        "shell launch line must remain once: {retained:#?}"
    );

    // The delegate tool fact commits exactly once; no complete duplicate card
    // repeats it afterwards.
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("Used Read"))
            .count(),
        1,
        "delegate tool fact must appear exactly once: {retained:#?}"
    );
    assert!(
        retained.iter().any(|row| row.contains("delegate done")),
        "delegate terminal status missing: {retained:#?}"
    );

    // Non-terminal workflow states stay live; only the terminal group enters
    // native scrollback.
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("delegate workflow") && row.contains("running"))
            .count(),
        0,
        "non-terminal workflow state must not enter history: {retained:#?}"
    );
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("delegate workflow") && row.contains("completed"))
            .count(),
        1,
        "workflow terminal outcome must appear once: {retained:#?}"
    );

    // The resolved approval commits as one terminal fact.
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("approval: Allow once"))
            .count(),
        1,
        "resolved approval must appear once: {retained:#?}"
    );
    assert!(
        retained
            .iter()
            .all(|row| !row.contains("earlier rows omitted")),
        "no presentation-level omission: {retained:#?}"
    );
}

#[test]
fn workflow_group_progress_preserves_bottom_region_and_native_history() {
    use neo_agent_core::multi_agent::AgentLifecycleState;
    use neo_agent_core::workflow::WorkflowState;
    use neo_agent_core::{
        ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
        PermissionOperation,
    };
    use neo_tui::dialogs::{QuestionDisplayData, QuestionDisplayOption};
    use neo_tui::shell::StreamUpdate;
    use neo_tui::transcript::{BlockingEntryKind, TranscriptBlockId};
    use neo_tui::widgets::{TodoDisplayItem, TodoDisplayStatus};

    let sizes = [(72u16, 18u16), (96, 22), (120, 26)];
    let (initial_width, initial_height) = sizes[0];
    let mut screen = vt100::Parser::new(initial_height, initial_width, 1024);
    screen.process(b"workflow-shell-sentinel\r\n");
    let mut inline =
        InlineTerminal::for_test_with_cursor(initial_width, initial_height, 0, initial_height - 1);
    let mut output = Vec::new();

    let mut chrome = NeoChromeState::new("neo", "session", "model", ".");
    chrome.set_todo_items(vec![TodoDisplayItem::new(
        "todo-sentinel",
        TodoDisplayStatus::InProgress,
    )]);
    chrome.set_custom_working_label(Some("footer-sentinel".to_owned()));
    chrome.prompt_mut().text = "composer-sentinel".to_owned();
    chrome.prompt_mut().cursor = chrome.prompt().text.chars().count();
    let transcript = TranscriptPane::new(usize::from(initial_width), usize::from(initial_height));
    let mut tui = NeoTui::new(chrome, transcript);

    tui.transcript_mut()
        .apply_agent_event(AgentEvent::WorkflowStarted {
            turn: 1,
            workflow: scrollback_workflow_snapshot(WorkflowState::Running, 1),
        });

    let read_origin = scrollback_workflow_origin("read-call");
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "read-call".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({"path": "workflow-source-sentinel.rs"}),
            workflow_origin: Some(read_origin.clone()),
        });
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionUpdate {
            turn: 1,
            id: "read-call".to_owned(),
            name: "Read".to_owned(),
            partial_result: ToolResult::ok("workflow-tool-progress-sentinel"),
            workflow_origin: Some(read_origin.clone()),
        });
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "read-call".to_owned(),
            name: "Read".to_owned(),
            result: ToolResult::ok("workflow-tool-final-sentinel"),
            workflow_origin: Some(read_origin),
        });

    let delegate_origin = scrollback_workflow_origin("delegate-call");
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "delegate-call".to_owned(),
            name: "Delegate".to_owned(),
            arguments: serde_json::json!({"task": "delegate-sentinel"}),
            workflow_origin: Some(delegate_origin.clone()),
        });
    let delegate = scrollback_agent_snapshot("delegate-sentinel");
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::DelegateStarted {
            turn: 1,
            agent: delegate.clone(),
            workflow_origin: Some(delegate_origin.clone()),
        });

    let swarm_origin = scrollback_workflow_origin("swarm-call");
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "swarm-call".to_owned(),
            name: "DelegateSwarm".to_owned(),
            arguments: serde_json::json!({"tasks": ["swarm-child-sentinel"]}),
            workflow_origin: Some(swarm_origin.clone()),
        });
    let swarm = scrollback_swarm_snapshot(
        "swarm-sentinel",
        scrollback_agent_snapshot("swarm-child-sentinel"),
    );
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::DelegateSwarmStarted {
            turn: 1,
            swarm: swarm.clone(),
            workflow_origin: Some(swarm_origin.clone()),
        });
    tui.transcript_mut()
        .push_status("unrelated-history-sentinel");

    for (index, (width, height)) in sizes.into_iter().enumerate() {
        let mut updated_delegate = delegate.clone();
        updated_delegate.updated_at_ms = 10 + index as u64;
        updated_delegate.latest_text = Some(format!("delegate-progress-{index}"));
        tui.transcript_mut()
            .apply_agent_event(AgentEvent::DelegateUpdated {
                turn: 1,
                agent: updated_delegate,
                workflow_origin: Some(delegate_origin.clone()),
            });

        let mut updated_swarm = swarm.clone();
        updated_swarm.children[0].agent.updated_at_ms = 10 + index as u64;
        updated_swarm.children[0].agent.latest_text = Some(format!("swarm-progress-{index}"));
        tui.transcript_mut()
            .apply_agent_event(AgentEvent::DelegateSwarmUpdated {
                turn: 1,
                swarm: updated_swarm,
                workflow_origin: Some(swarm_origin.clone()),
            });
        tui.transcript_mut()
            .apply_agent_event(AgentEvent::WorkflowUpdated {
                turn: 1,
                workflow: scrollback_workflow_snapshot(
                    WorkflowState::Running,
                    u64::try_from(index).expect("small index") + 2,
                ),
            });

        screen.screen_mut().set_size(height, width);
        inline.resize_for_test(width, height);
        let frame = tui.render_terminal_frame(usize::from(width), usize::from(height));
        assert_workflow_frame_geometry(&frame, usize::from(width), usize::from(height));
        let live = frame
            .live
            .iter()
            .map(|line| strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            live.contains("workflow-frame-sentinel"),
            "workflow group missing at {width}x{height}: {live}"
        );
        assert!(
            frame
                .history
                .iter()
                .all(|block| !matches!(block.id, TranscriptBlockId::Workflow { .. })),
            "non-terminal workflow entered history"
        );
        if index == 0 {
            assert!(frame.history.iter().any(|block| {
                block
                    .lines
                    .iter()
                    .any(|line| strip_ansi(line).contains("unrelated-history-sentinel"))
            }));
        }
        render_and_process(&mut inline, &mut screen, &frame, &mut output);
        tui.acknowledge_history(&frame);
    }

    let approval_origin = scrollback_workflow_origin("approval-call");
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::ApprovalRequested {
            request: ApprovalRequest {
                turn: 1,
                id: "approval-sentinel".to_owned(),
                operation: PermissionOperation::Shell,
                presentation: ApprovalPresentation::Tool {
                    title: "approval-input-sentinel".to_owned(),
                    details: vec!["cargo test".to_owned()],
                },
                options: vec![ApprovalOption {
                    action: ApprovalAction::PermitOnce,
                    label: "Allow once".to_owned(),
                    description: None,
                }],
                workflow_origin: Some(approval_origin),
            },
        });
    let question_origin = scrollback_workflow_origin("question-call");
    tui.transcript_mut()
        .apply_question_stream_update(StreamUpdate::QuestionRequested {
            id: "question-sentinel".to_owned(),
            questions: vec![QuestionDisplayData {
                question: "question-input-sentinel".to_owned(),
                header: None,
                body: None,
                options: vec![QuestionDisplayOption {
                    label: "Continue".to_owned(),
                    description: Some(String::new()),
                }],
                multi_select: false,
            }],
            workflow_origin: Some(question_origin.clone()),
        });
    assert!(matches!(
        tui.transcript().earliest_blocking_entry(),
        Some(BlockingEntryKind::Approval(id)) if id == "approval-sentinel"
    ));
    assert!(tui.transcript().transcript().entries().iter().any(
        |entry| matches!(entry, TranscriptEntry::QuestionPrompt(data)
            if data.workflow_origin.as_ref() == Some(&question_origin))
    ));

    let (barrier_width, barrier_height) = sizes[1];
    screen.screen_mut().set_size(barrier_height, barrier_width);
    inline.resize_for_test(barrier_width, barrier_height);
    let approval_frame =
        tui.render_terminal_frame(usize::from(barrier_width), usize::from(barrier_height));
    assert_workflow_frame_geometry(
        &approval_frame,
        usize::from(barrier_width),
        usize::from(barrier_height),
    );
    render_and_process(&mut inline, &mut screen, &approval_frame, &mut output);
    tui.acknowledge_history(&approval_frame);

    tui.transcript_mut().resolve_approval(
        "approval-sentinel",
        &ApprovalResolution::Selected {
            action: ApprovalAction::PermitOnce,
            label: "Allow once".to_owned(),
            feedback: None,
        },
    );
    assert!(matches!(
        tui.transcript().earliest_blocking_entry(),
        Some(BlockingEntryKind::Question(id)) if id == "question-sentinel"
    ));

    let (final_width, final_height) = sizes[2];
    screen.screen_mut().set_size(final_height, final_width);
    inline.resize_for_test(final_width, final_height);
    let question_frame =
        tui.render_terminal_frame(usize::from(final_width), usize::from(final_height));
    let bottom_offsets = assert_workflow_frame_geometry(
        &question_frame,
        usize::from(final_width),
        usize::from(final_height),
    );
    render_and_process(&mut inline, &mut screen, &question_frame, &mut output);
    tui.acknowledge_history(&question_frame);

    let completed_delegate = scrollback_completed_agent(delegate, "delegate-final-sentinel");
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::DelegateFinished {
            turn: 1,
            agent: completed_delegate,
            workflow_origin: Some(delegate_origin),
        });
    let mut completed_swarm = swarm;
    completed_swarm.children[0].agent = scrollback_completed_agent(
        completed_swarm.children[0].agent.clone(),
        "swarm-final-sentinel",
    );
    completed_swarm.state = AgentLifecycleState::Completed;
    completed_swarm.aggregate =
        neo_agent_core::multi_agent::SwarmAggregate::from_states([AgentLifecycleState::Completed]);
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::DelegateSwarmFinished {
            turn: 1,
            swarm: completed_swarm,
            workflow_origin: Some(swarm_origin),
        });
    tui.transcript_mut()
        .apply_agent_event(AgentEvent::WorkflowFinished {
            turn: 1,
            workflow: scrollback_workflow_snapshot(WorkflowState::Completed, 99),
        });

    let final_frame =
        tui.render_terminal_frame(usize::from(final_width), usize::from(final_height));
    assert_eq!(
        assert_workflow_frame_geometry(
            &final_frame,
            usize::from(final_width),
            usize::from(final_height),
        ),
        bottom_offsets,
        "bottom region moved when the workflow committed"
    );
    let workflow_blocks = final_frame
        .history
        .iter()
        .filter(|block| matches!(block.id, TranscriptBlockId::Workflow { .. }))
        .collect::<Vec<_>>();
    assert_eq!(workflow_blocks.len(), 1, "one terminal workflow group");
    let terminal_group = workflow_blocks[0]
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    for sentinel in [
        "workflow-frame-sentinel",
        "workflow-source-sentinel.rs",
        "delegate-sentinel",
        "swarm-child-sentinel",
    ] {
        assert!(
            terminal_group.contains(sentinel),
            "terminal group missing {sentinel}: {terminal_group}"
        );
    }
    assert!(
        final_frame
            .live
            .iter()
            .all(|line| !strip_ansi(line).contains("workflow-frame-sentinel")),
        "workflow remained live while final group was offered"
    );
    render_and_process(&mut inline, &mut screen, &final_frame, &mut output);
    tui.acknowledge_history(&final_frame);

    let retry = tui.render_terminal_frame(usize::from(final_width), usize::from(final_height));
    assert!(
        retry
            .history
            .iter()
            .all(|block| !matches!(block.id, TranscriptBlockId::Workflow { .. })),
        "acknowledged workflow group replayed"
    );
    assert_eq!(
        assert_workflow_frame_geometry(&retry, usize::from(final_width), usize::from(final_height),),
        bottom_offsets
    );

    let retained = all_terminal_rows(&mut screen);
    let unrelated = retained
        .iter()
        .position(|row| row.contains("unrelated-history-sentinel"))
        .expect("unrelated history present");
    let workflow = retained
        .iter()
        .position(|row| row.contains("workflow-frame-sentinel"))
        .expect("terminal workflow present");
    assert!(
        unrelated < workflow,
        "terminal history order: {retained:#?}"
    );
    assert_eq!(
        retained
            .iter()
            .filter(|row| row.contains("workflow-frame-sentinel"))
            .count(),
        1,
        "terminal workflow group duplicated: {retained:#?}"
    );
    let output_text = String::from_utf8_lossy(&output);
    assert!(!output_text.contains("?1049h") && !output_text.contains("?1049l"));
    assert!(!output_text.contains("?1000h") && !output_text.contains("?1002h"));
    assert!(!output_text.contains("\x1b[2J") && !output_text.contains("\x1b[3J"));
}

#[test]
fn restored_terminal_workflow_history_is_bounded_before_final_assistant_message() {
    use neo_agent_core::workflow::WorkflowState;
    use neo_tui::transcript::TranscriptBlockId;

    let width = 88usize;
    let height = 12usize;
    let mut pane = TranscriptPane::new(width, height);
    pane.set_live_chrome_height(0);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: scrollback_workflow_snapshot(WorkflowState::Running, 1),
    });

    for index in 0..24 {
        let id = format!("restored-bash-{index}");
        let origin = scrollback_workflow_origin(&id);
        pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: id.clone(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({
                "command": format!("cargo test restored-terminal-tool-{index}")
            }),
            workflow_origin: Some(origin.clone()),
        });
        pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id,
            name: "Bash".to_owned(),
            result: ToolResult::ok(format!("restored-terminal-tool-{index}-done")),
            workflow_origin: Some(origin),
        });
    }

    for index in 0..16 {
        let call_id = format!("restored-delegate-call-{index}");
        let origin = scrollback_workflow_origin(&call_id);
        pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: call_id,
            name: "Delegate".to_owned(),
            arguments: serde_json::json!({"task": format!("restored-delegate-{index}")}),
            workflow_origin: Some(origin.clone()),
        });
        let agent = scrollback_agent_snapshot(&format!("restored-delegate-{index}"));
        pane.apply_agent_event(AgentEvent::DelegateStarted {
            turn: 1,
            agent: agent.clone(),
            workflow_origin: Some(origin.clone()),
        });
        pane.apply_agent_event(AgentEvent::DelegateFinished {
            turn: 1,
            agent: scrollback_completed_agent(agent, "restored delegate complete"),
            workflow_origin: Some(origin),
        });
    }

    pane.apply_agent_event(AgentEvent::WorkflowFinished {
        turn: 1,
        workflow: scrollback_workflow_snapshot(WorkflowState::Completed, 99),
    });
    pane.replay_assistant_message("restored-final-assistant-sentinel");

    let full_workflow = pane
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::Workflow { component } => {
                Some(component.render_with_theme(width, &Default::default()))
            }
            _ => None,
        })
        .expect("restored workflow entry");
    let full_workflow_text = full_workflow
        .iter()
        .map(|line| line.text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        full_workflow.len() > height,
        "full workflow view must remain larger than the ordinary history budget"
    );
    assert!(
        full_workflow_text.matches("Used Bash").count() == 24
            && full_workflow_text.contains("restored-delegate-15"),
        "full workflow view lost terminal activity: {full_workflow_text}"
    );

    let update = pane.render_terminal_update(width, height);
    let workflow_index = update
        .history
        .iter()
        .position(|block| matches!(block.id, TranscriptBlockId::Workflow { .. }))
        .expect("restored terminal workflow history");
    let assistant_index = update
        .history
        .iter()
        .position(|block| {
            block
                .lines
                .iter()
                .any(|line| strip_ansi(line).contains("restored-final-assistant-sentinel"))
        })
        .expect("restored final assistant history");
    assert!(
        workflow_index < assistant_index,
        "restored history order: {:#?}",
        update.history
    );
    let workflow = &update.history[workflow_index];
    assert!(
        workflow.lines.len() + usize::from(workflow.separator_before) <= height,
        "terminal workflow history exceeded the ordinary terminal budget: {workflow:#?}"
    );
    let workflow_text = workflow
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        workflow_text.contains("direct tools omitted"),
        "large terminal tool history was not summarized: {workflow_text}"
    );
    assert!(
        workflow_text.contains("agents omitted"),
        "large terminal delegate history was not summarized: {workflow_text}"
    );

    let mut screen = vt100::Parser::new(
        u16::try_from(height).expect("test height fits u16"),
        u16::try_from(width).expect("test width fits u16"),
        512,
    );
    let mut inline = InlineTerminal::for_test(
        u16::try_from(width).expect("test width fits u16"),
        u16::try_from(height).expect("test height fits u16"),
    );
    let mut output = Vec::new();
    render_and_process(
        &mut inline,
        &mut screen,
        &TerminalFrame::new(update.history, update.live, None),
        &mut output,
    );
    let rows = all_terminal_rows(&mut screen);
    let workflow_row = rows
        .iter()
        .position(|row| row.contains("workflow-frame-sentinel"))
        .expect("workflow row in native history");
    let assistant_row = rows
        .iter()
        .position(|row| row.contains("restored-final-assistant-sentinel"))
        .expect("final assistant row in native history");
    assert!(
        workflow_row < assistant_row,
        "restored workflow rows appeared below the final assistant message: {rows:#?}"
    );
    assert!(
        rows.iter().skip(assistant_row + 1).all(|row| {
            !row.contains("workflow-frame-sentinel")
                && !row.contains("restored-terminal-tool")
                && !row.contains("restored-delegate")
        }),
        "restored workflow projection appeared after the final assistant message: {rows:#?}"
    );
}

#[test]
fn terminal_workflow_waits_for_nonzero_history_budget_before_commit() {
    use neo_agent_core::workflow::WorkflowState;
    use neo_tui::transcript::TranscriptBlockId;

    let mut pane = TranscriptPane::new(80, 8);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: scrollback_workflow_snapshot(WorkflowState::Running, 1),
    });
    pane.apply_agent_event(AgentEvent::WorkflowFinished {
        turn: 1,
        workflow: scrollback_workflow_snapshot(WorkflowState::Completed, 2),
    });
    pane.replay_assistant_message("assistant-after-workflow");

    pane.set_live_chrome_height(8);
    let zero_budget = pane.render_terminal_update(80, 8);
    assert!(zero_budget.history.is_empty());
    assert!(zero_budget.live.is_empty());
    pane.acknowledge_history(&zero_budget.history);

    pane.set_live_chrome_height(0);
    let visible = pane.render_terminal_update(80, 8);
    let workflow_blocks = visible
        .history
        .iter()
        .filter(|block| matches!(block.id, TranscriptBlockId::Workflow { .. }))
        .collect::<Vec<_>>();
    assert_eq!(workflow_blocks.len(), 1);
    assert!(!workflow_blocks[0].lines.is_empty());
    let workflow_index = visible
        .history
        .iter()
        .position(|block| matches!(block.id, TranscriptBlockId::Workflow { .. }))
        .expect("workflow block");
    let assistant_index = visible
        .history
        .iter()
        .position(|block| matches!(block.id, TranscriptBlockId::AssistantSegment { .. }))
        .expect("assistant block");
    assert!(
        workflow_index < assistant_index,
        "history order: {visible:#?}"
    );

    pane.acknowledge_history(&visible.history);
    assert!(pane.render_terminal_update(80, 8).history.is_empty());
}

fn assert_workflow_frame_geometry(
    frame: &TerminalFrame,
    width: usize,
    height: usize,
) -> Vec<usize> {
    use neo_tui::primitive::visible_width;

    assert!(
        !frame.review_surface,
        "workflow progress left the normal screen"
    );
    assert!(!frame.mouse_capture, "workflow progress captured the mouse");
    assert!(
        frame.live.len() <= height,
        "frame height: {}",
        frame.live.len()
    );
    assert!(
        frame.live.iter().all(|line| visible_width(line) <= width),
        "frame width overflow: {:?}",
        frame.live
    );
    let cursor = frame.cursor.expect("composer cursor remains visible");
    assert!(cursor.row < frame.live.len() && cursor.row < height);
    assert!(cursor.col < width);

    let plain = frame
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>();
    ["todo-sentinel", "composer-sentinel", "footer-sentinel"]
        .into_iter()
        .map(|sentinel| {
            let positions = plain
                .iter()
                .enumerate()
                .filter_map(|(index, line)| line.contains(sentinel).then_some(index))
                .collect::<Vec<_>>();
            assert_eq!(positions.len(), 1, "bottom sentinel {sentinel}: {plain:#?}");
            plain.len() - positions[0]
        })
        .collect()
}

fn scrollback_workflow_snapshot(
    state: neo_agent_core::workflow::WorkflowState,
    sequence: u64,
) -> neo_agent_core::workflow::WorkflowSnapshot {
    neo_agent_core::workflow::WorkflowSnapshot {
        id: neo_agent_core::workflow::WorkflowId("workflow-frame".to_owned()),
        title: "workflow-frame-sentinel".to_owned(),
        state,
        current_phase: Some(format!("phase-{sequence}")),
        projection_sequence: Some(sequence),
        recovery_failure: false,
        started_at_ms: Some(1_000),
        updated_at_ms: Some(1_000 + sequence),
        invocation_count: 3,
        failure_count: 0,
        actual_usage: None,
        latest_log_summary: Some(format!("workflow-log-{sequence}")),
        latest_report_summary: Some(format!("workflow-report-{sequence}")),
        terminal_reason: state.is_terminal().then(|| "workflow complete".to_owned()),
        display_name: "workflow-frame-sentinel".to_owned(),
        purpose: "terminal geometry regression".to_owned(),
    }
}

fn scrollback_workflow_origin(
    invocation_id: &str,
) -> neo_agent_core::workflow::WorkflowExecutionOrigin {
    neo_agent_core::workflow::WorkflowExecutionOrigin {
        run_id: neo_agent_core::workflow::WorkflowId("workflow-frame".to_owned()),
        human_handle: None,
        definition_name: "workflow-frame".to_owned(),
        definition_revision: None,
        phase_id: Some("verify".to_owned()),
        invocation_id: Some(invocation_id.to_owned()),
        swarm_item_id: None,
    }
}

fn scrollback_agent_snapshot(id: &str) -> neo_agent_core::multi_agent::AgentSnapshot {
    use neo_agent_core::multi_agent::{
        AgentDisplayName, AgentId, AgentLifecycleState, AgentPath, AgentRole, AgentRunMode,
        DelegateContext,
    };

    let display_name = AgentDisplayName::new(id);
    neo_agent_core::multi_agent::AgentSnapshot {
        id: AgentId::from_suffix_for_test(id),
        display_name: display_name.clone(),
        path: AgentPath::root_child(&display_name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: id.to_owned(),
        task_title: id.to_owned(),
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: false,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 0,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: std::time::Duration::ZERO,
        latest_text: Some(format!("{id}-progress")),
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    }
}

fn scrollback_completed_agent(
    mut agent: neo_agent_core::multi_agent::AgentSnapshot,
    summary: &str,
) -> neo_agent_core::multi_agent::AgentSnapshot {
    agent.state = neo_agent_core::multi_agent::AgentLifecycleState::Completed;
    agent.updated_at_ms += 100;
    agent.terminal_at_ms = Some(agent.updated_at_ms);
    agent.outcome = Some(neo_agent_core::multi_agent::AgentTerminalOutcome {
        summary: summary.to_owned(),
        is_error: false,
    });
    agent
}

fn scrollback_swarm_snapshot(
    id: &str,
    agent: neo_agent_core::multi_agent::AgentSnapshot,
) -> neo_agent_core::multi_agent::SwarmSnapshot {
    use neo_agent_core::multi_agent::{
        AgentRole, AgentRunMode, SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
    };

    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "swarm-child-sentinel".to_owned(),
        agent,
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    SwarmSnapshot {
        swarm_id: id.to_owned(),
        description: "swarm-sentinel".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 1,
        aggregate,
        children,
    }
}
