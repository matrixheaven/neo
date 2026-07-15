use neo_tui::primitive::theme::ChromeMode;
use neo_tui::shell::{
    ApprovalChoice, CommandPaletteState, CommandSpec, ContextWindow, ModelPickerState,
    NeoChromeState, Overlay, OverlayKind, PickerItem, PromptEdit, SessionPickerItem,
    SessionPickerScope, SessionPickerState, StreamUpdate, ToolStatusKind,
};
use neo_tui::tasks_browser::{
    TaskBrowserItem, TaskBrowserKind, TaskBrowserSnapshot, TaskBrowserState, TaskBrowserStatus,
};
use neo_tui::terminal_image::{
    ImageProtocolPreference, ImageRenderPolicy, TerminalImageCapabilities,
};
use neo_tui::transcript::{TranscriptImageAttachment, TranscriptPane, render_chrome_lines};
use std::path::PathBuf;

fn write_session_scope() -> neo_agent_core::SessionApprovalScope {
    neo_agent_core::SessionApprovalScope {
        keys: vec![neo_agent_core::SessionApprovalKey::FileWrite {
            workspace: "/tmp/neo-ws".to_owned(),
            path: "/tmp/neo-ws/approved.txt".to_owned(),
            operation: neo_agent_core::FileWriteApprovalOperation::Write,
        }],
        label: "Approve writes to this file for this session".to_owned(),
        detail: "/tmp/neo-ws/approved.txt".to_owned(),
    }
}

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

fn task_browser_item(id: &str, status: TaskBrowserStatus) -> TaskBrowserItem {
    TaskBrowserItem {
        id: id.to_owned(),
        kind: TaskBrowserKind::Bash,
        status,
        title: "cargo test".to_owned(),
        description: "cargo test".to_owned(),
        elapsed: "00:05".to_owned(),
        detail_lines: vec![format!("id:          {id}")],
        preview_lines: vec!["running tests".to_owned()],
        can_stop: status.is_active(),
    }
}

#[test]
fn task_browser_overlay_blocks_prompt_and_renders_own_footer() {
    let mut app = NeoChromeState::new("neo", "test-session", "model", "/tmp/neo-ws");
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![task_browser_item(
        "bash-1",
        TaskBrowserStatus::Running,
    )]));
    app.push_task_browser_overlay(state);

    assert!(app.focused_overlay_blocks_prompt());
    assert!(app.focused_overlay_is_rich_dialog());

    let mut tui = neo_tui::NeoTui::new(app, TranscriptPane::new(80, 20));
    let (lines, cursor) = tui.render_frame(80, 20);
    let plain = lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>();
    let rendered = plain.join("\n");

    assert!(cursor.is_none());
    assert!(rendered.contains("TASK BROWSER"));
    assert!(rendered.contains("Tasks [all]"));
    assert!(rendered.contains("Detail"));
    assert!(rendered.contains("Preview Output"));
    assert!(rendered.contains("Q/Esc close"));
    assert!(!rendered.contains("/tmp/neo-ws"));
    assert_eq!(
        plain
            .iter()
            .filter(|line| line.contains("Q/Esc close"))
            .count(),
        1
    );
}

#[test]
fn task_browser_overlay_replaces_existing_transcript_body() {
    let mut app = NeoChromeState::new("neo", "test-session", "model", "/tmp/neo-ws");
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![task_browser_item(
        "bash-1",
        TaskBrowserStatus::Running,
    )]));
    app.push_task_browser_overlay(state);

    let mut transcript = TranscriptPane::new(80, 20);
    transcript.push_status("old transcript line should be hidden");
    let mut tui = neo_tui::NeoTui::new(app, transcript);
    let (lines, cursor) = tui.render_frame(80, 20);
    let rendered = lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(cursor.is_none());
    assert!(rendered.contains("TASK BROWSER"));
    assert!(!rendered.contains("old transcript line should be hidden"));
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
    tui.chrome_mut().advance_activity_frame();
    let second = strip_lines(tui.render_frame(80, 20).0).join("\n");

    assert!(first.contains("⠋ thinking..."), "first frame: {first}");
    assert!(second.contains("⠙ thinking..."), "second frame: {second}");
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
fn cwd_label_uses_shell_home_slash_format() {
    // Read the real HOME to build a workspace path under it. We cannot use
    // std::env::set_var (it is `unsafe` in edition 2024 and the workspace
    // forbids unsafe code), so we rely on the ambient HOME instead. On systems
    // without HOME the test is skipped rather than failing.
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let workspace = PathBuf::from(&home).join("Workspace").join("neo");
    let app = NeoChromeState::new("neo", "test-session", "openai/gpt-4.1", workspace);

    assert_eq!(app.cwd_label(), "~/Workspace/neo");
}

