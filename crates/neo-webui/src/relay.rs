//! Bounded per-session event relay with a two-tier subscription per
//! connection.
//!
//! One service start owns one `stream_id`; sequences increase monotonically
//! per session, and workspace summaries increase monotonically in their own
//! `workspace_sequence` space. Every event is cached in a bounded per-session
//! window (globally bounded too) so a reconnect can resume without a snapshot
//! while the cache is contiguous. Full tool and terminal output never enters
//! the cache — only opaque `WebUiOutputRef` values. Each connection gets one
//! bounded non-blocking outbound queue shared by its two subscriptions; a
//! slow consumer is deregistered and closed with WebSocket `1013` instead of
//! growing an unbounded buffer.
//!
//! A connection holds at most two subscriptions over the single queue:
//!
//! - a workspace summary subscription (small `WebUiSessionSummary` updates
//!   for every session, never an `AgentEvent`), and
//! - one full-session subscription (snapshot, `AgentEvent`, state and
//!   metadata for one `session_id`).
//!
//! Re-subscribing one layer clears only that layer's pending messages; the
//! other layer keeps flowing. Subscription is atomic under the relay lock:
//! observer registration plus watermark recording happen together, then the
//! snapshot is delivered and contiguous events resume from `watermark + 1`.
//! Different `stream_id`, impossible cursors, sessions the relay has never
//! seen, non-contiguous or evicted ranges all fall back to a full snapshot —
//! an unknown or never-cached session never produces an empty replay. The
//! server never creates gaps or reorders; duplicate sequences are allowed and
//! deduplicated by the frontend.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;

use crate::protocol::{WebUiCursor, WebUiEventBody, WebUiServerMessage, WebUiSessionSummary};

// ── Fixed, tested limits (single source of truth; no magic numbers) ──────

/// Per-session event cache budget.
pub const SESSION_EVENT_CACHE_BYTES: usize = 256 * 1024;
/// Whole-service event cache budget.
pub const GLOBAL_EVENT_CACHE_BYTES: usize = 4 * 1024 * 1024;
/// One web command body (HTTP) limit.
pub const COMMAND_BODY_LIMIT_BYTES: usize = 256 * 1024;
/// One long-connection inbound frame limit; also the whole inbound message
/// limit so fragmented messages cannot bypass it.
pub const WS_FRAME_LIMIT_BYTES: usize = 64 * 1024;
/// Pending outbound data per connection.
pub const CONNECTION_QUEUE_BYTES: usize = 512 * 1024;
/// Pending outbound messages per connection.
pub const CONNECTION_QUEUE_MESSAGES: usize = 256;
/// Tool-output range read cap per request.
pub const TOOL_OUTPUT_MAX_LINES: u32 = 1_000;
/// Session-list page cap.
pub const SESSION_PAGE_LIMIT: usize = 100;
/// Decoded byte cap for one uploaded attachment.
pub const ATTACHMENT_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Maximum attachments accepted on one message.
pub const ATTACHMENTS_PER_MESSAGE_MAX: usize = 4;
/// `POST /api/attachments` body cap: base64 of [`ATTACHMENT_MAX_BYTES`] plus
/// JSON envelope slack.
pub const ATTACHMENT_BODY_LIMIT_BYTES: usize = 12 * 1024 * 1024;
/// Deadline for the first subscribe frame on a fresh long connection.
pub const FIRST_SUBSCRIBE_DEADLINE: Duration = Duration::from_secs(5);
/// WebSocket close code for a slow consumer whose bounded queue overflowed.
pub const WS_CLOSE_SLOW_CONSUMER: u16 = 1013;

/// Which subscription layer of a connection a message belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionLayer {
    /// Full-session subscription (snapshot, events, state, metadata).
    Session,
    /// Workspace summary subscription (summaries only, never `AgentEvent`).
    Workspace,
}

/// One outbound message of a connection queue.
#[derive(Debug, Clone)]
pub enum OutboundMessage {
    /// Serialized session-layer `WebUiServerMessage` JSON.
    SessionJson(Arc<str>),
    /// Serialized workspace-layer `WebUiServerMessage` JSON.
    WorkspaceJson(Arc<str>),
    /// WebSocket close with a code (currently only `1013`).
    Close { code: u16 },
}

impl OutboundMessage {
    fn byte_len(&self) -> usize {
        match self {
            Self::SessionJson(json) | Self::WorkspaceJson(json) => json.len(),
            Self::Close { .. } => 0,
        }
    }

