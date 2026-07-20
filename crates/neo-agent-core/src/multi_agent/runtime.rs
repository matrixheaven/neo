use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures::StreamExt;
use neo_ai::ModelClient;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::instructions::{
    AgentInstructionState, InstructionBudget, InstructionEpochData, InstructionInheritance,
    InstructionPreflightDecision,
};
use crate::runtime::{
    ActiveTurnInput, AgentConfig, AgentContext, SteerInputHandle, effective_max_context_tokens,
};
use crate::{
    AgentEvent, AgentMessage, AgentRuntime, AgentToolCall, Content, StopReason, ToolRegistry,
};

use super::state::derive_title;
use super::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentProgressSnapshot, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome,
    DelegateContext, DisplayNamePool, SwarmAggregate,
};
use super::{AgentTerminalReason, AgentToolActivityPhase, AgentToolOutputPreview};
use super::{apply_agent_progress, apply_swarm_child_progress};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DelegateRequest {
    #[schemars(
        description = "Required non-empty task for the subagent. For resume, this is the next user prompt for the same child agent."
    )]
    pub task: String,
    #[serde(default)]
    #[schemars(
        description = "Existing agent_id to continue. Must be omitted for a new agent. Must start with agent_, not swarm_."
    )]
    pub resume: Option<String>,
    #[serde(default)]
    #[schemars(
        description = "Short UI title. If omitted, Neo derives a deterministic local title from task."
    )]
    pub title: Option<String>,
    #[serde(default)]
    #[schemars(
        description = "Subagent role for new agents only. Defaults to coder. Must be omitted when resume is set."
    )]
    pub role: Option<AgentRole>,
    #[serde(default)]
    #[schemars(description = "Run mode. Defaults to foreground.")]
    pub mode: AgentRunMode,
    #[serde(default = "default_context")]
    #[schemars(description = "Context mode: inherit, summary, or none. Defaults to inherit.")]
    pub context: DelegateContext,
}

impl DelegateRequest {
    #[must_use]
    pub fn actual_role(&self) -> AgentRole {
        self.role.unwrap_or_default()
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DelegateSwarmItem {
    #[schemars(
        description = "Short human title for this child agent in ListDelegates and transcripts."
    )]
    pub title: String,
    #[schemars(description = "Item value inserted into prompt_template as {{item}}.")]
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DelegateSwarmRequest {
    #[schemars(
        description = "Required non-empty human title for the swarm. Not injected into every child task."
    )]
    pub description: String,
    #[serde(default)]
    #[schemars(
        description = "New child task items as an object array. Each item must be an object with required string fields title and value; value is inserted into prompt_template as {{item}}."
    )]
    pub items: Vec<DelegateSwarmItem>,
    #[serde(default)]
    #[schemars(
        description = "Template for new child tasks. Supports exactly {{item}} and optional {{description}}. Required when items is present."
    )]
    pub prompt_template: Option<String>,
    #[serde(default)]
    #[schemars(
        description = "JSON object map from existing agent_id to per-agent resume prompt, for example {\"agent_xxx\": \"continue with this prompt\"}. Do not pass an array."
    )]
    pub resume_agent_ids: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    #[schemars(description = "Subagent role for new children. Defaults to coder.")]
    pub role: AgentRole,
    #[serde(default)]
    #[schemars(description = "Run mode. Defaults to foreground.")]
    pub mode: AgentRunMode,
    #[schemars(
        description = "Optional max parallel child agents. Must be greater than 0 when provided."
    )]
    pub max_concurrency: Option<usize>,
}

fn default_context() -> DelegateContext {
    DelegateContext::Inherit
}

#[derive(Debug, Default)]
struct MultiAgentState {
    names: DisplayNamePool,
    next_created_index: u64,
    next_cancel_generation: u64,
    agent_order: BTreeMap<String, u64>,
    swarm_order: BTreeMap<String, u64>,
    agents: BTreeMap<String, AgentSnapshot>,
    retry_activity_starts: BTreeMap<String, usize>,
    swarms: BTreeMap<String, super::SwarmSnapshot>,
    steer_handles: BTreeMap<String, SteerInputHandle>,
    /// Live cancellation tokens for actively running child agents. Registered
    /// when a child run starts, removed when it finishes. Cancelling a token
    /// here stops the child's model stream immediately.
    agent_cancel_tokens: BTreeMap<String, LiveAgentCancel>,
}

impl MultiAgentState {
    fn next_created_index(&mut self) -> u64 {
        let index = self.next_created_index;
        self.next_created_index = self.next_created_index.saturating_add(1);
        index
    }

    fn register_agent_order(&mut self, agent_id: &str) {
        if !self.agent_order.contains_key(agent_id) {
            let index = self.next_created_index();
            self.agent_order.insert(agent_id.to_owned(), index);
        }
    }

    fn register_swarm_order(&mut self, swarm_id: &str) {
        if !self.swarm_order.contains_key(swarm_id) {
            let index = self.next_created_index();
            self.swarm_order.insert(swarm_id.to_owned(), index);
        }
    }

    fn next_cancel_generation(&mut self) -> u64 {
        let generation = self.next_cancel_generation;
        self.next_cancel_generation = self.next_cancel_generation.saturating_add(1);
        generation
    }
}

#[derive(Debug, Clone, Default)]
pub struct MultiAgentRuntime {
    state: Arc<Mutex<MultiAgentState>>,
    session_state_update_lock: Arc<tokio::sync::Mutex<()>>,
    session_directory: Option<PathBuf>,
}

