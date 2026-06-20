use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Tool, ToolContext, ToolError, ToolFuture, ToolResult,
    goal::{Goal, GoalManager},
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StartGoalArgs {
    pub objective: String,
    #[serde(default)]
    pub completion_criterion: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateGoalStatusArgs {
    pub status: GoalStatusArg,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatusArg {
    Complete,
    Blocked,
    Active,
}

impl From<GoalStatusArg> for crate::goal::GoalStatus {
    fn from(value: GoalStatusArg) -> Self {
        match value {
            GoalStatusArg::Complete => Self::Complete,
            GoalStatusArg::Blocked => Self::Blocked,
            GoalStatusArg::Active => Self::Active,
        }
    }
}

pub struct UpdateGoalStatusTool {
    manager: Arc<GoalManager>,
}

impl UpdateGoalStatusTool {
    #[must_use]
    pub fn new(manager: Arc<GoalManager>) -> Self {
        Self { manager }
    }
}

impl Tool for UpdateGoalStatusTool {
    fn name(&self) -> &'static str {
        "UpdateGoalStatus"
    }

    fn description(&self) -> &'static str {
        "Update the status of the active goal. Call this when you believe the goal is complete, blocked, or should remain active."
    }

    fn input_schema(&self) -> serde_json::Value {
        neo_ai::tool_schema::schema_for::<UpdateGoalStatusArgs>()
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        let manager = Arc::clone(&self.manager);
        Box::pin(async move {
            let args: UpdateGoalStatusArgs =
                serde_json::from_value(input).map_err(|err| ToolError::InvalidInput {
                    tool: "UpdateGoalStatus".to_owned(),
                    message: err.to_string(),
                })?;
            let status = args.status.into();
            match manager.update_status(status, args.reason.clone()).await {
                Ok(Some(goal)) => Ok(ToolResult::ok(format!(
                    "Goal `{}` status updated to {:?}.",
                    goal.objective, goal.status
                ))),
                Ok(None) => Ok(ToolResult::error("no active goal to update")),
                Err(err) => Ok(ToolResult::error(format!("failed to update goal: {err}"))),
            }
        })
    }
}

pub struct StartGoalTool {
    manager: Arc<GoalManager>,
}

impl StartGoalTool {
    #[must_use]
    pub fn new(manager: Arc<GoalManager>) -> Self {
        Self { manager }
    }
}

impl Tool for StartGoalTool {
    fn name(&self) -> &'static str {
        "StartGoal"
    }

    fn description(&self) -> &'static str {
        "Start a new autonomous goal with the given objective. Replaces any currently active goal."
    }

    fn input_schema(&self) -> serde_json::Value {
        neo_ai::tool_schema::schema_for::<StartGoalArgs>()
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        let manager = Arc::clone(&self.manager);
        Box::pin(async move {
            let args: StartGoalArgs =
                serde_json::from_value(input).map_err(|err| ToolError::InvalidInput {
                    tool: "StartGoal".to_owned(),
                    message: err.to_string(),
                })?;
            let mut goal = Goal::new(args.objective);
            if let Some(criterion) = args.completion_criterion {
                goal = goal.with_completion_criterion(criterion);
            }
            let objective = goal.objective.clone();
            match manager.start(goal).await {
                Ok(Some(_previous)) => Ok(ToolResult::ok(format!(
                    "Started goal: {objective} (previous goal superseded)"
                ))),
                Ok(None) => Ok(ToolResult::ok(format!("Started goal: {objective}"))),
                Err(err) => Ok(ToolResult::error(format!("failed to start goal: {err}"))),
            }
        })
    }
}

pub struct GetGoalStatusTool {
    manager: Arc<GoalManager>,
}

impl GetGoalStatusTool {
    #[must_use]
    pub fn new(manager: Arc<GoalManager>) -> Self {
        Self { manager }
    }
}

impl Tool for GetGoalStatusTool {
    fn name(&self) -> &'static str {
        "GetGoalStatus"
    }

    fn description(&self) -> &'static str {
        "Get the current active goal, its status, and how many turns have been spent on it."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        let manager = Arc::clone(&self.manager);
        Box::pin(async move {
            match manager.active() {
                Some(goal) => Ok(ToolResult::ok(format!(
                    "Active goal: {}\nStatus: {:?}\nTurns: {}\nCompletion criterion: {}",
                    goal.objective,
                    goal.status,
                    goal.turn_count,
                    goal.completion_criterion.as_deref().unwrap_or("none")
                ))),
                None => Ok(ToolResult::ok("No active goal.")),
            }
        })
    }
}
