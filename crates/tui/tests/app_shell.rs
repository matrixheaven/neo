use neo_tui::{
    AppMode, CommandPaletteState, CommandSpec, ModelPickerState, NeoTuiApp, Overlay, OverlayKind,
    PickerItem, SessionPickerState, StreamUpdate, TranscriptLine, TranscriptRenderer,
};
use ratatui::{Terminal, backend::TestBackend, buffer::Cell};
use std::path::PathBuf;

fn render_app(width: u16, height: u16, app: &NeoTuiApp) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend is valid");
    terminal
        .draw(|frame| frame.render_widget(app, frame.area()))
        .expect("app renders");
    terminal
        .backend()
        .buffer()
        .content
        .chunks(width as usize)
        .map(|line| line.iter().map(Cell::symbol).collect::<String>())
        .collect()
}

#[test]
fn app_shell_maps_agent_core_approval_request_to_approval_overlay() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1");

    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 7,
        id: "approval-7".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "shell.run".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test -p neo-tui" }),
    });

    assert_eq!(app.mode(), AppMode::Approval);
    assert_eq!(
        app.approval_choice(),
        Some(neo_tui::ApprovalChoice::Approve)
    );
    assert!(matches!(
        app.focused_overlay().map(|overlay| &overlay.kind),
        Some(OverlayKind::Approval(modal))
            if modal.request_id == "approval-7"
                && modal.modal.title.contains("Tool")
                && modal.modal.body.contains("cargo test -p neo-tui")
    ));
}

#[test]
fn app_shell_maps_agent_core_shell_command_lifecycle_to_tool_status() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1");

    app.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-1".to_owned(),
        command: "cargo test -p neo-tui".to_owned(),
        cwd: PathBuf::from("/workspace/neo"),
    });

    let statuses = app.tool_statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].name, "shell.run");
    assert_eq!(statuses[0].kind, neo_tui::ToolStatusKind::Running);
    assert!(statuses[0].detail.as_deref().is_some_and(|detail| {
        detail.contains("cargo test -p neo-tui") && detail.contains("/workspace/neo")
    }));

    app.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "shell-1".to_owned(),
        exit_code: Some(0),
        stdout: "ok".to_owned(),
        stderr: String::new(),
        truncated: false,
    });

    assert!(app.tool_statuses().is_empty());
    assert!(matches!(
        app.transcript().items().last(),
        Some(neo_tui::TranscriptItem::Tool {
            name,
            detail,
            status,
        }) if name == "shell.run"
            && detail.contains("exit 0")
            && detail.contains("stdout: ok")
            && status == &neo_tui::ToolStatusKind::Succeeded
    ));
}

#[test]
fn app_shell_merges_model_tool_call_and_execution_lifecycle() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1");

    app.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "read".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: r#"{"path":"README.md"}"#.to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "read".to_owned(),
        result: neo_agent_core::ToolResult::ok("read README"),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::tool_result(
            "tool-1",
            "read",
            [neo_agent_core::Content::text("read README")],
            false,
        ),
    });

    assert!(app.tool_statuses().is_empty());
    let tool_items = app
        .transcript()
        .items()
        .iter()
        .filter(|item| matches!(item, neo_tui::TranscriptItem::Tool { .. }))
        .count();
    assert_eq!(tool_items, 1);
    assert!(matches!(
        app.transcript().items().last(),
        Some(neo_tui::TranscriptItem::Tool {
            name,
            detail,
            status,
        }) if name == "read"
            && detail == "read README"
            && status == &neo_tui::ToolStatusKind::Succeeded
    ));
}