#[test]
fn footer_and_banner_include_git_status_after_cwd() {
    let mut app = NeoChromeState::new(
        "neo",
        "test-session",
        "deepseek/deepseek-v4-pro[1m]",
        "/tmp/neo-ws",
    );
    app.set_git_status_label(Some("main [+12 -3 ↑2↓1]".to_owned()));

    let footer_lines = render_app(140, &app);
    let footer = footer_lines
        .iter()
        .find(|line| line.contains("/tmp/neo-ws"))
        .expect("footer contains cwd");
    assert!(footer.contains("/tmp/neo-ws main [+12 -3 ↑2↓1]"));

    let mut runtime = TranscriptPane::new(100, 16);
    runtime.push_welcome_banner(
        app.title(),
        app.session_label(),
        app.model_label(),
        &app.cwd_label(),
        env!("CARGO_PKG_VERSION"),
        None,
    );
    let banner = render_transcript(100, 16, &mut runtime).join("\n");
    assert!(banner.contains("Directory:"));
    assert!(banner.contains("/tmp/neo-ws"));
    assert!(!banner.contains("main [+12 -3 ↑2↓1]"));
}

#[test]
fn footer_git_status_uses_github_segment_colors() {
    let mut app = NeoChromeState::new(
        "neo",
        "test-session",
        "deepseek/deepseek-v4-pro[1m]",
        "/tmp/neo-ws",
    );
    app.set_git_status_label(Some("main [+12 -3 ↑2↓1]".to_owned()));

    let footer = render_chrome_lines(&app, 140, 30)
        .lines
        .into_iter()
        .find(|line| line.contains("main"))
        .expect("footer contains git status");

    assert!(footer.contains("\x1b[38;2;191;135;0mmain"));
    assert!(footer.contains("\x1b[38;2;26;127;55m+12"));
    assert!(footer.contains("\x1b[38;2;207;34;46m-3"));
    assert!(footer.contains("\x1b[38;2;9;105;218m↑2↓1"));
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
fn footer_renders_permission_mode_badge() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_permission_mode(neo_agent_core::PermissionMode::Ask);
    let lines = render_app(80, &app);
    assert!(lines.iter().any(|line| line.contains("[ask]")));

    app.set_permission_mode(neo_agent_core::PermissionMode::Auto);
    let lines = render_app(80, &app);
    assert!(lines.iter().any(|line| line.contains("[auto]")));

    app.set_permission_mode(neo_agent_core::PermissionMode::Yolo);
    let lines = render_app(80, &app);
    assert!(lines.iter().any(|line| line.contains("[yolo]")));
}

#[test]
fn footer_shows_plan_mode_indicator() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_plan_mode(true);
    let lines = render_app(80, &app);
    assert!(lines.iter().any(|line| line.contains("[ask]")));
    assert!(lines.iter().any(|line| line.contains("[plan]")));
    assert!(!lines.iter().any(|line| line.contains("[PLAN MODE]")));

    app.set_plan_mode(false);
    let lines = render_app(80, &app);
    assert!(!lines.iter().any(|line| line.contains("[plan]")));
}

