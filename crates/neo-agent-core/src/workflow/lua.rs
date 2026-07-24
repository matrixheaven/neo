use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Value, VmState};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    WorkflowError, WorkflowHandle, WorkflowInvocationKind, WorkflowInvocationOutcome,
    WorkflowLimits, WorkflowOutcomeStatus,
};
use crate::multi_agent::{
    AgentRole, AgentRunMode, DelegateContext, DelegateRequest, DelegateSwarmItem,
    DelegateSwarmRequest,
};
use crate::runtime::WorkflowDispatchHandle;
use crate::tools::{ToolError, validate_delegate_request, validate_swarm_request};

const VERIFY_WRAPPER: &str = r"
return function(host_verify)
    return function(...)
        local outcome = host_verify(...)
        if outcome.ok then
            return nil
        end
        error(outcome, 0)
    end
end
";

const VERIFY_COMMAND_WRAPPER: &str = r"
return function(host_verify_command)
    return function(...)
        local outcome = host_verify_command(...)
        if outcome.ok then
            return outcome
        end
        error(outcome, 0)
    end
end
";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DelegateInput {
    task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<AgentRole>,
    #[serde(default)]
    context: DelegateContext,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SwarmItem {
    title: String,
    value: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SwarmInput {
    description: String,
    #[serde(default)]
    items: Vec<SwarmItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_template: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    resume_agent_ids: BTreeMap<String, String>,
    #[serde(default)]
    role: AgentRole,
}

impl DelegateInput {
    fn canonical_request(&self) -> DelegateRequest {
        DelegateRequest {
            task: self.task.clone(),
            resume: self.resume.clone(),
            title: self.title.clone(),
            role: self.role,
            mode: AgentRunMode::Foreground,
            context: self.context,
        }
    }
}

impl SwarmInput {
    fn canonical_request(&self, max_concurrency: usize) -> DelegateSwarmRequest {
        DelegateSwarmRequest {
            description: self.description.clone(),
            items: self
                .items
                .iter()
                .map(|item| DelegateSwarmItem {
                    title: item.title.clone(),
                    value: item.value.clone(),
                })
                .collect(),
            prompt_template: self.prompt_template.clone(),
            resume_agent_ids: self.resume_agent_ids.clone(),
            role: self.role,
            mode: AgentRunMode::Foreground,
            max_concurrency: Some(max_concurrency),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyCommandInput {
    command: String,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    failure_message: Option<String>,
}

/// Runs Lua workflow scripts in a sandboxed `mlua` VM with strict host APIs.
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
        let libs = StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH;
        let lua = Lua::new_with(libs, LuaOptions::default())
            .map_err(|error| WorkflowError::Lua(error.to_string()))?;
        let memory_limit = usize::try_from(self.limits.lua_vm_memory_bytes).map_err(|_| {
            WorkflowError::InvalidInput("Lua VM memory limit does not fit this platform".to_owned())
        })?;
        lua.set_memory_limit(memory_limit).map_err(map_lua_error)?;

        let interval = u32::try_from(self.limits.pause_hook_interval)
            .ok()
            .filter(|interval| *interval > 0)
            .ok_or_else(|| {
                WorkflowError::InvalidInput(
                    "Lua hook interval must be between 1 and u32::MAX".to_owned(),
                )
            })?;
        let instructions = Arc::new(AtomicU64::new(0));
        let resource_limited = Arc::new(AtomicBool::new(false));
        let fatal_reason = Arc::new(Mutex::new(None));
        self.install_neo_table(&lua, &args, &instructions, &fatal_reason)?;
        restrict_base_globals(&lua)?;

        let function = lua
            .load(source)
            .set_name("workflow script")
            .into_function()
            .map_err(map_lua_error)?;
        let thread = lua.create_thread(function).map_err(map_lua_error)?;
        self.install_hook(
            &thread,
            interval,
            Arc::clone(&instructions),
            Arc::clone(&resource_limited),
            Arc::clone(&fatal_reason),
        );
        let result: mlua::Result<Value> = thread.into_async(()).await;

        if let Some(reason) = fatal_message(&fatal_reason)? {
            return Err(WorkflowError::Failed(reason));
        }
        if resource_limited.load(Ordering::Acquire) {
            return Err(WorkflowError::ResourceLimited(format!(
                "Lua uninterrupted instruction limit {} reached",
                self.limits.max_uninterrupted_instructions
            )));
        }
        if self.handle.is_stop_requested() {
            return Err(WorkflowError::Cancelled(
                "workflow stop requested".to_owned(),
            ));
        }
        if self.handle.is_pause_requested() {
            return Err(WorkflowError::Paused("workflow pause requested".to_owned()));
        }
        match result {
            Ok(value) => lua_return_to_json(&lua, value).map_err(map_lua_error),
            Err(error) => Err(map_lua_error(error)),
        }
    }

    fn install_hook(
        &self,
        thread: &mlua::Thread,
        interval: u32,
        instructions: Arc<AtomicU64>,
        resource_limited: Arc<AtomicBool>,
        fatal_reason: Arc<Mutex<Option<String>>>,
    ) {
        let handle = self.handle.clone();
        let max_instructions = self.limits.max_uninterrupted_instructions;
        thread.set_hook(
            HookTriggers::new().every_nth_instruction(interval),
            move |_, _| {
                check_fatal(&fatal_reason)?;
                if handle.is_stop_requested() {
                    return Err(mlua::Error::external(WorkflowError::Cancelled(
                        "workflow stop requested".to_owned(),
                    )));
                }
                if handle.is_pause_requested() {
                    return Err(mlua::Error::external(WorkflowError::Paused(
                        "workflow pause requested".to_owned(),
                    )));
                }
                let executed = instructions
                    .fetch_add(u64::from(interval), Ordering::Relaxed)
                    .saturating_add(u64::from(interval));
                if executed >= max_instructions {
                    resource_limited.store(true, Ordering::Release);
                    return Err(mlua::Error::external(WorkflowError::ResourceLimited(
                        format!("Lua uninterrupted instruction limit {max_instructions} reached"),
                    )));
                }
                Ok(VmState::Continue)
            },
        );
    }

    fn install_neo_table(
        &self,
        lua: &Lua,
        args: &serde_json::Value,
        instructions: &Arc<AtomicU64>,
        fatal_reason: &Arc<Mutex<Option<String>>>,
    ) -> Result<(), WorkflowError> {
        let neo = lua
            .create_table()
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        let args_value = lua
            .to_value(&args)
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set(
            "args",
            make_read_only(args_value, lua, "args are read-only")
                .map_err(|error| WorkflowError::Host(error.to_string()))?,
        )
        .map_err(|error| WorkflowError::Host(error.to_string()))?;

        let next_call = Arc::new(AtomicU64::new(0));

        let handle = self.handle.clone();
        let call_index = Arc::clone(&next_call);
        let boundary = Arc::clone(instructions);
        let fatal = Arc::clone(fatal_reason);
        let phase = lua
            .create_async_function(move |_, id: String| {
                let handle = handle.clone();
                let call_index = Arc::clone(&call_index);
                let boundary = Arc::clone(&boundary);
                let fatal = Arc::clone(&fatal);
                async move {
                    check_fatal(&fatal)?;
                    require_non_empty("phase id", &id)?;
                    let output = handle.output().await.map_err(mlua::Error::external)?;
                    if !output.metadata.phases.iter().any(|phase| phase.id == id) {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(format!(
                            "unknown phase id: {id}"
                        ))));
                    }
                    let input = json!({"id": id});
                    let details = json!({"phase": id});
                    invoke_local(
                        &handle,
                        &call_index,
                        WorkflowInvocationKind::Phase,
                        input,
                        completed_outcome("phase selected", details),
                    )
                    .await?;
                    boundary.store(0, Ordering::Relaxed);
                    Ok(())
                }
            })
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set("phase", phase)
            .map_err(|error| WorkflowError::Host(error.to_string()))?;

        let handle = self.handle.clone();
        let call_index = Arc::clone(&next_call);
        let boundary = Arc::clone(instructions);
        let fatal = Arc::clone(fatal_reason);
        let log = lua
            .create_async_function(move |_, message: String| {
                let handle = handle.clone();
                let call_index = Arc::clone(&call_index);
                let boundary = Arc::clone(&boundary);
                let fatal = Arc::clone(&fatal);
                async move {
                    check_fatal(&fatal)?;
                    require_non_empty("log message", &message)?;
                    invoke_local(
                        &handle,
                        &call_index,
                        WorkflowInvocationKind::Log,
                        json!({"message": message}),
                        completed_outcome("log recorded", json!({"message": message})),
                    )
                    .await?;
                    boundary.store(0, Ordering::Relaxed);
                    Ok(())
                }
            })
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set("log", log)
            .map_err(|error| WorkflowError::Host(error.to_string()))?;

        let dispatch = self.dispatch.clone();
        let handle = self.handle.clone();
        let call_index = Arc::clone(&next_call);
        let boundary = Arc::clone(instructions);
        let fatal = Arc::clone(fatal_reason);
        let delegate = lua
            .create_async_function(move |lua, value: Value| {
                let dispatch = dispatch.clone();
                let handle = handle.clone();
                let call_index = Arc::clone(&call_index);
                let boundary = Arc::clone(&boundary);
                let fatal = Arc::clone(&fatal);
                async move {
                    check_fatal(&fatal)?;
                    let (input, canonical_input): (DelegateInput, _) =
                        decode_input(&lua, value, "delegate")?;
                    if input
                        .title
                        .as_deref()
                        .is_some_and(|title| title.trim().is_empty())
                    {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "delegate title must be non-empty when present".to_owned(),
                        )));
                    }
                    validate_delegate_request("Delegate", &input.canonical_request())
                        .map_err(|error| invalid_tool_input(&error))?;
                    let input = canonical_input.clone();
                    let index = call_index.fetch_add(1, Ordering::Relaxed);
                    let outcome = Box::pin(handle.invoke(
                        index,
                        WorkflowInvocationKind::Delegate,
                        canonical_input,
                        true,
                        move |invocation| async move {
                            dispatch.run_one(invocation, "Delegate", input).await
                        },
                    ))
                    .await
                    .map_err(mlua::Error::external)?;
                    boundary.store(0, Ordering::Relaxed);
                    immutable_outcome(&lua, &outcome)
                }
            })
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set("delegate", delegate)
            .map_err(|error| WorkflowError::Host(error.to_string()))?;

        let dispatch = self.dispatch.clone();
        let handle = self.handle.clone();
        let call_index = Arc::clone(&next_call);
        let boundary = Arc::clone(instructions);
        let fatal = Arc::clone(fatal_reason);
        let max_concurrency = self.limits.swarm_concurrency;
        let swarm = lua
            .create_async_function(move |lua, value: Value| {
                let dispatch = dispatch.clone();
                let handle = handle.clone();
                let call_index = Arc::clone(&call_index);
                let boundary = Arc::clone(&boundary);
                let fatal = Arc::clone(&fatal);
                async move {
                    check_fatal(&fatal)?;
                    let (input, canonical_input): (SwarmInput, _) =
                        decode_input(&lua, value, "swarm")?;
                    validate_swarm_request(
                        "DelegateSwarm",
                        &input.canonical_request(max_concurrency),
                    )
                    .map_err(|error| invalid_tool_input(&error))?;
                    let mut tool_input = canonical_input.clone();
                    tool_input
                        .as_object_mut()
                        .expect("strict swarm input is an object")
                        .insert("max_concurrency".to_owned(), max_concurrency.into());
                    let index = call_index.fetch_add(1, Ordering::Relaxed);
                    let outcome = Box::pin(handle.invoke(
                        index,
                        WorkflowInvocationKind::Swarm,
                        canonical_input,
                        true,
                        move |invocation| async move {
                            dispatch
                                .run_one(invocation, "DelegateSwarm", tool_input)
                                .await
                        },
                    ))
                    .await
                    .map_err(mlua::Error::external)?;
                    boundary.store(0, Ordering::Relaxed);
                    immutable_outcome(&lua, &outcome)
                }
            })
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set("swarm", swarm)
            .map_err(|error| WorkflowError::Host(error.to_string()))?;

        let handle = self.handle.clone();
        let call_index = Arc::clone(&next_call);
        let boundary = Arc::clone(instructions);
        let fatal = Arc::clone(fatal_reason);
        let host_verify = lua
            .create_async_function(move |lua, (condition, message): (bool, String)| {
                let handle = handle.clone();
                let call_index = Arc::clone(&call_index);
                let boundary = Arc::clone(&boundary);
                let fatal = Arc::clone(&fatal);
                async move {
                    check_fatal(&fatal)?;
                    require_non_empty("verify message", &message)?;
                    let outcome = if condition {
                        completed_outcome("verification passed", json!({"message": message}))
                    } else {
                        failed_outcome(message.clone(), json!({"message": message}))
                    };
                    let outcome = invoke_local(
                        &handle,
                        &call_index,
                        WorkflowInvocationKind::Verify,
                        json!({"condition": condition, "message": message}),
                        outcome,
                    )
                    .await?;
                    boundary.store(0, Ordering::Relaxed);
                    immutable_outcome(&lua, &outcome)
                }
            })
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set(
            "verify",
            wrap_host_function(lua, VERIFY_WRAPPER, host_verify)?,
        )
        .map_err(|error| WorkflowError::Host(error.to_string()))?;

        let dispatch = self.dispatch.clone();
        let handle = self.handle.clone();
        let call_index = Arc::clone(&next_call);
        let boundary = Arc::clone(instructions);
        let fatal = Arc::clone(fatal_reason);
        let host_verify_command = lua
            .create_async_function(move |lua, value: Value| {
                let dispatch = dispatch.clone();
                let handle = handle.clone();
                let call_index = Arc::clone(&call_index);
                let boundary = Arc::clone(&boundary);
                let fatal = Arc::clone(&fatal);
                async move {
                    check_fatal(&fatal)?;
                    let (input, _canonical): (VerifyCommandInput, _) =
                        decode_input(&lua, value, "verify_command")?;
                    require_non_empty("verify_command command", &input.command)?;
                    if let Some(message) = input.failure_message.as_deref() {
                        require_non_empty("verify_command failure_message", message)?;
                    }
                    let tool_input = json!({
                        "command": input.command,
                        "cwd": input.cwd,
                    });
                    let canonical_input = json!({
                        "command": input.command,
                        "cwd": input.cwd,
                        "failure_message": input.failure_message,
                    });
                    let failure_message = input.failure_message;
                    let index = call_index.fetch_add(1, Ordering::Relaxed);
                    let outcome = Box::pin(handle.invoke(
                        index,
                        WorkflowInvocationKind::VerifyCommand,
                        canonical_input,
                        false,
                        move |invocation| async move {
                            let mut outcome =
                                dispatch.run_one(invocation, "Bash", tool_input).await;
                            if !outcome.ok
                                && let Some(message) = failure_message
                            {
                                outcome.summary = message;
                            }
                            outcome
                        },
                    ))
                    .await
                    .map_err(mlua::Error::external)?;
                    boundary.store(0, Ordering::Relaxed);
                    immutable_outcome(&lua, &outcome)
                }
            })
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set(
            "verify_command",
            wrap_host_function(lua, VERIFY_COMMAND_WRAPPER, host_verify_command)?,
        )
        .map_err(|error| WorkflowError::Host(error.to_string()))?;

        let handle = self.handle.clone();
        let call_index = Arc::clone(&next_call);
        let boundary = Arc::clone(instructions);
        let fatal = Arc::clone(fatal_reason);
        let report = lua
            .create_async_function(move |lua, value: Value| {
                let handle = handle.clone();
                let call_index = Arc::clone(&call_index);
                let boundary = Arc::clone(&boundary);
                let fatal = Arc::clone(&fatal);
                async move {
                    check_fatal(&fatal)?;
                    let report = lua_value_to_json(&lua, value)?;
                    invoke_local(
                        &handle,
                        &call_index,
                        WorkflowInvocationKind::Report,
                        json!({"value": report}),
                        completed_outcome("report recorded", json!({"report": report})),
                    )
                    .await?;
                    boundary.store(0, Ordering::Relaxed);
                    Ok(())
                }
            })
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set("report", report)
            .map_err(|error| WorkflowError::Host(error.to_string()))?;

        let handle = self.handle.clone();
        let call_index = Arc::clone(&next_call);
        let boundary = Arc::clone(instructions);
        let fatal = Arc::clone(fatal_reason);
        let fail = lua
            .create_async_function(move |_, message: String| {
                let handle = handle.clone();
                let call_index = Arc::clone(&call_index);
                let boundary = Arc::clone(&boundary);
                let fatal = Arc::clone(&fatal);
                async move {
                    check_fatal(&fatal)?;
                    require_non_empty("fail message", &message)?;
                    let recorded = message.clone();
                    let outcome = failed_outcome(message.clone(), json!({"message": message}));
                    invoke_local(
                        &handle,
                        &call_index,
                        WorkflowInvocationKind::Fail,
                        json!({"message": message}),
                        outcome,
                    )
                    .await?;
                    boundary.store(0, Ordering::Relaxed);
                    *fatal.lock().map_err(|_| {
                        mlua::Error::external(WorkflowError::Host(
                            "workflow fail state lock poisoned".to_owned(),
                        ))
                    })? = Some(recorded.clone());
                    Err::<(), _>(mlua::Error::RuntimeError(recorded))
                }
            })
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set("fail", fail)
            .map_err(|error| WorkflowError::Host(error.to_string()))?;

        lua.globals()
            .set("neo", neo)
            .map_err(|error| WorkflowError::Host(error.to_string()))
    }
}

