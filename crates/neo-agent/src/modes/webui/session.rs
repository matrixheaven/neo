//! Per-session web runtime state: the composable phase machine, pending
//! approval/question entries, the canonical history projection, and the
//! background drain loop that consumes the four turn channels.
//!
//! One `WebSessionState` exists per known session while the service runs. It
//! owns the per-session mutable turn containers (live permission mode,
//! workspace policy, plan mode, manual compaction, theme drafts, instruction
//! registry) so a session's turn overrides never leak into another session or
//! the global `AppConfig`. The drain loop is spawned per turn and owns the
//! channel receivers; HTTP requests and web connections never hold turn
//! lifecycle state.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Result as AnyhowResult;
use neo_agent_core::session::{SessionEventPersistence, SessionMetadataStore, ToolOutputRef};
use neo_agent_core::{
    ActiveTurnInput, AgentEvent, AgentMessage, AgentTokenUsage, ApprovalPresentation,
    ApprovalResponse, McpConnectionManager, PendingQuestion, PermissionMode, QuestionResponse,
    TodoEventData, WorkspaceAccessPolicy,
    instructions::{InstructionRegistry, InstructionRegistryConfig},
    mode::PlanMode,
};
use neo_webui::protocol::{
    WebUiContextWindow, WebUiError, WebUiErrorCode, WebUiEventBody, WebUiHistoryEntry,
    WebUiOutputRef, WebUiPendingApproval, WebUiPendingQuestion, WebUiPhase, WebUiSessionState,
    WebUiSessionSummary, WebUiSummaryState,
};
use neo_webui::relay::{EventPublisher, Relay, TOOL_OUTPUT_MAX_LINES};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::{AppConfig, neo_home};
use crate::modes::interactive::TurnOutcome;
use crate::modes::run::PendingApproval as RunPendingApproval;
use crate::theme_draft::ThemeDraftStore;

/// Maximum events processed per drain wakeup before approvals, questions,
/// session ids and task completion are checked (mirrors the interactive
/// `MAX_TURN_EVENTS_PER_TICK` so a text flood can never starve approvals).
const MAX_EVENTS_PER_WAKEUP: usize = 256;

/// Per-session mutable turn containers, created fresh for every session.
pub(crate) struct PerSessionContainers {
    pub(crate) live_permission_mode: Arc<RwLock<PermissionMode>>,
    pub(crate) workspace_policy: Arc<RwLock<Option<WorkspaceAccessPolicy>>>,
    pub(crate) plan_mode: Arc<RwLock<PlanMode>>,
    pub(crate) manual_compact_request: Arc<Mutex<Option<String>>>,
    pub(crate) theme_draft_store: Arc<Mutex<ThemeDraftStore>>,
    pub(crate) instruction_registry: Option<Arc<InstructionRegistry>>,
    pub(crate) mcp_manager: Option<McpConnectionManager>,
}

impl PerSessionContainers {
    pub(crate) fn fresh(config: &AppConfig) -> Self {
        let instruction_registry = InstructionRegistry::new(InstructionRegistryConfig {
            primary_workspace: config.project_dir.clone(),
            neo_home: neo_home(),
            project_trusted: config.project_trusted,
        })
        .ok()
        .map(Arc::new);
        Self {
            live_permission_mode: Arc::new(RwLock::new(config.permission_mode)),
            workspace_policy: Arc::new(RwLock::new(None)),
            plan_mode: Arc::new(RwLock::new(PlanMode::default())),
            manual_compact_request: Arc::new(Mutex::new(None)),
            theme_draft_store: Arc::new(Mutex::new(ThemeDraftStore::new())),
            instruction_registry,
            mcp_manager: Some(McpConnectionManager::new(
                neo_agent_core::ProcessSupervisor::default(),
            )),
        }
    }
}

/// One registered approval: the wire presentation plus the single-use
/// response channel. Only one resolver can take the sender.
pub(crate) struct PendingApprovalEntry {
    pub(crate) turn_id: String,
    pub(crate) request_id: String,
    pub(crate) response_tx: oneshot::Sender<ApprovalResponse>,
    pub(crate) web: WebUiPendingApproval,
}

/// One registered question batch: the wire presentation plus the single-use
/// response channel.
pub(crate) struct PendingQuestionEntry {
    pub(crate) turn_id: String,
    pub(crate) id: String,
    pub(crate) response_tx: oneshot::Sender<QuestionResponse>,
    pub(crate) web: WebUiPendingQuestion,
}

/// Control handles kept for the active turn so commands can push input and
/// cancel without owning any channel receiver or task handle.
pub(crate) struct ActiveTurnControl {
    pub(crate) cancel_token: CancellationToken,
    pub(crate) steer_input: neo_agent_core::SteerInputHandle,
}

/// Workspace summary sink: publishes the small per-session summary (metadata
/// from the canonical metadata store plus the current dynamic state) into the
/// relay's workspace layer. Summaries never carry an `AgentEvent`.
pub(crate) struct SessionSummarySink {
    relay: Relay,
    sessions_dir: PathBuf,
}

impl SessionSummarySink {
    pub(crate) fn new(relay: Relay, sessions_dir: PathBuf) -> Self {
        Self {
            relay,
            sessions_dir,
        }
    }

