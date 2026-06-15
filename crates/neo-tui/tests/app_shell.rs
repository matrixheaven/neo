use neo_tui::{
    AppMode, CommandPaletteState, CommandSpec, ContextWindow, ImageProtocolPreference,
    ImageRenderPolicy, ModelPickerState, NeoTuiApp, Overlay, OverlayKind, PickerItem, Rect,
    SessionPickerState, StreamUpdate, TerminalImageCapabilities, TranscriptLine,
    TranscriptRenderer, runtime_chrome_ansi_lines,
};
use std::path::PathBuf;

fn render_app(width: u16, height: u16, app: &NeoTuiApp) -> Vec<String> {
    let (mut transcript, chrome) = render_runtime_shell(width, height, app);
    transcript.extend(chrome);
    transcript
}

fn render_runtime_shell(width: u16, height: u16, app: &NeoTuiApp) -> (Vec<String>, Vec<String>) {
    let mut runtime = neo_tui::NeoTuiRuntime::new(width.into(), height.into());
    let layout = neo_tui::app_layout(app, Rect::new(0, 0, width, height));
    runtime.set_live_chrome_height(usize::from(
        layout
            .todo
            .height
            .saturating_add(layout.approval.height)
            .saturating_add(layout.session_picker.height)
            .saturating_add(layout.prompt.height)
            .saturating_add(layout.footer.height),
    ));

    for item in app.transcript().items() {
        push_runtime_item(&mut runtime, item);
    }
    if runtime.transcript().live_entries().is_empty() && app.tool_statuses().is_empty() {
        runtime.request_render(neo_tui::RenderKind::Incremental);
    }
    runtime.render_tick();

    let transcript = runtime
        .frame_ansi_lines()
        .into_iter()
        .map(|line| neo_tui::ansi::strip_ansi(&line))
        .collect();
    let chrome = runtime_chrome_ansi_lines(app, usize::from(width))
        .0
        .into_iter()
        .map(|line| neo_tui::ansi::strip_ansi(&line))
        .collect();
    (transcript, chrome)
}

fn push_runtime_item(runtime: &mut neo_tui::NeoTuiRuntime, item: &neo_tui::TranscriptItem) {
    match item {
        neo_tui::TranscriptItem::User { content } => runtime.push_user_message(content),
        neo_tui::TranscriptItem::Assistant { content, .. } => {
            runtime.push_assistant_final(content.clone())
        }
        neo_tui::TranscriptItem::Tool {
            name,
            status,
            tool_run,
            ..
        } => {
            runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
                turn: 0,
                id: runtime_tool_id(tool_run),
                name: name.clone(),
                arguments: runtime_tool_arguments(tool_run),
            });
            if !tool_run.live_output.is_empty() {
                runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
                    turn: 0,
                    id: runtime_tool_id(tool_run),
                    name: name.clone(),
                    partial_result: neo_agent_core::ToolResult::ok(tool_run.live_output.join("\n")),
                });
            }
            if matches!(
                status,
                neo_tui::ToolStatusKind::Succeeded
                    | neo_tui::ToolStatusKind::Failed
                    | neo_tui::ToolStatusKind::Cancelled
            ) {
                runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
                    turn: 0,
                    id: runtime_tool_id(tool_run),
                    name: name.clone(),
                    result: neo_agent_core::ToolResult {
                        content: tool_run.result.clone().unwrap_or_default(),
                        is_error: *status != neo_tui::ToolStatusKind::Succeeded,
                        details: tool_run.details.clone(),
                        terminate: false,
                    },
                });
            }
        }
        neo_tui::TranscriptItem::Image { metadata, .. } => runtime.append_notice(metadata.clone()),
        neo_tui::TranscriptItem::Compaction {
            compacted_message_count,
            tokens_before,
            ..
        } => runtime.append_notice(format!(
            "Compacted {compacted_message_count} messages ({tokens_before} tokens)"
        )),
        neo_tui::TranscriptItem::Notice { content } => runtime.append_notice(content.clone()),
        neo_tui::TranscriptItem::Banner {
            title,
            session_label,
            model_label,
            workspace_root,
        } => {
            runtime.push_banner(title.clone());
            runtime.append_notice(format!("Session: {session_label}"));
            runtime.append_notice(format!("Model: {model_label}"));
            runtime.append_notice(format!("Workspace: {}", workspace_root.display()));
        }
    }
}

fn runtime_tool_id(tool_run: &neo_tui::ToolRunTranscript) -> String {
    format!(
        "{}:{}",
        tool_run.name,
        tool_run.arguments.as_deref().unwrap_or_default()
    )
}

fn runtime_tool_arguments(tool_run: &neo_tui::ToolRunTranscript) -> serde_json::Value {
    tool_run
        .arguments
        .as_deref()
        .and_then(|arguments| serde_json::from_str(arguments).ok())
        .unwrap_or_else(|| {
            serde_json::Value::String(tool_run.arguments.clone().unwrap_or_default())
        })
}

#[test]
fn app_shell_renders_context_window_and_working_status() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));
    app.prompt_mut()
        .apply_edit(neo_tui::PromptEdit::Insert("hello"));
    assert_eq!(app.submit_prompt(), Some("hello".to_owned()));

    let lines = render_app(100, 12, &app);

    assert!(lines.iter().any(|line| line.contains("ctx 12k/200k")));
    assert!(lines.iter().any(|line| line.contains("● working")));
}

