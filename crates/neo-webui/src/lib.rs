//! Local web UI crate: loopback HTTP service, web long connection, cookie
//! authentication, bounded relay, and the strong-typed web protocol.
//!
//! Dependency direction: `neo-webui` depends only on `neo-agent-core` and
//! std-ecosystem crates — never on `neo-agent` or `neo-tui`. It never reads
//! JSONL, creates a runtime, executes tools, parses global configuration, or
//! accepts filesystem paths.
//!
//! The frontend `web/dist` has not been delivered, so no static resources
//! are embedded yet: the API and long-connection surface is complete and
//! `GET /` intentionally 404s.

pub mod auth;
pub mod protocol;
pub mod relay;
pub mod server;

pub use auth::{AuthState, CONTENT_SECURITY_POLICY, SESSION_COOKIE_NAME};
pub use protocol::{
    WebUiBootstrap, WebUiCommand, WebUiCursor, WebUiError, WebUiErrorBody, WebUiErrorCode,
    WebUiEventBody, WebUiHistoryEntry, WebUiHost, WebUiInputDelivery, WebUiOutputRef,
    WebUiPendingApproval, WebUiPendingQuestion, WebUiPhase, WebUiReply, WebUiServerMessage,
    WebUiSessionMetadata, WebUiSessionPage, WebUiSessionScope, WebUiSessionStarted,
    WebUiSessionState, WebUiSessionSummary, WebUiSnapshot, WebUiSummaryState, WebUiWatchRequest,
    WebUiWorkspaceSnapshot,
};
pub use relay::{
    COMMAND_BODY_LIMIT_BYTES, CONNECTION_QUEUE_BYTES, CONNECTION_QUEUE_MESSAGES, EventPublisher,
    FIRST_SUBSCRIBE_DEADLINE, GLOBAL_EVENT_CACHE_BYTES, ObserverQueue, OutboundMessage, Relay,
    SESSION_EVENT_CACHE_BYTES, SESSION_PAGE_LIMIT, SubscribeMode, SubscribeOutcome,
    SubscriptionLayer, TOOL_OUTPUT_MAX_LINES, WS_CLOSE_SLOW_CONSUMER, WS_FRAME_LIMIT_BYTES,
};
pub use server::{AppState, RunningWebUi, start};