impl MultiAgentRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_session_directory(mut self, session_directory: PathBuf) -> Self {
        self.session_directory = Some(session_directory);
        self
    }

    #[must_use]
    pub fn start_foreground_delegate_for_test(&self, task: &str) -> AgentSnapshot {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let display_name: AgentDisplayName = state.names.next_name();
        let id = AgentId::new();
        let path = AgentPath::root_child(&display_name);
        let snapshot = new_agent_snapshot(AgentSnapshotSeed {
            id: id.clone(),
            display_name,
            path,
            role: AgentRole::Coder,
            mode: AgentRunMode::Foreground,
            context: DelegateContext::Inherit,
            state: AgentLifecycleState::Running,
            task,
            title: None,
        });
        state.register_agent_order(id.as_str());
        state
            .agents
            .insert(id.as_str().to_owned(), snapshot.clone());
        snapshot
    }

    #[must_use]
    pub fn start_delegate(
        &self,
        task: &str,
        title: Option<&str>,
        role: AgentRole,
        mode: AgentRunMode,
        context: DelegateContext,
        path: AgentPathKind<'_>,
    ) -> AgentSnapshot {
        self.create_delegate(
            task,
            title,
            role,
            mode,
            context,
            path,
            AgentLifecycleState::Running,
        )
    }

    #[must_use]
    pub fn queue_delegate(
        &self,
        task: &str,
        title: Option<&str>,
        role: AgentRole,
        mode: AgentRunMode,
        context: DelegateContext,
        path: AgentPathKind<'_>,
    ) -> AgentSnapshot {
        self.create_delegate(
            task,
            title,
            role,
            mode,
            context,
            path,
            AgentLifecycleState::Queued,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn create_delegate(
        &self,
        task: &str,
        title: Option<&str>,
        role: AgentRole,
        mode: AgentRunMode,
        context: DelegateContext,
        path: AgentPathKind<'_>,
        lifecycle_state: AgentLifecycleState,
    ) -> AgentSnapshot {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let display_name: AgentDisplayName = state.names.next_name();
        let id = AgentId::new();
        let agent_path = match path {
            AgentPathKind::Root => AgentPath::root_child(&display_name),
            AgentPathKind::SwarmChild(swarm_id) => AgentPath::swarm_child(swarm_id, &display_name),
        };
        let snapshot = new_agent_snapshot(AgentSnapshotSeed {
            id: id.clone(),
            display_name,
            path: agent_path,
            role,
            mode,
            context,
            state: lifecycle_state,
            task,
            title,
        });
        state.register_agent_order(id.as_str());
        state
            .agents
            .insert(id.as_str().to_owned(), snapshot.clone());
        snapshot
    }

    #[must_use]
    pub fn mark_delegate_running(&self, id: &AgentId) -> Option<AgentSnapshot> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = state.agents.get_mut(id.as_str())?;
        // Never revive a terminal snapshot back to Running. This prevents a
        // queued swarm child that was cancelled by cancel_swarm from being
        // started again when its turn comes up in the scheduler.
        if snapshot.state.is_terminal() {
            return Some(snapshot.clone());
        }
        let now = now_ms();
        snapshot.state = AgentLifecycleState::Running;
        snapshot.started_at_ms.get_or_insert(now);
        snapshot.terminal_at_ms = None;
        snapshot.terminal_reason = None;
        snapshot.updated_at_ms = now;
        let activity_start = snapshot.activity.len();
        let snapshot = snapshot.clone();
        state
            .retry_activity_starts
            .insert(id.as_str().to_owned(), activity_start);
        Some(snapshot)
    }

    #[must_use]
    pub fn complete_delegate(&self, id: &AgentId, update: AgentRunUpdate) -> AgentSnapshot {
        self.update_terminal_delegate(id, AgentLifecycleState::Completed, update, false)
    }

    #[must_use]
    pub fn fail_delegate(&self, id: &AgentId, update: AgentRunUpdate) -> AgentSnapshot {
        self.update_terminal_delegate(id, AgentLifecycleState::Failed, update, true)
    }

    #[must_use]
    pub fn mark_background_terminal_reason(
        &self,
        id: &AgentId,
        state: AgentLifecycleState,
        reason: AgentTerminalReason,
        message: Option<String>,
    ) -> Option<AgentSnapshot> {
        let mut locked = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = locked.agents.get_mut(id.as_str())?;
        if snapshot.state.is_terminal() {
            return Some(snapshot.clone());
        }
        let now = now_ms();
        snapshot.state = state;
        snapshot.terminal_reason = Some(reason);
        snapshot.terminal_at_ms.get_or_insert(now);
        snapshot.updated_at_ms = now;
        if let Some(message) = message.filter(|value| !value.trim().is_empty()) {
            snapshot.latest_text = Some(bounded_latest_text(&message));
            snapshot.outcome = Some(AgentTerminalOutcome {
                summary: bounded_latest_text(&message),
                is_error: state != AgentLifecycleState::Completed,
            });
        }
        Some(snapshot.clone())
    }

    fn update_terminal_delegate(
        &self,
        id: &AgentId,
        state: AgentLifecycleState,
        update: AgentRunUpdate,
        is_error: bool,
    ) -> AgentSnapshot {
        let mut locked = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = locked
            .agents
            .get_mut(id.as_str())
            .expect("agent should exist");
        apply_terminal_delegate_update(snapshot, state, update, is_error)
    }

    #[must_use]
    pub fn complete_delegate_for_test(&self, id: &AgentId, summary: &str) -> AgentSnapshot {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = state
            .agents
            .get_mut(id.as_str())
            .expect("test agent should exist");
        let now = now_ms();
        snapshot.state = AgentLifecycleState::Completed;
        snapshot.terminal_at_ms.get_or_insert(now);
        snapshot.updated_at_ms = now;
        snapshot.terminal_reason = Some(AgentTerminalReason::Completed);
        snapshot.outcome = Some(AgentTerminalOutcome {
            summary: summary.to_owned(),
            is_error: false,
        });
        snapshot.clone()
    }

    #[must_use]
    pub fn snapshot(&self, id: &AgentId) -> Option<AgentSnapshot> {
        self.state
            .lock()
            .expect("multi-agent state poisoned")
            .agents
            .get(id.as_str())
            .cloned()
    }

    /// Flip a foreground agent to background mode. Returns the updated
    /// snapshot, or `None` if the agent doesn't exist.
    #[must_use]
    pub fn detach_agent(&self, id: &AgentId) -> Option<AgentSnapshot> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = state.agents.get_mut(id.as_str())?;
        snapshot.mode = AgentRunMode::Background;
        snapshot.detached_from_foreground = true;
        snapshot.updated_at_ms = now_ms();
        Some(snapshot.clone())
    }

    /// Flip a foreground swarm and all its children to background mode.
    /// Returns the updated snapshot, or `None` if the swarm doesn't exist.
    #[must_use]
    pub fn detach_swarm(&self, swarm_id: &str) -> Option<super::SwarmSnapshot> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let child_ids = state
            .swarms
            .get(swarm_id)?
            .children
            .iter()
            .map(|child| child.agent.id.as_str().to_owned())
            .collect::<Vec<_>>();
        let now = now_ms();
        for agent_id in &child_ids {
            if let Some(agent) = state.agents.get_mut(agent_id) {
                agent.mode = AgentRunMode::Background;
                agent.detached_from_foreground = true;
                agent.updated_at_ms = now;
            }
        }
        let mut snapshot = project_swarm_from_agents(&state, state.swarms.get(swarm_id)?);
        snapshot.mode = AgentRunMode::Background;
        for child in &mut snapshot.children {
            child.agent.mode = AgentRunMode::Background;
            child.agent.detached_from_foreground = true;
            child.agent.updated_at_ms = now;
        }
        state.swarms.insert(swarm_id.to_owned(), snapshot.clone());
        Some(snapshot)
    }

    /// Register a swarm snapshot in the runtime state.
    pub fn register_swarm(&self, snapshot: super::SwarmSnapshot) {
        let swarm_id = snapshot.swarm_id.clone();
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        state.register_swarm_order(&swarm_id);
        state.swarms.insert(swarm_id, snapshot);
    }

    pub fn restore_from_replay<'a>(&self, events: impl IntoIterator<Item = &'a AgentEvent>) {
        let mut restored_agent_ids = BTreeSet::new();
        let mut restored_swarm_ids = BTreeSet::new();
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        for event in events {
            match event {
                AgentEvent::DelegateStarted { agent, .. }
                | AgentEvent::DelegateUpdated { agent, .. }
                | AgentEvent::DelegateFinished { agent, .. } => {
                    restored_agent_ids.insert(agent.id.as_str().to_owned());
                    restore_agent_snapshot_locked(&mut state, agent.clone());
                }
                AgentEvent::DelegateProgressUpdated { progress, .. } => {
                    let agent_id = progress.agent_id.as_str().to_owned();
                    if let Some(agent) = state.agents.get_mut(&agent_id) {
                        restored_agent_ids.insert(agent_id);
                        let _ = apply_agent_progress(agent, progress);
                    }
                }
                AgentEvent::DelegateSwarmStarted { swarm, .. }
                | AgentEvent::DelegateSwarmUpdated { swarm, .. }
                | AgentEvent::DelegateSwarmFinished { swarm, .. } => {
                    restored_swarm_ids.insert(swarm.swarm_id.clone());
                    restored_agent_ids.extend(
                        swarm
                            .children
                            .iter()
                            .map(|child| child.agent.id.as_str().to_owned()),
                    );
                    restore_swarm_snapshot_locked(&mut state, swarm.clone());
                }
                AgentEvent::DelegateSwarmProgressUpdated {
                    swarm_id,
                    state: swarm_state,
                    aggregate,
                    child_progress,
                    ..
                } => {
                    let restored_agent = state.swarms.get_mut(swarm_id).and_then(|swarm| {
                        apply_swarm_child_progress(swarm, child_progress, *aggregate, *swarm_state)
                    });
                    if let Some(agent) = restored_agent {
                        restored_swarm_ids.insert(swarm_id.clone());
                        restored_agent_ids.insert(agent.id.as_str().to_owned());
                        let restored = restore_agent_snapshot_locked(&mut state, agent);
                        if let Some(swarm) = state.swarms.get_mut(swarm_id)
                            && let Some(child) = swarm
                                .children
                                .iter_mut()
                                .find(|child| child.agent.id.as_str() == restored.id.as_str())
                        {
                            child.agent = restored;
                        }
                    }
                }
                _ => {}
            }
        }
        mark_restored_running_agents_interrupted_locked(
            &mut state,
            &restored_agent_ids,
            &restored_swarm_ids,
        );
    }

    #[must_use]
    pub fn new_swarm_id(&self) -> String {
        loop {
            let id = format!("swarm_{}", Uuid::new_v4().simple());
            let state = self.state.lock().expect("multi-agent state poisoned");
            if !state.swarms.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mark a running agent as cancelled.
    ///
    /// Returns `None` and leaves the state unchanged if the agent is already
    /// terminal. Also cancels the agent's live cancellation token so any
    /// active model stream stops immediately.
    #[must_use]
    pub fn cancel_agent(&self, id: &AgentId) -> Option<AgentSnapshot> {
        self.cancel_agent_by_id(id.as_str())
    }

    /// Mark a running agent as cancelled by its string ID.
    ///
    /// Returns `None` and leaves the state unchanged if the agent is already
    /// terminal. Also cancels the agent's live cancellation token so any
    /// active model stream stops immediately.
    #[must_use]
    pub fn cancel_agent_by_id(&self, id: &str) -> Option<AgentSnapshot> {
        let (snapshot, token) = {
            let mut state = self.state.lock().expect("multi-agent state poisoned");
            let token = state
                .agent_cancel_tokens
                .get(id)
                .map(|entry| entry.token.clone());
            let snapshot = state.agents.get_mut(id)?;
            if snapshot.state.is_terminal() {
                return None;
            }
            let now = now_ms();
            snapshot.state = AgentLifecycleState::Cancelled;
            snapshot.terminal_at_ms.get_or_insert(now);
            snapshot.updated_at_ms = now;
            snapshot.terminal_reason = Some(AgentTerminalReason::CancelledByUser);
            snapshot.outcome = Some(AgentTerminalOutcome {
                summary: "Cancelled by user.".to_owned(),
                is_error: true,
            });
            (snapshot.clone(), token)
        };
        if let Some(token) = token {
            token.cancel();
        }
        Some(snapshot)
    }

    /// Mark every non-terminal child in a swarm as cancelled.
    ///
    /// Returns `None` and leaves the state unchanged if the swarm does not
    /// exist or all of its children are already terminal.
    #[must_use]
    pub fn cancel_swarm_by_id(&self, swarm_id: &str) -> Option<super::SwarmSnapshot> {
        let (snapshot, tokens) = {
            let mut state = self.state.lock().expect("multi-agent state poisoned");
            let mut snapshot = project_swarm_from_agents(&state, state.swarms.get(swarm_id)?);
            let mut changed = false;
            // Collect the child agent IDs that need cancelling.
            let cancelled_ids: Vec<String> = snapshot
                .children
                .iter()
                .filter(|child| !child.agent.state.is_terminal())
                .map(|child| child.agent.id.as_str().to_owned())
                .collect();
            for child in &mut snapshot.children {
                if child.agent.state.is_terminal() {
                    continue;
                }
                let now = now_ms();
                child.agent.state = AgentLifecycleState::Cancelled;
                child.agent.terminal_at_ms.get_or_insert(now);
                child.agent.updated_at_ms = now;
                child.agent.terminal_reason = Some(AgentTerminalReason::CancelledByUser);
                child.agent.outcome = Some(AgentTerminalOutcome {
                    summary: "Cancelled by user.".to_owned(),
                    is_error: true,
                });
                changed = true;
            }
            // Collect tokens before mutating agents to avoid borrow conflicts.
            let tokens = cancelled_ids
                .iter()
                .filter_map(|id| {
                    state
                        .agent_cancel_tokens
                        .get(id)
                        .map(|entry| entry.token.clone())
                })
                .collect::<Vec<_>>();
            for agent_id in &cancelled_ids {
                if let Some(agent) = state.agents.get_mut(agent_id) {
                    if agent.state.is_terminal() {
                        continue;
                    }
                    let now = now_ms();
                    agent.state = AgentLifecycleState::Cancelled;
                    agent.terminal_at_ms.get_or_insert(now);
                    agent.updated_at_ms = now;
                    agent.terminal_reason = Some(AgentTerminalReason::CancelledByUser);
                    agent.outcome = Some(AgentTerminalOutcome {
                        summary: "Cancelled by user.".to_owned(),
                        is_error: true,
                    });
                }
                // Sync the swarm child snapshot with the runtime agent.
                if let Some(agent) = state.agents.get(agent_id)
                    && let Some(child) = snapshot
                        .children
                        .iter_mut()
                        .find(|c| c.agent.id.as_str() == agent_id)
                {
                    child.agent = agent.clone();
                }
            }
            if !changed {
                return None;
            }
            refresh_swarm(&mut snapshot);
            state.register_swarm_order(swarm_id);
            state.swarms.insert(swarm_id.to_owned(), snapshot.clone());
            (snapshot, tokens)
        };
        for token in tokens {
            token.cancel();
        }
        Some(snapshot)
    }

    /// List all agent snapshots in the runtime, optionally including completed
    /// ones.
    #[must_use]
    pub fn list_agents(&self, include_completed: bool) -> Vec<AgentSnapshot> {
        let state = self.state.lock().expect("multi-agent state poisoned");
        state
            .agents
            .values()
            .filter(|agent| include_completed || !agent.state.is_terminal())
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn agent_created_index(&self, agent_id: &str) -> Option<u64> {
        self.state
            .lock()
            .expect("multi-agent state poisoned")
            .agent_order
            .get(agent_id)
            .copied()
    }

    #[must_use]
    pub fn swarm_created_index(&self, swarm_id: &str) -> Option<u64> {
        self.state
            .lock()
            .expect("multi-agent state poisoned")
            .swarm_order
            .get(swarm_id)
            .copied()
    }

    #[must_use]
    pub fn agent_snapshot(&self, agent_id: &str) -> Option<AgentSnapshot> {
        self.state
            .lock()
            .expect("multi-agent state poisoned")
            .agents
            .get(agent_id)
            .cloned()
    }

    pub fn start_resume_delegate(
        &self,
        agent_id: &str,
        request: &DelegateRequest,
    ) -> Result<AgentSnapshot, String> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let Some(agent) = state.agents.get_mut(agent_id) else {
            return Err(format!("unknown delegate target `{agent_id}`"));
        };
        if matches!(
            agent.state,
            AgentLifecycleState::Queued | AgentLifecycleState::Running
        ) {
            return Err(
                "agent is already running; use MessageDelegate for live follow-up".to_owned(),
            );
        }
        let previous_status = agent.state;
        if previous_status.is_terminal() {
            agent.terminal_status_history.push(previous_status);
        }
        agent.state = AgentLifecycleState::Running;
        agent.mode = request.mode;
        agent.context = request.context;
        agent.task.clone_from(&request.task);
        agent.task_title = derive_title(&request.task, request.title.as_deref());
        agent.run_count = agent.run_count.saturating_add(1);
        agent.live_messages_received = 0;
        agent.previous_status = Some(previous_status);
        agent.resumed_from = Some(AgentId::from_existing(agent_id));
        agent.elapsed = Duration::ZERO;
        agent.latest_text = None;
        agent.activity.clear();
        agent.outcome = None;
        let now = now_ms();
        agent.started_at_ms = Some(now);
        agent.terminal_at_ms = None;
        agent.terminal_reason = None;
        agent.updated_at_ms = now;
        let agent = agent.clone();
        state.retry_activity_starts.insert(agent_id.to_owned(), 0);
        Ok(agent)
    }

    pub fn deliver_live_agent_message(
        &self,
        agent_id: &str,
        message: String,
    ) -> Result<(), String> {
        let Some(agent) = self.agent_snapshot(agent_id) else {
            return Err(format!("unknown delegate target `{agent_id}`"));
        };
        if !matches!(agent.state, AgentLifecycleState::Running) {
            return Err(format!(
                "agent already {}; terminal agents cannot receive live messages. To continue this agent, call Delegate with resume=\"{}\".",
                agent.state.as_str(),
                agent.id.as_str()
            ));
        }
        let mailbox_message = super::DelegateMailboxMessage {
            id: format!("live_{}", uuid::Uuid::new_v4().simple()),
            text: message,
            delivered: false,
        };
        if self.deliver_live_message(agent_id, &mailbox_message) {
            self.record_live_message(agent_id);
            Ok(())
        } else {
            Err(format!(
                "agent is not running; use Delegate with resume=\"{}\" to continue it",
                agent.id.as_str()
            ))
        }
    }

    #[must_use]
    pub fn deliver_live_message(
        &self,
        agent_id: &str,
        message: &super::DelegateMailboxMessage,
    ) -> bool {
        let handle = self
            .state
            .lock()
            .expect("multi-agent state poisoned")
            .steer_handles
            .get(agent_id)
            .cloned();
        let Some(handle) = handle else {
            return false;
        };
        handle.push(ActiveTurnInput::SteerNow(AgentMessage::user_text(format!(
            "Delegate message {}:\n{}",
            message.id, message.text
        ))));
        true
    }

    fn record_live_message(&self, agent_id: &str) {
        if let Some(agent) = self
            .state
            .lock()
            .expect("multi-agent state poisoned")
            .agents
            .get_mut(agent_id)
        {
            agent.live_messages_received = agent.live_messages_received.saturating_add(1);
            agent.updated_at_ms = now_ms();
        }
    }

    /// Return the item indices of children that can be resumed (queued, failed,
    /// or cancelled). Completed and running children are skipped.
    #[must_use]
    pub fn resumable_swarm_items(&self, swarm_id: &str) -> Vec<usize> {
        let state = self.state.lock().expect("multi-agent state poisoned");
        let Some(swarm) = state
            .swarms
            .get(swarm_id)
            .map(|swarm| project_swarm_from_agents(&state, swarm))
        else {
            return Vec::new();
        };
        swarm
            .children
            .iter()
            .filter(|child| {
                matches!(
                    child.agent.state,
                    AgentLifecycleState::Queued
                        | AgentLifecycleState::Failed
                        | AgentLifecycleState::Cancelled
                        | AgentLifecycleState::TimedOut
                )
            })
            .map(|child| child.item_index)
            .collect()
    }

    /// Create a test swarm with the given children states. Returns the swarm ID.
    #[must_use]
    pub fn create_swarm_for_test(&self, children: Vec<(&str, AgentLifecycleState)>) -> String {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let swarm_id = format!("swarm-test-{}", state.swarms.len());
        let child_snapshots: Vec<_> = children
            .into_iter()
            .enumerate()
            .map(|(index, (item, lifecycle_state))| {
                let name = state.names.next_name();
                super::SwarmChildSnapshot {
                    item_index: index,
                    item: item.to_owned(),
                    agent: new_agent_snapshot(AgentSnapshotSeed {
                        id: AgentId::from_suffix_for_test(&format!("{swarm_id}-{index}")),
                        display_name: name.clone(),
                        path: AgentPath::swarm_child(&swarm_id, &name),
                        role: AgentRole::Coder,
                        mode: AgentRunMode::Foreground,
                        context: DelegateContext::None,
                        state: lifecycle_state,
                        task: item,
                        title: None,
                    }),
                }
            })
            .collect();
        let aggregate =
            SwarmAggregate::from_states(child_snapshots.iter().map(|child| child.agent.state));
        let swarm = super::SwarmSnapshot {
            swarm_id: swarm_id.clone(),
            description: "test swarm".to_owned(),
            role: AgentRole::Coder,
            mode: AgentRunMode::Foreground,
            state: AgentLifecycleState::Running,
            max_concurrency: child_snapshots.len().max(1),
            aggregate,
            children: child_snapshots,
        };
        state.register_swarm_order(&swarm_id);
        state.swarms.insert(swarm_id.clone(), swarm);
        swarm_id
    }

    /// Look up a swarm snapshot by id.
    #[must_use]
    pub fn swarm_snapshot(&self, swarm_id: &str) -> Option<super::SwarmSnapshot> {
        let state = self.state.lock().expect("multi-agent state poisoned");
        state
            .swarms
            .get(swarm_id)
            .map(|swarm| project_swarm_from_agents(&state, swarm))
    }

    /// List all swarm snapshots in the runtime.
    #[must_use]
    pub fn list_swarms(&self) -> Vec<super::SwarmSnapshot> {
        let state = self.state.lock().expect("multi-agent state poisoned");
        state
            .swarms
            .values()
            .map(|swarm| project_swarm_from_agents(&state, swarm))
            .collect()
    }

    /// Cancel all non-terminal children in a swarm.
    ///
    /// Returns the refreshed snapshot, or an error message if the swarm is
    /// unknown or already terminal.
    pub fn cancel_swarm(&self, swarm_id: &str) -> Result<super::SwarmSnapshot, String> {
        let (swarm_snapshot, tokens) = {
            let mut state = self.state.lock().expect("multi-agent state poisoned");
            let mut snapshot = project_swarm_from_agents(
                &state,
                state
                    .swarms
                    .get(swarm_id)
                    .ok_or_else(|| format!("unknown delegate target `{swarm_id}`"))?,
            );
            if snapshot.state.is_terminal() {
                return Err(format!(
                    "swarm already {}; terminal swarm state is immutable",
                    snapshot.state.as_str()
                ));
            }
            // Collect the child agent ids that need cancelling before borrowing
            // state.agents separately.
            let cancelled_ids: Vec<String> = snapshot
                .children
                .iter()
                .filter(|child| !child.agent.state.is_terminal())
                .map(|child| child.agent.id.as_str().to_owned())
                .collect();
            for child in &mut snapshot.children {
                if !child.agent.state.is_terminal() {
                    let now = now_ms();
                    child.agent.state = AgentLifecycleState::Cancelled;
                    child.agent.terminal_at_ms.get_or_insert(now);
                    child.agent.updated_at_ms = now;
                    child.agent.terminal_reason = Some(AgentTerminalReason::CancelledByUser);
                    child.agent.outcome = Some(AgentTerminalOutcome {
                        summary: "Cancelled by user.".to_owned(),
                        is_error: true,
                    });
                }
            }
            let mut tokens = Vec::new();
            for agent_id in &cancelled_ids {
                // Collect token before mutable borrow of agents.
                if let Some(token) = state.agent_cancel_tokens.get(agent_id) {
                    tokens.push(token.token.clone());
                }
                if let Some(agent) = state.agents.get_mut(agent_id) {
                    if agent.state.is_terminal() {
                        continue;
                    }
                    let now = now_ms();
                    agent.state = AgentLifecycleState::Cancelled;
                    agent.terminal_at_ms.get_or_insert(now);
                    agent.updated_at_ms = now;
                    agent.terminal_reason = Some(AgentTerminalReason::CancelledByUser);
                    agent.outcome = Some(AgentTerminalOutcome {
                        summary: "Cancelled by user.".to_owned(),
                        is_error: true,
                    });
                }
            }
            snapshot = project_swarm_from_agents(&state, &snapshot);
            state.swarms.insert(swarm_id.to_owned(), snapshot.clone());
            (snapshot, tokens)
        };
        for token in tokens {
            token.cancel();
        }
        Ok(swarm_snapshot)
    }

    /// Broadcast a live message to all running children in a swarm.
    ///
    /// Returns `(delivered, skipped)` on success, or an error if the swarm is
    /// unknown. When no children received the message (all terminal), the
    /// result is still `Ok` — the caller decides how to present an
    /// all-skipped outcome.
    pub fn broadcast_live_swarm_message(
        &self,
        swarm_id: &str,
        message: &str,
    ) -> LiveSwarmMessageResult {
        let Some(swarm) = self.swarm_snapshot(swarm_id) else {
            return Err(format!("unknown delegate target `{swarm_id}`"));
        };
        let mut delivered = Vec::new();
        let mut skipped = Vec::new();
        for child in &swarm.children {
            if child.agent.state == AgentLifecycleState::Running {
                let mailbox_message = super::DelegateMailboxMessage {
                    id: format!("live_{}", uuid::Uuid::new_v4().simple()),
                    text: message.to_owned(),
                    delivered: false,
                };
                if self.deliver_live_message(child.agent.id.as_str(), &mailbox_message) {
                    delivered.push(child.agent.id.as_str().to_owned());
                    self.record_live_message(child.agent.id.as_str());
                } else {
                    skipped.push((child.agent.id.as_str().to_owned(), child.agent.state));
                }
            } else {
                skipped.push((child.agent.id.as_str().to_owned(), child.agent.state));
            }
        }
        Ok((delivered, skipped))
    }
}

/// Refresh the aggregate and state of a swarm snapshot from its children.
fn refresh_swarm(snapshot: &mut super::SwarmSnapshot) {
    snapshot.aggregate =
        SwarmAggregate::from_states(snapshot.children.iter().map(|child| child.agent.state));
    snapshot.state = snapshot.aggregate.status();
}

/// Sync projected swarm children from the canonical `state.agents` map.
fn sync_swarm_children_from_agents(state: &MultiAgentState, snapshot: &mut super::SwarmSnapshot) {
    for child in &mut snapshot.children {
        if let Some(agent) = state.agents.get(child.agent.id.as_str()) {
            child.agent = agent.clone();
        }
    }
}

fn project_swarm_from_agents(
    state: &MultiAgentState,
    snapshot: &super::SwarmSnapshot,
) -> super::SwarmSnapshot {
    let mut projected = snapshot.clone();
    sync_swarm_children_from_agents(state, &mut projected);
    refresh_swarm(&mut projected);
    projected
}

#[derive(Debug, Clone, Copy)]
pub enum AgentPathKind<'a> {
    Root,
    SwarmChild(&'a str),
}

#[derive(Debug, Clone)]
pub struct AgentRunUpdate {
    pub summary: String,
    pub tool_count: usize,
    pub token_count: usize,
    pub cache_read_token_count: usize,
    pub cache_write_token_count: usize,
    pub elapsed: Duration,
    pub latest_text: Option<String>,
    pub activity: Vec<AgentActivityEntry>,
}

#[derive(Debug, Clone)]
pub struct ChildRunOutput {
    pub snapshot: AgentSnapshot,
    pub events: Vec<AgentEvent>,
    pub messages: Vec<AgentMessage>,
}

type LiveSwarmMessageResult = Result<(Vec<String>, Vec<(String, AgentLifecycleState)>), String>;

fn restore_agent_snapshot_locked(
    state: &mut MultiAgentState,
    snapshot: AgentSnapshot,
) -> AgentSnapshot {
    let agent_id = snapshot.id.as_str().to_owned();
    state.register_agent_order(&agent_id);
    match state.agents.get(&agent_id) {
        Some(existing) if existing.updated_at_ms > snapshot.updated_at_ms => existing.clone(),
        _ => {
            state.agents.insert(agent_id, snapshot.clone());
            snapshot
        }
    }
}

fn restore_swarm_snapshot_locked(state: &mut MultiAgentState, snapshot: super::SwarmSnapshot) {
    let swarm_id = snapshot.swarm_id.clone();
    state.register_swarm_order(&swarm_id);
    let mut restored = snapshot;
    for child in &mut restored.children {
        child.agent = restore_agent_snapshot_locked(state, child.agent.clone());
    }
    refresh_swarm(&mut restored);
    state.swarms.insert(swarm_id, restored);
}

fn mark_restored_running_agents_interrupted_locked(
    state: &mut MultiAgentState,
    agent_ids: &BTreeSet<String>,
    swarm_ids: &BTreeSet<String>,
) {
    let now = now_ms();
    for agent_id in agent_ids {
        if let Some(snapshot) = state.agents.get_mut(agent_id) {
            mark_restored_snapshot_interrupted(snapshot, now);
        }
    }
    for swarm_id in swarm_ids {
        let Some(swarm) = state.swarms.get_mut(swarm_id) else {
            continue;
        };
        for child in &mut swarm.children {
            if let Some(agent) = state.agents.get(child.agent.id.as_str()) {
                child.agent = agent.clone();
            } else {
                mark_restored_snapshot_interrupted(&mut child.agent, now);
            }
        }
        refresh_swarm(swarm);
    }
}

fn mark_restored_snapshot_interrupted(snapshot: &mut AgentSnapshot, now: u64) {
    if !matches!(
        snapshot.state,
        AgentLifecycleState::Queued | AgentLifecycleState::Running
    ) {
        return;
    }
    snapshot.state = AgentLifecycleState::Interrupted;
    snapshot.terminal_reason = Some(AgentTerminalReason::ProcessExited);
    snapshot.terminal_at_ms.get_or_insert(now);
    snapshot.updated_at_ms = now;
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: format!(
            "Interrupted because the previous Neo process exited. \
             Resume with Delegate(resume=\"{}\", task=\"continue\").",
            snapshot.id.as_str()
        ),
        is_error: true,
    });
}

