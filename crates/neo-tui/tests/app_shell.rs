use neo_tui::chrome::{
    ApprovalChoice, ChromeMode, CommandPaletteState, CommandSpec, ContextWindow, ModelPickerState,
    NeoChromeState, Overlay, OverlayKind, PickerItem, PromptEdit, SessionPickerItem,
    SessionPickerScope, SessionPickerState, StreamUpdate, ToolStatusKind,
};
use neo_tui::image::{ImageProtocolPreference, ImageRenderPolicy, TerminalImageCapabilities};
use neo_tui::transcript::{TranscriptPane, render_chrome_lines};
use std::path::PathBuf;

fn render_app(width: u16, app: &NeoChromeState) -> Vec<String> {
    render_chrome_lines(app, usize::from(width))
        .lines
        .into_iter()
        .map(|line| neo_tui::ansi::strip_ansi(&line))
        .collect()
}

fn render_transcript(width: usize, height: usize, transcript: &mut TranscriptPane) -> Vec<String> {
    transcript
        .render_frame(width, height)
        .expect("transcript frame")
        .into_iter()
        .map(|line| neo_tui::ansi::strip_ansi(&line))
        .collect()
}

#[test]
fn app_shell_renders_context_window_and_working_status() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));
    app.prompt_mut().apply_edit(PromptEdit::Insert("hello"));
    assert_eq!(app.submit_prompt(), Some("hello".to_owned()));

    let lines = render_app(100, &app);

    assert!(lines.iter().any(|line| line.contains("ctx 12k/200k")));
    assert!(lines.iter().any(|line| line.contains("working")));
}

#[test]
fn transcript_pane_renders_startup_banner() {
    let app = NeoChromeState::new("neo", "test-session", "openai/gpt-4.1", "/tmp/neo-ws");
    let mut runtime = TranscriptPane::new(80, 12);
    runtime.push_welcome_banner(
        app.title(),
        app.session_label(),
        app.model_label(),
        &app.cwd_label(),
        env!("CARGO_PKG_VERSION"),
        None,
    );

    let lines = render_transcript(80, 12, &mut runtime);

    assert!(lines.iter().any(|line| line.contains("Welcome to neo")));
    assert!(lines.iter().any(|line| line.contains("test-session")));
    assert!(lines.iter().any(|line| line.contains("openai/gpt-4.1")));
    assert!(lines.iter().any(|line| line.contains("/tmp/neo-ws")));
}

#[test]
fn app_shell_context_color_changes_by_threshold() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.set_context_window(Some(ContextWindow::new(100_000).with_used_tokens(50_000)));
    assert_eq!(app.context_color(), app.theme().footer_context_ok);

    app.set_context_window(Some(ContextWindow::new(100_000).with_used_tokens(75_000)));
    assert_eq!(app.context_color(), app.theme().footer_context_warn);

    app.set_context_window(Some(ContextWindow::new(100_000).with_used_tokens(95_000)));
    assert_eq!(app.context_color(), app.theme().footer_context_critical);
}

#[test]
fn app_shell_footer_has_two_lines_when_tall() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));

    let lines = render_app(100, &app);
    let last = lines.len().saturating_sub(1);
    let second_last = last.saturating_sub(1);

    assert!(lines[second_last].contains("[ask]"));
    assert!(lines[last].contains("enter send"));
    assert!(lines[second_last].contains("ctx 12k/200k"));
}

#[test]
fn app_shell_working_status_hides_running_tool_names_from_chrome() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_stream_update(StreamUpdate::ToolStarted {
        id: "tool-1".to_owned(),
        name: "shell.run".to_owned(),
        detail: "cargo test --workspace".to_owned(),
    });

    assert_eq!(
        app.working_label().as_deref(),
        Some("working · esc interrupt")
    );
    let lines = render_app(100, &app);
    assert!(!lines.iter().any(|line| line.contains("shell.run")));
    assert!(lines.iter().any(|line| line.contains("working")));
}

#[test]
fn app_shell_updates_context_usage_from_agent_event() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000)));

    app.apply_agent_event(neo_agent_core::AgentEvent::TokenUsage {
        turn: 1,
        usage: neo_agent_core::AgentTokenUsage {
            input_tokens: 123,
            output_tokens: 45,
        },
    });

    assert_eq!(
        app.context_window(),
        Some(ContextWindow::new(200_000).with_used_tokens(168))
    );
    let lines = render_app(100, &app);
    assert!(lines.iter().any(|line| line.contains("ctx 168/200k")));
}

