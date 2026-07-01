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
    pub fn is_root_child(&self) -> bool {
        self.0
            .strip_prefix("/root/")
            .is_some_and(|tail| !tail.contains('/'))
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
}

impl AgentRole {
    /// All variants in the order presented to the model.
    pub const ALL: [AgentRole; 4] = [
        AgentRole::Coder,
        AgentRole::Explorer,
        AgentRole::Planner,
        AgentRole::Reviewer,
    ];

    /// The snake_case identifier used in tool schemas and persisted snapshots.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AgentRole::Coder => "coder",
            AgentRole::Explorer => "explorer",
            AgentRole::Planner => "planner",
            AgentRole::Reviewer => "reviewer",
        }
    }
}
