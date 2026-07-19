use std::path::PathBuf;
use std::time::Instant;

use neo_tui::NeoTui;
use neo_tui::primitive::{strip_ansi, visible_width};
use neo_tui::shell::{NeoChromeState, PromptEdit};
use neo_tui::transcript::{CHROME_GUTTER, TranscriptBrowserState, TranscriptEntry, TranscriptPane};

#[test]
fn terminal_frame_acknowledges_history_without_replaying_live_chrome() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.push_status("committed status");
    transcript.start_assistant_message();
    transcript.append_assistant_delta("streaming tail");
    let mut tui = NeoTui::new(chrome, transcript);

    let first = tui.render_terminal_frame(80, 12);
    let history = first
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    let live = first
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(history.contains("committed status"));
    assert!(live.contains("streaming tail"));

    tui.acknowledge_history(&first);
    let second = tui.render_terminal_frame(80, 12);
    assert!(second.history.is_empty());
    assert!(
        second
            .live
            .iter()
            .map(|line| strip_ansi(line))
            .any(|line| line.contains("streaming tail"))
    );
}

#[test]
fn visible_footer_working_state_requests_an_animation_deadline() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let transcript = TranscriptPane::new(80, 12);
    let mut tui = NeoTui::new(chrome, transcript);
    tui.chrome_mut().set_shell_running(true);

    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());

    assert!(frame.next_animation_deadline.is_some());
}

#[test]
fn rendering_at_the_same_instant_does_not_advance_a_thinking_spinner() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.push_transcript(neo_tui::transcript::TranscriptEntry::thinking_streaming(
        "working it out",
    ));
    let mut tui = NeoTui::new(chrome, transcript);
    let now = Instant::now();

    let first = tui.render_terminal_frame_at(80, 12, now).live.join("\n");
    let second = tui.render_terminal_frame_at(80, 12, now).live.join("\n");

    assert_eq!(first, second);
}

#[test]
fn terminal_frame_is_bounded_when_chrome_exhausts_terminal_height() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = TranscriptPane::new(40, 4);
    transcript.start_assistant_message();
    transcript.append_assistant_delta("live assistant output");
    let mut tui = NeoTui::new(chrome, transcript);

    for height in 1..=4 {
        let frame = tui.render_terminal_frame(40, height);
        assert!(
            frame.live.len() <= height,
            "height {height} produced {} live rows",
            frame.live.len()
        );
    }
}

#[test]
fn transcript_browser_frame_is_bounded_and_marked_review_surface() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = TranscriptPane::new(80, 12);
    for index in 0..32 {
        transcript.push_status(format!("browser-status-{index}"));
    }
    let mut tui = NeoTui::new(chrome, transcript);

    tui.chrome_mut().open_transcript_browser(false);
    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());

    assert!(frame.review_surface);
    assert!(frame.history.is_empty());
    assert!(frame.live.len() <= 12);
}

#[test]
fn transcript_browser_expansion_reserves_chrome_rows() {
    let mut chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    chrome.prompt_mut().apply_edit(PromptEdit::Insert("draft"));
    chrome.open_transcript_browser(true);
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.push_transcript(TranscriptEntry::thinking_complete(
        (1..=20)
            .map(|index| format!("expanded-line-{index}"))
            .collect::<Vec<_>>()
            .join("\n"),
    ));
    let mut tui = NeoTui::new(chrome, transcript);

    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());
    let text = frame
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    let cursor = frame.cursor.expect("review keeps the prompt cursor");

    assert!(frame.review_surface);
    assert_eq!(frame.live.len(), 12);
    assert!(text.contains("expanded-line-20"), "frame: {text}");
    assert!(text.contains("draft"), "frame: {text}");
    assert!(text.contains("[ask]"), "frame: {text}");
    assert!(cursor.row < frame.live.len());
    assert!(cursor.row < 12);
}

#[test]
fn browser_render_does_not_consume_normal_pane_dirty_state() {
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.push_status("pending normal render");
    let mut browser = TranscriptBrowserState::new(false);

    let _ = transcript.render_browser_rows(&mut browser, 80, 8);

    assert!(transcript.is_dirty());
}

#[test]
fn transcript_browser_uses_terminal_width_before_gutter() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = TranscriptPane::new(20, 8);
    transcript.push_transcript(TranscriptEntry::assistant_message(
        "0123456789012345678901234567890123456789",
    ));

    let mut tui = NeoTui::new(chrome, transcript);
    tui.chrome_mut().open_transcript_browser(false);
    let frame = tui.render_terminal_frame_at(20, 8, Instant::now());
    let body_line = frame
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .find(|line| line.contains("012345"))
        .expect("review body line is visible");

    assert!(frame.live.iter().all(|line| visible_width(line) <= 20));
    assert!(
        frame
            .live
            .iter()
            .any(|line| visible_width(line) == 20 - CHROME_GUTTER)
    );
    assert!(body_line.starts_with(" ●"), "body line: {body_line:?}");
}