    pub(crate) fn publish(
        &self,
        session_id: &str,
        state: WebUiSummaryState,
        workspace_label: &str,
    ) {
        let record = SessionMetadataStore::new(self.sessions_dir.clone())
            .list()
            .ok()
            .and_then(|records| records.into_iter().find(|record| record.id == session_id));
        // A missing record (a brand-new session whose metadata is not written
        // yet) still publishes its dynamic state; later changes republish.
        let summary = WebUiSessionSummary {
            session_id: session_id.to_owned(),
            title: record
                .as_ref()
                .and_then(|record| record.name.clone().or(record.title.clone())),
            updated_at: record.as_ref().and_then(|record| record.updated_at.clone()),
            pinned: record.as_ref().is_some_and(|record| record.pinned),
            archived: record.as_ref().is_some_and(|record| record.archived),
            state,
            workspace_label: workspace_label.to_owned(),
        };
        self.relay.publish_summary(summary);
    }
}

/// All per-session state guarded by one mutex. Lock ordering is fixed:
/// session state first, then the relay. The relay lock is never held while
/// acquiring a session state, so host commands and drain loops cannot
/// deadlock with server-side subscribes.
pub(crate) struct WebSessionState {
    pub(crate) session_id: String,
    pub(crate) session_dir: std::path::PathBuf,
    pub(crate) containers: PerSessionContainers,
    pub(crate) publisher: EventPublisher,
    /// Workspace root used by the display projection: path metadata fields
    /// (`cwd`, approval paths) leave the service workspace-relative or as
    /// `.`; canonical events and JSONL are never touched. For a session that
    /// belongs to another workspace this is the session's own recorded
    /// workspace, matching CLI cross-directory resume semantics.
    workspace: PathBuf,
    /// Display label of the session's workspace group (never a path).
    workspace_label: String,
    /// Workspace summary publisher (absent in unit tests).
    summary_sink: Option<SessionSummarySink>,
    /// Every published display event with its relay sequence: canonical JSONL
    /// history (bootstrapped once per service run) plus committed live events
    /// of completed and in-flight turns, projected for the web (opaque output
    /// references, workspace-relative paths). Sequences are contiguous
    /// because only retry-filtered events are published.
    pub(crate) history: Vec<WebUiHistoryEntry>,
    /// Latest sequence assigned by the relay for this session, including
    /// transport-state envelopes (`session_state`, `session_metadata_changed`).
    pub(crate) last_sequence: u64,
    /// Retry filter with the exact JSONL persistence semantics: failed attempt
    /// deltas are dropped at `RetryScheduled`, deltas are merged and flushed at
    /// assistant `MessageAppended`.
    pub(crate) persistence: SessionEventPersistence,
    pub(crate) turn_id: Option<String>,
    pub(crate) phase: WebUiPhase,
    pub(crate) waiting_approval: bool,
    pub(crate) waiting_question: bool,
    pub(crate) active: Option<ActiveTurnControl>,
    pub(crate) pending_approval: Option<PendingApprovalEntry>,
    pub(crate) pending_question: Option<PendingQuestionEntry>,
    pub(crate) last_todos: Vec<TodoEventData>,
    /// Latest `TokenUsage` seen on the canonical stream (cached like
    /// `last_todos` so snapshots and reconnects restore it immediately).
    last_token_usage: Option<AgentTokenUsage>,
    /// Latest `ContextWindowUpdated` occupancy, same caching discipline.
    last_context_window: Option<WebUiContextWindow>,
    /// URL-safe encoded `ToolOutputRef` values owned by this session, built
    /// from the canonical history and live events; rebuilt from JSONL after a
    /// service restart.
    pub(crate) output_refs: HashSet<String>,
    pub(crate) cancel_requested: bool,
    pub(crate) turn_error: Option<String>,
    /// True while the re-projection (history, output refs, todo projection)
    /// is released after a completed turn. The lightweight transport state
    /// stays; the next access rebuilds the projection from canonical JSONL.
    projection_released: bool,
}

impl WebSessionState {
    #[must_use]
    pub(crate) fn new(
        session_id: String,
        session_dir: std::path::PathBuf,
        publisher: EventPublisher,
        containers: PerSessionContainers,
        workspace: PathBuf,
        workspace_label: String,
        summary_sink: Option<SessionSummarySink>,
    ) -> Self {
        Self {
            session_id,
            session_dir,
            containers,
            publisher,
            workspace,
            workspace_label,
            summary_sink,
            history: Vec::new(),
            last_sequence: 0,
            persistence: SessionEventPersistence::default(),
            turn_id: None,
            phase: WebUiPhase::Idle,
            waiting_approval: false,
            waiting_question: false,
            active: None,
            pending_approval: None,
            pending_question: None,
            last_todos: Vec::new(),
            last_token_usage: None,
            last_context_window: None,
            output_refs: HashSet::new(),
            cancel_requested: false,
            turn_error: None,
            projection_released: false,
        }
    }

    #[must_use]
    pub(crate) fn state_snapshot(&self) -> WebUiSessionState {
        WebUiSessionState {
            phase: self.phase,
            waiting_approval: self.waiting_approval,
            waiting_question: self.waiting_question,
            current_turn_id: self.turn_id.clone(),
            token_usage: self.last_token_usage,
            context_window: self.last_context_window,
        }
    }

