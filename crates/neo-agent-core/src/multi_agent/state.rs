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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl AgentLifecycleState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut
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
pub enum AgentTerminalReason {
    Completed,
    Error,
    CancelledByUser,
    TimedOut,
    Killed,
    Lost,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
                AgentLifecycleState::Cancelled => aggregate.cancelled += 1,
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

fn default_swarm_max_concurrency() -> usize {
    1
}
