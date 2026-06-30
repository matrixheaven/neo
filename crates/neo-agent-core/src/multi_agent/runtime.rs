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

use super::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome, DelegateMailbox,
    DisplayNamePool,
};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DelegateRequest {
    #[schemars(description = "Required non-empty task for the subagent.")]
    pub task: String,
    #[serde(default)]
    #[schemars(description = "Subagent role. Defaults to coder.")]
    pub role: AgentRole,
    #[serde(default)]
    #[schemars(description = "Run mode. Defaults to foreground.")]
    pub mode: AgentRunMode,
    #[serde(default = "default_context")]
    #[schemars(description = "Context mode: inherit, summary, or none. Defaults to inherit.")]
    pub context: DelegateContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DelegateContext {
    Inherit,
    Summary,
    None,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DelegateSwarmRequest {
    #[schemars(
        description = "Required non-empty human title for the swarm. Not injected into every child task."
    )]
    pub description: String,
    #[schemars(description = "Required non-empty list of child task items.")]
    pub items: Vec<String>,
    #[schemars(
        description = "Required child task template. It must contain {{item}} exactly once or more; each swarm item replaces {{item}}. Optionally include {{description}} to inject the swarm description. No other placeholders are supported."
    )]
    pub prompt_template: String,
    #[serde(default)]
    #[schemars(description = "Subagent role for each child. Defaults to coder.")]
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
    agents: BTreeMap<String, AgentSnapshot>,
    swarms: BTreeMap<String, super::SwarmSnapshot>,
    mailboxes: BTreeMap<String, DelegateMailbox>,
    steer_handles: BTreeMap<String, SteerInputHandle>,
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
        let snapshot = AgentSnapshot {
            id: id.clone(),
            display_name,
            path,
            role: AgentRole::Coder,
            mode: AgentRunMode::Foreground,
            state: AgentLifecycleState::Running,
            task: task.to_owned(),
            tool_count: 0,
            token_count: 0,
            elapsed: std::time::Duration::ZERO,
            latest_text: None,
            activity: Vec::new(),
            outcome: None,
        };
        state
            .agents
            .insert(id.as_str().to_owned(), snapshot.clone());
        snapshot
    }

    pub fn start_delegate(
        &self,
        task: &str,
        role: AgentRole,
        mode: AgentRunMode,
        path: AgentPathKind<'_>,
    ) -> AgentSnapshot {
        self.create_delegate(task, role, mode, path, AgentLifecycleState::Running)
    }

    pub fn queue_delegate(
        &self,
        task: &str,
        role: AgentRole,
        mode: AgentRunMode,
        path: AgentPathKind<'_>,
    ) -> AgentSnapshot {
        self.create_delegate(task, role, mode, path, AgentLifecycleState::Queued)
    }

    fn create_delegate(
        &self,
        task: &str,
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
        let snapshot = AgentSnapshot {
            id: id.clone(),
            display_name,
            path: agent_path,
            role,
            mode,
            state: lifecycle_state,
            task: task.to_owned(),
            tool_count: 0,
            token_count: 0,
            elapsed: Duration::ZERO,
            latest_text: None,
            activity: Vec::new(),
            outcome: None,
        };
        state
            .agents
            .insert(id.as_str().to_owned(), snapshot.clone());
        snapshot
    }

    pub fn mark_delegate_running(&self, id: &AgentId) -> Option<AgentSnapshot> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = state.agents.get_mut(id.as_str())?;
        snapshot.state = AgentLifecycleState::Running;
        Some(snapshot.clone())
    }

    pub fn complete_delegate(&self, id: &AgentId, update: AgentRunUpdate) -> AgentSnapshot {
        self.update_terminal_delegate(id, AgentLifecycleState::Completed, update, false)
    }

    pub fn fail_delegate(&self, id: &AgentId, update: AgentRunUpdate) -> AgentSnapshot {
        self.update_terminal_delegate(id, AgentLifecycleState::Failed, update, true)
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
        snapshot.state = state;
        snapshot.tool_count = update.tool_count;
        snapshot.token_count = update.token_count;
        snapshot.elapsed = update.elapsed;
        snapshot.latest_text.clone_from(&update.latest_text);
        snapshot.activity = update.activity;
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
        snapshot.state = AgentLifecycleState::Completed;
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
        }
        Some(snapshot.clone())
    }

    /// Register a swarm snapshot in the runtime state.
    pub fn register_swarm(&self, snapshot: super::SwarmSnapshot) {
        let swarm_id = snapshot.swarm_id.clone();
        self.state
            .lock()
            .expect("multi-agent state poisoned")
            .swarms
            .insert(swarm_id, snapshot);
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
    pub fn cancel_agent(&self, id: &AgentId) -> Option<AgentSnapshot> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = state.agents.get_mut(id.as_str())?;
        snapshot.state = AgentLifecycleState::Cancelled;
        Some(snapshot.clone())
    }

    /// Mark a running agent as cancelled by its string ID.
    pub fn cancel_agent_by_id(&self, id: &str) -> Option<AgentSnapshot> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let snapshot = state.agents.get_mut(id)?;
        snapshot.state = AgentLifecycleState::Cancelled;
        Some(snapshot.clone())
    }

    /// Mark every child in a swarm as cancelled.
    pub fn cancel_swarm_by_id(&self, swarm_id: &str) -> Option<super::SwarmSnapshot> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        let mut snapshot = state.swarms.get(swarm_id)?.clone();
        for child in &mut snapshot.children {
            child.agent.state = AgentLifecycleState::Cancelled;
            if let Some(agent) = state.agents.get_mut(child.agent.id.as_str()) {
                agent.state = AgentLifecycleState::Cancelled;
                child.agent = agent.clone();
            }
        }
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
            .filter(|agent| {
                include_completed
                    || !matches!(
                        agent.state,
                        AgentLifecycleState::Completed
                            | AgentLifecycleState::Failed
                            | AgentLifecycleState::Cancelled
                    )
            })
            .cloned()
            .collect()
    }

    /// Push a message to an existing agent's mailbox.
    pub fn push_mailbox_message(
        &self,
        agent_id: &str,
        text: String,
    ) -> Option<super::DelegateMailboxMessage> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        if !state.agents.contains_key(agent_id) {
            return None;
        }
        let mailbox = state.mailboxes.entry(agent_id.to_owned()).or_default();
        Some(mailbox.push(text))
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

    pub fn mark_mailbox_message_delivered(&self, agent_id: &str, message_id: &str) {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        if let Some(mailbox) = state.mailboxes.get_mut(agent_id) {
            mailbox.mark_delivered(message_id);
        }
    }

    pub fn take_pending_mailbox(&self, agent_id: &str) -> Vec<super::DelegateMailboxMessage> {
        let mut state = self.state.lock().expect("multi-agent state poisoned");
        state
            .mailboxes
            .get_mut(agent_id)
            .map_or_else(Vec::new, DelegateMailbox::take_pending)
    }

    pub fn pending_mailbox(&self, agent_id: &str) -> Vec<super::DelegateMailboxMessage> {
        let state = self.state.lock().expect("multi-agent state poisoned");
        state
            .mailboxes
            .get(agent_id)
            .map_or_else(Vec::new, DelegateMailbox::pending)
    }

    #[must_use]
    pub fn mailbox_pending_count(&self, agent_id: &str) -> usize {
        let state = self.state.lock().expect("multi-agent state poisoned");
        state
            .mailboxes
            .get(agent_id)
            .map_or(0, DelegateMailbox::pending_count)
    }

    #[must_use]
    pub fn latest_mailbox_message_id(&self, agent_id: &str) -> Option<String> {
        let state = self.state.lock().expect("multi-agent state poisoned");
        state
            .mailboxes
            .get(agent_id)
            .and_then(DelegateMailbox::latest_message_id)
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
                    agent: AgentSnapshot {
                        id: AgentId::from_suffix_for_test(&format!("{swarm_id}-{index}")),
                        display_name: name.clone(),
                        path: AgentPath::swarm_child(&swarm_id, &name),
                        role: AgentRole::Coder,
                        mode: AgentRunMode::Foreground,
                        state: lifecycle_state,
                        task: item.to_owned(),
                        tool_count: 0,
                        token_count: 0,
                        elapsed: std::time::Duration::ZERO,
                        latest_text: None,
                        activity: Vec::new(),
                        outcome: None,
                    },
                }
            })
            .collect();
        let swarm = super::SwarmSnapshot {
            swarm_id: swarm_id.clone(),
            description: "test swarm".to_owned(),
            mode: AgentRunMode::Foreground,
            max_concurrency: child_snapshots.len().max(1),
            children: child_snapshots,
        };
        state.swarms.insert(swarm_id.clone(), swarm);
        swarm_id
    }
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
    pub elapsed: Duration,
    pub latest_text: Option<String>,
    pub activity: Vec<AgentActivityEntry>,
}