#[test]
fn footer_shows_goal_mode_status_badges() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_development_mode(neo_tui::primitive::theme::DevelopmentMode::Goal(
        neo_tui::primitive::theme::GoalModeStatus::Pending,
    ));
    assert!(
        render_app(80, &app)
            .iter()
            .any(|line| line.contains("[goal]"))
    );

    app.apply_agent_event(neo_agent_core::AgentEvent::GoalStarted {
        turn: 1,
        objective: "ship it".to_owned(),
    });
    assert!(
        render_app(80, &app)
            .iter()
            .any(|line| line.contains("[goal•]"))
    );

    app.apply_agent_event(neo_agent_core::AgentEvent::GoalPaused {
        turn: 2,
        objective: "ship it".to_owned(),
    });
    assert!(
        render_app(80, &app)
            .iter()
            .any(|line| line.contains("[goal◌]"))
    );

    app.apply_agent_event(neo_agent_core::AgentEvent::GoalBlocked {
        turn: 3,
        objective: "ship it".to_owned(),
        reason: "needs input".to_owned(),
    });
    assert!(
        render_app(80, &app)
            .iter()
            .any(|line| line.contains("[goal✗]"))
    );

    app.apply_agent_event(neo_agent_core::AgentEvent::GoalFinished {
        turn: 4,
        objective: "ship it".to_owned(),
        outcome: "done".to_owned(),
    });
    assert!(
        !render_app(80, &app)
            .iter()
            .any(|line| line.contains("[goal"))
    );
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
fn footer_renders_projected_context_when_available() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ContextWindowUpdated {
        turn: 1,
        used_tokens: 72_000,
        projected_tokens: Some(43_000),
        max_tokens: Some(64_000),
        trigger_tokens: Some(51_200),
        remaining_tokens: Some(8_200),
        source: Some(neo_agent_core::ContextWindowSource::Configured),
    });

    assert_eq!(app.context_window_label(), Some("ctx 43k/64k".to_owned()));
}

#[test]
fn footer_falls_back_to_used_tokens_for_old_events() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ContextWindowUpdated {
        turn: 1,
        used_tokens: 12_345,
        projected_tokens: None,
        max_tokens: Some(200_000),
        trigger_tokens: None,
        remaining_tokens: None,
        source: None,
    });

    assert_eq!(app.context_window_label(), Some("ctx 12k/200k".to_owned()));
}

#[test]
fn app_shell_footer_shows_main_agent_token_usage_and_cache() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));

    app.apply_agent_event(neo_agent_core::AgentEvent::TokenUsage {
        turn: 1,
        usage: neo_agent_core::AgentTokenUsage {
            input_tokens: 40_800,
            output_tokens: 1_234,
            input_cache_read_tokens: 37_200,
            input_cache_write_tokens: 1_100,
        },
    });

    let footer = render_app(140, &app)
        .into_iter()
        .find(|line| line.contains("ctx "))
        .expect("footer contains context usage");

    assert!(footer.contains("ctx 12k/200k"));
    assert!(footer.contains("↑40.8k"));
    assert!(footer.contains("↓1.2k"));
    assert!(footer.contains("cache 37.2k read / 1.1k write"));
}

#[test]
fn app_shell_footer_omits_cache_segment_when_main_agent_cache_is_zero() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));

    app.apply_agent_event(neo_agent_core::AgentEvent::TokenUsage {
        turn: 1,
        usage: neo_agent_core::AgentTokenUsage {
            input_tokens: 40_800,
            output_tokens: 1_234,
            input_cache_read_tokens: 0,
            input_cache_write_tokens: 0,
        },
    });

    let footer = render_app(140, &app)
        .into_iter()
        .find(|line| line.contains("ctx "))
        .expect("footer contains context usage");

    assert!(footer.contains("↑40.8k"));
    assert!(footer.contains("↓1.2k"));
    assert!(!footer.contains("cache"));
}

#[test]
fn app_shell_footer_keeps_context_usage_within_narrow_width() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(190_000)));

    app.apply_agent_event(neo_agent_core::AgentEvent::TokenUsage {
        turn: 1,
        usage: neo_agent_core::AgentTokenUsage {
            input_tokens: 400_800,
            output_tokens: 10_234,
            input_cache_read_tokens: 370_200,
            input_cache_write_tokens: 101_100,
        },
    });

    let lines = render_app(40, &app);

    assert!(
        lines
            .iter()
            .all(|line| neo_tui::primitive::visible_width(line) <= 38),
        "footer should not exceed frame content width: {lines:?}"
    );
}

#[test]
fn app_shell_tracks_agent_core_approval_request_without_overlay_panel() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.push_overlay(Overlay::new(
        "commands",
        OverlayKind::CommandPalette(CommandPaletteState::new([CommandSpec::new(
            "cmd",
            "Command",
            None::<String>,
        )])),
    ));
    assert!(app.focused_overlay().is_some());

    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 7,
        id: "approval-7".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "shell.run".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test -p neo-tui" }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });

    assert_eq!(app.mode(), ChromeMode::Approval);
    assert_eq!(app.approval_choice(), Some(ApprovalChoice::Approve));
    assert!(app.focused_overlay().is_none());
}

