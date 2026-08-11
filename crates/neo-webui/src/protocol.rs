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
    AgentEvent, AgentTokenUsage, ApprovalAction, ApprovalOption, ApprovalPresentation,
    PermissionMode, QuestionEventData, TodoEventData,
};
use neo_ai::{ReasoningCapability, ReasoningSelection};
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
    pub reasoning: Option<ReasoningSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub development_mode: Option<WebUiDevelopmentMode>,
}

/// One row of the session list. `workspace_label` is the display label of
/// the workspace bucket the session belongs to (directory base name, with a
/// short hash suffix on collisions); an absolute workspace path never appears
/// anywhere on the wire.
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
    #[serde(default, skip_serializing_if = "is_false")]
    pub unread: bool,
    pub state: WebUiSummaryState,
    pub workspace_label: String,
}

/// Cursor-paginated session page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiSessionPage {
    pub items: Vec<WebUiSessionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Latest known context-window occupancy of a session, cached from the most
/// recent `ContextWindowUpdated` event so a reconnect or workspace switch
/// restores the composer context ring without waiting for new model traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiContextWindow {
    pub used_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_tokens: Option<u32>,
}

/// Transport state of one session (never a forged `AgentEvent`, never
/// written to JSONL). `token_usage` and `context_window` are the latest
/// values observed on the canonical event stream, cached host-side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiSessionState {
    pub phase: WebUiPhase,
    #[serde(default)]
    pub waiting_approval: bool,
    #[serde(default)]
    pub waiting_question: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<AgentTokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<WebUiContextWindow>,
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

fn is_false(value: &bool) -> bool {
    !value
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_questions: Vec<WebUiPendingQuestion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub todos: Vec<TodoEventData>,
}

/// One workspace group of the cross-workspace session aggregation. `label`
/// is the display label (directory base name, short hash suffix on
/// collisions); `current` marks the workspace the service was started in.
/// The workspace path itself never leaves the service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiWorkspaceGroup {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub current: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pinned: bool,
    #[serde(default)]
    pub sessions: Vec<WebUiSessionSummary>,
}

/// Workspace summary view sent on `watch_workspace` when the summary cache
/// cannot resume from the client's cursor. The only shape is the grouped
/// cross-workspace aggregation: sessions are always nested under their
/// workspace group, the current workspace first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiWorkspaceSnapshot {
    pub stream_id: String,
    pub workspace_sequence: u64,
    pub workspaces: Vec<WebUiWorkspaceGroup>,
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
        workspaces: Vec<WebUiWorkspaceGroup>,
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

/// Status of one workspace change row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebUiChangeStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

/// One workspace change row. `path` is always workspace-relative (never
/// absolute); `change_id` is the opaque detail reference, generated only by
/// the service and passed back verbatim by the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiWorkspaceChange {
    pub change_id: String,
    pub path: String,
    pub status: WebUiChangeStatus,
    pub added: u32,
    pub deleted: u32,
}

/// Body of `GET /api/workspace/changes`: the branch label, whether the
/// workspace has changes, and every change row. When the workspace is not a
/// repository (or git fails) the body is the "no status" form: no branch,
/// not dirty, no changes — never error text or paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiWorkspaceChanges {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub dirty: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<WebUiWorkspaceChange>,
}

/// Body of `GET /api/workspace/changes/<change_id>`: a length-bounded
/// unified-diff preview for one change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiWorkspaceChangeDetail {
    pub change_id: String,
    pub path: String,
    pub status: WebUiChangeStatus,
    pub diff: String,
    pub truncated: bool,
}

/// Body of `POST /api/attachments`: one media payload, base64-encoded. The
/// decoded bytes are bounded per attachment and the MIME type is whitelisted
/// (image types only); both limits are enforced before anything is stored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiAttachmentBody {
    pub mime: String,
    pub base64: String,
}