struct AgentSnapshotSeed<'a> {
    id: AgentId,
    display_name: AgentDisplayName,
    path: AgentPath,
    role: AgentRole,
    mode: AgentRunMode,
    context: DelegateContext,
    state: AgentLifecycleState,
    task: &'a str,
    title: Option<&'a str>,
}

fn new_agent_snapshot(seed: AgentSnapshotSeed<'_>) -> AgentSnapshot {
    let now = now_ms();
    let terminal_reason = seed
        .state
        .is_terminal()
        .then(|| terminal_reason_for_state(seed.state));
    AgentSnapshot {
        id: seed.id,
        display_name: seed.display_name,
        path: seed.path,
        role: seed.role,
        mode: seed.mode,
        context: seed.context,
        state: seed.state,
        task: seed.task.to_owned(),
        task_title: derive_title(seed.task, seed.title),
        created_at_ms: now,
        updated_at_ms: now,
        started_at_ms: (seed.state == AgentLifecycleState::Running).then_some(now),
        terminal_at_ms: seed.state.is_terminal().then_some(now),
        detached_from_foreground: false,
        terminal_reason,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 0,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::ZERO,
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

const fn terminal_reason_for_state(state: AgentLifecycleState) -> AgentTerminalReason {
    match state {
        AgentLifecycleState::Completed => AgentTerminalReason::Completed,
        AgentLifecycleState::Failed
        | AgentLifecycleState::Queued
        | AgentLifecycleState::Running => AgentTerminalReason::Error,
        AgentLifecycleState::Cancelled => AgentTerminalReason::CancelledByUser,
        AgentLifecycleState::TimedOut => AgentTerminalReason::TimedOut,
        AgentLifecycleState::Interrupted => AgentTerminalReason::ProcessExited,
    }
}

#[derive(Clone)]
pub struct ChildRuntimeDeps {
    pub config: AgentConfig,
    pub model: Arc<dyn ModelClient>,
    pub tools: Arc<ToolRegistry>,
    pub role: AgentRole,
    pub cancel_token: CancellationToken,
    /// Snapshot of the parent agent's visible instruction state, used to
    /// seed full-context child instruction baselines. `None` behaves like an
    /// empty parent (plain global/workspace baseline).
    pub parent_instruction_state: Option<AgentInstructionState>,
}

impl ChildRuntimeDeps {
    #[must_use]
    pub fn new(config: AgentConfig, model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            config,
            model,
            tools,
            role: AgentRole::Coder,
            cancel_token: CancellationToken::new(),
            parent_instruction_state: None,
        }
    }

    /// Set the subagent role for tool filtering and profile enforcement.
    #[must_use]
    pub fn with_role(mut self, role: AgentRole) -> Self {
        self.role = role;
        self
    }

    /// Set the parent cancellation token for foreground child runs.
    #[must_use]
    pub fn with_cancel_token(mut self, cancel_token: CancellationToken) -> Self {
        self.cancel_token = cancel_token;
        self
    }

    /// Set the parent agent's visible instruction state snapshot used to
    /// seed full-context child instruction baselines.
    #[must_use]
    pub fn with_parent_instruction_state(mut self, state: AgentInstructionState) -> Self {
        self.parent_instruction_state = Some(state);
        self
    }
}

impl MultiAgentRuntime {
    pub async fn run_child_turn(
        &self,
        deps: ChildRuntimeDeps,
        request: &DelegateRequest,
        mode: AgentRunMode,
    ) -> Result<ChildRunOutput, String> {
        let snapshot = self.start_delegate(
            &request.task,
            request.title.as_deref(),
            request.actual_role(),
            mode,
            request.context,
            AgentPathKind::Root,
        );
        Ok(self
            .run_started_child_turn(deps, snapshot, request.context, |_| {})
            .await)
    }

    pub async fn run_started_child_turn<F>(
        &self,
        deps: ChildRuntimeDeps,
        snapshot: AgentSnapshot,
        context: DelegateContext,
        mut on_update: F,
    ) -> ChildRunOutput
    where
        F: FnMut(AgentSnapshot) + Send,
    {
        let started_at = Instant::now();
        let snapshot = self.mark_delegate_running(&snapshot.id).unwrap_or(snapshot);
        on_update(snapshot.clone());
        // Short-circuit if the child was already cancelled before this turn
        // started (e.g. queued swarm child cancelled by cancel_swarm).
        if snapshot.state.is_terminal() {
            return ChildRunOutput {
                snapshot,
                events: Vec::new(),
                messages: Vec::new(),
            };
        }
        if let Err(error) = self.register_persistent_agent(&snapshot, None, None).await {
            return self.finish_child_run(snapshot, started_at, Err(error));
        }
        let prompt = child_prompt(&snapshot.task, context, snapshot.role);
        let prior_context = match self.replay_child_context(&snapshot).await {
            Ok(context) => context,
            Err(error) => return self.finish_child_run(snapshot, started_at, Err(error)),
        };
        let runtime = self.clone();
        let agent_id = snapshot.id.clone();
        let live_cancel = self.register_live_cancel(agent_id.as_str(), &deps.cancel_token);
        let mut deps = deps.with_cancel_token(live_cancel.token());
        deps.config.instruction_inheritance = instruction_inheritance_for(context);
        let live_steer = self.register_live_steer(agent_id.as_str());
        let child_wire_path = self.child_wire_path(agent_id.as_str());
        let run = run_agent_snapshot(
            deps,
            prompt,
            prior_context,
            live_steer.handle(),
            agent_id.as_str().to_owned(),
            child_wire_path,
            |event| {
                if runtime
                    .apply_child_event(&agent_id, started_at, event)
                    .is_some()
                    && let Some(updated) = runtime.agent_snapshot(agent_id.as_str())
                {
                    on_update(updated);
                }
            },
        )
        .await;
        drop(live_cancel);
        self.finish_child_run(snapshot, started_at, run)
    }

    pub async fn run_swarm_child_turn(
        &self,
        deps: ChildRuntimeDeps,
        request: &DelegateSwarmRequest,
        swarm_id: &str,
        item: &str,
        mode: AgentRunMode,
    ) -> Result<ChildRunOutput, String> {
        let task = swarm_child_task(request.prompt_template.as_deref().unwrap_or(""), item);
        let snapshot = self.start_delegate(
            &task,
            None,
            request.role,
            mode,
            DelegateContext::None,
            AgentPathKind::SwarmChild(swarm_id),
        );
        Ok(self
            .run_started_swarm_child_turn(
                deps,
                snapshot,
                swarm_id,
                item,
                DelegateContext::None,
                |_| {},
            )
            .await)
    }

    pub async fn run_started_swarm_child_turn<F>(
        &self,
        deps: ChildRuntimeDeps,
        snapshot: AgentSnapshot,
        swarm_id: &str,
        swarm_item: &str,
        context: DelegateContext,
        mut on_update: F,
    ) -> ChildRunOutput
    where
        F: FnMut(AgentProgressSnapshot) + Send,
    {
        let started_at = Instant::now();
        let snapshot = self.mark_delegate_running(&snapshot.id).unwrap_or(snapshot);
        on_update(snapshot.progress_snapshot());
        // Short-circuit if the child was already cancelled before this turn
        // started (e.g. queued swarm child cancelled by cancel_swarm).
        if snapshot.state.is_terminal() {
            return ChildRunOutput {
                snapshot,
                events: Vec::new(),
                messages: Vec::new(),
            };
        }
        if let Err(error) = self
            .register_persistent_agent(&snapshot, Some(swarm_id), Some(swarm_item))
            .await
        {
            return self.finish_child_run(snapshot, started_at, Err(error));
        }
        let prompt = child_prompt(&snapshot.task, context, snapshot.role);
        let prior_context = match self.replay_child_context(&snapshot).await {
            Ok(context) => context,
            Err(error) => return self.finish_child_run(snapshot, started_at, Err(error)),
        };
        let runtime = self.clone();
        let agent_id = snapshot.id.clone();
        let live_cancel = self.register_live_cancel(agent_id.as_str(), &deps.cancel_token);
        let mut deps = deps.with_cancel_token(live_cancel.token());
        deps.config.instruction_inheritance = instruction_inheritance_for(context);
        let live_steer = self.register_live_steer(agent_id.as_str());
        let child_wire_path = self.child_wire_path(agent_id.as_str());
        let run = run_agent_snapshot(
            deps,
            prompt,
            prior_context,
            live_steer.handle(),
            agent_id.as_str().to_owned(),
            child_wire_path,
            |event| {
                if let Some(updated) = runtime.apply_child_event(&agent_id, started_at, event) {
                    on_update(updated);
                }
            },
        )
        .await;
        drop(live_cancel);
        self.finish_child_run(snapshot, started_at, run)
    }

    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn apply_child_event(
        &self,
        id: &AgentId,
        started_at: Instant,
        event: &AgentEvent,
    ) -> Option<AgentProgressSnapshot> {
        let mut locked = self.state.lock().expect("multi-agent state poisoned");
        let MultiAgentState {
            agents,
            retry_activity_starts,
            ..
        } = &mut *locked;
        let snapshot = agents.get_mut(id.as_str())?;
        // Ignore late buffered events after the agent has reached a terminal
        // state (e.g. cancelled by InterruptDelegate). This prevents a
        // cancelled child from looking active again.
        if snapshot.state.is_terminal() {
            return None;
        }
        snapshot.elapsed = started_at.elapsed();
        snapshot.updated_at_ms = now_ms();
        let attempt_start = retry_activity_starts
            .entry(id.as_str().to_owned())
            .or_insert(snapshot.activity.len());
        let mut changed = false;
        match event {
            AgentEvent::RetryScheduled { .. }
            | AgentEvent::RetryStarted { .. }
            | AgentEvent::RetryResumed { .. }
            | AgentEvent::RetryExhausted { .. }
            | AgentEvent::Error { .. }
            | AgentEvent::TurnFinished {
                stop_reason: StopReason::Cancelled,
                ..
            }
            | AgentEvent::RunFinished {
                stop_reason: StopReason::Cancelled,
                ..
            } => {
                if let Some((retry_changed, latest_text)) =
                    apply_retry_activity(&mut snapshot.activity, attempt_start, event)
                {
                    changed = retry_changed;
                    if retry_changed {
                        snapshot.latest_text = latest_text;
                    }
                }
            }
            AgentEvent::ToolExecutionQueued {
                id,
                name,
                arguments,
                ..
            } => {
                changed = upsert_queued_tool_activity(
                    &mut snapshot.activity,
                    id,
                    name,
                    summarize_tool_arguments(name, arguments),
                    now_ms(),
                );
            }
            AgentEvent::ToolExecutionQueueUpdated {
                id,
                position,
                waiting_ms,
                ..
            } => {
                changed = update_queued_tool_activity(
                    &mut snapshot.activity,
                    id,
                    *position,
                    now_ms().saturating_sub(*waiting_ms),
                );
            }
            AgentEvent::ToolExecutionStarted {
                id,
                name,
                arguments,
                ..
            } => {
                changed = true;
                upsert_tool_activity(
                    &mut snapshot.activity,
                    id,
                    name,
                    summarize_tool_arguments(name, arguments),
                    AgentToolActivityPhase::Ongoing,
                    None,
                );
            }
            AgentEvent::ToolExecutionFinished {
                id, name, result, ..
            } => {
                changed = true;
                snapshot.tool_count = snapshot.tool_count.saturating_add(1);
                let phase = if result.is_error {
                    AgentToolActivityPhase::Failed
                } else {
                    AgentToolActivityPhase::Done
                };
                let summary = result
                    .details
                    .as_ref()
                    .and_then(summarize_edit_details)
                    .or_else(|| last_tool_summary(snapshot.activity.as_slice(), id));
                upsert_tool_activity(
                    &mut snapshot.activity,
                    id,
                    name,
                    summary,
                    phase,
                    tool_output_preview(name, result, false),
                );
            }
            AgentEvent::ToolExecutionUpdate {
                id,
                name,
                partial_result,
                ..
            } => {
                changed = true;
                let summary = partial_result
                    .details
                    .as_ref()
                    .and_then(summarize_edit_details)
                    .or_else(|| last_tool_summary(snapshot.activity.as_slice(), id));
                upsert_tool_activity(
                    &mut snapshot.activity,
                    id,
                    name,
                    summary,
                    AgentToolActivityPhase::Ongoing,
                    tool_output_preview(name, partial_result, true),
                );
            }
            AgentEvent::TextDelta { text, .. } => {
                changed = true;
                push_text_activity(snapshot, *attempt_start, text, false);
            }
            AgentEvent::ThinkingDelta { text, .. } => {
                changed = true;
                push_text_activity(snapshot, *attempt_start, text, true);
            }
            AgentEvent::MessageAppended {
                message: AgentMessage::Assistant { content, .. },
            } => {
                let text = content_text(content);
                if !text.trim().is_empty() {
                    changed = true;
                    let canonical_text = bounded_latest_text(&text);
                    snapshot.latest_text = Some(canonical_text.clone());
                    if latest_text_activity(snapshot.activity.as_slice(), false).as_deref()
                        != Some(canonical_text.as_str())
                    {
                        push_text_activity(snapshot, *attempt_start, &text, false);
                    }
                }
            }
            AgentEvent::TokenUsage { usage, .. } => {
                changed = true;
                snapshot.token_count = snapshot
                    .token_count
                    .saturating_add((usage.input_tokens + usage.output_tokens) as usize);
                snapshot.cache_read_token_count = snapshot
                    .cache_read_token_count
                    .saturating_add(usage.input_cache_read_tokens as usize);
                snapshot.cache_write_token_count = snapshot
                    .cache_write_token_count
                    .saturating_add(usage.input_cache_write_tokens as usize);
            }
            _ => {}
        }
        if matches!(
            event,
            AgentEvent::MessageAppended {
                message: AgentMessage::Assistant { .. }
            }
        ) {
            *attempt_start = snapshot.activity.len();
        }
        if !changed {
            return None;
        }
        *attempt_start = attempt_start.saturating_sub(trim_activity(&mut snapshot.activity));
        Some(snapshot.progress_snapshot())
    }

    fn finish_child_run(
        &self,
        snapshot: AgentSnapshot,
        started_at: Instant,
        run: Result<(Vec<AgentEvent>, Vec<AgentMessage>), String>,
    ) -> ChildRunOutput {
        match run {
            Ok((events, messages)) => {
                let mut update = summarize_child_events(&events, started_at.elapsed());
                let (state, is_error) = if child_events_were_cancelled(&events) {
                    (AgentLifecycleState::Cancelled, true)
                } else if child_events_have_error(&events) {
                    (AgentLifecycleState::Failed, true)
                } else {
                    update.latest_text = update.latest_text.as_deref().map(bounded_latest_text);
                    update.summary = bounded_latest_text(&update.summary);
                    (AgentLifecycleState::Completed, false)
                };
                let completed = self.finalize_child_run_with_messages(
                    &snapshot.id,
                    state,
                    update,
                    is_error,
                    &messages,
                );
                ChildRunOutput {
                    snapshot: completed,
                    events,
                    messages,
                }
            }
            Err(error) => {
                let messages = snapshot.prior_messages.clone();
                let update = AgentRunUpdate {
                    summary: error,
                    tool_count: 0,
                    token_count: 0,
                    cache_read_token_count: 0,
                    cache_write_token_count: 0,
                    elapsed: started_at.elapsed(),
                    latest_text: None,
                    activity: Vec::new(),
                };
                let failed = self.finalize_child_run_with_messages(
                    &snapshot.id,
                    AgentLifecycleState::Failed,
                    update,
                    true,
                    &messages,
                );
                ChildRunOutput {
                    snapshot: failed,
                    events: Vec::new(),
                    messages,
                }
            }
        }
    }

    fn finalize_child_run_with_messages(
        &self,
        id: &AgentId,
        terminal_state: AgentLifecycleState,
        update: AgentRunUpdate,
        is_error: bool,
        messages: &[AgentMessage],
    ) -> AgentSnapshot {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = state
            .agents
            .get_mut(id.as_str())
            .expect("agent should exist");
        snapshot.prior_messages = messages.to_vec();
        if snapshot.state == AgentLifecycleState::Cancelled {
            return snapshot.clone();
        }
        apply_terminal_delegate_update(snapshot, terminal_state, update, is_error)
    }

    fn register_live_steer(&self, agent_id: &str) -> LiveSteerRegistration {
        let handle = SteerInputHandle::new();
        self.state
            .lock()
            .expect("multi-agent state poisoned")
            .steer_handles
            .insert(agent_id.to_owned(), handle.clone());
        LiveSteerRegistration {
            runtime: self.clone(),
            agent_id: agent_id.to_owned(),
            handle,
        }
    }

    /// Register a live cancellation token for a child agent, linked to the
    /// parent turn's cancel token. When the parent token fires, the child
    /// token is cancelled too. Returns a guard whose Drop unregisters the
    /// token.
    fn register_live_cancel(
        &self,
        agent_id: &str,
        parent_token: &CancellationToken,
    ) -> LiveCancelRegistration {
        let token = CancellationToken::new();
        if parent_token.is_cancelled() {
            token.cancel();
        }
        let generation = {
            let mut state = self.state.lock().expect("multi-agent state poisoned");
            let generation = state.next_cancel_generation();
            state.agent_cancel_tokens.insert(
                agent_id.to_owned(),
                LiveAgentCancel {
                    token: token.clone(),
                    generation,
                },
            );
            generation
        };
        let bridge_child = token.clone();
        let bridge_parent = parent_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = bridge_parent.cancelled() => bridge_child.cancel(),
                () = bridge_child.cancelled() => {}
            }
        });
        LiveCancelRegistration {
            runtime: self.clone(),
            agent_id: agent_id.to_owned(),
            generation,
            token,
        }
    }

    fn child_wire_path(&self, agent_id: &str) -> Option<PathBuf> {
        self.session_directory
            .as_ref()
            .map(|session_dir| crate::session::agent_wire_path(session_dir, agent_id))
    }

    /// Replay the child's prior context (messages plus instruction
    /// visibility state) from its wire JSONL.
    async fn replay_child_context(&self, snapshot: &AgentSnapshot) -> Result<AgentContext, String> {
        let fallback = || {
            let mut context = AgentContext::new();
            for message in &snapshot.prior_messages {
                context.append_message(message.clone());
            }
            context
        };
        let Some(wire_path) = self.child_wire_path(snapshot.id.as_str()) else {
            return Ok(fallback());
        };
        match crate::session::JsonlSessionReader::replay_context(&wire_path).await {
            Ok(context) => Ok(context),
            Err(crate::session::SessionError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound
                    && snapshot.run_count == 1
                    && snapshot.resumed_from.is_none() =>
            {
                Ok(fallback())
            }
            Err(error) => Err(format!(
                "failed to replay delegate `{}` from {}: {error}",
                snapshot.id.as_str(),
                wire_path.display()
            )),
        }
    }

    async fn register_persistent_agent(
        &self,
        snapshot: &AgentSnapshot,
        swarm_id: Option<&str>,
        swarm_item: Option<&str>,
    ) -> Result<(), String> {
        let Some(session_dir) = &self.session_directory else {
            return Ok(());
        };
        let store = crate::session::SessionStateStore::new(session_dir);
        let _guard = self.session_state_update_lock.lock().await;
        let mut state = store.read().await.map_err(|err| err.to_string())?;
        state.upsert_agent(crate::session::SessionAgentRecord {
            kind: crate::session::SessionAgentKind::Sub,
            record_dir: crate::session::relative_agent_record_dir(snapshot.id.as_str()),
            parent_agent_id: Some(crate::session::MAIN_AGENT_ID.to_owned()),
            role: Some(snapshot.role.as_str().to_owned()),
            swarm_id: swarm_id.map(str::to_owned),
            swarm_item: swarm_item.map(str::to_owned),
        });
        store.write(&state).map_err(|err| err.to_string())
    }
}

