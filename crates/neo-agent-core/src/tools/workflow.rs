use std::collections::HashSet;

use serde_json::json;

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, schema};
use crate::WorkflowApprovalPresentation;
use crate::workflow::{
    DynamicWorkflowDefinitionInput, LaunchAuthorizationMode, WorkflowActor, WorkflowError,
    WorkflowErrorCode, WorkflowLaunchCoordinator, WorkflowLaunchHosts, WorkflowLaunchIntent,
    WorkflowLaunchRequest, WorkflowPhase, resolve_dynamic_definition,
};

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
    #[schemars(description = "Required JSON Schema for the final Lua return value.")]
    output_schema: serde_json::Value,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunWorkflowPhaseInput {
    #[schemars(description = "Phase identifier.")]
    id: String,
    #[schemars(description = "Human-readable phase summary.")]
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
        if self.output_schema.is_null() {
            return Err("output_schema is required".to_owned());
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
            output_schema: Some(self.output_schema.clone()),
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

fn map_launch_error(error: WorkflowError) -> ToolResult {
    match error.code() {
        WorkflowErrorCode::InvalidInput
        | WorkflowErrorCode::InvalidDefinition
        | WorkflowErrorCode::InvalidManifest
        | WorkflowErrorCode::InvalidSchema
        | WorkflowErrorCode::InputSchemaInvalid
        | WorkflowErrorCode::LuaCompileFailed => invalid_input_result(error.to_string()),
        WorkflowErrorCode::LaunchAuthorizationMissing => ToolResult::error(error.to_string()),
        WorkflowErrorCode::LaunchAuthorizationMismatch => {
            ToolResult::error(format!("workflow launch failed: {error}"))
        }
        _ => ToolResult::error(format!("workflow launch failed: {error}")),
    }
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

            let Some(launch_nonce) = ctx.workflow_capability.launch_nonce() else {
                return Ok(ToolResult::error(
                    "RunWorkflow requires a launch capability. Use the exact /workflow slash command first."
                        .to_owned(),
                ));
            };

            // Resolve through the canonical dynamic definition path so final
            // output_schema is compiled before any durable run is created.
            let phases: Vec<WorkflowPhase> = input
                .phases
                .iter()
                .map(|phase| WorkflowPhase {
                    id: phase.id.clone(),
                    description: phase.description.clone(),
                })
                .collect();
            let resolved = match resolve_dynamic_definition(
                DynamicWorkflowDefinitionInput {
                    name: input.name.clone(),
                    display_name: None,
                    description: input.description.clone(),
                    phases: phases.clone(),
                    script: input.script.clone(),
                    input_schema: None,
                    output_schema: input.output_schema.clone(),
                },
                &ctx.workflow_runtime.limits(),
            ) {
                Ok(resolved) => resolved,
                Err(error) => return Ok(map_launch_error(error)),
            };
            let schema_sha256 = crate::workflow::canonical_input_hash(&resolved.output_schema);

            let request = WorkflowLaunchRequest {
                name: resolved.name.as_str().to_owned(),
                description: resolved.description.clone(),
                phases,
                script: resolved.lua_source.clone(),
                args: input.args.clone(),
                launch_source: format!("/workflow ({})", permission_mode.label()),
                parent_run_id: None,
                output_schema: Some(resolved.output_schema.clone()),
            };
            let mut intent = WorkflowLaunchIntent::from_parts(
                request,
                crate::workflow::WorkflowLaunchBinding {
                    session_identity: session_dir.display().to_string(),
                    workspace_identity: ctx.cwd.display().to_string(),
                    launch_nonce,
                    actor: WorkflowActor::Model,
                    permission_mode,
                    parent_lineage: None,
                    compiled_input_schema: resolved.compiled_input_schema.clone(),
                    schema_sha256,
                },
            );
            intent.definition_revision = resolved.revision.clone();

            let outcome = match WorkflowLaunchCoordinator
                .launch(
                    &intent,
                    WorkflowLaunchHosts {
                        runtime: &ctx.workflow_runtime,
                        capability: &ctx.workflow_capability,
                        background_tasks: &ctx.background_tasks,
                        session_dir: &session_dir,
                    },
                    LaunchAuthorizationMode::SessionCapability,
                )
                .await
            {
                Ok(outcome) => outcome,
                Err(error) => return Ok(map_launch_error(error)),
            };

            let task_id = outcome.task_id;
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