#[test]
fn app_shell_renders_startup_banner() {
    let mut app = NeoTuiApp::new("neo", "test-session", "openai/gpt-4.1", "/tmp/neo-ws");
    app.transcript_mut().push(neo_tui::TranscriptItem::banner(
        "Welcome to neo",
        "test-session",
        "openai/gpt-4.1",
        "/tmp/neo-ws",
    ));

    let lines = render_app(80, 12, &app);

    assert!(lines.iter().any(|line| line.contains("Welcome to neo")));
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Session: test-session"))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Model: openai/gpt-4.1"))
    );
    assert!(lines.iter().any(|line| line.contains("Workspace:")));
}

#[test]
fn app_shell_context_color_changes_by_threshold() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.set_context_window(Some(ContextWindow::new(100_000).with_used_tokens(50_000)));
    assert_eq!(app.context_color(), app.theme().footer_context_ok);

    app.set_context_window(Some(ContextWindow::new(100_000).with_used_tokens(75_000)));
    assert_eq!(app.context_color(), app.theme().footer_context_warn);

    app.set_context_window(Some(ContextWindow::new(100_000).with_used_tokens(95_000)));
    assert_eq!(app.context_color(), app.theme().footer_context_critical);
}

#[test]
fn app_shell_footer_has_two_lines_when_tall() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));

    let lines = render_app(100, 12, &app);
    let last = lines.len().saturating_sub(1);
    let second_last = last.saturating_sub(1);

    assert!(
        lines[second_last].contains("[ask]"),
        "status line should be the second-to-last row:\n{}",
        lines.join("\n")
    );
    assert!(
        lines[last].contains("enter send"),
        "hint line should be the last row:\n{}",
        lines.join("\n")
    );
    assert!(
        lines[second_last].contains("ctx 12k/200k"),
        "context label should render on the status line:\n{}",
        lines.join("\n")
    );
}

#[test]
fn app_shell_renders_chrome_prompt_and_footer_lines_on_short_terminal() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));

    let (_, chrome) = render_runtime_shell(100, 10, &app);

    assert!(chrome.len() >= 5);
    let footer_row = &chrome[chrome.len() - 2];
    let hint_row = &chrome[chrome.len() - 1];
    assert!(footer_row.contains("[ask]"));
    assert!(footer_row.contains("ctx 12k/200k"));
    assert!(hint_row.contains("enter send"));
    assert!(hint_row.contains("/ commands"));
    assert!(chrome.iter().any(|line| line.starts_with("┌")));
}

#[test]
fn app_shell_working_status_hides_running_tool_names_from_chrome() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_stream_update(StreamUpdate::ToolStarted {
        id: "tool-1".to_owned(),
        name: "shell.run".to_owned(),
        detail: "cargo test --workspace".to_owned(),
    });

    assert_eq!(
        app.working_label().as_deref(),
        Some("working · esc interrupt")
    );
    let lines = render_app(100, 12, &app);
    assert!(!lines.iter().any(|line| line.contains("running shell.run")));
    assert!(!lines.iter().any(|line| line.contains("shell.run running")));
    assert!(lines.iter().any(|line| line.contains("● Using shell.run")));
}

#[test]
fn app_shell_updates_context_usage_from_agent_event() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
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
    let lines = render_app(100, 12, &app);
    assert!(lines.iter().any(|line| line.contains("ctx 168/200k")));
}

#[test]
fn app_shell_maps_agent_core_approval_request_to_approval_overlay() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

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
fn app_shell_renders_neo_branded_footer_and_boxed_composer_pinned_to_bottom() {
    let mut app = NeoTuiApp::new("neo", "new", "anthropic/deepseek-v4-pro[1m]", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));
    app.transcript_mut()
        .push(neo_tui::TranscriptItem::assistant("Ready"));
    app.prompt_mut()
        .apply_edit(neo_tui::PromptEdit::Insert("/"));

    let lines = render_app(92, 18, &app);
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

    assert!(
        composer_row >= lines.len().saturating_sub(4),
        "composer should stay pinned near bottom, got row {composer_row}"
    );
    assert!(
        hint_row > composer_row,
        "footer hint line should sit below composer, composer={composer_row}, hint={hint_row}"
    );
    assert!(
        lines.iter().any(|line| line.contains("shift+enter")),
        "footer should advertise compact keyboard hints"
    );
    assert!(lines.iter().any(|line| line.contains("ctx 12k/200k")));
    let footer_lines = &lines[status_row.min(hint_row)..=status_row.max(hint_row)];
    assert!(!footer_lines.iter().any(|line| line.contains("neo  ")));
    assert!(
        !footer_lines.iter().any(|line| line.contains(" new ")),
        "session label should not leak into footer"
    );
    assert!(
        lines[composer_row.saturating_sub(1)].contains('┌')
            || lines[composer_row.saturating_sub(1)].contains('─'),
        "composer should render inside a bordered input box"
    );
    assert!(status_row > composer_row);
}

