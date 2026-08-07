//! Sidecar panel input behavior (split from `input.rs`).

use std::fs;

use neo_agent_core::{AgentEvent, AgentMessage};
use neo_tui::input::InputEvent;

use super::super::*;
use super::*;

#[tokio::test]
async fn slash_btw_opens_empty_sidecar_panel_without_starting_main_turn() {
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

    assert!(
        controller.chrome().has_btw_panel(),
        "/btw opens the sidecar panel"
    );
    assert!(
        controller.btw_runner.is_some(),
        "/btw creates a sidecar runner"
    );
    assert!(
        controller.active_turn.is_none(),
        "/btw must not start a main turn"
    );
}

#[tokio::test]
async fn slash_btw_question_starts_in_memory_sidecar_only() {
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
    controller.set_btw_client(btw_fake_client("42"));

    controller.handle_slash_command("/btw what is 2+2?").await;

    assert!(controller.chrome().has_btw_panel());
    assert!(controller.btw_receiver.is_some());
    assert!(controller.active_turn.is_none());

    // Drain events so the panel state reflects the sidecar answer.
    for _ in 0..10 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }
    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns.len(), 1);
    assert_eq!(state.sidecar.turns[0].prompt, "what is 2+2?");
    assert_eq!(state.sidecar.turns[0].answer, "42");
}

#[tokio::test]
async fn slash_btw_inherits_main_context_with_single_sidecar_projection() {
    use neo_ai::{AiStreamEvent, StopReason};

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
    controller.active_session_id = Some("session_00000000-0000-0000-0000-000000000001".into());
    // Persist the message to the session JSONL so /btw can inherit it.
    // In production, turn execution writes messages to disk; simulate that here.
    {
        let config = controller.local_config.as_ref().expect("config");
        let wire_path = crate::modes::sessions::session_path(
            "session_00000000-0000-0000-0000-000000000001",
            config,
        )
        .expect("session path");
        fs::create_dir_all(wire_path.parent().expect("wire parent")).expect("mkdir wire parent");
        let event = AgentEvent::MessageAppended {
            message: AgentMessage::user_text("main context in memory"),
        };
        let line = serde_json::to_string(&event).expect("serialize event");
        fs::write(&wire_path, format!("{line}\n")).expect("write wire");
    }
    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_text("main context in memory"),
    });
    let fake = neo_ai::providers::fake::FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg-1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "side".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    controller.set_btw_client(Arc::new(fake.clone()));

    controller
        .handle_slash_command("/btw inspect context")
        .await;
    for _ in 0..20 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }

    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let contents: Vec<String> = requests[0].messages.iter().map(chat_message_text).collect();
    assert!(
        contents
            .iter()
            .any(|content| content == "main context in memory"),
        "sidecar should inherit current in-memory main transcript: {contents:?}"
    );
    assert_eq!(
        contents
            .iter()
            .filter(|content| content.contains("side-channel conversation"))
            .count(),
        1,
        "sidecar reminder should be projected exactly once: {contents:?}"
    );
}

#[tokio::test]
async fn empty_composer_esc_closes_panel() {
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
    assert!(controller.chrome().has_btw_panel());

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("esc handled");

    assert!(
        !controller.chrome().has_btw_panel(),
        "Esc closes empty panel"
    );
}

#[tokio::test]
async fn slash_btw_while_main_turn_running_does_not_steer_or_queue_main_turn() {
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
    controller.set_btw_client(btw_fake_client("side answer"));

    controller.type_text("main question");
    controller
        .submit_current_prompt()
        .await
        .expect("main turn starts");
    assert!(
        controller.active_turn.is_some(),
        "main turn should be active"
    );

    controller.handle_slash_command("/btw side question").await;
    for _ in 0..20 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }

    assert!(
        controller.active_turn.is_some(),
        "/btw must not cancel or queue the main turn"
    );
    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns.len(), 1);
    assert_eq!(state.sidecar.turns[0].answer, "side answer");
}

#[tokio::test]
async fn shift_enter_inserts_newline_while_btw_panel_open() {
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
    assert!(controller.chrome().has_btw_panel());

    controller.type_text("line1");
    controller
        .handle_input_event(InputEvent::NewLine)
        .await
        .expect("newline handled");
    controller.type_text("line2");

    assert_eq!(controller.chrome().prompt().text, "line1\nline2");
}
