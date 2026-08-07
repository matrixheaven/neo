//! Skill-authoring workflow behavior (split from `workflow.rs`).

use neo_agent_core::{AgentEvent, Content, PermissionMode};
use neo_tui::input::{InputEvent, KeybindingAction};

use super::super::*;
use super::*;

#[tokio::test]
async fn self_evo_preflight_switch_to_ask_starts_skill_workflow() {
    let seen_prompt = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen_prompt_clone = std::sync::Arc::clone(&seen_prompt);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_prompt = std::sync::Arc::clone(&seen_prompt_clone);
            async move {
                *seen_prompt.lock().expect("prompt lock") = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);
    controller.skill_store = Some(skill_store_with_interactive_preflight_skills());

    controller.type_text("/skill:self-evo");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("preflight opens");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm recommended option");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(controller.pending_local_user_message_to_suppress, None);
    assert_eq!(controller.pending_skill_user_message_to_suppress, None);
    let prompt = seen_prompt.lock().expect("prompt lock").clone();
    assert!(prompt.contains("self-evo"), "{prompt}");
    assert!(
        prompt.contains("Ask me which session scope to distill"),
        "{prompt}"
    );
}

#[tokio::test]
async fn slash_create_skill_without_instruction_in_auto_opens_required_preflight() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);
    controller.skill_store = Some(skill_store_with_interactive_preflight_skills());

    controller.type_text("/skill:create-skill");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("create-skill preflight opens");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(controller.chrome().focused_overlay().is_some());
}

#[tokio::test]
async fn slash_create_skill_with_instruction_in_auto_skips_preflight() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_requests = std::sync::Arc::clone(&seen_requests);
            async move {
                seen_requests.lock().expect("requests lock").push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);
    controller.skill_store = Some(skill_store_with_interactive_preflight_skills());

    controller.type_text("/skill:create-skill make a rust panic review skill");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("create-skill instruction starts");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    assert!(controller.chrome().focused_overlay().is_none());
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    let skill_context = requests[0].skill_context.as_deref().expect("skill context");
    assert!(
        skill_context.contains("<neo-skill-loaded name=\"create-skill\""),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("CREATE SKILL: make a rust panic review skill"),
        "{skill_context}"
    );
}