async fn invoke_local(
    handle: &WorkflowHandle,
    call_index: &AtomicU64,
    kind: WorkflowInvocationKind,
    input: serde_json::Value,
    outcome: WorkflowInvocationOutcome,
) -> mlua::Result<WorkflowInvocationOutcome> {
    let index = call_index.fetch_add(1, Ordering::Relaxed);
    handle
        .invoke(index, kind, input, false, move |_| async move { outcome })
        .await
        .map_err(mlua::Error::external)
}

fn restrict_base_globals(lua: &Lua) -> Result<(), WorkflowError> {
    let globals = lua.globals();
    for name in ["dofile", "loadfile", "print", "rawset"] {
        globals
            .set(name, Value::Nil)
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
    }
    let math: mlua::Table = globals
        .get("math")
        .map_err(|error| WorkflowError::Host(error.to_string()))?;
    math.set("random", Value::Nil)
        .and_then(|()| math.set("randomseed", Value::Nil))
        .map_err(|error| WorkflowError::Host(error.to_string()))
}

fn wrap_host_function(
    lua: &Lua,
    wrapper_source: &str,
    host: Function,
) -> Result<Function, WorkflowError> {
    let factory: Function = lua
        .load(wrapper_source)
        .eval()
        .map_err(|error| WorkflowError::Host(error.to_string()))?;
    factory
        .call(host)
        .map_err(|error| WorkflowError::Host(error.to_string()))
}

