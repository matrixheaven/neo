//! Phase 6: thinking renders as a fixed 2-line floating window.
//!
//! Streaming thinking shows a `⠋ thinking...` header + the *last* 2 wrapped rows
//! (scrolling tail). Completed thinking shows the *first* 2 rows + a collapse
//! hint when the full text was longer.

use neo_tui::primitive::strip_ansi;
use neo_tui::transcript::TranscriptEntry;
use neo_tui::transcript::TranscriptPane;

fn plain_frame(runtime: &mut TranscriptPane, width: usize, height: usize) -> Vec<String> {
    runtime
        .render_frame(width, height)
        .expect("render frame")
        .iter()
        .map(|line| strip_ansi(line).trim_end().to_owned())
        .collect()
}

#[test]
fn live_thinking_shows_spinner_and_tail_window() {
    let mut runtime = TranscriptPane::new(40, 12);
    runtime.push_transcript(TranscriptEntry::thinking_streaming(
        "alpha\nbeta\ngamma\ndelta\nepsilon",
    ));

    let frame = plain_frame(&mut runtime, 40, 12);
    let joined = frame.join("\n");

    // Live header is the spinner line.
    assert!(
        joined.contains("thinking"),
        "should show thinking label: {joined}"
    );
    // The tail window shows the last 2 lines only.
    assert!(joined.contains("delta"), "tail shows delta: {joined}");
    assert!(joined.contains("epsilon"), "tail shows epsilon: {joined}");
    // Earlier lines are NOT in the live window.
    assert!(
        !joined.contains("alpha"),
        "live window drops head lines: {joined}"
    );
    assert!(
        !joined.contains("beta"),
        "live window drops head lines: {joined}"
    );
}

#[test]
fn live_thinking_spinner_advances_on_explicit_animation_tick() {
    let mut runtime = TranscriptPane::new(40, 12);
    runtime.push_transcript(TranscriptEntry::thinking_streaming("alpha"));

    let first = plain_frame(&mut runtime, 40, 12).join("\n");
    runtime.advance_animation_at_ms(1);
    let second = plain_frame(&mut runtime, 40, 12).join("\n");

    assert!(first.contains("⠋ thinking..."), "first spinner: {first}");
    assert!(second.contains("⠙ thinking..."), "second spinner: {second}");
}

#[test]
fn summary_thinking_shows_latest_title_and_spinner() {
    let mut runtime = TranscriptPane::new(60, 12);
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "summary-1".to_owned(),
        kind: neo_ai::ThinkingKind::Summary,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "**Planning context recall and repo inspection**".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "summary-2".to_owned(),
        kind: neo_ai::ThinkingKind::Summary,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "**Planning parallel recall**".to_owned(),
    });

    let first = plain_frame(&mut runtime, 60, 12).join("\n");
    runtime.advance_animation_at_ms(1);
    let second = plain_frame(&mut runtime, 60, 12).join("\n");

    assert!(
        first.contains("⠋ thinking · Planning parallel recall"),
        "latest summary title: {first}"
    );
    assert!(
        second.contains("⠙ thinking · Planning parallel recall"),
        "summary spinner advances: {second}"
    );
    assert!(
        !first.contains("**"),
        "summary markers stay out of UI: {first}"
    );
}

#[test]
fn summary_thinking_keeps_title_across_empty_active_part() {
    let mut runtime = TranscriptPane::new(80, 16);
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "summary-1".to_owned(),
        kind: neo_ai::ThinkingKind::Summary,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "**Plan**\nfirst body".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "summary-placeholder".to_owned(),
        kind: neo_ai::ThinkingKind::Summary,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "<!-- -->".to_owned(),
    });

    let rows = plain_frame(&mut runtime, 80, 16);
    let frame = rows.join("\n");

    assert_eq!(
        rows.iter().filter(|row| row.contains("thinking")).count(),
        1,
        "Summary streaming stays one spinner row: {rows:?}"
    );
    assert!(
        frame.contains("thinking · Plan"),
        "prior title remains visible: {frame}"
    );
    assert!(
        !frame.contains("first body"),
        "body is not streamed: {frame}"
    );
    assert!(
        !frame.contains("<!-- -->"),
        "placeholder is not streamed: {frame}"
    );
}

#[test]
fn summary_thinking_without_leading_title_uses_generic_spinner() {
    let mut runtime = TranscriptPane::new(60, 12);
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "summary".to_owned(),
        kind: neo_ai::ThinkingKind::Summary,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "body with **inline bold**".to_owned(),
    });

    let frame = plain_frame(&mut runtime, 60, 12).join("\n");

    assert!(
        frame.contains("⠋ thinking..."),
        "generic summary spinner: {frame}"
    );
    assert!(
        !frame.contains("thinking ·"),
        "body-only summary has no title label: {frame}"
    );
    assert!(
        !frame.contains("inline bold"),
        "summary body stays out of streaming scrollback: {frame}"
    );
}

