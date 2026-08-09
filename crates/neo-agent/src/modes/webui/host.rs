//! `WebSessionHost`: the only dynamic owner of web turns, approvals,
//! questions, cancellation, input, session state and event relay publishing.
//!
//! It reuses `TurnRequest`, `TurnChannels`, `SteerInputHandle`,
//! `run_prompt_streaming` / `run_prompt_in_session_streaming` and the existing
//! session metadata location; it never duplicates or wraps `AgentRuntime`.
//! Every command is arbitrated under one per-session state lock; cancellation
//! tokens and one-time response senders are invoked outside the lock. Web
//! connections and HTTP requests never own turn lifecycle state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use neo_agent_core::session::{JsonlSessionReader, SessionMetadataStore, validate_session_id};
use neo_agent_core::{AgentEvent, Content, PendingQuestion};
use neo_webui::protocol::{
    WebUiBootstrap, WebUiChangeStatus, WebUiCommand, WebUiComposer, WebUiDevelopmentMode,
    WebUiError, WebUiErrorCode, WebUiHost, WebUiReply, WebUiSessionMetadata, WebUiSessionPage,
    WebUiSessionScope, WebUiSessionSummary, WebUiSnapshot, WebUiSummaryState, WebUiWorkspaceChange,
    WebUiWorkspaceChangeDetail, WebUiWorkspaceChanges, WebUiWorkspaceSnapshot,
};
use neo_webui::relay::{Relay, SESSION_PAGE_LIMIT};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::{AppConfig, workspace_sessions_dir};
use crate::modes::interactive::{TurnChannels, TurnOutcome, TurnRequest};
use crate::modes::run::{run_prompt_in_session_streaming, run_prompt_streaming};
use crate::modes::sessions;

use super::session::{
    ActiveTurnControl, PerSessionContainers, TurnReceivers, WebSessionState, cancel_turn,
    drain_turn_loop, push_turn_input, resolve_approval, resolve_question,
};

/// In-memory launch record for a `CreateSession` whose first legitimate
/// session id has not arrived yet. Addressable only by the ephemeral turn id,
/// never by a session id, so the web cannot reach a session before it exists.
struct StartingTurn {
    cancel_token: CancellationToken,
}

pub(crate) struct WebSessionHost {
    config: AppConfig,
    relay: Arc<Relay>,
    states: Arc<Mutex<HashMap<String, Arc<Mutex<WebSessionState>>>>>,
    starting: Arc<Mutex<HashMap<String, StartingTurn>>>,
    /// Serializes lazy session-state creation (JSONL bootstrap) so two
    /// concurrent subscribers can never bootstrap the same session twice.
    bootstrap_lock: Arc<tokio::sync::Mutex<()>>,
}