fn apply_terminal_delegate_update(
    snapshot: &mut AgentSnapshot,
    state: AgentLifecycleState,
    update: AgentRunUpdate,
    is_error: bool,
) -> AgentSnapshot {
    let now = now_ms();
    snapshot.state = state;
    snapshot.tool_count = update.tool_count;
    snapshot.token_count = update.token_count;
    snapshot.cache_read_token_count = update.cache_read_token_count;
    snapshot.cache_write_token_count = update.cache_write_token_count;
    snapshot.elapsed = update.elapsed;
    snapshot.latest_text = update.latest_text;
    snapshot.activity = update.activity;
    snapshot.terminal_at_ms.get_or_insert(now);
    snapshot.updated_at_ms = now;
    snapshot.terminal_reason = Some(terminal_reason_for_state(state));
    snapshot.outcome = Some(AgentTerminalOutcome {
        summary: update.summary,
        is_error,
    });
    snapshot.clone()
}

struct LiveSteerRegistration {
    runtime: MultiAgentRuntime,
    agent_id: String,
    handle: SteerInputHandle,
}

impl LiveSteerRegistration {
    fn handle(&self) -> SteerInputHandle {
        self.handle.clone()
    }
}

impl Drop for LiveSteerRegistration {
    fn drop(&mut self) {
        self.runtime
            .state
            .lock()
            .expect("multi-agent state poisoned")
            .steer_handles
            .remove(&self.agent_id);
    }
}