    fn layer(&self) -> Option<SubscriptionLayer> {
        match self {
            Self::SessionJson(_) => Some(SubscriptionLayer::Session),
            Self::WorkspaceJson(_) => Some(SubscriptionLayer::Workspace),
            Self::Close { .. } => None,
        }
    }

    fn json(layer: SubscriptionLayer, json: Arc<str>) -> Self {
        match layer {
            SubscriptionLayer::Session => Self::SessionJson(json),
            SubscriptionLayer::Workspace => Self::WorkspaceJson(json),
        }
    }
}

#[derive(Debug, Default)]
struct QueueInner {
    messages: VecDeque<OutboundMessage>,
    bytes: usize,
    /// Per-layer readiness: buffered events of a layer are held back until
    /// that layer's snapshot (or replay) is delivered, so a snapshot always
    /// precedes the live events of its own layer.
    ready_session: bool,
    ready_workspace: bool,
    closed: bool,
}

/// Bounded outbound queue shared between the relay (producer) and the
/// connection task (single consumer). One queue serves both subscription
/// layers of the connection; the byte and message bounds cover the sum.
#[derive(Debug)]
pub struct ObserverQueue {
    inner: Mutex<QueueInner>,
    notify: Notify,
}

impl ObserverQueue {
    fn new() -> Self {
        Self {
            inner: Mutex::new(QueueInner::default()),
            notify: Notify::new(),
        }
    }

    /// Wait for the next push/close notification. Check-then-wait with a
    /// single consumer and `notify_one` guarantees no missed wakeup.
    pub fn notified(&self) -> impl Future<Output = ()> + '_ {
        self.notify.notified()
    }

    /// Take every message that may be sent now: messages of a layer whose
    /// snapshot/replay was delivered, and close frames. Held messages stay
    /// queued and keep their byte accounting.
    pub fn drain_sendable(&self) -> Vec<OutboundMessage> {
        let mut inner = self.inner.lock().expect("observer queue poisoned");
        let mut drained = Vec::new();
        let mut remaining = VecDeque::new();
        let mut bytes = 0usize;
        for message in std::mem::take(&mut inner.messages) {
            let sendable = match message.layer() {
                Some(SubscriptionLayer::Session) => inner.ready_session,
                Some(SubscriptionLayer::Workspace) => inner.ready_workspace,
                None => true,
            };
            if sendable {
                drained.push(message);
            } else {
                bytes += message.byte_len();
                remaining.push_back(message);
            }
        }
        inner.messages = remaining;
        inner.bytes = bytes;
        drained
    }

    /// Whether the connection was closed by the relay or the producer.
    pub fn is_closed(&self) -> bool {
        self.inner.lock().expect("observer queue poisoned").closed
    }

    /// Re-subscribing one layer drops only that layer's pending messages
    /// (they would duplicate the new snapshot); the other layer's messages
    /// and readiness are untouched.
    fn clear_layer(&self, layer: SubscriptionLayer) {
        let mut inner = self.inner.lock().expect("observer queue poisoned");
        if inner.closed {
            return;
        }
        inner
            .messages
            .retain(|message| message.layer() != Some(layer));
        inner.bytes = inner.messages.iter().map(OutboundMessage::byte_len).sum();
        match layer {
            SubscriptionLayer::Session => inner.ready_session = false,
            SubscriptionLayer::Workspace => inner.ready_workspace = false,
        }
    }

    fn mark_closed(&self) {
        self.inner.lock().expect("observer queue poisoned").closed = true;
        self.notify.notify_one();
    }

    fn set_ready(&self, layer: SubscriptionLayer) {
        let mut inner = self.inner.lock().expect("observer queue poisoned");
        match layer {
            SubscriptionLayer::Session => inner.ready_session = true,
            SubscriptionLayer::Workspace => inner.ready_workspace = true,
        }
        self.notify.notify_one();
    }

    /// Push one event message. Returns `false` when the bounded queue is
    /// full: the observer is then deregistered and closed with `1013`.
    fn try_push(&self, message: OutboundMessage) -> bool {
        let mut inner = self.inner.lock().expect("observer queue poisoned");
        if inner.closed {
            return false;
        }
        // A single message (e.g. a large event) may exceed the byte budget
        // when the queue is otherwise empty; everything beyond the first
        // message must keep the total within the bound. The message-count
        // bound always applies.
        let byte_ok =
            inner.messages.is_empty() || inner.bytes + message.byte_len() <= CONNECTION_QUEUE_BYTES;
        if inner.messages.len() >= CONNECTION_QUEUE_MESSAGES || !byte_ok {
            inner.closed = true;
            inner.messages.clear();
            inner.bytes = 0;
            inner.messages.push_back(OutboundMessage::Close {
                code: WS_CLOSE_SLOW_CONSUMER,
            });
            self.notify.notify_one();
            return false;
        }
        inner.bytes += message.byte_len();
        inner.messages.push_back(message);
        self.notify.notify_one();
        true
    }

    /// Insert one layer's snapshot ahead of that layer's held live events
    /// and release the layer. Snapshot bytes are not counted against
    /// [`CONNECTION_QUEUE_BYTES`]: that budget tracks real-time events only,
    /// so a large recovery snapshot cannot make every later live event look
    /// like an overflow and close the connection in a `1013` reconnect loop.
    fn insert_snapshot(&self, layer: SubscriptionLayer, snapshot_json: Arc<str>) {
        let mut inner = self.inner.lock().expect("observer queue poisoned");
        if inner.closed {
            return;
        }
        let position = inner
            .messages
            .iter()
            .position(|message| message.layer() == Some(layer))
            .unwrap_or(inner.messages.len());
        inner
            .messages
            .insert(position, OutboundMessage::json(layer, snapshot_json));
        match layer {
            SubscriptionLayer::Session => inner.ready_session = true,
            SubscriptionLayer::Workspace => inner.ready_workspace = true,
        }
        self.notify.notify_one();
    }
}

