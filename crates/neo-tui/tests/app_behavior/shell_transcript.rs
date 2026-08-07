use neo_tui::shell::{NeoChromeState, ToolStatusKind};
use neo_tui::terminal_image::{
    ImageProtocolPreference, ImageRenderPolicy, TerminalImageCapabilities,
};
use neo_tui::transcript::{TranscriptImageAttachment, TranscriptPane};
use std::path::PathBuf;

fn render_transcript(width: usize, height: usize, transcript: &mut TranscriptPane) -> Vec<String> {
    transcript
        .render_frame(width, height)
        .expect("transcript frame")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect()
}

#[test]
fn replayed_user_image_content_keeps_transcript_attachment() {
    let encoded = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1Pe";
    let mut transcript = TranscriptPane::new(100, 20);

    transcript.replay_message(&neo_agent_core::AgentMessage::user_content([
        neo_agent_core::Content::text("look "),
        neo_agent_core::Content::Image {
            mime_type: "image/png".into(),
            data: neo_agent_core::ImageRef::Base64(encoded.into()),
        },
    ]));

    assert!(matches!(
        transcript.transcript().entries().last(),
        Some(neo_tui::transcript::TranscriptEntry::UserMessage { content, images })
            if content == "look [image #1 (1x1)]" && images.len() == 1
    ));
}

#[test]
fn transcript_pane_frame_keeps_latest_live_row_visible() {
    let mut runtime = TranscriptPane::new(80, 12);
    for index in 0..36 {
        runtime.start_assistant_message();
        runtime.append_assistant_delta(&format!("history line {index}"));
    }

    let lines = render_transcript(80, 12, &mut runtime);

    assert!(lines.iter().any(|line| line.contains("history line 35")));
}

#[test]
fn transcript_pane_inline_images_are_structured_entries() {
    let mut runtime = TranscriptPane::new(100, 12);
    runtime.push_image(
        "image/png",
        &neo_agent_core::ImageRef::Base64("aGVsbG8=".into()),
    );

    assert!(matches!(
        runtime.transcript().entries().last(),
        Some(neo_tui::transcript::TranscriptEntry::Image { mime_type, payload, .. })
            if mime_type == "image/png" && payload.is_some()
    ));
}

#[test]
fn transcript_pane_maps_queue_notice_and_compaction_boundary() {
    let mut runtime = TranscriptPane::new(100, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::FollowUp,
        count: 2,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::CompactionApplied {
        summary: neo_agent_core::CompactionSummary {
            summary: "Older context summarized.".to_owned(),
            tokens_before: 12_345,
            tokens_after: 6_000,
            first_kept_message_index: 4,
        },
    });

    // Queue events are now rendered in the Pending Input Preview panel, not as
    // transcript status lines. Compaction events still produce transcript
    // entries.
    assert!(
        runtime
            .transcript()
            .entries()
            .iter()
            .all(|entry| !matches!(entry, neo_tui::transcript::TranscriptEntry::Status { text, .. } if text.contains("FollowUp queue drained"))),
        "queue events must not produce transcript status lines"
    );
    assert!(matches!(
        &runtime.transcript().entries()[0],
        neo_tui::transcript::TranscriptEntry::Compaction { compacted_message_count, tokens_before, .. }
            if *compacted_message_count == 4 && *tokens_before == 12_345
    ));
}

#[test]
fn transcript_pane_maps_shell_command_lifecycle_to_tool_run() {
    let mut runtime = TranscriptPane::new(100, 12);

    // The runtime emits ToolExecutionStarted (which creates the card) before
    // ShellCommandStarted for the same id; the shell events only update it.
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test -p neo-tui" }),
        workflow_origin: None,
        output_ref: None,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        command: "cargo test -p neo-tui".to_owned(),
        cwd: PathBuf::from("/workspace/neo"),
        origin: neo_agent_core::ShellCommandOrigin::ModelBashTool,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "shell-1".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "ok".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: neo_agent_core::ShellCommandOrigin::ModelBashTool,
        outcome: neo_agent_core::ShellCommandOutcome::Completed,
        output_ref: None,
    });

    let entries = runtime.transcript().entries();
    assert!(matches!(
        entries.last(),
        Some(neo_tui::transcript::TranscriptEntry::ToolRun { component })
            if component.name() == "Bash"
                && component.status() == ToolStatusKind::Succeeded
                && component.result().is_some_and(|result| result.contains("ok"))
    ));
    let lines = render_transcript(100, 12, &mut runtime);
    assert!(lines.iter().any(|line| line.contains("● Used Bash")));
}

