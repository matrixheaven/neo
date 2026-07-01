use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures::StreamExt;
use neo_ai::ModelClient;
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use crate::runtime::{ActiveTurnInput, AgentConfig, AgentContext, SteerInputHandle};
use crate::{AgentEvent, AgentMessage, AgentRuntime, AgentToolCall, Content, ToolRegistry};

use super::state::derive_title;
use super::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome, DisplayNamePool,
    SwarmAggregate,
};
use super::{AgentTerminalReason, AgentToolActivityPhase, AgentToolOutputPreview};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DelegateContext {
    Inherit,
    Summary,
    None,
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
        description = "New child task items as JSON objects with title and value. value is inserted into prompt_template as {{item}}."
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
    agent_order: BTreeMap<String, u64>,
    swarm_order: BTreeMap<String, u64>,
    agents: BTreeMap<String, AgentSnapshot>,
    swarms: BTreeMap<String, super::SwarmSnapshot>,
    steer_handles: BTreeMap<String, SteerInputHandle>,
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
}

#[derive(Debug, Clone, Default)]
pub struct MultiAgentRuntime {
    state: Arc<Mutex<MultiAgentState>>,
}

impl MultiAgentRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

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

    pub fn start_delegate(
        &self,
        task: &str,
        title: Option<&str>,
        role: AgentRole,
        mode: AgentRunMode,
        path: AgentPathKind<'_>,
    ) -> AgentSnapshot {
        self.create_delegate(task, title, role, mode, path, AgentLifecycleState::Running)
    }

    pub fn queue_delegate(
        &self,
        task: &str,
        title: Option<&str>,
        role: AgentRole,
        mode: AgentRunMode,
        path: AgentPathKind<'_>,
    ) -> AgentSnapshot {
        self.create_delegate(task, title, role, mode, path, AgentLifecycleState::Queued)
    }

    fn create_delegate(
        &self,
        task: &str,
        title: Option<&str>,
        role: AgentRole,
        mode: AgentRunMode,
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

    pub fn mark_delegate_running(&self, id: &AgentId) -> Option<AgentSnapshot> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = state.agents.get_mut(id.as_str())?;
        let now = now_ms();
        snapshot.state = AgentLifecycleState::Running;
        snapshot.started_at_ms.get_or_insert(now);
        snapshot.terminal_at_ms = None;
        snapshot.terminal_reason = None;
        snapshot.updated_at_ms = now;
        Some(snapshot.clone())
    }

    pub fn complete_delegate(&self, id: &AgentId, update: AgentRunUpdate) -> AgentSnapshot {
        self.update_terminal_delegate(id, AgentLifecycleState::Completed, update, false)
    }

    /// Like `complete_delegate` but also stores the accumulated conversation
    /// messages on the snapshot so they survive a future resume.
    fn complete_delegate_with_messages(
        &self,
        id: &AgentId,
        update: AgentRunUpdate,
        messages: &[AgentMessage],
    ) -> AgentSnapshot {
        let mut locked = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = locked
            .agents
            .get_mut(id.as_str())
            .expect("agent should exist");
        let now = now_ms();
        snapshot.state = AgentLifecycleState::Completed;
        snapshot.tool_count = update.tool_count;
        snapshot.token_count = update.token_count;
        snapshot.cache_read_token_count = update.cache_read_token_count;
        snapshot.cache_write_token_count = update.cache_write_token_count;
        snapshot.elapsed = update.elapsed;
        snapshot.latest_text.clone_from(&update.latest_text);
        snapshot.activity = update.activity;
        snapshot.prior_messages = messages.to_vec();
        snapshot.terminal_at_ms.get_or_insert(now);
        snapshot.updated_at_ms = now;
        snapshot.terminal_reason = Some(terminal_reason_for_state(AgentLifecycleState::Completed));
        snapshot.outcome = Some(AgentTerminalOutcome {
            summary: update.summary,
            is_error: false,
        });
        snapshot.clone()
    }

    pub fn fail_delegate(&self, id: &AgentId, update: AgentRunUpdate) -> AgentSnapshot {
        self.update_terminal_delegate(id, AgentLifecycleState::Failed, update, true)
    }

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
            snapshot.latest_text = Some(message.clone());
            snapshot.outcome = Some(AgentTerminalOutcome {
                summary: message,
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
        let now = now_ms();
        snapshot.state = state;
        snapshot.tool_count = update.tool_count;
        snapshot.token_count = update.token_count;
        snapshot.cache_read_token_count = update.cache_read_token_count;
        snapshot.cache_write_token_count = update.cache_write_token_count;
        snapshot.elapsed = update.elapsed;
        snapshot.latest_text.clone_from(&update.latest_text);
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
        let snapshot = state.swarms.get_mut(swarm_id)?;
        snapshot.mode = AgentRunMode::Background;
        for child in &mut snapshot.children {
            child.agent.mode = AgentRunMode::Background;
            child.agent.detached_from_foreground = true;
            child.agent.updated_at_ms = now_ms();
        }
        Some(snapshot.clone())
    }

    /// Register a swarm snapshot in the runtime state.
    pub fn register_swarm(&self, snapshot: super::SwarmSnapshot) {
        let swarm_id = snapshot.swarm_id.clone();
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        state.register_swarm_order(&swarm_id);
        state.swarms.insert(swarm_id, snapshot);
    }

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
    /// terminal.
    pub fn cancel_agent(&self, id: &AgentId) -> Option<AgentSnapshot> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = state.agents.get_mut(id.as_str())?;
        if snapshot.state.is_terminal() {
            return None;
        }
        let now = now_ms();
        snapshot.state = AgentLifecycleState::Cancelled;
        snapshot.terminal_at_ms.get_or_insert(now);
        snapshot.updated_at_ms = now;
        snapshot.terminal_reason = Some(AgentTerminalReason::CancelledByUser);
        Some(snapshot.clone())
    }

    /// Mark a running agent as cancelled by its string ID.
    ///
    /// Returns `None` and leaves the state unchanged if the agent is already
    /// terminal.
    pub fn cancel_agent_by_id(&self, id: &str) -> Option<AgentSnapshot> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = state.agents.get_mut(id)?;
        if snapshot.state.is_terminal() {
            return None;
        }
        let now = now_ms();
        snapshot.state = AgentLifecycleState::Cancelled;
        snapshot.terminal_at_ms.get_or_insert(now);
        snapshot.updated_at_ms = now;
        snapshot.terminal_reason = Some(AgentTerminalReason::CancelledByUser);
        Some(snapshot.clone())
    }

    /// Mark every non-terminal child in a swarm as cancelled.
    ///
    /// Returns `None` and leaves the state unchanged if the swarm does not
    /// exist or all of its children are already terminal.
    pub fn cancel_swarm_by_id(&self, swarm_id: &str) -> Option<super::SwarmSnapshot> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let mut snapshot = state.swarms.get(swarm_id)?.clone();
        let mut changed = false;
        for child in &mut snapshot.children {
            if child.agent.state.is_terminal() {
                continue;
            }
            let now = now_ms();
            child.agent.state = AgentLifecycleState::Cancelled;
            child.agent.terminal_at_ms.get_or_insert(now);
            child.agent.updated_at_ms = now;
            child.agent.terminal_reason = Some(AgentTerminalReason::CancelledByUser);
            if let Some(agent) = state.agents.get_mut(child.agent.id.as_str()) {
                agent.state = AgentLifecycleState::Cancelled;
                agent.terminal_at_ms.get_or_insert(now);
                agent.updated_at_ms = now;
                agent.terminal_reason = Some(AgentTerminalReason::CancelledByUser);
                child.agent = agent.clone();
            }
            changed = true;
        }
        if !changed {
            return None;
        }
        state.register_swarm_order(swarm_id);
        state.swarms.insert(swarm_id.to_owned(), snapshot.clone());
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
        agent.state = AgentLifecycleState::Running;
        agent.mode = request.mode;
        agent.task = request.task.clone();
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
        Ok(agent.clone())
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
        let Some(swarm) = state.swarms.get(swarm_id) else {
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
        let mut swarm = self
            .state
            .lock()
            .expect("multi-agent state poisoned")
            .swarms
            .get(swarm_id)?
            .clone();
        refresh_swarm(&mut swarm);
        Some(swarm)
    }

    /// List all swarm snapshots in the runtime.
    #[must_use]
    pub fn list_swarms(&self) -> Vec<super::SwarmSnapshot> {
        let state = self.state.lock().expect("multi-agent state poisoned");
        let mut swarms: Vec<_> = state.swarms.values().cloned().collect();
        for swarm in &mut swarms {
            refresh_swarm(swarm);
        }
        swarms
    }

    /// Cancel all non-terminal children in a swarm.
    ///
    /// Returns the refreshed snapshot, or an error message if the swarm is
    /// unknown or already terminal.
    pub fn cancel_swarm(&self, swarm_id: &str) -> Result<super::SwarmSnapshot, String> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let swarm = state
            .swarms
            .get_mut(swarm_id)
            .ok_or_else(|| format!("unknown delegate target `{swarm_id}`"))?;
        if swarm.state.is_terminal() {
            return Err(format!(
                "swarm already {}; terminal swarm state is immutable",
                swarm.state.as_str()
            ));
        }
        // Collect the child agent ids that need cancelling before borrowing
        // state.agents separately.
        let cancelled_ids: Vec<String> = swarm
            .children
            .iter()
            .filter(|child| !child.agent.state.is_terminal())
            .map(|child| child.agent.id.as_str().to_owned())
            .collect();
        for child in &mut swarm.children {
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
        for agent_id in &cancelled_ids {
            if let Some(agent) = state.agents.get_mut(agent_id) {
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
        let swarm = state.swarms.get_mut(swarm_id).expect("swarm exists");
        refresh_swarm(swarm);
        Ok(swarm.clone())
    }

    /// Broadcast a live message to all running children in a swarm.
    ///
    /// Returns `(delivered, skipped)` on success, or an error if the swarm is
    /// unknown or has no running children.
    pub fn broadcast_live_swarm_message(
        &self,
        swarm_id: &str,
        message: String,
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
                    text: message.clone(),
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
        if delivered.is_empty() {
            return Err(
                "swarm has no running children; use DelegateSwarm with resume_agent_ids to continue unfinished children"
                    .to_owned(),
            );
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

struct AgentSnapshotSeed<'a> {
    id: AgentId,
    display_name: AgentDisplayName,
    path: AgentPath,
    role: AgentRole,
    mode: AgentRunMode,
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
        AgentLifecycleState::Failed => AgentTerminalReason::Error,
        AgentLifecycleState::Cancelled => AgentTerminalReason::CancelledByUser,
        AgentLifecycleState::TimedOut => AgentTerminalReason::TimedOut,
        AgentLifecycleState::Queued | AgentLifecycleState::Running => AgentTerminalReason::Error,
    }
}

#[derive(Clone)]
pub struct ChildRuntimeDeps {
    pub config: AgentConfig,
    pub model: Arc<dyn ModelClient>,
    pub tools: Arc<ToolRegistry>,
    pub role: AgentRole,
}

impl ChildRuntimeDeps {
    #[must_use]
    pub fn new(config: AgentConfig, model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            config,
            model,
            tools,
            role: AgentRole::Coder,
        }
    }

    /// Set the subagent role for tool filtering and profile enforcement.
    #[must_use]
    pub fn with_role(mut self, role: AgentRole) -> Self {
        self.role = role;
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
            AgentPathKind::Root,
        );
        let started_at = Instant::now();
        let prompt = child_prompt(&request.task, request.context, request.actual_role());
        let run =
            run_agent_snapshot(deps, prompt, Vec::new(), SteerInputHandle::new(), |_| {}).await;
        Ok(self.finish_child_run(snapshot, started_at, run))
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
        let prompt = child_prompt(&snapshot.task, context, snapshot.role);
        let prior_messages = snapshot.prior_messages.clone();
        let runtime = self.clone();
        let agent_id = snapshot.id.clone();
        let steer_input = self.register_live_steer(agent_id.as_str());
        let run = run_agent_snapshot(deps, prompt, prior_messages, steer_input, |event| {
            if let Some(updated) = runtime.apply_child_event(&agent_id, started_at, event) {
                on_update(updated);
            }
        })
        .await;
        self.unregister_live_steer(agent_id.as_str());
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
            AgentPathKind::SwarmChild(swarm_id),
        );
        let started_at = Instant::now();
        let prompt = child_prompt(&task, DelegateContext::None, request.role);
        let run =
            run_agent_snapshot(deps, prompt, Vec::new(), SteerInputHandle::new(), |_| {}).await;
        Ok(self.finish_child_run(snapshot, started_at, run))
    }

    pub async fn run_started_swarm_child_turn<F>(
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
        let prompt = child_prompt(&snapshot.task, context, snapshot.role);
        let prior_messages = snapshot.prior_messages.clone();
        let runtime = self.clone();
        let agent_id = snapshot.id.clone();
        let steer_input = self.register_live_steer(agent_id.as_str());
        let run = run_agent_snapshot(deps, prompt, prior_messages, steer_input, |event| {
            if let Some(updated) = runtime.apply_child_event(&agent_id, started_at, event) {
                on_update(updated);
            }
        })
        .await;
        self.unregister_live_steer(agent_id.as_str());
        self.finish_child_run(snapshot, started_at, run)
    }

    pub fn apply_child_event(
        &self,
        id: &AgentId,
        started_at: Instant,
        event: &AgentEvent,
    ) -> Option<AgentSnapshot> {
        let mut locked = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = locked.agents.get_mut(id.as_str())?;
        snapshot.elapsed = started_at.elapsed();
        snapshot.updated_at_ms = now_ms();
        let mut changed = false;
        match event {
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
                    summarize_tool_arguments(arguments),
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
                let summary = summarize_tool_result(name, result)
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
                let summary = summarize_tool_result(name, partial_result)
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
                push_text_activity(snapshot, text, false);
            }
            AgentEvent::ThinkingDelta { text, .. } => {
                changed = true;
                push_text_activity(snapshot, text, true);
            }
            AgentEvent::MessageAppended {
                message: AgentMessage::Assistant { content, .. },
            } => {
                let text = content_text(content);
                if !text.trim().is_empty() {
                    changed = true;
                    snapshot.latest_text = Some(text.clone());
                    if latest_text_activity(snapshot.activity.as_slice(), false).as_deref()
                        != Some(text.trim())
                    {
                        push_text_activity(snapshot, &text, false);
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
            AgentEvent::Error { message, .. } => {
                changed = true;
                snapshot.latest_text = Some(message.clone());
                snapshot.activity.push(AgentActivityEntry {
                    kind: AgentActivityKind::Text {
                        text: message.clone(),
                        thinking: false,
                    },
                });
            }
            _ => {}
        }
        if !changed {
            return None;
        }
        trim_activity(&mut snapshot.activity);
        Some(snapshot.clone())
    }

    fn finish_child_run(
        &self,
        snapshot: AgentSnapshot,
        started_at: Instant,
        run: Result<(Vec<AgentEvent>, Vec<AgentMessage>), String>,
    ) -> ChildRunOutput {
        match run {
            Ok((events, messages)) => {
                if self
                    .snapshot(&snapshot.id)
                    .is_some_and(|current| current.state == AgentLifecycleState::Cancelled)
                {
                    let mut current = self.snapshot(&snapshot.id).unwrap_or(snapshot);
                    current.prior_messages = messages.clone();
                    return ChildRunOutput {
                        snapshot: current,
                        events,
                        messages,
                    };
                }
                let update = summarize_child_events(&events, started_at.elapsed());
                let completed = if child_events_have_error(&events) {
                    self.fail_delegate(&snapshot.id, update)
                } else {
                    self.complete_delegate_with_messages(&snapshot.id, update, &messages)
                };
                ChildRunOutput {
                    snapshot: completed,
                    events,
                    messages,
                }
            }
            Err(error) => {
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
                let failed = self.fail_delegate(&snapshot.id, update);
                ChildRunOutput {
                    snapshot: failed,
                    events: Vec::new(),
                    messages: Vec::new(),
                }
            }
        }
    }

    fn register_live_steer(&self, agent_id: &str) -> SteerInputHandle {
        let handle = SteerInputHandle::new();
        self.state
            .lock()
            .expect("multi-agent state poisoned")
            .steer_handles
            .insert(agent_id.to_owned(), handle.clone());
        handle
    }

    fn unregister_live_steer(&self, agent_id: &str) {
        self.state
            .lock()
            .expect("multi-agent state poisoned")
            .steer_handles
            .remove(agent_id);
    }
}

async fn run_agent_snapshot(
    deps: ChildRuntimeDeps,
    prompt: String,
    prior_messages: Vec<AgentMessage>,
    steer_input: SteerInputHandle,
    mut on_event: impl FnMut(&AgentEvent) + Send,
) -> Result<(Vec<AgentEvent>, Vec<AgentMessage>), String> {
    let child_config = child_config(deps.config, deps.role);
    let child_tools = Arc::new(deps.tools.filtered_for_agent_role(deps.role));
    let child_runtime =
        AgentRuntime::with_shared_tools_and_configured_specs(child_config, deps.model, child_tools)
            .with_steer_input(steer_input);
    let mut context = AgentContext::new();
    for message in prior_messages {
        context.append_message(message);
    }
    let mut stream = child_runtime.run_turn(&mut context, AgentMessage::user_text(prompt));
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        let event = event.map_err(|err| err.to_string())?;
        on_event(&event);
        events.push(event);
    }
    drop(stream);
    // Extract the accumulated messages (prior + this turn) so they can be
    // stored on the snapshot for future resume.
    let messages = context.messages().to_vec();
    Ok((events, messages))
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
    // Filter model-visible tool specs: remove standard Neo tools not in the
    // role's allowed set. Keep unknown/custom tools so test probes and
    // MCP tools are not stripped.
    config.tools = config
        .tools
        .iter()
        .filter(|spec| {
            if !is_standard_neo_tool(&spec.name) {
                return true;
            }
            profile.allowed_tools.contains(spec.name.as_str())
        })
        .cloned()
        .collect();
    let profile_clone = super::profile::AgentProfile::for_role(role);
    config.with_before_tool_call(move |tool_call| {
        block_forbidden_subagent_tool_call(tool_call, &profile_clone)
    })
}