#[test]
fn app_shell_runtime_frame_keeps_latest_live_row_visible() {
    // The runtime no longer clamps the live region: in the pi-tui single-buffer
    // model, the InlineRenderer decides what fits the viewport and scrolls the
    // rest into scrollback. The runtime just composes the full body. This test
    // verifies the latest streaming row is present in the composed frame so the
    // renderer can keep the tail visible.
    let mut runtime = neo_tui::NeoTuiRuntime::new(80, 12);
    runtime.set_live_chrome_height(5);
    for index in 0..36 {
        runtime.start_assistant_message();
        runtime.append_assistant_delta(&format!("history line {index}"));
    }

    let lines: Vec<String> = runtime
        .render_frame(80, 12)
        .expect("render frame")
        .iter()
        .map(|line| neo_tui::ansi::strip_ansi(line))
        .collect();

    assert!(
        lines.iter().any(|line| line.contains("history line 35")),
        "latest runtime live row should be present in the composed frame:\n{}",
        lines.join("\n")
    );
}

#[test]
fn app_shell_maps_agent_core_shell_command_lifecycle_to_tool_status() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

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
            tool_run,
        }) if name == "shell.run"
            && detail.contains("exit 0")
            && detail.contains("stdout: ok")
            && status == &neo_tui::ToolStatusKind::Succeeded
            && tool_run.arguments.as_deref().is_some_and(|arguments| {
                arguments.contains("cargo test -p neo-tui")
                    && arguments.contains("/workspace/neo")
            })
            && tool_run.result.as_deref().is_some_and(|result| {
                result.contains("exit 0") && result.contains("stdout: ok")
            })
            && tool_run.metadata.exit_code == Some(0)
    ));
}

#[test]
fn running_tool_call_is_rendered_in_transcript_before_finish() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "list".to_owned(),
        arguments: serde_json::json!({ "path": "crates/neo-tui/src" }),
    });

    assert_eq!(
        app.transcript()
            .items()
            .iter()
            .filter(|item| matches!(item, neo_tui::TranscriptItem::Tool { .. }))
            .count(),
        1
    );
    assert!(matches!(
        app.transcript().items().last(),
        Some(neo_tui::TranscriptItem::Tool {
            name,
            status,
            tool_run,
            ..
        }) if name == "list"
            && status == &neo_tui::ToolStatusKind::Running
            && tool_run.arguments.as_deref().is_some_and(|arguments| {
                arguments.contains("crates/neo-tui/src")
            })
    ));

    let lines = render_app(100, 12, &app);
    assert!(lines.iter().any(|line| line.contains("● Using list")));
    assert!(!lines.iter().any(|line| line.contains("list running")));
}

#[test]
fn stream_updates_do_not_force_tail_when_transcript_is_detached() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    for index in 0..24 {
        app.transcript_mut()
            .push(neo_tui::TranscriptItem::notice(format!(
                "history line {index}"
            )));
    }
    app.sync_transcript_view_for_area(Rect::new(0, 0, 80, 12));
    app.scroll_transcript_up(6);
    let before = app.transcript_view().scrollback();

    app.apply_stream_update(StreamUpdate::TextDelta {
        text: "new streamed content".to_owned(),
    });
    app.apply_stream_update(StreamUpdate::ToolStarted {
        id: "tool-1".to_owned(),
        name: "list".to_owned(),
        detail: r#"{"path":"."}"#.to_owned(),
    });
    app.apply_stream_update(StreamUpdate::ToolFinished {
        id: "tool-1".to_owned(),
        detail: "done".to_owned(),
        success: true,
        details: None,
    });

    assert_eq!(app.transcript_view().scrollback(), before);
}

#[test]
fn app_shell_preserves_tool_arguments_separately_from_result() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

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
            tool_run,
        }) if name == "read"
            && detail == "read README"
            && status == &neo_tui::ToolStatusKind::Succeeded
            && tool_run.name == "read"
            && tool_run.arguments.as_deref() == Some(r#"{"path":"README.md"}"#)
            && tool_run.result.as_deref() == Some("read README")
    ));
}

#[test]
fn app_shell_failed_shell_transcript_keeps_exit_code_metadata() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shell-2".to_owned(),
        command: "false".to_owned(),
        cwd: PathBuf::from("/workspace/neo"),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "shell-2".to_owned(),
        exit_code: Some(2),
        stdout: String::new(),
        stderr: "nope".to_owned(),
        truncated: true,
    });

    assert!(matches!(
        app.transcript().items().last(),
        Some(neo_tui::TranscriptItem::Tool {
            name,
            detail,
            status,
            tool_run,
        }) if name == "shell.run"
            && status == &neo_tui::ToolStatusKind::Failed
            && detail.contains("exit 2")
            && detail.contains("stderr: nope")
            && detail.contains("truncated")
            && tool_run.result.as_deref().is_some_and(|result| {
                result.contains("exit 2") && result.contains("stderr: nope")
            })
            && tool_run.metadata.exit_code == Some(2)
            && tool_run.metadata.stderr.as_deref() == Some("nope")
            && tool_run.metadata.truncated
    ));
}

#[test]
fn app_shell_maps_agent_core_queue_notice_and_compaction_boundary() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

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
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("FollowUp queue drained (2)"));
    assert!(matches!(
        app.transcript().items()[1],
        neo_tui::TranscriptItem::Compaction {
            compacted_message_count: 4,
            tokens_before: 12_345,
            ..
        }
    ));
}

