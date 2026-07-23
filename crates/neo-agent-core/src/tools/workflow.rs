use std::collections::HashSet;

use serde_json::json;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, schema};
use crate::WorkflowApprovalPresentation;
use crate::workflow::capability::WorkflowCapabilityReservation;
use crate::workflow::{WorkflowError, WorkflowLaunchRequest, WorkflowPhase};

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunWorkflowInput {
    #[schemars(description = "Human-readable workflow name.")]
    name: String,
    #[schemars(description = "What this workflow orchestrates.")]
    description: String,
    #[schemars(description = "Ordered reviewed workflow phases.")]
    phases: Vec<RunWorkflowPhaseInput>,
    #[schemars(description = "Complete Lua source for the workflow.")]
    script: String,
    #[serde(default = "empty_args")]
    #[schemars(description = "Read-only object exposed to Lua as args.")]
    args: serde_json::Value,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunWorkflowPhaseInput {
    id: String,
    description: String,
}

fn empty_args() -> serde_json::Value {
    json!({})
}

impl RunWorkflowInput {
    fn validate(self) -> Result<Self, String> {
        if self.name.trim().is_empty() {
            return Err("name must not be empty".to_owned());
        }
        if self.description.trim().is_empty() {
            return Err("description must not be empty".to_owned());
        }
        if self.phases.is_empty() {
            return Err("phases must contain at least one phase".to_owned());
        }
        let mut phase_ids = HashSet::with_capacity(self.phases.len());
        for phase in &self.phases {
            if phase.id.trim().is_empty() || phase.description.trim().is_empty() {
                return Err("phase id and description must not be empty".to_owned());
            }
            if !phase_ids.insert(phase.id.as_str()) {
                return Err(format!("duplicate phase id `{}`", phase.id));
            }
        }
        if self.script.trim().is_empty() {
            return Err("script must not be empty".to_owned());
        }
        if !self.args.is_object() {
            return Err("args must be an object".to_owned());
        }
        Ok(self)
    }

    pub(crate) fn launch_request(
        &self,
        permission_mode: crate::PermissionMode,
    ) -> WorkflowLaunchRequest {
        WorkflowLaunchRequest {
            name: self.name.clone(),
            description: self.description.clone(),
            phases: self
                .phases
                .iter()
                .map(|phase| WorkflowPhase {
                    id: phase.id.clone(),
                    description: phase.description.clone(),
                })
                .collect(),
            script: self.script.clone(),
            args: self.args.clone(),
            launch_source: format!("/workflow ({})", permission_mode.label()),
            parent_run_id: None,
        }
    }
}

pub(crate) fn validated_input(value: &serde_json::Value) -> Result<RunWorkflowInput, String> {
    serde_json::from_value::<RunWorkflowInput>(value.clone())
        .map_err(|error| error.to_string())?
        .validate()
}

pub(crate) fn invalid_input_result(message: impl Into<String>) -> ToolResult {
    let message = message.into();
    ToolResult::error(format!("invalid workflow input: {message}")).with_details(json!({
        "kind": "invalid_workflow_input",
        "message": message,
        "side_effect_occurred": false,
    }))
}

pub(crate) fn approval_presentation(
    value: &serde_json::Value,
) -> Result<WorkflowApprovalPresentation, String> {
    let input = validated_input(value)?;
    let args = serde_json::to_string_pretty(&input.args).map_err(|error| error.to_string())?;
    Ok(WorkflowApprovalPresentation {
        name: input.name,
        description: input.description,
        phases: input
            .phases
            .into_iter()
            .map(|phase| format!("{}: {}", phase.id, phase.description))
            .collect(),
        args,
        line_count: input.script.split('\n').count().max(1),
        byte_count: input.script.len(),
        source: input.script,
        warning: "Launch approval authorizes orchestration only; child tool effects remain independently authorized."
            .to_owned(),
    })
}

pub struct RunWorkflowTool;

async fn rollback_registration_failure(
    runtime: &crate::workflow::WorkflowRuntime,
    reservation: WorkflowCapabilityReservation,
    handle: &crate::workflow::WorkflowHandle,
    register_error: impl std::fmt::Display,
) -> ToolResult {
    match runtime.rollback_created_run(&handle.run_id).await {
        Ok(()) => ToolResult::error(format!("workflow registration failed: {register_error}")),
        Err(rollback_error) => {
            let capability_consumed = reservation.commit();
            let terminal_error = runtime
                .fail_worker_start(&handle.run_id, &rollback_error)
                .await
                .err();
            let suffix = terminal_error.as_ref().map_or_else(String::new, |error| {
                format!("; failed to persist terminal state: {error}")
            });
            ToolResult::error(format!(
                "task_id: {}\nkind: workflow\nstatus: failed\nerror: registration failed: {register_error}; rollback failed: {rollback_error}{suffix}",
                handle.run_id.0
            ))
            .with_details(json!({
                "task_id": handle.run_id.0.clone(),
                "kind": "workflow_launch_failure",
                "status": "failed",
                "capability_consumed": capability_consumed,
                "registration_error": register_error.to_string(),
                "rollback_error": rollback_error.to_string(),
                "terminal_error": terminal_error.map(|error| error.to_string()),
            }))
        }
    }
}

async fn rollback_capability_change(
    runtime: &crate::workflow::WorkflowRuntime,
    background_tasks: &crate::tools::BackgroundTaskManager,
    handle: &crate::workflow::WorkflowHandle,
) -> ToolResult {
    let task_id = handle.run_id.0.clone();
    background_tasks.remove_workflow(&task_id).await;
    if let Err(rollback_error) = runtime.rollback_created_run(&handle.run_id).await {
        let terminal_error = runtime
            .fail_worker_start(&handle.run_id, &rollback_error)
            .await
            .err();
        let suffix = terminal_error.as_ref().map_or_else(String::new, |error| {
            format!("; failed to persist terminal state: {error}")
        });
        return ToolResult::error(format!(
            "task_id: {task_id}\nkind: workflow\nstatus: failed\nerror: workflow capability changed during launch; rollback failed: {rollback_error}{suffix}"
        ))
        .with_details(json!({
            "task_id": task_id,
            "kind": "workflow_launch_failure",
            "status": "failed",
            "reservation_consumed": true,
            "rollback_error": rollback_error.to_string(),
            "terminal_error": terminal_error.map(|error| error.to_string()),
        }));
    }
    ToolResult::error("workflow capability changed during launch".to_owned())
}

impl Tool for RunWorkflowTool {
    fn name(&self) -> &'static str {
        "RunWorkflow"
    }

    fn description(&self) -> &'static str {
        "Launch an approved Lua orchestration workflow in the background. Child tool effects remain independently authorized."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<RunWorkflowInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input = match validated_input(&input) {
                Ok(input) => input,
                Err(message) => return Ok(invalid_input_result(message)),
            };
            let session_dir =
                ctx.session_directory
                    .clone()
                    .ok_or_else(|| ToolError::InvalidInput {
                        tool: self.name().to_owned(),
                        message: "RunWorkflow requires a durable session directory".to_owned(),
                    })?;
            let child_config =
                ctx.child_config
                    .as_ref()
                    .ok_or_else(|| ToolError::InvalidInput {
                        tool: self.name().to_owned(),
                        message: "RunWorkflow requires the canonical runtime dispatch context"
                            .to_owned(),
                    })?;
            let permission_mode = child_config
                .live_permission_mode
                .read()
                .map_or(child_config.permission_mode, |mode| *mode);
            let request = input.launch_request(permission_mode);
            if let Err(error) = ctx.workflow_runtime.validate_launch_request(&request) {
                return Ok(invalid_input_result(error.to_string()));
            }

            let Some(reservation) = ctx.workflow_capability.reserve() else {
                return Ok(ToolResult::error(
                    "RunWorkflow requires a launch capability. Use the exact /workflow slash command first."
                        .to_owned(),
                ));
            };

            let handle = match ctx.workflow_runtime.create_run(&session_dir, request).await {
                Ok(handle) => handle,
                Err(WorkflowError::InvalidInput(message)) => {
                    return Ok(invalid_input_result(message));
                }
                Err(error) => {
                    return Ok(ToolResult::error(format!(
                        "workflow launch failed: {error}"
                    )));
                }
            };
            let task_id = handle.run_id.0.clone();
            if let Err(register_error) = ctx
                .background_tasks
                .start_workflow(task_id.clone(), input.description.clone(), handle.clone())
                .await
            {
                return Ok(rollback_registration_failure(
                    &ctx.workflow_runtime,
                    reservation,
                    &handle,
                    register_error,
                )
                .await);
            }

            if !reservation.commit() {
                return Ok(rollback_capability_change(
                    &ctx.workflow_runtime,
                    &ctx.background_tasks,
                    &handle,
                )
                .await);
            }

            ctx.workflow_runtime
                .emit_started(&handle.run_id)
                .await
                .map_err(|error| ToolError::InvalidInput {
                    tool: self.name().to_owned(),
                    message: error.to_string(),
                })?;

            if let Err(error) = ctx.workflow_runtime.start_worker(&handle.run_id).await {
                let terminal_error = ctx
                    .workflow_runtime
                    .fail_worker_start(&handle.run_id, &error)
                    .await
                    .err();
                let suffix = terminal_error.map_or_else(String::new, |terminal_error| {
                    format!("; failed to persist terminal state: {terminal_error}")
                });
                return Ok(ToolResult::error(format!(
                    "task_id: {task_id}\nkind: workflow\nstatus: failed\nerror: worker startup failed: {error}{suffix}"
                ))
                .with_details(json!({
                    "task_id": task_id,
                    "kind": "workflow",
                    "status": "failed",
                    "error": error.to_string(),
                })));
            }

            Ok(ToolResult::ok(format!(
                "task_id: {task_id}\nkind: workflow\nstatus: running\nautomatic_notification: true\nnext_step: Use TaskOutput with this task_id to inspect the workflow."
            ))
            .with_details(json!({
                "task_id": task_id,
                "kind": "workflow",
                "status": "running",
                "automatic_notification": true,
                "next_step": "Use TaskOutput with this task_id to inspect the workflow.",
            })))
        })
    }
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
