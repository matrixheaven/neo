//! Prompt-history behavior (split from `input.rs`).

use std::path::PathBuf;

use neo_agent_core::AgentEvent;
use neo_tui::input::{InputEvent, KeyId, KeybindingAction};

use super::super::*;
use super::*;

#[tokio::test]
async fn event_loop_uses_up_down_keys_for_prompt_history() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("first prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("first turn completes");

    controller.type_text("second prompt");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("second prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("second turn completes");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls latest prompt");
    assert_eq!(controller.chrome().prompt().text, "second prompt");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls older prompt");
    assert_eq!(controller.chrome().prompt().text, "first prompt");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down moves toward newer prompt");
    assert_eq!(controller.chrome().prompt().text, "second prompt");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down restores empty draft");
    assert_eq!(controller.chrome().prompt().text, "");
}

#[tokio::test]
async fn slash_commands_are_not_persisted_to_prompt_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("prompt-history.jsonl");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));

    let mut controller = controller_with_history_store(store);

    // `/model` opens the model picker overlay and never becomes a user
    // turn, so it must not be written to prompt history.
    controller.type_text("/model");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash command handled");

    let persisted = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        !persisted.contains("/model"),
        "slash commands must not be persisted: {persisted}"
    );
    drop(dir);
}

#[tokio::test]
async fn shell_mode_commands_do_not_enter_prompt_history() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(dir.path());
    let mut controller = controller_with_history_store(store.clone());
    controller.set_shell_driver(Arc::new(|request| {
        Box::pin(async move {
            assert_eq!(request.command, "echo hidden");
            Ok(completed_shell_result("hidden\n"))
        })
    }));

    controller.type_text("!");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter shell mode");
    controller.type_text("echo hidden");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("run shell command");
    controller
        .wait_for_active_shell_command()
        .await
        .expect("shell completes");

    let history = store.load_recent().expect("history loads");
    assert!(
        history.is_empty(),
        "shell commands should not be written to prompt history"
    );
}
