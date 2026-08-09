//! Strong-typed web protocol shared by the `neo-webui` server and its host
//! implementation (`WebUiHost`). Wire fields are `snake_case`; session,
//! turn and pending-item identifiers are dedicated typed fields, never free
//! path strings. `AgentEvent` is carried verbatim (no renaming, no downgrade
//! to legacy RPC events) except that structured `ToolOutputRef` values never
//! leave the service: the transport replaces them with opaque
//! [`WebUiOutputRef`] display metadata. `session_state` and
//! `session_metadata_changed` are transport states that share the same
//! per-session, per-service-start `sequence` and are never written to JSONL.

use async_trait::async_trait;
use neo_agent_core::session::ToolOutputRange;
use neo_agent_core::{
    AgentEvent, ApprovalAction, ApprovalOption, ApprovalPresentation, PermissionMode,
    QuestionEventData, TodoEventData,
};
use serde::{Deserialize, Serialize};

/// Body of `POST /api/auth/claim`. The token is sensitive: it must never be
/// logged, echoed, or persisted.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiClaimRequest {
    pub token: String,
}

/// Which session list the client wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebUiSessionScope {
    Active,
    Archived,
}

/// Development modes offered to a composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebUiDevelopmentMode {
    Normal,
    Plan,
    Goal,
}

/// Composable session phase. "Waiting for approval" and "waiting for
/// question" are separate booleans so both can be true at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebUiPhase {
    Starting,
    Running,
    Finishing,
    Idle,
    Cancelled,
    Failed,
}

/// Single-field dynamic state shown in the session list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebUiSummaryState {
    Idle,
    Running,
    WaitingApproval,
    WaitingQuestion,
    Failed,
}

/// How a running turn accepts follow-up text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebUiInputDelivery {
    FollowUp,
    Steer,
}

/// Per-turn overrides scoped to one session; never written back to global
/// configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiComposer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub development_mode: Option<WebUiDevelopmentMode>,
}

/// One row of the session list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiSessionSummary {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
    pub state: WebUiSummaryState,
}

/// Cursor-paginated session page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiSessionPage {
    pub items: Vec<WebUiSessionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Transport state of one session (never a forged `AgentEvent`, never
/// written to JSONL).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiSessionState {
    pub phase: WebUiPhase,
    #[serde(default)]
    pub waiting_approval: bool,
    #[serde(default)]
    pub waiting_question: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn_id: Option<String>,
}

/// Session metadata projection (title, pinned, archived).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiSessionMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

/// A pending approval surfaced to the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiPendingApproval {
    pub request_id: String,
    pub turn_id: String,
    pub presentation: ApprovalPresentation,
    pub options: Vec<ApprovalOption>,
}

/// A pending question surfaced to the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiPendingQuestion {
    pub id: String,
    pub turn_id: String,
    pub questions: Vec<QuestionEventData>,
}

/// One canonical-history entry inside a snapshot, tagged with its relay
/// sequence so the frontend can deduplicate against live envelopes. `output`
/// is the opaque display reference for full tool or terminal output; the
/// structured `ToolOutputRef` never appears on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiHistoryEntry {
    pub sequence: u64,
    pub event: AgentEvent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<WebUiOutputRef>,
}

/// Opaque display metadata for full tool or terminal output. `id` is
/// generated only by the service and passed back verbatim by the browser;
/// the structured `ToolOutputRef` stays inside the canonical record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiOutputRef {
    pub id: String,
    pub byte_len: u64,
    pub line_count: u64,
    pub complete: bool,
}

/// Full session view sent on subscribe and served by `GET .../snapshot`.
/// `watermark` is the sequence up to which `history` is authoritative;
/// retried failed attempts never appear in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiSnapshot {
    pub stream_id: String,
    pub session_id: String,
    pub watermark: u64,
    pub session: WebUiSessionState,
    pub metadata: WebUiSessionMetadata,
    pub history: Vec<WebUiHistoryEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_approval: Option<WebUiPendingApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_question: Option<WebUiPendingQuestion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub todos: Vec<TodoEventData>,
}

/// Workspace summary view sent on `watch_workspace` when the summary cache
/// cannot resume from the client's cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiWorkspaceSnapshot {
    pub stream_id: String,
    pub workspace_sequence: u64,
    pub sessions: Vec<WebUiSessionSummary>,
}