#[derive(Debug, Clone)]
struct CachedEvent {
    seq: u64,
    order: u64,
    bytes: usize,
    json: Arc<str>,
}

/// One monotonically sequenced, bounded event window. Used per session and
/// once for the workspace summary stream.
#[derive(Debug, Default)]
struct SessionRelay {
    sequence: u64,
    cache: VecDeque<CachedEvent>,
    cache_bytes: usize,
}

#[derive(Debug)]
struct Observer {
    /// Full-session subscription target (`None` while only the workspace
    /// summary layer is subscribed).
    session_id: Option<String>,
    /// Whether the workspace summary layer is subscribed.
    workspace: bool,
    queue: Arc<ObserverQueue>,
}

#[derive(Debug, Default)]
struct RelayState {
    sessions: HashMap<String, SessionRelay>,
    workspace: SessionRelay,
    observers: HashMap<u64, Observer>,
    total_cache_bytes: usize,
    order: u64,
}

#[derive(Debug)]
struct RelayInner {
    stream_id: String,
    state: Mutex<RelayState>,
}

/// Per-session event source handed to the host.
#[derive(Clone, Debug)]
pub struct EventPublisher {
    relay: Arc<RelayInner>,
    session_id: String,
}

impl EventPublisher {
    /// Publish one transport payload; returns the assigned sequence.
    pub fn publish(&self, event: WebUiEventBody) -> u64 {
        self.relay.publish(&self.session_id, event)
    }
}

/// Result of an atomic subscribe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscribeMode {
    /// Cache is contiguous from the cursor: replay was queued, no snapshot.
    Replay,
    /// Cursor invalid, stream changed, session never cached, or cache
    /// non-contiguous: the caller must deliver a full snapshot via
    /// [`Relay::deliver_snapshot`].
    NeedsSnapshot,
}

/// Outcome of [`Relay::subscribe_session`] / [`Relay::subscribe_workspace`].
#[derive(Debug)]
pub struct SubscribeOutcome {
    pub mode: SubscribeMode,
    /// Sequence watermark recorded at subscribe time (per-session sequence
    /// for the session layer, workspace sequence for the workspace layer).
    pub watermark: u64,
    /// The connection's outbound queue (shared with the send loop).
    pub queue: Arc<ObserverQueue>,
}

/// Bounded relay: sequences, caches, observers, and their queues.
#[derive(Debug, Clone)]
pub struct Relay {
    inner: Arc<RelayInner>,
}