impl WebSessionHost {
    #[must_use]
    pub(crate) fn new(config: AppConfig, relay: Arc<Relay>) -> Self {
        Self {
            config,
            relay,
            states: Arc::new(Mutex::new(HashMap::new())),
            starting: Arc::new(Mutex::new(HashMap::new())),
            bootstrap_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn internal() -> WebUiError {
        WebUiError::new(WebUiErrorCode::Internal)
    }

    /// The session bucket directory used by the metadata store (the same
    /// bucket the runtime writes).
    fn metadata_store(&self) -> SessionMetadataStore {
        SessionMetadataStore::new(workspace_sessions_dir(&self.config))
    }

    /// Get the in-memory state for a session, creating it lazily from the
    /// canonical JSONL history (bootstrapped exactly once per service run).
    /// A released (idle) projection is rebuilt in place from the canonical
    /// JSONL history; the synthetic sequence block ends at the retained last
    /// known sequence. Unknown sessions return `not_found` without creating
    /// anything.
    async fn state_for(&self, session_id: &str) -> Result<Arc<Mutex<WebSessionState>>, WebUiError> {
        let is_released = |state: &Arc<Mutex<WebSessionState>>| {
            state
                .lock()
                .map(|guard| guard.projection_released())
                .unwrap_or(false)
        };
        // Fast path: registered state with a live projection.
        if let Some(state) = self
            .states
            .lock()
            .map_err(|_| Self::internal())?
            .get(session_id)
        {
            if !is_released(state) {
                return Ok(Arc::clone(state));
            }
        } else {
            // Not registered: fresh bootstrap (new service run or first access).
            return self.bootstrap_state(session_id).await;
        }
        // Released projection: rebuild it from the canonical JSONL history
        // under the bootstrap lock so concurrent accesses rebuild once.
        let _bootstrap = self.bootstrap_lock.lock().await;
        let state = self
            .states
            .lock()
            .map_err(|_| Self::internal())?
            .get(session_id)
            .cloned()
            .ok_or_else(|| WebUiError::new(WebUiErrorCode::NotFound))?;
        if !is_released(&state) {
            return Ok(state);
        }
        let wire_path = sessions::session_path(session_id, &self.config)
            .map_err(|_| WebUiError::new(WebUiErrorCode::NotFound))?;
        let events = JsonlSessionReader::read_all(&wire_path)
            .await
            .map_err(|_| WebUiError::new(WebUiErrorCode::NotFound))?;
        {
            let mut guard = state.lock().map_err(|_| Self::internal())?;
            guard.rebuild_projection(events);
        }
        Ok(state)
    }

    /// Fresh in-memory state for a session not yet registered: read the
    /// canonical JSONL history and ingest it (publishing under the bootstrap
    /// lock so two concurrent subscribers never bootstrap twice).
    async fn bootstrap_state(
        &self,
        session_id: &str,
    ) -> Result<Arc<Mutex<WebSessionState>>, WebUiError> {
        let _bootstrap = self.bootstrap_lock.lock().await;
        if let Some(state) = self
            .states
            .lock()
            .map_err(|_| Self::internal())?
            .get(session_id)
        {
            return Ok(Arc::clone(state));
        }
        let wire_path = sessions::session_path(session_id, &self.config)
            .map_err(|_| WebUiError::new(WebUiErrorCode::NotFound))?;
        if !wire_path.exists() {
            return Err(WebUiError::new(WebUiErrorCode::NotFound));
        }
        let session_dir = sessions::session_dir(session_id, &self.config)
            .map_err(|_| WebUiError::new(WebUiErrorCode::NotFound))?;
        let events = JsonlSessionReader::read_all(&wire_path)
            .await
            .map_err(|_| WebUiError::new(WebUiErrorCode::NotFound))?;
        let mut state = WebSessionState::new(
            session_id.to_owned(),
            session_dir,
            self.relay.publisher(session_id),
            PerSessionContainers::fresh(&self.config),
            self.config.project_dir.clone(),
            Some(self.summary_sink()),
        );
        for event in events {
            state.ingest_event(event);
        }
        let state = Arc::new(Mutex::new(state));
        let mut states = self.states.lock().map_err(|_| Self::internal())?;
        Ok(Arc::clone(
            states
                .entry(session_id.to_owned())
                .or_insert_with(|| Arc::clone(&state)),
        ))
    }

    /// Resolve a composer model alias into the typed turn selection.
    fn resolve_model(&self, alias: &str) -> Option<crate::modes::interactive::SelectedModel> {
        if let Some(cfg) = self.config.models.get(alias) {
            return Some(crate::modes::interactive::SelectedModel {
                alias: alias.to_owned(),
                provider: cfg.provider.clone(),
                model: cfg.model.clone(),
                max_context_tokens: cfg.max_context_tokens,
            });
        }
        let (provider, model) = alias.split_once('/')?;
        Some(crate::modes::interactive::SelectedModel {
            alias: alias.to_owned(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            max_context_tokens: None,
        })
    }

    /// Build the per-turn `TurnRequest` from the session's mutable containers
    /// and the composer overrides. The overrides apply only to this session's
    /// current turn and are never written back to the global `AppConfig`.
    fn build_request(
        &self,
        containers: &PerSessionContainers,
        session_id: Option<String>,
        prompt: Vec<Content>,
        prompt_display_text: Option<String>,
        composer: Option<&WebUiComposer>,
    ) -> Result<TurnRequest, WebUiError> {
        let model = match composer.and_then(|c| c.model.as_deref()) {
            None => None,
            Some(alias) => Some(
                self.resolve_model(alias)
                    .ok_or_else(|| WebUiError::new(WebUiErrorCode::InvalidRequest))?,
            ),
        };
        let reasoning = match composer.and_then(|c| c.reasoning_effort.as_deref()) {
            None => self.config.runtime.reasoning.clone(),
            Some(effort) => {
                let effort = neo_ai::ReasoningEffort::try_from(effort)
                    .map_err(|_| WebUiError::new(WebUiErrorCode::InvalidRequest))?;
                neo_ai::ReasoningSelection::Effort { effort }
            }
        };
        let permission_mode = composer
            .and_then(|c| c.permission_mode)
            .unwrap_or(self.config.permission_mode);
        if let Some(mode) = composer.and_then(|c| c.permission_mode)
            && let Ok(mut live) = containers.live_permission_mode.write()
        {
            *live = mode;
        }
        let development_mode = composer.and_then(|c| c.development_mode);
        if let Ok(mut plan_mode) = containers.plan_mode.write() {
            match development_mode {
                Some(WebUiDevelopmentMode::Plan) => plan_mode.enter_in_memory(),
                Some(WebUiDevelopmentMode::Normal) | Some(WebUiDevelopmentMode::Goal) => {
                    *plan_mode = neo_agent_core::mode::PlanMode::default();
                }
                None => {}
            }
        }
        let goal_mode_authoring = matches!(development_mode, Some(WebUiDevelopmentMode::Goal));
        let mut request = TurnRequest::new(prompt, session_id, model, reasoning);
        request.prompt_display_text = prompt_display_text;
        request.permission_mode = permission_mode;
        request.live_permission_mode = Arc::clone(&containers.live_permission_mode);
        request.workspace_policy = Arc::clone(&containers.workspace_policy);
        request.plan_mode = Arc::clone(&containers.plan_mode);
        request.goal_mode_authoring = goal_mode_authoring;
        request.mcp_manager.clone_from(&containers.mcp_manager);
        request.base_config = Some(self.config.clone());
        request
            .instruction_registry
            .clone_from(&containers.instruction_registry);
        request.manual_compact_request = Arc::clone(&containers.manual_compact_request);
        request.theme_draft_store = Arc::clone(&containers.theme_draft_store);
        Ok(request)
    }

    /// Shared turn-task wrapper: applies the per-turn effective config (model,
    /// reasoning, permission and workspace overrides from the request) exactly
    /// like the interactive driver, runs the streaming turn, and forwards
    /// errors through the event channel.
    fn spawn_turn_task(
        &self,
        session_id: Option<String>,
        mut request: TurnRequest,
        channels: TurnChannels,
        event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    ) -> JoinHandle<anyhow::Result<TurnOutcome>> {
        let base_config = self.config.clone();
        tokio::spawn(async move {
            let mut effective_config = request
                .base_config
                .take()
                .unwrap_or_else(|| base_config.clone());
            if let Some(model) = request.model.take() {
                effective_config.default_provider = model.provider;
                effective_config.default_model = model.alias;
            }
            effective_config.runtime.reasoning = request.reasoning.clone();
            effective_config.permission_mode = request.permission_mode;
            effective_config.live_permission_mode = Arc::clone(&request.live_permission_mode);
            effective_config.workspace_policy = Arc::clone(&request.workspace_policy);
            let result = match session_id {
                Some(session_id) => {
                    run_prompt_in_session_streaming(
                        &session_id,
                        request,
                        channels,
                        &effective_config,
                    )
                    .await
                }
                None => run_prompt_streaming(request, channels, &effective_config).await,
            };
            if let Err(error) = &result {
                let _ = event_tx.send(Err(anyhow::anyhow!(error.to_string())));
            }
            result.map(|_| TurnOutcome::default())
        })
    }

    /// Build a snapshot of one session from the retained history projection
    /// plus current transport state.
    fn snapshot_locked(
        &self,
        guard: &WebSessionState,
        metadata: WebUiSessionMetadata,
    ) -> WebUiSnapshot {
        WebUiSnapshot {
            stream_id: self.relay.stream_id().to_owned(),
            session_id: guard.session_id.clone(),
            watermark: guard.last_sequence,
            session: guard.state_snapshot(),
            metadata,
            history: guard.history.clone(),
            pending_approval: guard
                .pending_approval
                .as_ref()
                .map(|entry| entry.web.clone()),
            pending_question: guard
                .pending_question
                .as_ref()
                .map(|entry| entry.web.clone()),
            todos: guard.last_todos.clone(),
        }
    }

    fn summary_sink(&self) -> super::session::SessionSummarySink {
        super::session::SessionSummarySink::new(
            (*self.relay).clone(),
            workspace_sessions_dir(&self.config),
        )
    }

    fn session_summary(
        &self,
        record: neo_agent_core::session::SessionRecord,
    ) -> WebUiSessionSummary {
        let state = self
            .states
            .lock()
            .ok()
            .and_then(|states| states.get(&record.id).cloned());
        let state = match state {
            Some(state) => state
                .lock()
                .ok()
                .map(|guard| guard.summary_state())
                .unwrap_or(WebUiSummaryState::Idle),
            None => WebUiSummaryState::Idle,
        };
        WebUiSessionSummary {
            session_id: record.id,
            title: record.name.clone().or(record.title),
            updated_at: record.updated_at.clone(),
            pinned: record.pinned,
            archived: record.archived,
            state,
        }
    }

    fn list_sessions(
        &self,
        scope: WebUiSessionScope,
        query: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<WebUiSessionPage, WebUiError> {
        let records = self.metadata_store().list().map_err(|_| Self::internal())?;
        let mut items: Vec<WebUiSessionSummary> = records
            .into_iter()
            .filter(|record| match scope {
                WebUiSessionScope::Active => !record.archived,
                WebUiSessionScope::Archived => record.archived,
            })
            .filter(|record| {
                query.is_none_or(|query| {
                    record
                        .name
                        .as_deref()
                        .or(record.title.as_deref())
                        .is_some_and(|title| title.to_lowercase().contains(&query.to_lowercase()))
                })
            })
            .map(|record| self.session_summary(record))
            .collect();
        // Pinned first, then updated_at descending, then id ascending.
        items.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        let after = match cursor {
            None => None,
            Some(cursor) => Some(
                decode_list_cursor(cursor)
                    .ok_or_else(|| WebUiError::new(WebUiErrorCode::InvalidRequest))?,
            ),
        };
        let start = after
            .map(|cursor| {
                items
                    .iter()
                    .position(|item| keyset_after(item, &cursor))
                    .unwrap_or(items.len())
            })
            .unwrap_or(0);
        let limit = usize::try_from(limit.unwrap_or(SESSION_PAGE_LIMIT as u32))
            .unwrap_or(SESSION_PAGE_LIMIT)
            .min(SESSION_PAGE_LIMIT);
        let end = start.saturating_add(limit).min(items.len());
        let page = items[start..end].to_vec();
        let next_cursor = if end < items.len() {
            page.last().map(encode_list_cursor)
        } else {
            None
        };
        Ok(WebUiSessionPage {
            items: page,
            next_cursor,
        })
    }

    fn metadata_for(&self, session_id: &str) -> Result<WebUiSessionMetadata, WebUiError> {
        let record = self
            .metadata_store()
            .list()
            .map_err(|_| Self::internal())?
            .into_iter()
            .find(|record| record.id == session_id)
            .ok_or_else(|| WebUiError::new(WebUiErrorCode::NotFound))?;
        Ok(WebUiSessionMetadata {
            title: record.name.clone().or(record.title),
            pinned: record.pinned,
            archived: record.archived,
            updated_at: record.updated_at.clone(),
        })
    }

    /// Snapshot read with a stable projection: `state_for` checks the released
    /// flag before the snapshot lock is taken, so a turn completing in between
    /// could otherwise serve an idle snapshot with an empty (just-released)
    /// history. Re-check under the lock and rebuild via `state_for` when the
    /// release won the race.
    async fn snapshot_for(&self, session_id: &str) -> Result<WebUiSnapshot, WebUiError> {
        loop {
            let state = self.state_for(session_id).await?;
            let metadata = self.metadata_for(session_id)?;
            let guard = state.lock().map_err(|_| Self::internal())?;
            if !guard.projection_released() {
                return Ok(self.snapshot_locked(&guard, metadata));
            }
            drop(guard);
        }
    }

    fn read_tool_output(
        &self,
        output_ref: &str,
        start_line: u64,
        max_lines: u32,
        state: &WebSessionState,
    ) -> Result<WebUiReply, WebUiError> {
        // Path-form input, undecodable strings and cross-session references all
        // resolve to the same 404 without leaking other sessions' existence.
        let range =
            super::session::read_owned_tool_output(state, output_ref, start_line, max_lines)?;
        Ok(WebUiReply::ToolOutput(range))
    }

    /// Structured workspace change summary, read on demand (overlay open).
    /// Uses the shared git collector off the async runtime; any git failure
    /// yields the "no status" body instead of error text or paths.
    async fn workspace_changes(&self) -> Result<WebUiReply, WebUiError> {
        let root = self.config.project_dir.clone();
        let status =
            tokio::task::spawn_blocking(move || crate::git_status::collect_workspace_status(&root))
                .await
                .map_err(|_| Self::internal())?;
        Ok(WebUiReply::WorkspaceChanges(web_workspace_changes(status)))
    }

    /// Bounded unified-diff preview for one opaque change reference. Forged,
    /// absolute, outside, stale or otherwise unresolvable references all get
    /// the same `not_found` (the opaque output-reference rejection style);
    /// no absolute path or error text ever leaves the service.
    async fn workspace_change_detail(&self, change_id: &str) -> Result<WebUiReply, WebUiError> {
        let path = crate::git_status::decode_change_id(change_id)
            .ok_or_else(|| WebUiError::new(WebUiErrorCode::NotFound))?;
        let root = self.config.project_dir.clone();
        let resolved = tokio::task::spawn_blocking(move || {
            // The reference must resolve to a change of the *current*
            // workspace status, never to an arbitrary browser-supplied path.
            let status = crate::git_status::collect_workspace_status(&root)?;
            let change = status
                .changes
                .iter()
                .find(|change| change.path == path)
                .cloned()?;
            let diff = crate::git_status::change_diff_preview(&root, &change)?;
            Some((change, diff))
        })
        .await
        .map_err(|_| Self::internal())?;
        let (change, diff) = resolved.ok_or_else(|| WebUiError::new(WebUiErrorCode::NotFound))?;
        Ok(WebUiReply::WorkspaceChangeDetail(
            WebUiWorkspaceChangeDetail {
                change_id: crate::git_status::encode_change_id(&change.path)
                    .ok_or_else(|| WebUiError::new(WebUiErrorCode::NotFound))?,
                path: change.path.to_string_lossy().into_owned(),
                status: web_change_status(change.kind),
                diff: diff.diff,
                truncated: diff.truncated,
            },
        ))
    }
}

/// Project the collected workspace status into the web wire form. Git
/// failure ("no status") maps to the empty body: no branch, not dirty, no
/// changes. Paths stay workspace-relative; unencodable entries are dropped.
fn web_workspace_changes(
    status: Option<crate::git_status::GitWorkspaceStatus>,
) -> WebUiWorkspaceChanges {
    let Some(status) = status else {
        return WebUiWorkspaceChanges {
            branch: None,
            dirty: false,
            changes: Vec::new(),
        };
    };
    let changes: Vec<WebUiWorkspaceChange> = status
        .changes
        .iter()
        .filter_map(|change| {
            Some(WebUiWorkspaceChange {
                change_id: crate::git_status::encode_change_id(&change.path)?,
                path: change.path.to_string_lossy().into_owned(),
                status: web_change_status(change.kind),
                added: change.added,
                deleted: change.deleted,
            })
        })
        .collect();
    WebUiWorkspaceChanges {
        branch: Some(status.branch),
        dirty: !changes.is_empty(),
        changes,
    }
}

fn web_change_status(kind: crate::git_status::GitChangeKind) -> WebUiChangeStatus {
    match kind {
        crate::git_status::GitChangeKind::Modified => WebUiChangeStatus::Modified,
        crate::git_status::GitChangeKind::Added => WebUiChangeStatus::Added,
        crate::git_status::GitChangeKind::Deleted => WebUiChangeStatus::Deleted,
        crate::git_status::GitChangeKind::Renamed => WebUiChangeStatus::Renamed,
        crate::git_status::GitChangeKind::Untracked => WebUiChangeStatus::Untracked,
    }
}

/// Keyset pagination cursor: `pinned`, `updated_at` and `id` of the last item
/// of the previous page, URL-safe base64 of its JSON form.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ListCursor {
    pinned: bool,
    updated_at: String,
    id: String,
}

fn encode_list_cursor(item: &WebUiSessionSummary) -> String {
    use base64::Engine as _;
    let cursor = ListCursor {
        pinned: item.pinned,
        updated_at: item.updated_at.clone().unwrap_or_default(),
        id: item.session_id.clone(),
    };
    let bytes = serde_json::to_vec(&cursor).expect("list cursor serializes");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn decode_list_cursor(encoded: &str) -> Option<ListCursor> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Whether `item` sorts after the cursor under (pinned desc, updated_at desc,
/// id asc) ordering.
fn keyset_after(item: &WebUiSessionSummary, cursor: &ListCursor) -> bool {
    let updated_at = item.updated_at.clone().unwrap_or_default();
    if item.pinned != cursor.pinned {
        return cursor.pinned && !item.pinned;
    }
    updated_at < cursor.updated_at
        || (updated_at == cursor.updated_at && item.session_id > cursor.id)
}

fn new_turn_id() -> String {
    format!("turn_{}", Uuid::new_v4().simple())
}

#[async_trait::async_trait]
impl WebUiHost for WebSessionHost {
    async fn execute(&self, command: WebUiCommand) -> Result<WebUiReply, WebUiError> {
        match command {
            WebUiCommand::Bootstrap => {
                let mut models: Vec<String> = self.config.models.keys().cloned().collect();
                models.sort();
                models.dedup();
                let sessions = self.list_sessions(WebUiSessionScope::Active, None, None, None)?;
                Ok(WebUiReply::Bootstrap(WebUiBootstrap {
                    workspace_label: self
                        .config
                        .project_dir
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned),
                    models,
                    permission_modes: vec![
                        neo_agent_core::PermissionMode::Ask,
                        neo_agent_core::PermissionMode::Auto,
                        neo_agent_core::PermissionMode::Yolo,
                    ],
                    development_modes: vec![
                        WebUiDevelopmentMode::Normal,
                        WebUiDevelopmentMode::Plan,
                        WebUiDevelopmentMode::Goal,
                    ],
                    sessions: sessions.items,
                }))
            }
            WebUiCommand::ListSessions {
                scope,
                query,
                cursor,
                limit,
            } => {
                let page = self.list_sessions(scope, query.as_deref(), cursor.as_deref(), limit)?;
                Ok(WebUiReply::Sessions(page))
            }
            WebUiCommand::Snapshot { session_id } => {
                Ok(WebUiReply::Snapshot(self.snapshot_for(&session_id).await?))
            }
            WebUiCommand::CreateSession { message, composer } => {
                self.create_session(message, composer).await
            }
            WebUiCommand::StartTurn {
                session_id,
                message,
                composer,
            } => self.start_turn(&session_id, message, composer).await,
            WebUiCommand::SendInput {
                session_id,
                turn_id,
                delivery,
                message,
            } => {
                if message.trim().is_empty() {
                    return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
                }
                let state = self.state_for(&session_id).await?;
                match push_turn_input(&state, &turn_id, delivery, &message) {
                    Ok(true) => Ok(WebUiReply::InputAccepted { turn_id }),
                    Ok(false) => Err(WebUiError::new(WebUiErrorCode::TurnTransition)),
                    Err(error) => Err(error),
                }
            }
            WebUiCommand::CancelTurn {
                session_id,
                turn_id,
            } => {
                let state = self.state_for(&session_id).await?;
                cancel_turn(&state, &turn_id)?;
                Ok(WebUiReply::Cancelling { turn_id })
            }
            WebUiCommand::ResolveApproval {
                session_id,
                turn_id,
                request_id,
                action,
                feedback,
            } => {
                let state = self.state_for(&session_id).await?;
                resolve_approval(&state, &turn_id, &request_id, action, feedback)?;
                Ok(WebUiReply::Resolved)
            }
            WebUiCommand::ResolveQuestion {
                session_id,
                turn_id,
                question_id,
                answer,
            } => {
                let state = self.state_for(&session_id).await?;
                resolve_question(&state, &turn_id, &question_id, answer)?;
                Ok(WebUiReply::Resolved)
            }
            WebUiCommand::UpdateMetadata {
                session_id,
                title,
                pinned,
                archived,
            } => {
                let record = self
                    .metadata_store()
                    .update_metadata(&session_id, title, pinned, archived)
                    .map_err(|_| WebUiError::new(WebUiErrorCode::NotFound))?;
                let metadata = WebUiSessionMetadata {
                    title: record.name.clone().or(record.title),
                    pinned: record.pinned,
                    archived: record.archived,
                    updated_at: record.updated_at.clone(),
                };
                self.relay.publisher(&session_id).publish(
                    neo_webui::protocol::WebUiEventBody::SessionMetadataChanged(metadata.clone()),
                );
                // The workspace summary layer follows every metadata change.
                let dynamic = self
                    .states
                    .lock()
                    .ok()
                    .and_then(|states| states.get(&session_id).cloned())
                    .and_then(|state| state.lock().ok().map(|guard| guard.summary_state()))
                    .unwrap_or(WebUiSummaryState::Idle);
                self.summary_sink().publish(&session_id, dynamic);
                Ok(WebUiReply::MetadataUpdated(metadata))
            }
            WebUiCommand::ReadToolOutput {
                session_id,
                output_ref,
                start_line,
                max_lines,
            } => {
                // Same released-projection race as the snapshot read: the
                // ownership set must be checked against a stable projection.
                loop {
                    let state = self.state_for(&session_id).await?;
                    let guard = state.lock().map_err(|_| Self::internal())?;
                    if !guard.projection_released() {
                        return self.read_tool_output(&output_ref, start_line, max_lines, &guard);
                    }
                    drop(guard);
                }
            }
            WebUiCommand::WorkspaceChanges => self.workspace_changes().await,
            WebUiCommand::WorkspaceChangeDetail { change_id } => {
                self.workspace_change_detail(&change_id).await
            }
        }
    }

    async fn session_exists(&self, session_id: &str) -> bool {
        sessions::session_path(session_id, &self.config)
            .map(|path| path.exists())
            .unwrap_or(false)
    }

    async fn workspace_snapshot(&self) -> Result<WebUiWorkspaceSnapshot, WebUiError> {
        let records = self.metadata_store().list().map_err(|_| Self::internal())?;
        let mut sessions: Vec<WebUiSessionSummary> = records
            .into_iter()
            .map(|record| self.session_summary(record))
            .collect();
        // Same ordering as the session list: pinned first, then updated_at
        // descending, then id ascending.
        sessions.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        Ok(WebUiWorkspaceSnapshot {
            stream_id: self.relay.stream_id().to_owned(),
            workspace_sequence: self.relay.workspace_sequence(),
            sessions,
        })
    }

    async fn subscribe(&self, session_id: &str) -> Result<WebUiSnapshot, WebUiError> {
        self.snapshot_for(session_id).await
    }
}

impl WebSessionHost {
    async fn create_session(
        &self,
        message: String,
        composer: Option<WebUiComposer>,
    ) -> Result<WebUiReply, WebUiError> {
        if message.trim().is_empty() {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let turn_id = new_turn_id();
        let containers = PerSessionContainers::fresh(&self.config);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (approval_tx, approval_rx) = mpsc::unbounded_channel();
        let (session_id_tx, session_id_rx) = mpsc::unbounded_channel();
        let (question_tx, question_rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let cancel_token = CancellationToken::new();
        let steer_input = neo_agent_core::SteerInputHandle::new();
        let channels = TurnChannels {
            events: event_tx.clone(),
            approvals: approval_tx,
            session_ids: session_id_tx,
            cancel_token: cancel_token.clone(),
            questions: question_tx,
            steer_input: steer_input.clone(),
        };
        let request = self.build_request(
            &containers,
            None,
            vec![Content::text(message.clone())],
            Some(message),
            composer.as_ref(),
        )?;
        let task = self.spawn_turn_task(None, request, channels, event_tx.clone());
        let (result_tx, result_rx) = oneshot::channel();
        self.starting.lock().map_err(|_| Self::internal())?.insert(
            turn_id.clone(),
            StartingTurn {
                cancel_token: cancel_token.clone(),
            },
        );
        let states = Arc::clone(&self.states);
        let starting = Arc::clone(&self.starting);
        let relay = Arc::clone(&self.relay);
        let config = self.config.clone();
        tokio::spawn(launch_new_session_turn(
            states,
            starting,
            relay,
            config,
            turn_id.clone(),
            containers,
            TurnReceivers {
                events: event_rx,
                approvals: approval_rx,
                session_ids: session_id_rx,
                questions: question_rx,
                task,
                cancel_token,
                steer_input,
            },
            result_tx,
        ));
        // Wait for the first legitimate session id; the launch task monitors
        // task completion and channel closure so this never waits for the
        // model turn to finish.
        match result_rx.await {
            Ok(result) => result,
            Err(_) => Err(Self::internal()),
        }
    }

    async fn start_turn(
        &self,
        session_id: &str,
        message: String,
        composer: Option<WebUiComposer>,
    ) -> Result<WebUiReply, WebUiError> {
        // New turns, like new sessions and active-turn input, reject blank
        // messages: the canonical `MessageAppended` user event is the only
        // source of the web user bubble, so an empty prompt can never start
        // a turn.
        if message.trim().is_empty() {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let state = self.state_for(session_id).await?;
        let turn_id = new_turn_id();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (approval_tx, approval_rx) = mpsc::unbounded_channel();
        let (session_id_tx, session_id_rx) = mpsc::unbounded_channel();
        let (question_tx, question_rx) = mpsc::unbounded_channel::<PendingQuestion>();
        let cancel_token = CancellationToken::new();
        let steer_input = neo_agent_core::SteerInputHandle::new();
        let channels = TurnChannels {
            events: event_tx.clone(),
            approvals: approval_tx,
            session_ids: session_id_tx,
            cancel_token: cancel_token.clone(),
            questions: question_tx,
            steer_input: steer_input.clone(),
        };
        let (request, started) = {
            let mut guard = state.lock().map_err(|_| Self::internal())?;
            if guard.active.is_some() || guard.turn_id.is_some() {
                let code = if guard.phase == neo_webui::protocol::WebUiPhase::Finishing {
                    WebUiErrorCode::TurnTransition
                } else {
                    WebUiErrorCode::SessionBusy
                };
                return Err(WebUiError::new(code));
            }
            if guard.pending_approval.is_some() || guard.pending_question.is_some() {
                return Err(WebUiError::new(WebUiErrorCode::TurnTransition));
            }
            // Validate the composer (model alias, reasoning effort) before
            // registering the turn: an invalid request must leave the session
            // untouched instead of registering a turn that can never run.
            let request = self.build_request(
                &guard.containers,
                Some(session_id.to_owned()),
                vec![Content::text(message.clone())],
                Some(message),
                composer.as_ref(),
            )?;
            guard.turn_id = Some(turn_id.clone());
            guard.phase = neo_webui::protocol::WebUiPhase::Starting;
            guard.active = Some(ActiveTurnControl {
                cancel_token: cancel_token.clone(),
                steer_input: steer_input.clone(),
            });
            guard.cancel_requested = false;
            guard.turn_error = None;
            guard.emit_state();
            let started = guard.state_snapshot();
            (request, started)
        };
        let task = self.spawn_turn_task(
            Some(session_id.to_owned()),
            request,
            channels,
            event_tx.clone(),
        );
        let loop_state = Arc::clone(&state);
        tokio::spawn(drain_turn_loop(
            loop_state,
            turn_id.clone(),
            TurnReceivers {
                events: event_rx,
                approvals: approval_rx,
                session_ids: session_id_rx,
                questions: question_rx,
                task,
                cancel_token,
                steer_input,
            },
        ));
        Ok(WebUiReply::TurnStarted {
            session_id: session_id.to_owned(),
            turn_id,
            state: started,
        })
    }
}

/// Temp-phase launch: wait only for the first legitimate session id (while
/// monitoring task completion and channel closure), atomically move the launch
/// record into that session's `WebSessionState`, publish `starting`, return
/// the `201` payload, then run the regular drain loop for the whole turn.
#[allow(clippy::too_many_arguments)]
async fn launch_new_session_turn(
    states: Arc<Mutex<HashMap<String, Arc<Mutex<WebSessionState>>>>>,
    starting: Arc<Mutex<HashMap<String, StartingTurn>>>,
    relay: Arc<Relay>,
    config: AppConfig,
    turn_id: String,
    containers: PerSessionContainers,
    mut receivers: TurnReceivers,
    result_tx: oneshot::Sender<Result<WebUiReply, WebUiError>>,
) {
    let session_id = {
        let mut session_id = None;
        let mut done = false;
        while !done && session_id.is_none() {
            tokio::select! {
                biased;
                id = receivers.session_ids.recv() => match id {
                    Some(id) => session_id = Some(id),
                    None => done = true,
                },
                _ = &mut receivers.task => {
                    // Task finished first: drain any queued session ids before
                    // deciding there is no legitimate id.
                    session_id = receivers.session_ids.try_recv().ok();
                    done = session_id.is_none();
                }
            }
        }
        session_id
    };
    let Some(session_id) = session_id else {
        // No legitimate session id before task completion/channel closure:
        // generic failure without a session id, never a fabricated one.
        fail_launch(&starting, &turn_id, &mut receivers, result_tx).await;
        return;
    };
    if validate_session_id(&session_id).is_err() {
        fail_launch(&starting, &turn_id, &mut receivers, result_tx).await;
        return;
    }
    let session_dir = match sessions::session_dir(&session_id, &config) {
        Ok(session_dir) => session_dir,
        Err(_) => {
            fail_launch(&starting, &turn_id, &mut receivers, result_tx).await;
            return;
        }
    };
    // Fresh sessions are NOT bootstrapped from JSONL: the runtime may already
    // have persisted the first events, and those same events arrive on the
    // live channel, so publishing them here would duplicate them. The live
    // channel is the single source for a session born in this service run.
    let state = {
        let mut state = WebSessionState::new(
            session_id.clone(),
            session_dir,
            relay.publisher(&session_id),
            containers,
            config.project_dir.clone(),
            Some(super::session::SessionSummarySink::new(
                (*relay).clone(),
                workspace_sessions_dir(&config),
            )),
        );
        state.turn_id = Some(turn_id.clone());
        state.phase = neo_webui::protocol::WebUiPhase::Starting;
        state.active = Some(ActiveTurnControl {
            cancel_token: receivers.cancel_token.clone(),
            steer_input: receivers.steer_input.clone(),
        });
        let state = Arc::new(Mutex::new(state));
        let occupied = {
            let mut states = states.lock().expect("web session states poisoned");
            if states.contains_key(&session_id) {
                true
            } else {
                states.insert(session_id.clone(), Arc::clone(&state));
                false
            }
        };
        if occupied {
            // A session id that already has an in-memory state must not be
            // replaced; the current turn is failed and cancelled and the
            // existing canonical record is kept.
            fail_launch(&starting, &turn_id, &mut receivers, result_tx).await;
            return;
        }
        state
    };
    {
        let mut guard = state.lock().expect("web session state poisoned");
        guard.emit_state();
        let snapshot = guard.state_snapshot();
        drop(guard);
        let _ = starting
            .lock()
            .expect("web starting map poisoned")
            .remove(&turn_id);
        let _ = result_tx.send(Ok(WebUiReply::SessionCreated {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            state: snapshot,
        }));
    }
    drain_turn_loop(state, turn_id, receivers).await;
}

/// Generic pre-session-id failure: cancel the turn, drop the launch record
/// (using its cancellation token), join the task and return a failure that
/// never carries a session id.
async fn fail_launch(
    starting: &Mutex<HashMap<String, StartingTurn>>,
    turn_id: &str,
    receivers: &mut TurnReceivers,
    result_tx: oneshot::Sender<Result<WebUiReply, WebUiError>>,
) {
    if let Ok(mut records) = starting.lock()
        && let Some(record) = records.remove(turn_id)
    {
        record.cancel_token.cancel();
    }
    receivers.cancel_token.cancel();
    let _ = (&mut receivers.task).await;
    let _ = result_tx.send(Err(WebUiError::new(WebUiErrorCode::Internal)));
}
