//! Phase 6: thinking renders as a fixed 2-line floating window.
//!
//! Live thinking shows a `⠋ thinking...` header + the *last* 2 wrapped rows
//! (scrolling tail). Finalized thinking shows the *first* 2 rows + a collapse
//! hint when the full text was longer.

use neo_tui::NeoTuiRuntime;
use neo_tui::ansi::strip_ansi;
use neo_tui::transcript::TranscriptEntry;

fn plain_frame(runtime: &mut NeoTuiRuntime, width: usize, height: usize) -> Vec<String> {
    runtime
        .render_frame(width, height)
        .expect("render frame")
        .iter()
        .map(|line| strip_ansi(line).trim_end().to_owned())
        .collect()
}

#[test]
fn live_thinking_shows_spinner_and_tail_window() {
    let mut runtime = NeoTuiRuntime::new(40, 12);
    // Build a live assistant entry with multi-line thinking.
    runtime.push_transcript(TranscriptEntry::Assistant {
        thinking: "alpha\nbeta\ngamma\ndelta\nepsilon".to_owned(),
        content: String::new(),
        finalized: false,
    });

    let frame = plain_frame(&mut runtime, 40, 12);
    let joined = frame.join("\n");

    // Live header is the spinner line.
    assert!(joined.contains("⠋ thinking..."), "spinner header: {joined}");
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
fn finalized_thinking_shows_head_window_and_collapse_hint() {
    let mut runtime = NeoTuiRuntime::new(40, 12);
    runtime.push_transcript(TranscriptEntry::Assistant {
        thinking: "alpha\nbeta\ngamma\ndelta\nepsilon".to_owned(),
        content: String::new(),
        finalized: true,
    });

    let frame = plain_frame(&mut runtime, 40, 12);
    let joined = frame.join("\n");

    // Finalized shows the first 2 lines with a ● bullet on the first.
    assert!(joined.contains("● alpha"), "head bullet: {joined}");
    assert!(joined.contains("beta"), "head second line: {joined}");
    // Collapse hint reports the dropped lines.
    assert!(
        joined.contains("3 more lines (ctrl+o to expand)"),
        "collapse hint: {joined}"
    );
    // Tail lines are hidden in the finalized preview.
    assert!(
        !joined.contains("epsilon"),
        "finalized hides tail: {joined}"
    );
}

#[test]
fn finalized_short_thinking_shows_all_without_hint() {
    let mut runtime = NeoTuiRuntime::new(40, 12);
    runtime.push_transcript(TranscriptEntry::Assistant {
        thinking: "just one line".to_owned(),
        content: String::new(),
        finalized: true,
    });

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
