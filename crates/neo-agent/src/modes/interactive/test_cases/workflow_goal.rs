//! Goal-mode workflow behavior (split from `workflow.rs`).

use neo_agent_core::{AgentEvent, Content};
use neo_tui::{
    input::{InputEvent, KeybindingAction},
    transcript::TranscriptEntry,
};

use super::super::*;
use super::*;

#[tokio::test]
async fn goal_development_mode_sets_turn_authoring_flag() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("lock")
                    .push(request.goal_mode_authoring);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
        .await
        .expect("cycle to plan");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
        .await
        .expect("cycle to goal");
    controller.type_text("draft a goal");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit goal-mode turn");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    assert_eq!(*requests.lock().expect("lock"), vec![true]);
}

#[tokio::test]
async fn slash_goal_starts_goal_and_submits_objective_as_first_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        temp.path(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests.lock().expect("lock").push(request.prompt);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.local_config = Some(test_config(temp.path(), sessions_dir));
    controller.active_session_id = Some(SESSION_A.to_owned());

    controller.type_text("/goal fix checkout tests");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit goal command");
    controller
        .wait_for_active_turn()
        .await
        .expect("goal turn completes");

    let statuses = transcript_entries(&controller)
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Status { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        *requests.lock().expect("lock"),
        vec![vec![Content::text("fix checkout tests")]],
        "statuses: {statuses:?}"
    );
    assert!(transcript_has_status(
        &controller,
        "Goal started: fix checkout tests"
    ));
}