/// Body of `201 Created` attachment uploads: the opaque id (digest of the
/// stored bytes), the accepted MIME type and the decoded byte length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiAttachmentAck {
    pub id: String,
    pub mime: String,
    pub byte_len: u64,
}

/// Body of `GET /api/sessions/<id>/agents/<agent_id>/history`: the child
/// agent's persisted wire history, projected exactly like the main session
/// snapshot (opaque output references, workspace-relative paths). `watermark`
/// is the count of replayed events; entries carry contiguous `sequence`
/// values starting at 1. Read on demand — never cached, never a new event
/// store. A still-running child agent's history only reaches its last flush
/// point (the wire writer flushes at run end; text deltas land at message
/// boundaries), so the tail of a live run is legitimately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiAgentHistory {
    pub agent_id: String,
    pub watermark: u64,
    #[serde(default)]
    pub history: Vec<WebUiHistoryEntry>,
}

/// Strong-typed commands executed against the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebUiCommand {
    Bootstrap,
    CompleteInput {
        query: String,
    },
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
        workspace_id: Option<String>,
        message: String,
        composer: Option<WebUiComposer>,
        attachments: Option<Vec<String>>,
    },
    StartTurn {
        session_id: String,
        message: String,
        composer: Option<WebUiComposer>,
        attachments: Option<Vec<String>>,
    },
    SendInput {
        session_id: String,
        turn_id: String,
        delivery: WebUiInputDelivery,
        message: String,
        attachments: Option<Vec<String>>,
    },
    UploadAttachment {
        mime: String,
        base64: String,
    },
    AgentHistory {
        session_id: String,
        agent_id: String,
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
    WorkspaceChanges,
    WorkspaceChangeDetail {
        change_id: String,
    },
    AddWorkspace {
        path: String,
    },
    RevealWorkspace {
        workspace_id: String,
    },
    UpdateWorkspace {
        workspace_id: String,
        label: Option<String>,
        pinned: Option<bool>,
        removed: Option<bool>,
        mark_read: bool,
        read_session_id: Option<String>,
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

/// Read-only display row of the bootstrap model catalog (model pill overlay).
/// Display fields only: no keys, no base URLs, no provider secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiModelInfo {
    pub alias: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub reasoning: ReasoningCapability,
}

/// One slash-command or workspace-file candidate for the composer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiCompletionItem {
    pub value: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Bounded completion response. Values are commands or workspace-relative
/// references; absolute paths never cross the web boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiCompletions {
    #[serde(default)]
    pub items: Vec<WebUiCompletionItem>,
}

/// Initial page payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiBootstrap {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_label: Option<String>,
    pub default_model: String,
    pub default_reasoning: ReasoningSelection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<WebUiModelInfo>,
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
    Completions(WebUiCompletions),
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
    WorkspaceChanges(WebUiWorkspaceChanges),
    WorkspaceChangeDetail(WebUiWorkspaceChangeDetail),
    WorkspaceAdded(WebUiWorkspaceGroup),
    AttachmentUploaded(WebUiAttachmentAck),
    AgentHistory(WebUiAgentHistory),
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
    #[serde(default)]
    pub workspace_id: Option<String>,
    pub message: String,
    #[serde(default)]
    pub composer: Option<WebUiComposer>,
    #[serde(default)]
    pub attachments: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiAddWorkspaceBody {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiUpdateWorkspaceBody {
    pub label: Option<String>,
    pub pinned: Option<bool>,
    pub removed: Option<bool>,
    #[serde(default)]
    pub mark_read: bool,
    pub read_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiStartTurnBody {
    pub message: String,
    #[serde(default)]
    pub composer: Option<WebUiComposer>,
    #[serde(default)]
    pub attachments: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebUiInputBody {
    pub turn_id: String,
    pub delivery: WebUiInputDelivery,
    pub message: String,
    #[serde(default)]
    pub attachments: Option<Vec<String>>,
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