#[test]
fn app_shell_updates_compaction_progress_in_place() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.apply_agent_event(neo_agent_core::AgentEvent::CompactionStarted {
        reason: neo_agent_core::CompactionReason::Threshold,
        tokens_before: 12_345,
        message_count: 8,
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::CompactionProgress {
        phase: neo_agent_core::CompactionPhase::Summarizing,
        percent: 70,
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::CompactionApplied {
        summary: neo_agent_core::CompactionSummary {
            summary: "Older context summarized.".to_owned(),
            tokens_before: 12_345,
            first_kept_message_index: 4,
        },
    });

    assert_eq!(app.transcript().items().len(), 1);
    assert!(matches!(
        app.transcript().items()[0],
        neo_tui::TranscriptItem::Compaction {
            phase: Some(neo_agent_core::CompactionPhase::Applying),
            percent: 100,
            compacted_message_count: 4,
            tokens_before: 12_345
        }
    ));
}

#[test]
fn app_shell_reduces_agent_core_streaming_message_and_turn_events() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

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
        Some(neo_tui::TranscriptItem::Assistant { content, .. }) if content == "Hello"
    ));
}

#[test]
fn app_shell_reduces_agent_core_thinking_events_without_polluting_answer_text() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-1".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "Checked ".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "the plan.".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Final answer".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });

    assert_eq!(app.mode(), AppMode::Editing);
    assert!(matches!(
        &app.transcript().items()[0],
        neo_tui::TranscriptItem::Assistant { thinking, content }
            if thinking.as_deref() == Some("Checked the plan.") && content == "Final answer"
    ));
    assert_eq!(app.transcript().items().len(), 1);
    assert!(!app.transcript().items().iter().any(|item| matches!(
        item,
        neo_tui::TranscriptItem::Notice { content } if content.contains("Thinking")
    )));
}

#[test]
fn app_shell_deduplicates_echoed_user_messages_for_active_turn() {
    let mut app = NeoTuiApp::new(
        "neo",
        "session-a",
        "anthropic/deepseek-v4-pro[1m]",
        "/tmp/neo-ws",
    );

    app.prompt_mut()
        .apply_edit(neo_tui::PromptEdit::Insert("你好"));
    assert_eq!(app.submit_prompt(), Some("你好".to_owned()));
    app.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::user_text("你好"),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::user_text("你好"),
    });

    let user_messages = app
        .transcript()
        .items()
        .iter()
        .filter(|item| {
            matches!(
                item,
                neo_tui::TranscriptItem::User { content } if content == "你好"
            )
        })
        .count();
    assert_eq!(user_messages, 1);
}

#[test]
fn app_shell_records_submissions_and_streaming_updates() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

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
        details: None,
    });
    app.apply_stream_update(StreamUpdate::TurnFinished);

    assert_eq!(app.mode(), AppMode::Editing);
    assert_eq!(app.active_assistant_id(), None);
    assert_eq!(app.transcript().items().len(), 3);
    assert!(app.tool_statuses().is_empty());
    assert!(matches!(
        &app.transcript().items()[1],
        neo_tui::TranscriptItem::Assistant { content, .. } if content == "Hello"
    ));
    assert!(matches!(
        &app.transcript().items()[2],
        neo_tui::TranscriptItem::Tool { status, detail, .. }
            if status == &neo_tui::ToolStatusKind::Succeeded && detail == "exit 0"
    ));
}

#[test]
fn app_loads_session_transcript_and_updates_label() {
    let mut app = NeoTuiApp::new("neo", "new", "openai/gpt-4.1", "/tmp/neo-ws");
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
        "alpha",
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

    assert_eq!(app.session_label(), "alpha");
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
        neo_tui::TranscriptItem::Assistant { content, .. } if content == "hi back"
    ));
}

#[test]
fn app_shell_renders_agent_core_image_content_as_safe_metadata_summary() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    let large_base64 = "a".repeat(256);

    app.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::assistant(
            [neo_agent_core::Content::Image {
                mime_type: "image/png".to_owned(),
                data: neo_agent_core::ImageRef::Url("https://example.test/cat.png".to_owned()),
            }],
            Vec::new(),
            neo_agent_core::StopReason::EndTurn,
        ),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::assistant(
            [
                neo_agent_core::Content::text("see attached"),
                neo_agent_core::Content::Image {
                    mime_type: "image/jpeg".to_owned(),
                    data: neo_agent_core::ImageRef::Base64(large_base64.clone()),
                },
            ],
            Vec::new(),
            neo_agent_core::StopReason::EndTurn,
        ),
    });

    assert_eq!(app.transcript().items().len(), 3);
    assert!(matches!(
        &app.transcript().items()[0],
        neo_tui::TranscriptItem::Image { mime_type, size_bytes, source, metadata, payload, .. }
            if mime_type == "image/png"
                && size_bytes.is_none()
                && *source == neo_tui::ImageSource::RemoteUrl
                && metadata == "[image: image/png url=https://example.test/cat.png]"
                && payload.is_none()
    ));
    assert!(matches!(
        &app.transcript().items()[1],
        neo_tui::TranscriptItem::Assistant { content, .. }
            if content == "see attached"
    ));
    assert!(matches!(
        &app.transcript().items()[2],
        neo_tui::TranscriptItem::Image { mime_type, size_bytes, source, metadata, payload, .. }
            if mime_type == "image/jpeg"
                && *size_bytes == payload.as_ref().map(Vec::len)
                && *source == neo_tui::ImageSource::Base64
                && metadata == "[image: image/jpeg data=192 bytes]"
                && payload.as_ref().is_some_and(|bytes| bytes.len() <= large_base64.len())
    ));

    let rendered = render_app(80, 12, &app).join("\n");
    assert!(rendered.contains("[image: image/png url=https://example.test/cat.png]"));
    assert!(rendered.contains("see attached"));
    assert!(rendered.contains("[image: image/jpeg data=192 bytes]"));
    assert!(!rendered.contains(&large_base64));
}