#[test]
fn summary_thinking_preserves_body_after_title() {
    let mut runtime = TranscriptPane::new(80, 16);
    runtime.push_transcript(TranscriptEntry::thinking_complete_with_kind(
        "**Plan**\n- inspect [the repository](https://example.com/repo)\n- preserve the ordered parts",
        neo_ai::ThinkingKind::Summary,
    ));

    assert!(runtime.toggle_tool_output_expanded());
    let frame = plain_frame(&mut runtime, 80, 16).join("\n");

    assert!(frame.contains("● Plan"), "summary title: {frame}");
    assert!(
        frame.contains("- inspect [the repository](https://example.com/repo)"),
        "summary link body: {frame}"
    );
    assert!(
        frame.contains("- preserve the ordered parts"),
        "summary bullet body: {frame}"
    );
}

#[test]
fn summary_thinking_preserves_indented_body_after_title() {
    let mut runtime = TranscriptPane::new(80, 16);
    runtime.push_transcript(TranscriptEntry::thinking_complete_with_kind(
        "**Title**\n    ```\n    let value = 42;\n    ```",
        neo_ai::ThinkingKind::Summary,
    ));

    assert!(runtime.toggle_tool_output_expanded());
    let rows = plain_frame(&mut runtime, 80, 16);

    assert!(
        rows.iter().any(|row| row.ends_with("    ```")),
        "code fence indentation: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.ends_with("    let value = 42;")),
        "code body indentation: {rows:?}"
    );
}

#[test]
fn summary_thinking_keeps_inline_bold_body() {
    let mut runtime = TranscriptPane::new(80, 12);
    runtime.push_transcript(TranscriptEntry::thinking_complete_with_kind(
        "**Plan**\nThe **inline bold** detail remains body text.",
        neo_ai::ThinkingKind::Summary,
    ));

    let frame = plain_frame(&mut runtime, 80, 12).join("\n");

    assert!(frame.contains("● Plan"), "summary title: {frame}");
    assert!(
        frame.contains("The **inline bold** detail remains body text."),
        "inline bold stays in body: {frame}"
    );
    assert!(
        !frame.contains("● inline bold"),
        "inline bold is not promoted to a title: {frame}"
    );
}

#[test]
fn summary_thinking_collapses_adjacent_duplicate_titles() {
    let mut runtime = TranscriptPane::new(80, 16);
    for (id, text) in [
        ("summary-1", "**Plan**\nfirst body"),
        ("summary-2", "**Plan**\nsecond body"),
        ("summary-3", "**Review**\nthird body"),
        ("summary-4", "**Plan**\nfourth body"),
    ] {
        runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
            turn: 1,
            id: id.to_owned(),
            kind: neo_ai::ThinkingKind::Summary,
        });
        runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
            turn: 1,
            text: text.to_owned(),
        });
        runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
            turn: 1,
            signature: None,
            redacted: false,
        });
    }

    assert!(runtime.toggle_tool_output_expanded());
    let frame = plain_frame(&mut runtime, 80, 16).join("\n");

    assert_eq!(
        frame.matches("Plan").count(),
        2,
        "adjacent Plan collapses but non-adjacent Plan remains: {frame}"
    );
    assert_eq!(
        frame.matches("Review").count(),
        1,
        "Review remains distinct: {frame}"
    );
    assert!(frame.contains("first body"), "first body retained: {frame}");
    assert!(
        frame.contains("second body"),
        "second body retained: {frame}"
    );
    assert!(frame.contains("third body"), "third body retained: {frame}");
    assert!(
        frame.contains("fourth body"),
        "fourth body retained: {frame}"
    );
}

#[test]
fn summary_thinking_omits_placeholder_and_collapses_titles_across_it() {
    let mut runtime = TranscriptPane::new(80, 16);
    for (id, text) in [
        ("summary-1", "**Plan**\nfirst body"),
        ("summary-placeholder", "<!-- -->"),
        ("summary-2", "**Plan**\nsecond body"),
    ] {
        runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
            turn: 1,
            id: id.to_owned(),
            kind: neo_ai::ThinkingKind::Summary,
        });
        runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
            turn: 1,
            text: text.to_owned(),
        });
        runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
            turn: 1,
            signature: None,
            redacted: false,
        });
    }

    assert!(runtime.toggle_tool_output_expanded());
    let frame = plain_frame(&mut runtime, 80, 16).join("\n");

    assert_eq!(
        frame.matches("Plan").count(),
        1,
        "placeholder does not break adjacent title collapse: {frame}"
    );
    assert!(frame.contains("first body"), "first body retained: {frame}");
    assert!(
        frame.contains("second body"),
        "second body retained: {frame}"
    );
    assert!(
        !frame.contains("<!-- -->"),
        "empty placeholder body is omitted: {frame}"
    );
}

