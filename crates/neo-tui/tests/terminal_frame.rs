use std::path::PathBuf;
use std::time::Instant;

use neo_tui::NeoTui;
use neo_tui::primitive::{strip_ansi, visible_width};
use neo_tui::shell::{NeoChromeState, PromptEdit};
use neo_tui::tasks_browser::TaskBrowserState;
use neo_tui::transcript::TranscriptEntry;

#[test]
fn interactive_frame_is_one_bounded_fullscreen_document() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.push_status("committed status");
    transcript.start_assistant_message();
    transcript.append_assistant_delta("streaming tail");
    let mut tui = NeoTui::new(chrome, transcript);

    let frame = tui.render_terminal_frame(80, 12);
    let text = frame
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    // One bounded frame: the visible document slice plus fitted chrome.
    assert!(frame.lines.len() <= 12, "frame rows: {}", frame.lines.len());
    assert!(text.contains("committed status"), "frame: {text}");
    assert!(text.contains("streaming tail"), "frame: {text}");
    assert!(
        frame
            .cursor
            .is_none_or(|cursor| cursor.row < frame.lines.len() && cursor.row < 12),
        "cursor must stay inside the bounded frame: {:?}",
        frame.cursor
    );
    assert!(
        frame.lines.iter().all(|line| visible_width(line) <= 80),
        "every frame line must fit the terminal width"
    );
}

#[test]
fn visible_footer_working_state_requests_an_animation_deadline() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    let mut tui = NeoTui::new(chrome, transcript);
    tui.chrome_mut().set_shell_running(true);

    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());

    assert!(frame.next_animation_deadline.is_some());
}

#[test]
fn rendering_at_the_same_instant_does_not_advance_a_thinking_spinner() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.push_transcript(TranscriptEntry::thinking_streaming("working it out"));
    let mut tui = NeoTui::new(chrome, transcript);
    let now = Instant::now();

    let first = tui.render_terminal_frame_at(80, 12, now).lines.join("\n");
    let second = tui.render_terminal_frame_at(80, 12, now).lines.join("\n");

    assert_eq!(first, second);
}

#[test]
fn frame_is_bounded_when_chrome_exhausts_terminal_height() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(40, 4);
    transcript.start_assistant_message();
    transcript.append_assistant_delta("live assistant output");
    let mut tui = NeoTui::new(chrome, transcript);

    for height in 1..=4 {
        let frame = tui.render_terminal_frame(40, height);
        assert!(
            frame.lines.len() <= height,
            "height {height} produced {} rows",
            frame.lines.len()
        );
    }
}

#[test]
fn streaming_thinking_requests_an_animation_deadline() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.push_transcript(TranscriptEntry::thinking_streaming("still thinking"));
    let mut tui = NeoTui::new(chrome, transcript);

    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());

    assert!(frame.next_animation_deadline.is_some());
}

#[test]
fn running_file_write_advances_transcript_animation_state() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
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

        workflow_origin: None,
        output_ref: None,
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
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "read-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "notes.txt"}),

        workflow_origin: None,
        output_ref: None,
    });
    let mut tui = NeoTui::new(chrome, transcript);

    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());

    assert!(frame.next_animation_deadline.is_none());
}

#[test]
fn running_sleep_requests_animation_deadline() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "sleep-anim".to_owned(),
        name: "Sleep".to_owned(),
        arguments: serde_json::json!({
            "duration_seconds": 45,
            "reason": "wait for cooldown"
        }),

        workflow_origin: None,
        output_ref: None,
    });
    let mut tui = NeoTui::new(chrome, transcript);

    let running = tui.render_terminal_frame_at(80, 12, Instant::now());
    assert!(
        running.next_animation_deadline.is_some(),
        "running Sleep must request animation deadline"
    );

    tui.transcript_mut()
        .apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "sleep-anim".to_owned(),
            name: "Sleep".to_owned(),
            result: neo_agent_core::ToolResult::ok("Waited 45 seconds: wait for cooldown"),

            workflow_origin: None,
            output_ref: None,
        });
    let finished = tui.render_terminal_frame_at(80, 12, Instant::now());
    assert!(
        finished.next_animation_deadline.is_none(),
        "completed Sleep must not request animation deadline"
    );
}