#[test]
fn app_shell_maps_agent_core_queue_and_compaction_events_to_notices() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1");

    app.apply_agent_event(neo_agent_core::AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::FollowUp,
        count: 2,
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::CompactionApplied {
        summary: neo_agent_core::CompactionSummary {
            summary: "Older context summarized.".to_owned(),
            tokens_before: 12_345,
            first_kept_message_index: 4,
        },
    });

    let notices: Vec<&str> = app
        .transcript()
        .items()
        .iter()
        .filter_map(|item| match item {
            neo_tui::TranscriptItem::Notice { content } => Some(content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(notices.len(), 2);
    assert!(notices[0].contains("FollowUp queue drained (2)"));
    assert!(notices[1].contains("Compaction applied"));
    assert!(notices[1].contains("12345 tokens before"));
}

#[test]
fn app_shell_reduces_agent_core_streaming_message_and_turn_events() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1");

    app.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Hel".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "lo".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });

    assert_eq!(app.mode(), AppMode::Editing);
    assert_eq!(app.active_assistant_id(), None);
    assert!(matches!(
        app.transcript().items().last(),
        Some(neo_tui::TranscriptItem::Assistant { content }) if content == "Hello"
    ));
}

#[test]
fn app_shell_records_submissions_and_streaming_updates() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1");

    assert_eq!(app.mode(), AppMode::Editing);
    app.prompt_mut()
        .apply_edit(neo_tui::PromptEdit::Insert("hello"));
    assert_eq!(app.submit_prompt(), Some("hello".to_owned()));
    assert_eq!(app.mode(), AppMode::Streaming);

    app.apply_stream_update(StreamUpdate::AssistantStarted {
        id: "msg-1".to_owned(),
    });
    app.apply_stream_update(StreamUpdate::TextDelta {
        text: "Hel".to_owned(),
    });
    app.apply_stream_update(StreamUpdate::TextDelta {
        text: "lo".to_owned(),
    });
    app.apply_stream_update(StreamUpdate::ToolStarted {
        id: "tool-1".to_owned(),
        name: "shell.run".to_owned(),
        detail: "cargo test".to_owned(),
    });
    app.apply_stream_update(StreamUpdate::ToolFinished {
        id: "tool-1".to_owned(),
        detail: "exit 0".to_owned(),
        success: true,
    });
    app.apply_stream_update(StreamUpdate::TurnFinished);

    assert_eq!(app.mode(), AppMode::Editing);
    assert_eq!(app.active_assistant_id(), None);
    assert_eq!(app.transcript().items().len(), 3);
    assert!(app.tool_statuses().is_empty());
    assert!(matches!(
        &app.transcript().items()[1],
        neo_tui::TranscriptItem::Assistant { content } if content == "Hello"
    ));
    assert!(matches!(
        &app.transcript().items()[2],
        neo_tui::TranscriptItem::Tool { status, detail, .. }
            if status == &neo_tui::ToolStatusKind::Succeeded && detail == "exit 0"
    ));
}

#[test]
fn app_loads_read_only_session_transcript_and_updates_label() {
    let mut app = NeoTuiApp::new("neo", "new", "openai/gpt-4.1");
    app.apply_stream_update(StreamUpdate::AssistantStarted {
        id: "in-flight".to_owned(),
    });
    app.apply_stream_update(StreamUpdate::ToolStarted {
        id: "tool-1".to_owned(),
        name: "shell.run".to_owned(),
        detail: "cargo test".to_owned(),
    });
    app.prompt_mut()
        .apply_edit(neo_tui::PromptEdit::Insert("draft"));

    app.load_session_transcript(
        "alpha (read-only)",
        ["compaction: older context summarized".to_owned()],
        [
            neo_agent_core::AgentMessage::user_text("hello"),
            neo_agent_core::AgentMessage::assistant(
                [neo_agent_core::Content::text("hi back")],
                Vec::new(),
                neo_agent_core::StopReason::EndTurn,
            ),
        ],
    );

    assert_eq!(app.session_label(), "alpha (read-only)");
    assert_eq!(app.mode(), AppMode::Editing);
    assert_eq!(app.active_assistant_id(), None);
    assert!(app.tool_statuses().is_empty());
    assert!(app.prompt().text.is_empty());
    assert_eq!(app.transcript().items().len(), 3);
    assert!(matches!(
        &app.transcript().items()[0],
        neo_tui::TranscriptItem::Notice { content }
            if content == "compaction: older context summarized"
    ));
    assert!(matches!(
        &app.transcript().items()[1],
        neo_tui::TranscriptItem::User { content } if content == "hello"
    ));
    assert!(matches!(
        &app.transcript().items()[2],
        neo_tui::TranscriptItem::Assistant { content } if content == "hi back"
    ));
}