impl Relay {
    #[must_use]
    pub fn new(stream_id: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(RelayInner {
                stream_id: stream_id.into(),
                state: Mutex::new(RelayState::default()),
            }),
        }
    }

    #[must_use]
    pub fn stream_id(&self) -> &str {
        &self.inner.stream_id
    }

    /// Current sequence of one session (0 when unknown).
    #[must_use]
    pub fn current_sequence(&self, session_id: &str) -> u64 {
        self.inner
            .state
            .lock()
            .expect("relay state poisoned")
            .sessions
            .get(session_id)
            .map_or(0, |session| session.sequence)
    }

    /// Current workspace summary sequence (0 before the first summary).
    #[must_use]
    pub fn workspace_sequence(&self) -> u64 {
        self.inner
            .state
            .lock()
            .expect("relay state poisoned")
            .workspace
            .sequence
    }

    /// Current number of registered connection observers. Test support for
    /// asserting slow-consumer deregistration over a real connection.
    #[doc(hidden)]
    #[must_use]
    pub fn observer_count(&self) -> usize {
        self.inner
            .state
            .lock()
            .expect("relay state poisoned")
            .observers
            .len()
    }

    /// Create a per-session publisher.
    #[must_use]
    pub fn publisher(&self, session_id: impl Into<String>) -> EventPublisher {
        EventPublisher {
            relay: self.inner.clone(),
            session_id: session_id.into(),
        }
    }

    /// Publish one workspace summary update: assigns the workspace sequence,
    /// caches the small envelope and delivers it to every workspace-summary
    /// observer. Summaries never carry an `AgentEvent`.
    pub fn publish_summary(&self, summary: WebUiSessionSummary) -> u64 {
        self.inner.publish_summary(summary)
    }

    /// Atomically (re)bind one connection's full-session layer: register the
    /// observer, record the watermark, and queue any contiguous replay. Only
    /// the session layer of a previous binding is cleared — the workspace
    /// summary layer keeps flowing across session switches.
    pub fn subscribe_session(
        &self,
        conn_id: u64,
        session_id: &str,
        after: Option<WebUiCursor>,
    ) -> SubscribeOutcome {
        let mut state = self.inner.state.lock().expect("relay state poisoned");
        let (queue, workspace) = match state.observers.get(&conn_id) {
            Some(previous) => {
                previous.queue.clear_layer(SubscriptionLayer::Session);
                (previous.queue.clone(), previous.workspace)
            }
            None => (Arc::new(ObserverQueue::new()), false),
        };
        let session = state.sessions.entry(session_id.to_string()).or_default();
        let watermark = session.sequence;
        let mode = match resume(&self.inner.stream_id, session, after.as_ref()) {
            Resume::Snapshot => SubscribeMode::NeedsSnapshot,
            Resume::Replay(jsons) => {
                for json in jsons {
                    let _ = queue.try_push(OutboundMessage::SessionJson(json));
                }
                queue.set_ready(SubscriptionLayer::Session);
                SubscribeMode::Replay
            }
        };
        state.observers.insert(
            conn_id,
            Observer {
                session_id: Some(session_id.to_string()),
                workspace,
                queue: queue.clone(),
            },
        );
        SubscribeOutcome {
            mode,
            watermark,
            queue,
        }
    }

    /// Atomically (re)bind one connection's workspace summary layer. Only
    /// the workspace layer of a previous binding is cleared — the full-session
    /// layer keeps flowing.
    pub fn subscribe_workspace(
        &self,
        conn_id: u64,
        after: Option<WebUiCursor>,
    ) -> SubscribeOutcome {
        let mut state = self.inner.state.lock().expect("relay state poisoned");
        let (queue, session_id) = match state.observers.get(&conn_id) {
            Some(previous) => {
                previous.queue.clear_layer(SubscriptionLayer::Workspace);
                (previous.queue.clone(), previous.session_id.clone())
            }
            None => (Arc::new(ObserverQueue::new()), None),
        };
        let watermark = state.workspace.sequence;
        let mode = match resume(&self.inner.stream_id, &state.workspace, after.as_ref()) {
            Resume::Snapshot => SubscribeMode::NeedsSnapshot,
            Resume::Replay(jsons) => {
                for json in jsons {
                    let _ = queue.try_push(OutboundMessage::WorkspaceJson(json));
                }
                queue.set_ready(SubscriptionLayer::Workspace);
                SubscribeMode::Replay
            }
        };
        state.observers.insert(
            conn_id,
            Observer {
                session_id,
                workspace: true,
                queue: queue.clone(),
            },
        );
        SubscribeOutcome {
            mode,
            watermark,
            queue,
        }
    }

    /// Deliver one layer's full snapshot ahead of that layer's held live
    /// events and release the layer. The snapshot is the recovery path: it
    /// is always queued (single message) even if it exceeds the byte budget,
    /// so large histories cannot wedge a reconnect; its bytes are not counted
    /// against [`CONNECTION_QUEUE_BYTES`].
    pub fn deliver_snapshot(
        &self,
        conn_id: u64,
        layer: SubscriptionLayer,
        snapshot_json: Arc<str>,
    ) {
        let state = self.inner.state.lock().expect("relay state poisoned");
        if let Some(observer) = state.observers.get(&conn_id) {
            observer.queue.insert_snapshot(layer, snapshot_json);
        }
    }

    /// Remove one connection: its queue is closed so the send loop exits.
    /// Idempotent.
    pub fn deregister(&self, conn_id: u64) {
        let mut state = self.inner.state.lock().expect("relay state poisoned");
        if let Some(observer) = state.observers.remove(&conn_id) {
            observer.queue.mark_closed();
        }
    }
}