/// Guard that unregister a live cancellation token when the child run
/// finishes. The token is cloned into `ChildRuntimeDeps` so the child
/// stream observes cancellation immediately.
struct LiveCancelRegistration {
    runtime: MultiAgentRuntime,
    agent_id: String,
    generation: u64,
    token: CancellationToken,
}

impl LiveCancelRegistration {
    fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Drop for LiveCancelRegistration {
    fn drop(&mut self) {
        self.token.cancel();
        let mut state = self
            .runtime
            .state
            .lock()
            .expect("multi-agent state poisoned");
        if state
            .agent_cancel_tokens
            .get(&self.agent_id)
            .is_some_and(|entry| entry.generation == self.generation)
        {
            state.agent_cancel_tokens.remove(&self.agent_id);
        }
    }
}

#[derive(Debug, Clone)]
struct LiveAgentCancel {
    token: CancellationToken,
    generation: u64,
}

/// Maps the delegate context mode to instruction inheritance. `inherit`
/// may explicitly copy revisions already represented in inherited full
/// messages; `summary` and `none` never infer visibility from prose.
fn instruction_inheritance_for(context: DelegateContext) -> InstructionInheritance {
    match context {
        DelegateContext::Inherit => InstructionInheritance::FullContext,
        DelegateContext::Summary | DelegateContext::None => InstructionInheritance::Summary,
    }
}

/// Seeds a freshly created child context from the session-shared
/// instruction registry: attaches the registry handle (an `Arc` clone,
/// never a process-global), reconciles a child-owned baseline keyed by the
/// child's actual agent id, and applies it so the rules are pinned before
/// the child's first prompt. Returns the emitted baseline epoch so the
/// caller can persist it to the child's wire JSONL, or `None` when no
/// registry is attached or the baseline is a no-op.
pub async fn seed_child_instruction_baseline(
    context: &mut AgentContext,
    config: &AgentConfig,
    parent: Option<&AgentInstructionState>,
    child_agent_id: &str,
) -> Option<InstructionEpochData> {
    let registry = config.instruction_registry.clone()?;
    context.attach_instruction_registry(registry.clone());
    let effective_max = effective_max_context_tokens(config);
    let budget = if effective_max == 0 {
        InstructionBudget::from_context(None, u64::MAX)
    } else {
        let effective_max = u64::try_from(effective_max).unwrap_or(u64::MAX);
        let reserved = u64::try_from(context.estimated_tokens())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::from(config.max_tokens.unwrap_or(0)));
        InstructionBudget::from_context(Some(effective_max), effective_max.saturating_sub(reserved))
    };
    let parent_state = parent.cloned().unwrap_or_default();
    let request = registry.child_baseline_request(
        &parent_state,
        child_agent_id.to_owned(),
        config.instruction_inheritance,
        budget,
    );
    let child_state = context.instruction_state().clone();
    let (epoch, fingerprint) = match registry.reconcile(request, &child_state).await {
        InstructionPreflightDecision::Proceed { fingerprint } => {
            context.instruction_state_mut().last_epoch_fingerprint = Some(fingerprint.hash);
            return None;
        }
        InstructionPreflightDecision::Defer { epoch, fingerprint }
        | InstructionPreflightDecision::Block { epoch, fingerprint } => (epoch, fingerprint),
    };
    context.apply_instruction_epoch(&epoch);
    context.instruction_state_mut().last_epoch_fingerprint = Some(fingerprint.hash);
    Some(epoch)
}

async fn run_agent_snapshot(
    deps: ChildRuntimeDeps,
    prompt: String,
    prior_context: AgentContext,
    steer_input: SteerInputHandle,
    agent_id: String,
    child_wire_path: Option<PathBuf>,
    mut on_event: impl FnMut(&AgentEvent) + Send,
) -> Result<(Vec<AgentEvent>, Vec<AgentMessage>), String> {
    let parent_instruction_state = deps.parent_instruction_state;
    let child_config = child_config(deps.config, deps.role).with_agent_id(agent_id.clone());
    let child_tools = Arc::new(deps.tools.filtered_for_agent_role(deps.role));
    let parent_cancel_token = deps.cancel_token.clone();
    let cancel_token = CancellationToken::new();
    if parent_cancel_token.is_cancelled() {
        cancel_token.cancel();
    }
    let cancel_bridge_token = cancel_token.clone();
    let bridge_parent = parent_cancel_token.clone();
    tokio::spawn(async move {
        tokio::select! {
            () = bridge_parent.cancelled() => cancel_bridge_token.cancel(),
            () = cancel_bridge_token.cancelled() => {}
        }
    });
    let mut writer = if let Some(path) = child_wire_path {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|err| err.to_string())?;
        }
        Some(
            crate::session::JsonlSessionWriter::open_append(path)
                .await
                .map_err(|err| err.to_string())?,
        )
    } else {
        None
    };
    let mut persistence = crate::session::SessionEventPersistence::default();
    let mut context = prior_context;
    // Materialize the child-owned instruction baseline before the child's
    // first prompt: attach the session-shared registry handle, reconcile a
    // baseline keyed by this child's actual agent id, and pin the result.
    // A resumed child (replay already restored an epoch) skips re-baselining
    // and reconciles its replayed state against current disk at the first
    // live tool preflight, like the main agent.
    let baseline_epoch = if context.instruction_state().visible_generation == 0 {
        seed_child_instruction_baseline(
            &mut context,
            &child_config,
            parent_instruction_state.as_ref(),
            &agent_id,
        )
        .await
    } else {
        if let Some(registry) = child_config.instruction_registry.clone() {
            context.attach_instruction_registry(registry);
        }
        None
    };
    let child_runtime =
        AgentRuntime::with_shared_tools_and_configured_specs(child_config, deps.model, child_tools)
            .with_steer_input(steer_input);
    let mut events = Vec::new();
    if let Some(epoch) = baseline_epoch {
        // Child epochs go to the child's own wire JSONL.
        let event = AgentEvent::InstructionEpoch { epoch };
        persist_child_wire_event(&mut writer, &mut persistence, &event).await?;
        on_event(&event);
        events.push(event);
    }
    let mut stream = child_runtime.run_turn_with_cancel(
        &mut context,
        AgentMessage::user_text(prompt),
        cancel_token.clone(),
    );
    while let Some(event) = stream.next().await {
        let event = match event {
            Ok(event) => event,
            Err(err) => {
                flush_child_writer(&mut writer).await?;
                cancel_token.cancel();
                return Err(err.to_string());
            }
        };
        let write_result = if let Some(child_writer) = writer.as_mut() {
            let mut result = Ok(());
            for persisted in persistence.persisted_events(&event) {
                if let Err(error) = child_writer.append_event(&persisted).await {
                    result = Err(error);
                    break;
                }
            }
            result
        } else {
            Ok(())
        };
        if let Err(err) = write_result {
            cancel_token.cancel();
            drain_child_stream(&mut stream).await;
            let _ = flush_child_writer(&mut writer).await;
            return Err(err.to_string());
        }
        on_event(&event);
        events.push(event);
    }
    flush_child_writer(&mut writer).await?;
    cancel_token.cancel();
    drop(stream);
    // Extract the accumulated messages (prior + this turn) so they can be
    // stored on the snapshot for future resume.
    let messages = context.messages().to_vec();
    Ok((events, messages))
}

async fn flush_child_writer(
    writer: &mut Option<crate::session::JsonlSessionWriter>,
) -> Result<(), String> {
    if let Some(writer) = writer.as_mut() {
        writer.flush().await.map_err(|err| err.to_string())?;
    }
    Ok(())
}

async fn persist_child_wire_event(
    writer: &mut Option<crate::session::JsonlSessionWriter>,
    persistence: &mut crate::session::SessionEventPersistence,
    event: &AgentEvent,
) -> Result<(), String> {
    if let Some(child_writer) = writer.as_mut() {
        for persisted in persistence.persisted_events(event) {
            child_writer
                .append_event(&persisted)
                .await
                .map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

async fn drain_child_stream(stream: &mut crate::runtime::AgentEventStream<'_>) {
    while stream.next().await.is_some() {}
}

fn child_config(mut config: AgentConfig, role: AgentRole) -> AgentConfig {
    let profile = super::profile::AgentProfile::for_role(role);
    let base = config
        .system_prompt
        .as_deref()
        .unwrap_or_else(|| subagent_system_constraints());
    config.system_prompt = Some(format!(
        "{base}\n\n<subagent_profile>\n{}\n\nDo not repeat or acknowledge this profile text in your final answer. Return only the requested findings or summary.\n</subagent_profile>",
        profile.prompt_addendum
    ));
    // Filter model-visible tool specs to the role's explicit allowlist.
    config.tools = config
        .tools
        .iter()
        .filter(|spec| profile.allowed_tools.contains(spec.name.as_str()))
        .cloned()
        .collect();
    let profile_clone = super::profile::AgentProfile::for_role(role);
    config.with_before_tool_call(move |tool_call| {
        block_forbidden_subagent_tool_call(tool_call, &profile_clone)
    })
}

fn block_forbidden_subagent_tool_call(
    tool_call: &AgentToolCall,
    profile: &super::profile::AgentProfile,
) -> Option<crate::ToolResult> {
    // Shell access (Bash/Terminal): denied unless the role's policy allows it.
    // Read-only behavior for Explorer/Reviewer is enforced by the profile's
    // `prompt_addendum`, not by command-syntax classification — see profile.rs.
    let is_shell = matches!(tool_call.name.as_ref(), "Bash" | "Terminal");
    if is_shell && !profile.tool_policy.allow_shell {
        return Some(crate::ToolResult::error(format!(
            "{} agents may not run shell commands",
            profile.display_label
        )));
    }

    // File writes (Write/Edit): denied unless the role's policy allows it.
    if matches!(tool_call.name.as_ref(), "Write" | "Edit") && !profile.tool_policy.allow_file_writes
    {
        return Some(crate::ToolResult::error(format!(
            "{} agents may not edit or write files",
            profile.display_label
        )));
    }

    None
}

fn summarize_child_events(events: &[AgentEvent], elapsed: Duration) -> AgentRunUpdate {
    let latest_text = latest_assistant_text(events).map(|text| bounded_latest_text(&text));
    let summary = latest_text
        .clone()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| "Child agent completed without text output.".to_owned());
    AgentRunUpdate {
        summary,
        tool_count: events
            .iter()
            .filter(|event| matches!(event, AgentEvent::ToolExecutionFinished { .. }))
            .count(),
        token_count: events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TokenUsage { usage, .. } => {
                    Some((usage.input_tokens + usage.output_tokens) as usize)
                }
                _ => None,
            })
            .sum(),
        cache_read_token_count: events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TokenUsage { usage, .. } => {
                    Some(usage.input_cache_read_tokens as usize)
                }
                _ => None,
            })
            .sum(),
        cache_write_token_count: events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::TokenUsage { usage, .. } => {
                    Some(usage.input_cache_write_tokens as usize)
                }
                _ => None,
            })
            .sum(),
        elapsed,
        latest_text,
        activity: summarize_child_activity(events),
    }
}

fn child_events_have_error(events: &[AgentEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, AgentEvent::Error { .. }))
}

fn child_events_were_cancelled(events: &[AgentEvent]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::TurnFinished {
                stop_reason: StopReason::Cancelled,
                ..
            } | AgentEvent::RunFinished {
                stop_reason: StopReason::Cancelled,
                ..
            }
        )
    })
}

fn latest_assistant_text(events: &[AgentEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| match event {
        AgentEvent::Error { message, .. } => (!message.trim().is_empty()).then(|| message.clone()),
        AgentEvent::MessageAppended {
            message: AgentMessage::Assistant { content, .. },
        } => {
            let text = content
                .iter()
                .filter_map(|part| match part {
                    Content::Text { text } => Some(text.as_ref()),
                    _ => None,
                })
                .collect::<String>();
            (!text.trim().is_empty()).then_some(text)
        }
        _ => None,
    })
}

fn summarize_child_activity(events: &[AgentEvent]) -> Vec<AgentActivityEntry> {
    let mut activity = Vec::new();
    let mut attempt_start = 0;
    let mut tool_args: HashMap<String, serde_json::Value> = HashMap::new();
    for event in events {
        if apply_retry_activity(&mut activity, &mut attempt_start, event).is_none() {
            apply_activity_event(&mut activity, attempt_start, &mut tool_args, event);
        }
        if matches!(
            event,
            AgentEvent::MessageAppended {
                message: AgentMessage::Assistant { .. }
            }
        ) {
            attempt_start = activity.len();
        }
        attempt_start = attempt_start.saturating_sub(trim_activity(&mut activity));
    }
    activity
}

fn apply_activity_event(
    activity: &mut Vec<AgentActivityEntry>,
    attempt_start: usize,
    tool_args: &mut HashMap<String, serde_json::Value>,
    event: &AgentEvent,
) {
    if apply_tool_activity_event(activity, tool_args, event) {
        return;
    }
    match event {
        AgentEvent::MessageAppended {
            message: AgentMessage::Assistant { content, .. },
        } => {
            let text = content_text(content);
            let canonical_text = bounded_latest_text(&text);
            if !canonical_text.is_empty()
                && latest_text_activity(activity.as_slice(), false).as_deref()
                    != Some(canonical_text.as_str())
            {
                let _ = append_text_activity(activity, attempt_start, &text, false);
            }
        }
        AgentEvent::ThinkingDelta { text, .. } => {
            let _ = append_text_activity(activity, attempt_start, text, true);
        }
        AgentEvent::TextDelta { text, .. } => {
            let _ = append_text_activity(activity, attempt_start, text, false);
        }
        _ => {}
    }
}

