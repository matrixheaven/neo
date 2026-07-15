use std::path::PathBuf;
use std::time::Instant;

use neo_tui::NeoTui;
use neo_tui::primitive::strip_ansi;
use neo_tui::shell::NeoChromeState;
use neo_tui::transcript::TranscriptPane;

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
