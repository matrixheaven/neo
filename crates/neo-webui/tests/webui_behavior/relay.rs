//! Bounded relay behavior: per-session sequences, snapshot-plus-resume,
//! fallback rules, slow consumers, session switching, retry snapshots, the
//! fixed sample fixture, and the real web long connection. Everything runs
//! in-process against the in-memory fake host and a loopback server on a
//! random port — no fixed waits, no fixed ports.

use std::collections::HashSet;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use neo_agent_core::AgentEvent;
use neo_webui::protocol::{
    WebUiCursor, WebUiErrorBody, WebUiHost, WebUiServerMessage, WebUiSessionSummary, WebUiSnapshot,
    WebUiSummaryState,
};
use neo_webui::relay::{
    CONNECTION_QUEUE_MESSAGES, OutboundMessage, Relay, SubscribeMode, SubscriptionLayer,
    WS_CLOSE_SLOW_CONSUMER,
};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::http_server::{
    FakeHost, RawRequest, TestServer, raw_request, watch, watch_workspace, ws_connect,
};

fn make_relay(stream_id: &str) -> Relay {
    Relay::new(stream_id.to_string())
}

fn text(turn: u32, content: &str) -> AgentEvent {
    AgentEvent::TextDelta {
        turn,
        text: content.to_string(),
    }
}

fn publish_text(publisher: &neo_webui::EventPublisher, turn: u32, content: &str) -> u64 {
    publisher.publish(neo_webui::protocol::WebUiEventBody::SessionEvent {
        event: text(turn, content),
        output: None,
    })
}

fn envelopes_of(messages: &[OutboundMessage]) -> Vec<WebUiServerMessage> {
    messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::SessionJson(json) => serde_json::from_str(json).ok(),
            OutboundMessage::WorkspaceJson(_) | OutboundMessage::Close { .. } => None,
        })
        .collect()
}

fn sequences(envelopes: &[WebUiServerMessage]) -> Vec<u64> {
    envelopes
        .iter()
        .filter_map(|envelope| match envelope {
            WebUiServerMessage::SessionEvent { sequence, .. }
            | WebUiServerMessage::SessionState { sequence, .. }
            | WebUiServerMessage::SessionMetadataChanged { sequence, .. } => Some(*sequence),
            _ => None,
        })
        .collect()
}