#[test]
fn app_shell_maps_agent_core_approval_request_to_approval_overlay() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 7,
        id: "approval-7".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "shell.run".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test -p neo-tui" }),
    });

    assert_eq!(app.mode(), ChromeMode::Approval);
    assert_eq!(app.approval_choice(), Some(ApprovalChoice::Approve));
    assert!(matches!(
        app.focused_overlay().map(|overlay| &overlay.kind),
        Some(OverlayKind::Approval(modal))
            if modal.request_id == "approval-7"
                && modal.modal.title.contains("Tool")
                && modal.modal.body.contains("cargo test -p neo-tui")
    ));
}

#[test]
fn app_shell_renders_neo_branded_footer_and_boxed_composer_pinned_to_bottom() {
    let mut app = NeoChromeState::new("neo", "new", "anthropic/deepseek-v4-pro[1m]", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));
    app.prompt_mut().apply_edit(PromptEdit::Insert("/"));

    let lines = render_app(92, &app);
    let composer_row = lines
        .iter()
        .rposition(|line| line.contains("> /"))
        .expect("composer prompt renders");
    let status_row = lines
        .iter()
        .rposition(|line| line.contains("[ask]"))
        .expect("footer status line renders");
    let hint_row = lines
        .iter()
        .rposition(|line| line.contains("enter send"))
        .expect("footer hint line renders");

    assert!(lines.iter().any(|line| line.contains("shift+enter")));
    assert!(lines.iter().any(|line| line.contains("ctx 12k/200k")));
    let footer_lines = &lines[status_row.min(hint_row)..=status_row.max(hint_row)];
    assert!(!footer_lines.iter().any(|line| line.contains("neo  ")));
    assert!(!footer_lines.iter().any(|line| line.contains(" new ")));
    assert!(lines[composer_row.saturating_sub(1)].contains('╭'));
    assert!(status_row > composer_row);
}

#[test]
fn transcript_pane_frame_keeps_latest_live_row_visible() {
    let mut runtime = TranscriptPane::new(80, 12);
    runtime.set_live_chrome_height(5);
    for index in 0..36 {
        runtime.start_assistant_message();
        runtime.append_assistant_delta(&format!("history line {index}"));
    }

    let lines = render_transcript(80, 12, &mut runtime);

    assert!(lines.iter().any(|line| line.contains("history line 35")));
}

#[test]
fn transcript_pane_maps_shell_command_lifecycle_to_tool_run() {
    let mut runtime = TranscriptPane::new(100, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        command: "cargo test -p neo-tui".to_owned(),
        cwd: PathBuf::from("/workspace/neo"),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "shell-1".to_owned(),
        exit_code: Some(0),
        stdout: "ok".to_owned(),
        stderr: String::new(),
        truncated: false,
    });

    let entries = runtime.transcript().entries();
    assert!(matches!(
        entries.last(),
        Some(neo_tui::transcript::TranscriptEntry::ToolRun { component })
            if component.name() == "shell.run"
                && component.status() == ToolStatusKind::Succeeded
                && component.result().is_some_and(|result| result.contains("stdout: ok"))
    ));
    let lines = render_transcript(100, 12, &mut runtime);
    assert!(lines.iter().any(|line| line.contains("● Used shell.run")));
}