/// Process tool-related activity events. Returns `true` if the event was handled.
fn apply_tool_activity_event(
    activity: &mut Vec<AgentActivityEntry>,
    tool_args: &mut HashMap<String, serde_json::Value>,
    event: &AgentEvent,
) -> bool {
    match event {
        AgentEvent::ToolExecutionQueued {
            id,
            name,
            arguments,
            ..
        } => {
            tool_args.insert(id.clone(), arguments.clone());
            let _ = upsert_queued_tool_activity(
                activity,
                id,
                name,
                summarize_tool_arguments(name, arguments),
                now_ms(),
            );
            true
        }
        AgentEvent::ToolExecutionQueueUpdated {
            id,
            position,
            waiting_ms,
            ..
        } => {
            let _ = update_queued_tool_activity(
                activity,
                id,
                *position,
                now_ms().saturating_sub(*waiting_ms),
            );
            true
        }
        AgentEvent::ToolExecutionStarted {
            id,
            name,
            arguments,
            ..
        } => {
            tool_args.insert(id.clone(), arguments.clone());
            upsert_tool_activity(
                activity,
                id,
                name,
                summarize_tool_arguments(name, arguments),
                AgentToolActivityPhase::Ongoing,
                None,
            );
            true
        }
        AgentEvent::ToolExecutionFinished {
            id, name, result, ..
        } => {
            let phase = if result.is_error {
                AgentToolActivityPhase::Failed
            } else {
                AgentToolActivityPhase::Done
            };
            let summary = result
                .details
                .as_ref()
                .and_then(summarize_edit_details)
                .or_else(|| {
                    tool_args
                        .get(id)
                        .and_then(|arguments| summarize_tool_arguments(name, arguments))
                })
                .or_else(|| last_tool_summary(activity.as_slice(), id));
            upsert_tool_activity(
                activity,
                id,
                name,
                summary,
                phase,
                tool_output_preview(name, result, false),
            );
            true
        }
        AgentEvent::ToolExecutionUpdate {
            id,
            name,
            partial_result,
            ..
        } => {
            let summary = partial_result
                .details
                .as_ref()
                .and_then(summarize_edit_details)
                .or_else(|| {
                    tool_args
                        .get(id)
                        .and_then(|arguments| summarize_tool_arguments(name, arguments))
                })
                .or_else(|| last_tool_summary(activity.as_slice(), id));
            upsert_tool_activity(
                activity,
                id,
                name,
                summary,
                AgentToolActivityPhase::Ongoing,
                tool_output_preview(name, partial_result, true),
            );
            true
        }
        _ => false,
    }
}

fn push_text_activity(
    snapshot: &mut AgentSnapshot,
    attempt_start: usize,
    text: &str,
    thinking: bool,
) {
    let Some(accumulated) =
        append_text_activity(&mut snapshot.activity, attempt_start, text, thinking)
    else {
        return;
    };

    if !thinking {
        snapshot.latest_text = Some(accumulated);
    }
}

fn apply_retry_activity(
    activity: &mut Vec<AgentActivityEntry>,
    attempt_start: &mut usize,
    event: &AgentEvent,
) -> Option<(bool, Option<String>)> {
    let start = (*attempt_start).min(activity.len());
    match event {
        AgentEvent::RetryScheduled {
            retry, max_retries, ..
        }
        | AgentEvent::RetryStarted {
            retry, max_retries, ..
        } => {
            let mut current_attempt = activity.split_off(start);
            current_attempt.retain(|entry| matches!(&entry.kind, AgentActivityKind::Tool { .. }));
            activity.append(&mut current_attempt);
            let reconnecting = bounded_latest_text(&format!("Reconnecting {retry}/{max_retries}"));
            activity.push(AgentActivityEntry {
                kind: AgentActivityKind::Text {
                    text: reconnecting.clone(),
                    thinking: false,
                },
            });
            Some((true, Some(reconnecting)))
        }
        AgentEvent::RetryResumed { .. } => {
            let mut current_attempt = activity.split_off(start);
            let previous_len = current_attempt.len();
            current_attempt.retain(|entry| {
                !matches!(
                    &entry.kind,
                    AgentActivityKind::Text { text, thinking: false }
                        if text.starts_with("Reconnecting ")
                )
            });
            let cleared = current_attempt.len() != previous_len;
            activity.append(&mut current_attempt);
            *attempt_start = activity.len();
            let latest_text = cleared
                .then(|| latest_text_activity(activity, false))
                .flatten();
            Some((cleared, latest_text))
        }
        AgentEvent::RetryExhausted { message, .. } | AgentEvent::Error { message, .. } => {
            let mut current_attempt = activity.split_off(start);
            current_attempt.retain(|entry| matches!(&entry.kind, AgentActivityKind::Tool { .. }));
            activity.append(&mut current_attempt);
            let error = bounded_latest_text(message);
            if latest_text_activity(activity, false).as_deref() != Some(error.as_str()) {
                activity.push(AgentActivityEntry {
                    kind: AgentActivityKind::Text {
                        text: error.clone(),
                        thinking: false,
                    },
                });
            }
            *attempt_start = activity.len();
            Some((true, Some(error)))
        }
        AgentEvent::TurnFinished {
            stop_reason: StopReason::Cancelled,
            ..
        }
        | AgentEvent::RunFinished {
            stop_reason: StopReason::Cancelled,
            ..
        } => {
            let mut current_attempt = activity.split_off(start);
            let previous_len = current_attempt.len();
            current_attempt.retain(|entry| matches!(&entry.kind, AgentActivityKind::Tool { .. }));
            let cleared = current_attempt.len() != previous_len;
            activity.append(&mut current_attempt);
            *attempt_start = activity.len();
            Some((cleared, latest_text_activity(activity, false)))
        }
        _ => None,
    }
}

fn append_text_activity(
    activity: &mut Vec<AgentActivityEntry>,
    attempt_start: usize,
    text: &str,
    thinking: bool,
) -> Option<String> {
    if activity.len() > attempt_start
        && let Some(AgentActivityEntry {
            kind:
                AgentActivityKind::Text {
                    text: previous,
                    thinking: previous_thinking,
                },
        }) = activity.last_mut()
        && *previous_thinking == thinking
    {
        previous.push_str(text);
        *previous = bounded_stream_text(previous);
        return Some(previous.trim().to_owned());
    }

    if text.trim().is_empty() {
        return None;
    }

    let bounded = bounded_stream_text(text);
    let accumulated = bounded.trim().to_owned();
    activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: bounded,
            thinking,
        },
    });
    Some(accumulated)
}

const MAX_LATEST_MODEL_TEXT_CHARS: usize = 512;

fn bounded_latest_text(text: &str) -> String {
    bounded_stream_text(text.trim())
}

fn bounded_stream_text(text: &str) -> String {
    if text.chars().count() <= MAX_LATEST_MODEL_TEXT_CHARS {
        return text.to_owned();
    }
    let keep = MAX_LATEST_MODEL_TEXT_CHARS.saturating_sub(3);
    let start = text
        .char_indices()
        .nth(text.chars().count().saturating_sub(keep))
        .map_or(0, |(index, _)| index);
    format!("...{}", &text[start..])
}

fn latest_text_activity(activity: &[AgentActivityEntry], thinking: bool) -> Option<String> {
    activity
        .iter()
        .rev()
        .filter_map(|entry| match &entry.kind {
            AgentActivityKind::Text {
                text,
                thinking: entry_thinking,
            } if *entry_thinking == thinking => Some(text.trim()),
            _ => None,
        })
        .find(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn upsert_tool_activity(
    activity: &mut Vec<AgentActivityEntry>,
    id: &str,
    name: &str,
    summary: Option<String>,
    phase: AgentToolActivityPhase,
    output: Option<AgentToolOutputPreview>,
) {
    for entry in activity.iter_mut().rev() {
        let AgentActivityKind::Tool {
            id: entry_id,
            name: entry_name,
            summary: entry_summary,
            phase: entry_phase,
            output: entry_output,
        } = &mut entry.kind
        else {
            continue;
        };
        if entry_id == id {
            if summary.is_some() {
                *entry_summary = summary;
            }
            name.clone_into(entry_name);
            *entry_phase = phase;
            if output.is_some() {
                *entry_output = output;
            }
            return;
        }
    }
    activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            summary,
            phase,
            output,
        },
    });
}

fn upsert_queued_tool_activity(
    activity: &mut Vec<AgentActivityEntry>,
    id: &str,
    name: &str,
    summary: Option<String>,
    queued_at_ms: u64,
) -> bool {
    for entry in activity.iter_mut().rev() {
        let AgentActivityKind::Tool {
            id: entry_id,
            name: entry_name,
            summary: entry_summary,
            phase: entry_phase,
            ..
        } = &mut entry.kind
        else {
            continue;
        };
        if entry_id != id {
            continue;
        }
        match entry_phase {
            AgentToolActivityPhase::Ongoing
            | AgentToolActivityPhase::Done
            | AgentToolActivityPhase::Failed => {
                // Late queue transition must not regress live/terminal work.
                return false;
            }
            AgentToolActivityPhase::Queued { .. } => {
                if summary.is_some() {
                    *entry_summary = summary;
                }
                name.clone_into(entry_name);
                *entry_phase = AgentToolActivityPhase::Queued {
                    position: None,
                    queued_at_ms,
                };
                return true;
            }
        }
    }
    activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            summary,
            phase: AgentToolActivityPhase::Queued {
                position: None,
                queued_at_ms,
            },
            output: None,
        },
    });
    true
}

fn update_queued_tool_activity(
    activity: &mut [AgentActivityEntry],
    id: &str,
    position: usize,
    queued_at_ms: u64,
) -> bool {
    for entry in activity.iter_mut().rev() {
        let AgentActivityKind::Tool {
            id: entry_id,
            phase: entry_phase,
            ..
        } = &mut entry.kind
        else {
            continue;
        };
        if entry_id != id {
            continue;
        }
        let AgentToolActivityPhase::Queued {
            position: entry_position,
            queued_at_ms: entry_queued_at_ms,
        } = entry_phase
        else {
            // Only live Queued rows accept rank/wait ticks.
            return false;
        };
        *entry_position = Some(position);
        *entry_queued_at_ms = queued_at_ms;
        return true;
    }
    false
}

fn last_tool_summary(activity: &[AgentActivityEntry], id: &str) -> Option<String> {
    activity.iter().rev().find_map(|entry| {
        let AgentActivityKind::Tool {
            id: entry_id,
            summary,
            ..
        } = &entry.kind
        else {
            return None;
        };
        (entry_id == id).then(|| summary.clone()).flatten()
    })
}

fn summarize_tool_arguments(name: &str, arguments: &serde_json::Value) -> Option<String> {
    if name == "Edit" {
        return summarize_edit_arguments(arguments);
    }
    let starts_shell = name == "Bash"
        || (name == "Terminal"
            && arguments.get("mode").and_then(serde_json::Value::as_str) == Some("start"));
    if starts_shell
        && let Some(command) = arguments.get("command").and_then(serde_json::Value::as_str)
        && !command.trim().is_empty()
    {
        return Some(compact_shell_command(command));
    }
    for key in [
        "path",
        "pattern",
        "query",
        "command",
        "description",
        "task_id",
        "id",
    ] {
        if let Some(value) = arguments.get(key).and_then(serde_json::Value::as_str)
            && !value.trim().is_empty()
        {
            return Some(compact_line(value));
        }
    }
    arguments
        .as_object()
        .and_then(|object| object.iter().next())
        .map(|(key, value)| {
            if let Some(value) = value.as_str() {
                compact_line(value)
            } else {
                format!("{key}: {}", compact_line(&value.to_string()))
            }
        })
}

fn summarize_edit_arguments(arguments: &serde_json::Value) -> Option<String> {
    let files = arguments.get("files")?.as_array()?;
    if files.is_empty() {
        return None;
    }
    let mut replacements = 0usize;
    let mut paths = Vec::new();
    for file in files {
        if let Some(path) = file.get("path").and_then(serde_json::Value::as_str) {
            paths.push(path);
        }
        replacements += file
            .get("replacements")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
    }
    let head = paths.first().copied().unwrap_or("?");
    let tail = paths.last().copied().unwrap_or(head);
    let path_part = if paths.len() <= 1 {
        compact_line(head)
    } else if paths.len() == 2 {
        format!("{} · {}", compact_line(head), compact_line(tail))
    } else {
        format!("{} … {}", compact_line(head), compact_line(tail))
    };
    Some(bounded_edit_summary(format!(
        "{} files · {} replacements · {path_part}",
        files.len(),
        replacements
    )))
}