#[test]
fn app_shell_stores_agent_core_images_as_sanitized_transcript_items() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    let encoded = "iVBORw0KGgo=".to_owned();

    app.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::assistant(
            [
                neo_agent_core::Content::text("generated preview"),
                neo_agent_core::Content::Image {
                    mime_type: "image/png".to_owned(),
                    data: neo_agent_core::ImageRef::Base64(encoded.clone()),
                },
                neo_agent_core::Content::Image {
                    mime_type: "image/jpeg".to_owned(),
                    data: neo_agent_core::ImageRef::Url(
                        "https://example.test/private.jpg?token=secret".to_owned(),
                    ),
                },
            ],
            Vec::new(),
            neo_agent_core::StopReason::EndTurn,
        ),
    });

    assert_eq!(app.transcript().items().len(), 3);
    assert!(matches!(
        &app.transcript().items()[0],
        neo_tui::TranscriptItem::Assistant { content, .. } if content == "generated preview"
    ));
    assert!(matches!(
        &app.transcript().items()[1],
        neo_tui::TranscriptItem::Image { id, mime_type, size_bytes, alt, source, .. }
            if id == "image-1"
                && mime_type == "image/png"
                && *size_bytes == Some(8)
                && alt.is_none()
                && *source == neo_tui::ImageSource::Base64
    ));
    assert!(matches!(
        &app.transcript().items()[2],
        neo_tui::TranscriptItem::Image { id, mime_type, size_bytes, alt, source, metadata, .. }
            if id == "image-2"
                && mime_type == "image/jpeg"
                && size_bytes.is_none()
                && alt.is_none()
                && *source == neo_tui::ImageSource::RemoteUrl
                && metadata.contains("https://example.test/private.jpg")
                && !metadata.contains("secret")
                && !metadata.contains("token=")
    ));
}

#[test]
fn app_shell_renders_byte_backed_images_with_negotiated_terminal_protocol() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_image_render_policy(ImageRenderPolicy::new(
        ImageProtocolPreference::Kitty,
        false,
    ));
    app.set_image_capabilities(TerminalImageCapabilities::default().with_kitty(true));

    app.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::assistant(
            [neo_agent_core::Content::Image {
                mime_type: "image/png".to_owned(),
                data: neo_agent_core::ImageRef::Base64("iVBORw0KGgo=".to_owned()),
            }],
            Vec::new(),
            neo_agent_core::StopReason::EndTurn,
        ),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::assistant(
            [neo_agent_core::Content::Image {
                mime_type: "image/png".to_owned(),
                data: neo_agent_core::ImageRef::Url(
                    "https://example.test/private.png?token=secret".to_owned(),
                ),
            }],
            Vec::new(),
            neo_agent_core::StopReason::EndTurn,
        ),
    });

    let image_sequences = app.inline_image_sequences();
    assert_eq!(image_sequences.len(), 1);
    assert!(image_sequences[0].starts_with("\x1b_G"));

    let rendered = render_app(96, 12, &app).join("\n");
    assert!(!rendered.contains("\x1b_G"));
    assert!(rendered.contains("[image: image/png data=8 bytes]"));
    assert!(rendered.contains("[image: image/png url=https://example.test/private.png]"));
    assert!(!rendered.contains("token=secret"));
}

#[test]
fn modal_stack_tracks_focus_and_restores_previous_overlay() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

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

    let mut app = NeoTuiApp::new("neo", "new", "openai/gpt-4.1", "/tmp/neo-ws");
    app.open_command_palette([
        CommandSpec::new("sessions", "Sessions", Some("Open sessions")),
        CommandSpec::new("models", "Models", Some("Open models")),
    ]);
    app.move_overlay_selection_down();
    let selected = app
        .confirm_command_palette()
        .expect("selected command returned");
    assert_eq!(selected.id, "models");
    assert!(app.focused_overlay().is_none());

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

    app.open_model_picker([
        PickerItem::new("openai/gpt-4.1", "openai/gpt-4.1", Some("responses")),
        PickerItem::new("anthropic/claude", "anthropic/claude", Some("messages")),
    ]);
    app.move_overlay_selection_down();
    let selected = app.confirm_model_picker().expect("selected model returned");
    app.set_model_label(selected.label.clone());
    assert_eq!(selected.value, "anthropic/claude");
    assert_eq!(app.model_label(), "anthropic/claude");
    assert!(app.focused_overlay().is_none());
}

