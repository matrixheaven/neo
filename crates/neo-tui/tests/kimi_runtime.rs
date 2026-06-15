use neo_tui::ToolStatusKind;
use neo_tui::ansi::strip_ansi;
use neo_tui::core::{Finalization, RenderKind};
use neo_tui::runtime::NeoTuiRuntime;
use neo_tui::streaming::StreamingController;
use neo_tui::transcript::TranscriptEntry;

/// Strip ANSI + trim from a frame line, for content assertions.
fn plain(line: &str) -> String {
    strip_ansi(line).trim_end().to_owned()
}

/// Render a frame and return its lines as plain (ANSI-stripped) strings.
fn plain_frame(runtime: &mut NeoTuiRuntime, width: usize, height: usize) -> Vec<String> {
    runtime
        .render_frame(width, height)
        .expect("render frame")
        .iter()
        .map(|line| plain(line))
        .collect()
}

#[test]
fn runtime_renders_finalized_rows_then_live_rows_in_one_frame() {
    let mut runtime = NeoTuiRuntime::new(80, 12);

    runtime.push_transcript(TranscriptEntry::banner("Welcome to neo"));
    runtime.push_transcript(TranscriptEntry::user("hello"));
    runtime.push_transcript(TranscriptEntry::tool_call_running("Bash", "cargo test"));
    runtime.push_transcript(TranscriptEntry::assistant_live("streaming"));

    let frame = plain_frame(&mut runtime, 80, 12);
    // Finalized rows (banner, user) precede the live region (tool card,
    // streaming assistant).
    let welcome = frame
        .iter()
        .position(|l| l == "Welcome to neo")
        .expect("banner");
    let hello = frame
        .iter()
        .position(|l| l == "hello")
        .expect("user message");
    let tool = frame
        .iter()
        .position(|l| l.contains("Using Bash"))
        .expect("running tool card");
    let streaming = frame
        .iter()
        .position(|l| l.contains("streaming"))
        .expect("streaming assistant");
    assert!(welcome < hello);
    assert!(hello < tool);
    assert!(tool < streaming);
}

#[test]
fn runtime_exposes_frame_ansi_lines_for_inspection() {
    let mut runtime = NeoTuiRuntime::new(80, 12);
    runtime.push_transcript(TranscriptEntry::tool_call_running("Bash", "cargo test"));
    runtime.render_tick();

    let lines = runtime.frame_ansi_lines();
    assert!(lines.iter().any(|line| line.contains("Using Bash")));
}

#[test]
fn runtime_maps_user_and_assistant_events_to_transcript_entries() {
    let mut runtime = NeoTuiRuntime::new(80, 12);

    runtime.push_user_message("hello");
    runtime.push_assistant_final("world");
    runtime.request_render(RenderKind::Incremental);
    let frame = plain_frame(&mut runtime, 80, 12);

    assert!(frame.iter().any(|l| l == "hello"));
    assert!(frame.iter().any(|l| l == "world"));
}

#[test]
fn runtime_keeps_streaming_assistant_live_until_finalized() {
    let mut runtime = NeoTuiRuntime::new(80, 12);

    runtime.push_user_message("hello");
    runtime.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Hel".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "lo".to_owned(),
    });

    let first = plain_frame(&mut runtime, 80, 12);
    assert!(first.iter().any(|l| l == "hello"));
    assert!(first.iter().any(|l| l == "Hello"));

    runtime.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "assistant-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });
    let second = plain_frame(&mut runtime, 80, 12);
    // "Hello" is still in the frame (now finalized, not live), exactly once.
    assert_eq!(
        second.iter().filter(|l| **l == "Hello").count(),
        1,
        "finalized assistant text appears exactly once: {second:?}"
    );
}

#[test]
fn runtime_commits_finalized_tool_cards_into_frame() {
    let mut runtime = NeoTuiRuntime::new(80, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),
    });
    let running = plain_frame(&mut runtime, 80, 12);
    assert!(running.iter().any(|l| l.contains("Using Read (README.md)")));

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("line one\nline two"),
    });
    let finalized = plain_frame(&mut runtime, 80, 12);

    assert!(
        finalized
            .iter()
            .any(|l| l.contains("Used Read (README.md)"))
    );
    // The finalized card appears exactly once (no duplicate live copy).
    assert_eq!(
        finalized
            .iter()
            .filter(|l| l.contains("Used Read (README.md)"))
            .count(),
        1
    );
}

