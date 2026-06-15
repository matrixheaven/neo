use neo_tui::ToolStatusKind;
use neo_tui::core::{Finalization, Line, RenderKind};
use neo_tui::runtime::NeoTuiRuntime;
use neo_tui::streaming::StreamingController;
use neo_tui::transcript::TranscriptEntry;

#[test]
fn runtime_commits_finalized_rows_and_keeps_live_region_bounded() {
    let mut runtime = NeoTuiRuntime::new(80, 2);

    runtime.push_transcript(TranscriptEntry::banner("Welcome to neo"));
    runtime.push_transcript(TranscriptEntry::user("hello"));
    runtime.push_transcript(TranscriptEntry::tool_call_running("Bash", "cargo test"));
    runtime.push_transcript(TranscriptEntry::assistant_live("streaming"));
    runtime.render_tick();

    assert!(
        runtime
            .terminal()
            .committed_rows()
            .contains(&Line::raw("Welcome to neo"))
    );
    assert!(
        runtime
            .terminal()
            .committed_rows()
            .iter()
            .any(|row| neo_tui::ansi::strip_ansi(&row.to_ansi()) == "hello")
    );
    assert_eq!(runtime.terminal().live_rows().len(), 2);
    assert!(
        runtime
            .terminal()
            .live_rows()
            .iter()
            .any(|row| neo_tui::ansi::strip_ansi(&row.to_ansi()).contains("Using Bash"))
    );
}

#[test]
fn runtime_render_output_returns_newly_committed_rows_once() {
    let mut runtime = NeoTuiRuntime::new(80, 12);
    runtime.push_transcript(TranscriptEntry::banner("Welcome to neo"));

    let first = runtime.render_output().expect("first render output");
    assert_eq!(first.committed, vec![Line::raw("Welcome to neo")]);

    runtime.request_render(RenderKind::Incremental);
    let second = runtime.render_output().expect("second render output");
    assert!(second.committed.is_empty());
}

#[test]
fn runtime_exposes_ansi_live_rows_for_terminal_writer() {
    let mut runtime = NeoTuiRuntime::new(80, 12);
    runtime.push_transcript(TranscriptEntry::tool_call_running("Bash", "cargo test"));
    runtime.render_tick();

    let lines = runtime.live_ansi_lines();
    assert!(lines.iter().any(|line| line.contains("Using Bash")));
}

#[test]
fn runtime_maps_user_and_assistant_events_to_transcript_entries() {
    let mut runtime = NeoTuiRuntime::new(80, 12);

    runtime.push_user_message("hello");
    runtime.push_assistant_final("world");
    runtime.request_render(RenderKind::Incremental);
    let output = runtime.render_output().expect("render output");

    assert!(output.committed.contains(&Line::raw("hello")));
    assert!(output.committed.contains(&Line::raw("world")));
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

    let first = runtime.render_output().expect("first output");
    assert!(first.committed.contains(&Line::raw("hello")));
    assert!(!first.committed.contains(&Line::raw("Hello")));
    assert!(first.live.contains(&Line::raw("Hello")));

    runtime.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "assistant-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });
    let second = runtime.render_output().expect("second output");
    assert!(second.committed.contains(&Line::raw("Hello")));
    assert!(!second.live.contains(&Line::raw("Hello")));
}

#[test]
fn runtime_commits_finalized_tool_cards_into_scrollback() {
    let mut runtime = NeoTuiRuntime::new(80, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),
    });
    let running = runtime.render_output().expect("running output");
    assert!(running.live.iter().any(|row| {
        neo_tui::ansi::strip_ansi(&row.to_ansi()).contains("Using Read (README.md)")
    }));

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("line one\nline two"),
    });
    let finalized = runtime.render_output().expect("finalized output");

    assert!(finalized.committed.iter().any(|row| {
        neo_tui::ansi::strip_ansi(&row.to_ansi()).contains("Used Read (README.md)")
    }));
    assert!(!finalized.live.iter().any(|row| {
        neo_tui::ansi::strip_ansi(&row.to_ansi()).contains("Used Read (README.md)")
    }));
    assert!(runtime.terminal().committed_rows().iter().any(|row| {
        neo_tui::ansi::strip_ansi(&row.to_ansi()).contains("Used Read (README.md)")
    }));
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

    let output = runtime.render_output().expect("render output");
    assert!(output.live.iter().any(|row| {
        neo_tui::ansi::strip_ansi(&row.to_ansi()).contains("Using Read (README.md)")
    }));
}

#[test]
fn runtime_live_region_reserves_rows_for_prompt_and_footer_chrome() {
    let mut runtime = NeoTuiRuntime::new(80, 6);

    runtime.set_live_chrome_height(4);
    runtime.push_transcript(TranscriptEntry::tool_call_running("Bash", "cargo test"));
    runtime.push_transcript(TranscriptEntry::assistant_live("streaming"));
    let output = runtime.render_output().expect("render output");

    assert_eq!(output.live.len(), 2);
    assert_eq!(runtime.terminal().live_rows().len(), 2);
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
    let live = runtime.render_output().expect("live output");
    assert_eq!(
        live.live
            .iter()
            .filter(|row| row == &&Line::raw("hello"))
            .count(),
        1
    );

    runtime.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "assistant-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });
    let finalized = runtime.render_output().expect("final output");
    assert_eq!(
        finalized
            .committed
            .iter()
            .filter(|row| row == &&Line::raw("hello"))
            .count(),
        1
    );
    assert!(!finalized.live.contains(&Line::raw("hello")));
}

#[test]
fn replayed_messages_commit_through_same_runtime_path() {
    let mut runtime = NeoTuiRuntime::new(80, 12);
    runtime.replay_user_message("previous prompt");
    runtime.replay_assistant_message("previous answer");
    runtime.request_render(RenderKind::Incremental);

    let output = runtime.render_output().expect("render output");
    assert!(output.committed.contains(&Line::raw("previous prompt")));
    assert!(output.committed.contains(&Line::raw("previous answer")));
}
