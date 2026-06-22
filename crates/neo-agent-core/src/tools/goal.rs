use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    Tool, ToolContext, ToolError, ToolFuture, ToolResult,
    goal::{Goal, GoalManager, GoalStatus},
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StartGoalArgs {
    /// The objective to pursue. Must have a verifiable end state.
    #[schemars(description = "The objective to pursue. Must have a verifiable end state.")]
    pub objective: String,
    /// How to verify the goal is complete. Include when the user provides one.
    #[serde(default)]
    #[schemars(
        description = "How to verify the goal is complete. Include when the user provides one."
    )]
    pub completion_criterion: Option<String>,
    /// Replace an existing active or paused goal instead of failing.
    #[serde(default)]
    #[schemars(
        description = "Replace an existing active or paused goal instead of failing. Use only when the user explicitly wants to abandon the current goal and start a new one."
    )]
    pub replace: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExitGoalModeArgs {
    /// Approved goal objective to start after user review.
    #[schemars(description = "Approved goal objective to start after user review.")]
    pub objective: String,
    /// How to verify the goal is complete.
    #[serde(default)]
    #[schemars(description = "How to verify the goal is complete.")]
    pub completion_criterion: Option<String>,
    /// Ordered phase plan for the goal run.
    #[serde(default)]
    #[schemars(description = "Ordered phase plan for the goal run.")]
    pub phases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateGoalStatusArgs {
    /// The lifecycle status to set for the current goal.
    #[schemars(
        description = "The lifecycle status to set for the current goal: active, complete, paused, or blocked."
    )]
    pub status: GoalStatusArg,
    /// Explanation when the goal becomes blocked. Optional but recommended for blocked status.
    #[serde(default)]
    #[schemars(
        description = "Explanation when the goal becomes blocked. Optional but recommended when status is blocked."
    )]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatusArg {
    /// Resume a paused or blocked goal when the user explicitly asks you to work on that goal.
    Active,
    /// The objective is satisfied and any stated validation has passed.
    Complete,
    /// Set the goal aside for now; it can be resumed later.
    Paused,
    /// An external condition or required user input prevents progress.
    Blocked,
}

impl From<GoalStatusArg> for crate::goal::GoalStatus {
    fn from(value: GoalStatusArg) -> Self {
        match value {
            GoalStatusArg::Complete => Self::Complete,
            GoalStatusArg::Blocked => Self::Blocked,
            GoalStatusArg::Active => Self::Active,
            GoalStatusArg::Paused => Self::Paused,
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
        "Set the status of the current goal. This is how you resume, end, or yield an autonomous goal.\n\n\
         - `active` - resume a paused or blocked goal when the user explicitly asks you to work on that goal.\n\
         - `complete` - the objective is satisfied and any stated validation has passed. The goal ends and a completion summary is recorded.\n\
         - `paused` - set the goal aside for now (e.g. to hand control back to the user). It can be resumed later.\n\
         - `blocked` - an external condition or required user input prevents progress, or the objective cannot be completed as stated. The goal stops but can be resumed later.\n\n\
         If the goal is active and you do not call this, the goal keeps running: after your turn ends you will be prompted to continue. Call `complete` only when all required work is done, any stated validation has passed, and there is no useful next action. Do not call `complete` after only producing a plan, summary, first pass, or partial result. If you call `blocked`, explain the blocker in your next message. This tool only records the status."
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
                ))
                .with_details(goal_details("updated", &goal))),
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

pub struct ExitGoalModeTool {
    manager: Arc<GoalManager>,
}

impl ExitGoalModeTool {
    #[must_use]
    pub fn new(manager: Arc<GoalManager>) -> Self {
        Self { manager }
    }
}