/// One message the service sends on the web long connection. One connection
/// holds two subscriptions over a single bounded send queue: a workspace
/// summary subscription (`workspace_snapshot` + `session_summary_changed`,
/// with their own `workspace_sequence`) and one full-session subscription
/// (`session_snapshot` + `session_event` / `session_state` /
/// `session_metadata_changed`, with the per-session `sequence`). Summary
/// messages never carry an `AgentEvent`; `session_event` carries the
/// canonical `AgentEvent` verbatim plus an optional opaque output reference.
#[allow(clippy::large_enum_variant)] // wire type: the event variant must carry `AgentEvent` by value
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebUiServerMessage {
    WorkspaceSnapshot {
        stream_id: String,
        workspace_sequence: u64,
        sessions: Vec<WebUiSessionSummary>,
    },
    SessionSnapshot {
        snapshot: Box<WebUiSnapshot>,
    },
    SessionSummaryChanged {
        stream_id: String,
        workspace_sequence: u64,
        event: WebUiSessionSummary,
    },
    SessionEvent {
        stream_id: String,
        session_id: String,
        sequence: u64,
        event: AgentEvent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<WebUiOutputRef>,
    },
    SessionState {
        stream_id: String,
        session_id: String,
        sequence: u64,
        event: WebUiSessionState,
    },
    SessionMetadataChanged {
        stream_id: String,
        session_id: String,
        sequence: u64,
        event: WebUiSessionMetadata,
    },
}

/// Payload published into the relay by the host; the relay assigns the
/// `sequence` and builds the wire envelope.
#[allow(clippy::large_enum_variant)] // carries canonical `AgentEvent` by value
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebUiEventBody {
    SessionEvent {
        event: AgentEvent,
        /// Opaque output reference generated by the host before the event
        /// enters the relay (the structured `ToolOutputRef` was stripped).
        output: Option<WebUiOutputRef>,
    },
    SessionState(WebUiSessionState),
    SessionMetadataChanged(WebUiSessionMetadata),
}

/// Cursor used by `watch_session` and `watch_workspace` for
/// snapshot-plus-resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiCursor {
    pub stream_id: String,
    pub sequence: u64,
}

/// The only inbound long-connection messages: (re)subscribe the workspace
/// summary layer or the single full-session layer. Re-watching a session
/// replaces only the full-session subscription; the workspace summary
/// subscription is untouched.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WebUiWatchRequest {
    WatchWorkspace {
        #[serde(default)]
        after: Option<WebUiCursor>,
    },
    WatchSession {
        session_id: String,
        #[serde(default)]
        after: Option<WebUiCursor>,
    },
}

/// Strong-typed commands executed against the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebUiCommand {
    Bootstrap,
    ListSessions {
        scope: WebUiSessionScope,
        query: Option<String>,
        cursor: Option<String>,
        limit: Option<u32>,
    },
    Snapshot {
        session_id: String,
    },
    CreateSession {
        message: String,
        composer: Option<WebUiComposer>,
    },
    StartTurn {
        session_id: String,
        message: String,
        composer: Option<WebUiComposer>,
    },
    SendInput {
        session_id: String,
        turn_id: String,
        delivery: WebUiInputDelivery,
        message: String,
    },
    CancelTurn {
        session_id: String,
        turn_id: String,
    },
    ResolveApproval {
        session_id: String,
        turn_id: String,
        request_id: String,
        action: ApprovalAction,
        feedback: Option<String>,
    },
    ResolveQuestion {
        session_id: String,
        turn_id: String,
        question_id: String,
        answer: WebUiQuestionAnswer,
    },
    UpdateMetadata {
        session_id: String,
        title: Option<String>,
        pinned: Option<bool>,
        archived: Option<bool>,
    },
    ReadToolOutput {
        session_id: String,
        output_ref: String,
        start_line: u64,
        max_lines: u32,
    },
}

/// Answer to a pending question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiQuestionAnswer {
    #[serde(default)]
    pub selections: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Initial page payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiBootstrap {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permission_modes: Vec<PermissionMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub development_modes: Vec<WebUiDevelopmentMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<WebUiSessionSummary>,
}

/// Body of `201 Created` / `202 Accepted` session-start replies: current
/// state plus the initial resume cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebUiSessionStarted {
    pub session_id: String,
    pub turn_id: String,
    pub state: WebUiSessionState,
    pub stream_id: String,
    pub sequence: u64,
}

/// Body of `202 Accepted` input replies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebUiInputAccepted {
    pub turn_id: String,
}

/// Body of `202 Accepted` cancel replies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebUiCancelling {
    pub turn_id: String,
}

