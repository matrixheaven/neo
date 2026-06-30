use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AgentId(String);

impl AgentId {
    #[must_use]
    pub fn new() -> Self {
        Self(format!("agent_{}", Uuid::new_v4().simple()))
    }

    #[must_use]
    pub fn from_suffix_for_test(suffix: &str) -> Self {
        Self(format!("agent_{suffix}"))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AgentId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AgentDisplayName(String);

impl AgentDisplayName {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AgentPath(String);

impl AgentPath {
    #[must_use]
    pub fn root_child(display_name: &AgentDisplayName) -> Self {
        Self(format!("/root/{}", display_name.as_str()))
    }

    #[must_use]
    pub fn swarm_child(swarm_id: &str, display_name: &AgentDisplayName) -> Self {
        Self(format!("/root/{swarm_id}/{}", display_name.as_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    #[default]
    Coder,
    Explorer,
    Planner,
    Reviewer,
    Orchestrator,
}
