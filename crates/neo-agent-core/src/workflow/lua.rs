use std::collections::{BTreeMap, HashSet};
use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mlua::{
    Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, MultiValue, StdLib, Value, VmState,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::schema::CompiledSchema;
use super::state::WorkflowRevision;
use super::user_input::AwaitUserInput;
use super::{
    WorkflowError, WorkflowErrorCode, WorkflowHandle, WorkflowInvocationKind,
    WorkflowInvocationOutcome, WorkflowLimits, WorkflowOutcomeStatus,
};
use crate::multi_agent::{
    AgentRole, AgentRunMode, ChildPlan, ChildWorktreePolicy, DelegateContext, DelegateRequest,
    DelegateSwarmItem, DelegateSwarmRequest, SwarmResourceLimits, child_plans_from_delegate_swarm,
};
use crate::runtime::WorkflowDispatchHandle;
use crate::tools::{
    ToolError, is_workflow_tool_denied, validate_child_plans, validate_delegate_request,
    validate_swarm_request,
};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(default)]
    context: DelegateContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_allow: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_schema: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SwarmItem {
    /// Homogeneous DelegateSwarm adapter fields (title + value + prompt_template).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    /// Heterogeneous direct child fields (design §31.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<AgentRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<DelegateContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_allow: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_schema: Option<serde_json::Value>,
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
    fn parse_worktree(&self) -> Result<ChildWorktreePolicy, String> {
        match self
            .worktree
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None | Some("shared") => Ok(ChildWorktreePolicy::Shared),
            Some("isolated") => Ok(ChildWorktreePolicy::Isolated),
            Some(other) => Err(format!("worktree must be shared or isolated; got {other}")),
        }
    }

    /// New-child vs resume union (design §28.1). Resume may only carry
    /// `resume`, `task`, and `output_schema`.
    fn validate_union(&self) -> Result<(), String> {
        if self.resume.is_some() {
            if self.role.is_some()
                || self.model.is_some()
                || self.provider.is_some()
                || self.worktree.is_some()
                || self.tool_allow.is_some()
                || self.title.is_some()
            {
                return Err("resumed child accepts only resume, task, and output_schema".to_owned());
            }
            // context defaults to Inherit via serde; treat non-default as a policy field.
            if self.context != DelegateContext::Inherit {
                return Err("resumed child accepts only resume, task, and output_schema".to_owned());
            }
        }
        Ok(())
    }

    fn to_isolation_request(&self) -> Result<crate::workflow::ChildIsolationRequest, String> {
        self.validate_union()?;
        let worktree = self.parse_worktree()?;
        Ok(crate::workflow::ChildIsolationRequest {
            item_id: "delegate".to_owned(),
            context: if self.resume.is_some() {
                // Resumed children keep original context; request carries none of the
                // new-child policy fields.
                DelegateContext::Inherit
            } else {
                self.context
            },
            worktree,
            tool_allow: self.tool_allow.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            permission_mode: None,
        })
    }

    fn canonical_request(&self) -> Result<DelegateRequest, String> {
        self.validate_union()?;
        Ok(DelegateRequest {
            task: self.task.clone(),
            resume: self.resume.clone(),
            title: self.title.clone(),
            role: self.role,
            mode: AgentRunMode::Foreground,
            context: if self.resume.is_some() {
                // Resumed children inherit original context from the agent snapshot.
                DelegateContext::Inherit
            } else {
                self.context
            },
            output_schema: self.output_schema.clone(),
        })
    }
}

impl SwarmItem {
    fn is_homogeneous(&self) -> bool {
        self.task.is_none()
            && self.resume.is_none()
            && self.role.is_none()
            && self.model.is_none()
            && self.provider.is_none()
            && self.context.is_none()
            && self.worktree.is_none()
            && self.tool_allow.is_none()
            && self.output_schema.is_none()
    }

