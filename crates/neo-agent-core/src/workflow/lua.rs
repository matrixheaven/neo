use mlua::{Lua, LuaSerdeExt, Value};
use serde_json::json;

use super::host_api::WorkflowHostRecorder;
use super::{WorkflowError, WorkflowId, WorkflowSnapshot, WorkflowState, WorkflowStepRecord};
use crate::AgentEvent;
use crate::multi_agent::{AgentSnapshot, SwarmSnapshot};
use crate::tools::{ToolContext, ToolError, ToolResult};

/// Runs Lua workflow scripts in a sandboxed `mlua` VM. The `neo` table
/// exposes `delegate`, `swarm`, `verify`, `report`, and `fail`. No raw
/// OS/file/process/network APIs are available to the script.
#[derive(Debug, Clone, Default)]
pub struct LuaWorkflowRunner {
    recorder: WorkflowHostRecorder,
}

impl LuaWorkflowRunner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn recorder(&self) -> &WorkflowHostRecorder {
        &self.recorder
    }

    /// Run a Lua workflow script with recorder-only reporting functions. This
    /// keeps sandbox smoke tests lightweight; runtime host APIs such as
    /// `delegate`, `swarm`, and `verify` are only installed by
    /// `run_script_with_context` so they cannot silently become recorder demos.
    pub fn run_script(&self, source: &str) -> Result<(), WorkflowError> {
        let lua = Lua::new();
        self.install_recorder_neo_table(&lua)?;
        lua.load(source)
            .set_name("workflow script")
            .exec()
            .map_err(|err| WorkflowError::Lua(err.to_string()))
    }

    pub async fn run_script_with_context(
        &self,
        ctx: &ToolContext,
        event_context: WorkflowEventContext,
        source: &str,
    ) -> Result<Option<serde_json::Value>, WorkflowError> {
        let lua = Lua::new();
        self.install_host_neo_table(&lua, ctx, event_context)?;
        let value = lua
            .load(source)
            .set_name("workflow script")
            .eval_async::<Value>()
            .await
            .map_err(|err| WorkflowError::Lua(err.to_string()))?;
        lua_return_to_json(&lua, value).map_err(|err| WorkflowError::Lua(err.to_string()))
    }

    fn install_recorder_neo_table(&self, lua: &Lua) -> Result<(), WorkflowError> {
        let neo = lua
            .create_table()
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        let recorder_report = self.recorder.clone();
        let report = lua
            .create_function(move |_, value: Value| {
                let text = format!("{value:?}");
                recorder_report.record(format!("report: {text}"));
                recorder_report.record_step("report", Some(text));
                Ok(())
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("report", report)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        let recorder_fail = self.recorder.clone();
        let fail = lua
            .create_function(move |_, message: String| -> mlua::Result<()> {
                recorder_fail.record_step_state(
                    "fail",
                    WorkflowState::Failed,
                    Some(message.clone()),
                );
                Err(mlua::Error::RuntimeError(message))
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("fail", fail)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        lua.globals()
            .set("neo", neo)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn install_host_neo_table(
        &self,
        lua: &Lua,
        ctx: &ToolContext,
        event_context: WorkflowEventContext,
    ) -> Result<(), WorkflowError> {
        let neo = lua
            .create_table()
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        let recorder_report = self.recorder.clone();
        let report_events = event_context.clone();
        let report_ctx = ctx.clone();
        let report = lua
            .create_function(move |lua, value: Value| {
                let report = lua_value_to_json(lua, value)?;
                let summary = report_summary(&report);
                recorder_report.record(format!("report: {summary}"));
                recorder_report.record_report(report.clone());
                recorder_report.push_step(workflow_step(
                    "report",
                    WorkflowState::Completed,
                    Some(summary),
                    Some(json!({ "report": report })),
                    None,
                    None,
                    None,
                ));
                emit_workflow_update(&report_ctx, &report_events, &recorder_report);
                Ok(())
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("report", report)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        let recorder_fail = self.recorder.clone();
        let fail_events = event_context.clone();
        let fail_ctx = ctx.clone();
        let fail = lua
            .create_function(move |_, message: String| -> mlua::Result<()> {
                recorder_fail.record_step_state(
                    "fail",
                    WorkflowState::Failed,
                    Some(message.clone()),
                );
                emit_workflow_update(&fail_ctx, &fail_events, &recorder_fail);
                Err(mlua::Error::RuntimeError(message))
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("fail", fail)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        let delegate_ctx = ctx.clone();
        let recorder_del = self.recorder.clone();
        let delegate_events = event_context.clone();
        let delegate = lua
            .create_async_function(move |lua, table: Value| {
                let ctx = delegate_ctx.clone();
                let recorder = recorder_del.clone();
                let event_context = delegate_events.clone();
                async move {
                    let input = lua_value_to_json(&lua, table)?;
                    let task = input
                        .get("task")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    recorder.record(format!("delegate: {task}"));
                    let result = run_tool(&ctx, "Delegate", input).await?;
                    let handle = delegate_handle_from_result(&result, None);
                    recorder.push_step(workflow_step(
                        format!("delegate: {task}"),
                        workflow_state_from_tool_and_agent(&result, None),
                        Some(handle.summary.clone()),
                        result.details.clone(),
                        None,
                        None,
                        None,
                    ));
                    emit_workflow_update(&ctx, &event_context, &recorder);
                    Ok(handle)
                }
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("delegate", delegate)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        let swarm_ctx = ctx.clone();
        let recorder_sw = self.recorder.clone();
        let swarm_events = event_context.clone();
        let swarm = lua
            .create_async_function(move |lua, table: Value| {
                let ctx = swarm_ctx.clone();
                let recorder = recorder_sw.clone();
                let event_context = swarm_events.clone();
                async move {
                    let input = lua_value_to_json(&lua, table)?;
                    let description = input
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let item_count = input
                        .get("items")
                        .and_then(serde_json::Value::as_array)
                        .map_or(0, Vec::len);
                    recorder.record(format!("swarm: {description} ({item_count} items)"));
                    let result = run_tool(&ctx, "DelegateSwarm", input).await?;
                    let details = result.details.as_ref();
                    let items = result
                        .details
                        .as_ref()
                        .and_then(|details| details.get("items"))
                        .and_then(serde_json::Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let has_failures = result.is_error || swarm_items_have_failures(&items);
                    let summary = swarm_summary(&result, details, has_failures);
                    let swarm_id = result
                        .details
                        .as_ref()
                        .and_then(|details| details.get("swarm_id"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    let status = details_string(&result, "status")
                        .unwrap_or_else(|| status_from_result(&result).to_owned());
                    recorder.push_step(workflow_step(
                        format!("swarm: {description}"),
                        if has_failures {
                            WorkflowState::Failed
                        } else {
                            WorkflowState::Completed
                        },
                        Some(summary.clone()),
                        result.details.clone(),
                        None,
                        None,
                        Some(has_failures),
                    ));
                    emit_workflow_update(&ctx, &event_context, &recorder);
                    Ok(SwarmHandle {
                        kind: "swarm",
                        swarm_id,
                        status,
                        summary,
                        items,
                        has_failures,
                        result: result.details.clone().unwrap_or_else(|| json!({})),
                    })
                }
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("swarm", swarm)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        let recorder_verify = self.recorder.clone();
        let verify_events = event_context.clone();
        let verify_ctx = ctx.clone();
        let verify = lua
            .create_function(
                move |_, (condition, message): (bool, String)| -> mlua::Result<()> {
                    if condition {
                        recorder_verify.push_step(workflow_step(
                            "verify",
                            WorkflowState::Completed,
                            Some(message.clone()),
                            Some(json!({ "condition": true, "message": message })),
                            None,
                            None,
                            None,
                        ));
                        emit_workflow_update(&verify_ctx, &verify_events, &recorder_verify);
                        Ok(())
                    } else {
                        recorder_verify.push_step(workflow_step(
                            "verify",
                            WorkflowState::Failed,
                            Some(message.clone()),
                            Some(json!({ "condition": false, "message": message.clone() })),
                            None,
                            None,
                            None,
                        ));
                        emit_workflow_update(&verify_ctx, &verify_events, &recorder_verify);
                        Err(mlua::Error::RuntimeError(message))
                    }
                },
            )
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("verify", verify)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        let verify_command_ctx = ctx.clone();
        let verify_command_recorder = self.recorder.clone();
        let verify_command_events = event_context;
        let verify_command = lua
            .create_async_function(
                move |_, (command, failure_message): (String, Option<String>)| {
                    let ctx = verify_command_ctx.clone();
                    let recorder = verify_command_recorder.clone();
                    let event_context = verify_command_events.clone();
                    async move {
                        recorder.record(format!("verify_command: {command}"));
                        let result = run_tool(&ctx, "Bash", json!({ "command": command })).await;
                        match result {
                            Ok(result) if !result.is_error => {
                                recorder.push_step(workflow_step(
                                    "verify_command",
                                    WorkflowState::Completed,
                                    Some(result.content.clone()),
                                    result.details,
                                    None,
                                    None,
                                    None,
                                ));
                                emit_workflow_update(&ctx, &event_context, &recorder);
                                Ok(true)
                            }
                            Ok(result) => {
                                let message = failure_message.unwrap_or(result.content);
                                recorder.push_step(workflow_step(
                                    "verify_command",
                                    WorkflowState::Failed,
                                    Some(message.clone()),
                                    result.details,
                                    None,
                                    None,
                                    None,
                                ));
                                emit_workflow_update(&ctx, &event_context, &recorder);
                                Err(mlua::Error::RuntimeError(message))
                            }
                            Err(err) => {
                                let raw = err.to_string();
                                let message = if raw.to_ascii_lowercase().contains("permission") {
                                    "verify_command denied by Bash permission policy".to_owned()
                                } else {
                                    failure_message.unwrap_or(raw)
                                };
                                recorder.push_step(workflow_step(
                                    "verify_command",
                                    WorkflowState::Failed,
                                    Some(message.clone()),
                                    None,
                                    None,
                                    None,
                                    None,
                                ));
                                emit_workflow_update(&ctx, &event_context, &recorder);
                                Err(mlua::Error::RuntimeError(message))
                            }
                        }
                    }
                },
            )
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("verify_command", verify_command)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        lua.globals()
            .set("neo", neo)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct WorkflowEventContext {
    pub turn: u32,
    pub id: WorkflowId,
    pub title: String,
}

/// Handle returned by `neo.delegate()`.
#[derive(Debug, Clone, serde::Serialize)]
struct DelegateHandle {
    kind: &'static str,
    agent_id: Option<String>,
    name: Option<String>,
    status: String,
    summary: String,
    result: serde_json::Value,
}

impl DelegateHandle {
    fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("delegate handle serializes")
    }

    fn to_lua_table(&self, lua: &Lua) -> mlua::Result<Value> {
        lua.to_value(&self.to_json())
    }
}

impl mlua::UserData for DelegateHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("id", |_, this, ()| Ok(this.agent_id.clone()));
        methods.add_method("status", |_, this, ()| Ok(this.status.clone()));
        methods.add_method("summary", |_, this, ()| Ok(this.summary.clone()));
        methods.add_method("name", |_, this, ()| Ok(this.name.clone()));
        methods.add_method("result", |lua, this, ()| lua.to_value(&this.result));
        methods.add_method("to_table", |lua, this, ()| this.to_lua_table(lua));
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, this, ()| {
            Ok(this
                .agent_id
                .clone()
                .unwrap_or_else(|| this.summary.clone()))
        });
        methods.add_meta_method(
            mlua::MetaMethod::Index,
            |lua, this, key: String| match key.as_str() {
                "kind" => lua.to_value(&this.kind),
                "agent_id" => lua.to_value(&this.agent_id),
                "name" => lua.to_value(&this.name),
                "status" => lua.to_value(&this.status),
                "summary" => lua.to_value(&this.summary),
                "result" => lua.to_value(&this.result),
                _ => Ok(mlua::Value::Nil),
            },
        );
    }
}

/// Handle returned by `neo.swarm()`.
#[derive(Debug, Clone, serde::Serialize)]
struct SwarmHandle {
    kind: &'static str,
    swarm_id: Option<String>,
    status: String,
    summary: String,
    items: Vec<serde_json::Value>,
    has_failures: bool,
    result: serde_json::Value,
}

impl SwarmHandle {
    fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("swarm handle serializes")
    }

    fn to_lua_table(&self, lua: &Lua) -> mlua::Result<Value> {
        lua.to_value(&self.to_json())
    }
}

impl mlua::UserData for SwarmHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("id", |_, this, ()| Ok(this.swarm_id.clone()));
        methods.add_method("status", |_, this, ()| Ok(this.status.clone()));
        methods.add_method("summary", |_, this, ()| Ok(this.summary.clone()));
        methods.add_method("items", |lua, this, ()| lua.to_value(&this.items));
        methods.add_method("results", |lua, this, ()| lua.to_value(&this.items));
        methods.add_method("has_failures", |_, this, ()| Ok(this.has_failures));
        methods.add_method("to_table", |lua, this, ()| this.to_lua_table(lua));
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, this, ()| {
            Ok(this
                .swarm_id
                .clone()
                .unwrap_or_else(|| this.summary.clone()))
        });
        methods.add_meta_method(
            mlua::MetaMethod::Index,
            |lua, this, key: String| match key.as_str() {
                "kind" => lua.to_value(&this.kind),
                "swarm_id" => lua.to_value(&this.swarm_id),
                "status" => lua.to_value(&this.status),
                "summary" => lua.to_value(&this.summary),
                "items" => lua.to_value(&this.items),
                "has_failures" => lua.to_value(&this.has_failures),
                "result" => lua.to_value(&this.result),
                _ => Ok(mlua::Value::Nil),
            },
        );
    }
}

fn workflow_step(
    name: impl Into<String>,
    state: WorkflowState,
    summary: Option<String>,
    details: Option<serde_json::Value>,
    agent: Option<AgentSnapshot>,
    swarm: Option<SwarmSnapshot>,
    has_failures: Option<bool>,
) -> WorkflowStepRecord {
    WorkflowStepRecord {
        index: 0,
        name: name.into(),
        state,
        summary,
        details,
        agent,
        swarm,
        has_failures,
    }
}

fn emit_workflow_update(
    ctx: &ToolContext,
    event_context: &WorkflowEventContext,
    recorder: &WorkflowHostRecorder,
) {
    ctx.emit_event(AgentEvent::WorkflowUpdated {
        turn: event_context.turn,
        workflow: WorkflowSnapshot {
            id: event_context.id.clone(),
            title: event_context.title.clone(),
            state: WorkflowState::Running,
            steps: recorder.steps(),
        },
    });
}

async fn run_tool(
    ctx: &ToolContext,
    name: &str,
    input: serde_json::Value,
) -> mlua::Result<ToolResult> {
    let tools = ctx.child_tools.as_ref().ok_or_else(|| {
        mlua::Error::external(ToolError::InvalidInput {
            tool: name.to_owned(),
            message: format!("{name} requires tool registry in ToolContext"),
        })
    })?;
    tools
        .run(name, ctx, input)
        .await
        .map_err(mlua::Error::external)
}

fn lua_value_to_json(lua: &Lua, value: Value) -> mlua::Result<serde_json::Value> {
    lua.from_value(value)
}

fn lua_return_to_json(lua: &Lua, value: Value) -> mlua::Result<Option<serde_json::Value>> {
    match value {
        Value::Nil => Ok(None),
        Value::UserData(userdata) => {
            if let Ok(handle) = userdata.borrow::<DelegateHandle>() {
                return Ok(Some(handle.to_json()));
            }
            if let Ok(handle) = userdata.borrow::<SwarmHandle>() {
                return Ok(Some(handle.to_json()));
            }
            Err(mlua::Error::external(
                "unsupported workflow return userdata",
            ))
        }
        other => lua_value_to_json(lua, other).map(Some),
    }
}

fn report_summary(value: &serde_json::Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    value.to_string()
}

fn delegate_handle_from_result(
    result: &ToolResult,
    agent: Option<&AgentSnapshot>,
) -> DelegateHandle {
    let summary = agent
        .and_then(|agent| agent.outcome.as_ref())
        .map(|outcome| outcome.summary.clone())
        .or_else(|| details_string(result, "summary"))
        .unwrap_or_else(|| result.content.clone());
    DelegateHandle {
        kind: "delegate",
        agent_id: agent
            .map(|agent| agent.id.as_str().to_owned())
            .or_else(|| details_string(result, "agent_id")),
        name: agent
            .map(|agent| agent.display_name.as_str().to_owned())
            .or_else(|| details_string(result, "display_name")),
        status: agent.map_or_else(
            || {
                details_string(result, "status")
                    .unwrap_or_else(|| status_from_result(result).to_owned())
            },
            |agent| agent.state.as_str().to_owned(),
        ),
        summary,
        result: result.details.clone().unwrap_or_else(|| json!({})),
    }
}

fn workflow_state_from_tool_and_agent(
    result: &ToolResult,
    agent: Option<&AgentSnapshot>,
) -> WorkflowState {
    if result.is_error {
        return WorkflowState::Failed;
    }
    let status = agent
        .map(|agent| agent.state.as_str().to_owned())
        .or_else(|| details_string(result, "status"));
    match status.as_deref() {
        Some("failed" | "cancelled" | "timed_out") => WorkflowState::Failed,
        Some("queued" | "running") => WorkflowState::Running,
        _ => WorkflowState::Completed,
    }
}

fn swarm_items_have_failures(items: &[serde_json::Value]) -> bool {
    items.iter().any(|item| {
        matches!(
            item.get("status").and_then(serde_json::Value::as_str),
            Some("failed" | "cancelled" | "timed_out")
        )
    })
}

fn swarm_summary(
    result: &ToolResult,
    details: Option<&serde_json::Value>,
    has_failures: bool,
) -> String {
    if let Some(details) = details {
        let status = if has_failures {
            "failed"
        } else {
            details
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(status_from_result(result))
        };
        let description = details
            .get("description")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("swarm");
        let item_count = details
            .get("items")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        return format!("{description}: {status} ({item_count} items)");
    }
    result.content.clone()
}

fn details_string(result: &ToolResult, key: &str) -> Option<String> {
    result
        .details
        .as_ref()
        .and_then(|details| details.get(key))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn status_from_result(result: &ToolResult) -> &'static str {
    if result.is_error {
        "failed"
    } else {
        "completed"
    }
}