#[test]
fn prompt_completion_overlay_confirms_selected_replacement() {
    let mut app = NeoTuiApp::new("neo", "new", "openai/gpt-4.1", "/tmp/neo-ws");
    app.prompt_mut()
        .apply_edit(neo_tui::PromptEdit::Insert("open src/ma"));
    let prefix = app
        .prompt()
        .completion_prefix()
        .expect("prompt has completion prefix");

    app.open_prompt_completion_picker(
        prefix,
        [
            PickerItem::new("src/main.rs", "src/main.rs", Some("file")),
            PickerItem::new("src/modes/", "src/modes/", Some("directory")),
        ],
    );
    assert_eq!(app.mode(), AppMode::Overlay);
    assert!(matches!(
        app.focused_overlay().map(|overlay| &overlay.kind),
        Some(OverlayKind::PromptCompletion(_))
    ));

    let selected = app
        .confirm_prompt_completion()
        .expect("selected completion returned");
    assert_eq!(selected.value, "src/main.rs");
    assert_eq!(app.prompt().text, "open src/main.rs");
    assert_eq!(app.prompt().cursor, 16);
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
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
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
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
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
            TranscriptLine::ListItem { text, indent, .. } if text == "inspect files" && *indent == 0
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
fn transcript_renderer_preserves_ordered_markers_and_task_states() {
    let renderer = TranscriptRenderer::new(40);
    let lines = renderer.render_markdownish("1. inspect\n2. implement\n- [ ] verify\n- [x] ship");
    let displayed = lines
        .iter()
        .map(TranscriptLine::display_text)
        .collect::<Vec<_>>();

    assert!(displayed.iter().any(|line| line == "1. inspect"));
    assert!(displayed.iter().any(|line| line == "2. implement"));
    assert!(displayed.iter().any(|line| line == "○ verify"));
    assert!(displayed.iter().any(|line| line == "✓ ship"));
}

#[test]
fn transcript_renderer_renders_markdown_tables_without_separator_rows() {
    let renderer = TranscriptRenderer::new(80);
    let lines = renderer.render_markdownish(
        "| File | Change |\n| --- | --- |\n| app.rs | remove footer tool status |\n| components.rs | render tool row |",
    );
    let displayed = lines
        .iter()
        .map(TranscriptLine::display_text)
        .collect::<Vec<_>>();

    assert!(displayed.iter().any(|line| line.contains("File")));
    assert!(displayed.iter().any(|line| line.contains("app.rs")));
    assert!(!displayed.iter().any(|line| line.contains("---")));
    assert!(displayed.iter().all(|line| !line.starts_with('|')));
}

#[test]
fn transcript_renderer_classifies_unified_diff_blocks() {
    let renderer = TranscriptRenderer::new(17);
    let lines = renderer.render_markdownish(
        "--- src/lib.rs\n+++ src/lib.rs\n@@\n unchanged line\n-old value with a very long tail\n+new value\n",
    );

    assert!(matches!(
        &lines[0],
        TranscriptLine::DiffFileHeader { marker, path } if *marker == '-' && path == "src/lib.rs"
    ));
    assert!(matches!(
        &lines[1],
        TranscriptLine::DiffFileHeader { marker, path } if *marker == '+' && path == "src/lib.rs"
    ));
    assert!(matches!(&lines[2], TranscriptLine::DiffHunk { text } if text == "@@"));
    assert!(matches!(
        &lines[3],
        TranscriptLine::DiffContext { text } if text == "unchanged line"
    ));
    assert!(matches!(
        &lines[4],
        TranscriptLine::DiffRemoved { text } if text == "old value with a"
    ));
    assert!(matches!(
        &lines[5],
        TranscriptLine::DiffRemoved { text } if text.contains("very long tail")
    ));
    assert!(matches!(
        &lines[6],
        TranscriptLine::DiffAdded { text } if text == "new value"
    ));
    assert_eq!(lines[4].display_text(), "-old value with a");
    assert_eq!(lines[6].display_text(), "+new value");
    assert!(
        lines
            .iter()
            .all(|line| neo_tui::visible_width(&line.display_text()) <= 17)
    );
}

#[test]
fn transcript_renderer_does_not_classify_plain_plus_minus_text_as_diff() {
    let renderer = TranscriptRenderer::new(40);
    let lines = renderer.render_markdownish("+ plain plus\n- plain minus\n---\n+++");

    assert!(!matches!(&lines[0], TranscriptLine::DiffAdded { .. }));
    assert!(!matches!(&lines[1], TranscriptLine::DiffRemoved { .. }));
    assert!(!matches!(&lines[2], TranscriptLine::DiffFileHeader { .. }));
    assert!(!matches!(&lines[3], TranscriptLine::DiffFileHeader { .. }));
}

#[test]
fn app_shell_streams_live_bash_output_and_clears_on_finish() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "bash".to_owned(),
        arguments: serde_json::json!({ "command": "echo live" }),
    });

    for line in ["line one", "line two", "line three", "line four"] {
        app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
            turn: 1,
            id: "tool-1".to_owned(),
            name: "bash".to_owned(),
            partial_result: neo_agent_core::ToolResult::ok(line),
        });
    }

    let Some(neo_tui::TranscriptItem::Tool { tool_run, .. }) = app.transcript().items().last()
    else {
        panic!("expected tool item");
    };
    // live_output is a rolling 3-line buffer; oldest ("line one") is dropped.
    assert_eq!(
        tool_run.live_output,
        vec![
            "line two".to_owned(),
            "line three".to_owned(),
            "line four".to_owned(),
        ]
    );
    assert!(tool_run.result.is_none());

    // Running phase: header shows "Using bash", body shows live output lines.
    let lines = render_app(80, 14, &app);
    assert!(
        lines.iter().any(|line| line.contains("● Using bash")),
        "expected running bash header"
    );
    assert!(
        lines.iter().any(|line| line.contains("line two")),
        "expected live output line two"
    );
    assert!(
        lines.iter().any(|line| line.contains("line four")),
        "expected live output line four"
    );
    assert!(
        !lines.iter().any(|line| line.contains("line one")),
        "oldest line should have been dropped from the rolling buffer"
    );

    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "bash".to_owned(),
        result: neo_agent_core::ToolResult::ok("final result"),
    });

    let Some(neo_tui::TranscriptItem::Tool { tool_run, .. }) = app.transcript().items().last()
    else {
        panic!("expected tool item");
    };
    assert!(tool_run.live_output.is_empty());
    assert_eq!(tool_run.result.as_deref(), Some("final result"));

    // Finished: header shows "Used bash", body shows result (no live output).
    let lines = render_app(80, 10, &app);
    assert!(
        lines.iter().any(|line| line.contains("✓ Used bash")),
        "expected used bash header"
    );
    assert!(
        lines.iter().any(|line| line.contains("final result")),
        "expected final result in body"
    );
    assert!(
        !lines.iter().any(|line| line.contains("line two")),
        "live output should be cleared after finish"
    );
}