#[test]
fn transcript_browser_frame_requests_deadline_for_streaming_thinking() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.push_transcript(TranscriptEntry::thinking_streaming("still thinking"));
    let mut tui = NeoTui::new(chrome, transcript);

    tui.chrome_mut().open_transcript_browser(false);
    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());

    assert!(frame.next_animation_deadline.is_some());
}

#[test]
fn running_file_write_advances_transcript_animation_state() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "write-1".to_owned(),
        name: "Write".to_owned(),
    });
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "write-1".to_owned(),
        json_fragment: r#"{"path":"notes.txt","content":"draft"}"#.to_owned(),
    });
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "write-1".to_owned(),
        name: "Write".to_owned(),
        arguments: serde_json::json!({"path": "notes.txt", "content": "draft"}),
    });
    let mut tui = NeoTui::new(chrome, transcript);
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 12, now);
    assert!(frame.next_animation_deadline.is_some());
    assert!(!tui.is_transcript_dirty());

    tui.advance_animation_at(now);

    assert!(tui.is_transcript_dirty());
}

#[test]
fn running_static_tool_does_not_request_an_animation_deadline() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "read-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "notes.txt"}),
    });
    let mut tui = NeoTui::new(chrome, transcript);

    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());

    assert!(frame.next_animation_deadline.is_none());
}

fn push_overflowing_live_suffix(transcript: &mut TranscriptPane) {
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "overflow-live-tool".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "overflow-living-command" }),
    });
    let body = (0..40)
        .map(|index| format!("overflow-source-sentinel-{index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "overflow-live-tool".to_owned(),
        name: "Bash".to_owned(),
        partial_result: neo_agent_core::ToolResult::ok(body),
    });
}

#[test]
fn automatic_transcript_overflow_is_bounded_and_preserves_source_and_chrome() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = TranscriptPane::new(40, 8);
    push_overflowing_live_suffix(&mut transcript);
    let mut tui = NeoTui::new(chrome, transcript);

    let frame = tui.render_terminal_frame_at(40, 8, Instant::now());
    let text = frame
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(tui.automatic_overflow_active());
    assert!(frame.review_surface);
    assert!(frame.history.is_empty());
    assert!(frame.live.len() <= 8, "frame height: {}", frame.live.len());
    assert!(
        frame
            .cursor
            .is_some_and(|cursor| cursor.row < frame.live.len() && cursor.row < 8),
        "cursor must stay inside the bounded frame: {:?}",
        frame.cursor
    );
    assert!(text.contains("[ask]") || text.contains("ask"), "chrome missing: {text}");
    assert!(!text.contains("earlier rows omitted"), "frame: {text}");

    // Follow-tail keeps the latest source rows reachable without scrolling.
    // Card-local preview limits remain; this only proves presentation source
    // is viewported without presentation-level omission.
    assert!(
        text.contains("overflow-source-sentinel") || text.contains("Using Bash"),
        "expected overflow source in viewport: {text}"
    );

    // Scroll toward the top so the living tool header becomes visible.
    for _ in 0..20 {
        tui.scroll_automatic_overflow_up(4);
    }
    let scrolled = tui.render_terminal_frame_at(40, 8, Instant::now());
    let scrolled_text = scrolled
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(scrolled.review_surface);
    assert!(scrolled.live.len() <= 8);
    assert!(
        scrolled_text.contains("Using Bash") || scrolled_text.contains("overflow-living"),
        "early source must become reachable via viewport scroll: {scrolled_text}"
    );
    assert!(!scrolled_text.contains("earlier rows omitted"));
    // Scrolling away from the tail must change the visible window.
    assert_ne!(
        text.lines().take(3).collect::<Vec<_>>(),
        scrolled_text.lines().take(3).collect::<Vec<_>>(),
        "scroll should move the viewport window"
    );
}

#[test]
fn manual_review_reuses_latched_automatic_alternate_surface() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = TranscriptPane::new(40, 8);
    push_overflowing_live_suffix(&mut transcript);
    let mut tui = NeoTui::new(chrome, transcript);

    let automatic = tui.render_terminal_frame_at(40, 8, Instant::now());
    assert!(tui.automatic_overflow_active());
    assert!(automatic.review_surface);

    tui.chrome_mut().open_transcript_browser(false);
    let manual = tui.render_terminal_frame_at(40, 8, Instant::now());
    assert!(tui.automatic_overflow_active(), "manual review must not release latch");
    assert!(manual.review_surface);
    assert!(manual.history.is_empty());

    tui.chrome_mut().close_transcript_browser();
    let restored = tui.render_terminal_frame_at(40, 8, Instant::now());
    assert!(tui.automatic_overflow_active());
    assert!(restored.review_surface);
    assert!(restored.history.is_empty());
}