impl Tool for ExitGoalModeTool {
    fn name(&self) -> &'static str {
        "ExitGoalMode"
    }

    fn description(&self) -> &'static str {
        "Use this when goal mode has produced a reviewed goal draft and is ready for user approval. \
         The user will review the objective, completion criterion, and phases in a blocking dialog.\n\n\
         How this tool works:\n\
         - This tool submits the drafted goal for user review. It does NOT start the goal directly — \
         the user must approve it first.\n\
         - If approved, the durable goal is created and the runtime begins autonomous turns to \
         pursue it.\n\
         - If rejected, goal mode remains active so you can revise the draft.\n\
         - If the user requests revisions, update the objective/phases and call this tool again.\n\n\
         Two paths to create a goal:\n\
         1. Goal mode (this tool) — the AI drafts a structured goal through conversation, then \
         submits it via ExitGoalMode for blocking review.\n\
         2. Direct /goal command — the user authors the goal objective directly via the \
         /goal <objective> slash command, bypassing the AI draft step.\n\n\
         Parameters:\n\
         - objective: The approved goal objective. Must have a verifiable end state.\n\
         - completion_criterion: How to verify the goal is complete. Example: \"all integration \
         tests pass\" or \"the API returns 200 for all documented endpoints\".\n\
         - phases: Ordered list of phase descriptions. Each phase should be a self-contained \
         milestone. Example: [\"Phase 1: Set up test fixtures and data models\", \"Phase 2: \
         Implement core API endpoints\", \"Phase 3: Add error handling and integration tests\"].\n\n\
         Permission mode notes:\n\
         - In yolo and ask modes, the user reviews the goal in a blocking dialog.\n\
         - In auto permission mode, the goal starts without user review.\n\n\
         Before using:\n\
         - Make sure the objective has a checkable completion condition.\n\
         - If the user's request is vague, ask for the missing completion criterion before calling \
         this tool."
    }

    fn input_schema(&self) -> serde_json::Value {
        neo_ai::tool_schema::schema_for::<ExitGoalModeArgs>()
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let args: ExitGoalModeArgs =
                serde_json::from_value(input).map_err(|err| ToolError::InvalidInput {
                    tool: "ExitGoalMode".to_owned(),
                    message: err.to_string(),
                })?;
            let mut goal = Goal::new(args.objective);
            if let Some(criterion) = args.completion_criterion {
                goal = goal.with_completion_criterion(criterion);
            }
            if !args.phases.is_empty() {
                goal.phases = args.phases;
                goal.current_phase = Some(0);
            }
            let objective = goal.objective.clone();
            Ok(match self.manager.start(goal).await {
                Ok(Some(_previous)) => started_goal_result(
                    &self.manager,
                    format!("Approved and started goal: {objective} (previous goal superseded)"),
                    true,
                ),
                Ok(None) => started_goal_result(
                    &self.manager,
                    format!("Approved and started goal: {objective}"),
                    true,
                ),
                Err(err) => ToolResult::error(format!("failed to start goal: {err}")),
            })
        })
    }
}

impl Tool for StartGoalTool {
    fn name(&self) -> &'static str {
        "StartGoal"
    }

    fn description(&self) -> &'static str {
        "Start a durable, structured goal that the runtime will pursue across multiple turns.\n\n\
         Call this tool only when:\n\
         - the user explicitly asks you to start a goal or work autonomously toward an outcome, or\n\
         - a host goal-intake prompt asks you to create one.\n\n\
         Do NOT create a goal for greetings, ordinary questions, or vague requests that lack a verifiable completion condition. A goal needs a checkable end state.\n\n\
         When the request is vague, ask the user for the missing completion criterion before creating the goal. If the user clearly insists after you warn them that the wording is vague or risky, respect that and create the goal.\n\n\
         Include a `completion_criterion` when the user provides one, or when it can be stated without inventing new requirements. Keep `objective` concise; reference long task descriptions by file path rather than pasting them.\n\n\
         Use `replace: true` only when the user explicitly wants to abandon the current goal and start a new one.\n\n\
         Returns:\n\
         On success, returns the created goal's ID and initial status (\"active\"). \
         If an active goal already exists and replace is false, the call fails with an error \
         identifying the existing goal.\n\n\
         completion_criterion examples:\n\
         - \"All integration tests in tests/api/ pass without errors\"\n\
         - \"The README.md contains a quickstart section with a working code example\"\n\
         - \"cargo clippy reports zero warnings across the workspace\""
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

            let result = if args.replace {
                manager.replace(goal).await
            } else {
                manager.start(goal).await
            };

            Ok(match result {
                Ok(Some(_previous)) => started_goal_result(
                    &manager,
                    format!("Started goal: {objective} (previous goal superseded)"),
                    false,
                ),
                Ok(None) => {
                    started_goal_result(&manager, format!("Started goal: {objective}"), false)
                }
                Err(err) => ToolResult::error(format!("failed to start goal: {err}")),
            })
        })
    }
}

fn started_goal_result(manager: &GoalManager, content: String, terminate: bool) -> ToolResult {
    let Some(goal) = manager.active() else {
        return ToolResult::error("failed to load started goal");
    };
    let result = ToolResult::ok(content).with_details(goal_details("started", &goal));
    if terminate {
        result.terminate()
    } else {
        result
    }
}

fn goal_details(event: &str, goal: &Goal) -> serde_json::Value {
    json!({
        "kind": "goal",
        "event": event,
        "id": goal.id,
        "objective": goal.objective,
        "status": goal_status_label(goal.status),
        "reason": goal.blocked_reason,
    })
}