#[test]
fn plan_review_modal_offers_only_approve_reject_revise() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "exit-plan-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::PlanTransition,
        subject: "Exit plan mode".to_owned(),
        arguments: serde_json::json!({ "plan_summary": "Ready" }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });
    assert!(app.approval_is_pending());

    // Number 1 = Approve confirms immediately.
    let result = app
        .choose_approval_number(1)
        .expect("plan review option 1 (Approve) should confirm");
    assert_eq!(result.request_id, "exit-plan-1");
    assert_eq!(result.choice, ApprovalChoice::Approve);
    assert!(!app.approval_is_pending());
}

#[test]
fn plan_review_number_two_is_reject_in_three_option_modal() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "exit-plan-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::PlanTransition,
        subject: "Exit plan mode".to_owned(),
        arguments: serde_json::json!({ "plan_summary": "Ready" }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });

    // In the 3-option plan review modal, number 2 = Reject (no feedback).
    let result = app
        .choose_approval_number(2)
        .expect("plan review option 2 (Reject) should confirm");

    assert_eq!(result.request_id, "exit-plan-1");
    assert_eq!(result.choice, ApprovalChoice::Deny);
    assert!(
        result.feedback.is_none(),
        "Reject must not collect feedback"
    );
}

#[test]
fn plan_review_number_three_is_revise_and_collects_feedback() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "exit-plan-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::PlanTransition,
        subject: "Exit plan mode".to_owned(),
        arguments: serde_json::json!({ "plan_summary": "Ready" }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });

    // Number 3 = Revise does NOT confirm yet; it enters feedback collection.
    let pre = app.choose_approval_number(3);
    assert!(
        pre.is_none(),
        "Revise should enter feedback collection, not confirm"
    );
    assert_eq!(app.approval_choice(), Some(ApprovalChoice::Revise));

    app.handle_pending_approval_input(neo_tui::input::InputEvent::Insert('r'));
    let result = app
        .handle_pending_approval_input(neo_tui::input::InputEvent::Submit)
        .expect("confirming revise feedback returns a result");

    assert_eq!(result.request_id, "exit-plan-1");
    assert_eq!(result.choice, ApprovalChoice::Revise);
    assert_eq!(result.feedback.as_deref(), Some("r"));
}

#[test]
fn arrow_nav_to_revise_waits_for_enter_before_collecting_feedback() {
    use neo_tui::input::{InputEvent, KeybindingAction};

    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "exit-plan-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::PlanTransition,
        subject: "Exit plan mode".to_owned(),
        arguments: serde_json::json!({ "plan_summary": "Ready" }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });

    // Options: [0] Approve, [1] Reject, [2] Reject with feedback.
    app.handle_pending_approval_input(InputEvent::Action(KeybindingAction::SelectDown)); // → [1] Reject
    app.handle_pending_approval_input(InputEvent::Action(KeybindingAction::SelectDown)); // → [2] Revise
    assert_eq!(app.approval_choice(), Some(ApprovalChoice::Revise));
    assert!(
        !render_app(100, &app)
            .iter()
            .any(|line| line.contains("feedback:")),
        "selecting Revise should not render feedback input before Enter"
    );

    app.handle_pending_approval_input(InputEvent::Insert('x'));
    let (_, _, feedback, _, collecting_feedback) =
        app.approval_selection().expect("approval selection");
    assert_eq!(
        feedback, "",
        "typing before confirming Revise must not start feedback input"
    );
    assert!(!collecting_feedback);

    // First Enter on Revise should enter feedback input mode, not submit.
    let result = app.handle_pending_approval_input(InputEvent::Submit);
    assert!(
        result.is_none(),
        "first Enter on Revise should enter feedback collection, not submit"
    );
    assert_eq!(app.approval_choice(), Some(ApprovalChoice::Revise));
    assert!(
        app.approval_selection()
            .is_some_and(|(_, _, _, _, collecting)| collecting),
        "Enter on Revise should activate feedback collection"
    );

    // Type feedback.
    app.handle_pending_approval_input(InputEvent::Insert('f'));
    app.handle_pending_approval_input(InputEvent::Insert('x'));

    // Now Enter should submit with the feedback.
    let result = app
        .handle_pending_approval_input(InputEvent::Submit)
        .expect("Enter after typing feedback should submit");
    assert_eq!(result.request_id, "exit-plan-1");
    assert_eq!(result.choice, ApprovalChoice::Revise);
    assert_eq!(result.feedback.as_deref(), Some("fx"));
}