#[derive(Debug, Clone)]
pub struct ChildRunOutput {
    pub snapshot: AgentSnapshot,
    pub events: Vec<AgentEvent>,
}

#[derive(Clone)]
pub struct ChildRuntimeDeps {
    pub config: AgentConfig,
    pub model: Arc<dyn ModelClient>,
    pub tools: Arc<ToolRegistry>,
}

impl ChildRuntimeDeps {
    #[must_use]
    pub fn new(config: AgentConfig, model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            config,
            model,
            tools,
        }
    }
}

impl MultiAgentRuntime {
    pub async fn run_child_turn(
        &self,
        deps: ChildRuntimeDeps,
        request: &DelegateRequest,
        mode: AgentRunMode,
    ) -> Result<ChildRunOutput, String> {
        let snapshot = self.start_delegate(&request.task, request.role, mode, AgentPathKind::Root);
        let started_at = Instant::now();
        let prompt = child_prompt(&request.task, request.context, request.role, &[]);
        let run = run_agent_snapshot(deps, prompt, SteerInputHandle::new(), |_| {}).await;
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
        let mailbox_messages = self.take_pending_mailbox(snapshot.id.as_str());
        let prompt = child_prompt(&snapshot.task, context, snapshot.role, &mailbox_messages);
        let runtime = self.clone();
        let agent_id = snapshot.id.clone();
        let steer_input = self.register_live_steer(agent_id.as_str());
        let run = run_agent_snapshot(deps, prompt, steer_input, |event| {
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
        let task = swarm_child_task(&request.prompt_template, item);
        let snapshot = self.start_delegate(
            &task,
            request.role,
            mode,
            AgentPathKind::SwarmChild(swarm_id),
        );
        let started_at = Instant::now();
        let prompt = child_prompt(&task, DelegateContext::None, request.role, &[]);
        let run = run_agent_snapshot(deps, prompt, SteerInputHandle::new(), |_| {}).await;
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
        let mailbox_messages = self.take_pending_mailbox(snapshot.id.as_str());
        let prompt = child_prompt(&snapshot.task, context, snapshot.role, &mailbox_messages);
        let runtime = self.clone();
        let agent_id = snapshot.id.clone();
        let steer_input = self.register_live_steer(agent_id.as_str());
        let run = run_agent_snapshot(deps, prompt, steer_input, |event| {
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
        let mut changed = false;
        match event {
            AgentEvent::ToolExecutionStarted {
                id,
                name,
                arguments,
                ..
            } => {
                changed = true;
                snapshot.activity.push(AgentActivityEntry {
                    kind: AgentActivityKind::Tool {
                        id: id.clone(),
                        name: name.clone(),
                        summary: summarize_tool_arguments(arguments),
                        failed: false,
                    },
                });
            }
            AgentEvent::ToolExecutionFinished {
                id, name, result, ..
            } => {
                changed = true;
                snapshot.tool_count = snapshot.tool_count.saturating_add(1);
                let failed = result.is_error;
                let summary = summarize_tool_result(name, result)
                    .or_else(|| last_tool_summary(snapshot.activity.as_slice(), id));
                upsert_tool_activity(&mut snapshot.activity, id, name, summary, failed);
            }
            AgentEvent::ToolExecutionUpdate {
                id,
                name,
                partial_result,
                ..
            } => {
                changed = true;
                upsert_tool_activity(
                    &mut snapshot.activity,
                    id,
                    name,
                    summarize_tool_result(name, partial_result),
                    partial_result.is_error,
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
                    push_text_activity(snapshot, &text, false);
                }
            }
            AgentEvent::TokenUsage { usage, .. } => {
                changed = true;
                snapshot.token_count = snapshot
                    .token_count
                    .saturating_add((usage.input_tokens + usage.output_tokens) as usize);
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
        run: Result<Vec<AgentEvent>, String>,
    ) -> ChildRunOutput {
        match run {
            Ok(events) => {
                if self
                    .snapshot(&snapshot.id)
                    .is_some_and(|current| current.state == AgentLifecycleState::Cancelled)
                {
                    return ChildRunOutput {
                        snapshot: self.snapshot(&snapshot.id).unwrap_or(snapshot),
                        events,
                    };
                }
                let update = summarize_child_events(&events, started_at.elapsed());
                let completed = if child_events_have_error(&events) {
                    self.fail_delegate(&snapshot.id, update)
                } else {
                    self.complete_delegate(&snapshot.id, update)
                };
                ChildRunOutput {
                    snapshot: completed,
                    events,
                }
            }
            Err(error) => {
                let update = AgentRunUpdate {
                    summary: error,
                    tool_count: 0,
                    token_count: 0,
                    elapsed: started_at.elapsed(),
                    latest_text: None,
                    activity: Vec::new(),
                };
                let failed = self.fail_delegate(&snapshot.id, update);
                ChildRunOutput {
                    snapshot: failed,
                    events: Vec::new(),
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
    steer_input: SteerInputHandle,
    mut on_event: impl FnMut(&AgentEvent) + Send,
) -> Result<Vec<AgentEvent>, String> {
    let child_config = child_config(deps.config);
    let child_runtime =
        AgentRuntime::with_shared_tools_and_configured_specs(child_config, deps.model, deps.tools)
            .with_steer_input(steer_input);
    let mut context = AgentContext::new();
    let mut stream = child_runtime.run_turn(&mut context, AgentMessage::user_text(prompt));
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        let event = event.map_err(|err| err.to_string())?;
        on_event(&event);
        events.push(event);
    }
    Ok(events)
}

fn child_config(mut config: AgentConfig) -> AgentConfig {
    config.system_prompt = Some(match config.system_prompt {
        Some(prompt) => format!("{prompt}\n\n{}", subagent_system_constraints()),
        None => subagent_system_constraints().to_owned(),
    });
    config.tools = config
        .tools
        .iter()
        .filter(|spec| !is_parent_orchestration_tool(&spec.name))
        .cloned()
        .collect();
    config.with_before_tool_call(block_forbidden_subagent_tool_call)
}

fn block_forbidden_subagent_tool_call(tool_call: &AgentToolCall) -> Option<crate::ToolResult> {
    if is_parent_orchestration_tool(&tool_call.name) {
        return Some(crate::ToolResult::error(format!(
            "Subagents may not call parent orchestration tool `{}`.",
            tool_call.name
        )));
    }

    let command = match tool_call.name.as_str() {
        "Bash" => tool_call
            .arguments
            .get("command")
            .and_then(serde_json::Value::as_str),
        "Terminal" => tool_call
            .arguments
            .get("command")
            .and_then(serde_json::Value::as_str),
        _ => None,
    }?;
    if is_forbidden_subagent_git_command(command) {
        return Some(crate::ToolResult::error(format!(
            "Subagents may not run git mutation commands: {command}"
        )));
    }
    None
}

fn is_parent_orchestration_tool(name: &str) -> bool {
    matches!(
        name,
        "Delegate"
            | "DelegateSwarm"
            | "ListDelegates"
            | "WaitDelegate"
            | "InterruptDelegate"
            | "MessageDelegate"
            | "RunWorkflow"
    )
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
                activity.push(AgentActivityEntry {
                    kind: AgentActivityKind::Tool {
                        id: id.clone(),
                        name: name.clone(),
                        summary: summarize_tool_arguments(arguments),
                        failed: false,
                    },
                });
            }
            AgentEvent::ToolExecutionFinished {
                id, name, result, ..
            } => {
                let summary = summarize_tool_result(name, result)
                    .or_else(|| tool_args.get(id).and_then(summarize_tool_arguments));
                upsert_tool_activity(&mut activity, id, name, summary, result.is_error);
            }
            AgentEvent::MessageAppended {
                message: AgentMessage::Assistant { content, .. },
            } => {
                let text = content_text(content);
                if !text.trim().is_empty() {
                    activity.push(AgentActivityEntry {
                        kind: AgentActivityKind::Text {
                            text,
                            thinking: false,
                        },
                    });
                }
            }
            AgentEvent::ThinkingDelta { text, .. } if !text.trim().is_empty() => {
                activity.push(AgentActivityEntry {
                    kind: AgentActivityKind::Text {
                        text: text.clone(),
                        thinking: true,
                    },
                });
            }
            _ => {}
        }
    }
    trim_activity(&mut activity);
    activity
}

fn push_text_activity(snapshot: &mut AgentSnapshot, text: &str, thinking: bool) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if !thinking {
        snapshot.latest_text = Some(text.to_owned());
    }
    snapshot.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: text.to_owned(),
            thinking,
        },
    });
}

fn upsert_tool_activity(
    activity: &mut Vec<AgentActivityEntry>,
    id: &str,
    name: &str,
    summary: Option<String>,
    failed: bool,
) {
    for entry in activity.iter_mut().rev() {
        let AgentActivityKind::Tool {
            id: entry_id,
            name: entry_name,
            summary: entry_summary,
            failed: entry_failed,
        } = &mut entry.kind
        else {
            continue;
        };
        if entry_id == id {
            if summary.is_some() {
                *entry_summary = summary;
            }
            *entry_name = name.to_owned();
            *entry_failed = failed;
            return;
        }
    }
    activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            summary,
            failed,
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
    if activity.len() > MAX_AGENT_ACTIVITY {
        let remove = activity.len() - MAX_AGENT_ACTIVITY;
        activity.drain(0..remove);
    }
}

fn child_prompt(
    task: &str,
    context: DelegateContext,
    role: AgentRole,
    mailbox_messages: &[super::DelegateMailboxMessage],
) -> String {
    let mut prompt = format!(
        "You are a bounded Neo subagent.\n\nRole: {role:?}\nTask: {task}\nContext mode: {}\n\nReturn a concise result for the parent agent. Do not perform git mutations. Do not run git add, git commit, git reset, git checkout, git restore, git stash, git clean, git rebase, git push, git rm, git branch, git switch, git merge, git cherry-pick, git tag, or git worktree.",
        context.as_str()
    );
    if !mailbox_messages.is_empty() {
        prompt.push_str("\n\nFollow-up messages delivered before this run:");
        for message in mailbox_messages {
            prompt.push_str(&format!("\n- [{}] {}", message.id, message.text));
        }
    }
    prompt
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

/// Deny git subcommands that mutate state when issued by a subagent.
/// Read-only commands (`status`, `diff`, `log`, `blame`, etc.) are allowed.
#[must_use]
pub fn is_forbidden_subagent_git_command(command: &str) -> bool {
    let trimmed = command.trim();
    let Some(rest) = trimmed.strip_prefix("git ") else {
        return false;
    };
    let first = rest.split_whitespace().next().unwrap_or_default();
    matches!(
        first,
        "add"
            | "am"
            | "apply"
            | "branch"
            | "checkout"
            | "cherry-pick"
            | "clean"
            | "commit"
            | "filter-branch"
            | "gc"
            | "merge"
            | "mv"
            | "push"
            | "rebase"
            | "reflog"
            | "reset"
            | "restore"
            | "rm"
            | "stash"
            | "switch"
            | "tag"
            | "worktree"
    )
}

// Keep `Instant` imported for future elapsed-time tracking in P2.
const _: fn() = || {
    let _ = Instant::now();
};