    fn parse_worktree(&self, index: usize) -> Result<ChildWorktreePolicy, String> {
        match self
            .worktree
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None | Some("shared") => Ok(ChildWorktreePolicy::Shared),
            Some("isolated") => Ok(ChildWorktreePolicy::Isolated),
            Some(other) => Err(format!(
                "items[{index}].worktree must be shared or isolated; got {other}"
            )),
        }
    }
}

impl SwarmInput {
    fn is_homogeneous_template_form(&self) -> bool {
        self.items.iter().all(SwarmItem::is_homogeneous)
    }

    fn canonical_request(&self, max_concurrency: usize) -> Result<DelegateSwarmRequest, String> {
        if !self.is_homogeneous_template_form() {
            return Err(
                "heterogeneous neo.swarm items cannot lower through the DelegateSwarm template adapter"
                    .to_owned(),
            );
        }
        let mut items = Vec::with_capacity(self.items.len());
        for (index, item) in self.items.iter().enumerate() {
            let title = item
                .title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("items[{index}].title must not be empty"))?
                .to_owned();
            let value = item
                .value
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| format!("items[{index}].value must not be empty"))?
                .to_owned();
            items.push(DelegateSwarmItem { title, value });
        }
        Ok(DelegateSwarmRequest {
            description: self.description.clone(),
            items,
            prompt_template: self.prompt_template.clone(),
            resume_agent_ids: self.resume_agent_ids.clone(),
            role: self.role,
            mode: AgentRunMode::Foreground,
            max_concurrency: Some(max_concurrency),
        })
    }

    /// Lower workflow `neo.swarm` input into canonical [`ChildPlan`]s.
    fn to_child_plans(&self, max_concurrency: usize) -> Result<Vec<ChildPlan>, String> {
        if self.is_homogeneous_template_form() {
            let request = self.canonical_request(max_concurrency)?;
            return child_plans_from_delegate_swarm(&request);
        }
        if !self.resume_agent_ids.is_empty() {
            return Err(
                "resume_agent_ids is only valid with homogeneous title/value swarm items"
                    .to_owned(),
            );
        }
        if self
            .prompt_template
            .as_deref()
            .is_some_and(|template| !template.trim().is_empty())
        {
            return Err(
                "prompt_template is only valid with homogeneous title/value swarm items".to_owned(),
            );
        }
        if self.items.is_empty() {
            return Err("items must contain at least one child".to_owned());
        }
        let mut plans = Vec::with_capacity(self.items.len());
        for (index, item) in self.items.iter().enumerate() {
            let task = if let Some(task) = item
                .task
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                task.to_owned()
            } else if item
                .value
                .as_deref()
                .map(str::trim)
                .is_some_and(|s| !s.is_empty())
            {
                // Mixed form with value but no task is not allowed for heterogeneous.
                return Err(format!(
                    "items[{index}].task is required for heterogeneous child specs"
                ));
            } else {
                return Err(format!("items[{index}].task must not be empty"));
            };
            let title = item
                .title
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let worktree = item.parse_worktree(index)?;
            // `output_schema` is optional projection metadata: when supplied and
            // valid it is compiled at the local boundary; when omitted the child
            // simply returns its ordinary result.
            let output_schema = item.output_schema.clone();
            if let Some(schema) = &output_schema {
                CompiledSchema::compile(schema)
                    .map_err(|error| format!("items[{index}].output_schema is invalid: {error}"))?;
            }
            plans.push(ChildPlan {
                item_id: format!("item-{index}"),
                item_label: title.clone().unwrap_or_else(|| task.clone()),
                task,
                title,
                resume: item.resume.clone(),
                role: item.role,
                model: item.model.clone(),
                provider: item.provider.clone(),
                context: item.context.unwrap_or(DelegateContext::None),
                worktree,
                tool_allow: item.tool_allow.clone(),
                output_schema,
            });
        }
        Ok(plans)
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolHostInput {
    name: String,
    input: serde_json::Value,
}

/// Maximum nesting depth for Lua-to-JSON conversion.
const MAX_JSON_DEPTH: usize = 128;