/// Resume decision for one cache window and cursor.
enum Resume {
    /// The cursor cannot be served from the cache: deliver a full snapshot.
    Snapshot,
    /// Contiguous cached envelopes from `cursor.sequence + 1` to the
    /// watermark, already known to fit the connection queue.
    Replay(Vec<Arc<str>>),
}

/// Fixed resume rules: a different `stream_id`, an impossible cursor, a
/// window the relay has never sequenced (unknown or never-cached session —
/// an empty replay would hide the canonical history), a non-contiguous or
/// evicted range, or a replay that cannot fit the connection queue all fall
/// back to a full snapshot.
fn resume(stream_id: &str, window: &SessionRelay, after: Option<&WebUiCursor>) -> Resume {
    let watermark = window.sequence;
    let Some(cursor) = after else {
        return Resume::Snapshot;
    };
    if watermark == 0 || cursor.stream_id != stream_id || cursor.sequence > watermark {
        return Resume::Snapshot;
    }
    let mut expected = cursor.sequence + 1;
    let mut replay_bytes = 0usize;
    let mut jsons = Vec::new();
    for cached in &window.cache {
        if cached.seq <= cursor.sequence {
            continue;
        }
        if cached.seq != expected {
            break;
        }
        expected += 1;
        replay_bytes += cached.bytes;
        jsons.push(cached.json.clone());
    }
    let contiguous = expected == watermark + 1;
    // A replay that cannot fit the connection queue would overflow and
    // close in a reconnect loop; fall back to a full snapshot instead (the
    // snapshot covers the gap).
    let fits = jsons.len() < CONNECTION_QUEUE_MESSAGES
        && (jsons.is_empty() || replay_bytes <= CONNECTION_QUEUE_BYTES);
    if contiguous && fits {
        Resume::Replay(jsons)
    } else {
        Resume::Snapshot
    }
}

impl RelayInner {
    fn publish(&self, session_id: &str, event: WebUiEventBody) -> u64 {
        let mut state = self.state.lock().expect("relay state poisoned");
        // Sequences are 1-based per session: the first published event is
        // sequence 1, so a cursor of 0 unambiguously means "nothing seen".
        let sequence = {
            let session = state.sessions.entry(session_id.to_string()).or_default();
            session.sequence += 1;
            session.sequence
        };

        let envelope = match event {
            WebUiEventBody::SessionEvent { event, output } => WebUiServerMessage::SessionEvent {
                stream_id: self.stream_id.clone(),
                session_id: session_id.to_string(),
                sequence,
                event,
                output,
            },
            WebUiEventBody::SessionState(event) => WebUiServerMessage::SessionState {
                stream_id: self.stream_id.clone(),
                session_id: session_id.to_string(),
                sequence,
                event,
            },
            WebUiEventBody::SessionMetadataChanged(event) => {
                WebUiServerMessage::SessionMetadataChanged {
                    stream_id: self.stream_id.clone(),
                    session_id: session_id.to_string(),
                    sequence,
                    event,
                }
            }
        };
        let Some(json) = Self::serialize(&envelope) else {
            return sequence;
        };
        let layer_match = |observer: &Observer| observer.session_id.as_deref() == Some(session_id);
        self.deliver_locked(
            &mut state,
            Window::Session(session_id),
            sequence,
            json,
            OutboundMessage::SessionJson,
            layer_match,
        );
        sequence
    }

    fn publish_summary(&self, summary: WebUiSessionSummary) -> u64 {
        let mut state = self.state.lock().expect("relay state poisoned");
        let sequence = {
            state.workspace.sequence += 1;
            state.workspace.sequence
        };
        let envelope = WebUiServerMessage::SessionSummaryChanged {
            stream_id: self.stream_id.clone(),
            workspace_sequence: sequence,
            event: summary,
        };
        let Some(json) = Self::serialize(&envelope) else {
            return sequence;
        };
        self.deliver_locked(
            &mut state,
            Window::Workspace,
            sequence,
            json,
            OutboundMessage::WorkspaceJson,
            |observer| observer.workspace,
        );
        sequence
    }

