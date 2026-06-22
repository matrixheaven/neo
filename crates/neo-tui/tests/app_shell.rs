use neo_tui::chrome::{
    ApprovalChoice, ChromeMode, CommandPaletteState, CommandSpec, ContextWindow, ModelPickerState,
    NeoChromeState, Overlay, OverlayKind, PickerItem, PromptEdit, SessionPickerItem,
    SessionPickerScope, SessionPickerState, StreamUpdate, ToolStatusKind,
};
use neo_tui::image::{ImageProtocolPreference, ImageRenderPolicy, TerminalImageCapabilities};
use neo_tui::transcript::{TranscriptPane, render_chrome_lines};
use std::path::PathBuf;

fn render_app(width: u16, app: &NeoChromeState) -> Vec<String> {
    render_chrome_lines(app, usize::from(width), 30)
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
fn cwd_label_uses_shell_home_slash_format() {
    let home = std::env::var("HOME").expect("HOME is set for test");
    let workspace = PathBuf::from(home).join("Workspace").join("neo");
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
    app.set_development_mode(neo_tui::chrome::DevelopmentMode::Goal(
        neo_tui::chrome::GoalModeStatus::Pending,
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
    });

    assert_eq!(
        app.context_window(),
        Some(ContextWindow::new(200_000).with_used_tokens(168))
    );
    let lines = render_app(100, &app);
    assert!(lines.iter().any(|line| line.contains("ctx 168/200k")));
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
fn tool_approval_number_two_is_approve_for_session_in_four_option_modal() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        turn: 1,
        id: "shell-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "shell.run".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
        session_scope: None,
        prefix_rule: None,
    });

    // Ordinary tool approvals keep 4 options: number 2 = Approve for this
    // session (AlwaysApprove), which IS meaningful for repeating tools.
    let result = app
        .choose_approval_number(2)
        .expect("tool approval option 2 should confirm");
    assert_eq!(result.request_id, "shell-1");
    assert_eq!(result.choice, ApprovalChoice::AlwaysApprove);
}

#[test]
fn blocking_question_dialog_hides_composer_prompt() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.prompt_mut().apply_edit(PromptEdit::Insert("draft"));
    app.push_question_overlay(
        "question-1",
        vec![neo_tui::widgets::QuestionDisplayData {
            question: "Pick one".to_owned(),
            header: Some("Question".to_owned()),
            body: None,
            options: vec![neo_tui::widgets::QuestionDisplayOption {
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
        .map(|line| neo_tui::ansi::strip_ansi(line))
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
    });

    let mut tui = neo_tui::NeoTui::new(app, TranscriptPane::new(80, 20));
    let (lines, cursor) = tui.render_frame(80, 20);
    let frame = lines
        .iter()
        .map(|line| neo_tui::ansi::strip_ansi(line))
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
        .map(|line| neo_tui::ansi::strip_ansi(line))
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
            .all(|line| neo_tui::ansi::visible_width(line) <= content_width),
        "prompt lines must stay inside composer width: {prompt_box_lines:?}"
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
    assert_eq!(result, neo_tui::core::InputResult::Handled);

    // Render at a narrow width to ensure the masked field does not overflow.
    let _ = app.focused_overlay_lines(60);

    // The keybinding layer delivers Enter as `Action(SelectConfirm)` for
    // focused overlays (see `OVERLAY_ACTION_PRIORITY`). The dialog translate
    // layer must normalize it back to Submit.
    let result =
        app.handle_focused_dialog_input(InputEvent::Action(KeybindingAction::SelectConfirm));
    assert_eq!(
        result,
        neo_tui::core::InputResult::Submitted,
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
    assert_eq!(result, neo_tui::core::InputResult::Cancelled);
    assert!(matches!(
        app.api_key_input_result(),
        Some(ApiKeyInputResult::Cancelled)
    ));
}
