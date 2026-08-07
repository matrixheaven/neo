use neo_tui::primitive::{strip_ansi, visible_width};
use neo_tui::transcript::TranscriptPane;

#[test]
fn live_budget_truncation_keeps_recent_whole_blocks() {
    let mut pane = TranscriptPane::new(80, 8);
    for (id, path) in [("read-1", "one.rs"), ("read-2", "two.rs")] {
        pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: id.to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({"path": path}),

            workflow_origin: None,
            output_ref: None,
        });
    }
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "read-2".to_owned(),
        name: "Read".to_owned(),
        partial_result: neo_agent_core::ToolResult::ok(
            "output-1\noutput-2\noutput-3\noutput-4\noutput-5\nlatest-output",
        ),

        workflow_origin: None,
        output_ref: None,
    });

    let slice = pane.render_visible_slice(80, 8);
    let live = slice
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        slice.len() <= 8,
        "slice must stay bounded by the terminal height: {}",
        slice.len()
    );
    // The adjacent tool cards render as one grouped block; the newest rows
    // survive instead of a mid-card row slice.
    assert_eq!(live.matches("Using Read").count(), 1, "slice:\n{live}");
    assert!(live.contains("latest-output"), "slice:\n{live}");
    assert!(!live.contains("earlier rows omitted"), "slice:\n{live}");
}

#[test]
fn long_live_tool_output_lines_never_exceed_terminal_width() {
    let mut transcript_pane = TranscriptPane::new(80, 30);
    let long_memory_row = format!(
        "    0.563,01KVG2WP5FW4GXDQK93WZYFTA9,context-neo,high,0.927,\"{}\"",
        "Fixed clippy warnings in crates/neo-tui ".repeat(20)
    );

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({
            "command": "icm recall \"compact\"",
            "cwd": "/Users/chenyuanhao/Workspace/neo"
        }),

        workflow_origin: None,
        output_ref: None,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        partial_result: neo_agent_core::ToolResult::ok(long_memory_row),

        workflow_origin: None,
        output_ref: None,
    });

    let width = 80_u16;
    let frame = transcript_pane
        .render_frame(usize::from(width), 30)
        .expect("render frame");

    for (i, line) in frame.iter().enumerate() {
        let w = visible_width(line);
        assert!(
            w <= usize::from(width),
            "line {i} visible width {w} exceeds terminal width {width}: {}",
            strip_ansi(line)
        );
    }
}

#[test]
fn long_shell_result_lines_never_exceed_terminal_width() {
    let mut transcript_pane = TranscriptPane::new(80, 30);
    let long_memory_row = format!(
        "    0.563,01KVG2WP5FW4GXDQK93WZYFTA9,context-neo,high,0.927,\"{}\"",
        "Fixed clippy warnings in crates/neo-tui ".repeat(20)
    );

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({
            "command": "icm recall \"compact\"",
            "cwd": "/Users/chenyuanhao/Workspace/neo"
        }),

        workflow_origin: None,
        output_ref: None,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok(long_memory_row),

        workflow_origin: None,
        output_ref: None,
    });

    let width = 80_u16;
    let frame = transcript_pane
        .render_frame(usize::from(width), 30)
        .expect("render frame");

    for (i, line) in frame.iter().enumerate() {
        let w = visible_width(line);
        assert!(
            w <= usize::from(width),
            "line {i} visible width {w} exceeds terminal width {width}: {}",
            strip_ansi(line)
        );
    }
}

#[test]
fn long_unstable_assistant_tail_stays_bounded_and_commits_once() {
    let mut pane = TranscriptPane::new(30, 8);
    pane.start_assistant_message();
    pane.append_assistant_delta(&"unfinished ".repeat(80));

    let slice = pane.render_visible_slice(30, 8);
    let live = slice
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        slice.len() <= 8,
        "slice must stay bounded by the terminal height: {}",
        slice.len()
    );
    assert!(live.contains("unfinished"), "slice:\n{live}");

    // Completion keeps the canonical assistant entry in the document.
    pane.finish_assistant_message();
    let finished = pane
        .render_visible_slice(30, 8)
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(finished.contains("unfinished"), "slice:\n{finished}");
}

#[test]
fn long_user_message_with_wide_chars_never_exceeds_terminal_width() {
    // Regression for a width-overflow crash in `bulleted_wrap`: the `✨ `
    // prefix width was not subtracted from the wrap budget, so long CJK
    // prompts produced a first row wider than the terminal and tripped the
    // renderer's width invariant. Keep this test if you touch that path.
    let mut transcript_pane = TranscriptPane::new(40, 30);
    let prompt = "停下来所有提交工作，总结一下你的工作，为什么你之前要用工具来提交？还有就是你用工具时遇到了什么问题？";
    transcript_pane.push_user_message(prompt);
    transcript_pane.mark_dirty();
    let width = 40_u16;
    let frame = transcript_pane
        .render_frame(usize::from(width), 30)
        .expect("render frame");

    for (i, line) in frame.iter().enumerate() {
        let w = visible_width(line);
        assert!(
            w <= usize::from(width),
            "line {i} visible width {w} exceeds terminal width {width}: {}",
            strip_ansi(line)
        );
    }
}
