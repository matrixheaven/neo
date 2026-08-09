use neo_tui::shell::{ContextWindow, NeoChromeState, PromptEdit};
use neo_tui::transcript::{TranscriptPane, render_chrome_lines};
use std::path::PathBuf;

fn render_app(width: u16, app: &NeoChromeState) -> Vec<String> {
    render_chrome_lines(app, usize::from(width), 30)
        .lines
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect()
}
fn render_transcript(width: usize, height: usize, transcript: &mut TranscriptPane) -> Vec<String> {
    transcript
        .render_frame(width, height)
        .expect("transcript frame")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect()
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
fn app_shell_footer_shows_main_agent_token_usage_and_cache() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_context_window(Some(ContextWindow::new(200_000).with_used_tokens(12_345)));

    app.apply_agent_event(neo_agent_core::AgentEvent::TokenUsage {
        turn: 1,
        usage: neo_agent_core::AgentTokenUsage {
            input_tokens: 3_200_000,
            output_tokens: 40_000,
            input_cache_read_tokens: 3_170_000,
            input_cache_write_tokens: 0,
        },
    });
    app.apply_agent_event(neo_agent_core::AgentEvent::TokenUsage {
        turn: 2,
        usage: neo_agent_core::AgentTokenUsage {
            input_tokens: 3_190_000,
            output_tokens: 38_100,
            input_cache_read_tokens: 3_130_000,
            input_cache_write_tokens: 0,
        },
    });

    let footer = render_app(140, &app)
        .into_iter()
        .find(|line| line.contains("ctx "))
        .expect("footer contains context usage");

    assert!(footer.contains("ctx 12k/200k"));
    assert!(footer.contains("↑6.4M"));
    assert!(footer.contains("↓78.1k"));
    assert!(footer.contains("cache 6.3M read"));
    assert!(footer.contains("hit 98.6%"));
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