fn make_read_only(value: Value, lua: &Lua, message: &'static str) -> mlua::Result<Value> {
    let Value::Table(table) = value else {
        return Ok(value);
    };
    let backing = lua.create_table()?;
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        backing.raw_set(key, make_read_only(value, lua, message)?)?;
    }
    let read_only = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set("__index", backing.clone())?;
    meta.raw_set("__neo_readonly_backing", backing.clone())?;
    let next: Function = lua.globals().get("next")?;
    let iterator_backing = backing.clone();
    let iterator = lua.create_function(move |_, (_state, key): (Value, Value)| {
        next.call::<mlua::MultiValue>((iterator_backing.clone(), key))
    })?;
    meta.set(
        "__pairs",
        lua.create_function(move |_, _: Value| Ok((iterator.clone(), Value::Nil, Value::Nil)))?,
    )?;
    meta.set(
        "__len",
        lua.create_function(move |_, _: Value| Ok(backing.raw_len()))?,
    )?;
    meta.set(
        "__newindex",
        lua.create_function(move |_, (_table, _key, _value): (Value, Value, Value)| {
            Err::<(), _>(mlua::Error::external(WorkflowError::InvalidOperation(
                message.to_owned(),
            )))
        })?,
    )?;
    meta.set("__metatable", "read-only")?;
    read_only.set_metatable(Some(meta));
    Ok(Value::Table(read_only))
}

