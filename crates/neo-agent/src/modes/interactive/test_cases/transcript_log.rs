//! Transcript log-capture behavior (split from `transcript.rs`).

use neo_agent_core::AgentEvent;
use neo_tui::transcript::TranscriptEntry;
use tracing_subscriber::prelude::*;

use super::super::*;
use super::*;

#[test]
fn drain_log_events_pushes_warn_as_status() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (layer, rx) = crate::log_capture::capture_channel(8);
    controller.set_log_event_receiver(rx);
    let _guard = tracing_subscriber::registry().with(layer).set_default();
    tracing::warn!(server_id = "linear", "MCP server unavailable");
    controller.drain_log_events();
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("MCP server unavailable"),
        "transcript should show captured WARN, got:\n{snapshot}"
    );
}

#[test]
fn drain_log_events_pushes_error_as_status() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (layer, rx) = crate::log_capture::capture_channel(8);
    controller.set_log_event_receiver(rx);
    let _guard = tracing_subscriber::registry().with(layer).set_default();
    tracing::error!("critical failure");
    controller.drain_log_events();
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("critical failure"),
        "transcript should show captured ERROR, got:\n{snapshot}"
    );
}

#[test]
fn drain_log_events_does_not_crash_without_receiver() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    // No set_log_event_receiver — should be a no-op, not a panic.
    controller.drain_log_events();
}

#[test]
fn drain_log_events_caps_ticks_and_session_transcript() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (layer, rx) = crate::log_capture::capture_channel(
        super::log_events::MAX_CAPTURED_LOG_STATUSES_PER_SESSION,
    );
    controller.set_log_event_receiver(rx);
    let _guard = tracing_subscriber::registry().with(layer).set_default();
    let entries_before = transcript_entries(&controller).len();
    for batch in 0..=super::log_events::MAX_CAPTURED_LOG_STATUSES_PER_SESSION
        / super::log_events::MAX_LOG_EVENTS_PER_TICK
        + 1
    {
        for offset in 0..super::log_events::MAX_LOG_EVENTS_PER_TICK {
            tracing::warn!("captured log {batch}-{offset}");
        }
        controller.drain_log_events();
        if batch == 0 {
            let captured = transcript_entries(&controller)
                .iter()
                .filter(|entry| matches!(entry, TranscriptEntry::Status { text, .. } if text.starts_with("captured log")))
                .count();
            assert_eq!(captured, super::log_events::MAX_LOG_EVENTS_PER_TICK);
        }
    }

    let entries = transcript_entries(&controller);
    let captured = entries
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::Status { text, .. } if text.starts_with("captured log")))
        .count();
    let suppression_notices = entries
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::Status { text, .. } if text == "Some captured log events were suppressed for this session"))
        .count();
    assert_eq!(
        captured,
        super::log_events::MAX_CAPTURED_LOG_STATUSES_PER_SESSION
    );
    assert_eq!(suppression_notices, 1);
    assert_eq!(
        entries.len() - entries_before,
        super::log_events::MAX_CAPTURED_LOG_STATUSES_PER_SESSION + 1
    );
}

#[test]
fn rebuilding_session_transcript_resets_captured_log_budget() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.captured_log_status_count = super::log_events::MAX_CAPTURED_LOG_STATUSES_PER_SESSION;
    controller.captured_log_suppression_notified = true;

    controller.rebuild_transcript_from_session(&LoadedSessionTranscript::new(
        "new-session",
        Vec::new(),
        Vec::new(),
    ));

    assert_eq!(controller.captured_log_status_count, 0);
    assert!(!controller.captured_log_suppression_notified);
    let (layer, rx) = crate::log_capture::capture_channel(1);
    controller.set_log_event_receiver(rx);
    let _guard = tracing_subscriber::registry().with(layer).set_default();
    tracing::warn!("new session warning");
    controller.drain_log_events();
    assert_eq!(controller.captured_log_status_count, 1);
    assert!(transcript_has_status(&controller, "new session warning"));
}