fn texts(envelopes: &[WebUiServerMessage]) -> Vec<String> {
    envelopes
        .iter()
        .filter_map(|envelope| match envelope {
            WebUiServerMessage::SessionEvent {
                event: AgentEvent::TextDelta { text, .. },
                ..
            } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn overflow_falls_back_to_a_snapshot_without_unbounded_queue_growth() {
    let relay = make_relay("svc_overflow");
    let publisher = relay.publisher("s1");

    // A connected-but-slow consumer: subscribe, deliver the snapshot, then
    // never drain the queue while the session floods.
    let slow = relay.subscribe_session(1, "s1", None);
    assert_eq!(slow.mode, SubscribeMode::NeedsSnapshot);
    relay.deliver_snapshot(
        1,
        SubscriptionLayer::Session,
        Arc::from(r#"{"watermark":0}"#),
    );

    for index in 0..CONNECTION_QUEUE_MESSAGES * 2 {
        publish_text(&publisher, 1, &format!("event {index}"));
    }

    // The queue stayed bounded: overflow cleared it and queued a single
    // 1013 close; the observer was deregistered.
    let drained = slow.queue.drain_sendable();
    assert_eq!(drained.len(), 1, "only the close message survives overflow");
    assert!(matches!(
        &drained[0],
        OutboundMessage::Close {
            code: WS_CLOSE_SLOW_CONSUMER
        }
    ));
    assert!(slow.queue.is_closed());

    // Further publishes reach nothing: the dead observer stays empty.
    publish_text(&publisher, 1, "after overflow");
    assert!(slow.queue.drain_sendable().is_empty());

    // A fresh consumer whose replay cannot fit the queue falls back to a
    // snapshot instead of looping on 1013.
    let fresh = relay.subscribe_session(
        2,
        "s1",
        Some(WebUiCursor {
            stream_id: "svc_overflow".to_string(),
            sequence: 0,
        }),
    );
    assert_eq!(fresh.mode, SubscribeMode::NeedsSnapshot);
}

#[tokio::test]
async fn replay_resumes_continuously_after_the_cursor() {
    let relay = make_relay("svc_replay");
    let publisher = relay.publisher("s1");
    for index in 1..=10 {
        publish_text(&publisher, 1, &format!("m{index}"));
    }

    let outcome = relay.subscribe_session(
        7,
        "s1",
        Some(WebUiCursor {
            stream_id: "svc_replay".to_string(),
            sequence: 5,
        }),
    );
    assert_eq!(outcome.mode, SubscribeMode::Replay);
    assert_eq!(
        sequences(&envelopes_of(&outcome.queue.drain_sendable())),
        [6, 7, 8, 9, 10]
    );

    // Live events continue in order: no gaps, no duplicates.
    publish_text(&publisher, 1, "m11");
    publish_text(&publisher, 1, "m12");
    assert_eq!(
        sequences(&envelopes_of(&outcome.queue.drain_sendable())),
        [11, 12]
    );
    assert_eq!(relay.current_sequence("s1"), 12);

    // A cursor at the watermark replays nothing and stays live.
    let current = relay.subscribe_session(
        8,
        "s1",
        Some(WebUiCursor {
            stream_id: "svc_replay".to_string(),
            sequence: 12,
        }),
    );
    assert_eq!(current.mode, SubscribeMode::Replay);
    assert!(current.queue.drain_sendable().is_empty());
}

#[tokio::test]
async fn disconnect_and_resubscribe_never_lose_or_duplicate_the_final_projection() {
    let relay = make_relay("svc_projection");
    let publisher = relay.publisher("s1");
    for index in 1..=5 {
        publish_text(&publisher, 1, &format!("m{index}"));
    }

    let first = relay.subscribe_session(1, "s1", None);
    assert_eq!(first.mode, SubscribeMode::NeedsSnapshot);
    relay.deliver_snapshot(
        1,
        SubscriptionLayer::Session,
        Arc::from(r#"{"watermark":5}"#),
    );
    assert_eq!(
        first.queue.drain_sendable().len(),
        1,
        "snapshot is the only message"
    );

    // Disconnect, then let the session advance.
    relay.deregister(1);
    for index in 6..=8 {
        publish_text(&publisher, 1, &format!("m{index}"));
    }

    // Reconnect with the snapshot watermark: only the missing tail replays.
    let second = relay.subscribe_session(
        2,
        "s1",
        Some(WebUiCursor {
            stream_id: "svc_projection".to_string(),
            sequence: 5,
        }),
    );
    assert_eq!(second.mode, SubscribeMode::Replay);
    let replayed = envelopes_of(&second.queue.drain_sendable());
    assert_eq!(sequences(&replayed), [6, 7, 8]);
    // Final projection = snapshot 1..5 + replay 6..8, each text exactly once.
    assert_eq!(texts(&replayed), ["m6", "m7", "m8"]);
}

#[tokio::test]
async fn different_stream_id_invalid_cursor_or_evicted_cache_fall_back_to_snapshot() {
    let relay = make_relay("svc_fallback");
    let publisher = relay.publisher("s1");
    publish_text(&publisher, 1, "m1");
    publish_text(&publisher, 1, "m2");

    // A cursor from another service start is never resumed.
    let outcome = relay.subscribe_session(
        1,
        "s1",
        Some(WebUiCursor {
            stream_id: "other_stream".to_string(),
            sequence: 1,
        }),
    );
    assert_eq!(outcome.mode, SubscribeMode::NeedsSnapshot);

    // A cursor ahead of the current sequence is impossible.
    let outcome = relay.subscribe_session(
        2,
        "s1",
        Some(WebUiCursor {
            stream_id: "svc_fallback".to_string(),
            sequence: 99,
        }),
    );
    assert_eq!(outcome.mode, SubscribeMode::NeedsSnapshot);

    // An oversized event breaks cache contiguity for older cursors.
    let oversized = "x".repeat(300 * 1024);
    publish_text(&publisher, 1, &oversized);
    publish_text(&publisher, 1, "m3");
    let outcome = relay.subscribe_session(
        3,
        "s1",
        Some(WebUiCursor {
            stream_id: "svc_fallback".to_string(),
            sequence: 0,
        }),
    );
    assert_eq!(outcome.mode, SubscribeMode::NeedsSnapshot);

    // Global eviction: filling past the 4 MiB budget evicts the oldest
    // session's tail, so its cursor falls into the evicted range.
    let relay = make_relay("svc_eviction");
    let payload = "y".repeat(250 * 1024);
    for session in 0..17 {
        let publisher = relay.publisher(format!("s{session}"));
        publish_text(&publisher, 1, &payload);
    }
    let outcome = relay.subscribe_session(
        1,
        "s0",
        Some(WebUiCursor {
            stream_id: "svc_eviction".to_string(),
            sequence: 0,
        }),
    );
    assert_eq!(outcome.mode, SubscribeMode::NeedsSnapshot);
    let outcome = relay.subscribe_session(
        2,
        "s16",
        Some(WebUiCursor {
            stream_id: "svc_eviction".to_string(),
            sequence: 0,
        }),
    );
    assert_eq!(
        outcome.mode,
        SubscribeMode::Replay,
        "recent sessions still replay"
    );
}

/// A snapshot larger than the connection byte budget must not make later
/// live events overflow: the queue closes with 1013 only when real-time
/// events themselves exceed the budget.
#[tokio::test]
async fn large_snapshot_never_blocks_live_events_behind_it() {
    let relay = make_relay("svc_large_snapshot");
    let publisher = relay.publisher("s1");

    let conn = relay.subscribe_session(1, "s1", None);
    assert_eq!(conn.mode, SubscribeMode::NeedsSnapshot);
    let oversized = format!(r#"{{"history":["{}"]}}"#, "x".repeat(600 * 1024));
    relay.deliver_snapshot(1, SubscriptionLayer::Session, Arc::from(oversized));
    assert!(
        !conn.queue.is_closed(),
        "snapshot itself never closes the queue"
    );

    // A real-time event behind the oversized snapshot still enqueues: the
    // byte budget counts live events only, so no 1013 reconnect loop.
    publish_text(&publisher, 1, "live after snapshot");
    let drained = conn.queue.drain_sendable();
    assert_eq!(drained.len(), 2, "snapshot plus the live event");
    assert!(
        matches!(&drained[0], OutboundMessage::SessionJson(json) if json.contains("history")),
        "snapshot stays at the front"
    );
    assert!(
        matches!(&drained[1], OutboundMessage::SessionJson(json) if json.contains("live after snapshot")),
        "live event follows the snapshot"
    );
    assert!(!conn.queue.is_closed());
}

#[tokio::test]
async fn snapshot_switch_clears_the_previous_session() {
    let relay = make_relay("svc_switch");
    let first_publisher = relay.publisher("s1");
    let second_publisher = relay.publisher("s2");
    publish_text(&first_publisher, 1, "m1");

    let outcome = relay.subscribe_session(1, "s1", None);
    relay.deliver_snapshot(
        1,
        SubscriptionLayer::Session,
        Arc::from(r#"{"snapshot":"s1"}"#),
    );
    publish_text(&first_publisher, 1, "m2");
    assert_eq!(
        outcome.queue.drain_sendable().len(),
        2,
        "snapshot then live event"
    );

    // Same connection switches to s2: the queue is cleared and only s2
    // content follows.
    let switched = relay.subscribe_session(1, "s2", None);
    relay.deliver_snapshot(
        1,
        SubscriptionLayer::Session,
        Arc::from(r#"{"snapshot":"s2"}"#),
    );
    publish_text(&second_publisher, 1, "n1");
    let drained = switched.queue.drain_sendable();
    assert!(
        matches!(&drained[0], OutboundMessage::SessionJson(json) if json.contains("\"snapshot\":\"s2\"")),
        "s2 snapshot precedes everything"
    );
    let envelopes = envelopes_of(&drained);
    assert_eq!(envelopes.len(), 1);
    match &envelopes[0] {
        WebUiServerMessage::SessionEvent {
            session_id,
            sequence,
            event,
            ..
        } => {
            assert_eq!(session_id, "s2");
            assert_eq!(*sequence, 1);
            assert!(matches!(event, AgentEvent::TextDelta { text, .. } if text == "n1"));
        }
        _ => panic!("expected a session event"),
    }
}

#[tokio::test]
async fn workspace_subscription_updates_background_session_without_full_transcript() {
    let relay = make_relay("svc_two_tier");
    let foreground = relay.publisher("s1");
    let background = relay.publisher("s2");

    // One connection holds two subscriptions: the full s1 transcript plus the
    // workspace summary layer.
    let session = relay.subscribe_session(1, "s1", None);
    relay.deliver_snapshot(
        1,
        SubscriptionLayer::Session,
        Arc::from(r#"{"snapshot":"s1"}"#),
    );
    let workspace = relay.subscribe_workspace(1, None);
    assert_eq!(workspace.mode, SubscribeMode::NeedsSnapshot);
    relay.deliver_snapshot(
        1,
        SubscriptionLayer::Workspace,
        Arc::from(r#"{"workspace":0}"#),
    );
    let queue = session.queue;
    assert!(
        Arc::ptr_eq(&queue, &workspace.queue),
        "both layers share one bounded queue"
    );
    assert_eq!(queue.drain_sendable().len(), 2, "both layer snapshots land");

    // The background session s2 advances: its AgentEvent never reaches this
    // connection — only the small summary update does.
    publish_text(&background, 1, "background transcript");
    relay.publish_summary(WebUiSessionSummary {
        session_id: "s2".to_string(),
        title: Some("background".to_string()),
        updated_at: None,
        pinned: false,
        archived: false,
        unread: false,
        state: WebUiSummaryState::Running,
        workspace_label: "workspace".to_string(),
    });
    publish_text(&foreground, 1, "foreground transcript");

    let drained = queue.drain_sendable();
    let workspace_jsons: Vec<String> = drained
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::WorkspaceJson(json) => Some(json.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(workspace_jsons.len(), 1, "exactly one summary update");
    assert!(
        !workspace_jsons[0].contains("background transcript"),
        "summary layer never carries an AgentEvent"
    );
    match serde_json::from_str::<WebUiServerMessage>(&workspace_jsons[0])
        .expect("summary envelope parses")
    {
        WebUiServerMessage::SessionSummaryChanged {
            workspace_sequence,
            event,
            ..
        } => {
            assert_eq!(workspace_sequence, 1);
            assert_eq!(event.session_id, "s2");
            assert_eq!(event.state, WebUiSummaryState::Running);
        }
        other => panic!("expected a summary change, got {other:?}"),
    }
    // The foreground event still flows on the session layer.
    assert_eq!(texts(&envelopes_of(&drained)), ["foreground transcript"]);

    // Switching the full subscription clears only the session layer: the
    // workspace summary layer survives the switch untouched.
    let switched = relay.subscribe_session(1, "s2", None);
    relay.deliver_snapshot(
        1,
        SubscriptionLayer::Session,
        Arc::from(r#"{"snapshot":"s2"}"#),
    );
    relay.publish_summary(WebUiSessionSummary {
        session_id: "s1".to_string(),
        title: Some("foreground".to_string()),
        updated_at: None,
        pinned: false,
        archived: false,
        unread: false,
        state: WebUiSummaryState::Idle,
        workspace_label: "workspace".to_string(),
    });
    let drained = switched.queue.drain_sendable();
    assert!(
        matches!(&drained[0], OutboundMessage::SessionJson(json) if json.contains("\"snapshot\":\"s2\"")),
        "the new session snapshot leads"
    );
    let workspace_jsons: Vec<&Arc<str>> = drained
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::WorkspaceJson(json) => Some(json),
            _ => None,
        })
        .collect();
    assert_eq!(
        workspace_jsons.len(),
        1,
        "summary updates keep flowing across session switches"
    );
    assert!(workspace_jsons[0].contains("\"session_id\":\"s1\""));

    // End to end over a real connection: both tiers arrive as wire messages
    // and the summary layer still carries no AgentEvent.
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;
    server.host.create_with_id("s1");
    server.host.create_with_id("s2");
    let mut ws = ws_connect(server.addr, &cookie, &server.origin())
        .await
        .expect("websocket upgrade");
    watch_workspace(&mut ws, None).await;
    watch(&mut ws, "s1", None).await;

    let first = ws.next().await.expect("workspace snapshot").expect("ok");
    let WsMessage::Text(body) = first else {
        panic!("expected a text workspace snapshot");
    };
    let body: serde_json::Value = serde_json::from_str(&body).expect("workspace snapshot JSON");
    assert_eq!(body["type"], "workspace_snapshot");
    assert_eq!(
        body["workspaces"][0]["sessions"]
            .as_array()
            .expect("sessions")
            .len(),
        2
    );

    let second = ws.next().await.expect("session snapshot").expect("ok");
    let WsMessage::Text(body) = second else {
        panic!("expected a text session snapshot");
    };
    let body: serde_json::Value = serde_json::from_str(&body).expect("session snapshot JSON");
    assert_eq!(body["type"], "session_snapshot");
    assert_eq!(body["snapshot"]["session_id"], "s1");

    server.host.publish("s2", text(1, "background wire"));
    server.host.publish_summary("s2");
    server.host.publish("s1", text(1, "foreground wire"));

    let third = ws.next().await.expect("summary change").expect("ok");
    let WsMessage::Text(body) = third else {
        panic!("expected a text summary change");
    };
    assert!(
        !body.contains("background wire"),
        "the summary layer never carries an AgentEvent"
    );
    let body: serde_json::Value = serde_json::from_str(&body).expect("summary JSON");
    assert_eq!(body["type"], "session_summary_changed");
    assert_eq!(body["event"]["session_id"], "s2");
    assert_eq!(body["workspace_sequence"], 1);

    let fourth = ws.next().await.expect("foreground event").expect("ok");
    let WsMessage::Text(body) = fourth else {
        panic!("expected a text session event");
    };
    let body: serde_json::Value = serde_json::from_str(&body).expect("event JSON");
    assert_eq!(body["type"], "session_event");
    assert_eq!(body["event"]["TextDelta"]["text"], "foreground wire");
    let _ = ws.close(None).await;
}

#[tokio::test]
async fn unknown_or_unloaded_session_never_receives_an_empty_replay() {
    // A session that does not exist at all is rejected before any replay.
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;
    let mut ws = ws_connect(server.addr, &cookie, &server.origin())
        .await
        .expect("websocket upgrade");
    watch(&mut ws, "never_persisted", None).await;
    let message = ws
        .next()
        .await
        .expect("watch error")
        .expect("watch error ok");
    let WsMessage::Text(body) = message else {
        panic!("expected a text watch error");
    };
    let body: WebUiErrorBody = serde_json::from_str(&body).expect("error shape");
    assert_eq!(body.code, neo_webui::WebUiErrorCode::NotFound);
    let _ = ws.close(None).await;

    // A persisted-but-never-loaded session has relay watermark 0: even a
    // cursor at sequence 0 must fall back to a full snapshot — an empty
    // replay would silently drop the whole history.
    let relay = make_relay("svc_unloaded");
    let outcome = relay.subscribe_session(
        1,
        "persisted_elsewhere",
        Some(WebUiCursor {
            stream_id: "svc_unloaded".to_string(),
            sequence: 0,
        }),
    );
    assert_eq!(outcome.mode, SubscribeMode::NeedsSnapshot);
    assert_eq!(outcome.watermark, 0);
    assert!(
        outcome.queue.drain_sendable().is_empty(),
        "no empty replay is ever queued"
    );
}

#[tokio::test]
async fn retried_snapshot_excludes_failed_attempt_text() {
    let relay = Arc::new(make_relay("svc_retry"));
    let host = FakeHost::new(relay.clone());
    host.create_with_id("s1");
    host.publish("s1", text(1, "good one"));
    host.publish(
        "s1",
        AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 2,
            delay_ms: 5,
            error_code: "provider.rate_limit".to_string(),
            message: "limited".to_string(),
        },
    );
    host.publish("s1", text(1, "failed attempt text"));
    host.publish("s1", AgentEvent::RetryResumed { turn: 1, retry: 1 });
    host.publish("s1", text(1, "final text"));

    let outcome = relay.subscribe_session(1, "s1", None);
    assert_eq!(outcome.mode, SubscribeMode::NeedsSnapshot);
    let snapshot = host.subscribe("s1").await.expect("session exists");
    assert_eq!(
        snapshot.watermark, 5,
        "watermark covers the retried sequence"
    );
    let json = serde_json::to_string(&snapshot).expect("snapshot serializes");
    assert!(json.contains("good one"));
    assert!(json.contains("final text"));
    assert!(
        !json.contains("failed attempt text"),
        "failed attempts never appear in a reconnected projection"
    );
}

#[test]
fn fixture_parses_into_protocol_types_and_covers_required_samples() {
    let raw = include_str!("../../fixtures/webui-events.json");
    let root: serde_json::Value = serde_json::from_str(raw).expect("fixture is valid JSON");

    let sessions = root["sessions"].as_array().expect("two sessions");
    assert_eq!(sessions.len(), 2, "fixture has two sessions");

    let mut event_kinds = HashSet::new();
    let mut envelope_kinds = HashSet::new();
    let mut opaque_outputs = 0usize;
    let collect_envelopes = |value: &serde_json::Value,
                             event_kinds: &mut HashSet<String>,
                             envelope_kinds: &mut HashSet<String>,
                             opaque_outputs: &mut usize| {
        let envelopes: Vec<WebUiServerMessage> =
            serde_json::from_value(value.clone()).expect("envelope shapes match protocol");
        for envelope in &envelopes {
            match envelope {
                WebUiServerMessage::SessionEvent { event, output, .. } => {
                    event_kinds.insert(variant_name(event));
                    if let Some(reference) = output {
                        assert!(!reference.id.is_empty(), "opaque output id is set");
                        *opaque_outputs += 1;
                    }
                    let serialized = serde_json::to_value(event).expect("event serializes");
                    assert!(
                        !serialized.to_string().contains("\"output_ref\""),
                        "structured tool output references never reach the web wire"
                    );
                }
                WebUiServerMessage::SessionState { .. } => {
                    envelope_kinds.insert("session_state".to_string());
                }
                WebUiServerMessage::SessionMetadataChanged { .. } => {
                    envelope_kinds.insert("session_metadata_changed".to_string());
                }
                WebUiServerMessage::WorkspaceSnapshot { .. } => {
                    envelope_kinds.insert("workspace_snapshot".to_string());
                }
                WebUiServerMessage::SessionSummaryChanged { .. } => {
                    envelope_kinds.insert("session_summary_changed".to_string());
                }
                WebUiServerMessage::SessionSnapshot { .. } => {
                    envelope_kinds.insert("session_snapshot".to_string());
                }
            }
        }
    };

    let mut watermark_and_resume_ok = false;
    for session in sessions {
        let snapshot: WebUiSnapshot =
            serde_json::from_value(session["snapshot"].clone()).expect("snapshot shape");
        for entry in &snapshot.history {
            event_kinds.insert(variant_name(&entry.event));
        }
        collect_envelopes(
            &session["after_snapshot"],
            &mut event_kinds,
            &mut envelope_kinds,
            &mut opaque_outputs,
        );
        if let Some(replay) = session["replay_after_cursor"].as_object() {
            let after: WebUiCursor =
                serde_json::from_value(replay["after"].clone()).expect("cursor shape");
            let envelopes: Vec<WebUiServerMessage> =
                serde_json::from_value(replay["envelopes"].clone()).expect("replay shape");
            let resumed = sequences(&envelopes);
            assert_eq!(
                resumed.first().copied(),
                Some(after.sequence + 1),
                "replay resumes at cursor + 1"
            );
            for entry in &envelopes {
                if let WebUiServerMessage::SessionEvent { event, .. } = entry {
                    event_kinds.insert(variant_name(event));
                }
            }
            watermark_and_resume_ok = true;
        }
        assert!(snapshot.watermark > 0, "snapshot carries a watermark");
        // MessageAppended carries the user turn; it is the only user-bubble
        // source on the web wire.
        assert!(
            snapshot
                .history
                .iter()
                .any(|entry| matches!(entry.event, AgentEvent::MessageAppended { .. })),
            "snapshot history contains the submitted user message"
        );
    }
    assert!(
        watermark_and_resume_ok,
        "fixture shows watermark plus resume"
    );

    // A different stream_id replaces the snapshot for the same session.
    let replacement = &root["snapshot_replacement"];
    let replacement_stream = replacement["stream_id"]
        .as_str()
        .expect("replacement stream");
    assert_ne!(
        replacement_stream,
        root["stream_id"].as_str().expect("base stream"),
        "replacement uses a different stream_id"
    );
    let replacement_snapshot: WebUiSnapshot =
        serde_json::from_value(replacement["snapshot"].clone()).expect("replacement snapshot");
    assert_eq!(replacement_snapshot.session_id, "session_0002");
    collect_envelopes(
        &replacement["after_snapshot"],
        &mut event_kinds,
        &mut envelope_kinds,
        &mut opaque_outputs,
    );
    for entry in &replacement_snapshot.history {
        event_kinds.insert(variant_name(&entry.event));
    }

    // Retry retraction: the live stream carries the failed attempt, the
    // replacement snapshot does not.
    assert!(
        sessions[1]["after_snapshot"]
            .to_string()
            .contains("这次尝试的输出会在重试时被撤回。"),
        "live stream shows the failed attempt"
    );
    assert!(
        !replacement["snapshot"]
            .to_string()
            .contains("这次尝试的输出会在重试时被撤回。"),
        "snapshot after retry omits the failed attempt"
    );

    for required in [
        "MessageAppended",
        "MessageStarted",
        "TextDelta",
        "ThinkingStarted",
        "ThinkingDelta",
        "ThinkingFinished",
        "ToolExecutionQueued",
        "ToolExecutionQueueUpdated",
        "ToolExecutionStarted",
        "ToolExecutionFinished",
        "ApprovalRequested",
        "ApprovalResolved",
        "QuestionRequested",
        "RetryScheduled",
        "RetryResumed",
        "TodoUpdated",
        "DelegateStarted",
        "DelegateUpdated",
        "DelegateProgressUpdated",
        "DelegateFinished",
        "DelegateSwarmStarted",
        "DelegateSwarmUpdated",
        "DelegateSwarmProgressUpdated",
        "DelegateSwarmFinished",
        "WorkflowUpdated",
        "WorkflowFinished",
        "TerminalSessionOutput",
        "TokenUsage",
        "ContextWindowUpdated",
    ] {
        assert!(
            event_kinds.contains(required),
            "fixture misses required event {required}"
        );
    }
    assert!(envelope_kinds.contains("session_state"));
    assert!(envelope_kinds.contains("session_metadata_changed"));
    assert!(
        opaque_outputs >= 3,
        "tool lifecycle and terminal output travel as opaque references"
    );

    // The two-tier wire: one workspace summary subscription plus one full
    // session subscription on a single connection.
    let long_connection = &root["long_connection"];
    let client_messages: Vec<neo_webui::protocol::WebUiWatchRequest> =
        serde_json::from_value(long_connection["client_messages"].clone())
            .expect("watch requests match protocol");
    assert!(
        client_messages.iter().any(|request| matches!(
            request,
            neo_webui::protocol::WebUiWatchRequest::WatchWorkspace { .. }
        )),
        "fixture shows the workspace subscription"
    );
    assert!(
        client_messages.iter().any(|request| matches!(
            request,
            neo_webui::protocol::WebUiWatchRequest::WatchSession { .. }
        )),
        "fixture shows the full session subscription"
    );
    let workspace_snapshot: Vec<WebUiServerMessage> = serde_json::from_value(serde_json::json!([
        long_connection["workspace_snapshot"].clone()
    ]))
    .expect("workspace snapshot matches protocol");
    let WebUiServerMessage::WorkspaceSnapshot { workspaces, .. } = &workspace_snapshot[0] else {
        panic!("workspace wire starts with a workspace_snapshot");
    };
    let grouped: usize = workspaces.iter().map(|group| group.sessions.len()).sum();
    assert_eq!(grouped, 3, "workspace snapshot groups every session");
    assert_eq!(workspaces.len(), 2, "two workspace groups");
    assert!(
        workspaces.iter().any(|group| group.current),
        "one group is the current workspace"
    );
    assert!(
        workspaces.iter().all(|group| group
            .sessions
            .iter()
            .all(|s| s.workspace_label == group.label)),
        "every session carries its group label, never a path"
    );
    let summary: Vec<WebUiServerMessage> = serde_json::from_value(serde_json::json!([
        long_connection["session_summary_changed"].clone()
    ]))
    .expect("summary change matches protocol");
    let WebUiServerMessage::SessionSummaryChanged { .. } = &summary[0] else {
        panic!("summary layer delivers session_summary_changed");
    };

    assert!(
        !root.to_string().contains("\"output_ref\""),
        "structured tool output references never appear in web samples"
    );
    assert!(
        root.to_string().contains("+37 −26"),
        "workflow sample carries change statistics"
    );

    // Error samples cover the documented short codes.
    let errors: Vec<WebUiErrorBody> =
        serde_json::from_value(root["errors"].clone()).expect("error shapes");
    let codes: Vec<neo_webui::WebUiErrorCode> = errors.iter().map(|error| error.code).collect();
    for required in [
        neo_webui::WebUiErrorCode::Unauthorized,
        neo_webui::WebUiErrorCode::StaleControl,
        neo_webui::WebUiErrorCode::StaleTurn,
        neo_webui::WebUiErrorCode::NoActiveTurn,
        neo_webui::WebUiErrorCode::OutputNotInSession,
    ] {
        assert!(
            codes.contains(&required),
            "fixture misses error {required:?}"
        );
    }

    // Slow-consumer close sample.
    let closes = root["close_samples"].as_array().expect("close samples");
    assert!(
        closes.iter().any(|close| close["code"] == 1013),
        "fixture shows the slow-consumer 1013 close"
    );
}

fn variant_name(event: &AgentEvent) -> String {
    serde_json::to_value(event)
        .expect("event serializes")
        .as_object()
        .and_then(|object| object.keys().next().cloned())
        .expect("externally tagged event")
}

#[tokio::test]
async fn websocket_watch_delivers_snapshot_then_live_events() {
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;
    server.host.create_with_id("s_ws");
    let mut ws = ws_connect(server.addr, &cookie, &server.origin())
        .await
        .expect("websocket upgrade with cookie and origin");
    watch(&mut ws, "s_ws", None).await;

    let first = ws
        .next()
        .await
        .expect("snapshot message")
        .expect("snapshot ok");
    let WsMessage::Text(snapshot) = first else {
        panic!("expected a text snapshot");
    };
    let snapshot: serde_json::Value = serde_json::from_str(&snapshot).expect("snapshot JSON");
    assert_eq!(snapshot["type"], "session_snapshot");
    assert_eq!(snapshot["snapshot"]["session_id"], "s_ws");
    assert_eq!(snapshot["snapshot"]["watermark"], 0);

    server.host.publish("s_ws", text(1, "live one"));
    server.host.publish("s_ws", text(1, "live two"));
    let second = ws.next().await.expect("live event").expect("live event ok");
    let WsMessage::Text(envelope) = second else {
        panic!("expected a text envelope");
    };
    let envelope: serde_json::Value = serde_json::from_str(&envelope).expect("envelope JSON");
    assert_eq!(envelope["type"], "session_event");
    assert_eq!(envelope["sequence"], 1);
    assert_eq!(envelope["event"]["TextDelta"]["text"], "live one");

    let third = ws
        .next()
        .await
        .expect("second live event")
        .expect("live event ok");
    let WsMessage::Text(envelope) = third else {
        panic!("expected a text envelope");
    };
    let envelope: serde_json::Value = serde_json::from_str(&envelope).expect("envelope JSON");
    assert_eq!(envelope["sequence"], 2);
    let _ = ws.close(None).await;
}

#[tokio::test]
async fn websocket_oversized_inbound_frame_closes_the_connection() {
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;
    server.host.create_with_id("s_big");
    let mut ws = ws_connect(server.addr, &cookie, &server.origin())
        .await
        .expect("websocket upgrade");
    watch(&mut ws, "s_big", None).await;
    let _ = ws.next().await.expect("snapshot");

    // A frame beyond the 64 KiB inbound limit ends the connection with a
    // protocol close (1009) instead of being buffered.
    ws.send(WsMessage::Text("z".repeat(65 * 1024)))
        .await
        .expect("send oversized frame");
    let close = ws.next().await;
    match close {
        Some(Err(_)) => {}
        Some(Ok(WsMessage::Close(Some(frame)))) => {
            assert_eq!(
                frame.code,
                tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Size
            );
        }
        other => panic!("expected a size close, got {other:?}"),
    }
}

#[tokio::test]
async fn fragmented_websocket_message_over_limit_is_rejected() {
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;
    server.host.create_with_id("s_frag");
    let mut ws = ws_connect(server.addr, &cookie, &server.origin())
        .await
        .expect("websocket upgrade");
    watch(&mut ws, "s_frag", None).await;
    let _ = ws.next().await.expect("snapshot");

    // Two 48 KiB fragments: each frame fits the 64 KiB frame limit, but the
    // reassembled 96 KiB message exceeds the message limit and must be
    // rejected with a 1009 size close.
    use tokio_tungstenite::tungstenite::protocol::frame::{
        Frame,
        coding::{Data, OpCode},
    };
    let half = "y".repeat(48 * 1024).into_bytes();
    ws.send(WsMessage::Frame(Frame::message(
        half.clone(),
        OpCode::Data(Data::Text),
        false,
    )))
    .await
    .expect("first fragment");
    ws.send(WsMessage::Frame(Frame::message(
        half,
        OpCode::Data(Data::Continue),
        true,
    )))
    .await
    .expect("final fragment");

    let close = ws.next().await;
    match close {
        Some(Ok(WsMessage::Close(Some(frame)))) => {
            assert_eq!(
                frame.code,
                tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Size
            );
        }
        Some(Err(_)) => {}
        other => panic!("expected a size close, got {other:?}"),
    }
}

#[tokio::test]
async fn slow_websocket_client_receives_1013_and_is_deregistered() {
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;
    server.host.create_with_id("s_slow");
    let mut ws = ws_connect(server.addr, &cookie, &server.origin())
        .await
        .expect("websocket upgrade");
    watch(&mut ws, "s_slow", None).await;
    let _ = ws.next().await.expect("snapshot");
    assert_eq!(server.relay.observer_count(), 1);

    // Flood synchronously: the send loop cannot drain between publishes, the
    // bounded queue overflows, and the relay queues one 1013 close then drops
    // the observer instead of growing memory.
    for index in 0..CONNECTION_QUEUE_MESSAGES * 2 {
        server
            .host
            .publish("s_slow", text(1, &format!("flood {index}")));
    }
    assert_eq!(
        server.relay.observer_count(),
        0,
        "overflow deregisters the observer"
    );

    // The client eventually reads the queued envelopes followed by exactly
    // one 1013 slow-consumer close.
    let mut close_code = None;
    while let Some(message) = ws.next().await {
        match message {
            Ok(WsMessage::Close(Some(frame))) => {
                close_code = Some(u16::from(frame.code));
                break;
            }
            Ok(_) => {}
            Err(error) => panic!("unexpected websocket error: {error}"),
        }
    }
    assert_eq!(close_code, Some(WS_CLOSE_SLOW_CONSUMER));
}

#[tokio::test]
async fn authenticated_http_reads_and_writes_round_trip_through_the_host() {
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;

    // Write: create a session, then read its snapshot and list it.
    let created = raw_request(
        server.addr,
        RawRequest {
            method: "POST".to_string(),
            path: "/api/sessions".to_string(),
            origin: Some(server.origin()),
            cookie: Some(cookie.clone()),
            content_type: Some("application/json".to_string()),
            body: br#"{"message":"hello webui"}"#.to_vec(),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(created.status, 201);
    let session_id = created.json()["session_id"]
        .as_str()
        .expect("created session id")
        .to_string();
    assert!(
        created.json()["stream_id"].is_string(),
        "201 carries the stream_id"
    );
    assert!(
        created.json()["sequence"].is_u64(),
        "201 carries the resume cursor"
    );

    let snapshot = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: format!("/api/sessions/{session_id}/snapshot"),
            cookie: Some(cookie.clone()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(snapshot.status, 200);
    assert_eq!(snapshot.json()["session_id"], session_id);

    let list = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/sessions".to_string(),
            cookie: Some(cookie.clone()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(list.status, 200);
    assert_eq!(list.json()["items"][0]["session_id"], session_id);

    // Metadata patch round-trips through the host.
    let patched = raw_request(
        server.addr,
        RawRequest {
            method: "PATCH".to_string(),
            path: format!("/api/sessions/{session_id}"),
            origin: Some(server.origin()),
            cookie: Some(cookie.clone()),
            content_type: Some("application/json".to_string()),
            body: br#"{"title":"renamed","pinned":true}"#.to_vec(),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(patched.status, 200);
    assert_eq!(patched.json()["title"], "renamed");
    assert_eq!(patched.json()["pinned"], true);

    // Unknown session is a stable not_found error.
    let missing = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/sessions/session_9999/snapshot".to_string(),
            cookie: Some(cookie.clone()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(missing.status, 404);
    assert_eq!(missing.error_code(), "not_found");
}