fn decode_input<T>(lua: &Lua, value: Value, api: &str) -> mlua::Result<(T, serde_json::Value)>
where
    T: serde::de::DeserializeOwned,
{
    let value = lua_value_to_json(lua, value)?;
    let decoded = serde_json::from_value(value.clone()).map_err(|error| {
        mlua::Error::external(WorkflowError::InvalidInput(format!("{api}: {error}")))
    })?;
    Ok((decoded, value))
}

fn invalid_tool_input(error: &ToolError) -> mlua::Error {
    mlua::Error::external(WorkflowError::InvalidInput(error.to_string()))
}

fn require_non_empty(field: &str, value: &str) -> mlua::Result<()> {
    if value.is_empty() {
        return Err(mlua::Error::external(WorkflowError::InvalidInput(format!(
            "{field} must be non-empty"
        ))));
    }
    Ok(())
}

fn completed_outcome(
    summary: impl Into<String>,
    details: serde_json::Value,
) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        ok: true,
        status: WorkflowOutcomeStatus::Completed,
        summary: summary.into(),
        interruption: None,
        details,
        actual_usage: None,
        child_refs: Vec::new(),
    }
}

fn failed_outcome(
    summary: impl Into<String>,
    details: serde_json::Value,
) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        ok: false,
        status: WorkflowOutcomeStatus::Failed,
        summary: summary.into(),
        interruption: None,
        details,
        actual_usage: None,
        child_refs: Vec::new(),
    }
}