#[test]
fn plan_review_with_options_aligns_chrome_and_transcript_revise_index() {
    use neo_tui::input::{InputEvent, KeybindingAction};

    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "exit-plan-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::PlanTransition,
        subject: "Exit plan mode".to_owned(),
        arguments: serde_json::json!({
            "plan_summary": "Ready",
            "options": [
                {"label": "Approach A", "description": "Simple"},
                {"label": "Approach B", "description": "Fast"},
            ]
        }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });

    // Chrome has 4 options: [0] A, [1] B, [2] Reject, [3] Revise.
    // Navigate to Revise (index 3).
    app.handle_pending_approval_input(InputEvent::Action(KeybindingAction::SelectDown)); // → 1
    app.handle_pending_approval_input(InputEvent::Action(KeybindingAction::SelectDown)); // → 2
    app.handle_pending_approval_input(InputEvent::Action(KeybindingAction::SelectDown)); // → 3
    assert_eq!(
        app.approval_choice(),
        Some(ApprovalChoice::Revise),
        "Chrome should be on Revise at index 3"
    );

    assert!(
        app.handle_pending_approval_input(InputEvent::Submit)
            .is_none(),
        "first Enter on Revise should enter feedback collection"
    );

    // Typing works after the user explicitly enters feedback mode.
    app.handle_pending_approval_input(InputEvent::Insert('f'));
    app.handle_pending_approval_input(InputEvent::Insert('x'));

    // Verify feedback was collected.
    let result = app
        .handle_pending_approval_input(InputEvent::Submit)
        .expect("Submit after typing should produce a result");
    assert_eq!(result.choice, ApprovalChoice::Revise);
    assert_eq!(result.feedback.as_deref(), Some("fx"));
}

#[test]
fn plan_review_number_four_is_out_of_range_in_three_option_modal() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "exit-plan-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::PlanTransition,
        subject: "Exit plan mode".to_owned(),
        arguments: serde_json::json!({ "plan_summary": "Ready" }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });

    // The plan review modal has only 3 options, so number 4 is out of range and
    // must not confirm. This locks the 3-option layout (a 4-option modal would
    // let number 4 select Revise).
    let result = app.choose_approval_number(4);
    assert!(
        result.is_none(),
        "number 4 must be out of range in the 3-option plan review modal"
    );
}

#[test]
fn plan_review_renders_model_options_as_picker_choices() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "exit-plan-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::PlanTransition,
        subject: "Exit plan mode".to_owned(),
        arguments: serde_json::json!({
            "plan_summary": "Two approaches",
            "options": [
                {"label": "Option A", "description": "fast"},
                {"label": "Option B", "description": "safe"},
            ],
        }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });
    assert!(app.approval_is_pending());

    // Layout: 1=Approach: Option A, 2=Approach: Option B, 3=Reject, 4=Revise.
    // Picking number 2 approves model option "Option B" and surfaces its label.
    let result = app
        .choose_approval_number(2)
        .expect("option B should confirm as an approve choice");
    assert_eq!(result.request_id, "exit-plan-1");
    assert_eq!(result.choice, ApprovalChoice::Approve);
    assert_eq!(
        result.selected_option_label.as_deref(),
        Some("Option B"),
        "approving a model option must surface its label for the runtime"
    );
    assert!(
        result.feedback.is_none(),
        "approving an option must not collect feedback"
    );
}

#[test]
fn plan_review_approve_option_a_surfaces_its_label() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "exit-plan-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::PlanTransition,
        subject: "Exit plan mode".to_owned(),
        arguments: serde_json::json!({
            "plan_summary": "Two approaches",
            "options": [
                {"label": "Option A", "description": "fast"},
                {"label": "Option B", "description": "safe"},
            ],
        }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });

    // Number 1 is the first model option (Option A).
    let result = app
        .choose_approval_number(1)
        .expect("option A should confirm");
    assert_eq!(result.choice, ApprovalChoice::Approve);
    assert_eq!(result.selected_option_label.as_deref(), Some("Option A"));
}