fn summarize_edit_details(details: &serde_json::Value) -> Option<String> {
    let kind = details.get("kind")?.as_str()?;
    let summary = match kind {
        "edit_prepared" => {
            let files = details.get("files")?.as_u64()?;
            let replacements = details.get("replacements")?.as_u64()?;
            let added = details
                .get("added")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let removed = details
                .get("removed")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            format!("prepared · {files} files · {replacements} replacements · +{added} -{removed}")
        }
        "edit_progress" => {
            let committed = details.get("committed")?.as_u64()?;
            let total = details.get("total")?.as_u64()?;
            let latest = details
                .get("latest_path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            format!("committing {committed}/{total} · {}", compact_line(latest))
        }
        "edit" => {
            let status = details.get("status")?.as_str()?;
            let number = |key| {
                details
                    .get(key)
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            };
            let files = number("files");
            let replacements = number("replacements");
            let added = number("added");
            let removed = number("removed");
            let changes = details.get("changes").and_then(serde_json::Value::as_array);
            let committed = changes.map_or(0, |changes| {
                changes
                    .iter()
                    .filter(|change| {
                        matches!(
                            change.get("status").and_then(serde_json::Value::as_str),
                            Some("committed" | "committed_unsynced")
                        )
                    })
                    .count()
            });
            let failed_path = details
                .get("failed_path")
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    changes?
                        .iter()
                        .find(|change| {
                            change.get("status").and_then(serde_json::Value::as_str)
                                == Some("failed")
                        })
                        .and_then(|change| change.get("path"))
                        .and_then(serde_json::Value::as_str)
                });
            match status {
                "committed" => {
                    format!("{files} files · {replacements} replacements · +{added} -{removed}")
                }
                "prepare_failed" => "prepare failed · zero writes".to_owned(),
                "stale" => "stale · zero writes".to_owned(),
                "cancelled" => "cancelled · zero writes".to_owned(),
                "commit_failed" => failed_path.map_or_else(
                    || "commit failed · zero writes".to_owned(),
                    |path| format!("commit failed · zero writes · {}", compact_line(path)),
                ),
                "partial_commit"
                    if details.get("cause").and_then(serde_json::Value::as_str)
                        == Some("cancelled") =>
                {
                    format!("cancelled · {committed}/{files} committed")
                }
                "partial_commit" => failed_path.map_or_else(
                    || format!("partial · {committed}/{files} committed"),
                    |path| {
                        format!(
                            "partial · {committed}/{files} committed · failed at {}",
                            compact_line(path)
                        )
                    },
                ),
                "durability_uncertain" => {
                    format!("durability uncertain · {committed}/{files} committed")
                }
                other => other.to_owned(),
            }
        }
        _ => return None,
    };
    Some(bounded_edit_summary(summary))
}

fn bounded_edit_summary(summary: String) -> String {
    const MAX: usize = 160;
    const HEAD: usize = 78;
    const SEPARATOR: &str = " … ";
    const TAIL: usize = MAX - HEAD - 3;

    let chars = summary.chars().count();
    if chars <= MAX {
        return summary;
    }
    format!(
        "{}{}{}",
        summary.chars().take(HEAD).collect::<String>(),
        SEPARATOR,
        summary.chars().skip(chars - TAIL).collect::<String>()
    )
}

const MAX_AGENT_TOOL_OUTPUT_PREVIEW_BYTES: usize = 512;

fn tool_output_preview(
    name: &str,
    result: &crate::ToolResult,
    tail: bool,
) -> Option<AgentToolOutputPreview> {
    if !should_preview_tool_output(name) || result.content.trim().is_empty() {
        return None;
    }
    let (text, truncated) = cap_preview_text(&result.content, MAX_AGENT_TOOL_OUTPUT_PREVIEW_BYTES);
    Some(AgentToolOutputPreview {
        text,
        is_error: result.is_error,
        truncated,
        tail,
    })
}

fn compact_shell_command(command: &str) -> String {
    const MAX: usize = 96;
    const HEAD: usize = 46;
    const SEPARATOR: &str = " … ";
    const TAIL: usize = MAX - HEAD - 3;

    let mut display = String::new();
    for character in command.chars() {
        if character.is_whitespace() {
            display.push(' ');
        } else if character.is_control() {
            display.extend(character.escape_default());
        } else {
            display.push(character);
        }
    }
    let line = display.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = line.chars().count();
    if chars <= MAX {
        return line;
    }
    format!(
        "{}{}{}",
        line.chars().take(HEAD).collect::<String>(),
        SEPARATOR,
        line.chars().skip(chars - TAIL).collect::<String>()
    )
}

fn should_preview_tool_output(name: &str) -> bool {
    matches!(name, "Bash" | "Terminal") || name.starts_with("mcp__")
}

fn cap_preview_text(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_owned(), false);
    }
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    (format!("[...truncated]\n{}", &text[start..]), true)
}

fn content_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            Content::Text { text } => Some(text.as_ref()),
            _ => None,
        })
        .collect::<String>()
}

fn compact_line(text: &str) -> String {
    const MAX: usize = 96;
    let mut line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > MAX {
        line = format!(
            "{}...",
            line.chars().take(MAX.saturating_sub(3)).collect::<String>()
        );
    }
    line
}

fn trim_activity(activity: &mut Vec<AgentActivityEntry>) -> usize {
    const MAX_AGENT_ACTIVITY: usize = 24;
    if activity.len() <= MAX_AGENT_ACTIVITY {
        return 0;
    }

    // Find the latest ongoing tool entry. If found, move it to the end so the
    // plain drain-from-front keeps it.
    let ongoing_index = activity.iter().rposition(|entry| {
        matches!(
            &entry.kind,
            AgentActivityKind::Tool {
                phase: AgentToolActivityPhase::Ongoing,
                ..
            }
        )
    });

    if let Some(index) = ongoing_index {
        // Move the latest ongoing tool to the very end.
        let entry = activity.remove(index);
        activity.push(entry);
    }

    let excess = activity.len().saturating_sub(MAX_AGENT_ACTIVITY);
    activity.drain(..excess);
    excess
}

fn child_prompt(task: &str, context: DelegateContext, role: AgentRole) -> String {
    format!(
        "You are a bounded Neo subagent.\n\nRole: {role:?}\nTask: {task}\nContext mode: {}\n\nReturn a concise result for the parent agent. Do not perform git mutations. Do not run git add, git commit, git reset, git checkout, git restore, git stash, git clean, git rebase, git push, git rm, git branch, git switch, git merge, git cherry-pick, git tag, or git worktree.",
        context.as_str()
    )
}

fn subagent_system_constraints() -> &'static str {
    "Subagent safety constraints: never mutate git state. You may inspect with read-only git commands such as git status, git diff, git log, or git blame, but you must not run git add, git commit, git reset, git checkout, git restore, git stash, git clean, git rebase, git push, git rm, git branch, git switch, git merge, git cherry-pick, git tag, git worktree, git apply, git am, git mv, git gc, git reflog, or git filter-branch."
}

impl DelegateContext {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Summary => "summary",
            Self::None => "none",
        }
    }
}

fn swarm_child_task(template: &str, item: &str) -> String {
    apply_swarm_template(template, item, "")
}

#[must_use]
pub fn apply_swarm_template(template: &str, item: &str, description: &str) -> String {
    template
        .replace("{{item}}", item)
        .replace("{{description}}", description)
}

