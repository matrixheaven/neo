use neo_tui::primitive::theme::ChromeMode;
use neo_tui::shell::{
    CommandPaletteState, CommandSpec, ContextWindow, ModelPickerState, NeoChromeState, PickerItem,
    PromptEdit, SessionPickerItem, SessionPickerScope, SessionPickerState, StreamUpdate,
};
use neo_tui::transcript::{TranscriptPane, render_chrome_lines};
use std::path::PathBuf;
use std::time::Instant;

fn render_app(width: u16, app: &NeoChromeState) -> Vec<String> {
    render_chrome_lines(app, usize::from(width), 30)
        .lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect()
}
fn todo_item(
    title: &str,
    status: neo_tui::widgets::TodoDisplayStatus,
) -> neo_tui::widgets::TodoDisplayItem {
    neo_tui::widgets::TodoDisplayItem::new(title, status)
}
fn render_transcript(width: usize, height: usize, transcript: &mut TranscriptPane) -> Vec<String> {
    transcript
        .render_frame(width, height)
        .expect("transcript frame")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect()
}
fn strip_lines(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect()
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
fn app_shell_explicit_animation_tick_animates_transcript_thinking_spinner() {
    let chrome = NeoChromeState::new("neo", "test-session", "model", "/tmp/neo-ws");
    let mut transcript = TranscriptPane::new(80, 20);
    transcript.push_transcript(neo_tui::transcript::TranscriptEntry::thinking_streaming(
        "working it out",
    ));
    let mut tui = neo_tui::NeoTui::new(chrome, transcript);

    let first = strip_lines(tui.render_frame(80, 20).0).join("\n");
    tui.advance_animation_at(Instant::now());
    let second = strip_lines(tui.render_frame(80, 20).0).join("\n");

    assert!(first.contains("⠋ thinking..."), "first frame: {first}");
    assert!(second.contains("⠙ thinking..."), "second frame: {second}");
}

#[test]
fn app_shell_mcp_startup_shows_interrupt_hint() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_mcp_startup_active(true);

    assert_eq!(
        app.working_label().as_deref(),
        Some("MCP connecting · esc to interrupt")
    );
    assert!(
        render_app(100, &app)
            .iter()
            .any(|line| line.contains("MCP connecting · esc to interrupt"))
    );
}

#[test]
fn app_shell_renders_context_window_and_working_status() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));
    app.prompt_mut().apply_edit(PromptEdit::Insert("hello"));
    assert_eq!(app.submit_prompt(), Some("hello".to_owned()));

    let lines = render_app(100, &app);

    assert!(
        lines
            .iter()
            .any(|line| line.contains("ctx ") && line.contains('/')),
        "should show context window info"
    );
    assert!(lines.iter().any(|line| line.contains("working")));
}

#[test]
fn app_shell_updates_context_usage_from_agent_event() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000)));

    app.apply_agent_event(neo_agent_core::AgentEvent::ContextWindowUpdated {
        turn: 1,
        used_tokens: 168,
        projected_tokens: None,
        max_tokens: None,
        trigger_tokens: None,
        remaining_tokens: None,
        source: None,
    });

    assert_eq!(
        app.context_window(),
        Some(ContextWindow::new(200_000).with_used_tokens(168))
    );
    let lines = render_app(100, &app);
    assert!(lines.iter().any(|line| line.contains("ctx 168/200k")));
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
fn empty_todo_events_reset_expanded_state() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_todo_items(
        (0..7)
            .map(|i| {
                todo_item(
                    &format!("agent-{i}"),
                    neo_tui::widgets::TodoDisplayStatus::Pending,
                )
            })
            .collect(),
    );
    app.set_todo_panel_expanded(true);
    app.apply_agent_event(neo_agent_core::AgentEvent::TodoUpdated {
        turn: 2,
        todos: vec![],
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::TodoUpdated {
        turn: 3,
        todos: (0..7)
            .map(|i| neo_agent_core::TodoEventData {
                title: format!("new-agent-{i}"),
                status: "pending".to_owned(),
            })
            .collect(),
    });

    let plain = render_app(80, &app).join("\n");

    assert!(plain.contains("\u{2026} +2 more"));
    assert!(plain.contains("ctrl+t to expand"));
    assert!(!plain.contains("new-agent-6"));

    app.set_todo_panel_expanded(true);
    app.apply_stream_update(StreamUpdate::TodoUpdated { todos: vec![] });
    app.apply_stream_update(StreamUpdate::TodoUpdated {
        todos: (0..7)
            .map(|i| {
                todo_item(
                    &format!("new-stream-{i}"),
                    neo_tui::widgets::TodoDisplayStatus::Pending,
                )
            })
            .collect(),
    });

    let plain = render_app(80, &app).join("\n");

    assert!(plain.contains("\u{2026} +2 more"));
    assert!(plain.contains("ctrl+t to expand"));
    assert!(!plain.contains("new-stream-6"));
}

