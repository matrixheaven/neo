use serde_json::json;

use super::{Tool, ToolContext, ToolFuture, ToolResult, parse_input, schema};
use crate::AgentEvent;
use crate::workflow::{
    LuaWorkflowRunner, WorkflowEventContext, WorkflowId, WorkflowSnapshot, WorkflowState,
};

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
            let runner = LuaWorkflowRunner::new();
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
            let event_context = WorkflowEventContext {
                turn,
                id: workflow_id.clone(),
                title: input.title.clone(),
            };
            match runner
                .run_script_with_context(ctx, event_context, &input.script)
                .await
            {
                Ok(return_value) => {
                    let steps = runner.recorder().steps();
                    let reports = runner.recorder().reports();
                    let report_preview = format_report_preview(&reports);
                    let failed = steps.iter().any(|step| step.state == WorkflowState::Failed);
                    let state = if failed {
                        WorkflowState::Failed
                    } else {
                        WorkflowState::Completed
                    };
                    let snapshot =
                        workflow_snapshot(workflow_id, input.title.clone(), state, steps.clone());
                    ctx.emit_event(AgentEvent::WorkflowFinished {
                        turn,
                        workflow: snapshot.clone(),
                    });
                    let reports_details: Vec<serde_json::Value> = reports
                        .iter()
                        .enumerate()
                        .map(|(index, value)| json!({ "index": index + 1, "value": value }))
                        .collect();
                    let result = if failed {
                        let content = format!(
                            "workflow: {}\nstatus: failed\nsteps: {}\n{}result: {}",
                            input.title,
                            steps.len(),
                            if report_preview.is_empty() {
                                String::new()
                            } else {
                                format!("reports:\n{report_preview}\n")
                            },
                            format_workflow_result(return_value.as_ref()),
                        );
                        ToolResult::error(content)
                    } else {
                        let content = format!(
                            "workflow: {}\nstatus: completed\nsteps: {}\n{}result: {}",
                            input.title,
                            steps.len(),
                            if report_preview.is_empty() {
                                String::new()
                            } else {
                                format!("reports:\n{report_preview}\n")
                            },
                            format_workflow_result(return_value.as_ref()),
                        );
                        ToolResult::ok(content)
                    };
                    Ok(result.with_details(json!({
                        "kind": "workflow",
                        "title": input.title,
                        "id": snapshot.id.0,
                        "status": if failed { "failed" } else { "completed" },
                        "steps": steps,
                        "reports": reports_details,
                        "result": return_value,
                    })))
                }
                Err(err) => {
                    let steps = runner.recorder().steps();
                    let reports = runner.recorder().reports();
                    let report_preview = format_report_preview(&reports);
                    let reports_details: Vec<serde_json::Value> = reports
                        .iter()
                        .enumerate()
                        .map(|(index, value)| json!({ "index": index + 1, "value": value }))
                        .collect();
                    let snapshot = workflow_snapshot(
                        workflow_id,
                        input.title.clone(),
                        WorkflowState::Failed,
                        steps.clone(),
                    );
                    ctx.emit_event(AgentEvent::WorkflowFinished {
                        turn,
                        workflow: snapshot.clone(),
                    });
                    let content = if report_preview.is_empty() {
                        format!("workflow: {}\nstatus: failed\nerror: {}", input.title, err)
                    } else {
                        format!(
                            "workflow: {}\nstatus: failed\nerror: {}\nreports:\n{}",
                            input.title, err, report_preview
                        )
                    };
                    Ok(ToolResult::error(content).with_details(json!({
                        "kind": "workflow",
                        "title": input.title,
                        "id": snapshot.id.0,
                        "status": "failed",
                        "error": err.to_string(),
                        "steps": steps,
                        "reports": reports_details,
                    })))
                }
            }
        })
    }
}

fn format_workflow_result(result: Option<&serde_json::Value>) -> String {
    result.map_or_else(|| "null".to_owned(), serde_json::Value::to_string)
}

fn format_report_preview(reports: &[serde_json::Value]) -> String {
    reports
        .iter()
        .take(5)
        .enumerate()
        .map(|(index, value)| format!("report {}: {}", index + 1, compact_json(value)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn compact_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => {
            serde_json::to_string(other).unwrap_or_else(|_| "<unserializable report>".to_owned())
        }
    }
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
