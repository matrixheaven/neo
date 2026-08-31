//! `WebSessionHost`: the only dynamic owner of web turns, approvals,
//! questions, cancellation, input, session state and event relay publishing.
//!
//! It reuses `TurnRequest`, `TurnChannels`, `SteerInputHandle`,
//! `run_prompt_streaming` / `run_prompt_in_session_streaming` and the existing
//! session metadata location; it never duplicates or wraps `AgentRuntime`.
//! Every command is arbitrated under one per-session state lock; cancellation
//! tokens and one-time response senders are invoked outside the lock. Web
//! connections and HTTP requests never own turn lifecycle state.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use neo_agent_core::session::{
    JsonlSessionReader, SessionIndex, SessionMetadataStore, SessionRecord, agent_wire_path,
    validate_session_id,
};
use neo_agent_core::{AgentEvent, Content, MediaRef, PendingQuestion};
use neo_webui::protocol::{
    WebUiAgentHistory, WebUiAppearance, WebUiAttachmentAck, WebUiBootstrap, WebUiBranchList,
    WebUiChangeStatus, WebUiCommand, WebUiCompletionItem, WebUiCompletions, WebUiComposer,
    WebUiDevelopmentMode, WebUiError, WebUiErrorCode, WebUiFsEntry, WebUiFsListing, WebUiHost,
    WebUiMcpServerEdit, WebUiMcpServerInfo, WebUiModelEdit, WebUiModelInfo, WebUiProviderEdit,
    WebUiProviderInfo, WebUiReply, WebUiSessionMetadata, WebUiSessionPage, WebUiSessionScope,
    WebUiSessionSummary, WebUiSettingsSnapshot, WebUiSkillInfo, WebUiSnapshot, WebUiSummaryState,
    WebUiWorkspaceChange, WebUiWorkspaceChangeDetail, WebUiWorkspaceChanges, WebUiWorkspaceGroup,
    WebUiWorkspaceSnapshot,
};
use neo_webui::relay::{
    ATTACHMENT_MAX_BYTES, ATTACHMENTS_PER_MESSAGE_MAX, Relay, SESSION_PAGE_LIMIT,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::{
    AppConfig, ConfigOverrides, WebUiAppearance as ConfigWebUiAppearance, neo_home,
    workspace_sessions_dir,
};
use crate::modes::interactive::{TurnChannels, TurnOutcome, TurnRequest};
use crate::modes::run::{run_prompt_in_session_streaming, run_prompt_streaming};
use crate::modes::sessions;
use crate::resources;

use super::session::{
    ActiveTurnControl, PerSessionContainers, TurnReceivers, WebSessionState, cancel_turn,
    drain_turn_loop, push_queue_control, push_turn_input, resolve_approval, resolve_question,
};

/// In-memory launch record for a `CreateSession` whose first legitimate
/// session id has not arrived yet. Addressable only by the ephemeral turn id,
/// never by a session id, so the web cannot reach a session before it exists.
struct StartingTurn {
    cancel_token: CancellationToken,
}

/// One staged attachment upload. The bytes live digest-addressed under
/// `<neo_home>/attachments/<digest>.bin`; the in-memory map is the
/// authorization set (unknown ids are rejected with `invalid_request`) and
/// starts empty every service run.
#[derive(Debug, Clone)]
struct StagedAttachment {
    mime: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct WebUiProjectPreference {
    display_name: Option<String>,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    removed: bool,
    #[serde(default)]
    read_at_by_session: HashMap<String, String>,
}

type WebUiProjectPreferences = HashMap<String, WebUiProjectPreference>;

/// Hard cap on directory entries returned per folder-picker listing. Large
/// directories truncate; navigation by path or search stays available.
const FS_LIST_MAX_ENTRIES: usize = 500;

pub(crate) struct WebSessionHost {
    config: AppConfig,
    relay: Arc<Relay>,
    states: Arc<Mutex<HashMap<String, Arc<Mutex<WebSessionState>>>>>,
    starting: Arc<Mutex<HashMap<String, StartingTurn>>>,
    attachments: Arc<Mutex<HashMap<String, StagedAttachment>>>,
    known_workspaces: Arc<Mutex<Vec<PathBuf>>>,
    /// Serializes lazy session-state creation (JSONL bootstrap) so two
    /// concurrent subscribers can never bootstrap the same session twice.
    bootstrap_lock: Arc<tokio::sync::Mutex<()>>,
}

impl WebSessionHost {
    #[must_use]
    pub(crate) fn new(config: AppConfig, relay: Arc<Relay>) -> Self {
        let known_workspaces = neo_home()
            .and_then(|home| {
                crate::json_store::read_or_default(
                    &home.join("webui_workspaces.json"),
                    "webui workspace",
                )
                .ok()
            })
            .unwrap_or_default();
        Self {
            config,
            relay,
            states: Arc::new(Mutex::new(HashMap::new())),
            starting: Arc::new(Mutex::new(HashMap::new())),
            attachments: Arc::new(Mutex::new(HashMap::new())),
            known_workspaces: Arc::new(Mutex::new(known_workspaces)),
            bootstrap_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn internal() -> WebUiError {
        WebUiError::new(WebUiErrorCode::Internal)
    }

    fn project_preferences(&self) -> WebUiProjectPreferences {
        neo_home()
            .and_then(|home| {
                crate::json_store::read_or_default(
                    &home.join("webui_projects.json"),
                    "webui project",
                )
                .ok()
            })
            .unwrap_or_default()
    }

    /// The session bucket directory used by the metadata store (the same
    /// bucket the runtime writes).
    fn metadata_store(&self) -> SessionMetadataStore {
        SessionMetadataStore::new(workspace_sessions_dir(&self.config))
    }

    /// The global session index handle (cross-workspace aggregation and
    /// per-session bucket resolution both read it).
    fn session_index(&self) -> Result<SessionIndex, WebUiError> {
        let neo_home = self
            .config
            .sessions_dir
            .parent()
            .ok_or_else(Self::internal)?;
        Ok(SessionIndex::new(neo_home))
    }

    /// The bucket directory that actually owns `session_id` (its indexed
    /// bucket for a session from another workspace, the current bucket
    /// otherwise).
    fn session_bucket_for(&self, session_id: &str) -> PathBuf {
        if let Ok(index) = self.session_index()
            && let Ok(entries) = index.list_all_in_sessions_root(&self.config.sessions_dir)
            && let Some(entry) = entries
                .into_iter()
                .rev()
                .find(|entry| entry.session_id == session_id)
        {
            return entry.session_dir;
        }
        workspace_sessions_dir(&self.config)
    }

    /// The metadata store of the bucket that actually owns `session_id`.
    fn metadata_store_for(&self, session_id: &str) -> SessionMetadataStore {
        SessionMetadataStore::new(self.session_bucket_for(session_id))
    }

    /// The session's own recorded workspace and its display label. Sessions
    /// from another workspace load with their recorded workdir (CLI
    /// cross-directory resume semantics); the label is never a path.
    fn session_workspace(&self, session_id: &str) -> (PathBuf, String) {
        let workdir = self
            .session_index()
            .ok()
            .and_then(|index| index.find(session_id).ok().flatten())
            .map(|entry| entry.workdir)
            .unwrap_or_else(|| self.config.project_dir.clone());
        let label = workspace_label_for(&workdir, &self.known_workdirs());
        (workdir, label)
    }

    /// Every known workspace root: the current project, explicitly added
    /// projects and workdirs recorded in the session index.
    fn known_workdirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.config.project_dir.clone()];
        if let Ok(known) = self.known_workspaces.lock() {
            for workdir in known.iter().filter(|path| path.is_dir()) {
                if !dirs.contains(workdir) {
                    dirs.push(workdir.clone());
                }
            }
        }
        if let Ok(index) = self.session_index()
            && let Ok(entries) = index.list_all_in_sessions_root(&self.config.sessions_dir)
        {
            for entry in entries {
                if !dirs.contains(&entry.workdir) {
                    dirs.push(entry.workdir);
                }
            }
        }
        dirs
    }

    fn workspace_for_id(&self, workspace_id: Option<&str>) -> Result<PathBuf, WebUiError> {
        let Some(workspace_id) = workspace_id else {
            return Ok(self.config.project_dir.clone());
        };
        self.known_workdirs()
            .into_iter()
            .find(|path| workspace_id_for(path) == workspace_id)
            .ok_or_else(|| WebUiError::new(WebUiErrorCode::InvalidRequest))
    }

    fn config_for_workspace(&self, workspace: PathBuf) -> Result<AppConfig, WebUiError> {
        let yolo = self.config.permission_mode == neo_agent_core::PermissionMode::Yolo
            && !self.config.project_trusted
            && matches!(
                &self.config.project_trust,
                crate::trust::ProjectTrustState::NotRequired
            );
        let mut config = AppConfig::load(ConfigOverrides {
            config_path: Some(self.config.config_path.clone()),
            yolo,
            auto: !yolo && self.config.permission_mode == neo_agent_core::PermissionMode::Auto,
            trust_store: None,
            project_dir: Some(workspace),
        })
        .map_err(|_| Self::internal())?;
        let workflow_definitions = config.workflow_definitions.clone();
        config.inherit_live_state(&self.config);
        config.workflow_definitions = workflow_definitions;
        Ok(config)
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
    /// canonical JSONL history and build one snapshot projection under the
    /// bootstrap lock so two concurrent subscribers never bootstrap twice.
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
        // The session loads with its own recorded workspace (projection and
        // summary label both follow it), exactly like CLI cross-directory
        // resume; the browser only ever sees the label.
        let (workspace, workspace_label) = self.session_workspace(session_id);
        let summary_sink = super::session::SessionSummarySink::new(
            (*self.relay).clone(),
            self.session_bucket_for(session_id),
        );
        let mut state = WebSessionState::new(
            session_id.to_owned(),
            session_dir,
            self.relay.publisher(session_id),
            PerSessionContainers::fresh(&self.config_for_workspace(workspace.clone())?),
            workspace,
            workspace_label,
            Some(summary_sink),
        );
        state.bootstrap_projection(events);
        let state = Arc::new(Mutex::new(state));
        let mut states = self.states.lock().map_err(|_| Self::internal())?;
        Ok(Arc::clone(
            states
                .entry(session_id.to_owned())
                .or_insert_with(|| Arc::clone(&state)),
        ))
    }

    /// Resolve a composer model alias into the typed turn selection.
    fn resolve_model(
        config: &AppConfig,
        alias: &str,
    ) -> Option<crate::modes::interactive::SelectedModel> {
        if let Some(cfg) = config.models.get(alias) {
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
        config: &AppConfig,
        containers: &PerSessionContainers,
        session_id: Option<String>,
        prompt: Vec<Content>,
        prompt_display_text: Option<String>,
        composer: Option<&WebUiComposer>,
    ) -> Result<TurnRequest, WebUiError> {
        let model = match composer.and_then(|c| c.model.as_deref()) {
            None => None,
            Some(alias) => Some(
                Self::resolve_model(config, alias)
                    .ok_or_else(|| WebUiError::new(WebUiErrorCode::InvalidRequest))?,
            ),
        };
        let reasoning = composer
            .and_then(|composer| composer.reasoning.clone())
            .unwrap_or_else(|| config.runtime.reasoning.clone());
        let reasoning_model = composer
            .and_then(|composer| composer.model.as_deref())
            .unwrap_or(&config.default_model);
        if let Some(model) = config.models.get(reasoning_model)
            && !model.reasoning.supports(&reasoning)
        {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let permission_mode = composer
            .and_then(|c| c.permission_mode)
            .unwrap_or(config.permission_mode);
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
        request.base_config = Some(config.clone());
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
            pending_questions: guard
                .pending_questions
                .iter()
                .map(|entry| entry.web.clone())
                .collect(),
            todos: guard.last_todos.clone(),
        }
    }

    /// Sessions grouped under every known workspace (the current workspace
    /// first). Explicitly added projects remain visible before their first
    /// session. Any aggregation
    /// failure degrades to the current workspace only. The group label is a
    /// display label; absolute workspace paths never leave the service.
    fn aggregated_workspaces(&self) -> Vec<WebUiWorkspaceGroup> {
        let current_bucket = workspace_sessions_dir(&self.config);
        let current_records = self.metadata_store().list().unwrap_or_default();
        let mut groups: Vec<(PathBuf, Vec<SessionRecord>)> =
            vec![(self.config.project_dir.clone(), current_records)];
        if let Ok(index) = self.session_index()
            && let Ok(entries) = index.list_all_in_sessions_root(&self.config.sessions_dir)
        {
            // The index is append-only: the latest entry per session id wins.
            let mut latest: HashMap<String, neo_agent_core::session::SessionIndexEntry> =
                HashMap::new();
            for entry in entries {
                latest.insert(entry.session_id.clone(), entry);
            }
            let current_ids: std::collections::HashSet<String> =
                groups[0].1.iter().map(|record| record.id.clone()).collect();
            let mut bucket_records: HashMap<PathBuf, Vec<SessionRecord>> = HashMap::new();
            for entry in latest.into_values() {
                if !entry.workdir.is_dir() {
                    continue;
                }
                if entry.session_dir == current_bucket
                    || current_ids.contains(entry.session_id.as_str())
                {
                    continue;
                }
                let records = bucket_records
                    .entry(entry.session_dir.clone())
                    .or_insert_with(|| {
                        SessionMetadataStore::new(&entry.session_dir)
                            .list()
                            .unwrap_or_default()
                    });
                let Some(record) = records
                    .iter()
                    .find(|record| record.id == entry.session_id)
                    .cloned()
                else {
                    continue;
                };
                match groups.iter_mut().find(|(dir, _)| *dir == entry.workdir) {
                    Some((_, sessions)) => sessions.push(record),
                    None => groups.push((entry.workdir.clone(), vec![record])),
                }
            }
        }
        for workdir in self.known_workdirs() {
            if !groups.iter().any(|(dir, _)| *dir == workdir) {
                groups.push((workdir, Vec::new()));
            }
        }
        let preferences = self.project_preferences();
        groups.retain(|(dir, _)| {
            !preferences
                .get(&workspace_id_for(dir))
                .is_some_and(|preference| preference.removed)
        });
        let workdirs: Vec<PathBuf> = groups.iter().map(|(dir, _)| dir.clone()).collect();
        let mut workspaces: Vec<WebUiWorkspaceGroup> = groups
            .into_iter()
            .enumerate()
            .map(|(position, (dir, mut records))| {
                let workspace_id = workspace_id_for(&dir);
                let preference = preferences.get(&workspace_id);
                let label = preference
                    .and_then(|preference| preference.display_name.clone())
                    .unwrap_or_else(|| workspace_label_for(&dir, &workdirs));
                // Same ordering as the session list: pinned first, then
                // updated_at descending, then id ascending.
                records.sort_by(|left, right| {
                    right
                        .pinned
                        .cmp(&left.pinned)
                        .then_with(|| right.updated_at.cmp(&left.updated_at))
                        .then_with(|| left.id.cmp(&right.id))
                });
                let sessions = records
                    .into_iter()
                    .map(|record| {
                        let mut summary = self.session_summary(record, &label);
                        summary.unread = summary.updated_at.as_ref().is_some_and(|updated_at| {
                            preference.is_some_and(|preference| {
                                preference
                                    .read_at_by_session
                                    .get(&summary.session_id)
                                    .is_none_or(|read_at| updated_at > read_at)
                            })
                        });
                        summary
                    })
                    .collect();
                WebUiWorkspaceGroup {
                    id: workspace_id,
                    label,
                    branch: crate::git_status::collect_workspace_status(&dir)
                        .map(|status| status.branch),
                    current: position == 0,
                    pinned: preference.is_some_and(|preference| preference.pinned),
                    sessions,
                }
            })
            .collect();
        // Current workspace first; the rest by most recent activity.
        workspaces[1..].sort_by(|left, right| {
            let recent = |group: &WebUiWorkspaceGroup| {
                group
                    .sessions
                    .iter()
                    .filter_map(|session| session.updated_at.clone())
                    .max()
            };
            right.pinned.cmp(&left.pinned).then_with(|| {
                recent(right)
                    .cmp(&recent(left))
                    .then_with(|| left.label.cmp(&right.label))
            })
        });
        workspaces
    }

    fn session_summary(
        &self,
        record: neo_agent_core::session::SessionRecord,
        workspace_label: &str,
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
            unread: false,
            state,
            workspace_label: workspace_label.to_owned(),
        }
    }

    fn list_sessions(
        &self,
        scope: WebUiSessionScope,
        query: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<WebUiSessionPage, WebUiError> {
        // The flat page aggregates every known workspace (each row carries
        // its `workspace_label`); the grouped shape is `workspace_snapshot`.
        let mut items: Vec<WebUiSessionSummary> =
            self.aggregated_workspaces()
                .into_iter()
                .flat_map(|group| group.sessions)
                .filter(|summary| match scope {
                    WebUiSessionScope::Active => !summary.archived,
                    WebUiSessionScope::Archived => summary.archived,
                })
                .filter(|summary| {
                    query.is_none_or(|query| {
                        summary.title.as_deref().is_some_and(|title| {
                            title.to_lowercase().contains(&query.to_lowercase())
                        })
                    })
                })
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
            .metadata_store_for(session_id)
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

    /// Directory of the digest-addressed attachment staging area. Bytes land
    /// here at upload; at send time they are materialized into the target
    /// session's own blob store (`<session_dir>/blobs/<sha256>.bin`), the
    /// same shape the runtime media chain resolves. This is the WebUI
    /// transport half of the media design's `MediaRef::Blob` flow; the
    /// agent-core request projection (capability trim, deterministic text
    /// replacement) is already upstream and untouched here.
    fn attachments_dir(&self) -> Result<PathBuf, WebUiError> {
        let neo_home = crate::config::neo_home().ok_or_else(Self::internal)?;
        Ok(neo_home.join("attachments"))
    }

    /// `POST /api/attachments`: whitelist the MIME type (images only), bound
    /// the decoded bytes, then stage them digest-addressed. The digest is the
    /// opaque id passed back by the browser on send.
    async fn upload_attachment(
        &self,
        mime: String,
        base64: String,
    ) -> Result<WebUiReply, WebUiError> {
        let valid_mime = mime.len() <= 128
            && mime
                .strip_prefix("image/")
                .is_some_and(|rest| !rest.is_empty())
            && mime.chars().all(|c: char| c.is_ascii_graphic());
        if !valid_mime {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(base64.as_bytes())
            .map_err(|_| WebUiError::new(WebUiErrorCode::InvalidRequest))?;
        if bytes.is_empty() {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        if bytes.len() > ATTACHMENT_MAX_BYTES {
            return Err(WebUiError::new(WebUiErrorCode::TooLarge));
        }
        let digest = sha256_hex(&bytes);
        let dir = self.attachments_dir()?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|_| Self::internal())?;
        tokio::fs::write(dir.join(format!("{digest}.bin")), &bytes)
            .await
            .map_err(|_| Self::internal())?;
        let byte_len = bytes.len() as u64;
        self.attachments
            .lock()
            .map_err(|_| Self::internal())?
            .insert(digest.clone(), StagedAttachment { mime: mime.clone() });
        Ok(WebUiReply::AttachmentUploaded(WebUiAttachmentAck {
            id: digest,
            mime,
            byte_len,
        }))
    }

    /// Validate one attachments id list (count cap, well-formed digests,
    /// known staged ids) without materializing anything.
    fn validate_attachment_ids(&self, ids: &Option<Vec<String>>) -> Result<(), WebUiError> {
        let Some(ids) = ids else {
            return Ok(());
        };
        if ids.len() > ATTACHMENTS_PER_MESSAGE_MAX {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let staged = self.attachments.lock().map_err(|_| Self::internal())?;
        for id in ids {
            // Staged ids are sha256 hex digests; anything else is rejected
            // before it can reach a path join.
            let well_formed = id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit());
            if !well_formed || !staged.contains_key(id) {
                return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
            }
        }
        Ok(())
    }

    /// Resolve attachment ids into prompt media parts: every staged blob is
    /// materialized into the session's own blob store before the prompt is
    /// built, so the runtime's media projection resolves `MediaRef::Blob`
    /// exactly like tool-produced media. Unsendable media becomes the
    /// runtime's deterministic text replacement on the request copy; the
    /// canonical history keeps the blob reference and is never rewritten.
    async fn attachment_parts(
        &self,
        ids: &Option<Vec<String>>,
        session_dir: &Path,
    ) -> Result<Vec<Content>, WebUiError> {
        self.validate_attachment_ids(ids)?;
        let Some(ids) = ids else {
            return Ok(Vec::new());
        };
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let staging_dir = self.attachments_dir()?;
        let staged: Vec<(String, String)> = {
            let staged = self.attachments.lock().map_err(|_| Self::internal())?;
            ids.iter()
                .map(|id| {
                    let mime = staged
                        .get(id)
                        .map(|entry| entry.mime.clone())
                        .ok_or_else(|| WebUiError::new(WebUiErrorCode::InvalidRequest))?;
                    Ok((id.clone(), mime))
                })
                .collect::<Result<_, WebUiError>>()?
        };
        let blob_dir = session_dir.join("blobs");
        tokio::fs::create_dir_all(&blob_dir)
            .await
            .map_err(|_| Self::internal())?;
        let mut parts = Vec::with_capacity(staged.len());
        for (id, mime) in staged {
            let bytes = tokio::fs::read(staging_dir.join(format!("{id}.bin")))
                .await
                .map_err(|_| WebUiError::new(WebUiErrorCode::InvalidRequest))?;
            // Atomic write (temp + rename), mirroring the runtime blob store.
            let blob_path = blob_dir.join(format!("{id}.bin"));
            let tmp_path = blob_dir.join(format!(".tmp-{id}"));
            if tokio::fs::write(&tmp_path, &bytes).await.is_err() {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(Self::internal());
            }
            if tokio::fs::rename(&tmp_path, &blob_path).await.is_err() {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(Self::internal());
            }
            parts.push(Content::Image {
                mime_type: mime.into(),
                data: MediaRef::Blob(id.into()),
            });
        }
        Ok(parts)
    }

    /// `GET .../agents/<agent_id>/history`: replay the child agent's
    /// persisted wire through the main session's web projection. Unknown
    /// sessions, malformed ids, and ids whose wire file does not exist under
    /// this exact session (including agents of other sessions) all get the
    /// same `not_found`. Read on demand: no cache, no new event store.
    async fn agent_history(
        &self,
        session_id: &str,
        agent_id: &str,
    ) -> Result<WebUiReply, WebUiError> {
        let not_found = || WebUiError::new(WebUiErrorCode::NotFound);
        let well_formed = !agent_id.is_empty()
            && agent_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            && !agent_id.contains("..");
        if !well_formed {
            return Err(not_found());
        }
        let session_dir =
            sessions::session_dir(session_id, &self.config).map_err(|_| not_found())?;
        let wire_path = agent_wire_path(&session_dir, agent_id);
        if !wire_path.exists() {
            return Err(not_found());
        }
        let events = JsonlSessionReader::read_all(&wire_path)
            .await
            .map_err(|_| not_found())?;
        // Reuse the session's own projection (workspace-relative paths,
        // opaque output references) via its state; the child events never
        // enter the session history or the relay.
        let state = self.state_for(session_id).await?;
        let guard = state.lock().map_err(|_| Self::internal())?;
        let history = guard.project_agent_history(events);
        let watermark = history.len() as u64;
        Ok(WebUiReply::AgentHistory(WebUiAgentHistory {
            agent_id: agent_id.to_owned(),
            watermark,
            history,
        }))
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

/// Display label for one workspace: the directory base name, disambiguated
/// with a short digest suffix only when another known workspace shares the
/// base name. Never contains a path separator or a full path.
fn workspace_label_for(workdir: &Path, all: &[PathBuf]) -> String {
    let base = workdir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace");
    let collides = all.iter().any(|other| {
        other != workdir && other.file_name().and_then(|name| name.to_str()) == Some(base)
    });
    if collides {
        format!(
            "{base}-{}",
            short_sha256_hex(workdir.to_string_lossy().as_bytes())
        )
    } else {
        base.to_owned()
    }
}

fn workspace_id_for(workdir: &Path) -> String {
    format!(
        "workspace_{}",
        sha256_hex(workdir.to_string_lossy().as_bytes())
    )
}

/// Display form of a canonicalized path for the folder picker. On Windows the
/// canonical form carries the `\\?\` verbatim prefix; strip it so breadcrumbs
/// and the submitted path read like a normal directory.
fn fs_display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        text.strip_prefix(r"\\?\")
            .map(str::to_owned)
            .unwrap_or_else(|| text.into_owned())
    }
    #[cfg(not(windows))]
    {
        text.into_owned()
    }
}

fn short_sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(bytes);
    digest[..3]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Full sha256 hex digest: the blob-store addressing form
/// (`blobs/<sha256>.bin`) the runtime media chain resolves.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[async_trait::async_trait]
impl WebUiHost for WebSessionHost {
    async fn execute(&self, command: WebUiCommand) -> Result<WebUiReply, WebUiError> {
        match command {
            WebUiCommand::Bootstrap => {
                // Read-only display catalog for the model pill overlay: alias,
                // provider id, context window and capability tags only — never
                // keys, base URLs or other provider configuration.
                let models: Vec<WebUiModelInfo> = self
                    .config
                    .models
                    .iter()
                    .map(|(alias, model)| WebUiModelInfo {
                        alias: alias.clone(),
                        provider: model.provider.clone(),
                        display_name: model.display_name.clone(),
                        context_window: model.max_context_tokens,
                        capabilities: model.capabilities.clone(),
                        reasoning: model.reasoning.clone(),
                    })
                    .collect();
                let sessions = self.list_sessions(WebUiSessionScope::Active, None, None, None)?;
                Ok(WebUiReply::Bootstrap(WebUiBootstrap {
                    workspace_label: self
                        .config
                        .project_dir
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_owned),
                    default_model: self.config.default_model.clone(),
                    default_reasoning: self.config.runtime.reasoning.clone(),
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
                    appearance: self.appearance(),
                    sessions: sessions.items,
                }))
            }
            WebUiCommand::CompleteInput { query } => {
                let skill_store = resources::load_skill_store(
                    neo_home().as_deref(),
                    &self.config.extra_skill_dirs,
                    &self.config.skill_path,
                )
                .ok();
                let items =
                    crate::modes::interactive::prompt_completion::prompt_completions_with_registry(
                        &self.config.project_dir,
                        &query,
                        skill_store.as_ref(),
                        self.config.project_trusted,
                        Some(&self.config.workflow_definitions),
                    )
                    .map_err(|_| Self::internal())?
                    .into_iter()
                    .map(|item| WebUiCompletionItem {
                        value: item.value,
                        label: item.label,
                        description: item.description,
                    })
                    .collect();
                Ok(WebUiReply::Completions(WebUiCompletions { items }))
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
            WebUiCommand::CreateSession {
                workspace_id,
                message,
                composer,
                attachments,
            } => {
                self.create_session(workspace_id.as_deref(), message, composer, attachments)
                    .await
            }
            WebUiCommand::StartTurn {
                session_id,
                message,
                composer,
                attachments,
            } => {
                self.start_turn(&session_id, message, composer, attachments)
                    .await
            }
            WebUiCommand::SendInput {
                session_id,
                turn_id,
                delivery,
                message,
                attachments,
            } => {
                if message.trim().is_empty() {
                    return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
                }
                let state = self.state_for(&session_id).await?;
                let session_dir = {
                    let guard = state.lock().map_err(|_| Self::internal())?;
                    guard.session_dir.clone()
                };
                let media = self.attachment_parts(&attachments, &session_dir).await?;
                match push_turn_input(&state, &turn_id, delivery, &message, media) {
                    Ok(true) => Ok(WebUiReply::InputAccepted { turn_id }),
                    Ok(false) => Err(WebUiError::new(WebUiErrorCode::TurnTransition)),
                    Err(error) => Err(error),
                }
            }
            WebUiCommand::QueueControl {
                session_id,
                turn_id,
                control,
            } => {
                let state = self.state_for(&session_id).await?;
                match push_queue_control(&state, &turn_id, control) {
                    Ok(true) => Ok(WebUiReply::Resolved),
                    Ok(false) => Err(WebUiError::new(WebUiErrorCode::TurnTransition)),
                    Err(error) => Err(error),
                }
            }
            WebUiCommand::UploadAttachment { mime, base64 } => {
                self.upload_attachment(mime, base64).await
            }
            WebUiCommand::AgentHistory {
                session_id,
                agent_id,
            } => self.agent_history(&session_id, &agent_id).await,
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
                    .metadata_store_for(&session_id)
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
                let (_, label) = self.session_workspace(&session_id);
                super::session::SessionSummarySink::new(
                    (*self.relay).clone(),
                    self.session_bucket_for(&session_id),
                )
                .publish(&session_id, dynamic, &label);
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
            WebUiCommand::AddWorkspace { path } => self.add_workspace(path),
            WebUiCommand::FsList { path } => self.fs_list(path.as_deref()),
            WebUiCommand::SettingsSnapshot => self.settings_snapshot(),
            WebUiCommand::SetDefaultModel { alias } => self.set_default_model(&alias),
            WebUiCommand::SetDefaultModelSelection { alias, reasoning } => {
                self.set_default_model_selection(&alias, reasoning)
            }
            WebUiCommand::SetPermissionMode { mode } => self.set_permission_mode(&mode),
            WebUiCommand::SetAppearance { appearance } => self.set_appearance(appearance),
            WebUiCommand::SetMcpServerEnabled { server_id, enabled } => {
                self.set_mcp_server_enabled(&server_id, enabled)
            }
            WebUiCommand::RemoveMcpServer { server_id } => self.remove_mcp_server(&server_id),
            WebUiCommand::UpsertMcpServer { server } => self.upsert_mcp_server(server),
            WebUiCommand::AddProvider { provider } => self.add_provider(provider),
            WebUiCommand::RemoveProvider { provider_id } => self.remove_provider(&provider_id),
            WebUiCommand::AddModel { model } => self.add_model(model),
            WebUiCommand::RemoveModel { alias } => self.remove_model(&alias),
            WebUiCommand::ListBranches { workspace_id } => self.list_branches(&workspace_id).await,
            WebUiCommand::CheckoutBranch {
                workspace_id,
                name,
                create,
            } => self.checkout_branch(&workspace_id, &name, create).await,
            WebUiCommand::RevealWorkspace { workspace_id } => self.reveal_workspace(&workspace_id),
            WebUiCommand::UpdateWorkspace {
                workspace_id,
                label,
                pinned,
                removed,
                mark_read,
                read_session_id,
            } => self.update_workspace(
                &workspace_id,
                label,
                pinned,
                removed,
                mark_read,
                read_session_id.as_deref(),
            ),
        }
    }

    async fn session_exists(&self, session_id: &str) -> bool {
        sessions::session_path(session_id, &self.config)
            .map(|path| path.exists())
            .unwrap_or(false)
    }

    async fn workspace_snapshot(&self) -> Result<WebUiWorkspaceSnapshot, WebUiError> {
        Ok(WebUiWorkspaceSnapshot {
            stream_id: self.relay.stream_id().to_owned(),
            workspace_sequence: self.relay.workspace_sequence(),
            workspaces: self.aggregated_workspaces(),
        })
    }

    async fn subscribe(&self, session_id: &str) -> Result<WebUiSnapshot, WebUiError> {
        self.snapshot_for(session_id).await
    }
}

impl WebSessionHost {
    fn update_workspace(
        &self,
        workspace_id: &str,
        label: Option<String>,
        pinned: Option<bool>,
        removed: Option<bool>,
        mark_read: bool,
        read_session_id: Option<&str>,
    ) -> Result<WebUiReply, WebUiError> {
        if !self
            .known_workdirs()
            .iter()
            .any(|path| workspace_id_for(path) == workspace_id)
        {
            return Err(WebUiError::new(WebUiErrorCode::NotFound));
        }
        let read_at_by_session = mark_read.then(|| {
            self.aggregated_workspaces()
                .into_iter()
                .find(|group| group.id == workspace_id)
                .map(|group| {
                    group
                        .sessions
                        .into_iter()
                        .filter_map(|session| {
                            session
                                .updated_at
                                .map(|updated_at| (session.session_id, updated_at))
                        })
                        .collect::<HashMap<_, _>>()
                })
                .unwrap_or_default()
        });
        let read_session = read_session_id.and_then(|session_id| {
            // Resolve the marker timestamp from the session's own bucket
            // metadata instead of the aggregated view: aggregation can lag or
            // omit a session (index/bucket mismatch), which previously made
            // the PATCH silently write nothing while the frontend locally
            // hid the unread dot.
            let record = self
                .metadata_store_for(session_id)
                .list()
                .ok()?
                .into_iter()
                .find(|record| record.id == session_id)?;
            record
                .updated_at
                .map(|updated_at| (session_id.to_owned(), updated_at))
        });
        let home = neo_home().ok_or_else(Self::internal)?;
        crate::json_store::update(
            &home.join("webui_projects.json"),
            "webui project",
            |preferences: &mut WebUiProjectPreferences| {
                let preference = preferences.entry(workspace_id.to_owned()).or_default();
                if let Some(label) = label {
                    preference.display_name =
                        (!label.trim().is_empty()).then(|| label.trim().to_owned());
                }
                if let Some(pinned) = pinned {
                    preference.pinned = pinned;
                }
                if let Some(removed) = removed {
                    preference.removed = removed;
                }
                if let Some(read_at_by_session) = read_at_by_session {
                    preference.read_at_by_session = read_at_by_session;
                }
                if let Some((session_id, updated_at)) = read_session {
                    preference.read_at_by_session.insert(session_id, updated_at);
                }
            },
        )
        .map_err(|_| Self::internal())?;
        Ok(WebUiReply::Resolved)
    }

    fn reveal_workspace(&self, workspace_id: &str) -> Result<WebUiReply, WebUiError> {
        let workspace = self
            .known_workdirs()
            .into_iter()
            .find(|path| workspace_id_for(path) == workspace_id)
            .ok_or_else(|| WebUiError::new(WebUiErrorCode::NotFound))?;
        reveal_in_file_manager(&workspace).map_err(|_| Self::internal())?;
        Ok(WebUiReply::Resolved)
    }

    fn fs_list(&self, path: Option<&str>) -> Result<WebUiReply, WebUiError> {
        let directory = match path.map(str::trim).filter(|value| !value.is_empty()) {
            Some(raw) => crate::config::expand_user_path(PathBuf::from(raw))
                .canonicalize()
                .map_err(|_| WebUiError::new(WebUiErrorCode::InvalidRequest))?,
            None => crate::config::user_home()
                .or_else(neo_home)
                .unwrap_or_else(|| self.config.project_dir.clone()),
        };
        if !directory.is_dir() {
            return Err(WebUiError::new(WebUiErrorCode::NotFound));
        }
        let mut entries = Vec::new();
        let read =
            fs::read_dir(&directory).map_err(|_| WebUiError::new(WebUiErrorCode::NotFound))?;
        for entry in read.flatten() {
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            entries.push(WebUiFsEntry { name, is_dir });
            if entries.len() >= FS_LIST_MAX_ENTRIES {
                break;
            }
        }
        // Folders first, then case-insensitive name order for stable browsing.
        entries.sort_by(|left, right| {
            right
                .is_dir
                .cmp(&left.is_dir)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
        });
        let parent = directory.parent().map(|parent| fs_display_path(parent));
        Ok(WebUiReply::FsList(WebUiFsListing {
            path: fs_display_path(&directory),
            parent,
            entries,
        }))
    }

    fn settings_snapshot(&self) -> Result<WebUiReply, WebUiError> {
        // Default model and MCP servers are persisted in config.toml; read
        // the file so a mutation made through this same service is reflected
        // immediately (the in-memory AppConfig is a startup snapshot).
        let persisted = crate::config::default_config_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| toml::from_str::<crate::config::FileConfig>(&text).ok());
        let default_model = persisted
            .as_ref()
            .and_then(|file| file.default_model.clone())
            .unwrap_or_else(|| self.config.default_model.clone());
        let default_provider = persisted
            .as_ref()
            .and_then(|file| file.default_provider.clone())
            .unwrap_or_else(|| self.config.default_provider.clone());
        let mcp_servers: Vec<WebUiMcpServerInfo> = persisted
            .as_ref()
            .and_then(|file| file.mcp.as_ref())
            .map(|mcp| mcp.servers.as_slice())
            .unwrap_or(&self.config.mcp.servers)
            .iter()
            .map(|server| WebUiMcpServerInfo {
                id: server.id.clone(),
                enabled: server.enabled,
                transport: server.transport.as_str().to_owned(),
                command: server.command.clone(),
                url: server.url.clone(),
                tool_count: server.enabled_tools.len(),
                env_keys: server.env.keys().cloned().collect(),
                header_keys: server.headers.keys().cloned().collect(),
            })
            .collect();

        let base_models = persisted
            .as_ref()
            .and_then(|file| file.models.as_ref())
            .unwrap_or(&self.config.models);
        let base_providers = persisted
            .as_ref()
            .and_then(|file| file.providers.as_ref())
            .unwrap_or(&self.config.providers);
        let mut models: Vec<WebUiModelInfo> = base_models
            .iter()
            .map(|(alias, model)| WebUiModelInfo {
                alias: alias.clone(),
                provider: model.provider.clone(),
                display_name: model.display_name.clone(),
                context_window: model.max_context_tokens,
                capabilities: model.capabilities.clone(),
                reasoning: model.reasoning.clone(),
            })
            .collect();
        // The model pill catalog only covers inline `[models.*]`; the default
        // alias can also be a catalog/module model (`provider/model`) not in
        // the file. Include it and the registry models of configured
        // providers so the settings select can reach every resolvable alias.
        let mut known_aliases: HashSet<String> =
            models.iter().map(|model| model.alias.clone()).collect();
        let default_alias = default_model.clone();
        if !known_aliases.contains(&default_alias) {
            let (provider, model) = default_alias
                .split_once('/')
                .map(|(provider, model)| (provider.to_owned(), model.to_owned()))
                .unwrap_or_else(|| (default_provider.clone(), default_alias.clone()));
            models.push(WebUiModelInfo {
                alias: default_alias.clone(),
                provider,
                display_name: (!model.is_empty()).then_some(model),
                context_window: None,
                capabilities: Vec::new(),
                reasoning: neo_ai::ReasoningCapability::None,
            });
            known_aliases.insert(default_alias.clone());
        }
        let registry = neo_ai::registry::ModelRegistry::seeded();
        for provider_id in base_providers.keys() {
            for spec in registry.list() {
                if spec.provider.0 != *provider_id {
                    continue;
                }
                let alias = format!("{provider_id}/{}", spec.model);
                if known_aliases.insert(alias.clone()) {
                    models.push(WebUiModelInfo {
                        alias,
                        provider: provider_id.clone(),
                        display_name: Some(spec.model.clone()),
                        context_window: None,
                        capabilities: Vec::new(),
                        reasoning: neo_ai::ReasoningCapability::None,
                    });
                }
            }
        }
        models.sort_by(|left, right| left.alias.cmp(&right.alias));
        let providers: Vec<WebUiProviderInfo> = base_providers
            .iter()
            .map(|(id, provider)| WebUiProviderInfo {
                id: id.clone(),
                display_name: provider.display_name.clone(),
                provider_type: provider
                    .provider_type
                    .as_ref()
                    .map(|provider_type| provider_type.as_config_str().to_owned()),
                base_url: provider.base_url.clone(),
                has_api_key: provider.api_key.is_some() || provider.api_key_env.is_some(),
            })
            .collect();
        let skills = resources::load_skill_store(
            neo_home().as_deref(),
            &self.config.extra_skill_dirs,
            &self.config.skill_path,
        )
        .map(|store| {
            store
                .iter()
                .map(|skill| WebUiSkillInfo {
                    name: skill.name.clone(),
                    display_name: Some(skill.display_name().to_owned()),
                    description: skill
                        .short_description()
                        .map(str::to_owned)
                        .or_else(|| Some(skill.manifest.description.clone())),
                })
                .collect()
        })
        .unwrap_or_default();
        let default_reasoning = persisted
            .as_ref()
            .and_then(|file| file.runtime.as_ref())
            .and_then(|runtime| runtime.reasoning.clone())
            .unwrap_or_else(|| self.config.runtime.reasoning.clone());
        let permission_mode = persisted
            .as_ref()
            .and_then(|file| file.permission_mode)
            .unwrap_or(self.config.permission_mode);
        Ok(WebUiReply::Settings(WebUiSettingsSnapshot {
            default_model,
            default_provider: Some(default_provider),
            permission_mode: permission_mode.label().to_owned(),
            default_reasoning,
            appearance: self.appearance(),
            providers,
            models,
            mcp_servers,
            skills,
        }))
    }

    fn set_default_model(&self, alias: &str) -> Result<WebUiReply, WebUiError> {
        let known = alias == self.config.default_model
            || self.model_known(alias)
            || alias
                .split_once('/')
                .is_some_and(|(provider, _)| self.provider_known(provider));
        if !known {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let config_path = crate::config::default_config_path().ok_or_else(Self::internal)?;
        crate::config::mutations::set_default_model(&config_path, alias)
            .map_err(|_| Self::internal())?;
        self.settings_snapshot()
    }

    /// Select the default model together with its reasoning intensity. Uses the
    /// same validation as `set_default_model` and persists model/provider/
    /// reasoning atomically.
    fn set_default_model_selection(
        &self,
        alias: &str,
        reasoning: neo_ai::ReasoningSelection,
    ) -> Result<WebUiReply, WebUiError> {
        let provider_id = self.provider_for_alias(alias)?;
        let config_path = crate::config::default_config_path().ok_or_else(Self::internal)?;
        crate::config::mutations::set_model_selection(
            &config_path,
            alias,
            &provider_id,
            &reasoning,
        )
        .map_err(|_| Self::internal())?;
        self.settings_snapshot()
    }

    fn set_permission_mode(&self, mode: &str) -> Result<WebUiReply, WebUiError> {
        let mode = match mode {
            "ask" => neo_agent_core::PermissionMode::Ask,
            "auto" => neo_agent_core::PermissionMode::Auto,
            "yolo" => neo_agent_core::PermissionMode::Yolo,
            _ => return Err(WebUiError::new(WebUiErrorCode::InvalidRequest)),
        };
        let config_path = crate::config::default_config_path().ok_or_else(Self::internal)?;
        crate::config::mutations::set_permission_mode(&config_path, mode)
            .map_err(|_| Self::internal())?;
        self.settings_snapshot()
    }

    fn set_appearance(&self, appearance: WebUiAppearance) -> Result<WebUiReply, WebUiError> {
        let config_path = crate::config::default_config_path().ok_or_else(Self::internal)?;
        crate::config::mutations::set_appearance(
            &config_path,
            &ConfigWebUiAppearance {
                theme: appearance.theme,
                ui_font_size: appearance.ui_font_size,
                code_font_size: appearance.code_font_size,
                code_theme: appearance.code_theme,
                show_line_numbers: appearance.show_line_numbers,
                word_wrap: appearance.word_wrap,
            },
        )
        .map_err(|_| Self::internal())?;
        self.settings_snapshot()
    }

    fn add_provider(&self, edit: WebUiProviderEdit) -> Result<WebUiReply, WebUiError> {
        if edit.id.trim().is_empty() {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let provider_type = match edit.provider_type.as_deref() {
            None => None,
            Some(raw) => Some(
                neo_ai::ApiType::from_config_str(raw)
                    .ok_or_else(|| WebUiError::new(WebUiErrorCode::InvalidRequest))?,
            ),
        };
        if edit.api_key.is_some() && edit.api_key_env.is_some() {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let provider = crate::config::ProviderConfig {
            display_name: edit.display_name,
            provider_type,
            base_url: edit.base_url,
            api_key: edit.api_key,
            api_key_env: edit.api_key_env,
        };
        let config_path = crate::config::default_config_path().ok_or_else(Self::internal)?;
        crate::config::mutations::add_provider(&config_path, &edit.id, provider)
            .map_err(|_| Self::internal())?;
        self.settings_snapshot()
    }

    fn remove_provider(&self, provider_id: &str) -> Result<WebUiReply, WebUiError> {
        let config_path = crate::config::default_config_path().ok_or_else(Self::internal)?;
        crate::config::mutations::remove_provider(&config_path, provider_id)
            .map_err(|_| Self::internal())?;
        self.settings_snapshot()
    }

    fn add_model(&self, edit: WebUiModelEdit) -> Result<WebUiReply, WebUiError> {
        if edit.alias.trim().is_empty()
            || edit.model.trim().is_empty()
            || !self.provider_known(&edit.provider)
        {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let model = crate::config::ModelConfig {
            provider: edit.provider,
            model: edit.model,
            max_context_tokens: edit.max_context_tokens,
            max_output_tokens: edit.max_output_tokens,
            capabilities: edit.capabilities,
            reasoning: edit.reasoning,
            display_name: edit.display_name,
        };
        let config_path = crate::config::default_config_path().ok_or_else(Self::internal)?;
        crate::config::mutations::add_model(&config_path, &edit.alias, model)
            .map_err(|_| Self::internal())?;
        self.settings_snapshot()
    }

    fn remove_model(&self, alias: &str) -> Result<WebUiReply, WebUiError> {
        let config_path = crate::config::default_config_path().ok_or_else(Self::internal)?;
        crate::config::mutations::remove_model(&config_path, alias)
            .map_err(|_| Self::internal())?;
        self.settings_snapshot()
    }

    /// Resolve the provider id that owns `alias` (inline model, `provider/model`,
    /// or the default provider) for `set_model_selection`. Checks the persisted
    /// config first so a model added through this service resolves immediately.
    fn provider_for_alias(&self, alias: &str) -> Result<String, WebUiError> {
        if let Some(file) = self.persisted_file()
            && let Some(model) = file.models.as_ref().and_then(|models| models.get(alias))
        {
            return Ok(model.provider.clone());
        }
        if let Some(model) = self.config.models.get(alias) {
            return Ok(model.provider.clone());
        }
        if let Some((provider, _)) = alias.split_once('/') {
            return Ok(provider.to_owned());
        }
        Ok(self.config.default_provider.clone())
    }

    /// Read the persisted `config.toml` (the in-memory `AppConfig` is a
    /// startup snapshot; mutations only touch the file).
    fn persisted_file(&self) -> Option<crate::config::FileConfig> {
        crate::config::default_config_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| toml::from_str::<crate::config::FileConfig>(&text).ok())
    }

    fn provider_known(&self, id: &str) -> bool {
        self.config.providers.contains_key(id)
            || self
                .persisted_file()
                .and_then(|file| file.providers)
                .is_some_and(|providers| providers.contains_key(id))
    }

    fn model_known(&self, alias: &str) -> bool {
        self.config.models.contains_key(alias)
            || self
                .persisted_file()
                .and_then(|file| file.models)
                .is_some_and(|models| models.contains_key(alias))
    }

    /// Persistent WebUI appearance from the `[webui]` config table, with
    /// defaults applied. Reads the file directly so a write is reflected
    /// immediately (the in-memory `AppConfig` is a startup snapshot).
    fn appearance(&self) -> WebUiAppearance {
        let persisted = self.persisted_file();
        let cfg = ConfigWebUiAppearance::from_file(
            persisted.as_ref().and_then(|file| file.webui.as_ref()),
        );
        WebUiAppearance {
            theme: cfg.theme,
            ui_font_size: cfg.ui_font_size,
            code_font_size: cfg.code_font_size,
            code_theme: cfg.code_theme,
            show_line_numbers: cfg.show_line_numbers,
            word_wrap: cfg.word_wrap,
        }
    }

    fn set_mcp_server_enabled(
        &self,
        server_id: &str,
        enabled: bool,
    ) -> Result<WebUiReply, WebUiError> {
        let config_path = crate::config::default_config_path().ok_or_else(Self::internal)?;
        crate::config::mutations::set_mcp_server_enabled(server_id, enabled, &config_path)
            .map_err(|_| Self::internal())?;
        self.settings_snapshot()
    }

    fn remove_mcp_server(&self, server_id: &str) -> Result<WebUiReply, WebUiError> {
        let config_path = crate::config::default_config_path().ok_or_else(Self::internal)?;
        crate::config::mutations::remove_mcp_server(server_id, &config_path)
            .map_err(|_| Self::internal())?;
        self.settings_snapshot()
    }

    fn upsert_mcp_server(&self, edit: WebUiMcpServerEdit) -> Result<WebUiReply, WebUiError> {
        let transport = match edit.transport.as_str() {
            "stdio" => crate::config::McpTransport::Stdio,
            "http" => crate::config::McpTransport::Http,
            "sse" => crate::config::McpTransport::Sse,
            _ => return Err(WebUiError::new(WebUiErrorCode::InvalidRequest)),
        };
        let server = crate::config::McpServerConfig {
            id: edit.id,
            enabled: edit.enabled,
            transport,
            command: edit.command,
            url: edit.url,
            args: edit.args,
            env: edit.env,
            headers: edit.headers,
            cwd: None,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            startup_timeout_ms: None,
            tool_timeout_ms: None,
        };
        let config_path = crate::config::default_config_path().ok_or_else(Self::internal)?;
        crate::config::mutations::upsert_mcp_server(&server, &config_path)
            .map_err(|_| Self::internal())?;
        self.settings_snapshot()
    }

    fn add_workspace(&self, path: String) -> Result<WebUiReply, WebUiError> {
        if path.trim().is_empty() {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let workspace = PathBuf::from(path)
            .canonicalize()
            .map_err(|_| WebUiError::new(WebUiErrorCode::InvalidRequest))?;
        if !workspace.is_dir() {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let home = neo_home().ok_or_else(Self::internal)?;
        crate::json_store::update(
            &home.join("webui_workspaces.json"),
            "webui workspace",
            |paths: &mut Vec<PathBuf>| {
                if !paths.contains(&workspace) {
                    paths.push(workspace.clone());
                }
            },
        )
        .map_err(|_| Self::internal())?;
        let mut known = self.known_workspaces.lock().map_err(|_| Self::internal())?;
        if !known.contains(&workspace) {
            known.push(workspace.clone());
        }
        drop(known);
        let workspace_id = workspace_id_for(&workspace);
        self.update_workspace(&workspace_id, None, None, Some(false), false, None)?;
        let group = self
            .aggregated_workspaces()
            .into_iter()
            .find(|group| group.id == workspace_id)
            .ok_or_else(Self::internal)?;
        Ok(WebUiReply::WorkspaceAdded(group))
    }

    /// List local branches of one workspace; read on demand with the shared
    /// git collector off the async runtime. Non-repositories produce the
    /// empty list, never an error.
    async fn list_branches(&self, workspace_id: &str) -> Result<WebUiReply, WebUiError> {
        let workspace = self.workspace_for_id(Some(workspace_id))?;
        let list =
            tokio::task::spawn_blocking(move || crate::git_status::list_branches(&workspace))
                .await
                .map_err(|_| Self::internal())?;
        Ok(WebUiReply::Branches(WebUiBranchList {
            current: list.as_ref().and_then(|branches| branches.current.clone()),
            branches: list.map(|branches| branches.branches).unwrap_or_default(),
        }))
    }

    /// Check out (optionally create) a workspace branch. Git failures are
    /// logged with the stderr tail and surfaced as a stable 400; success
    /// returns the re-aggregated workspace group so the browser refreshes
    /// the branch label in place.
    async fn checkout_branch(
        &self,
        workspace_id: &str,
        name: &str,
        create: bool,
    ) -> Result<WebUiReply, WebUiError> {
        let workspace = self.workspace_for_id(Some(workspace_id))?;
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let result = tokio::task::spawn_blocking(move || {
            crate::git_status::checkout_branch(&workspace, &name, create)
        })
        .await
        .map_err(|_| Self::internal())?;
        if let Err(message) = result {
            tracing::warn!(
                workspace = %workspace_id,
                message = %message,
                "webui branch checkout failed"
            );
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let group = self
            .aggregated_workspaces()
            .into_iter()
            .find(|group| group.id == workspace_id)
            .ok_or_else(Self::internal)?;
        Ok(WebUiReply::WorkspaceUpdated(group))
    }

    async fn create_session(
        &self,
        workspace_id: Option<&str>,
        message: String,
        composer: Option<WebUiComposer>,
        attachments: Option<Vec<String>>,
    ) -> Result<WebUiReply, WebUiError> {
        if message.trim().is_empty() {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        let workspace = self.workspace_for_id(workspace_id)?;
        let config = self.config_for_workspace(workspace.clone())?;
        let turn_id = new_turn_id();
        let containers = PerSessionContainers::fresh(&config);
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
        // With attachments the session must exist before the first model
        // request so the blobs can land in its own blob store: pre-create it,
        // materialize the blobs, then run the first turn in that session
        // (the runtime resolves `MediaRef::Blob` from `<session>/blobs/`).
        // Without attachments the runtime creates the session itself, as
        // before.
        // Validate attachment ids before any session is created so an
        // invalid request never leaves an orphan session behind.
        if attachments.is_some() {
            self.validate_attachment_ids(&attachments)?;
        }
        let mut prompt = vec![Content::text(message.clone())];
        let pre_created: Option<String> = if attachments.as_ref().is_some_and(|ids| !ids.is_empty())
        {
            let created = sessions::create_new_session(&config)
                .await
                .map_err(|_| Self::internal())?;
            // The existing-session driver replays the wire file; the
            // pre-created session has none yet.
            tokio::fs::write(&created.wire_path, b"")
                .await
                .map_err(|_| Self::internal())?;
            let session_dir = sessions::session_dir(&created.session_id, &config)
                .map_err(|_| Self::internal())?;
            let media = self.attachment_parts(&attachments, &session_dir).await?;
            prompt.extend(media);
            Some(created.session_id)
        } else {
            None
        };
        let request = self.build_request(
            &config,
            &containers,
            pre_created.clone(),
            prompt,
            Some(message),
            composer.as_ref(),
        )?;
        let task = self.spawn_turn_task(pre_created, request, channels, event_tx.clone());
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
        let workspace_label = workspace_label_for(&workspace, &self.known_workdirs());
        tokio::spawn(launch_new_session_turn(
            states,
            starting,
            relay,
            config,
            turn_id.clone(),
            containers,
            workspace_label,
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
        attachments: Option<Vec<String>>,
    ) -> Result<WebUiReply, WebUiError> {
        // New turns, like new sessions and active-turn input, reject blank
        // messages: the canonical `MessageAppended` user event is the only
        // source of the web user bubble, so an empty prompt can never start
        // a turn.
        if message.trim().is_empty() {
            return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
        }
        self.validate_attachment_ids(&attachments)?;
        let state = self.state_for(session_id).await?;
        let (workspace, _) = self.session_workspace(session_id);
        let config = self.config_for_workspace(workspace)?;
        // Materialize attachment blobs before the turn-registration lock so
        // the std mutex guard never crosses an `.await`.
        let media = {
            let session_dir = {
                let guard = state.lock().map_err(|_| Self::internal())?;
                guard.session_dir.clone()
            };
            self.attachment_parts(&attachments, &session_dir).await?
        };
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
            if guard.pending_approval.is_some() || !guard.pending_questions.is_empty() {
                return Err(WebUiError::new(WebUiErrorCode::TurnTransition));
            }
            // Validate the composer (model alias and reasoning selection) before
            // registering the turn: an invalid request must leave the session
            // untouched instead of registering a turn that can never run.
            let mut prompt = vec![Content::text(message.clone())];
            prompt.extend(media);
            let request = self.build_request(
                &config,
                &guard.containers,
                Some(session_id.to_owned()),
                prompt,
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

#[cfg(target_os = "macos")]
fn reveal_in_file_manager(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "windows")]
fn reveal_in_file_manager(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("explorer")
        .arg(path)
        .spawn()
        .map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_in_file_manager(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
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
    workspace_label: String,
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
            workspace_label,
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