    /// Single-field dynamic state shown in the session list.
    #[must_use]
    pub(crate) fn summary_state(&self) -> WebUiSummaryState {
        if self.waiting_approval {
            WebUiSummaryState::WaitingApproval
        } else if self.waiting_question {
            WebUiSummaryState::WaitingQuestion
        } else if matches!(
            self.phase,
            WebUiPhase::Starting | WebUiPhase::Running | WebUiPhase::Finishing
        ) {
            WebUiSummaryState::Running
        } else if self.phase == WebUiPhase::Failed {
            WebUiSummaryState::Failed
        } else {
            WebUiSummaryState::Idle
        }
    }

    /// Publish a `session_state` transport envelope (never an `AgentEvent`,
    /// never JSONL) and record its sequence; the small workspace summary
    /// follows on the same state change (summaries never carry events).
    pub(crate) fn emit_state(&mut self) {
        let sequence = self
            .publisher
            .publish(WebUiEventBody::SessionState(self.state_snapshot()));
        self.last_sequence = sequence;
        if let Some(sink) = &self.summary_sink {
            sink.publish(
                &self.session_id,
                self.summary_state(),
                &self.workspace_label,
            );
        }
    }

    /// Project one canonical event for the web: extract and strip the
    /// structured `ToolOutputRef` into an opaque [`WebUiOutputRef`], and make
    /// path metadata fields workspace-relative (or `.`). The canonical event
    /// and JSONL keep the original values; output text stays verbatim.
    fn project_event_for_web(&self, event: &AgentEvent) -> (AgentEvent, Option<WebUiOutputRef>) {
        let mut event = event.clone();
        let output = match &mut event {
            AgentEvent::ToolExecutionStarted { output_ref, .. }
            | AgentEvent::ToolExecutionFinished { output_ref, .. }
            | AgentEvent::ToolExecutionUpdate { output_ref, .. }
            | AgentEvent::ShellCommandFinished { output_ref, .. }
            | AgentEvent::TerminalSessionStarted { output_ref, .. }
            | AgentEvent::TerminalSessionOutput { output_ref, .. }
            | AgentEvent::TerminalSessionFinished { output_ref, .. } => {
                output_ref.take().as_ref().and_then(web_output_ref)
            }
            _ => None,
        };
        match &mut event {
            AgentEvent::ShellCommandStarted { cwd, .. }
            | AgentEvent::ShellCommandQueued { cwd, .. }
            | AgentEvent::TerminalSessionStarted { cwd, .. } => {
                relativize_path(cwd, &self.workspace);
            }
            AgentEvent::ApprovalRequested { request } => {
                project_presentation(&mut request.presentation, &self.workspace);
            }
            _ => {}
        }
        (event, output)
    }

    /// Ingest one raw runtime event: apply the JSONL-equivalent retry filter,
    /// update the output-reference set and todo projection first, then publish
    /// the valid events into the relay and record them in the history. The
    /// first published event of a `starting` turn moves the phase to `running`.
    pub(crate) fn ingest_event(&mut self, event: AgentEvent) {
        let valid = self.persistence.persisted_events(&event);
        if valid.is_empty() {
            return;
        }
        if self.phase == WebUiPhase::Starting && self.turn_id.is_some() {
            self.phase = WebUiPhase::Running;
            self.emit_state();
        }
        for event in valid {
            collect_output_refs(&event, &mut self.output_refs);
            self.cache_session_metrics(&event);
            let (event, output) = self.project_event_for_web(&event);
            let sequence = self.publisher.publish(WebUiEventBody::SessionEvent {
                event: event.clone(),
                output: output.clone(),
            });
            self.history.push(WebUiHistoryEntry {
                sequence,
                event,
                output,
            });
            self.last_sequence = sequence;
        }
    }

