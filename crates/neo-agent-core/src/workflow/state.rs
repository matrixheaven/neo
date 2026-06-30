use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::multi_agent::{AgentSnapshot, SwarmSnapshot};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    Running,
    Failed,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowStepRecord {
    pub index: usize,
    pub name: String,
    pub state: WorkflowState,
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm: Option<SwarmSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_failures: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowSnapshot {
    pub id: WorkflowId,
    pub title: String,
    pub state: WorkflowState,
    pub steps: Vec<WorkflowStepRecord>,
}
