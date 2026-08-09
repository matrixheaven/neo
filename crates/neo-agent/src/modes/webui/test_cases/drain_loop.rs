//! Turn drain loop: batched event processing never starves approvals, and
//! every value queued before the task completed is still published before
//! the projection is released.

use neo_agent_core::{ApprovalAction, ApprovalOption, ApprovalRequest, PermissionOperation};
use neo_webui::protocol::WebUiServerMessage;

use super::state_fixtures::{test_state, user_message};
use super::*;

/// Read the relay cache for one session by subscribing from the start and
/// replaying; returns the parsed envelopes in sequence order.
fn relay_envelopes(
    relay: &Relay,
    session_id: &str,
    after_sequence: u64,
) -> Vec<WebUiServerMessage> {
    let outcome = relay.subscribe_session(
        1,
        session_id,
        Some(neo_webui::protocol::WebUiCursor {
            stream_id: relay.stream_id().to_owned(),
            sequence: after_sequence,
        }),
    );
    let mut envelopes = Vec::new();
    if outcome.mode == neo_webui::relay::SubscribeMode::Replay {
        for message in outcome.queue.drain_sendable() {
            if let neo_webui::relay::OutboundMessage::SessionJson(json) = message {
                if let Ok(envelope) = serde_json::from_str(&json) {
                    envelopes.push(envelope);
                }
            }
        }
    }
    envelopes
}

#[tokio::test]
async fn drain_loop_checks_approvals_after_256_events_and_keeps_late_values() {
    let relay = Relay::new("test_stream");
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (approval_tx, approval_rx) = mpsc::unbounded_channel();
    let (session_tx, session_rx) = mpsc::unbounded_channel();
    let (question_tx, question_rx) = mpsc::unbounded_channel();
    let cancel_token = CancellationToken::new();
    let steer_input = neo_agent_core::SteerInputHandle::new();
    let state = test_state(&relay, "session_1", Some("turn_1"));
    let (response_tx, approval_response_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        for index in 0..300 {
            event_tx
                .send(Ok(user_message(&format!("message {index}"))))
                .expect("send event");
        }
        let pending = RunPendingApproval {
            request: ApprovalRequest {
                turn: 1,
                id: "approval_1".to_owned(),
                operation: PermissionOperation::Tool,
                presentation: ApprovalPresentation::Tool {
                    title: "Run tool?".to_owned(),
                    details: vec!["tool: read".to_owned()],
                },
                options: vec![ApprovalOption {
                    label: "Approve once".to_owned(),
                    description: None,
                    action: ApprovalAction::PermitOnce,
                }],
                workflow_origin: None,
            },
            response_tx,
        };
        approval_tx.send(pending).expect("send approval");
        drop(session_tx);
        drop(question_tx);
        Ok(TurnOutcome::default())
    });
    // Keep the approval's response receiver alive so the sender stays
    // open for the whole drain (closed senders are never surfaced).
    let _kept_open = approval_response_rx;
    drain_turn_loop(
        Arc::clone(&state),
        "turn_1".to_owned(),
        TurnReceivers {
            events: event_rx,
            approvals: approval_rx,
            session_ids: session_rx,
            questions: question_rx,
            task,
            cancel_token,
            steer_input,
        },
    )
    .await;

    assert_eq!(relay.current_sequence("session_1"), 303);
    // The cache replay is bounded, so resume from a later cursor; the
    // 256-event batch boundary still falls inside the replayed window.
    let envelopes = relay_envelopes(&relay, "session_1", 100);
    assert!(
        envelopes.len() >= 202,
        "expected replayed events plus state envelopes, got {}",
        envelopes.len()
    );
    // The approval state must appear after exactly the 256-event batch,
    // before the remaining events — text deltas never starve approvals.
    let waiting_sequence = envelopes
        .iter()
        .find_map(|envelope| match envelope {
            WebUiServerMessage::SessionState {
                sequence, event, ..
            } if event.waiting_approval => Some(*sequence),
            _ => None,
        })
        .expect("approval state envelope");
    assert_eq!(waiting_sequence, 257);
    let final_sequence = envelopes
        .iter()
        .filter_map(|envelope| match envelope {
            WebUiServerMessage::SessionState {
                sequence, event, ..
            } if !event.waiting_approval => Some(*sequence),
            _ => None,
        })
        .max()
        .expect("final state envelope");
    assert_eq!(final_sequence, 303);
    // Every late value was processed: all 300 events reached the relay
    // (the replay window covers sequences 101..=303, i.e. 200 events).
    let event_count = envelopes
        .iter()
        .filter(|envelope| matches!(envelope, WebUiServerMessage::SessionEvent { .. }))
        .count();
    assert_eq!(event_count, 200);
    // The turn is idle and cleared after finalization.
    let guard = state.lock().expect("state lock");
    assert_eq!(guard.phase, WebUiPhase::Idle);
    assert!(guard.turn_id.is_none());
    assert!(guard.pending_approval.is_none());
}

#[tokio::test]
async fn drain_loop_publishes_events_queued_before_task_completion_was_observed() {
    let relay = Relay::new("test_stream");
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (approval_tx, approval_rx) = mpsc::unbounded_channel();
    let (session_tx, session_rx) = mpsc::unbounded_channel();
    let (question_tx, question_rx) = mpsc::unbounded_channel();
    let cancel_token = CancellationToken::new();
    let steer_input = neo_agent_core::SteerInputHandle::new();
    let state = test_state(&relay, "session_1", Some("turn_1"));
    let (done_tx, done_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        for index in 0..300 {
            event_tx
                .send(Ok(user_message(&format!("message {index}"))))
                .expect("send event");
        }
        drop(event_tx);
        drop(approval_tx);
        drop(session_tx);
        drop(question_tx);
        let _ = done_tx.send(());
        Ok(TurnOutcome::default())
    });
    // Barrier: the task is already finished and its channels closed before
    // the drain loop starts, so every queued event is a "late" value.
    done_rx.await.expect("task finished barrier");
    assert!(task.is_finished());
    drain_turn_loop(
        Arc::clone(&state),
        "turn_1".to_owned(),
        TurnReceivers {
            events: event_rx,
            approvals: approval_rx,
            session_ids: session_rx,
            questions: question_rx,
            task,
            cancel_token,
            steer_input,
        },
    )
    .await;

    let guard = state.lock().expect("state lock");
    assert!(
        guard.history.is_empty(),
        "the completed turn's projection is released at finalize"
    );
    assert_eq!(
        relay.current_sequence("session_1"),
        302,
        "no late event may be dropped: all 300 events plus the finishing \
         and idle state envelopes reached the relay before the release"
    );
    assert_eq!(guard.phase, WebUiPhase::Idle);
}