fn lua_value_to_json(lua: &Lua, value: Value) -> mlua::Result<serde_json::Value> {
    lua.from_value(thaw_read_only(lua, value, 0)?)
}

fn thaw_read_only(lua: &Lua, value: Value, depth: usize) -> mlua::Result<Value> {
    if depth >= 128 {
        return Err(mlua::Error::SerializeError(
            "Lua table nesting exceeds 128 levels".to_owned(),
        ));
    }
    let Value::Table(table) = value else {
        return Ok(value);
    };
    let source = table
        .metatable()
        .and_then(|meta| meta.raw_get::<mlua::Table>("__neo_readonly_backing").ok())
        .unwrap_or(table);
    let copy = lua.create_table()?;
    for pair in source.pairs::<Value, Value>() {
        let (key, value) = pair?;
        copy.raw_set(
            thaw_read_only(lua, key, depth + 1)?,
            thaw_read_only(lua, value, depth + 1)?,
        )?;
    }
    Ok(Value::Table(copy))
}

fn lua_return_to_json(lua: &Lua, value: Value) -> mlua::Result<Option<serde_json::Value>> {
    match value {
        Value::Nil => Ok(None),
        other => lua_value_to_json(lua, other).map(Some),
    }
}

fn outcome_to_lua_table(lua: &Lua, outcome: &WorkflowInvocationOutcome) -> mlua::Result<Value> {
    let table = lua.create_table()?;
    table.set("ok", outcome.ok)?;
    table.set(
        "status",
        match outcome.status {
            WorkflowOutcomeStatus::Completed => "completed",
            WorkflowOutcomeStatus::Failed => "failed",
            WorkflowOutcomeStatus::Denied => "denied",
            WorkflowOutcomeStatus::Cancelled => "cancelled",
            WorkflowOutcomeStatus::ResourceLimited => "resource_limited",
            WorkflowOutcomeStatus::Interrupted => "interrupted",
        },
    )?;
    table.set("summary", outcome.summary.as_str())?;
    table.set("details", lua.to_value(&outcome.details)?)?;
    if let Some(usage) = outcome.actual_usage {
        table.set("actual_usage", lua.to_value(&usage)?)?;
    }
    for child in &outcome.child_refs {
        let field = match child.kind.as_str() {
            "delegate" => "agent_id",
            "delegate_swarm" => "swarm_id",
            "task" => "task_id",
            _ => continue,
        };
        if !table.contains_key(field)? {
            table.set(field, child.id.as_str())?;
        }
    }
    Ok(Value::Table(table))
}

fn immutable_outcome(lua: &Lua, outcome: &WorkflowInvocationOutcome) -> mlua::Result<Value> {
    make_read_only(
        outcome_to_lua_table(lua, outcome)?,
        lua,
        "workflow outcomes are read-only",
    )
}

fn fatal_message(fatal: &Mutex<Option<String>>) -> Result<Option<String>, WorkflowError> {
    fatal
        .lock()
        .map_err(|_| WorkflowError::Host("workflow fail state lock poisoned".to_owned()))
        .map(|reason| reason.clone())
}

fn check_fatal(fatal: &Mutex<Option<String>>) -> mlua::Result<()> {
    if let Some(reason) = fatal_message(fatal).map_err(mlua::Error::external)? {
        return Err(mlua::Error::external(WorkflowError::Failed(reason)));
    }
    Ok(())
}

fn map_lua_error(error: mlua::Error) -> WorkflowError {
    for source in error.chain() {
        if let Some(error) = source.downcast_ref::<WorkflowError>() {
            return error.clone();
        }
    }
    match error {
        mlua::Error::MemoryError(message) => WorkflowError::ResourceLimited(message),
        other => WorkflowError::Lua(other.to_string()),
    }
}