#[test]
fn full_thinking_renders_bounded_preview() {
    let mut runtime = TranscriptPane::new(40, 12);
    runtime.push_transcript(TranscriptEntry::thinking_complete_with_kind(
        "alpha\nbeta\ngamma\ndelta",
        neo_ai::ThinkingKind::Full,
    ));

    let collapsed = plain_frame(&mut runtime, 40, 12).join("\n");
    assert!(collapsed.contains("● alpha"), "full head: {collapsed}");
    assert!(collapsed.contains("beta"), "full preview: {collapsed}");
    assert!(
        collapsed.contains("2 more lines (ctrl+o to expand)"),
        "full collapse hint: {collapsed}"
    );
    assert!(
        !collapsed.contains("gamma"),
        "full preview is bounded: {collapsed}"
    );

    assert!(runtime.toggle_tool_output_expanded());
    let expanded = plain_frame(&mut runtime, 40, 12).join("\n");
    assert!(expanded.contains("gamma"), "expanded full body: {expanded}");
    assert!(expanded.contains("delta"), "expanded full tail: {expanded}");
    assert!(
        !expanded.contains("ctrl+o to expand"),
        "expanded full thinking has no hint: {expanded}"
    );
}

#[test]
fn unknown_thinking_does_not_extract_title() {
    let mut runtime = TranscriptPane::new(60, 12);
    runtime.push_transcript(TranscriptEntry::thinking_complete_with_kind(
        "**Title**\nbody remains generic",
        neo_ai::ThinkingKind::Unknown,
    ));

    let frame = plain_frame(&mut runtime, 60, 12).join("\n");

    assert!(
        frame.contains("● **Title**"),
        "unknown keeps raw text: {frame}"
    );
    assert!(
        frame.contains("body remains generic"),
        "unknown body: {frame}"
    );
    assert!(
        !frame.contains("thinking · Title"),
        "unknown has no summary title: {frame}"
    );
}

#[test]
fn completed_thinking_shows_head_window_and_collapse_hint() {
    let mut runtime = TranscriptPane::new(40, 12);
    runtime.push_transcript(TranscriptEntry::thinking_complete(
        "alpha\nbeta\ngamma\ndelta\nepsilon",
    ));

    let frame = plain_frame(&mut runtime, 40, 12);
    let joined = frame.join("\n");

    // Completed thinking shows the first 2 lines with a ● bullet on the first.
    assert!(joined.contains("● alpha"), "head bullet: {joined}");
    assert!(joined.contains("beta"), "head second line: {joined}");
    // Collapse hint reports the dropped lines.
    assert!(
        joined.contains("3 more lines (ctrl+o to expand)"),
        "collapse hint: {joined}"
    );
    // Tail lines are hidden in the completed preview.
    assert!(
        !joined.contains("epsilon"),
        "completed thinking hides tail: {joined}"
    );
}

#[test]
fn completed_short_thinking_shows_all_without_hint() {
    let mut runtime = TranscriptPane::new(40, 12);
    runtime.push_transcript(TranscriptEntry::thinking_complete("just one line"));

    let frame = plain_frame(&mut runtime, 40, 12);
    let joined = frame.join("\n");
    assert!(
        joined.contains("● just one line"),
        "short thinking: {joined}"
    );
    assert!(
        !joined.contains("more lines"),
        "no collapse hint for short thinking: {joined}"
    );
}

#[test]
fn ctrl_o_toggle_expands_completed_thinking() {
    let mut runtime = TranscriptPane::new(40, 12);
    runtime.push_transcript(TranscriptEntry::thinking_complete(
        "alpha\nbeta\ngamma\ndelta\nepsilon",
    ));

    assert!(runtime.toggle_tool_output_expanded());
    let frame = plain_frame(&mut runtime, 40, 12);
    let joined = frame.join("\n");

    assert!(joined.contains("● alpha"), "expanded head: {joined}");
    assert!(joined.contains("epsilon"), "expanded tail: {joined}");
    assert!(
        !joined.contains("ctrl+o to expand"),
        "expanded thinking should not show collapse hint: {joined}"
    );
}

#[test]
fn consecutive_thinking_events_render_as_one_completed_block() {
    let mut runtime = TranscriptPane::new(40, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-1".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "first".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-2".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "second".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });

    assert!(runtime.toggle_tool_output_expanded());
    let frame = plain_frame(&mut runtime, 40, 12);
    let joined = frame.join("\n");
    let bullet_count = joined.matches('●').count();

    assert_eq!(bullet_count, 1, "one thinking bullet: {joined}");
    assert!(
        joined.contains("first"),
        "merged thinking keeps first: {joined}"
    );
    assert!(
        joined.contains("second"),
        "merged thinking keeps second: {joined}"
    );
}

#[test]
fn delta_first_thinking_inherits_expansion_state() {
    let mut runtime = TranscriptPane::new(40, 12);
    runtime.set_tool_output_expanded(true);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "alpha\nbeta\ngamma\ndelta\nepsilon".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });

    let joined = plain_frame(&mut runtime, 40, 12).join("\n");
    assert!(
        joined.contains("epsilon"),
        "expanded thinking body: {joined}"
    );
    assert!(
        !joined.contains("ctrl+o to expand"),
        "delta-first thinking should inherit expansion: {joined}"
    );
}