#[test]
fn plan_review_reject_does_not_surface_a_selected_label() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "exit-plan-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::PlanTransition,
        subject: "Exit plan mode".to_owned(),
        arguments: serde_json::json!({
            "plan_summary": "Two approaches",
            "options": [
                {"label": "Option A", "description": "fast"},
                {"label": "Option B", "description": "safe"},
            ],
        }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });

    // Reject is now number 3 in the 5-option layout (A, B, Reject, Revise).
    let result = app
        .choose_approval_number(3)
        .expect("Reject should confirm");
    assert_eq!(result.choice, ApprovalChoice::Deny);
    assert!(
        result.selected_option_label.is_none(),
        "Reject must not surface a model option label"
    );
}

#[test]
fn scope_less_tool_approval_omits_approve_for_session_option() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "shell-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "shell.run".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });

    let result = app
        .choose_approval_number(2)
        .expect("tool approval option 2 should confirm");
    assert_eq!(result.request_id, "shell-1");
    assert_eq!(result.choice, ApprovalChoice::Deny);
}

#[test]
fn scoped_tool_approval_number_two_is_approve_for_session() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "write-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::FileWrite,
        subject: "Write".to_owned(),
        arguments: serde_json::json!({ "path": "approved.txt" }),
        session_scope: Some(write_session_scope()),
        prefix_rule: None,
        suggestions: vec![],
    });

    let result = app
        .choose_approval_number(2)
        .expect("scoped approval option 2 should confirm");
    assert_eq!(result.request_id, "write-1");
    assert_eq!(result.choice, ApprovalChoice::AlwaysApprove);
    assert!(!result.picked_prefix);
}

#[test]
fn prefix_tool_approval_marks_prefix_choice() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "shell-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "cargo test".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
        session_scope: Some(write_session_scope()),
        prefix_rule: Some(neo_agent_core::PrefixApprovalRule {
            prefix: vec!["cargo".to_owned(), "test".to_owned()],
            label: "cargo test".to_owned(),
        }),
        suggestions: vec![],
    });

    let result = app
        .choose_approval_number(3)
        .expect("prefix approval option should confirm");
    assert_eq!(result.choice, ApprovalChoice::AlwaysApprove);
    assert!(result.picked_prefix);
}

#[test]
fn blocking_question_dialog_hides_composer_prompt() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.prompt_mut().apply_edit(PromptEdit::Insert("draft"));
    app.push_question_overlay(
        "question-1",
        vec![neo_tui::dialogs::QuestionDisplayData {
            question: "Pick one".to_owned(),
            header: Some("Question".to_owned()),
            body: None,
            options: vec![neo_tui::dialogs::QuestionDisplayOption {
                label: "Yes".to_owned(),
                description: None,
            }],
            multi_select: false,
        }],
    );

    let mut tui = neo_tui::NeoTui::new(app, TranscriptPane::new(80, 20));
    let (lines, cursor) = tui.render_frame(80, 20);
    let frame = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(frame.contains("question"));
    assert!(
        !frame.contains("> draft"),
        "composer should be hidden: {frame}"
    );
    assert!(
        cursor.is_none(),
        "blocking dialog should not expose prompt cursor"
    );
}

#[test]
fn pending_approval_hides_composer_prompt() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.prompt_mut().apply_edit(PromptEdit::Insert("draft"));
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "approval-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "echo hi" }),
        session_scope: None,
        prefix_rule: None,
        suggestions: vec![],
    });

    let mut tui = neo_tui::NeoTui::new(app, TranscriptPane::new(80, 20));
    let (lines, cursor) = tui.render_frame(80, 20);
    let frame = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !frame.contains("> draft"),
        "composer should be hidden: {frame}"
    );
    assert!(
        frame.contains("[ask]"),
        "footer should remain visible: {frame}"
    );
    assert!(
        cursor.is_none(),
        "blocking approval should not expose prompt cursor"
    );
}