#[test]
fn live_delegate_keeps_animation_deadline_when_live_surface_is_hidden() {
    let chrome = NeoChromeState::new("neo", "test-session", "model", "/tmp/neo-ws");
    let runtime = neo_agent_core::multi_agent::MultiAgentRuntime::new();
    let agent = runtime.start_foreground_delegate_for_test("live task");
    let mut transcript = TranscriptPane::new(80, 1);
    transcript.apply_agent_event(neo_agent_core::AgentEvent::DelegateStarted {
        turn: 1,
        agent,
        workflow_origin: None,
    });
    let mut tui = neo_tui::NeoTui::new(chrome, transcript);

    let frame = tui.render_terminal_frame_at(80, 1, Instant::now());

    assert!(
        frame.next_animation_deadline.is_some(),
        "live delegates must keep the refresh deadline even when the live surface has no rows"
    );
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
fn retry_keeps_working_mode_until_turn_finishes() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    let events = [
        neo_agent_core::AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 5,
            delay_ms: 12_000,
            error_code: "provider.transport_error".to_owned(),
            message: "error decoding response body".to_owned(),
        },
        neo_agent_core::AgentEvent::RetryStarted {
            turn: 1,
            retry: 1,
            max_retries: 5,
        },
        neo_agent_core::AgentEvent::RetryResumed { turn: 1, retry: 1 },
        neo_agent_core::AgentEvent::RetrySucceeded {
            turn: 1,
            retries_used: 1,
        },
        neo_agent_core::AgentEvent::RetryExhausted {
            turn: 1,
            retries_used: 5,
            error_code: "provider.transport_error".to_owned(),
            message: "error decoding response body".to_owned(),
        },
    ];

    for event in events {
        app.apply_agent_event(event);
        assert_eq!(app.mode(), ChromeMode::Streaming);
        assert_eq!(
            app.working_label().as_deref(),
            Some("working · esc interrupt")
        );
        let footer = render_app(100, &app).join("\n");
        assert!(!footer.contains("retry in"));
        assert!(!footer.contains("error decoding response body"));
    }

    app.apply_agent_event(neo_agent_core::AgentEvent::Error {
        turn: 1,
        message: "transport error: error decoding response body".to_owned(),
        code: Some("provider.transport_error".to_owned()),
        retry_after: None,
    });
    assert_eq!(app.mode(), ChromeMode::Streaming);
    assert_eq!(
        app.working_label().as_deref(),
        Some("working · esc interrupt")
    );

    app.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::Error,
    });
    assert_ne!(app.mode(), ChromeMode::Streaming);
    assert!(app.working_label().is_none());

    let mut ordinary = NeoChromeState::new("neo", "ordinary", "openai/gpt-4.1", "/tmp/neo-ws");
    ordinary.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 2,
        id: "ordinary-error".to_owned(),
    });
    ordinary.apply_agent_event(neo_agent_core::AgentEvent::Error {
        turn: 2,
        message: "terminal error".to_owned(),
        code: Some("provider.transport_error".to_owned()),
        retry_after: None,
    });
    assert_ne!(ordinary.mode(), ChromeMode::Streaming);
    assert!(ordinary.working_label().is_none());
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
fn todo_events_with_all_done_remain_visible_until_cleared() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.apply_agent_event(neo_agent_core::AgentEvent::TodoUpdated {
        turn: 1,
        todos: vec![neo_agent_core::TodoEventData {
            title: "ship".to_owned(),
            status: "done".to_owned(),
        }],
    });
    assert!(app.has_todos());

    app.apply_agent_event(neo_agent_core::AgentEvent::TodoUpdated {
        turn: 2,
        todos: vec![],
    });
    assert!(!app.has_todos());
}