// Keep `Instant` imported for future elapsed-time tracking in P2.
const _: fn() = || {
    let _ = Instant::now();
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolResult;

    #[test]
    fn shell_tool_summary_preserves_head_and_tail_within_budget() {
        let command = format!(
            "cargo test --package neo-agent-core --lib {} --exact --nocapture",
            "multi_agent::runtime::tests::very_long_filter_".repeat(4)
        );
        for (name, arguments) in [
            ("Bash", serde_json::json!({"command": command.clone()})),
            (
                "Terminal",
                serde_json::json!({"mode": "start", "command": command.clone()}),
            ),
        ] {
            let summary = summarize_tool_arguments(name, &arguments).expect("shell summary");
            assert_eq!(summary.chars().count(), 96, "{summary}");
            assert!(
                summary.starts_with("cargo test --package neo-agent-core"),
                "{summary}"
            );
            assert!(summary.contains(" … "), "{summary}");
            assert!(summary.ends_with("--exact --nocapture"), "{summary}");
        }

        let summary = summarize_tool_arguments(
            "Terminal",
            &serde_json::json!({"mode": "write", "command": command}),
        )
        .expect("terminal write summary");
        assert_eq!(summary.chars().count(), 96, "{summary}");
        assert!(summary.ends_with("..."), "{summary}");
        assert!(!summary.contains(" … "), "{summary}");

        let path = format!("/workspace/{}", "very-long-path-segment/".repeat(8));
        let summary = summarize_tool_arguments("Read", &serde_json::json!({"path": path}))
            .expect("read summary");
        assert_eq!(summary.chars().count(), 96, "{summary}");
        assert!(
            summary.starts_with("/workspace/very-long-path"),
            "{summary}"
        );
        assert!(summary.ends_with("..."), "{summary}");
        assert!(!summary.contains('…'), "{summary}");

        let summary = summarize_tool_arguments(
            "Bash",
            &serde_json::json!({"command": "printf\t'audit'\x1b[31m danger\x03"}),
        )
        .expect("unsafe bash summary");
        assert!(summary.contains("printf 'audit'"), "{summary}");
        assert!(summary.contains(r"\u{1b}[31m danger\u{3}"), "{summary}");
        assert!(
            summary.chars().all(|character| !character.is_control()),
            "{summary:?}"
        );
    }

    #[test]
    fn edit_tool_summary_preserves_counts_and_head_tail_within_budget() {
        let summary = summarize_tool_arguments(
            "Edit",
            &serde_json::json!({
                "files": [
                    {"path": "src/a.rs", "replacements": [{"old":"a","new":"A"}]},
                    {"path": "src/b.rs", "replacements": [{"old":"b","new":"B"},{"old":"c","new":"C"}]},
                    {"path": "src/c.rs", "replacements": [{"old":"d","new":"D"}]}
                ]
            }),
        )
        .expect("edit summary");
        assert!(summary.contains("3 files"), "{summary}");
        assert!(summary.contains("4 replacements"), "{summary}");
        assert!(summary.contains("src/a.rs"), "{summary}");
        assert!(summary.contains("src/c.rs"), "{summary}");
        assert!(summary.chars().count() <= 160, "{summary}");
    }

    #[test]
    fn edit_tool_summary_prefers_structured_partial_progress() {
        let mut activity = Vec::new();
        let mut tool_args = HashMap::new();
        let started = AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "e1".to_owned(),
            name: "Edit".to_owned(),
            arguments: serde_json::json!({
                "files": [{"path":"a.rs","replacements":[{"old":"a","new":"A"}]}]
            }),
        };
        assert!(apply_tool_activity_event(
            &mut activity,
            &mut tool_args,
            &started
        ));
        let update = AgentEvent::ToolExecutionUpdate {
            turn: 1,
            id: "e1".to_owned(),
            name: "Edit".to_owned(),
            partial_result: ToolResult::ok("progress").with_details(serde_json::json!({
                "kind": "edit_progress",
                "committed": 2,
                "total": 5,
                "latest_path": "src/lib.rs",
                "added": 9,
                "removed": 4
            })),
        };
        assert!(apply_tool_activity_event(
            &mut activity,
            &mut tool_args,
            &update
        ));
        let summary = last_tool_summary(&activity, "e1").expect("summary");
        assert!(summary.contains("committing 2/5"), "{summary}");
        assert!(summary.contains("src/lib.rs"), "{summary}");
    }

    #[test]
    fn live_edit_summary_uses_structured_progress_and_terminal_partial() {
        let runtime = MultiAgentRuntime::new();
        let child = runtime.start_foreground_delegate_for_test("edit files");
        let started_at = Instant::now();
        let progress = AgentEvent::ToolExecutionUpdate {
            turn: 1,
            id: "e-live".to_owned(),
            name: "Edit".to_owned(),
            partial_result: ToolResult::ok("progress").with_details(serde_json::json!({
                "kind": "edit_progress",
                "committed": 1,
                "total": 3,
                "latest_path": "a.rs",
                "added": 1,
                "removed": 1
            })),
        };
        runtime
            .apply_child_event(&child.id, started_at, &progress)
            .expect("progress update");
        let progress_snapshot = runtime.agent_snapshot(child.id.as_str()).expect("snapshot");
        let progress_summary =
            last_tool_summary(&progress_snapshot.activity, "e-live").expect("progress summary");
        assert!(
            progress_summary.contains("committing 1/3"),
            "{progress_summary}"
        );

        let finished = AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "e-live".to_owned(),
            name: "Edit".to_owned(),
            result: ToolResult::error("partial").with_details(serde_json::json!({
                "kind": "edit",
                "status": "partial_commit",
                "files": 3,
                "replacements": 3,
                "added": 1,
                "removed": 1,
                "changes": [
                    {"path":"a.rs","status":"committed"},
                    {"path": format!("{}b.rs", "very-long-path/".repeat(20)), "status":"failed"},
                    {"path":"c.rs","status":"not_attempted"}
                ]
            })),
        };
        runtime
            .apply_child_event(&child.id, started_at, &finished)
            .expect("finished update");
        let finished_snapshot = runtime.agent_snapshot(child.id.as_str()).expect("snapshot");
        let finished_summary =
            last_tool_summary(&finished_snapshot.activity, "e-live").expect("finished summary");
        assert!(
            finished_summary.contains("partial · 1/3 committed"),
            "{finished_summary}"
        );
        assert!(finished_summary.contains("failed at"), "{finished_summary}");
        assert!(
            finished_summary.chars().count() <= 160,
            "{finished_summary}"
        );
    }

    #[test]
    fn replayed_unfinished_edit_is_interrupted_and_not_resumed() {
        // Replay projects unfinished progress as a terminal interrupted card
        // without re-submitting PreparedEdit to runtime. Activity summary alone
        // never starts commit.
        let mut activity = Vec::new();
        let mut tool_args = HashMap::new();
        let update = AgentEvent::ToolExecutionUpdate {
            turn: 1,
            id: "e2".to_owned(),
            name: "Edit".to_owned(),
            partial_result: ToolResult::ok("progress").with_details(serde_json::json!({
                "kind": "edit_progress",
                "committed": 1,
                "total": 3,
                "latest_path": "a.rs",
                "added": 1,
                "removed": 1
            })),
        };
        assert!(apply_tool_activity_event(
            &mut activity,
            &mut tool_args,
            &update
        ));
        assert!(
            activity.iter().all(|entry| match &entry.kind {
                AgentActivityKind::Tool { phase, .. } => *phase == AgentToolActivityPhase::Ongoing,
                _ => true,
            }),
            "progress alone must not invent a completed commit"
        );
        // No execution attempt is recorded beyond the projected activity entry.
        assert_eq!(tool_args.len(), 0);
    }

    #[test]
    fn summarized_child_activity_preserves_whitespace_deltas_without_duplicate_body() {
        let events = [
            AgentEvent::TextDelta {
                turn: 1,
                text: "All".to_owned(),
            },
            AgentEvent::TextDelta {
                turn: 1,
                text: " ".to_owned(),
            },
            AgentEvent::TextDelta {
                turn: 1,
                text: "edits applied.".to_owned(),
            },
            AgentEvent::ThinkingDelta {
                turn: 1,
                text: "Let".to_owned(),
            },
            AgentEvent::ThinkingDelta {
                turn: 1,
                text: " ".to_owned(),
            },
            AgentEvent::ThinkingDelta {
                turn: 1,
                text: "me verify.".to_owned(),
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::assistant(
                    vec![Content::text("All edits applied.")],
                    Vec::new(),
                    StopReason::EndTurn,
                ),
            },
        ];

        let activity = summarize_child_activity(&events);
        let body = latest_text_activity(&activity, false);
        let thinking = latest_text_activity(&activity, true);

        assert_eq!(body.as_deref(), Some("All edits applied."));
        assert_eq!(thinking.as_deref(), Some("Let me verify."));
    }

    #[test]
    fn summarized_retry_activity_keeps_only_winning_attempt() {
        let events = [
            AgentEvent::TextDelta {
                turn: 1,
                text: "prior answer".to_owned(),
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::assistant(
                    vec![Content::text("prior answer")],
                    Vec::new(),
                    StopReason::ToolUse,
                ),
            },
            AgentEvent::ThinkingDelta {
                turn: 1,
                text: "failed reasoning one".to_owned(),
            },
            AgentEvent::TextDelta {
                turn: 1,
                text: "failed partial one".to_owned(),
            },
            AgentEvent::ThinkingDelta {
                turn: 1,
                text: "failed reasoning two".to_owned(),
            },
            AgentEvent::TextDelta {
                turn: 1,
                text: "failed partial two".to_owned(),
            },
            AgentEvent::RetryScheduled {
                turn: 1,
                retry: 1,
                max_retries: 5,
                delay_ms: 500,
                error_code: "provider.transport_error".to_owned(),
                message: "transport error: body closed".to_owned(),
            },
            AgentEvent::RetryStarted {
                turn: 1,
                retry: 1,
                max_retries: 5,
            },
            AgentEvent::RetryResumed { turn: 1, retry: 1 },
            AgentEvent::TextDelta {
                turn: 1,
                text: "winning answer".to_owned(),
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::assistant(
                    vec![Content::text("winning answer")],
                    Vec::new(),
                    StopReason::EndTurn,
                ),
            },
        ];

        let activity = summarize_child_activity(&events);

        assert_eq!(
            latest_text_activity(&activity, false).as_deref(),
            Some("winning answer")
        );
        assert_eq!(latest_text_activity(&activity, true), None);
        assert!(activity.iter().any(|entry| matches!(
            &entry.kind,
            AgentActivityKind::Text { text, thinking: false } if text == "prior answer"
        )));
        assert!(activity.iter().all(|entry| !matches!(
            &entry.kind,
            AgentActivityKind::Text { text, .. }
                if text.contains("failed") || text.starts_with("Reconnecting ")
        )));
    }

    #[test]
    fn retry_activity_fold_matches_live_snapshot_at_capacity() {
        let runtime = MultiAgentRuntime::new();
        let child = runtime.start_foreground_delegate_for_test("retry at activity cap");
        let started_at = Instant::now();
        let mut events = Vec::new();
        let mut record = |event| {
            let _ = runtime.apply_child_event(&child.id, started_at, &event);
            events.push(event);
        };

        record(AgentEvent::TextDelta {
            turn: 1,
            text: "prior answer".to_owned(),
        });
        record(AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                vec![Content::text("prior answer")],
                Vec::new(),
                StopReason::ToolUse,
            ),
        });
        for index in 0..24 {
            record(AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: format!("tool-{index}"),
                name: "Read".to_owned(),
                arguments: serde_json::json!({"path": format!("file-{index}")}),
            });
        }
        record(AgentEvent::ThinkingDelta {
            turn: 1,
            text: "failed reasoning".to_owned(),
        });
        record(AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 5,
            delay_ms: 500,
            error_code: "provider.transport_error".to_owned(),
            message: "transport error: body closed".to_owned(),
        });
        record(AgentEvent::RetryStarted {
            turn: 1,
            retry: 1,
            max_retries: 5,
        });

        let live = runtime.snapshot(&child.id).expect("live child snapshot");
        let summarized = summarize_child_activity(&events);

        assert_eq!(live.activity, summarized);
        assert_eq!(
            latest_text_activity(&summarized, false).as_deref(),
            Some("Reconnecting 1/5")
        );
    }

    #[test]
    fn retry_exhaustion_fold_matches_live_and_preserves_error() {
        let runtime = MultiAgentRuntime::new();
        let child = runtime.start_foreground_delegate_for_test("retry exhaustion");
        let started_at = Instant::now();
        let mut events = Vec::new();
        let mut record = |event| {
            let _ = runtime.apply_child_event(&child.id, started_at, &event);
            events.push(event);
        };

        record(AgentEvent::TextDelta {
            turn: 1,
            text: "failed partial one".to_owned(),
        });
        record(AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 1,
            delay_ms: 500,
            error_code: "provider.transport_error".to_owned(),
            message: "transport error: body closed".to_owned(),
        });
        record(AgentEvent::RetryStarted {
            turn: 1,
            retry: 1,
            max_retries: 1,
        });
        record(AgentEvent::RetryResumed { turn: 1, retry: 1 });
        record(AgentEvent::ThinkingDelta {
            turn: 1,
            text: "failed reasoning two".to_owned(),
        });
        record(AgentEvent::TextDelta {
            turn: 1,
            text: "failed partial two".to_owned(),
        });
        record(AgentEvent::RetryExhausted {
            turn: 1,
            retries_used: 1,
            error_code: "provider.transport_error".to_owned(),
            message: "transport error: connection reset".to_owned(),
        });
        record(AgentEvent::Error {
            turn: 1,
            message: "transport error: connection reset".to_owned(),
            code: Some("provider.transport_error".to_owned()),
            retry_after: None,
        });

        let live = runtime.snapshot(&child.id).expect("live child snapshot");
        let terminal = summarize_child_events(&events, Duration::ZERO);

        assert_eq!(live.activity, terminal.activity);
        assert_eq!(live.latest_text, terminal.latest_text);
        assert_eq!(
            terminal.latest_text.as_deref(),
            Some("transport error: connection reset")
        );
        assert_eq!(terminal.summary, "transport error: connection reset");
        assert!(terminal.activity.iter().all(|entry| !matches!(
            &entry.kind,
            AgentActivityKind::Text { text, .. }
                if text.contains("failed") || text.starts_with("Reconnecting ")
        )));
    }

    #[test]
    fn cancelled_retry_backoff_fold_matches_live() {
        let runtime = MultiAgentRuntime::new();
        let child = runtime.start_foreground_delegate_for_test("cancelled retry backoff");
        let started_at = Instant::now();
        let mut events = Vec::new();
        let mut record = |event| {
            let _ = runtime.apply_child_event(&child.id, started_at, &event);
            events.push(event);
        };

        record(AgentEvent::TextDelta {
            turn: 1,
            text: "prior answer".to_owned(),
        });
        record(AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                vec![Content::text("prior answer")],
                Vec::new(),
                StopReason::ToolUse,
            ),
        });
        record(AgentEvent::TextDelta {
            turn: 1,
            text: "failed partial".to_owned(),
        });
        record(AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 5,
            delay_ms: 500,
            error_code: "provider.transport_error".to_owned(),
            message: "transport error: body closed".to_owned(),
        });
        record(AgentEvent::RetryStarted {
            turn: 1,
            retry: 1,
            max_retries: 5,
        });
        record(AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        });
        record(AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Cancelled,
        });

        let live = runtime.snapshot(&child.id).expect("live child snapshot");
        let terminal = summarize_child_events(&events, Duration::ZERO);

        assert_eq!(live.activity, terminal.activity);
        assert_eq!(live.latest_text, terminal.latest_text);
        assert_eq!(terminal.latest_text.as_deref(), Some("prior answer"));
        assert_eq!(
            latest_text_activity(&terminal.activity, false).as_deref(),
            Some("prior answer")
        );
        assert!(terminal.activity.iter().all(|entry| !matches!(
            &entry.kind,
            AgentActivityKind::Text { text, .. }
                if text == "failed partial" || text.starts_with("Reconnecting ")
        )));
    }

    #[tokio::test]
    async fn live_cancel_guard_cancels_own_token_without_removing_replacement() {
        let runtime = MultiAgentRuntime::new();
        let parent = CancellationToken::new();
        let first = runtime.register_live_cancel("agent_test", &parent);
        let first_token = first.token();
        let second = runtime.register_live_cancel("agent_test", &parent);
        let second_token = second.token();

        drop(first);

        assert!(
            first_token.is_cancelled(),
            "dropping a live-cancel guard should stop its parent bridge"
        );
        assert!(
            !second_token.is_cancelled(),
            "dropping an old live-cancel guard must not cancel a newer run token"
        );
        assert!(
            runtime
                .state
                .lock()
                .expect("multi-agent state poisoned")
                .agent_cancel_tokens
                .contains_key("agent_test"),
            "dropping an old live-cancel guard must not remove a newer run token"
        );

        drop(second);

        assert!(
            second_token.is_cancelled(),
            "dropping the active live-cancel guard should stop its parent bridge"
        );
        assert!(
            !runtime
                .state
                .lock()
                .expect("multi-agent state poisoned")
                .agent_cancel_tokens
                .contains_key("agent_test"),
            "dropping the active live-cancel guard should unregister its token"
        );
    }

    #[test]
    fn swarm_operations_use_canonical_child_state() {
        let runtime = MultiAgentRuntime::new();
        let swarm_id = runtime.new_swarm_id();
        let first = runtime.start_delegate(
            "first",
            None,
            AgentRole::Coder,
            AgentRunMode::Foreground,
            DelegateContext::None,
            AgentPathKind::SwarmChild(&swarm_id),
        );
        let second = runtime.start_delegate(
            "second",
            None,
            AgentRole::Coder,
            AgentRunMode::Foreground,
            DelegateContext::None,
            AgentPathKind::SwarmChild(&swarm_id),
        );
        runtime.register_swarm(crate::multi_agent::SwarmSnapshot {
            swarm_id: swarm_id.clone(),
            description: "test".to_owned(),
            role: AgentRole::Coder,
            mode: AgentRunMode::Foreground,
            state: AgentLifecycleState::Running,
            max_concurrency: 2,
            aggregate: SwarmAggregate::from_states([
                AgentLifecycleState::Running,
                AgentLifecycleState::Running,
            ]),
            children: vec![
                crate::multi_agent::SwarmChildSnapshot {
                    item_index: 0,
                    item: "first".to_owned(),
                    agent: first.clone(),
                },
                crate::multi_agent::SwarmChildSnapshot {
                    item_index: 1,
                    item: "second".to_owned(),
                    agent: second.clone(),
                },
            ],
        });
        runtime.cancel_agent(&first.id).expect("cancel first");
        let _ = runtime.complete_delegate_for_test(&second.id, "done");

        let projected = runtime.swarm_snapshot(&swarm_id).expect("projected swarm");
        assert_eq!(projected.aggregate.completed, 1);
        assert_eq!(projected.aggregate.cancelled, 1);
        assert_eq!(
            projected.children[0].agent.state,
            AgentLifecycleState::Cancelled
        );
        assert_eq!(runtime.list_swarms()[0].aggregate.completed, 1);
        assert_eq!(runtime.resumable_swarm_items(&swarm_id), vec![0]);

        let detached = runtime.detach_swarm(&swarm_id).expect("detach swarm");
        assert_eq!(detached.mode, AgentRunMode::Background);
        assert!(
            detached
                .children
                .iter()
                .all(|child| child.agent.mode == AgentRunMode::Background)
        );
        for agent_id in [&first.id, &second.id] {
            let canonical = runtime.snapshot(agent_id).expect("canonical child");
            assert_eq!(canonical.mode, AgentRunMode::Background);
            assert!(canonical.detached_from_foreground);
        }
    }

    #[test]
    fn child_finalization_is_atomic_and_always_persists_messages() {
        let runtime = MultiAgentRuntime::new();
        let child = runtime.start_foreground_delegate_for_test("cancelled child");
        runtime.cancel_agent(&child.id).expect("cancel child");
        let messages = vec![AgentMessage::user_text("keep this context")];

        let output =
            runtime.finish_child_run(child, Instant::now(), Ok((Vec::new(), messages.clone())));

        assert_eq!(output.snapshot.state, AgentLifecycleState::Cancelled);
        assert_eq!(
            runtime
                .snapshot(&output.snapshot.id)
                .expect("canonical child")
                .prior_messages,
            messages
        );

        let event_cancelled = runtime.start_foreground_delegate_for_test("event cancelled child");
        let event_messages = vec![AgentMessage::user_text("event cancel context")];
        let event_output = runtime.finish_child_run(
            event_cancelled,
            Instant::now(),
            Ok((
                vec![AgentEvent::RunFinished {
                    turn: 1,
                    stop_reason: StopReason::Cancelled,
                }],
                event_messages.clone(),
            )),
        );
        assert_eq!(event_output.snapshot.state, AgentLifecycleState::Cancelled);
        assert_eq!(
            runtime
                .snapshot(&event_output.snapshot.id)
                .expect("event-cancelled child")
                .prior_messages,
            event_messages
        );

        let completed = runtime.start_foreground_delegate_for_test("completed child");
        let completed_messages = vec![AgentMessage::user_text("completed context")];
        let completed_output = runtime.finish_child_run(
            completed,
            Instant::now(),
            Ok((Vec::new(), completed_messages.clone())),
        );
        assert_eq!(
            completed_output.snapshot.state,
            AgentLifecycleState::Completed
        );
        assert_eq!(
            runtime
                .snapshot(&completed_output.snapshot.id)
                .expect("completed child")
                .prior_messages,
            completed_messages
        );
        assert!(
            runtime
                .cancel_agent(&completed_output.snapshot.id)
                .is_none()
        );

        let mut errored = runtime.start_foreground_delegate_for_test("cancelled error child");
        errored.prior_messages = vec![AgentMessage::user_text("prior error context")];
        runtime
            .cancel_agent(&errored.id)
            .expect("cancel error child");
        let error_output = runtime.finish_child_run(
            errored.clone(),
            Instant::now(),
            Err("stream failed after cancellation".to_owned()),
        );
        assert_eq!(error_output.snapshot.state, AgentLifecycleState::Cancelled);
        assert_eq!(
            runtime
                .snapshot(&error_output.snapshot.id)
                .expect("cancelled error child")
                .prior_messages,
            errored.prior_messages
        );
    }
}
