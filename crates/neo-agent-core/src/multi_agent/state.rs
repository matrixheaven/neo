use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::AgentMessage;

use super::{AgentDisplayName, AgentId, AgentPath, AgentRole};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunMode {
    #[default]
    Foreground,
    Background,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DelegateContext {
    #[default]
    Inherit,
    Summary,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Interrupted,
}

impl AgentLifecycleState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut | Self::Interrupted
        )
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentTerminalOutcome {
    pub summary: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolActivityPhase {
    Queued {
        position: Option<usize>,
        queued_at_ms: u64,
    },
    Ongoing,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentToolOutputPreview {
    pub text: String,
    pub is_error: bool,
    pub truncated: bool,
    pub tail: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolFileOperation {
    Edited,
    Created,
    Overwritten,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolFileStatus {
    Pending,
    Committed,
    CommittedUnsynced,
    Failed,
    NotAttempted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentToolFileChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<AgentToolFileOperation>,
    pub status: AgentToolFileStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentTerminalReason {
    Completed,
    Error,
    CancelledByUser,
    TimedOut,
    Killed,
    Lost,
    ProcessExited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentActivityKind {
    Tool {
        id: String,
        name: String,
        summary: Option<String>,
        phase: AgentToolActivityPhase,
        output: Option<AgentToolOutputPreview>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<AgentToolFileChange>,
    },
    Text {
        text: String,
        thinking: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentActivityEntry {
    pub kind: AgentActivityKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentSnapshot {
    pub id: AgentId,
    pub display_name: AgentDisplayName,
    pub path: AgentPath,
    pub role: AgentRole,
    pub mode: AgentRunMode,
    #[serde(default)]
    pub context: DelegateContext,
    pub state: AgentLifecycleState,
    pub task: String,
    #[serde(default)]
    pub task_title: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub terminal_at_ms: Option<u64>,
    pub detached_from_foreground: bool,
    pub terminal_reason: Option<AgentTerminalReason>,
    pub run_count: usize,
    pub live_messages_received: usize,
    pub previous_status: Option<AgentLifecycleState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terminal_status_history: Vec<AgentLifecycleState>,
    pub resumed_from: Option<AgentId>,
    pub tool_count: usize,
    pub token_count: usize,
    #[serde(default)]
    pub cache_read_token_count: usize,
    #[serde(default)]
    pub cache_write_token_count: usize,
    pub elapsed: Duration,
    pub latest_text: Option<String>,
    #[serde(default)]
    pub activity: Vec<AgentActivityEntry>,
    /// Prior conversation messages accumulated across previous runs of this
    /// agent. On resume, these are replayed into the fresh `AgentContext` so
    /// the model retains conversation history.
    #[serde(default, skip)]
    pub prior_messages: Vec<AgentMessage>,
    pub outcome: Option<AgentTerminalOutcome>,
}

impl AgentSnapshot {
    /// Return the configured title, falling back to a short derived title from
    /// the task when the title is missing or empty.
    #[must_use]
    pub fn display_title(&self) -> String {
        self.task_title.clone()
    }

    #[must_use]
    pub fn progress_snapshot(&self) -> AgentProgressSnapshot {
        AgentProgressSnapshot::from_agent(self)
    }

    pub(crate) fn clear_live_queue_metadata(&mut self) {
        clear_live_queue_metadata_from_activity(&mut self.activity);
    }
}

pub(crate) fn derive_title(task: &str, provided: Option<&str>) -> String {
    const MAX_TITLE_LEN: usize = 80;
    if let Some(title) = provided {
        let trimmed = title.trim();
        if !trimmed.is_empty() {
            return trimmed.to_owned();
        }
    }
    let trimmed = task.trim();
    let line = trimmed.lines().next().unwrap_or(trimmed).trim();
    if line.chars().count() > MAX_TITLE_LEN {
        line.chars().take(MAX_TITLE_LEN - 3).collect::<String>() + "..."
    } else {
        line.to_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwarmChildSnapshot {
    pub item_index: usize,
    pub item: String,
    pub agent: AgentSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DelegateToolProgress {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub phase: AgentToolActivityPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<AgentToolOutputPreview>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<AgentToolFileChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentProgressSnapshot {
    pub agent_id: AgentId,
    pub state: AgentLifecycleState,
    pub mode: AgentRunMode,
    pub detached_from_foreground: bool,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<AgentTerminalReason>,
    pub run_count: usize,
    #[serde(default)]
    pub live_messages_received: usize,
    pub tool_count: usize,
    pub token_count: usize,
    #[serde(default)]
    pub cache_read_token_count: usize,
    #[serde(default)]
    pub cache_write_token_count: usize,
    pub elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool: Option<DelegateToolProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AgentTerminalOutcome>,
}

impl AgentProgressSnapshot {
    #[must_use]
    pub fn from_agent(agent: &AgentSnapshot) -> Self {
        Self {
            agent_id: agent.id.clone(),
            state: agent.state,
            mode: agent.mode,
            detached_from_foreground: agent.detached_from_foreground,
            updated_at_ms: agent.updated_at_ms,
            terminal_at_ms: agent.terminal_at_ms,
            terminal_reason: agent.terminal_reason,
            run_count: agent.run_count,
            live_messages_received: agent.live_messages_received,
            tool_count: agent.tool_count,
            token_count: agent.token_count,
            cache_read_token_count: agent.cache_read_token_count,
            cache_write_token_count: agent.cache_write_token_count,
            elapsed_ms: duration_millis_u64(agent.elapsed),
            latest_text: agent
                .latest_text
                .as_deref()
                .map(|text| truncate_progress_text(text, MAX_PROGRESS_TEXT_CHARS)),
            last_tool: agent
                .activity
                .iter()
                .rev()
                .find_map(|entry| match &entry.kind {
                    AgentActivityKind::Tool {
                        id,
                        name,
                        summary,
                        phase,
                        output,
                        files,
                    } => Some(DelegateToolProgress {
                        id: id.clone(),
                        name: name.clone(),
                        summary: summary
                            .as_deref()
                            .map(|text| truncate_progress_text(text, MAX_TOOL_SUMMARY_CHARS)),
                        phase: *phase,
                        output: output.clone(),
                        files: files.clone(),
                    }),
                    AgentActivityKind::Text { .. } => None,
                }),
            outcome: agent.outcome.as_ref().map(|outcome| AgentTerminalOutcome {
                summary: truncate_progress_text(&outcome.summary, MAX_PROGRESS_TEXT_CHARS),
                is_error: outcome.is_error,
            }),
        }
    }

    #[must_use]
    pub fn signature(&self) -> AgentProgressSignature {
        AgentProgressSignature {
            state: self.state,
            mode: self.mode,
            detached_from_foreground: self.detached_from_foreground,
            terminal_reason: self.terminal_reason,
            run_count: self.run_count,
            live_messages_received: self.live_messages_received,
            tool_count: self.tool_count,
            token_count: self.token_count,
            cache_read_token_count: self.cache_read_token_count,
            cache_write_token_count: self.cache_write_token_count,
            latest_text: self.latest_text.clone(),
            last_tool: self.last_tool.clone(),
            outcome: self.outcome.clone(),
        }
    }

    pub(crate) fn clear_live_queue_metadata(&mut self) {
        if let Some(tool) = &mut self.last_tool {
            clear_live_queue_metadata_from_phase(&mut tool.phase);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProgressSignature {
    pub state: AgentLifecycleState,
    pub mode: AgentRunMode,
    pub detached_from_foreground: bool,
    pub terminal_reason: Option<AgentTerminalReason>,
    pub run_count: usize,
    pub live_messages_received: usize,
    pub tool_count: usize,
    pub token_count: usize,
    pub cache_read_token_count: usize,
    pub cache_write_token_count: usize,
    pub latest_text: Option<String>,
    pub last_tool: Option<DelegateToolProgress>,
    pub outcome: Option<AgentTerminalOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwarmChildProgress {
    pub item_index: usize,
    pub progress: AgentProgressSnapshot,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwarmAggregate {
    pub total: usize,
    pub queued: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub timed_out: usize,
}

impl SwarmAggregate {
    #[must_use]
    pub fn from_states(states: impl IntoIterator<Item = AgentLifecycleState>) -> Self {
        let mut aggregate = Self::default();
        for state in states {
            aggregate.total += 1;
            match state {
                AgentLifecycleState::Queued => aggregate.queued += 1,
                AgentLifecycleState::Running => aggregate.running += 1,
                AgentLifecycleState::Completed => aggregate.completed += 1,
                AgentLifecycleState::Failed => aggregate.failed += 1,
                AgentLifecycleState::Cancelled | AgentLifecycleState::Interrupted => {
                    aggregate.cancelled += 1;
                }
                AgentLifecycleState::TimedOut => aggregate.timed_out += 1,
            }
        }
        aggregate
    }

    #[must_use]
    pub const fn status(self) -> AgentLifecycleState {
        if self.running > 0 {
            AgentLifecycleState::Running
        } else if self.queued > 0 {
            AgentLifecycleState::Queued
        } else if self.failed > 0 || self.timed_out > 0 {
            AgentLifecycleState::Failed
        } else if self.cancelled > 0 {
            AgentLifecycleState::Cancelled
        } else {
            AgentLifecycleState::Completed
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwarmSnapshot {
    pub swarm_id: String,
    pub description: String,
    pub role: AgentRole,
    pub mode: AgentRunMode,
    pub state: AgentLifecycleState,
    #[serde(default = "default_swarm_max_concurrency")]
    pub max_concurrency: usize,
    pub aggregate: SwarmAggregate,
    pub children: Vec<SwarmChildSnapshot>,
}

impl SwarmSnapshot {
    pub(crate) fn clear_live_queue_metadata(&mut self) {
        for child in &mut self.children {
            child.agent.clear_live_queue_metadata();
        }
    }
}

fn default_swarm_max_concurrency() -> usize {
    1
}

const MAX_PROGRESS_TEXT_CHARS: usize = 512;
const MAX_TOOL_SUMMARY_CHARS: usize = 256;

#[must_use]
pub fn apply_agent_progress(
    snapshot: &mut AgentSnapshot,
    progress: &AgentProgressSnapshot,
) -> bool {
    if snapshot.id != progress.agent_id {
        return false;
    }
    snapshot.state = progress.state;
    snapshot.mode = progress.mode;
    snapshot.detached_from_foreground = progress.detached_from_foreground;
    snapshot.updated_at_ms = snapshot.updated_at_ms.max(progress.updated_at_ms);
    snapshot.terminal_at_ms = progress.terminal_at_ms;
    snapshot.terminal_reason = progress.terminal_reason;
    snapshot.run_count = progress.run_count;
    snapshot.live_messages_received = progress.live_messages_received;
    snapshot.tool_count = progress.tool_count;
    snapshot.token_count = progress.token_count;
    snapshot.cache_read_token_count = progress.cache_read_token_count;
    snapshot.cache_write_token_count = progress.cache_write_token_count;
    snapshot.elapsed = Duration::from_millis(progress.elapsed_ms);
    snapshot.latest_text.clone_from(&progress.latest_text);
    snapshot.outcome.clone_from(&progress.outcome);
    if let Some(tool) = &progress.last_tool {
        upsert_progress_tool(&mut snapshot.activity, tool);
    }
    if let Some(text) = &progress.latest_text {
        upsert_progress_text(&mut snapshot.activity, text);
    }
    trim_progress_activity(&mut snapshot.activity);
    true
}

pub fn apply_swarm_child_progress(
    swarm: &mut SwarmSnapshot,
    child_progress: &SwarmChildProgress,
    _aggregate: SwarmAggregate,
    _state: AgentLifecycleState,
) -> Option<AgentSnapshot> {
    let updated = {
        let child = swarm.children.iter_mut().find(|child| {
            child.item_index == child_progress.item_index
                || child.agent.id == child_progress.progress.agent_id
        })?;
        let _ = apply_agent_progress(&mut child.agent, &child_progress.progress);
        child.agent.clone()
    };
    swarm.aggregate =
        SwarmAggregate::from_states(swarm.children.iter().map(|child| child.agent.state));
    swarm.state = swarm.aggregate.status();
    Some(updated)
}

fn upsert_progress_tool(activity: &mut Vec<AgentActivityEntry>, tool: &DelegateToolProgress) {
    if let Some(existing) = activity.iter_mut().find_map(|entry| match &mut entry.kind {
        AgentActivityKind::Tool { id, .. } if id == &tool.id => Some(entry),
        _ => None,
    }) {
        existing.kind = AgentActivityKind::Tool {
            id: tool.id.clone(),
            name: tool.name.clone(),
            summary: tool.summary.clone(),
            phase: tool.phase,
            output: tool.output.clone(),
            files: tool.files.clone(),
        };
        return;
    }
    activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: tool.id.clone(),
            name: tool.name.clone(),
            summary: tool.summary.clone(),
            phase: tool.phase,
            output: tool.output.clone(),
            files: tool.files.clone(),
        },
    });
}

fn clear_live_queue_metadata_from_activity(activity: &mut [AgentActivityEntry]) {
    for entry in activity {
        if let AgentActivityKind::Tool { phase, .. } = &mut entry.kind {
            clear_live_queue_metadata_from_phase(phase);
        }
    }
}

fn clear_live_queue_metadata_from_phase(phase: &mut AgentToolActivityPhase) {
    if let AgentToolActivityPhase::Queued {
        position,
        queued_at_ms,
    } = phase
    {
        *position = None;
        *queued_at_ms = 0;
    }
}

fn upsert_progress_text(activity: &mut Vec<AgentActivityEntry>, text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    if activity.iter().rev().any(|entry| {
        matches!(&entry.kind, AgentActivityKind::Text { text, thinking: false } if text.trim() == trimmed)
    }) {
        return;
    }
    activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: text.to_owned(),
            thinking: false,
        },
    });
}

fn trim_progress_activity(activity: &mut Vec<AgentActivityEntry>) {
    const MAX_AGENT_ACTIVITY: usize = 24;
    let excess = activity.len().saturating_sub(MAX_AGENT_ACTIVITY);
    activity.drain(..excess);
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn truncate_progress_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let keep = max_chars.saturating_sub(3);
    let mut truncated = text.chars().take(keep).collect::<String>();
    truncated.push_str("...");
    truncated
}