fn push_overflowing_live_suffix(transcript: &mut neo_tui::transcript::TranscriptPane) {
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "overflow-live-tool".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "overflow-living-command" }),

        workflow_origin: None,
        output_ref: None,
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

        workflow_origin: None,
        output_ref: None,
    });
}

#[test]
fn tall_document_slice_stays_bounded_in_the_fullscreen_frame() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(40, 8);
    push_overflowing_live_suffix(&mut transcript);
    let mut tui = NeoTui::new(chrome, transcript);

    // A tall live workload renders as one bounded document slice inside the
    // already-active fullscreen surface: no second surface, no mouse flag,
    // and the visible slice stays frame-safe.
    let frame = tui.render_terminal_frame_at(40, 8, Instant::now());
    let text = frame
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!frame.lines.is_empty());
    assert!(
        frame.lines.len() <= 8,
        "slice must stay bounded by the terminal height: {}",
        frame.lines.len()
    );
    assert!(
        frame
            .cursor
            .is_none_or(|cursor| cursor.row < frame.lines.len() && cursor.row < 8),
        "cursor must stay inside the bounded frame: {:?}",
        frame.cursor
    );
    assert!(
        text.contains("[ask]") || text.contains("ask"),
        "chrome missing: {text}"
    );
    assert!(!text.contains("earlier rows omitted"), "frame: {text}");
    // Tail follow shows the newest output rows of the tall card; the card
    // header stays reachable by scrolling up the document.
    assert!(
        text.contains("overflow-source-sentinel-39"),
        "newest output row missing: {text}"
    );
    tui.transcript_mut().scroll_transcript_up(usize::MAX);
    let top = tui.render_terminal_frame_at(40, 8, Instant::now());
    let top_text = top
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        top_text.contains("Using Bash") || top_text.contains("overflow-living"),
        "the living card header is reachable by scrolling: {top_text}"
    );
    assert!(top.lines.len() <= 8);
}

#[test]
fn blocking_overlays_render_inside_the_active_fullscreen_frame() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(40, 8);
    transcript.push_status("history status");
    let mut tui = NeoTui::new(chrome, transcript);

    let plain = tui.render_terminal_frame_at(40, 8, Instant::now());
    assert!(plain.lines.len() <= 8);

    // Task Browser is an overlay inside the already-fullscreen session: the
    // frame is still one bounded line set with no physical transition.
    tui.chrome_mut()
        .push_task_browser_overlay(TaskBrowserState::new());
    let overlay = tui.render_terminal_frame_at(40, 8, Instant::now());
    assert!(overlay.lines.len() <= 8);
    assert!(!overlay.lines.is_empty());

    tui.chrome_mut().close_focused_overlay();
    let restored = tui.render_terminal_frame_at(40, 8, Instant::now());
    assert!(restored.lines.len() <= 8);

    tui.chrome_mut().open_help_panel(Vec::new());
    let dialog = tui.render_terminal_frame_at(40, 8, Instant::now());
    assert!(dialog.lines.len() <= 8);
    assert!(!dialog.lines.is_empty());
}

#[test]
fn expansion_toggle_resizes_the_primary_document_slice() {
    let mut chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    chrome.prompt_mut().apply_edit(PromptEdit::Insert("draft"));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.push_transcript(TranscriptEntry::thinking_complete(
        (1..=20)
            .map(|index| format!("expanded-line-{index}"))
            .collect::<Vec<_>>()
            .join("\n"),
    ));
    let mut tui = NeoTui::new(chrome, transcript);

    let collapsed = tui.render_terminal_frame_at(80, 12, Instant::now());
    let collapsed_text = collapsed
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !collapsed_text.contains("expanded-line-20"),
        "collapsed frame: {collapsed_text}"
    );

    tui.transcript_mut().toggle_tool_output_expanded();
    let expanded = tui.render_terminal_frame_at(80, 12, Instant::now());
    let expanded_text = expanded
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        expanded_text.contains("expanded-line-20"),
        "expanded frame: {expanded_text}"
    );
    assert!(expanded_text.contains("draft"), "frame: {expanded_text}");
    assert!(expanded_text.contains("[ask]"), "frame: {expanded_text}");
    assert_eq!(
        expanded.lines.len(),
        12,
        "fullscreen frame fills the height"
    );
}
