use mlua::{Lua, LuaSerdeExt, Value};
use serde_json::json;

use super::host_api::WorkflowHostRecorder;
use super::{WorkflowError, WorkflowId, WorkflowSnapshot, WorkflowState, WorkflowStepRecord};
use crate::AgentEvent;
use crate::multi_agent::{AgentLifecycleState, AgentSnapshot, SwarmSnapshot};
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
            .eval_async::<Value>()
            .await
            .map_err(|err| WorkflowError::Lua(err.to_string()))?;
        Ok(lua_return_to_json(&lua, value).map_err(|err| WorkflowError::Lua(err.to_string()))?)
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
                    let agent = result
                        .details
                        .as_ref()
                        .and_then(|details| details.get("agent"))
                        .and_then(|agent| {
                            serde_json::from_value::<AgentSnapshot>(agent.clone()).ok()
                        });
                    let handle = delegate_handle_from_result(&result, agent.as_ref());
                    recorder.push_step(workflow_step(
                        format!("delegate: {task}"),
                        workflow_state_from_tool_and_agent(&result, agent.as_ref()),
                        Some(handle.summary.clone()),
                        result.details.clone(),
                        agent,
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
                    let swarm = result
                        .details
                        .as_ref()
                        .and_then(|details| details.get("swarm"))
                        .and_then(|swarm| {
                            serde_json::from_value::<SwarmSnapshot>(swarm.clone()).ok()
                        });
                    let has_failures = swarm
                        .as_ref()
                        .is_some_and(|snapshot| swarm_has_failures(snapshot))
                        || result.is_error;
                    let summary = swarm_summary(&result, swarm.as_ref(), has_failures);
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
                        swarm,
                        Some(has_failures),
                    ));
                    emit_workflow_update(&ctx, &event_context, &recorder);
                    Ok(SwarmHandle {
                        summary,
                        has_failures,
                    })
                }
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("swarm", swarm)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        let verify_ctx = ctx.clone();
        let recorder_vf = self.recorder.clone();
        let verify_events = event_context;
        let verify = lua
            .create_async_function(move |_, command: String| {
                let ctx = verify_ctx.clone();
                let recorder = recorder_vf.clone();
                let event_context = verify_events.clone();
                async move {
                    recorder.record(format!("verify: {command}"));
                    let result = run_tool(&ctx, "Bash", json!({ "command": command })).await?;
                    let passed = !result.is_error;
                    recorder.push_step(workflow_step(
                        "verify",
                        if passed {
                            WorkflowState::Completed
                        } else {
                            WorkflowState::Failed
                        },
                        Some(result.content.clone()),
                        result.details,
                        None,
                        None,
                        None,
                    ));
                    emit_workflow_update(&ctx, &event_context, &recorder);
                    Ok(passed)
                }
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("verify", verify)
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
#[derive(Debug, Clone)]
struct DelegateHandle {
    summary: String,
    status: String,
    name: Option<String>,
    id: Option<String>,
}

impl mlua::UserData for DelegateHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("summary", |_, this, _: ()| Ok(this.summary.clone()));
        methods.add_method("status", |_, this, _: ()| Ok(this.status.clone()));
        methods.add_method("name", |_, this, _: ()| Ok(this.name.clone()));
        methods.add_method("id", |_, this, _: ()| Ok(this.id.clone()));
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, this, _: ()| {
            Ok(this.id.clone().unwrap_or_else(|| this.summary.clone()))
        });
    }
}

/// Handle returned by `neo.swarm()`.
#[derive(Debug, Clone)]
struct SwarmHandle {
    summary: String,
    has_failures: bool,
}

impl mlua::UserData for SwarmHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("summary", |_, this, _: ()| Ok(this.summary.clone()));
        methods.add_method("has_failures", |_, this, _: ()| Ok(this.has_failures));
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
    if matches!(value, Value::Nil) {
        return Ok(None);
    }
    lua_value_to_json(lua, value).map(Some)
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
        .map_or_else(|| result.content.clone(), |outcome| outcome.summary.clone());
    DelegateHandle {
        summary,
        status: agent.map_or_else(
            || status_from_result(result).to_owned(),
            |agent| agent_state_name(agent.state).to_owned(),
        ),
        name: agent.map(|agent| agent.display_name.as_str().to_owned()),
        id: agent.map(|agent| agent.id.as_str().to_owned()),
    }
}

fn workflow_state_from_tool_and_agent(
    result: &ToolResult,
    agent: Option<&AgentSnapshot>,
) -> WorkflowState {
    if result.is_error {
        return WorkflowState::Failed;
    }
    match agent.map(|agent| agent.state) {
        Some(AgentLifecycleState::Failed | AgentLifecycleState::Cancelled) => WorkflowState::Failed,
        Some(AgentLifecycleState::Queued | AgentLifecycleState::Running) => WorkflowState::Running,
        _ => WorkflowState::Completed,
    }
}

fn swarm_has_failures(swarm: &SwarmSnapshot) -> bool {
    swarm.children.iter().any(|child| {
        matches!(
            child.agent.state,
            AgentLifecycleState::Failed | AgentLifecycleState::Cancelled
        ) || child
            .agent
            .outcome
            .as_ref()
            .is_some_and(|outcome| outcome.is_error)
    })
}

fn swarm_summary(result: &ToolResult, swarm: Option<&SwarmSnapshot>, has_failures: bool) -> String {
    if let Some(swarm) = swarm {
        let status = if has_failures { "failed" } else { "completed" };
        return format!(
            "{}: {status} ({} items)",
            swarm.description,
            swarm.children.len()
        );
    }
    result.content.clone()
}

fn status_from_result(result: &ToolResult) -> &'static str {
    if result.is_error {
        "failed"
    } else {
        "completed"
    }
}

fn agent_state_name(state: AgentLifecycleState) -> &'static str {
    match state {
        AgentLifecycleState::Queued => "queued",
        AgentLifecycleState::Running => "running",
        AgentLifecycleState::Completed => "completed",
        AgentLifecycleState::Failed => "failed",
        AgentLifecycleState::Cancelled => "cancelled",
    }
}