    /// Cache the latest usage/context-window values from one canonical event
    /// (same discipline as the todo projection: the newest event wins and the
    /// value survives projection release so `session_state` always carries
    /// it).
    fn cache_session_metrics(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::TodoUpdated { todos, .. } => self.last_todos.clone_from(todos),
            AgentEvent::TokenUsage { usage, .. } => self.last_token_usage = Some(*usage),
            AgentEvent::ContextWindowUpdated {
                used_tokens,
                projected_tokens,
                max_tokens,
                remaining_tokens,
                ..
            } => {
                self.last_context_window = Some(WebUiContextWindow {
                    used_tokens: *used_tokens,
                    projected_tokens: *projected_tokens,
                    max_tokens: *max_tokens,
                    remaining_tokens: *remaining_tokens,
                });
            }
            _ => {}
        }
    }

    /// Register a pending approval. Closed senders are dropped unexposed;
    /// duplicate request ids are a runtime anomaly that fails and cancels the
    /// turn. Returns `true` when the caller must cancel the turn token.
    pub(crate) fn register_approval(&mut self, pending: RunPendingApproval) -> bool {
        if pending.response_tx.is_closed() {
            return false;
        }
        let Some(turn_id) = self.turn_id.clone() else {
            return false;
        };
        let request_id = pending.request.id.clone();
        if let Some(existing) = &self.pending_approval
            && existing.request_id == request_id
        {
            self.turn_error = Some("duplicate approval request id".to_owned());
            return true;
        }
        let mut presentation = pending.request.presentation;
        project_presentation(&mut presentation, &self.workspace);
        self.pending_approval = Some(PendingApprovalEntry {
            turn_id: turn_id.clone(),
            request_id: request_id.clone(),
            response_tx: pending.response_tx,
            web: WebUiPendingApproval {
                request_id,
                turn_id,
                presentation,
                options: pending.request.options,
            },
        });
        self.waiting_approval = true;
        self.emit_state();
        false
    }

    /// Register a pending question. Mirrors [`Self::register_approval`].
    pub(crate) fn register_question(&mut self, pending: PendingQuestion) -> bool {
        if pending.response_tx.is_closed() {
            return false;
        }
        let Some(turn_id) = self.turn_id.clone() else {
            return false;
        };
        if let Some(existing) = &self.pending_question
            && existing.id == pending.id
        {
            self.turn_error = Some("duplicate question id".to_owned());
            return true;
        }
        self.pending_question = Some(PendingQuestionEntry {
            turn_id: turn_id.clone(),
            id: pending.id.clone(),
            response_tx: pending.response_tx,
            web: WebUiPendingQuestion {
                id: pending.id,
                turn_id,
                questions: pending.questions,
            },
        });
        self.waiting_question = true;
        self.emit_state();
        false
    }

    /// Validate a session-id value arriving from the turn runtime. Only the
    /// session's own id is accepted; anything else is an anomaly that fails
    /// and cancels the turn. Returns `true` when the caller must cancel.
    pub(crate) fn accept_session_id(&mut self, session_id: &str) -> bool {
        if session_id != self.session_id {
            self.turn_error = Some("unexpected session id from turn runtime".to_owned());
            return true;
        }
        false
    }

    /// Take the single-use approval sender when session, turn and request id
    /// all match and the sender is still open. The sender is sent outside the
    /// lock so only one concurrent resolver wins.
    pub(crate) fn take_approval_sender(
        &mut self,
        turn_id: &str,
        request_id: &str,
    ) -> Option<oneshot::Sender<ApprovalResponse>> {
        let matches = self.pending_approval.as_ref().is_some_and(|entry| {
            entry.turn_id == turn_id
                && entry.request_id == request_id
                && !entry.response_tx.is_closed()
        });
        if !matches {
            return None;
        }
        let entry = self
            .pending_approval
            .take()
            .expect("pending approval present after match");
        self.waiting_approval = false;
        self.emit_state();
        Some(entry.response_tx)
    }

    /// Take the single-use question sender; mirrors [`Self::take_approval_sender`].
    pub(crate) fn take_question_sender(
        &mut self,
        turn_id: &str,
        question_id: &str,
    ) -> Option<oneshot::Sender<QuestionResponse>> {
        let matches = self.pending_question.as_ref().is_some_and(|entry| {
            entry.turn_id == turn_id && entry.id == question_id && !entry.response_tx.is_closed()
        });
        if !matches {
            return None;
        }
        let entry = self
            .pending_question
            .take()
            .expect("pending question present after match");
        self.waiting_question = false;
        self.emit_state();
        Some(entry.response_tx)
    }

    /// Whether the re-projection is released (idle session whose history was
    /// dropped; the next access rebuilds it from canonical JSONL).
    #[must_use]
    pub(crate) fn projection_released(&self) -> bool {
        self.projection_released
    }

    /// Release the re-projection after a completed turn: drop the history,
    /// the output-reference set and the todo projection, and reset the retry
    /// filter (the runtime uses a fresh filter per turn, so a released
    /// projection must not leak buffered deltas into the next turn). The
    /// lightweight transport state (phase, last known sequence, pending
    /// entries) is kept; the next access rebuilds the projection from the
    /// canonical JSONL history. The host publishes exactly the events the
    /// runtime persists to JSONL (message-block level, flushed before the
    /// turn task completes), so nothing published is lost by the release.
    pub(crate) fn release_projection(&mut self) {
        self.history.clear();
        self.output_refs.clear();
        self.last_todos.clear();
        self.persistence = SessionEventPersistence::default();
        self.projection_released = true;
    }

    /// Rebuild a released projection from the canonical JSONL event stream.
    /// Sequences are synthetic: a contiguous block ending at the retained
    /// last known sequence, so the snapshot stays consistent with the relay
    /// sequence space (the server clamps the watermark to the relay's
    /// current sequence). The events are already in the relay cache, so
    /// they are never re-published.
    pub(crate) fn rebuild_projection(&mut self, events: Vec<AgentEvent>) {
        let count = u64::try_from(events.len()).unwrap_or(0);
        let start = self.last_sequence.saturating_sub(count).saturating_add(1);
        self.history.clear();
        self.output_refs.clear();
        self.last_todos.clear();
        for (offset, event) in events.into_iter().enumerate() {
            collect_output_refs(&event, &mut self.output_refs);
            self.cache_session_metrics(&event);
            let (event, output) = self.project_event_for_web(&event);
            self.history.push(WebUiHistoryEntry {
                sequence: start + offset as u64,
                event,
                output,
            });
        }
        self.projection_released = false;
    }

    /// Project a child agent's persisted wire events with the exact same web
    /// projection as the main session (opaque output references,
    /// workspace-relative paths). Sequences are synthetic and contiguous from
    /// 1; the child wire is read on demand and never enters this session's
    /// history, the relay cache or any event store.
    pub(crate) fn project_agent_history(&self, events: Vec<AgentEvent>) -> Vec<WebUiHistoryEntry> {
        events
            .into_iter()
            .enumerate()
            .map(|(offset, event)| {
                let (event, output) = self.project_event_for_web(&event);
                WebUiHistoryEntry {
                    sequence: offset as u64 + 1,
                    event,
                    output,
                }
            })
            .collect()
    }
}