#[test]
fn transcript_pane_running_tool_call_is_rendered_before_finish() {
    let mut runtime = TranscriptPane::new(100, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "List".to_owned(),
        arguments: serde_json::json!({ "path": "crates/neo-tui/src" }),
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
fn transcript_pane_preserves_tool_arguments_separately_from_result() {
    let mut runtime = TranscriptPane::new(100, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "read".to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: r#"{"path":"README.md"}"#.to_owned(),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "read".to_owned(),
        result: neo_agent_core::ToolResult::ok("read README"),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::tool_result(
            "tool-1",
            "read",
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
            if component.name() == "read"
                && component.status() == ToolStatusKind::Succeeded
                && component.arguments() == Some(r#"{"path":"README.md"}"#)
                && component.result() == Some("read README")
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
            first_kept_message_index: 4,
        },
    });

    assert!(matches!(
        &runtime.transcript().entries()[0],
        neo_tui::transcript::TranscriptEntry::Status { text, .. }
            if text.contains("FollowUp queue drained")
    ));
    assert!(matches!(
        &runtime.transcript().entries()[1],
        neo_tui::transcript::TranscriptEntry::Compaction { compacted_message_count, tokens_before, .. }
            if *compacted_message_count == 4 && *tokens_before == 12_345
    ));
}

#[test]
fn transcript_pane_replays_thinking_tool_assistant_in_order() {
    let mut runtime = TranscriptPane::new(100, 20);
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-1".to_owned(),
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
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "List".to_owned(),
        result: neo_agent_core::ToolResult::ok("README.md"),
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-2".to_owned(),
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
fn transcript_pane_inline_images_are_structured_entries() {
    let mut runtime = TranscriptPane::new(100, 12);
    runtime.push_image(
        "image/png",
        &neo_agent_core::ImageRef::Base64("aGVsbG8=".to_owned()),
    );

    assert!(matches!(
        runtime.transcript().entries().last(),
        Some(neo_tui::transcript::TranscriptEntry::Image { mime_type, payload, .. })
            if mime_type == "image/png" && payload.is_some()
    ));

    let sequences = runtime.inline_image_sequences(
        ImageRenderPolicy::new(ImageProtocolPreference::Iterm2, false),
        TerminalImageCapabilities::default().with_iterm2(true),
    );
    assert_eq!(sequences.len(), 1);
}

#[test]
fn plan_mode_and_todo_events_remain_app_ui_state() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.apply_stream_update(StreamUpdate::PlanModeChanged { active: true });
    assert!(app.is_plan_mode());
    app.apply_stream_update(StreamUpdate::PlanModeChanged { active: false });
    assert!(!app.is_plan_mode());

    app.apply_agent_event(neo_agent_core::AgentEvent::TodoUpdated {
        turn: 1,
        todos: vec![neo_agent_core::TodoEventData {
            title: "ship".to_owned(),
            status: "in_progress".to_owned(),
        }],
    });
    assert!(app.has_todos());
}

#[test]
fn command_palette_filters_and_confirms_items() {
    let mut state = CommandPaletteState::new([
        CommandSpec::new("model", "Switch model", Some("Pick a model")),
        CommandSpec::new("resume", "Resume session", Some("Open history")),
    ]);
    state.set_filter("res");

    assert_eq!(
        state.selected_command().map(|cmd| cmd.id),
        Some("resume".to_owned())
    );
}

#[test]
fn session_picker_filters_scope_and_selection() {
    let mut picker = SessionPickerState::new(
        [
            SessionPickerItem::new(
                "alpha",
                "Alpha",
                Some("first prompt".to_owned()),
                PathBuf::from("/tmp/neo"),
                std::time::SystemTime::now(),
                true,
            ),
            SessionPickerItem::new(
                "beta",
                "Beta",
                Some("second prompt".to_owned()),
                PathBuf::from("/tmp/other"),
                std::time::SystemTime::now(),
                false,
            ),
        ],
        "alpha",
        SessionPickerScope::Workspace,
        4,
    );

    picker.set_filter("beta");
    assert_eq!(
        picker.confirm().map(|item| item.id),
        Some("beta".to_owned())
    );
    picker.set_scope(SessionPickerScope::All);
    assert_eq!(picker.scope(), SessionPickerScope::All);
}

#[test]
fn model_picker_confirms_selected_item() {
    let picker =
        ModelPickerState::new([PickerItem::new("openai/gpt-4.1", "GPT 4.1", Some("OpenAI"))]);

    assert_eq!(
        picker.confirm().map(|item| item.value),
        Some("openai/gpt-4.1".to_owned())
    );
}

#[test]
fn overlay_message_renders_plain_line() {
    let mut app = NeoChromeState::new("neo", "s", "m", "/tmp");
    app.push_overlay(Overlay::new(
        "message",
        OverlayKind::Message("hello".to_owned()),
    ));

    assert_eq!(app.focused_overlay_lines(80), vec!["hello".to_owned()]);
}
