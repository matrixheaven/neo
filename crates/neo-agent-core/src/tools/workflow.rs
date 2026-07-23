use serde_json::json;

use neo_ai::providers::fake::FakeModelClient;

use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};
use crate::AgentEvent;
use crate::workflow::{LuaWorkflowRunner, WorkflowId, WorkflowSnapshot, WorkflowState};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RunWorkflowInput {
    #[schemars(description = "Human-readable title for the workflow.")]
    title: String,
    #[schemars(description = "Lua source code for the workflow.")]
    script: String,
}

pub struct RunWorkflowTool;

impl Tool for RunWorkflowTool {
    fn name(&self) -> &'static str {
        "RunWorkflow"
    }

    fn description(&self) -> &'static str {
        "Run a Lua workflow script. The script can call neo.delegate, neo.swarm, \
         neo.verify, neo.report, and neo.fail. No raw OS APIs are exposed."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<RunWorkflowInput>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: RunWorkflowInput = parse_input(self.name(), input)?;

            // Check for workflow launch capability. Only exact /workflow
            // grants it; ordinary text or model inference cannot.
            if !ctx.workflow_capability.is_available().await {
                return Ok(ToolResult::error(
                    "RunWorkflow requires a launch capability. Use the /workflow slash command first."
                        .to_owned(),
                ));
            }
            // Consume the capability on successful durable creation
            ctx.workflow_capability.consume_if_available().await;

            let turn = ctx.current_turn.unwrap_or_default();
            let workflow_id = WorkflowId(format!("workflow-{turn}-{}", uuid::Uuid::new_v4()));

            ctx.emit_event(AgentEvent::WorkflowStarted {
                turn,
                workflow: workflow_snapshot(
                    workflow_id.clone(),
                    input.title.clone(),
                    WorkflowState::Running,
                    Vec::new(),
                ),
            });

            // Temporary foreground bridge: create a minimal dispatch handle and
            // run synchronously. This will be replaced by full background launch
            // in Task 5 (capability + launch approval).
            let dispatch = crate::runtime::WorkflowDispatchHandle {
                config: ctx.child_config.clone().unwrap_or_else(|| {
                    crate::AgentConfig::for_model(neo_ai::ModelSpec {
                        provider: neo_ai::ProviderId("in-workflow".to_owned()),
                        model: "in-workflow".to_owned(),
                        api: neo_ai::ApiKind::OpenAi,
                        capabilities: neo_ai::ModelCapabilities::chat(),
                    })
                }),
                model_client: ctx
                    .child_model
                    .clone()
                    .unwrap_or_else(|| std::sync::Arc::new(FakeModelClient::new(vec![]))),
                registry: ctx.child_tools.clone().unwrap_or_else(|| {
                    std::sync::Arc::new(super::ToolRegistry::with_builtin_tools())
                }),
                process_supervisor: super::ProcessSupervisor::default(),
                context: crate::AgentContext::new(),
            };

            let handle =
                crate::workflow::WorkflowRuntime::new(crate::workflow::WorkflowLimits::default())
                    .create_run(
                        &std::env::temp_dir(),
                        crate::workflow::WorkflowLaunchRequest {
                            name: input.title.clone(),
                            description: String::new(),
                            phases: vec![],
                            script: input.script.clone(),
                            args: json!({}),
                            launch_source: "RunWorkflow".to_owned(),
                            parent_run_id: None,
                        },
                    )
                    .await
                    .unwrap_or_else(|_| panic!("workflow launch failed"));

            let runner = LuaWorkflowRunner::new(
                dispatch,
                handle,
                crate::workflow::WorkflowLimits::default(),
            );

            match runner.execute(&input.script, json!({})).await {
                Ok(return_value) => {
                    let snapshot = workflow_snapshot(
                        workflow_id,
                        input.title.clone(),
                        WorkflowState::Completed,
                        Vec::new(),
                    );
                    ctx.emit_event(AgentEvent::WorkflowFinished {
                        turn,
                        workflow: snapshot.clone(),
                    });
                    let result = format!(
                        "workflow: {}\nstatus: completed\nresult: {}",
                        input.title,
                        format_workflow_result(return_value.as_ref()),
                    );
                    Ok(ToolResult::ok(result).with_details(json!({
                        "kind": "workflow",
                        "title": input.title,
                        "id": snapshot.id.0,
                        "status": "completed",
                        "result": return_value,
                    })))
                }
                Err(err) => {
                    let snapshot = workflow_snapshot(
                        workflow_id,
                        input.title.clone(),
                        WorkflowState::Failed,
                        Vec::new(),
                    );
                    ctx.emit_event(AgentEvent::WorkflowFinished {
                        turn,
                        workflow: snapshot.clone(),
                    });
                    let content =
                        format!("workflow: {}\nstatus: failed\nerror: {}", input.title, err);
                    Ok(ToolResult::error(content).with_details(json!({
                        "kind": "workflow",
                        "title": input.title,
                        "id": snapshot.id.0,
                        "status": "failed",
                        "error": err.to_string(),
                    })))
                }
            }
        })
    }
}

fn format_workflow_result(result: Option<&serde_json::Value>) -> String {
    result.map_or_else(|| "null".to_owned(), serde_json::Value::to_string)
}

fn workflow_snapshot(
    id: WorkflowId,
    title: String,
    state: WorkflowState,
    steps: Vec<crate::workflow::WorkflowStepRecord>,
) -> WorkflowSnapshot {
    WorkflowSnapshot {
        id,
        title,
        state,
        steps,
    }
}