/// Extract `ToolOutputRef` values carried by an event into the session's
/// ownership set (encoded form). Reused by the live path and by bootstrap so
/// the set is always rebuildable from the canonical history.
fn collect_output_refs(event: &AgentEvent, refs: &mut HashSet<String>) {
    let output_ref = match event {
        AgentEvent::ToolExecutionStarted { output_ref, .. }
        | AgentEvent::ToolExecutionFinished { output_ref, .. }
        | AgentEvent::ToolExecutionUpdate { output_ref, .. }
        | AgentEvent::ShellCommandFinished { output_ref, .. }
        | AgentEvent::TerminalSessionStarted { output_ref, .. }
        | AgentEvent::TerminalSessionOutput { output_ref, .. }
        | AgentEvent::TerminalSessionFinished { output_ref, .. } => output_ref.as_ref(),
        _ => return,
    };
    if let Some(output_ref) = output_ref
        && let Some(encoded) = encode_output_ref(output_ref)
    {
        refs.insert(encoded);
    }
}

/// Stable wire encoding for an opaque output reference:
/// URL-safe base64 (no padding) of the JSON form of `ToolOutputRef`.
pub(crate) fn encode_output_ref(output_ref: &ToolOutputRef) -> Option<String> {
    use base64::Engine as _;
    let bytes = serde_json::to_vec(output_ref).ok()?;
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Opaque display metadata for one structured `ToolOutputRef`: the `id` is
/// the service-generated encoding; the browser only passes it back verbatim.
fn web_output_ref(output_ref: &ToolOutputRef) -> Option<WebUiOutputRef> {
    Some(WebUiOutputRef {
        id: encode_output_ref(output_ref)?,
        byte_len: output_ref.byte_len,
        line_count: output_ref.line_count,
        complete: output_ref.complete,
    })
}

/// Display projection for one path metadata field: absolute paths become
/// workspace-relative (`<workspace>` itself or anything outside it becomes
/// `.`), so no absolute path ever reaches the browser. Already-relative
/// paths and output text are kept verbatim.
fn relativize_path(path: &mut PathBuf, workspace: &Path) {
    if !path.is_absolute() {
        return;
    }
    *path = path
        .strip_prefix(workspace)
        .map_or_else(|_| PathBuf::from("."), Path::to_path_buf);
}

/// Display projection for approval presentations: path fields are made
/// workspace-relative in place; everything else stays verbatim.
fn project_presentation(presentation: &mut ApprovalPresentation, workspace: &Path) {
    match presentation {
        ApprovalPresentation::Command { cwd: Some(cwd), .. } => {
            relativize_path(cwd, workspace);
        }
        ApprovalPresentation::Plan {
            path: Some(path), ..
        } => {
            relativize_path(path, workspace);
        }
        ApprovalPresentation::Edit { edit, .. } => {
            for change in &mut edit.changes {
                relativize_path(&mut change.path, workspace);
            }
        }
        ApprovalPresentation::Write { write, .. } => {
            for change in &mut write.changes {
                relativize_path(&mut change.path, workspace);
            }
        }
        _ => {}
    }
}

/// Read one owned tool-output range: ownership is verified before the
/// existing `ToolOutputStore::read_range` runs. The ownership set is the refs
/// collected from the session's own projection (canonical history and live
/// events) union any well-formed child agent record persisted under this
/// session's own `agents/` directory — the panel lazy-load projection mints
/// child output references without touching the collected set, and the data
/// physically lives in `agents/<agent_id>/`. Path-form input, undecodable
/// strings, forged, cross-session and stale references all resolve to the
/// same `404 output_not_in_session` without leaking other sessions.
pub(crate) fn read_owned_tool_output(
    state: &WebSessionState,
    output_ref: &str,
    start_line: u64,
    max_lines: u32,
) -> Result<neo_agent_core::session::ToolOutputRange, WebUiError> {
    if !(1..=TOOL_OUTPUT_MAX_LINES).contains(&max_lines) {
        return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
    }
    if start_line.checked_add(u64::from(max_lines)).is_none() {
        return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
    }
    let Some(reference) = decode_output_ref(output_ref) else {
        return Err(WebUiError::new(WebUiErrorCode::OutputNotInSession));
    };
    let owned = state.output_refs.contains(output_ref)
        || persisted_child_agent(&state.session_dir, &reference.agent_id);
    if !owned {
        return Err(WebUiError::new(WebUiErrorCode::OutputNotInSession));
    }
    let store = neo_agent_core::session::ToolOutputStore::new(state.session_dir.clone());
    store
        .read_range(
            &reference.agent_id,
            &reference.task_id,
            start_line,
            u64::from(max_lines),
        )
        .map_err(|error| match error.kind() {
            // An owned ref whose artifact is gone (stale, or a forged id pair
            // under a real child agent) is indistinguishable from a foreign
            // one: keep the uniform 404.
            std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => {
                WebUiError::new(WebUiErrorCode::OutputNotInSession)
            }
            _ => WebUiError::new(WebUiErrorCode::Internal),
        })
}

/// Whether `agent_id` names a persisted agent record inside this session's
/// own `agents/` directory. The charset rule matches the agent-history route
/// (alphanumeric plus `-_.`, no `..`), so a forged id never reaches the path
/// join and the lookup can never escape `session_dir`.
fn persisted_child_agent(session_dir: &Path, agent_id: &str) -> bool {
    let well_formed = !agent_id.is_empty()
        && agent_id
            .chars()
            .all(|c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !agent_id.contains("..");
    well_formed && neo_agent_core::session::agent_record_dir(session_dir, agent_id).is_dir()
}

/// Decode the wire form back into a typed `ToolOutputRef`. Anything that is
/// not the exact encoded shape (paths, free strings, partial JSON relying on
/// serde defaults) fails.
pub(crate) fn decode_output_ref(encoded: &str) -> Option<ToolOutputRef> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?;
    let reference: ToolOutputRef = serde_json::from_slice(&bytes).ok()?;
    // The canonical encoding must round-trip byte-for-byte; otherwise the
    // input was not the exact typed shape and must not authorize a read.
    if encode_output_ref(&reference)? != encoded {
        return None;
    }
    Some(reference)
}

/// Receivers and task of one running turn, owned by the drain loop task.
pub(crate) struct TurnReceivers {
    pub(crate) events: mpsc::UnboundedReceiver<AnyhowResult<AgentEvent>>,
    pub(crate) approvals: mpsc::UnboundedReceiver<RunPendingApproval>,
    pub(crate) session_ids: mpsc::UnboundedReceiver<String>,
    pub(crate) questions: mpsc::UnboundedReceiver<PendingQuestion>,
    pub(crate) task: JoinHandle<AnyhowResult<TurnOutcome>>,
    pub(crate) cancel_token: CancellationToken,
    pub(crate) steer_input: neo_agent_core::SteerInputHandle,
}

/// Background drain loop: continuously drains the four turn channels until the
/// task completes, then keeps draining until the channels close and the task
/// is joined before clearing the turn and publishing the terminal phase.
/// Every wakeup processes at most [`MAX_EVENTS_PER_WAKEUP`] events and then
/// checks approvals, questions, session ids and task completion, so a text
/// flood can never starve approvals. A closed channel only means that channel
/// is exhausted; late values on other channels are still processed.
pub(crate) async fn drain_turn_loop(
    state: Arc<Mutex<WebSessionState>>,
    turn_id: String,
    mut receivers: TurnReceivers,
) {
    let mut events_open = true;
    let mut approvals_open = true;
    let mut questions_open = true;
    let mut session_ids_open = true;
    let mut task_done = false;
    let mut pending_event: Option<AnyhowResult<AgentEvent>> = None;

    while !task_done {
        tokio::select! {
            biased;
            received = receivers.events.recv(), if events_open => {
                if received.is_none() {
                    events_open = false;
                }
                pending_event = received;
            }
            received = receivers.approvals.recv(), if approvals_open => {
                match received {
                    Some(approval) => {
                        if register_approval_locked(&state, &turn_id, approval) {
                            receivers.cancel_token.cancel();
                        }
                    }
                    None => approvals_open = false,
                }
            }
            received = receivers.questions.recv(), if questions_open => {
                match received {
                    Some(question) => {
                        if register_question_locked(&state, &turn_id, question) {
                            receivers.cancel_token.cancel();
                        }
                    }
                    None => questions_open = false,
                }
            }
            received = receivers.session_ids.recv(), if session_ids_open => {
                match received {
                    Some(id) => {
                        if accept_session_id_locked(&state, &turn_id, &id) {
                            receivers.cancel_token.cancel();
                        }
                    }
                    None => session_ids_open = false,
                }
            }
            _ = &mut receivers.task => {
                task_done = true;
            }
        }
        // Process at most MAX_EVENTS_PER_WAKEUP events per wakeup, then check
        // approvals, questions, session ids and task completion so a text
        // flood can never starve approvals.
        let mut processed = 0usize;
        if let Some(event) = pending_event.take() {
            match event {
                Ok(event) => ingest_locked(&state, &turn_id, event),
                Err(error) => record_error_locked(&state, &turn_id, error),
            }
            processed = 1;
        }
        while processed < MAX_EVENTS_PER_WAKEUP {
            match receivers.events.try_recv() {
                Ok(Ok(event)) => {
                    ingest_locked(&state, &turn_id, event);
                    processed += 1;
                }
                Ok(Err(error)) => {
                    record_error_locked(&state, &turn_id, error);
                    processed += 1;
                }
                Err(mpsc::error::TryRecvError::Empty)
                | Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        while let Ok(approval) = receivers.approvals.try_recv() {
            if register_approval_locked(&state, &turn_id, approval) {
                receivers.cancel_token.cancel();
            }
        }
        while let Ok(question) = receivers.questions.try_recv() {
            if register_question_locked(&state, &turn_id, question) {
                receivers.cancel_token.cancel();
            }
        }
        while let Ok(id) = receivers.session_ids.try_recv() {
            if accept_session_id_locked(&state, &turn_id, &id) {
                receivers.cancel_token.cancel();
            }
        }
        if receivers.task.is_finished() {
            task_done = true;
        }
    }

    finalize_turn(state, turn_id, receivers).await;
}

fn ingest_locked(state: &Mutex<WebSessionState>, turn_id: &str, event: AgentEvent) {
    let Ok(mut guard) = state.lock() else {
        return;
    };
    if guard.turn_id.as_deref() != Some(turn_id) {
        return;
    }
    guard.ingest_event(event);
}

fn record_error_locked(state: &Mutex<WebSessionState>, turn_id: &str, error: anyhow::Error) {
    let Ok(mut guard) = state.lock() else {
        return;
    };
    if guard.turn_id.as_deref() != Some(turn_id) {
        return;
    }
    guard.turn_error = Some(error.to_string());
}

fn register_approval_locked(
    state: &Mutex<WebSessionState>,
    turn_id: &str,
    approval: RunPendingApproval,
) -> bool {
    let Ok(mut guard) = state.lock() else {
        return false;
    };
    if guard.turn_id.as_deref() != Some(turn_id) {
        return false;
    }
    guard.register_approval(approval)
}

fn register_question_locked(
    state: &Mutex<WebSessionState>,
    turn_id: &str,
    question: PendingQuestion,
) -> bool {
    let Ok(mut guard) = state.lock() else {
        return false;
    };
    if guard.turn_id.as_deref() != Some(turn_id) {
        return false;
    }
    guard.register_question(question)
}

fn accept_session_id_locked(state: &Mutex<WebSessionState>, turn_id: &str, received: &str) -> bool {
    let Ok(mut guard) = state.lock() else {
        return false;
    };
    if guard.turn_id.as_deref() != Some(turn_id) {
        return false;
    }
    guard.accept_session_id(received)
}

/// Turn-completion path: enter `finishing`, keep draining every channel until
/// it closes, join the task, then clear the turn and publish the terminal
/// phase (`idle`, `cancelled` or `failed`). Late channel values are processed
/// (never dropped); late approvals/questions find their senders closed and are
/// discarded without being surfaced.
async fn finalize_turn(
    state: Arc<Mutex<WebSessionState>>,
    turn_id: String,
    mut receivers: TurnReceivers,
) {
    {
        let Ok(mut guard) = state.lock() else {
            return;
        };
        if guard.turn_id.as_deref() != Some(turn_id.as_str()) {
            return;
        }
        if guard.phase != WebUiPhase::Finishing {
            guard.phase = WebUiPhase::Finishing;
            guard.emit_state();
        }
    }
    loop {
        match receivers.events.try_recv() {
            Ok(Ok(event)) => ingest_locked(&state, &turn_id, event),
            Ok(Err(error)) => record_error_locked(&state, &turn_id, error),
            Err(mpsc::error::TryRecvError::Empty) => {
                tokio::task::yield_now().await;
                if receivers.events.try_recv().is_err() {
                    break;
                }
            }
            Err(mpsc::error::TryRecvError::Disconnected) => break,
        }
    }
    while let Ok(approval) = receivers.approvals.try_recv() {
        // The task has ended, so its receivers are closed; values drained here
        // are never surfaced and never answered.
        drop(approval);
    }
    while let Ok(question) = receivers.questions.try_recv() {
        drop(question);
    }
    while let Ok(_id) = receivers.session_ids.try_recv() {
        // Session routing values after task completion carry no transcript
        // content; they are consumed so the channel fully drains.
    }
    let outcome = match receivers.task.await {
        Ok(Ok(_outcome)) => Ok(()),
        Ok(Err(_error)) => Err(()),
        Err(_join) => Err(()),
    };
    let Ok(mut guard) = state.lock() else {
        return;
    };
    if guard.turn_id.as_deref() != Some(turn_id.as_str()) {
        return;
    }
    let final_phase = if guard.cancel_requested {
        WebUiPhase::Cancelled
    } else if guard.turn_error.is_some() || outcome.is_err() {
        WebUiPhase::Failed
    } else {
        WebUiPhase::Idle
    };
    guard.phase = final_phase;
    guard.turn_id = None;
    guard.active = None;
    guard.cancel_requested = false;
    guard.turn_error = None;
    // Final cleanup drops unresolved one-time senders; the runtime follows its
    // own cancellation path — cancellation is never forged into an answer.
    guard.pending_approval = None;
    guard.pending_question = None;
    guard.waiting_approval = false;
    guard.waiting_question = false;
    guard.emit_state();
    // The turn is fully over (task joined, channels drained, no pending
    // items): release the re-projection so an idle session's memory stays
    // bounded. The next access rebuilds it from the canonical JSONL history.
    guard.release_projection();
}

/// Push a follow-up or steer message into the active turn's input handle.
/// `media` carries already-staged attachment parts (blob references into the
/// session's own blob store); they ride the same user message as the text.
/// Returns `Ok(true)` when queued, `Ok(false)` when the handle was closed
/// (turn ending), and `Err(code)` for stale or absent turns.
pub(crate) fn push_turn_input(
    state: &Mutex<WebSessionState>,
    turn_id: &str,
    delivery: neo_webui::protocol::WebUiInputDelivery,
    message: &str,
    media: Vec<neo_agent_core::Content>,
) -> Result<bool, WebUiError> {
    let steer_input = {
        let guard = state
            .lock()
            .map_err(|_| WebUiError::new(WebUiErrorCode::Internal))?;
        let Some(current) = guard.turn_id.as_deref() else {
            return Err(WebUiError::new(WebUiErrorCode::NoActiveTurn));
        };
        if current != turn_id {
            return Err(WebUiError::new(WebUiErrorCode::StaleTurn));
        }
        if !matches!(guard.phase, WebUiPhase::Starting | WebUiPhase::Running) {
            return Err(WebUiError::new(WebUiErrorCode::TurnTransition));
        }
        guard
            .active
            .as_ref()
            .map(|active| active.steer_input.clone())
            .ok_or_else(|| WebUiError::new(WebUiErrorCode::TurnTransition))?
    };
    let mut content = Vec::with_capacity(media.len() + 1);
    content.push(neo_agent_core::Content::text(message));
    content.extend(media);
    let message = AgentMessage::user_content(content);
    let input = match delivery {
        neo_webui::protocol::WebUiInputDelivery::FollowUp => ActiveTurnInput::FollowUp(message),
        neo_webui::protocol::WebUiInputDelivery::Steer => ActiveTurnInput::SteerNow(message),
    };
    if steer_input.try_push(input) {
        return Ok(true);
    }
    // Input-handle close race: re-check under the lock; an ending turn keeps
    // the frontend draft (409 turn_transition) instead of dropping the input.
    let guard = state
        .lock()
        .map_err(|_| WebUiError::new(WebUiErrorCode::Internal))?;
    if guard.turn_id.as_deref() == Some(turn_id) {
        Err(WebUiError::new(WebUiErrorCode::TurnTransition))
    } else if guard.turn_id.is_none() {
        Err(WebUiError::new(WebUiErrorCode::NoActiveTurn))
    } else {
        Err(WebUiError::new(WebUiErrorCode::StaleTurn))
    }
}

/// Cancel the current turn: mark `finishing` and drop pending entries under
/// the lock, then cancel the token outside it. Repeats on the same turn id
/// return `Ok(())`; stale turn ids are rejected.
pub(crate) fn cancel_turn(state: &Mutex<WebSessionState>, turn_id: &str) -> Result<(), WebUiError> {
    let cancel_token = {
        let mut guard = state
            .lock()
            .map_err(|_| WebUiError::new(WebUiErrorCode::Internal))?;
        let Some(current) = guard.turn_id.as_deref() else {
            return Err(WebUiError::new(WebUiErrorCode::StaleTurn));
        };
        if current != turn_id {
            return Err(WebUiError::new(WebUiErrorCode::StaleTurn));
        }
        guard.cancel_requested = true;
        if guard.phase != WebUiPhase::Finishing {
            guard.phase = WebUiPhase::Finishing;
            guard.pending_approval = None;
            guard.pending_question = None;
            guard.waiting_approval = false;
            guard.waiting_question = false;
            guard.emit_state();
        }
        guard
            .active
            .as_ref()
            .map(|active| active.cancel_token.clone())
            .ok_or_else(|| WebUiError::new(WebUiErrorCode::TurnTransition))?
    };
    cancel_token.cancel();
    Ok(())
}

/// Resolve a pending approval with the triple match (session, turn, request).
/// The sender is taken under the lock and sent outside it; only one
/// concurrent resolver can win, all others get `stale_control`.
pub(crate) fn resolve_approval(
    state: &Mutex<WebSessionState>,
    turn_id: &str,
    request_id: &str,
    action: neo_agent_core::ApprovalAction,
    feedback: Option<String>,
) -> Result<(), WebUiError> {
    let sender = {
        let mut guard = state
            .lock()
            .map_err(|_| WebUiError::new(WebUiErrorCode::Internal))?;
        if guard.turn_id.as_deref() != Some(turn_id) {
            return Err(WebUiError::new(WebUiErrorCode::StaleControl));
        }
        guard
            .take_approval_sender(turn_id, request_id)
            .ok_or_else(|| WebUiError::new(WebUiErrorCode::StaleControl))?
    };
    let response = ApprovalResponse::Selected {
        request_id: request_id.to_owned(),
        action,
        feedback,
    };
    if sender.send(response).is_err() {
        return Err(WebUiError::new(WebUiErrorCode::StaleControl));
    }
    Ok(())
}

/// Resolve a pending question; mirrors [`resolve_approval`]. The answer is
/// validated before the single-use sender is taken: an empty answer returns
/// `invalid_request` and leaves the pending question untouched, so the
/// frontend can retry with a legal answer.
pub(crate) fn resolve_question(
    state: &Mutex<WebSessionState>,
    turn_id: &str,
    question_id: &str,
    answer: neo_webui::protocol::WebUiQuestionAnswer,
) -> Result<(), WebUiError> {
    let mut answers = answer.selections;
    if answers.is_empty()
        && let Some(text) = answer.text
        && !text.trim().is_empty()
    {
        answers.push(text);
    }
    if answers.is_empty() {
        return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
    }
    let sender = {
        let mut guard = state
            .lock()
            .map_err(|_| WebUiError::new(WebUiErrorCode::Internal))?;
        if guard.turn_id.as_deref() != Some(turn_id) {
            return Err(WebUiError::new(WebUiErrorCode::StaleControl));
        }
        guard
            .take_question_sender(turn_id, question_id)
            .ok_or_else(|| WebUiError::new(WebUiErrorCode::StaleControl))?
    };
    if sender.send(QuestionResponse { answers }).is_err() {
        return Err(WebUiError::new(WebUiErrorCode::StaleControl));
    }
    Ok(())
}

#[cfg(test)]
#[path = "test_cases/drain_loop.rs"]
mod drain_loop;
#[cfg(test)]
#[path = "test_cases/output_ref.rs"]
mod output_ref;
#[cfg(test)]
#[path = "test_cases/pending_items.rs"]
mod pending_items;
#[cfg(test)]
#[path = "test_cases/projection.rs"]
mod projection;
#[cfg(test)]
#[path = "test_cases/state_fixtures.rs"]
mod state_fixtures;
#[cfg(test)]
#[path = "test_cases/turn_controls.rs"]
mod turn_controls;
