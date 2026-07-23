use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::AgentTokenUsage;
use crate::multi_agent::{AgentSnapshot, SwarmSnapshot};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowId(pub String);

impl std::fmt::Display for WorkflowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
    ResourceLimited,
}

impl WorkflowState {
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::ResourceLimited
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowActor {
    Human,
    Model,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowInvocationKind {
    Phase,
    Log,
    Delegate,
    Swarm,
    Verify,
    VerifyCommand,
    Report,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowOutcomeStatus {
    Completed,
    Failed,
    Denied,
    Cancelled,
    ResourceLimited,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowInterruptionReason {
    InstructionReplanRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowPhase {
    pub id: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunMetadata {
    pub run_id: WorkflowId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<WorkflowId>,
    pub name: String,
    pub description: String,
    pub phases: Vec<WorkflowPhase>,
    pub script: String,
    pub script_sha256: String,
    #[serde(default = "default_args")]
    pub args: serde_json::Value,
    pub launch_source: String,
    pub journal_format_version: u32,
}

fn default_args() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowChildRef {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowInvocationOutcome {
    pub ok: bool,
    pub status: WorkflowOutcomeStatus,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interruption: Option<WorkflowInterruptionReason>,
    #[serde(default = "default_details")]
    pub details: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_usage: Option<AgentTokenUsage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_refs: Vec<WorkflowChildRef>,
}

fn default_details() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

// --- Legacy types retained for historical session read-only compatibility ---

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
    #[serde(default)]
    pub current_phase: Option<String>,
    #[serde(default)]
    pub projection_sequence: Option<u64>,
    #[serde(default)]
    pub recovery_failure: bool,
    #[serde(default)]
    pub started_at_ms: Option<u64>,
    #[serde(default)]
    pub updated_at_ms: Option<u64>,
    #[serde(default)]
    pub invocation_count: u64,
    #[serde(default)]
    pub failure_count: u64,
    #[serde(default)]
    pub actual_usage: Option<AgentTokenUsage>,
    #[serde(default)]
    pub latest_log_summary: Option<String>,
    #[serde(default)]
    pub latest_report_summary: Option<String>,
    #[serde(default)]
    pub terminal_reason: Option<String>,
    #[serde(default)]
    pub steps: Vec<WorkflowStepRecord>,
}
