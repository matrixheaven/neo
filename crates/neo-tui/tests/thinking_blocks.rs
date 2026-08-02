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
        id: "summary".to_owned(),
        kind: neo_ai::ThinkingKind::Summary,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "**Planning context recall and repo inspection****Planning parallel recall"
            .to_owned(),
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
fn summary_thinking_deduplicates_repeated_titles_when_expanded() {
    let mut runtime = TranscriptPane::new(80, 16);
    runtime.push_transcript(TranscriptEntry::thinking_complete_with_kind(
        "**Planning context recall and repo inspection****Planning parallel recall and codegraph check****Initiating parallel memory recall and repo status checks****Planning context recall and repo inspection**",
        neo_ai::ThinkingKind::Summary,
    ));

    assert!(runtime.toggle_tool_output_expanded());
    let frame = plain_frame(&mut runtime, 80, 16).join("\n");

    assert_eq!(
        frame
            .matches("Planning context recall and repo inspection")
            .count(),
        1,
        "repeated title is shown once: {frame}"
    );
    assert!(
        frame.contains("Planning parallel recall and codegraph check"),
        "second title remains: {frame}"
    );
    assert!(
        frame.contains("Initiating parallel memory recall and repo status checks"),
        "third title remains: {frame}"
    );
    assert!(
        !frame.contains("**"),
        "summary markers stay out of UI: {frame}"
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