#[test]
fn prompt_completion_keeps_composer_prompt_visible() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.prompt_mut()
        .apply_edit(PromptEdit::Insert("open src/ma"));
    app.open_prompt_completion_picker(
        app.prompt()
            .completion_prefix()
            .expect("completion prefix should exist"),
        [PickerItem::new(
            "src/main.rs",
            "src/main.rs",
            None::<String>,
        )],
    );

    let mut tui = neo_tui::NeoTui::new(app, TranscriptPane::new(80, 20));
    let (lines, cursor) = tui.render_frame(80, 20);
    let frame = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(frame.contains("> open src/ma"));
    assert!(
        cursor.is_some(),
        "prompt completion depends on composer cursor"
    );
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

    assert!(!lines.iter().any(|line| line.contains("enter send")));
    assert!(!lines.iter().any(|line| line.contains("shift+enter")));
    assert!(lines.iter().any(|line| line.contains("ctx 12k/200k")));
    assert!(!lines[status_row].contains("neo  "));
    assert!(!lines[status_row].contains(" new "));
    assert!(lines[composer_row.saturating_sub(1)].contains('╭'));
    assert!(status_row > composer_row);
}

#[test]
fn app_shell_uses_brand_border_for_non_empty_prompt() {
    let mut app = NeoChromeState::new("neo", "new", "anthropic/deepseek-v4-pro[1m]", "/tmp/neo-ws");
    app.prompt_mut().apply_edit(PromptEdit::Insert("aaaa"));

    let render = render_chrome_lines(&app, 92, 30);
    let top_border = render
        .lines
        .first()
        .expect("composer top border should render");

    assert!(
        top_border.contains("\x1b[38;2;198;120;221m"),
        "non-empty prompt should use Neo brand border: {top_border:?}"
    );
    assert!(
        !top_border.contains("\x1b[38;2;139;148;158m"),
        "non-empty prompt should not stay muted: {top_border:?}"
    );
}

#[test]
fn app_shell_prompt_renders_tabs_without_terminal_tab_controls() {
    let mut app = NeoChromeState::new("neo", "new", "anthropic/deepseek-v4-pro[1m]", "/tmp/neo-ws");
    app.prompt_mut()
        .apply_edit(PromptEdit::Insert("\t\t\t\t\t"));

    let width = 80;
    let render = render_chrome_lines(&app, width, 30);
    let content_width = neo_tui::transcript::frame_content_width(width);
    let prompt_box_lines = &render.lines[render.prompt_start_row..render.lines.len() - 1];

    assert!(
        prompt_box_lines.iter().all(|line| !line.contains('\t')),
        "prompt render must not emit raw tab controls: {prompt_box_lines:?}"
    );
    assert!(
        prompt_box_lines
            .iter()
            .all(|line| neo_tui::primitive::visible_width(line) <= content_width),
        "prompt lines must stay inside composer width: {prompt_box_lines:?}"
    );
}

#[test]
fn app_shell_prompt_grows_to_eight_lines() {
    let mut app = NeoChromeState::new("neo", "new", "anthropic/deepseek-v4-pro[1m]", "/tmp/neo-ws");
    for _ in 0..9 {
        app.prompt_mut().apply_edit(PromptEdit::Insert("\n"));
    }

    let width = 80;
    let render = render_chrome_lines(&app, width, 30);
    let prompt_box_lines = &render.lines[render.prompt_start_row..render.lines.len() - 1];

    // 8 content rows + top/bottom border = 10 rows.
    assert_eq!(
        prompt_box_lines.len(),
        10,
        "prompt should cap at 8 visible content lines: {prompt_box_lines:?}"
    );
}

#[test]
fn app_shell_prompt_shows_scroll_indicators_when_clipped() {
    let mut app = NeoChromeState::new("neo", "new", "anthropic/deepseek-v4-pro[1m]", "/tmp/neo-ws");
    for _ in 0..9 {
        app.prompt_mut().apply_edit(PromptEdit::Insert("\n"));
    }
    // Cursor is at the end; viewport should scroll to keep it visible.
    app.prompt_mut()
        .apply_edit_with_width(PromptEdit::MoveEnd, 72);

    let width = 80;
    let render = render_chrome_lines(&app, width, 30);
    let prompt_box_lines = &render.lines[render.prompt_start_row..render.lines.len() - 1];
    let top_border = neo_tui::primitive::strip_ansi(&prompt_box_lines[0]);
    assert!(
        top_border.contains('↑') && top_border.contains("more"),
        "top border should show scroll-up indicator when content is scrolled: {top_border:?}"
    );

    // Move cursor back to the top; viewport should scroll back and show bottom indicator.
    for _ in 0..9 {
        app.prompt_mut()
            .apply_edit_with_width(PromptEdit::MoveUp(72), 72);
    }
    let render = render_chrome_lines(&app, width, 30);
    let prompt_box_lines = &render.lines[render.prompt_start_row..render.lines.len() - 1];
    let bottom_border =
        neo_tui::primitive::strip_ansi(prompt_box_lines.last().expect("prompt has bottom border"));
    assert!(
        bottom_border.contains('↓') && bottom_border.contains("more"),
        "bottom border should show scroll-down indicator when content is clipped: {bottom_border:?}"
    );
}

