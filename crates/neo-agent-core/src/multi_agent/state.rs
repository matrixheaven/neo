use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{AgentDisplayName, AgentId, AgentPath, AgentRole};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunMode {
    Foreground,
    Background,
}

impl Default for AgentRunMode {
    fn default() -> Self {
        Self::Foreground
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentTerminalOutcome {
    pub summary: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentActivityKind {
    Tool {
        id: String,
        name: String,
        summary: Option<String>,
        failed: bool,
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
    pub tool_count: usize,
    pub token_count: usize,
    pub elapsed: Duration,
    pub latest_text: Option<String>,
    #[serde(default)]
    pub activity: Vec<AgentActivityEntry>,
    pub outcome: Option<AgentTerminalOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwarmChildSnapshot {
    pub item_index: usize,
    pub item: String,
    pub agent: AgentSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SwarmSnapshot {
    pub swarm_id: String,
    pub description: String,
    pub mode: AgentRunMode,
    #[serde(default = "default_swarm_max_concurrency")]
    pub max_concurrency: usize,
    pub children: Vec<SwarmChildSnapshot>,
}

fn default_swarm_max_concurrency() -> usize {
    1
}