fn is_standard_neo_tool(name: &str) -> bool {
    matches!(
        name,
        "Read"
            | "List"
            | "Grep"
            | "Find"
            | "Glob"
            | "Bash"
            | "Write"
            | "Edit"
            | "TodoList"
            | "Terminal"
            | "TaskList"
            | "TaskOutput"
            | "TaskStop"
            | "EnterPlanMode"
            | "ExitPlanMode"
            | "Delegate"
            | "DelegateSwarm"
            | "ListDelegates"
            | "WaitDelegate"
            | "InterruptDelegate"
            | "MessageDelegate"
            | "RunWorkflow"
    )
}

fn block_forbidden_subagent_tool_call(
    tool_call: &AgentToolCall,
    profile: &super::profile::AgentProfile,
) -> Option<crate::ToolResult> {
    // Shell access (Bash/Terminal): denied unless the role's policy allows it.
    // Read-only behavior for Explorer/Reviewer is enforced by the profile's
    // `prompt_addendum`, not by command-syntax classification — see profile.rs.
    let is_shell = matches!(tool_call.name.as_str(), "Bash" | "Terminal");
    if is_shell && !profile.tool_policy.allow_shell {
        return Some(crate::ToolResult::error(format!(
            "{} agents may not run shell commands",
            profile.display_label
        )));
    }

    // File writes (Write/Edit): denied unless the role's policy allows it.
    if matches!(tool_call.name.as_str(), "Write" | "Edit") && !profile.tool_policy.allow_file_writes
    {
        return Some(crate::ToolResult::error(format!(
            "{} agents may not edit or write files",
            profile.display_label
        )));
    }

    None
}