#[test]
fn modal_stack_tracks_focus_and_restores_previous_overlay() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1");

    let first = app.push_overlay(Overlay::new(
        "palette",
        OverlayKind::CommandPalette(CommandPaletteState::new([CommandSpec::new(
            "open",
            "Open",
            Some("Open session"),
        )])),
    ));
    let second = app.push_overlay(Overlay::new(
        "models",
        OverlayKind::ModelPicker(ModelPickerState::new([PickerItem::new(
            "gpt-4.1",
            "GPT-4.1",
            Some("default"),
        )])),
    ));

    assert_eq!(app.focused_overlay_id(), Some(second));
    assert_eq!(app.mode(), AppMode::Overlay);

    app.focus_overlay(first);
    assert_eq!(app.focused_overlay_id(), Some(first));

    let removed = app.close_focused_overlay().expect("focused overlay closes");
    assert_eq!(removed.id, first);
    assert_eq!(app.focused_overlay_id(), Some(second));

    app.close_overlay(second);
    assert_eq!(app.focused_overlay_id(), None);
    assert_eq!(app.mode(), AppMode::Editing);
}

#[test]
fn command_palette_session_and_model_pickers_filter_and_select_values() {
    let mut palette = CommandPaletteState::new([
        CommandSpec::new("new-session", "New session", Some("Start clean")),
        CommandSpec::new("resume", "Resume", Some("Open an existing session")),
        CommandSpec::new("model", "Model", Some("Switch model")),
    ]);
    palette.set_filter("mo");
    assert_eq!(palette.selected_command().expect("command").id, "model");
    assert_eq!(palette.confirm().expect("confirmed").id, "model");

    let mut sessions = SessionPickerState::new([
        PickerItem::new("s1", "Session one", Some("today")),
        PickerItem::new("s2", "Long task", Some("yesterday")),
    ]);
    sessions.set_filter("long");
    assert_eq!(sessions.confirm().expect("session").value, "s2");

    let mut models = ModelPickerState::new([
        PickerItem::new("openai/gpt-4.1", "GPT-4.1", Some("balanced")),
        PickerItem::new("anthropic/claude-sonnet", "Claude Sonnet", Some("coding")),
    ]);
    models.set_filter("claude");
    assert_eq!(
        models.selected_model().expect("model").value,
        "anthropic/claude-sonnet"
    );

    let mut app = NeoTuiApp::new("neo", "new", "openai/gpt-4.1");
    app.open_session_picker([
        PickerItem::new("alpha", "Alpha", Some("first session")),
        PickerItem::new("beta", "Beta", Some("second session")),
    ]);
    app.move_overlay_selection_down();
    let selected = app
        .confirm_session_picker()
        .expect("selected session returned");
    assert_eq!(selected.value, "beta");
    assert!(app.focused_overlay().is_none());
}

#[test]
fn command_palette_session_and_model_pickers_page_selection() {
    let mut palette = CommandPaletteState::new((0..10).map(|index| {
        CommandSpec::new(
            format!("command-{index}"),
            format!("Command {index}"),
            None::<String>,
        )
    }));
    palette.page_down();
    assert_eq!(palette.selected_command().expect("command").id, "command-8");
    palette.page_down();
    assert_eq!(palette.selected_command().expect("command").id, "command-9");
    palette.page_up();
    assert_eq!(palette.selected_command().expect("command").id, "command-1");
    palette.page_up();
    assert_eq!(palette.selected_command().expect("command").id, "command-0");

    let mut sessions = SessionPickerState::new((0..10).map(|index| {
        PickerItem::new(
            format!("session-{index}"),
            format!("Session {index}"),
            None::<String>,
        )
    }));
    sessions.page_down();
    assert_eq!(sessions.confirm().expect("session").value, "session-8");

    let mut models = ModelPickerState::new((0..10).map(|index| {
        PickerItem::new(
            format!("provider/model-{index}"),
            format!("Model {index}"),
            None::<String>,
        )
    }));
    models.page_down();
    models.page_up();
    assert_eq!(
        models.selected_model().expect("model").value,
        "provider/model-0"
    );
}