#[test]
fn todo_panel_clear_resets_expanded_state() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_todo_items(
        (0..7)
            .map(|i| {
                todo_item(
                    &format!("task-{i}"),
                    neo_tui::widgets::TodoDisplayStatus::Pending,
                )
            })
            .collect(),
    );
    app.set_todo_panel_expanded(true);
    app.clear_todos();
    app.set_todo_items(
        (0..7)
            .map(|i| {
                todo_item(
                    &format!("new-{i}"),
                    neo_tui::widgets::TodoDisplayStatus::Pending,
                )
            })
            .collect(),
    );

    let plain = render_app(80, &app).join("\n");

    assert!(plain.contains("\u{2026} +2 more"));
    assert!(plain.contains("ctrl+t to expand"));
    assert!(!plain.contains("new-6"));
}

#[test]
fn todo_panel_expanded_state_renders_all_items_before_prompt() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_todo_items(
        (0..7)
            .map(|i| {
                todo_item(
                    &format!("task-{i}"),
                    neo_tui::widgets::TodoDisplayStatus::Pending,
                )
            })
            .collect(),
    );
    app.set_todo_panel_expanded(true);
    app.prompt_mut()
        .apply_edit(PromptEdit::Insert("next prompt"));

    let lines = render_app(80, &app);
    let plain = lines.join("\n");

    assert!(plain.contains("task-0"));
    assert!(plain.contains("task-6"));
    assert!(plain.contains("all 7 items \u{b7} ctrl+t to collapse"));
    assert!(plain.contains("next prompt"));
}

#[test]
fn todo_panel_offsets_prompt_start_row() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_todo_items(vec![neo_tui::widgets::TodoDisplayItem::new(
        "ship todo panel",
        neo_tui::widgets::TodoDisplayStatus::InProgress,
    )]);

    let chrome = render_chrome_lines(&app, 80, 30);

    assert_eq!(chrome.prompt_start_row, 3);
    assert!(
        chrome.lines[chrome.prompt_start_row].contains("╭")
            || chrome.lines[chrome.prompt_start_row].contains("┌"),
        "lines: {:?}",
        chrome.lines
    );
}

#[test]
fn todo_panel_renders_before_prompt() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_todo_items(vec![neo_tui::widgets::TodoDisplayItem::new(
        "ship todo panel",
        neo_tui::widgets::TodoDisplayStatus::InProgress,
    )]);
    app.prompt_mut()
        .apply_edit(PromptEdit::Insert("next prompt"));

    let lines = render_app(80, &app);
    let todo = lines
        .iter()
        .position(|line| line.contains("ship todo panel"))
        .expect("todo row");
    let prompt = lines
        .iter()
        .position(|line| line.contains("next prompt"))
        .expect("prompt row");

    assert!(todo < prompt, "lines: {lines:?}");
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
fn workflow_picker_exposes_the_search_cursor_to_the_terminal() {
    let mut app = NeoChromeState::new("neo", "session-a", "model", "/tmp/neo-ws");
    app.open_workflow_picker(neo_tui::dialogs::WorkflowPickerOptions {
        items: vec![neo_tui::dialogs::WorkflowPickerItem {
            name: "review".to_owned(),
            display_name: "Review".to_owned(),
            description: "Review code".to_owned(),
            source: "Built-in".to_owned(),
            required_inputs: Vec::new(),
        }],
        theme: neo_tui::primitive::theme::TuiTheme::default(),
    });

    let mut tui = neo_tui::NeoTui::new(app, TranscriptPane::new(80, 20));
    let (lines, cursor) = tui.render_frame(80, 20);
    let plain = strip_lines(lines);
    let search_row = plain
        .iter()
        .position(|line| line.contains("Search"))
        .expect("workflow search row");
    let search_byte = plain[search_row].find("Search").expect("search label");
    let search_col = neo_tui::primitive::visible_width(&plain[search_row][..search_byte])
        + neo_tui::primitive::visible_width("Search  ");

    assert_eq!(
        cursor,
        Some(neo_tui::screen_output::CursorPos {
            row: search_row,
            col: search_col,
        })
    );
}