    /// `AgentEvent` serialization cannot fail; an impossible failure still
    /// consumes the sequence (no gaps) and skips delivery.
    fn serialize(envelope: &WebUiServerMessage) -> Option<Arc<str>> {
        serde_json::to_string(envelope).ok().map(Arc::from)
    }

    /// Cache one serialized envelope in its window (bounded; oversized
    /// envelopes stay live-only and break contiguity) and deliver it to every
    /// matching observer without blocking.
    fn deliver_locked(
        &self,
        state: &mut RelayState,
        window: Window<'_>,
        sequence: u64,
        json: Arc<str>,
        layer: impl Fn(Arc<str>) -> OutboundMessage,
        matches: impl Fn(&Observer) -> bool,
    ) {
        let bytes = json.len();
        let window_ref = match window {
            Window::Session(session_id) => {
                state.sessions.get_mut(session_id).expect("session present")
            }
            Window::Workspace => &mut state.workspace,
        };
        // Cache only when the envelope fits the window's remaining budget;
        // oversized envelopes are still delivered live but break contiguity
        // so any later resume falls back to a snapshot.
        let remaining = SESSION_EVENT_CACHE_BYTES.saturating_sub(window_ref.cache_bytes);
        if bytes <= remaining {
            window_ref.cache_bytes += bytes;
            state.order += 1;
            let order = state.order;
            window_ref.cache.push_back(CachedEvent {
                seq: sequence,
                order,
                bytes,
                json: json.clone(),
            });
            state.total_cache_bytes += bytes;
            self.evict_locked(state);
        }

        // Deliver to every matching observer (non-blocking).
        let mut closing = Vec::new();
        for (conn_id, observer) in &state.observers {
            if !matches(observer) {
                continue;
            }
            if !observer.queue.try_push(layer(json.clone())) {
                closing.push(*conn_id);
            }
        }
        for conn_id in closing {
            state.observers.remove(&conn_id);
        }
    }

    /// Global eviction: drop the globally oldest cached event until the
    /// whole-service budget fits. Only queue fronts are removed, so every
    /// window keeps a contiguous tail.
    fn evict_locked(&self, state: &mut RelayState) {
        while state.total_cache_bytes > GLOBAL_EVENT_CACHE_BYTES {
            let mut oldest: Option<(u64, EvictTarget)> = state
                .workspace
                .cache
                .front()
                .map(|front| (front.order, EvictTarget::Workspace));
            for (session_id, session) in &state.sessions {
                if let Some(front) = session.cache.front()
                    && oldest
                        .as_ref()
                        .is_none_or(|(order, _)| front.order < *order)
                {
                    oldest = Some((front.order, EvictTarget::Session(session_id.clone())));
                }
            }
            let Some((_, target)) = oldest else {
                break;
            };
            let window_ref = match target {
                EvictTarget::Session(session_id) => state
                    .sessions
                    .get_mut(&session_id)
                    .expect("eviction target present"),
                EvictTarget::Workspace => &mut state.workspace,
            };
            if let Some(front) = window_ref.cache.pop_front() {
                window_ref.cache_bytes -= front.bytes;
                state.total_cache_bytes -= front.bytes;
            }
        }
    }
}

/// One cache window: a session, or the workspace summary stream.
enum Window<'a> {
    Session(&'a str),
    Workspace,
}

/// Eviction candidate: one session window or the workspace summary window.
enum EvictTarget {
    Session(String),
    Workspace,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_limits_are_centralized_and_exact() {
        assert_eq!(SESSION_EVENT_CACHE_BYTES, 256 * 1024);
        assert_eq!(GLOBAL_EVENT_CACHE_BYTES, 4 * 1024 * 1024);
        assert_eq!(COMMAND_BODY_LIMIT_BYTES, 256 * 1024);
        assert_eq!(WS_FRAME_LIMIT_BYTES, 64 * 1024);
        assert_eq!(CONNECTION_QUEUE_BYTES, 512 * 1024);
        assert_eq!(CONNECTION_QUEUE_MESSAGES, 256);
        assert_eq!(TOOL_OUTPUT_MAX_LINES, 1_000);
        assert_eq!(SESSION_PAGE_LIMIT, 100);
        assert_eq!(FIRST_SUBSCRIBE_DEADLINE, Duration::from_secs(5));
        assert_eq!(WS_CLOSE_SLOW_CONSUMER, 1013);
    }
}
