//! Permission/plan mode input behavior (split from `input.rs`).

use neo_agent_core::{AgentEvent, PermissionMode};
use neo_tui::{
    input::{InputEvent, KeybindingAction},
    transcript::TranscriptEntry,
};

use super::super::*;
use super::*;

#[tokio::test]
async fn slash_permission_commands_set_mode_status_and_footer() {
    let cases = [
        ("/ask", PermissionMode::Ask, "ask"),
        ("/auto", PermissionMode::Auto, "auto"),
        ("/yolo", PermissionMode::Yolo, "yolo"),
    ];

    for (command, mode, label) in cases {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text(command);
        controller
            .handle_input_event(InputEvent::Submit)
            .await
            .expect("slash command handled");
        assert_eq!(
            controller.chrome().permission_mode(),
            mode,
            "case {command}"
        );
        assert!(
            transcript_has_status(&controller, &format!("Permission Mode: {label}")),
            "case {command}"
        );
        assert!(
            controller.render_snapshot().contains(&format!("[{label}]")),
            "case {command}"
        );
    }
}

#[tokio::test]
async fn permissions_picker_selects_auto_mode() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("/permissions");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("opens permission picker");
    assert!(controller.chrome().focused_overlay().is_some());

    // Move from Ask (index 0) to Auto (index 1) and confirm.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move selection");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm selection");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
    assert!(transcript_has_status(&controller, "Permission Mode: auto"));
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn slash_plan_toggles_plan_mode_and_footer() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("/plan");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("toggles plan mode on");
    assert!(controller.chrome().is_plan_mode());
    assert!(transcript_has_status(&controller, "Plan Mode On"));
    assert!(controller.render_snapshot().contains("[plan]"));
    assert!(!controller.render_snapshot().contains("[PLAN MODE]"));

    controller.type_text("/plan");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("toggles plan mode off");
    assert!(!controller.chrome().is_plan_mode());
    assert!(transcript_has_status(&controller, "Plan Mode Off"));
}

#[tokio::test]
async fn slash_plan_turn_request_uses_runtime_plan_mode() {
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
                let active = request
                    .plan_mode
                    .read()
                    .expect("plan mode lock")
                    .is_active();
                captured_requests.lock().expect("lock").push(active);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    controller.type_text("/plan on");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("plan on");
    controller.type_text("plan this");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit turn");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    assert_eq!(*requests.lock().expect("lock"), vec![true]);
}

#[tokio::test]
async fn permission_switch_does_not_split_streaming_thinking() {
    let mut controller = running_turn_controller().await;
    controller.apply_turn_event(AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-one".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
    });
    controller.apply_turn_event(AgentEvent::ThinkingDelta {
        turn: 1,
        text: "The ".to_owned(),
    });

    controller.type_text("/auto");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash handled");
    controller.apply_turn_event(AgentEvent::ThinkingDelta {
        turn: 1,
        text: "CompactionSettings".to_owned(),
    });

    assert!(controller.active_turn.is_some(), "turn should keep running");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
    assert!(!transcript_has_status(&controller, "Permission Mode: auto"));
    assert!(
        !transcript_has_status(&controller, "A turn is already running"),
        "live slash must not be blocked by the active-turn guard"
    );
    let thinking = transcript_entries(&controller)
        .iter()
        .filter_map(TranscriptEntry::thinking_content)
        .collect::<Vec<_>>();
    assert_eq!(thinking, vec!["The CompactionSettings".to_owned()]);

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn slash_ask_updates_permission_mode_while_turn_is_running() {
    let mut controller = running_turn_controller().await;
    // Flip to Auto first so /ask is a real change.
    controller.type_text("/auto");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash handled");

    controller.type_text("/ask");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash handled");

    assert!(controller.active_turn.is_some(), "turn should keep running");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert!(!transcript_has_status(&controller, "Permission Mode: ask"));

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn slash_yolo_updates_permission_mode_while_turn_is_running() {
    let mut controller = running_turn_controller().await;

    controller.type_text("/yolo");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash handled");

    assert!(controller.active_turn.is_some(), "turn should keep running");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Yolo);
    assert!(!transcript_has_status(&controller, "Permission Mode: yolo"));

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn permission_picker_keeps_working_status_while_turn_is_running() {
    let mut controller = running_turn_controller().await;

    controller.open_permission_picker();
    for _ in 0..2 {
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move permission selection");
    }
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("select permission mode");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Yolo);
    assert_eq!(
        controller.chrome().working_label().as_deref(),
        Some("working · esc interrupt")
    );

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn slash_permissions_degrades_to_hint_while_turn_is_running() {
    let mut controller = running_turn_controller().await;

    controller.type_text("/permissions");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash handled");

    assert!(controller.active_turn.is_some(), "turn should keep running");
    // The picker must NOT open during an active turn to avoid racing with
    // approval/question overlays from the running turn.
    assert!(
        controller.chrome().focused_overlay().is_none(),
        "picker overlay must not open during an active turn"
    );
    assert!(transcript_has_status(
        &controller,
        "Use /ask, /auto, or /yolo while a turn is running"
    ));

    controller.cancel_active_turn().await.expect("cancel turn");
}
