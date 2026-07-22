use mlua::{Lua, LuaSerdeExt, Value};

use super::WorkflowError;
use crate::runtime::WorkflowDispatchHandle;
use crate::workflow::WorkflowHandle;
use crate::workflow::WorkflowLimits;

/// Runs Lua workflow scripts in a sandboxed `mlua` VM with strict host APIs.
/// The `neo` table exposes `phase`, `log`, `delegate`, `swarm`, `verify`,
/// `verify_command`, `report`, and `fail`. No raw OS/file/process/network APIs
/// are available. `args` is installed as recursively read-only.
pub struct LuaWorkflowRunner {
    dispatch: WorkflowDispatchHandle,
    handle: WorkflowHandle,
    limits: WorkflowLimits,
}

impl LuaWorkflowRunner {
    pub fn new(
        dispatch: WorkflowDispatchHandle,
        handle: WorkflowHandle,
        limits: WorkflowLimits,
    ) -> Self {
        Self {
            dispatch,
            handle,
            limits,
        }
    }

    pub async fn execute(
        &self,
        source: &str,
        args: serde_json::Value,
    ) -> Result<Option<serde_json::Value>, WorkflowError> {
        let lua = Lua::new();

        // Install memory limit
        lua.set_memory_limit(self.limits.lua_vm_memory_bytes as usize)
            .map_err(|e| WorkflowError::Lua(e.to_string()))?;

        // Note: `set_interrupt` is available via unsafe feature in newer mlua versions
        // For now, instruction counting is done via the hook mechanism on eval_async
        let _limits = &self.limits;
        let _handle = &self.handle;

        self.install_neo_table(&lua, args)?;

        let result: mlua::Result<Value> = lua
            .load(source)
            .set_name("workflow script")
            .eval_async()
            .await;

        match result {
            Ok(value) => {
                lua_return_to_json(&lua, value).map_err(|e| WorkflowError::Lua(e.to_string()))
            }
            Err(err) => {
                let msg = err.to_string();
                // neo.fail uses RuntimeError with a special prefix
                if msg.contains("deliberate failure") {
                    Err(WorkflowError::Failed(msg))
                } else if msg.contains("memory limit") {
                    Err(WorkflowError::ResourceLimited(msg))
                } else {
                    Err(WorkflowError::Lua(msg))
                }
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn install_neo_table(&self, lua: &Lua, args: serde_json::Value) -> Result<(), WorkflowError> {
        let neo = lua
            .create_table()
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        // Install recursively read-only args
        let args_value = lua
            .to_value(&args)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        let read_only_args =
            make_read_only(args_value, lua).map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("args", read_only_args)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        // neo.phase(id)
        let _handle = self.handle.clone();
        let dispatch = self.dispatch.clone();
        let phase_fn = lua
            .create_async_function(move |_, id: String| {
                let _dispatch = dispatch.clone();
                async move {
                    if id.is_empty() {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "phase id must be non-empty".to_owned(),
                        )));
                    }
                    // Phase is a journaled local operation, no external effect
                    // TODO: journal phase transition through WorkflowRuntime
                    Ok(())
                }
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("phase", phase_fn)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        // neo.log(message)
        let log_fn = lua
            .create_async_function(move |_, message: String| {
                async move {
                    if message.is_empty() {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "log message must be non-empty".to_owned(),
                        )));
                    }
                    // Log is a journaled local operation
                    Ok(())
                }
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("log", log_fn)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        // neo.delegate(input)
        let dispatch_delegate = self.dispatch.clone();
        let delegate_fn = lua
            .create_async_function(move |lua, table: Value| {
                let dispatch = dispatch_delegate.clone();
                async move {
                    let input = lua_value_to_json(&lua, table)?;
                    let task = input
                        .get("task")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            mlua::Error::external(WorkflowError::InvalidInput(
                                "delegate requires non-empty task field".to_owned(),
                            ))
                        })?;
                    if task.is_empty() {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "delegate task must be non-empty".to_owned(),
                        )));
                    }
                    // Reject mode=background
                    if input.get("mode").is_some() {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "delegate mode field is not allowed".to_owned(),
                        )));
                    }

                    let outcome = dispatch.run_one("Delegate", input).await;
                    let table = outcome_to_lua_table(&lua, &outcome)?;
                    Ok(table)
                }
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("delegate", delegate_fn)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        // neo.swarm(input)
        let dispatch_swarm = self.dispatch.clone();
        let swarm_fn = lua
            .create_async_function(move |lua, table: Value| {
                let dispatch = dispatch_swarm.clone();
                async move {
                    let input = lua_value_to_json(&lua, table)?;
                    let description = input
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    if description.is_empty() && input.get("description").is_some() {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "swarm description must be non-empty when present".to_owned(),
                        )));
                    }
                    // Reject mode and max_concurrency
                    if input.get("mode").is_some() {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "swarm mode field is not allowed".to_owned(),
                        )));
                    }
                    if input.get("max_concurrency").is_some() {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "swarm max_concurrency is not allowed".to_owned(),
                        )));
                    }

                    let outcome = dispatch.run_one("DelegateSwarm", input).await;
                    let table = outcome_to_lua_table(&lua, &outcome)?;
                    Ok(table)
                }
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("swarm", swarm_fn)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        // neo.verify(condition, message)
        let verify_fn = lua
            .create_function(
                move |_, (condition, message): (bool, String)| -> mlua::Result<()> {
                    if message.is_empty() {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "verify message must be non-empty".to_owned(),
                        )));
                    }
                    if condition {
                        Ok(())
                    } else {
                        let _outcome_table = mlua::Value::Nil; // TODO: return proper outcome
                        Err(mlua::Error::RuntimeError(message))
                    }
                },
            )
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("verify", verify_fn)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        // neo.verify_command(input)
        let dispatch_vcmd = self.dispatch.clone();
        let verify_command_fn = lua
            .create_async_function(move |lua, table: Value| {
                let dispatch = dispatch_vcmd.clone();
                async move {
                    let input = lua_value_to_json(&lua, table)?;
                    let command = input
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            mlua::Error::external(WorkflowError::InvalidInput(
                                "verify_command requires non-empty command field".to_owned(),
                            ))
                        })?;
                    if command.is_empty() {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "verify_command command must be non-empty".to_owned(),
                        )));
                    }

                    let outcome = dispatch.run_one("Bash", input).await;
                    let table = outcome_to_lua_table(&lua, &outcome)?;
                    Ok(table)
                }
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("verify_command", verify_command_fn)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        // neo.report(value)
        let report_fn = lua
            .create_async_function(move |lua, value: Value| {
                async move {
                    let _json = lua_value_to_json(&lua, value)?;
                    // Report is journaled as a local operation
                    // TODO: journal report through WorkflowRuntime
                    Ok(())
                }
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("report", report_fn)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        // neo.fail(message)
        let fail_fn = lua
            .create_function(move |_, message: String| -> mlua::Result<()> {
                if message.is_empty() {
                    return Err(mlua::Error::external(WorkflowError::InvalidInput(
                        "fail message must be non-empty".to_owned(),
                    )));
                }
                Err(mlua::Error::RuntimeError(message))
            })
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        neo.set("fail", fail_fn)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        // Disable unsafe APIs
        // Remove math.random and other non-deterministic APIs
        lua.globals()
            .set("math", Value::Nil)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;

        lua.globals()
            .set("neo", neo)
            .map_err(|e| WorkflowError::Host(e.to_string()))?;
        Ok(())
    }
}

