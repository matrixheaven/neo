//! Interactive terminal behavior (moved from `tests.rs`).

use neo_agent_core::{AgentEvent, Content};
use neo_tui::screen_output::FullscreenTerminal;
use tokio::sync::oneshot;

use super::super::*;
use super::*;

#[test]
fn auto_image_protocol_uses_positive_runtime_hints_on_local_terminals() {
    let env = |name: &str| match name {
        "TERM" => Ok("xterm-kitty".to_owned()),
        "TERM_PROGRAM" => Ok("WezTerm".to_owned()),
        "KITTY_WINDOW_ID" => Ok("1".to_owned()),
        "WEZTERM_PANE" => Ok("2".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities = terminal_image_capabilities_for_policy(ImageProtocolPreference::Auto, env);

    assert!(capabilities.kitty());
    assert!(!capabilities.iterm2());
}

#[test]
fn auto_image_protocol_detects_ghostty_as_kitty_graphics() {
    let env = |name: &str| match name {
        "TERM" => Ok("xterm-ghostty".to_owned()),
        "TERM_PROGRAM" => Ok("ghostty".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities = terminal_image_capabilities_for_policy(ImageProtocolPreference::Auto, env);

    assert!(capabilities.kitty());
    assert!(!capabilities.iterm2());
}

#[test]
fn auto_image_protocol_falls_back_inside_tmux_screen_or_ssh() {
    let tmux_env = |name: &str| match name {
        "TERM" => Ok("xterm-kitty".to_owned()),
        "KITTY_WINDOW_ID" | "TMUX" => Ok("1".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };
    let ssh_env = |name: &str| match name {
        "TERM_PROGRAM" => Ok("iTerm.app".to_owned()),
        "SSH_CONNECTION" => Ok("127.0.0.1 1 127.0.0.1 2".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    assert_eq!(
        terminal_image_capabilities_for_policy(ImageProtocolPreference::Auto, tmux_env),
        TerminalImageCapabilities::default()
    );
    assert_eq!(
        terminal_image_capabilities_for_policy(ImageProtocolPreference::Auto, ssh_env),
        TerminalImageCapabilities::default()
    );
}

#[test]
fn explicit_image_protocol_uses_matching_static_terminal_hints() {
    let env = |name: &str| match name {
        "TERM" => Ok("xterm-kitty".to_owned()),
        "TERM_PROGRAM" => Ok("WezTerm".to_owned()),
        "KITTY_WINDOW_ID" => Ok("1".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities = terminal_image_capabilities_for_policy(ImageProtocolPreference::Kitty, env);

    assert!(capabilities.kitty());
    assert!(!capabilities.iterm2());
}

#[test]
fn terminal_capabilities_dumb_term_disables_ansi_and_images() {
    let env = |name: &str| match name {
        "TERM" => Ok("dumb".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities =
        detect_terminal_capabilities_with_env(ImageProtocolPreference::Auto, true, env);

    assert!(!capabilities.ansi.cursor_addressing);
    assert!(!capabilities.ansi.color);
    assert!(!capabilities.image.kitty());
    assert!(!capabilities.can_run_tui());
}

#[test]
fn terminal_capabilities_wt_session_disables_images_keeps_ansi() {
    let env = |name: &str| match name {
        "TERM" => Ok("xterm-256color".to_owned()),
        "WT_SESSION" => Ok("00000000-0000-0000-0000-000000000000".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities =
        detect_terminal_capabilities_with_env(ImageProtocolPreference::Auto, true, env);

    assert!(capabilities.ansi.cursor_addressing);
    assert!(capabilities.ansi.color);
    assert!(capabilities.can_run_tui());
    assert!(!capabilities.image.kitty());
    assert!(!capabilities.image.iterm2());
}

#[test]
fn terminal_capabilities_ci_disables_tui_and_images() {
    let env = |name: &str| match name {
        "TERM" => Ok("xterm-256color".to_owned()),
        "CI" => Ok("true".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities =
        detect_terminal_capabilities_with_env(ImageProtocolPreference::Auto, true, env);

    assert!(!capabilities.ansi.cursor_addressing);
    assert!(!capabilities.can_run_tui());
    assert!(!capabilities.image.kitty());
}

#[test]
fn terminal_capabilities_no_color_only_disables_color() {
    let no_color_env = |name: &str| match name {
        "TERM" => Ok("xterm-kitty".to_owned()),
        "NO_COLOR" => Ok("1".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };
    let color_env = |name: &str| match name {
        "TERM" => Ok("xterm-kitty".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities =
        detect_terminal_capabilities_with_env(ImageProtocolPreference::Auto, true, no_color_env);
    let color_capabilities =
        detect_terminal_capabilities_with_env(ImageProtocolPreference::Auto, true, color_env);

    assert!(capabilities.ansi.cursor_addressing);
    assert!(capabilities.ansi.bracketed_paste);
    assert!(capabilities.can_run_tui());
    assert!(!capabilities.ansi.color);
    assert_eq!(capabilities.image.kitty(), color_capabilities.image.kitty());
}

#[tokio::test]
async fn load_session_at_startup_sets_terminal_title_from_loaded_title() {
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs::default(),
        |session_id| async move {
            Ok(
                LoadedSessionTranscript::new(session_id, Vec::new(), Vec::new())
                    .with_terminal_title("Resume Title"),
            )
        },
    );

    controller
        .load_session_at_startup(SESSION_A)
        .await
        .expect("session loads at startup");

    assert_eq!(controller.chrome().session_label(), SESSION_A);
    assert_eq!(controller.chrome().terminal_title(), "Resume Title");
}

#[tokio::test]
async fn started_session_sets_terminal_title_before_turn_finishes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join("sessions");
    let config = test_config(temp.path(), sessions_dir);
    let bucket_dir = workspace_sessions_dir(&config);
    write_main_wire(&bucket_dir, SESSION_NEW, "");
    SessionMetadataStore::new(&bucket_dir)
        .record_activity(
            SESSION_NEW,
            Some(temp.path().display().to_string()),
            Some("Fix terminal title regression".to_owned()),
            "1".to_owned(),
        )
        .expect("record initial prompt");

    let (release_tx, release_rx) = oneshot::channel::<()>();
    let release_rx = Arc::new(std::sync::Mutex::new(Some(release_rx)));
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let release_rx = Arc::clone(&release_rx);
        Box::pin(async move {
            channels
                .session_ids
                .send(SESSION_NEW.to_owned())
                .expect("session id sent");
            let release = release_rx
                .lock()
                .expect("release lock")
                .take()
                .expect("single turn");
            let _ = release.await;
            Ok(TurnOutcome::session(SESSION_NEW))
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "new",
        "openai/gpt-4.1",
        temp.path(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );
    controller.local_config = Some(config);

    controller.start_turn_with_prompt_display(
        vec![Content::text("Fix terminal title regression")],
        "Fix terminal title regression".to_owned(),
    );
    tokio::task::yield_now().await;
    controller
        .drain_active_turn()
        .await
        .expect("drain session id");

    assert_eq!(
        controller.chrome().terminal_title(),
        "Fix terminal title regression"
    );

    release_tx.send(()).expect("release turn");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");
}

#[test]
fn terminal_exit_commits_interrupted_live_entries_before_leave() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "write-1".to_owned(),
        name: "Write".to_owned(),
        arguments: serde_json::json!({"path": "notes.txt", "content": "draft"}),
        workflow_origin: None,
        output_ref: None,
    });
    controller.tui.transcript_mut().start_assistant_message();
    controller
        .tui
        .transcript_mut()
        .append_assistant_delta("unfinished assistant text");
    let mut terminal = FullscreenTerminal::for_test(80, 24);
    let initial = controller.tui.render_terminal_frame(80, 24);
    terminal
        .render_to(&mut Vec::new(), &initial)
        .expect("render initial live frame");

    let mut final_frame = None;
    controller
        .finalize_and_render_terminal_exit(|tui| {
            final_frame = Some(tui.render_terminal_frame(80, 24));
            Ok(())
        })
        .expect("finalize and render terminal exit");
    let final_frame = final_frame.expect("final exit frame");
    let text = final_frame
        .lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("unfinished assistant text"), "frame:\n{text}");
    assert!(text.contains("Write"), "frame:\n{text}");
    let mut output = Vec::new();
    terminal
        .render_to(&mut output, &final_frame)
        .expect("commit interrupted frame");
    terminal.leave(&mut output).expect("leave terminal");
    let output = String::from_utf8(output).expect("terminal output is UTF-8");
    assert!(output.contains("unfinished assistant text"));
    assert!(!output.contains("\x1b[3J"));
}
