//! Turn control routing: cancel requires the current turn and follow-up
//! input distinguishes idle, stale and turn-ending sessions.

use super::state_fixtures::test_state;
use super::*;

#[test]
fn cancel_requires_current_turn_and_marks_finishing_once() {
    let relay = Relay::new("test_stream");
    let state = test_state(&relay, "session_1", Some("turn_1"));
    assert!(cancel_turn(&state, "turn_old").is_err());
    assert!(cancel_turn(&state, "turn_1").is_ok());
    {
        let guard = state.lock().expect("state lock");
        assert_eq!(guard.phase, WebUiPhase::Finishing);
        assert!(guard.cancel_requested);
        assert_eq!(guard.turn_id.as_deref(), Some("turn_1"));
    }
    // Repeats on the same turn id stay accepted (202 cancelling).
    assert!(cancel_turn(&state, "turn_1").is_ok());
}

#[test]
fn input_routing_distinguishes_idle_stale_and_finishing() {
    let relay = Relay::new("test_stream");
    let idle = test_state(&relay, "session_1", None);
    assert_eq!(
        push_turn_input(
            &idle,
            "turn_1",
            neo_webui::protocol::WebUiInputDelivery::FollowUp,
            "hi"
        )
        .expect_err("idle has no active turn")
        .code,
        WebUiErrorCode::NoActiveTurn
    );
    let running = test_state(&relay, "session_1", Some("turn_1"));
    assert!(
        push_turn_input(
            &running,
            "turn_old",
            neo_webui::protocol::WebUiInputDelivery::FollowUp,
            "hi"
        )
        .is_err(),
        "stale turn id rejected"
    );
    assert!(
        push_turn_input(
            &running,
            "turn_1",
            neo_webui::protocol::WebUiInputDelivery::Steer,
            "now"
        )
        .is_ok(),
        "steer is not degraded to follow-up"
    );
    // A closed input handle is the turn-ending race: 409 turn_transition.
    let closing = test_state(&relay, "session_2", Some("turn_1"));
    {
        let guard = closing.lock().expect("state lock");
        let _ = guard
            .active
            .as_ref()
            .expect("active turn")
            .steer_input
            .close_if_empty();
    }
    assert_eq!(
        push_turn_input(
            &closing,
            "turn_1",
            neo_webui::protocol::WebUiInputDelivery::FollowUp,
            "hi"
        )
        .expect_err("closed handle race")
        .code,
        WebUiErrorCode::TurnTransition
    );
}