fn summarize_child_events(events: &[AgentEvent], elapsed: Duration) -> AgentRunUpdate {
    let latest_text = latest_assistant_text(events);
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

fn latest_assistant_text(events: &[AgentEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| {
        let AgentEvent::MessageAppended {
            message: AgentMessage::Assistant { content, .. },
        } = event
        else {
            return None;
        };
        let text = content
            .iter()
            .filter_map(|part| match part {
                Content::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        (!text.trim().is_empty()).then_some(text)
    })
}

fn summarize_child_activity(events: &[AgentEvent]) -> Vec<AgentActivityEntry> {
    let mut activity = Vec::new();
    let mut tool_args: HashMap<String, serde_json::Value> = HashMap::new();
    for event in events {
        match event {
            AgentEvent::ToolExecutionStarted {
                id,
                name,
                arguments,
                ..
            } => {
                tool_args.insert(id.clone(), arguments.clone());
                upsert_tool_activity(
                    &mut activity,
                    id,
                    name,
                    summarize_tool_arguments(arguments),
                    AgentToolActivityPhase::Ongoing,
                    None,
                );
            }
            AgentEvent::ToolExecutionFinished {
                id, name, result, ..
            } => {
                let phase = if result.is_error {
                    AgentToolActivityPhase::Failed
                } else {
                    AgentToolActivityPhase::Done
                };
                let summary = summarize_tool_result(name, result)
                    .or_else(|| tool_args.get(id).and_then(summarize_tool_arguments));
                upsert_tool_activity(
                    &mut activity,
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
                let summary = summarize_tool_result(name, partial_result)
                    .or_else(|| tool_args.get(id).and_then(summarize_tool_arguments));
                upsert_tool_activity(
                    &mut activity,
                    id,
                    name,
                    summary,
                    AgentToolActivityPhase::Ongoing,
                    tool_output_preview(name, partial_result, true),
                );
            }
            AgentEvent::MessageAppended {
                message: AgentMessage::Assistant { content, .. },
            } => {
                let text = content_text(content);
                if !text.trim().is_empty()
                    && latest_text_activity(activity.as_slice(), false).as_deref()
                        != Some(text.trim())
                {
                    append_text_activity(&mut activity, &text, false);
                }
            }
            AgentEvent::ThinkingDelta { text, .. } if !text.trim().is_empty() => {
                append_text_activity(&mut activity, text, true);
            }
            AgentEvent::TextDelta { text, .. } if !text.trim().is_empty() => {
                append_text_activity(&mut activity, text, false);
            }
            _ => {}
        }
    }
    trim_activity(&mut activity);
    activity
}

fn push_text_activity(snapshot: &mut AgentSnapshot, text: &str, thinking: bool) {
    let Some(accumulated) = append_text_activity(&mut snapshot.activity, text, thinking) else {
        return;
    };

    if !thinking {
        snapshot.latest_text = Some(accumulated);
    }
}

fn append_text_activity(
    activity: &mut Vec<AgentActivityEntry>,
    text: &str,
    thinking: bool,
) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    if let Some(AgentActivityEntry {
        kind:
            AgentActivityKind::Text {
                text: previous,
                thinking: previous_thinking,
            },
    }) = activity.last_mut()
        && *previous_thinking == thinking
    {
        previous.push_str(text);
        return Some(previous.trim().to_owned());
    }

    activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: text.to_owned(),
            thinking,
        },
    });
    Some(text.trim().to_owned())
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
            *entry_name = name.to_owned();
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

fn summarize_tool_arguments(arguments: &serde_json::Value) -> Option<String> {
    for key in [
        "file_path",
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

fn summarize_tool_result(name: &str, result: &crate::ToolResult) -> Option<String> {
    if matches!(name, "Bash" | "Terminal") {
        result
            .content
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(compact_line)
    } else {
        None
    }
}

const MAX_AGENT_TOOL_OUTPUT_PREVIEW_BYTES: usize = 50_000;

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
            Content::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>()
}

fn compact_line(text: &str) -> String {
    let mut line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 96;
    if line.chars().count() > MAX {
        line = format!(
            "{}...",
            line.chars().take(MAX.saturating_sub(3)).collect::<String>()
        );
    }
    line
}

fn trim_activity(activity: &mut Vec<AgentActivityEntry>) {
    const MAX_AGENT_ACTIVITY: usize = 24;
    if activity.len() <= MAX_AGENT_ACTIVITY {
        return;
    }
    let mut keep = vec![false; activity.len()];
    for (index, entry) in activity.iter().enumerate().rev() {
        if matches!(
            entry.kind,
            AgentActivityKind::Tool {
                phase: AgentToolActivityPhase::Ongoing,
                ..
            }
        ) {
            keep[index] = true;
        }
    }
    let ongoing_count = keep.iter().filter(|value| **value).count();
    let mut remaining = MAX_AGENT_ACTIVITY.saturating_sub(ongoing_count);
    for index in (0..activity.len()).rev() {
        if keep[index] {
            continue;
        }
        if remaining > 0 {
            keep[index] = true;
            remaining -= 1;
        }
    }
    let mut index = 0usize;
    activity.retain(|_| {
        let retain = keep[index];
        index += 1;
        retain
    });
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