#[test]
fn app_moves_focused_overlay_selection_by_page() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1");
    app.push_overlay(Overlay::new(
        "palette",
        OverlayKind::CommandPalette(CommandPaletteState::new((0..10).map(|index| {
            CommandSpec::new(
                format!("command-{index}"),
                format!("Command {index}"),
                None::<String>,
            )
        }))),
    ));

    app.move_overlay_selection_page_down();
    let Some(OverlayKind::CommandPalette(palette)) =
        app.focused_overlay().map(|overlay| &overlay.kind)
    else {
        panic!("expected command palette overlay");
    };
    assert_eq!(palette.selected_command().expect("command").id, "command-8");

    app.move_overlay_selection_page_up();
    let Some(OverlayKind::CommandPalette(palette)) =
        app.focused_overlay().map(|overlay| &overlay.kind)
    else {
        panic!("expected command palette overlay");
    };
    assert_eq!(palette.selected_command().expect("command").id, "command-0");
}

#[test]
fn approval_overlay_exposes_selected_decision_without_runtime_logic() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1");
    let request = app.request_approval(
        "approval-1",
        "Run command?",
        "cargo clippy -p neo-tui --all-targets",
    );

    assert_eq!(app.focused_overlay_id(), Some(request));
    assert_eq!(app.mode(), AppMode::Approval);
    assert_eq!(
        app.approval_choice(),
        Some(neo_tui::ApprovalChoice::Approve)
    );

    app.move_overlay_selection_down();
    assert_eq!(app.approval_choice(), Some(neo_tui::ApprovalChoice::Deny));

    let confirmed = app.confirm_approval().expect("approval confirmed");
    assert_eq!(confirmed.request_id, "approval-1");
    assert_eq!(confirmed.choice, neo_tui::ApprovalChoice::Deny);
    assert_eq!(app.mode(), AppMode::Editing);
}

#[test]
fn transcript_renderer_handles_markdownish_blocks_and_wrapping() {
    let renderer = TranscriptRenderer::new(28);
    let lines = renderer.render_markdownish(
        "# Plan\n- inspect files\n- run tests\n```rust\nfn main() {}\n```\nplain text wraps across the available terminal width",
    );

    assert!(lines.iter().any(|line| {
        matches!(
            line,
            TranscriptLine::Heading { level: 1, text } if text == "Plan"
        )
    }));
    assert!(lines.iter().any(|line| {
        matches!(
            line,
            TranscriptLine::ListItem { text, indent } if text == "inspect files" && *indent == 0
        )
    }));
    assert!(lines.iter().any(|line| {
        matches!(
            line,
            TranscriptLine::Code { text, language } if text == "fn main() {}" && language.as_deref() == Some("rust")
        )
    }));
    assert!(
        lines
            .iter()
            .all(|line| neo_tui::visible_width(line.text()) <= 28)
    );
}

#[test]
fn app_widget_renders_header_transcript_prompt_and_top_overlay() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1");
    app.transcript_mut().push(neo_tui::TranscriptItem::notice(
        "Welcome to neo terminal shell",
    ));
    app.push_overlay(Overlay::new(
        "palette",
        OverlayKind::CommandPalette(CommandPaletteState::new([
            CommandSpec::new("new-session", "New session", Some("Start clean")),
            CommandSpec::new("resume", "Resume", Some("Open session")),
        ])),
    ));

    let lines = render_app(64, 18, &app);

    assert!(lines.iter().any(|line| line.contains("neo")));
    assert!(lines.iter().any(|line| line.contains("session-a")));
    assert!(lines.iter().any(|line| line.contains("openai/gpt-4.1")));
    assert!(lines.iter().any(|line| line.contains("Welcome to neo")));
    assert!(lines.iter().any(|line| line.contains("Command Palette")));
    assert!(lines.iter().any(|line| line.contains("New session")));
}