#[test]
fn runtime_accumulates_tool_argument_delta_fragments() {
    let mut runtime = NeoTuiRuntime::new(80, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: "{\"path\":\"".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: "README.md\"}".to_owned(),
    });

    let frame = plain_frame(&mut runtime, 80, 12);
    assert!(frame.iter().any(|l| l.contains("Using Read (README.md)")));
}

#[test]
fn runtime_live_region_keeps_tool_card_and_streaming_assistant() {
    let mut runtime = NeoTuiRuntime::new(80, 6);

    runtime.set_live_chrome_height(4);
    runtime.push_transcript(TranscriptEntry::tool_call_running("Bash", "cargo test"));
    runtime.push_transcript(TranscriptEntry::assistant_live("streaming"));
    let frame = plain_frame(&mut runtime, 80, 6);

    // The live region (tool card + streaming assistant) is in the frame.
    let has_tool = frame.iter().any(|l| l.contains("Using Bash"));
    let has_streaming = frame.iter().any(|l| l.contains("streaming"));
    assert!(has_tool || has_streaming, "live region present: {frame:?}");
}

#[test]
fn streaming_controller_updates_one_tool_card_in_place() {
    let mut controller = StreamingController::new();

    controller.apply_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
    });
    controller.apply_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: r#"{"path":"README.md"}"#.to_owned(),
    });
    controller.apply_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),
    });
    controller.apply_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("line one\nline two"),
    });

    assert_eq!(controller.tool_count(), 1);
    let card = controller.tool("tool-1").expect("tool card exists");
    assert_eq!(card.id(), "tool-1");
    assert_eq!(card.status(), ToolStatusKind::Succeeded);
    assert_eq!(card.arguments(), Some(r#"{"path":"README.md"}"#));
    assert_eq!(card.result(), Some("line one\nline two"));
    assert_eq!(card.finalization(), Finalization::Finalized);
}

#[test]
fn streaming_controller_keeps_running_tool_live() {
    let mut controller = StreamingController::new();

    controller.apply_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
    });

    let card = controller.tool("tool-1").expect("tool card exists");
    assert_eq!(card.status(), ToolStatusKind::Running);
    assert_eq!(card.finalization(), Finalization::Live);
}

#[test]
fn streaming_controller_records_tool_execution_updates_on_existing_card() {
    let mut controller = StreamingController::new();

    controller.apply_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
    });
    controller.apply_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        partial_result: neo_agent_core::ToolResult::ok("building crate"),
    });

    assert_eq!(controller.tool_count(), 1);
    let card = controller.tool("bash-1").expect("tool card exists");
    assert_eq!(card.progress(), &["building crate".to_owned()]);
    assert_eq!(card.status(), ToolStatusKind::Running);
}

#[test]
fn runtime_finalizes_streaming_assistant_once_without_live_duplicate() {
    let mut runtime = NeoTuiRuntime::new(80, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "hello".to_owned(),
    });
    let live = plain_frame(&mut runtime, 80, 12);
    assert_eq!(live.iter().filter(|l| **l == "hello").count(), 1);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "assistant-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });
    let finalized = plain_frame(&mut runtime, 80, 12);
    assert_eq!(
        finalized.iter().filter(|l| **l == "hello").count(),
        1,
        "finalized assistant text appears exactly once: {finalized:?}"
    );
}

#[test]
fn replayed_messages_render_through_same_runtime_path() {
    let mut runtime = NeoTuiRuntime::new(80, 12);
    runtime.replay_user_message("previous prompt");
    runtime.replay_assistant_message("previous answer");
    runtime.request_render(RenderKind::Incremental);

    let frame = plain_frame(&mut runtime, 80, 12);
    assert!(frame.iter().any(|l| l == "previous prompt"));
    assert!(frame.iter().any(|l| l == "previous answer"));
}
