//! Skill-authoring slash behavior (split from `input.rs`).

use neo_agent_core::{AgentEvent, PermissionMode};
use neo_tui::{
    input::{InputEvent, KeybindingAction},
    shell::OverlayKind,
};

use super::super::*;
use super::*;

#[tokio::test]
async fn slash_self_evo_without_args_in_auto_opens_required_preflight() {
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

    controller.type_text("/skill:self-evo");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("self-evo preflight opens");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("preflight overlay");
    assert!(matches!(overlay.kind, OverlayKind::ChoicePicker(_)));
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
}

#[tokio::test]
async fn slash_self_evo_with_scope_in_auto_skips_preflight() {
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

    controller.type_text("/skill:self-evo 7");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("self-evo scope starts");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    assert!(controller.chrome().focused_overlay().is_none());
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    let skill_context = requests[0].skill_context.as_deref().expect("skill context");
    assert!(
        skill_context.contains("<neo-skill-loaded name=\"self-evo\""),
        "{skill_context}"
    );
    assert!(skill_context.contains("SELF EVO: 7"), "{skill_context}");
}