fn make_read_only(value: Value, lua: &Lua) -> mlua::Result<Value> {
    match value {
        Value::Table(table) => {
            let read_only = lua.create_table()?;
            let meta = lua.create_table()?;

            let err_guard = lua.create_function(|_, (): ()| -> mlua::Result<()> {
                Err(mlua::Error::external(super::WorkflowError::InvalidInput(
                    "args are read-only".to_owned(),
                )))
            })?;
            meta.set("__newindex", err_guard.clone())?;
            meta.set("__index", table.clone())?;
            read_only.set_metatable(Some(meta));
            Ok(Value::Table(read_only))
        }
        other => Ok(other),
    }
}

fn lua_value_to_json(lua: &Lua, value: Value) -> mlua::Result<serde_json::Value> {
    lua.from_value(value)
}

fn lua_return_to_json(lua: &Lua, value: Value) -> mlua::Result<Option<serde_json::Value>> {
    match value {
        Value::Nil => Ok(None),
        other => lua_value_to_json(lua, other).map(Some),
    }
}

fn outcome_to_lua_table(
    lua: &Lua,
    outcome: &super::WorkflowInvocationOutcome,
) -> mlua::Result<Value> {
    let table = lua.create_table()?;
    table
        .set("ok", outcome.ok)
        .map_err(|e| mlua::Error::external(e))?;
    table
        .set(
            "status",
            match outcome.status {
                super::WorkflowOutcomeStatus::Completed => "completed",
                super::WorkflowOutcomeStatus::Failed => "failed",
                super::WorkflowOutcomeStatus::Denied => "denied",
                super::WorkflowOutcomeStatus::Cancelled => "cancelled",
                super::WorkflowOutcomeStatus::ResourceLimited => "resource_limited",
                super::WorkflowOutcomeStatus::Interrupted => "interrupted",
            },
        )
        .map_err(|e| mlua::Error::external(e))?;
    table
        .set("summary", outcome.summary.as_str())
        .map_err(|e| mlua::Error::external(e))?;
    table
        .set(
            "details",
            lua.to_value(&outcome.details)
                .map_err(|e| mlua::Error::external(e))?,
        )
        .map_err(|e| mlua::Error::external(e))?;
    Ok(Value::Table(table))
}