#[test]
fn transcript_pane_preserves_tool_arguments_separately_from_result() {
    let mut runtime = TranscriptPane::new(100, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: r#"{"path":"README.md"}"#.to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),

        workflow_origin: None,
        output_ref: None,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("read README"),

        workflow_origin: None,
        output_ref: None,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::tool_result(
            "tool-1",
            "Read",
            [neo_agent_core::Content::text("read README")],
            false,
        ),
    });

    let tool_runs = runtime
        .transcript()
        .entries()
        .iter()
        .filter(|entry| matches!(entry, neo_tui::transcript::TranscriptEntry::ToolRun { .. }))
        .count();
    assert_eq!(tool_runs, 1);
    assert!(matches!(
        runtime.transcript().entries().last(),
        Some(neo_tui::transcript::TranscriptEntry::ToolRun { component })
            if component.name() == "Read"
                && component.status() == ToolStatusKind::Succeeded
                && component.arguments() == Some(r#"{"path":"README.md"}"#)
                && component.result() == Some("read README")
    ));
}

#[test]
fn transcript_pane_renders_bash_result_as_terminal_output_without_structural_labels() {
    let mut runtime = TranscriptPane::new(100, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "printf out; printf err >&2" }),

        workflow_origin: None,
        output_ref: None,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok("outerr").with_details(serde_json::json!({
            "exit_code": 0,
            "stdout": "out",
            "stderr": "err",
            "stdout_truncated": false,
            "stderr_truncated": false,
            "truncated": false
        })),

        workflow_origin: None,
        output_ref: None,
    });

    let joined = render_transcript(100, 12, &mut runtime).join("\n");
    assert!(joined.contains("● Used Bash"));
    assert!(joined.contains("outerr"));
    assert!(!joined.contains("exit_code:"));
    assert!(!joined.contains("stdout:"));
    assert!(!joined.contains("stderr:"));
}

#[test]
fn transcript_pane_replays_thinking_tool_assistant_in_order() {
    let mut runtime = TranscriptPane::new(100, 20);
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-1".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "Need files".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "List".to_owned(),
        arguments: serde_json::json!({ "path": "." }),

        workflow_origin: None,
        output_ref: None,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "List".to_owned(),
        result: neo_agent_core::ToolResult::ok("README.md"),

        workflow_origin: None,
        output_ref: None,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-2".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "Ready".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Final answer".to_owned(),
    });

    let entries = runtime.transcript().entries();
    assert!(matches!(
        entries[0],
        neo_tui::transcript::TranscriptEntry::ThinkingBlock { .. }
    ));
    assert!(matches!(
        entries[1],
        neo_tui::transcript::TranscriptEntry::ToolRun { .. }
    ));
    assert!(matches!(
        entries[2],
        neo_tui::transcript::TranscriptEntry::ThinkingBlock { .. }
    ));
    assert!(matches!(
        entries[3],
        neo_tui::transcript::TranscriptEntry::AssistantMessage { .. }
    ));
}

#[test]
fn transcript_pane_running_tool_call_is_rendered_before_finish() {
    let mut runtime = TranscriptPane::new(100, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "List".to_owned(),
        arguments: serde_json::json!({ "path": "crates/neo-tui/src" }),

        workflow_origin: None,
        output_ref: None,
    });

    let entries = runtime.transcript().entries();
    assert_eq!(entries.len(), 1);
    assert!(matches!(
        entries.last(),
        Some(neo_tui::transcript::TranscriptEntry::ToolRun { component })
            if component.name() == "List"
                && component.status() == ToolStatusKind::Running
                && component.arguments().is_some_and(|arguments| arguments.contains("crates/neo-tui/src"))
    ));

    let lines = render_transcript(100, 12, &mut runtime);
    assert!(lines.iter().any(|line| line.contains("● Using List")));
}

#[test]
fn transcript_user_images_render_thumbnail_inside_normal_frame() {
    let mut chrome = NeoChromeState::new("neo", "session", "openai/gpt-4.1", "/tmp/neo-ws");
    chrome.set_image_render_policy(ImageRenderPolicy::new(ImageProtocolPreference::Kitty));
    chrome.set_image_capabilities(TerminalImageCapabilities::default().with_kitty(true));
    let mut transcript = TranscriptPane::new(100, 20);
    transcript.push_user_message_with_images(
        "look",
        vec![TranscriptImageAttachment::new(
            "image-1",
            "image/png",
            1_184,
            650,
            "[image #1 (1184x650)]",
            vec![
                0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
                0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
                0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78,
                0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xC9, 0xFE, 0x92,
                0xEF, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
            ],
        )],
    );
    let mut tui = neo_tui::NeoTui::new(chrome, transcript);

    let frame = tui.render_frame(100, 20).0;

    assert!(frame.iter().any(|line| line.contains("\x1b_G")));
    assert!(frame.iter().any(|line| line.contains("c=22")));
    assert!(frame.iter().any(|line| line.contains("r=12")));
    assert!(
        !frame
            .iter()
            .any(|line| line.contains("[image: image/png data="))
    );
}