#[test]
fn transcript_pane_frame_keeps_latest_live_row_visible() {
    let mut runtime = TranscriptPane::new(80, 12);
    runtime.set_live_chrome_height(4);
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
fn transcript_pane_renders_bash_result_as_terminal_output_without_structural_labels() {
    let mut runtime = TranscriptPane::new(100, 12);

    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "printf out; printf err >&2" }),
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
    });

    let joined = render_transcript(100, 12, &mut runtime).join("\n");
    assert!(joined.contains("● Used Bash"));
    assert!(joined.contains("outerr"));
    assert!(!joined.contains("exit_code:"));
    assert!(!joined.contains("stdout:"));
    assert!(!joined.contains("stderr:"));
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
    });
    runtime.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("read README"),
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
        &neo_agent_core::ImageRef::Base64("aGVsbG8=".into()),
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
fn transcript_user_images_render_thumbnail_inside_normal_frame() {
    let mut chrome = NeoChromeState::new("neo", "session", "openai/gpt-4.1", "/tmp/neo-ws");
    chrome.set_image_render_policy(ImageRenderPolicy::new(
        ImageProtocolPreference::Kitty,
        false,
    ));
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
            vec![137, 80, 78, 71],
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

/// Regression: pasting a long API key then pressing Enter must submit.
/// Previously the API Key dialog ignored `InputEvent::Paste` (Cmd+V / right-
/// click paste) entirely, an over-long masked value crashed the renderer, and
/// Enter submitted via `Action(SelectConfirm)` which the dialog never handled.
/// This test drives the real chrome dialog dispatch path end-to-end.
#[test]
fn api_key_dialog_paste_then_submit_closes_overlay_with_result() {
    use neo_tui::dialogs::{ApiKeyInputOptions, ApiKeyInputResult};
    use neo_tui::input::{InputEvent, KeybindingAction};

    let mut app = NeoChromeState::new("neo", "s", "m", "/tmp");
    app.open_api_key_input(ApiKeyInputOptions {
        title: "API Key".into(),
        provider_name: "minimax-cn-coding-plan".into(),
    });
    assert!(app.focused_overlay_is_rich_dialog());

    // Paste a long key (the scenario that used to crash / be ignored).
    let result = app.handle_focused_dialog_input(InputEvent::Paste(
        "sk-minimax-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
    ));
    assert_eq!(result, neo_tui::primitive::InputResult::Handled);

    // Render at a narrow width to ensure the masked field does not overflow.
    let _ = app.focused_overlay_lines(60);

    // The keybinding layer delivers Enter as `Action(SelectConfirm)` for
    // focused overlays (see `OVERLAY_ACTION_PRIORITY`). The dialog translate
    // layer must normalize it back to Submit.
    let result =
        app.handle_focused_dialog_input(InputEvent::Action(KeybindingAction::SelectConfirm));
    assert_eq!(
        result,
        neo_tui::primitive::InputResult::Submitted,
        "SelectConfirm (Enter) must submit the API key dialog"
    );

    // The dialog must expose the submitted result while still focused.
    match app.api_key_input_result() {
        Some(ApiKeyInputResult::Submitted(v)) => {
            assert_eq!(v, "sk-minimax-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        }
        other => panic!("expected Submitted result, got {other:?}"),
    }

    // Likewise Esc arrives as `Action(SelectCancel)` and must cancel.
    app.open_api_key_input(ApiKeyInputOptions {
        title: "API Key".into(),
        provider_name: "p".into(),
    });
    let result =
        app.handle_focused_dialog_input(InputEvent::Action(KeybindingAction::SelectCancel));
    assert_eq!(result, neo_tui::primitive::InputResult::Cancelled);
    assert!(matches!(
        app.api_key_input_result(),
        Some(ApiKeyInputResult::Cancelled)
    ));
}