#[test]
fn tool_call_lifecycle_does_not_duplicate_transcript_items() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    // Model starts the tool call.
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "list".to_owned(),
    });

    // Arguments stream in.
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: r#"{"path":"."}"#.to_owned(),
    });

    // Runtime starts executing.
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "list".to_owned(),
        arguments: serde_json::json!({ "path": "." }),
    });

    let tool_count = app
        .transcript()
        .items()
        .iter()
        .filter(|item| matches!(item, neo_tui::TranscriptItem::Tool { .. }))
        .count();
    assert_eq!(
        tool_count, 1,
        "ToolExecutionStarted should update existing tool, not add another"
    );

    // Runtime finishes.
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "list".to_owned(),
        result: neo_agent_core::ToolResult {
            content: "file1\nfile2\nfile3".to_owned(),
            is_error: false,
            details: None,
            terminate: false,
        },
    });

    let tool_count = app
        .transcript()
        .items()
        .iter()
        .filter(|item| matches!(item, neo_tui::TranscriptItem::Tool { .. }))
        .count();
    assert_eq!(
        tool_count, 1,
        "ToolExecutionFinished should update existing tool, not add another"
    );

    assert!(matches!(
        app.transcript().items().last(),
        Some(neo_tui::TranscriptItem::Tool {
            name,
            status,
            ..
        }) if name == "list" && status == &neo_tui::ToolStatusKind::Succeeded
    ));
}

#[test]
fn tool_call_lifecycle_renders_single_header() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "list".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: r#"{"path":"."}"#.to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "list".to_owned(),
        arguments: serde_json::json!({ "path": "." }),
    });

    let lines = render_app(100, 20, &app);
    let using_headers: Vec<_> = lines
        .iter()
        .filter(|l| l.contains("● Using list"))
        .cloned()
        .collect();
    assert_eq!(
        using_headers.len(),
        1,
        "expected exactly one running list header, got: {using_headers:?}"
    );

    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "list".to_owned(),
        result: neo_agent_core::ToolResult {
            content: (1..=25)
                .map(|i| format!("file{i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            is_error: false,
            details: None,
            terminate: false,
        },
    });

    let lines = render_app(100, 20, &app);
    let using_headers: Vec<_> = lines
        .iter()
        .filter(|l| l.contains("● Using list"))
        .cloned()
        .collect();
    let used_headers: Vec<_> = lines
        .iter()
        .filter(|l| l.contains("✓ Used list"))
        .cloned()
        .collect();
    assert!(
        using_headers.is_empty(),
        "running header should be gone: {using_headers:?}"
    );
    assert_eq!(
        used_headers.len(),
        1,
        "expected exactly one used list header, got: {used_headers:?}"
    );
}

#[test]
fn bash_tool_does_not_duplicate_with_shell_events() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    // Model turn
    app.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "msg-1".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "bash".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolCallFinished {
        turn: 1,
        tool_call: neo_agent_core::AgentToolCall {
            id: "bash-1".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({"command": "echo hello"}),
        },
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "msg-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::ToolUse,
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::Assistant {
            content: vec![],
            tool_calls: vec![neo_agent_core::AgentToolCall {
                id: "bash-1".to_owned(),
                name: "bash".to_owned(),
                arguments: serde_json::json!({"command": "echo hello"}),
            }],
            stop_reason: neo_agent_core::StopReason::ToolUse,
        },
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::ToolUse,
    });

    // Tool execution starts
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "bash".to_owned(),
        arguments: serde_json::json!({"command": "echo hello"}),
    });

    // Shell-specific events (emitted for bash by the runtime)
    app.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        command: "echo hello".to_owned(),
        cwd: std::path::PathBuf::from("/tmp/neo-ws"),
    });

    // Shell finishes FIRST (runtime emits ShellCommandFinished before
    // ToolExecutionFinished)
    app.apply_agent_event(neo_agent_core::AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        exit_code: Some(0),
        stdout: "hello\n".to_owned(),
        stderr: String::new(),
        truncated: false,
    });

    // ToolExecutionFinished arrives SECOND with the same id
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "bash".to_owned(),
        result: neo_agent_core::ToolResult {
            content: "exit_code: Some(0)\nstdout:\nhello\n\nstderr:\n\ntruncated: false\n"
                .to_owned(),
            is_error: false,
            details: Some(serde_json::json!({
                "exit_code": 0,
                "stdout": "hello\n",
                "stderr": "",
                "truncated": false,
            })),
            terminate: false,
        },
    });

    let tool_count = app
        .transcript()
        .items()
        .iter()
        .filter(|item| matches!(item, neo_tui::TranscriptItem::Tool { .. }))
        .count();
    assert_eq!(
        tool_count, 1,
        "bash tool should produce exactly one transcript item, got {tool_count}"
    );
}