/// Typed replies produced by the host for `WebUiCommand`s.
#[allow(clippy::large_enum_variant)] // snapshot payloads are inherently larger
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebUiReply {
    Bootstrap(WebUiBootstrap),
    Sessions(WebUiSessionPage),
    Snapshot(WebUiSnapshot),
    SessionCreated {
        session_id: String,
        turn_id: String,
        state: WebUiSessionState,
    },
    TurnStarted {
        session_id: String,
        turn_id: String,
        state: WebUiSessionState,
    },
    InputAccepted {
        turn_id: String,
    },
    Cancelling {
        turn_id: String,
    },
    Resolved,
    MetadataUpdated(WebUiSessionMetadata),
    ToolOutput(ToolOutputRange),
}

/// Stable short error codes returned by every web API surface. Error
/// responses never echo paths, tokens, cookies, or underlying error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebUiErrorCode {
    InvalidRequest,
    Unauthorized,
    NotFound,
    SessionBusy,
    TurnTransition,
    NoActiveTurn,
    StaleTurn,
    StaleControl,
    TooLarge,
    OutputNotInSession,
    Internal,
}

/// Wire shape of every error response (HTTP and long-connection): a stable
/// status plus one short code, nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiErrorBody {
    pub code: WebUiErrorCode,
}

/// Host-side error carrying only a stable code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebUiError {
    pub code: WebUiErrorCode,
}

impl WebUiError {
    #[must_use]
    pub const fn new(code: WebUiErrorCode) -> Self {
        Self { code }
    }

    /// Stable HTTP status for the code (no body text here).
    #[must_use]
    pub const fn http_status(&self) -> u16 {
        match self.code {
            WebUiErrorCode::InvalidRequest => 400,
            WebUiErrorCode::Unauthorized => 401,
            WebUiErrorCode::NotFound | WebUiErrorCode::OutputNotInSession => 404,
            WebUiErrorCode::SessionBusy
            | WebUiErrorCode::TurnTransition
            | WebUiErrorCode::NoActiveTurn
            | WebUiErrorCode::StaleTurn
            | WebUiErrorCode::StaleControl => 409,
            WebUiErrorCode::TooLarge => 413,
            WebUiErrorCode::Internal => 500,
        }
    }
}

impl From<WebUiErrorCode> for WebUiError {
    fn from(code: WebUiErrorCode) -> Self {
        Self::new(code)
    }
}

/// HTTP request bodies (design section 7). Every field is `snake_case`;
/// unknown fields are rejected instead of silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiCreateSessionBody {
    pub message: String,
    #[serde(default)]
    pub composer: Option<WebUiComposer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiStartTurnBody {
    pub message: String,
    #[serde(default)]
    pub composer: Option<WebUiComposer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiInputBody {
    pub turn_id: String,
    pub delivery: WebUiInputDelivery,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiCancelBody {
    pub turn_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiApprovalBody {
    pub turn_id: String,
    pub request_id: String,
    pub action: ApprovalAction,
    #[serde(default)]
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiQuestionBody {
    pub turn_id: String,
    pub question_id: String,
    pub answer: WebUiQuestionAnswer,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiMetadataBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub archived: Option<bool>,
}

/// The only cross-package boundary: execute typed web commands, and build
/// the snapshot halves of the two-tier (workspace summary + full session)
/// snapshot-plus-resume subscriptions. It never becomes a general service
/// locator.
#[async_trait]
pub trait WebUiHost: Send + Sync {
    /// Execute one strong-typed command.
    async fn execute(&self, command: WebUiCommand) -> Result<WebUiReply, WebUiError>;

    /// Whether the session is known to the host (persisted or loaded).
    /// Unknown sessions never reach the relay: they get `not_found` instead
    /// of an empty replay. Checked before the relay registers an observer.
    async fn session_exists(&self, session_id: &str) -> bool;

    /// Build the workspace summary view (every session's small summary plus
    /// current dynamic state) from session metadata and live state. The
    /// server stamps the `stream_id` and the workspace sequence recorded
    /// atomically at subscribe time.
    async fn workspace_snapshot(&self) -> Result<WebUiWorkspaceSnapshot, WebUiError>;

    /// Build the authoritative snapshot for one session (canonical history
    /// projection plus current transport state). The relay records the
    /// observer and watermark before this call; the server stamps the
    /// `stream_id` and clamps the watermark to the relay's current sequence.
    async fn subscribe(&self, session_id: &str) -> Result<WebUiSnapshot, WebUiError>;
}