const JSON_KIND_META: &str = "__neo_json_kind";
const JSON_KIND_ARRAY: &str = "array";
const JSON_KIND_OBJECT: &str = "object";
const READONLY_BACKING: &str = "__neo_readonly_backing";

/// Runs Lua workflow scripts in a sandboxed `mlua` VM with strict host APIs.
pub struct LuaWorkflowRunner {
    dispatch: WorkflowDispatchHandle,
    handle: WorkflowHandle,
    limits: WorkflowLimits,
    final_schema: Option<CompiledSchema>,
    schema_revision: Option<WorkflowRevision>,
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
            final_schema: None,
            schema_revision: None,
        }
    }

    /// Attach the definition final `output_schema` validated after Lua returns.
    #[must_use]
    pub fn with_final_schema(
        mut self,
        schema: CompiledSchema,
        revision: Option<WorkflowRevision>,
    ) -> Self {
        self.final_schema = Some(schema);
        self.schema_revision = revision;
        self
    }

    /// Execute a sandboxed Lua script and return one canonical JSON value.
    ///
    /// - Zero returns and a single `nil` become JSON `null`.
    /// - Multiple return values fail.
    /// - On success the value is schema-validated (when configured) and persisted
    ///   as the run's canonical final result — never discarded by the binder.
    pub async fn execute(
        &self,
        source: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, WorkflowError> {
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
        let result: mlua::Result<MultiValue> = thread.into_async(()).await;

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
        let multi = match result {
            Ok(values) => values,
            Err(error) => return Err(map_lua_error(error)),
        };
        if multi.len() > 1 {
            return Err(WorkflowError::InvalidInput(
                "workflow script must return at most one value".to_owned(),
            ));
        }
        let raw = multi.into_iter().next().unwrap_or(Value::Nil);
        let value = lua_return_to_json(&lua, raw, self.limits.artifact_record_bytes)
            .map_err(map_lua_error)?;

        // Persist before returning so production binders that discard the
        // Result value still leave a durable FinalResultRecorded record.
        self.handle
            .accept_final_lua_result(
                value.clone(),
                self.final_schema.as_ref(),
                self.schema_revision.clone(),
            )
            .await?;
        Ok(value)
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
                    let request = input.canonical_request().map_err(|message| {
                        mlua::Error::external(WorkflowError::InvalidInput(message))
                    })?;
                    // Validate isolation fields (worktree grammar, resume union) before dispatch.
                    let _isolation = input.to_isolation_request().map_err(|message| {
                        mlua::Error::external(WorkflowError::InvalidInput(message))
                    })?;
                    validate_delegate_request("Delegate", &request)
                        .map_err(|error| invalid_tool_input(&error))?;
                    let input = canonical_input.clone();
                    let index = call_index.fetch_add(1, Ordering::Relaxed);
                    let origin = handle.execution_origin(None).await;
                    let outcome = Box::pin(handle.invoke(
                        index,
                        WorkflowInvocationKind::Delegate,
                        canonical_input,
                        true,
                        move |invocation| async move {
                            dispatch
                                .run_one_with_origin(invocation, "Delegate", input, Some(origin))
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
                    let plans = input.to_child_plans(max_concurrency).map_err(|message| {
                        mlua::Error::external(WorkflowError::InvalidInput(message))
                    })?;
                    // Resource validation is shared with DelegateSwarm — byte/field
                    // ceilings only; no total child-count cap.
                    let limits = SwarmResourceLimits::default();
                    validate_child_plans("DelegateSwarm", &input.description, &plans, limits)
                        .map_err(|error| invalid_tool_input(&error))?;
                    let index = call_index.fetch_add(1, Ordering::Relaxed);
                    let outcome = if input.is_homogeneous_template_form() {
                        let request =
                            input
                                .canonical_request(max_concurrency)
                                .map_err(|message| {
                                    mlua::Error::external(WorkflowError::InvalidInput(message))
                                })?;
                        validate_swarm_request("DelegateSwarm", &request)
                            .map_err(|error| invalid_tool_input(&error))?;
                        let mut tool_input = canonical_input.clone();
                        tool_input
                            .as_object_mut()
                            .expect("strict swarm input is an object")
                            .insert("max_concurrency".to_owned(), max_concurrency.into());
                        // Ensure homogeneous title/value shape reaches the tool.
                        if let Some(obj) = tool_input.as_object_mut() {
                            obj.insert(
                                "items".to_owned(),
                                serde_json::to_value(&request.items).unwrap_or_default(),
                            );
                            obj.insert(
                                "prompt_template".to_owned(),
                                serde_json::to_value(&request.prompt_template).unwrap_or_default(),
                            );
                        }
                        Box::pin(handle.invoke(
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
                        .map_err(mlua::Error::external)?
                    } else {
                        // Heterogeneous: durable per-item batch via runtime owner.
                        let description = input.description.clone();
                        let role = input.role;
                        let multi_agent = dispatch.config.multi_agent.clone();
                        let deps = crate::multi_agent::ChildRuntimeDeps::new(
                            dispatch.config.clone(),
                            std::sync::Arc::clone(&dispatch.model_client),
                            std::sync::Arc::clone(&dispatch.registry),
                        );
                        Box::pin(handle.invoke_swarm_batch(
                            crate::workflow::SwarmBatchRequest {
                                call_index: index,
                                canonical_input,
                                description,
                                role,
                                max_concurrency,
                                plans,
                            },
                            multi_agent,
                            deps,
                        ))
                        .await
                        .map_err(mlua::Error::external)?
                    };
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
                    // Verification is business data, not host execution state: a
                    // false condition returns a completed outcome whose details
                    // carry `verified = false` and the message. It never aborts.
                    let outcome = completed_outcome(
                        if condition {
                            "verification passed"
                        } else {
                            "verification failed"
                        },
                        json!({"message": message, "verified": condition}),
                    );
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
        neo.set("verify", host_verify)
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
                            if outcome.status != WorkflowOutcomeStatus::Completed
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
        neo.set("verify_command", host_verify_command)
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
                    let report = lua_value_to_json(&lua, value, 16 * 1024 * 1024)?;
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

        let dispatch = self.dispatch.clone();
        let handle = self.handle.clone();
        let call_index = Arc::clone(&next_call);
        let boundary = Arc::clone(instructions);
        let fatal = Arc::clone(fatal_reason);
        let tool = lua
            .create_async_function(move |lua, value: Value| {
                let dispatch = dispatch.clone();
                let handle = handle.clone();
                let call_index = Arc::clone(&call_index);
                let boundary = Arc::clone(&boundary);
                let fatal = Arc::clone(&fatal);
                async move {
                    check_fatal(&fatal)?;
                    let (input, canonical_input): (ToolHostInput, _) =
                        decode_input(&lua, value, "tool")?;
                    require_non_empty("tool name", &input.name)?;
                    if !input.input.is_object() {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "tool input must be a JSON object".to_owned(),
                        )));
                    }
                    let tool_name = input.name.clone();
                    let failure = if !dispatch.registry.contains(&tool_name) {
                        Some(("unknown_tool", format!("unknown tool: {tool_name}")))
                    } else if is_workflow_tool_denied(&tool_name) {
                        Some((
                            "tool_not_workflow_eligible",
                            format!("tool `{tool_name}` is not workflow-eligible"),
                        ))
                    } else {
                        None
                    };
                    if let Some((code, summary)) = failure {
                        let outcome = invoke_local(
                            &handle,
                            &call_index,
                            WorkflowInvocationKind::Tool,
                            canonical_input,
                            failed_outcome(summary, json!({"code": code, "tool": tool_name})),
                        )
                        .await?;
                        boundary.store(0, Ordering::Relaxed);
                        return immutable_outcome(&lua, &outcome);
                    }
                    // Same-run recursive TaskOutput would re-enter the workflow
                    // output lock/path; reject before durable start.
                    if input.name == "TaskOutput" {
                        let task_id = input
                            .input
                            .get("task_id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("");
                        if task_id == handle.run_id.as_str() {
                            return Err(mlua::Error::external(WorkflowError::coded(
                                WorkflowErrorCode::ToolNotWorkflowEligible,
                                "TaskOutput cannot target the current workflow run".to_owned(),
                            )));
                        }
                    }
                    let tool_input = input.input.clone();
                    let index = call_index.fetch_add(1, Ordering::Relaxed);
                    let origin = handle.execution_origin(None).await;
                    let outcome = Box::pin(handle.invoke(
                        index,
                        WorkflowInvocationKind::Tool,
                        canonical_input,
                        false,
                        move |invocation| async move {
                            dispatch
                                .run_one_with_origin(
                                    invocation,
                                    &tool_name,
                                    tool_input,
                                    Some(origin),
                                )
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
        neo.set("tool", tool)
            .map_err(|error| WorkflowError::Host(error.to_string()))?;

        let handle = self.handle.clone();
        let call_index = Arc::clone(&next_call);
        let boundary = Arc::clone(instructions);
        let fatal = Arc::clone(fatal_reason);
        let await_user = lua
            .create_async_function(move |lua, value: Value| {
                let handle = handle.clone();
                let call_index = Arc::clone(&call_index);
                let boundary = Arc::clone(&boundary);
                let fatal = Arc::clone(&fatal);
                async move {
                    check_fatal(&fatal)?;
                    let (input, _canonical): (AwaitUserInput, _) =
                        decode_input(&lua, value, "await_user")?;
                    // Schema/default compile before any durable effect.
                    let _prepared = input.prepare().map_err(mlua::Error::external)?;
                    let index = call_index.fetch_add(1, Ordering::Relaxed);
                    let answer = handle
                        .await_user(index, input)
                        .await
                        .map_err(mlua::Error::external)?;
                    boundary.store(0, Ordering::Relaxed);
                    let lua_value = lua.to_value(&answer).map_err(|error| {
                        mlua::Error::external(WorkflowError::Host(error.to_string()))
                    })?;
                    make_read_only(lua_value, &lua, "await_user answers are read-only")
                }
            })
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set("await_user", await_user)
            .map_err(|error| WorkflowError::Host(error.to_string()))?;

        let json_array = lua
            .create_function(|lua, value: Value| mark_json_container(lua, value, JsonMarker::Array))
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set("json_array", json_array)
            .map_err(|error| WorkflowError::Host(error.to_string()))?;

        let json_object = lua
            .create_function(|lua, value: Value| {
                mark_json_container(lua, value, JsonMarker::Object)
            })
            .map_err(|error| WorkflowError::Host(error.to_string()))?;
        neo.set("json_object", json_object)
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
    meta.raw_set(READONLY_BACKING, backing.clone())?;
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
    // Host inputs share the strict converter; size is enforced again at journal append.
    let value = lua_value_to_json(lua, value, 16 * 1024 * 1024)?;
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
        status: WorkflowOutcomeStatus::Failed,
        summary: summary.into(),
        interruption: None,
        details,
        actual_usage: None,
        child_refs: Vec::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonMarker {
    Array,
    Object,
}

impl JsonMarker {
    fn as_str(self) -> &'static str {
        match self {
            Self::Array => JSON_KIND_ARRAY,
            Self::Object => JSON_KIND_OBJECT,
        }
    }

    fn from_meta(kind: &str) -> Option<Self> {
        match kind {
            JSON_KIND_ARRAY => Some(Self::Array),
            JSON_KIND_OBJECT => Some(Self::Object),
            _ => None,
        }
    }
}

fn lua_value_to_json(lua: &Lua, value: Value, max_bytes: u64) -> mlua::Result<serde_json::Value> {
    let mut visiting = HashSet::new();
    let mut size = 0_u64;
    let converted = convert_lua_value(lua, value, 0, &mut visiting, max_bytes, &mut size)?;
    Ok(converted)
}

/// Top-level return: `nil` becomes JSON `null` (never omitted).
fn lua_return_to_json(lua: &Lua, value: Value, max_bytes: u64) -> mlua::Result<serde_json::Value> {
    lua_value_to_json(lua, value, max_bytes)
}

fn convert_lua_value(
    lua: &Lua,
    value: Value,
    depth: usize,
    visiting: &mut HashSet<*const c_void>,
    max_bytes: u64,
    size: &mut u64,
) -> mlua::Result<serde_json::Value> {
    if depth > MAX_JSON_DEPTH {
        return Err(conversion_error(
            "Lua table nesting exceeds maximum JSON depth",
        ));
    }
    match value {
        Value::Nil => {
            account_bytes(size, max_bytes, 4)?;
            Ok(serde_json::Value::Null)
        }
        Value::Boolean(b) => {
            account_bytes(size, max_bytes, if b { 4 } else { 5 })?;
            Ok(serde_json::Value::Bool(b))
        }
        Value::Integer(i) => {
            account_bytes(size, max_bytes, 20)?;
            Ok(serde_json::Value::Number(i.into()))
        }
        Value::Number(n) => {
            if !n.is_finite() {
                return Err(conversion_error(
                    "non-finite Lua numbers cannot convert to JSON",
                ));
            }
            account_bytes(size, max_bytes, 32)?;
            let number = serde_json::Number::from_f64(n)
                .ok_or_else(|| conversion_error("non-finite Lua numbers cannot convert to JSON"))?;
            Ok(serde_json::Value::Number(number))
        }
        Value::String(s) => {
            let text = s
                .to_str()
                .map_err(|_| conversion_error("Lua string is not valid UTF-8"))?;
            account_bytes(size, max_bytes, text.len() as u64 + 2)?;
            Ok(serde_json::Value::String(text.to_owned()))
        }
        Value::Table(table) => convert_table(lua, table, depth, visiting, max_bytes, size),
        Value::Function(_) => Err(conversion_error("functions cannot convert to JSON")),
        Value::Thread(_) => Err(conversion_error("threads cannot convert to JSON")),
        Value::UserData(_) => Err(conversion_error("userdata cannot convert to JSON")),
        Value::LightUserData(_) => Err(conversion_error("light userdata cannot convert to JSON")),
        Value::Error(err) => Err(conversion_error(&format!(
            "error values cannot convert to JSON: {err}"
        ))),
        other => Err(conversion_error(&format!(
            "unsupported Lua value for JSON conversion: {other:?}"
        ))),
    }
}

fn convert_table(
    lua: &Lua,
    table: mlua::Table,
    depth: usize,
    visiting: &mut HashSet<*const c_void>,
    max_bytes: u64,
    size: &mut u64,
) -> mlua::Result<serde_json::Value> {
    let ptr = table.to_pointer();
    if !visiting.insert(ptr) {
        return Err(conversion_error("cyclic Lua tables cannot convert to JSON"));
    }
    let result = (|| {
        let marker = table_json_marker(&table)?;
        let source = table_source(&table)?;
        let mut string_entries: Vec<(String, Value)> = Vec::new();
        let mut integer_entries: Vec<(i64, Value)> = Vec::new();
        for pair in source.pairs::<Value, Value>() {
            let (key, value) = pair?;
            match key {
                Value::String(s) => {
                    let text = s.to_str().map_err(|_| {
                        conversion_error("Lua object keys must be valid UTF-8 strings")
                    })?;
                    string_entries.push((text.to_owned(), value));
                }
                Value::Integer(i) => integer_entries.push((i, value)),
                Value::Number(_) => {
                    return Err(conversion_error(
                        "Lua table number keys must be integers for JSON conversion",
                    ));
                }
                _ => {
                    return Err(conversion_error(
                        "Lua table keys must be strings or integers for JSON conversion",
                    ));
                }
            }
        }

        let has_strings = !string_entries.is_empty();
        let has_integers = !integer_entries.is_empty();
        if has_strings && has_integers {
            return Err(conversion_error(
                "mixed string/integer Lua tables cannot convert to JSON",
            ));
        }

        let prefer = match marker {
            Some(JsonMarker::Array) => {
                if has_strings {
                    return Err(conversion_error(
                        "neo.json_array requires integer keys 1..n only",
                    ));
                }
                JsonMarker::Array
            }
            Some(JsonMarker::Object) => {
                if has_integers {
                    return Err(conversion_error(
                        "neo.json_object requires string keys only",
                    ));
                }
                JsonMarker::Object
            }
            None if !has_strings && !has_integers => JsonMarker::Object,
            None if has_integers => JsonMarker::Array,
            None => JsonMarker::Object,
        };

        match prefer {
            JsonMarker::Array => {
                integer_entries.sort_by_key(|(k, _)| *k);
                let n = integer_entries.len();
                for (idx, (key, _)) in integer_entries.iter().enumerate() {
                    let expected = i64::try_from(idx + 1)
                        .map_err(|_| conversion_error("Lua array length exceeds i64"))?;
                    if *key != expected {
                        return Err(conversion_error(
                            "sparse or non-1-based Lua arrays cannot convert to JSON",
                        ));
                    }
                }
                account_bytes(size, max_bytes, 2)?;
                let mut array = Vec::with_capacity(n);
                for (_, value) in integer_entries {
                    array.push(convert_lua_value(
                        lua,
                        value,
                        depth + 1,
                        visiting,
                        max_bytes,
                        size,
                    )?);
                    account_bytes(size, max_bytes, 1)?;
                }
                Ok(serde_json::Value::Array(array))
            }
            JsonMarker::Object => {
                account_bytes(size, max_bytes, 2)?;
                let mut map = serde_json::Map::new();
                // Deterministic key order for stable host hashing.
                string_entries.sort_by(|(a, _), (b, _)| a.cmp(b));
                for (key, value) in string_entries {
                    account_bytes(size, max_bytes, key.len() as u64 + 3)?;
                    let converted =
                        convert_lua_value(lua, value, depth + 1, visiting, max_bytes, size)?;
                    map.insert(key, converted);
                }
                Ok(serde_json::Value::Object(map))
            }
        }
    })();
    visiting.remove(&ptr);
    result
}

fn table_source(table: &mlua::Table) -> mlua::Result<mlua::Table> {
    if let Some(meta) = table.metatable()
        && let Ok(backing) = meta.raw_get::<mlua::Table>(READONLY_BACKING)
    {
        return Ok(backing);
    }
    Ok(table.clone())
}

fn table_json_marker(table: &mlua::Table) -> mlua::Result<Option<JsonMarker>> {
    let Some(meta) = table.metatable() else {
        return Ok(None);
    };
    match meta.raw_get::<Value>(JSON_KIND_META)? {
        Value::Nil => Ok(None),
        Value::String(s) => {
            let kind = s
                .to_str()
                .map_err(|_| conversion_error("invalid JSON marker metatable"))?;
            Ok(JsonMarker::from_meta(kind.as_ref()))
        }
        _ => Err(conversion_error("invalid JSON marker metatable")),
    }
}

fn mark_json_container(lua: &Lua, value: Value, marker: JsonMarker) -> mlua::Result<Value> {
    let Value::Table(table) = value else {
        return Err(mlua::Error::external(WorkflowError::InvalidInput(format!(
            "neo.json_{} requires a table",
            marker.as_str()
        ))));
    };
    // Validate shape against the marker before wrapping.
    let source = table_source(&table)?;
    let mut has_string = false;
    let mut has_integer = false;
    let mut integer_keys = Vec::new();
    for pair in source.pairs::<Value, Value>() {
        let (key, _) = pair?;
        match key {
            Value::String(_) => has_string = true,
            Value::Integer(i) => {
                has_integer = true;
                integer_keys.push(i);
            }
            Value::Number(_) => {
                return Err(mlua::Error::external(WorkflowError::InvalidInput(
                    "JSON container keys must be strings or integers".to_owned(),
                )));
            }
            _ => {
                return Err(mlua::Error::external(WorkflowError::InvalidInput(
                    "JSON container keys must be strings or integers".to_owned(),
                )));
            }
        }
    }
    match marker {
        JsonMarker::Array => {
            if has_string {
                return Err(mlua::Error::external(WorkflowError::InvalidInput(
                    "neo.json_array requires integer keys 1..n only".to_owned(),
                )));
            }
            if has_integer {
                integer_keys.sort_unstable();
                for (idx, key) in integer_keys.iter().enumerate() {
                    let expected = i64::try_from(idx + 1).map_err(|_| {
                        mlua::Error::external(WorkflowError::InvalidInput(
                            "neo.json_array length exceeds i64".to_owned(),
                        ))
                    })?;
                    if *key != expected {
                        return Err(mlua::Error::external(WorkflowError::InvalidInput(
                            "neo.json_array requires dense integer keys 1..n".to_owned(),
                        )));
                    }
                }
            }
        }
        JsonMarker::Object => {
            if has_integer {
                return Err(mlua::Error::external(WorkflowError::InvalidInput(
                    "neo.json_object requires string keys only".to_owned(),
                )));
            }
        }
    }
    wrap_json_marker(lua, table, marker)
}

fn wrap_json_marker(lua: &Lua, table: mlua::Table, marker: JsonMarker) -> mlua::Result<Value> {
    // Preserve existing read-only backing if present; otherwise copy.
    let backing = if let Some(meta) = table.metatable() {
        if let Ok(existing) = meta.raw_get::<mlua::Table>(READONLY_BACKING) {
            existing
        } else {
            deep_copy_table(lua, &table, 0)?
        }
    } else {
        deep_copy_table(lua, &table, 0)?
    };
    let read_only = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set("__index", backing.clone())?;
    meta.raw_set(READONLY_BACKING, backing.clone())?;
    meta.raw_set(JSON_KIND_META, marker.as_str())?;
    let next: Function = lua.globals().get("next")?;
    let iterator_backing = backing.clone();
    let iterator = lua.create_function(move |_, (_state, key): (Value, Value)| {
        next.call::<MultiValue>((iterator_backing.clone(), key))
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
                "json markers are immutable".to_owned(),
            )))
        })?,
    )?;
    meta.set("__metatable", "json-marker")?;
    read_only.set_metatable(Some(meta));
    Ok(Value::Table(read_only))
}

fn deep_copy_table(lua: &Lua, table: &mlua::Table, depth: usize) -> mlua::Result<mlua::Table> {
    if depth > MAX_JSON_DEPTH {
        return Err(conversion_error(
            "Lua table nesting exceeds maximum JSON depth",
        ));
    }
    let source = table_source(table)?;
    let copy = lua.create_table()?;
    for pair in source.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let value = match value {
            Value::Table(inner) => Value::Table(deep_copy_table(lua, &inner, depth + 1)?),
            other => other,
        };
        copy.raw_set(key, value)?;
    }
    Ok(copy)
}

fn account_bytes(size: &mut u64, max_bytes: u64, add: u64) -> mlua::Result<()> {
    *size = size.saturating_add(add);
    if *size > max_bytes {
        return Err(conversion_error(&format!(
            "JSON value exceeds configured byte limit {max_bytes}"
        )));
    }
    Ok(())
}

fn conversion_error(message: &str) -> mlua::Error {
    mlua::Error::external(WorkflowError::InvalidInput(message.to_owned()))
}

fn outcome_to_lua_table(lua: &Lua, outcome: &WorkflowInvocationOutcome) -> mlua::Result<Value> {
    let table = lua.create_table()?;
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