#[test]
fn tool_call_with_assistant_message_does_not_duplicate() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "msg-1".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Let me explore the project structure.".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "list".to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "tool-1".to_owned(),
        json_fragment: r#"{"path":"."}"#.to_owned(),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolCallFinished {
        turn: 1,
        tool_call: neo_agent_core::AgentToolCall {
            id: "tool-1".to_owned(),
            name: "list".to_owned(),
            arguments: serde_json::json!({"path":"."}),
        },
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "msg-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::ToolUse,
    });
    // --- The events below mirror the real runtime sequence ---
    // After run_model_turn returns, the runtime emits MessageAppended
    // (with the complete assistant message) and TurnFinished.
    app.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::Assistant {
            content: vec![neo_agent_core::Content::Text {
                text: "Let me explore the project structure.".to_owned(),
            }],
            tool_calls: vec![neo_agent_core::AgentToolCall {
                id: "tool-1".to_owned(),
                name: "list".to_owned(),
                arguments: serde_json::json!({"path":"."}),
            }],
            stop_reason: neo_agent_core::StopReason::ToolUse,
        },
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::ToolUse,
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "list".to_owned(),
        arguments: serde_json::json!({"path":"."}),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "list".to_owned(),
        result: neo_agent_core::ToolResult {
            content: (1..=25)
                .map(|i| format!("file{i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            is_error: false,
            details: None,
            terminate: false,
        },
    });

    let tool_count = app
        .transcript()
        .items()
        .iter()
        .filter(|item| matches!(item, neo_tui::TranscriptItem::Tool { name, .. } if name == "list"))
        .count();
    assert_eq!(
        tool_count, 1,
        "expected exactly one list tool item, got {tool_count}"
    );

    let lines = render_app(100, 20, &app);
    let using_headers: Vec<_> = lines
        .iter()
        .filter(|l| l.contains("● Using list"))
        .cloned()
        .collect();
    let used_headers: Vec<_> = lines
        .iter()
        .filter(|l| l.contains("✓ Used list"))
        .cloned()
        .collect();
    assert!(
        using_headers.is_empty(),
        "running header should be gone: {using_headers:?}"
    );
    assert_eq!(
        used_headers.len(),
        1,
        "expected exactly one used list header, got: {used_headers:?}"
    );
}

#[test]
fn single_read_renders_standalone() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "read-0".to_owned(),
        name: "read".to_owned(),
        arguments: serde_json::json!({ "path": "src/main.rs" }),
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "read-0".to_owned(),
        name: "read".to_owned(),
        result: neo_agent_core::ToolResult {
            content: "line1\nline2\nline3".to_owned(),
            is_error: false,
            details: None,
            terminate: false,
        },
    });

    let lines = render_app(100, 10, &app);
    assert!(
        lines.iter().any(|line| line.contains("✓ Used read")),
        "expected standalone read header"
    );
    assert!(
        !lines.iter().any(|line| line.contains("Read 1 files")),
        "single read should not be grouped"
    );
    assert!(
        lines.iter().any(|line| line.contains("· 3 lines")),
        "expected line count chip"
    );
}

// ── Plan Mode tests ──────────────────────────────────────────

#[test]
fn plan_mode_starts_inactive() {
    let app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    assert!(!app.is_plan_mode());
}

#[test]
fn plan_mode_can_be_activated() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_plan_mode(true);
    assert!(app.is_plan_mode());
    app.set_plan_mode(false);
    assert!(!app.is_plan_mode());
}

#[test]
fn plan_mode_stream_update_activates_state() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_stream_update(StreamUpdate::PlanModeChanged { active: true });
    assert!(app.is_plan_mode());
    app.apply_stream_update(StreamUpdate::PlanModeChanged { active: false });
    assert!(!app.is_plan_mode());
}

#[test]
fn plan_mode_indicator_renders_in_footer_when_active() {
    let mut app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_plan_mode(true);

    let lines = render_app(100, 12, &app);

    assert!(
        lines.iter().any(|line| line.contains("[PLAN MODE]")),
        "footer should show [PLAN MODE] indicator:\n{}",
        lines.join("\n")
    );
}

#[test]
fn plan_mode_indicator_absent_when_inactive() {
    let app = NeoTuiApp::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");

    let lines = render_app(100, 12, &app);

    assert!(
        !lines.iter().any(|line| line.contains("[PLAN MODE]")),
        "footer should NOT show [PLAN MODE] indicator when inactive:\n{}",
        lines.join("\n")
    );
}