const fn goal_status_label(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::Active => "active",
        GoalStatus::Paused => "paused",
        GoalStatus::Blocked => "blocked",
        GoalStatus::Complete => "complete",
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
        "Read the current goal: its objective, completion criterion, status, and how many turns have been spent on it.\n\n\
         Use this tool before deciding whether to continue working, report completion via UpdateGoalStatus, report a blocker, or respect a pause. It returns a JSON goal snapshot; the `goal` field is `null` when there is no current goal."
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
                Some(goal) => Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&json!({"goal": goal})).unwrap_or_default(),
                )),
                None => Ok(ToolResult::ok(
                    serde_json::to_string_pretty(&json!({"goal": serde_json::Value::Null}))
                        .unwrap_or_default(),
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;
    use serde_json::json;
    use tempfile::TempDir;

    async fn make_manager() -> (TempDir, Arc<GoalManager>) {
        let temp = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(
            GoalManager::load(temp.path().to_path_buf())
                .await
                .expect("load goal manager"),
        );
        (temp, manager)
    }

    #[tokio::test]
    async fn start_goal_starts_active_goal() {
        let (_temp, manager) = make_manager().await;
        let tool = StartGoalTool::new(Arc::clone(&manager));
        let ctx = ToolContext::new(".").expect("context");

        let result = tool
            .execute(
                &ctx,
                json!({
                    "objective": "Refactor auth module",
                    "completion_criterion": "All auth tests pass"
                }),
            )
            .await
            .expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("Refactor auth module"));

        let active = manager.active().expect("active goal");
        assert_eq!(active.objective, "Refactor auth module");
        assert_eq!(
            active.completion_criterion,
            Some("All auth tests pass".to_owned())
        );
    }

    #[tokio::test]
    async fn start_goal_replace_supersedes_existing() {
        let (_temp, manager) = make_manager().await;
        let tool = StartGoalTool::new(Arc::clone(&manager));
        let ctx = ToolContext::new(".").expect("context");

        tool.execute(&ctx, json!({"objective": "First goal"}))
            .await
            .expect("start first");
        let result = tool
            .execute(
                &ctx,
                json!({
                    "objective": "Second goal",
                    "replace": true
                }),
            )
            .await
            .expect("replace");
        assert!(result.content.contains("previous goal superseded"));
        let active = manager.active().expect("active goal");
        assert_eq!(active.objective, "Second goal");
    }

    #[tokio::test]
    async fn update_goal_status_cycles() {
        let (_temp, manager) = make_manager().await;
        let start = StartGoalTool::new(Arc::clone(&manager));
        let update = UpdateGoalStatusTool::new(Arc::clone(&manager));
        let ctx = ToolContext::new(".").expect("context");

        start
            .execute(&ctx, json!({"objective": "Do work"}))
            .await
            .expect("start");

        let paused = update
            .execute(&ctx, json!({"status": "paused"}))
            .await
            .expect("pause");
        assert!(paused.content.contains("Paused"));

        let resumed = update
            .execute(&ctx, json!({"status": "active"}))
            .await
            .expect("resume");
        assert!(resumed.content.contains("Active"));

        let blocked = update
            .execute(
                &ctx,
                json!({
                    "status": "blocked",
                    "reason": "Missing API key"
                }),
            )
            .await
            .expect("block");
        assert!(blocked.content.contains("Blocked"));
        assert_eq!(
            manager.active().unwrap().blocked_reason,
            Some("Missing API key".to_owned())
        );

        let completed = update
            .execute(&ctx, json!({"status": "complete"}))
            .await
            .expect("complete");
        assert!(
            completed.content.contains("Complete"),
            "unexpected content: {}",
            completed.content
        );
        assert!(manager.active().is_none());
    }

    #[tokio::test]
    async fn get_goal_status_returns_json() {
        let (_temp, manager) = make_manager().await;
        let start = StartGoalTool::new(Arc::clone(&manager));
        let get = GetGoalStatusTool::new(Arc::clone(&manager));
        let ctx = ToolContext::new(".").expect("context");

        let empty = get.execute(&ctx, json!({})).await.expect("get empty");
        assert!(empty.content.contains("\"goal\": null"));

        start
            .execute(
                &ctx,
                json!({
                    "objective": "Build feature",
                    "completion_criterion": "Tests pass"
                }),
            )
            .await
            .expect("start");

        let status = get.execute(&ctx, json!({})).await.expect("get status");
        assert!(status.content.contains("Build feature"));
        assert!(status.content.contains("Tests pass"));
        assert!(status.content.contains("\"status\""));
    }

    #[tokio::test]
    async fn exit_goal_mode_starts_structured_goal() {
        let (_temp, manager) = make_manager().await;
        let tool = ExitGoalModeTool::new(Arc::clone(&manager));
        let ctx = ToolContext::new(".").expect("context");

        let result = tool
            .execute(
                &ctx,
                json!({
                    "objective": "Ship goal mode",
                    "completion_criterion": "Goal tests pass",
                    "phases": ["Draft", "Implement", "Audit"]
                }),
            )
            .await
            .expect("execute");

        assert!(!result.is_error);
        assert!(result.terminate);
        let active = manager.active().expect("active goal");
        assert_eq!(active.objective, "Ship goal mode");
        assert_eq!(active.phases, ["Draft", "Implement", "Audit"]);
        assert_eq!(active.current_phase, Some(0));
        let artifact_dir = active.artifact_dir.expect("artifact dir");
        assert!(artifact_dir.join("phases/phase-3.md").exists());
    }

    #[tokio::test]
    async fn descriptions_are_non_empty() {
        let (_temp, manager) = make_manager().await;
        assert!(
            !StartGoalTool::new(Arc::clone(&manager))
                .description()
                .is_empty()
        );
        assert!(
            !UpdateGoalStatusTool::new(Arc::clone(&manager))
                .description()
                .is_empty()
        );
        assert!(
            !ExitGoalModeTool::new(Arc::clone(&manager))
                .description()
                .is_empty()
        );
        assert!(!GetGoalStatusTool::new(manager).description().is_empty());
    }
}
