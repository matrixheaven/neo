//! Transcript sidecar (btw) behavior (split from `transcript.rs`).

use std::fs;

use neo_agent_core::{AgentEvent, AgentMessage};
use neo_tui::input::InputEvent;

use super::super::*;
use super::*;

#[tokio::test]
async fn chrome_only_btw_update_requests_a_frame() {
    struct IdleThenInterrupt {
        idle: bool,
    }

    impl TerminalEvents for IdleThenInterrupt {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            Ok(InputEvent::Interrupt)
        }

        fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
            if self.idle {
                self.idle = false;
                std::thread::sleep(timeout);
                Ok(None)
            } else {
                Ok(Some(InputEvent::Interrupt))
            }
        }
    }

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.tui.chrome_mut().set_btw_panel_state(Some(
        neo_tui::widgets::btw_panel::BtwPanelState::new(
            neo_tui::widgets::btw_panel::BtwSidecar::new("sidecar-1"),
        ),
    ));
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender
        .send(crate::modes::btw::BtwEvent::Started {
            sidecar_id: "sidecar-1".to_owned(),
            prompt: "question".to_owned(),
        })
        .expect("send sidecar event");
    controller.btw_receiver = Some(receiver);
    assert!(
        !controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("first interrupt requests confirmation")
    );

    let mut render_count = 0;
    controller
        .run_terminal_loop_with_suspend(
            |tui, _| {
                let _ = tui.render_terminal_frame_at(80, 24, Instant::now());
                render_count += 1;
                Ok(None)
            },
            || Ok(()),
            |_| Ok(()),
            IdleThenInterrupt { idle: true },
        )
        .await
        .expect("event loop exits");

    assert_eq!(render_count, 2, "BTW event must request one frame");
}

#[test]
fn draining_btw_sidecar_reports_only_real_chrome_updates() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.tui.chrome_mut().set_btw_panel_state(Some(
        neo_tui::widgets::btw_panel::BtwPanelState::new(
            neo_tui::widgets::btw_panel::BtwSidecar::new("sidecar-1"),
        ),
    ));
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    controller.btw_receiver = Some(receiver);

    assert!(!controller.drain_btw_sidecar());
    sender
        .send(crate::modes::btw::BtwEvent::Started {
            sidecar_id: "sidecar-1".to_owned(),
            prompt: "question".to_owned(),
        })
        .expect("send sidecar event");
    assert!(controller.drain_btw_sidecar());
    assert!(!controller.drain_btw_sidecar());
}

#[tokio::test]
async fn bare_slash_btw_while_sidecar_running_keeps_existing_panel() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client(""));

    controller.handle_slash_command("/btw").await;
    {
        let state = controller
            .tui
            .chrome_mut()
            .btw_panel_state_mut()
            .expect("panel state");
        state.sidecar.phase = neo_tui::widgets::btw_panel::BtwPhase::Running;
    }
    let original_id = controller
        .chrome()
        .btw_panel_state()
        .expect("panel state")
        .sidecar
        .id
        .0
        .clone();

    controller.handle_slash_command("/btw").await;

    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.id.0, original_id);
    assert_eq!(
        state.sidecar.phase,
        neo_tui::widgets::btw_panel::BtwPhase::Running
    );
    assert!(state.status_message.as_deref().is_some_and(|message| {
        message.contains("already open") || message.contains("Wait for /btw")
    }));
}

#[tokio::test]
async fn composer_routes_to_sidecar_when_panel_open() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client("answer"));

    controller.handle_slash_command("/btw").await;
    controller.type_text("explain this");
    controller
        .submit_current_prompt()
        .await
        .expect("submit routes to sidecar");

    assert!(controller.active_turn.is_none(), "must not start main turn");
    for _ in 0..10 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }
    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns.len(), 1);
    assert_eq!(state.sidecar.turns[0].prompt, "explain this");
}

#[tokio::test]
async fn sidecar_events_do_not_append_to_main_transcript() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client("side answer"));

    let entries_before = controller.tui.transcript().transcript().entries().len();
    controller.handle_slash_command("/btw side question").await;
    for _ in 0..20 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }
    let entries_after = controller.tui.transcript().transcript().entries().len();

    assert_eq!(
        entries_before, entries_after,
        "sidecar must not append to main transcript"
    );
    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns[0].answer, "side answer");
}

#[tokio::test]
async fn escape_closes_btw_without_touching_main_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { std::future::pending::<Result<Vec<AgentEvent>>>().await },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client(""));

    controller.type_text("main question");
    controller
        .submit_current_prompt()
        .await
        .expect("main turn starts");
    assert!(controller.active_turn.is_some());

    controller.handle_slash_command("/btw").await;
    assert!(controller.chrome().has_btw_panel());

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("esc handled");

    assert!(!controller.chrome().has_btw_panel(), "Esc closes BTW panel");
    assert!(
        controller.active_turn.is_some(),
        "Esc must not cancel the main turn"
    );
}

#[tokio::test]
async fn btw_running_preserves_composer_text_and_shows_busy_notice() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client(""));

    // Open an empty sidecar panel and mark it Running as if a turn were in
    // progress. This avoids coupling the test to a hanging model client.
    controller.handle_slash_command("/btw").await;
    if let Some(state) = controller.tui.chrome_mut().btw_panel_state_mut() {
        state.sidecar.phase = neo_tui::widgets::btw_panel::BtwPhase::Running;
    }

    controller.type_text("second question");
    controller
        .submit_current_prompt()
        .await
        .expect("busy check handled");

    assert_eq!(
        controller.chrome().prompt().text,
        "second question",
        "composer text must be preserved while sidecar is running"
    );
    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns.len(), 0, "no sidecar turn started");
    assert!(
        state
            .status_message
            .as_deref()
            .expect("busy notice")
            .contains("Wait for /btw to finish"),
        "busy notice should be shown"
    );
}

#[tokio::test]
async fn btw_conversation_is_not_written_to_main_session_jsonl() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let sessions_dir = project_dir.join(".neo/sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");

    let session_id = "session_00000000-0000-4000-8000-000000000901";
    let session_path = main_wire_path_for_session(sessions_dir.join(session_id));
    let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session");
    writer
        .append_event(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("existing main message"),
        })
        .await
        .expect("append event");
    writer.flush().await.expect("flush");
    drop(writer);

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client("side answer"));
    controller.active_session_id = Some(session_id.to_owned());

    controller.handle_slash_command("/btw side question").await;
    for _ in 0..20 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }

    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns[0].answer, "side answer");

    let content = fs::read_to_string(&session_path).expect("read session");
    assert!(
        content.contains("existing main message"),
        "original main event should still be present"
    );
    assert!(
        !content.contains("side question"),
        "side question must not be written to main JSONL"
    );
    assert!(
        !content.contains("side answer"),
        "side answer must not be written to main JSONL"
    );
}
