use futures::{StreamExt, stream};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::multi_agent_format::{
    SummaryScope, agent_details, context_mode_label, delegate_result_content,
    model_safe_swarm_snapshot, swarm_details,
};
use super::{
    Tool, ToolContext, ToolError, ToolEventCallback, ToolFuture, ToolResult, parse_input, schema,
};
use crate::AgentEvent;
use crate::multi_agent::{
    AgentLifecycleState, AgentProfile, AgentRunMode, ChildRuntimeDeps, DelegateContext,
    DelegateRequest, DelegateSwarmRequest, SwarmAggregate, SwarmChildProgress, SwarmChildSnapshot,
    SwarmSnapshot, apply_agent_progress, apply_swarm_template,
};

type SwarmProgressUpdate = (SwarmChildProgress, SwarmAggregate, AgentLifecycleState);

async fn publish_swarm_progress(
    event_callback: Option<&ToolEventCallback>,
    background: Option<&(crate::BackgroundTaskManager, String)>,
    turn: u32,
    swarm_id: &str,
    (child_progress, aggregate, state): SwarmProgressUpdate,
) {
    if let Some(callback) = event_callback {
        callback(AgentEvent::DelegateSwarmProgressUpdated {
            turn,
            swarm_id: swarm_id.to_owned(),
            state,
            aggregate,
            child_progress: child_progress.clone(),
        });
    }
    if let Some((manager, task_id)) = background {
        manager
            .update_delegate_swarm_progress(task_id, child_progress, aggregate, state)
            .await;
    }
}

/// Build the Delegate/DelegateSwarm input schema with the per-role selection
/// guide appended to the `role` field description, so the main agent knows when
/// to pick Coder vs Explorer vs Planner vs Reviewer. Without this the model
/// defaults to Coder and the specialisms are never used.
fn schema_with_role_guide<T>() -> serde_json::Value
where
    T: schemars::JsonSchema,
{
    let mut schema = schema::<T>();
    let Some(props) = schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return schema;
    };
    let Some(role) = props.get_mut("role") else {
        return schema;
    };
    // Read the existing description out, then overwrite — done as two steps so
    // the shared borrow from the read ends before the mutable assign.
    let old = role
        .get("description")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let merged = format!("{old}\n\n{}", AgentProfile::role_selection_guide());
    role["description"] = serde_json::Value::String(merged);
    if let Some(resume_agent_ids) = props.get_mut("resume_agent_ids") {
        resume_agent_ids["type"] = serde_json::Value::String("object".to_owned());
        resume_agent_ids["additionalProperties"] = serde_json::json!({
            "type": "string",
            "description": "Prompt used when resuming that specific agent_id."
        });
    }
    schema
}

pub struct DelegateTool;

impl Tool for DelegateTool {
    fn name(&self) -> &'static str {
        "Delegate"
    }

    fn description(&self) -> &'static str {
        "Delegate work to a subagent. Default mode is foreground, so the main agent waits for the result. \
         Use mode=\"background\" only when the main agent should continue in parallel. \
         To continue an existing completed/failed/cancelled/timed_out agent, pass resume=\"agent_xxx\" and a new task; this starts a new run on the same agent. \
         When resume is set, role must be omitted because the resumed agent keeps its original role/profile/name/history. \
         context controls parent context passed to the child: inherit passes selected parent context, summary passes a compact parent summary, and none passes only the task plus role/profile prompt."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_with_role_guide::<DelegateRequest>()
    }

    #[allow(clippy::too_many_lines)]
    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let request: DelegateRequest = parse_input(self.name(), input)?;
            if let Err(err) = validate_delegate_request(self.name(), &request) {
                return Ok(ToolResult::error(err.to_string()));
            }
            let mut deps = child_runtime_deps(ctx)?;
            // Set the subagent role for tool filtering and profile enforcement.
            // For resumed agents, keep their original role from the snapshot.
            deps.role = request.actual_role();
            let turn = ctx.current_turn.unwrap_or_default();

            let snapshot = if let Some(agent_id) = request.resume.as_deref() {
                match ctx.multi_agent.start_resume_delegate(agent_id, &request) {
                    Ok(snapshot) => {
                        deps.role = snapshot.role;
                        snapshot
                    }
                    Err(message) => return Ok(ToolResult::error(message)),
                }
            } else {
                ctx.multi_agent.start_delegate(
                    &request.task,
                    request.title.as_deref(),
                    request.actual_role(),
                    request.mode,
                    request.context,
                    crate::multi_agent::AgentPathKind::Root,
                )
            };

            // Background mode: register the agent in the background task manager
            // and return immediately.
            if request.mode == AgentRunMode::Background {
                deps = deps.with_cancel_token(CancellationToken::new());
                ctx.emit_event(AgentEvent::DelegateStarted {
                    turn,
                    agent: snapshot.clone(),
                });
                let task_id = ctx.background_tasks.start_delegate(snapshot.clone()).await;
                let runtime = ctx.multi_agent.clone();
                let background_tasks = ctx.background_tasks.clone();
                let request_for_worker = request.clone();
                let task_id_for_worker = task_id.clone();
                let snapshot_for_worker = snapshot.clone();
                let event_callback = ctx.tool_event.clone();
                tokio::spawn(async move {
                    let callback = event_callback.clone();
                    let output = runtime
                        .run_started_child_turn(
                            deps,
                            snapshot_for_worker,
                            request_for_worker.context,
                            move |agent| {
                                if let Some(callback) = &callback {
                                    callback(AgentEvent::DelegateUpdated { turn, agent });
                                }
                            },
                        )
                        .await;
                    if let Some(callback) = &event_callback {
                        callback(AgentEvent::DelegateFinished {
                            turn,
                            agent: output.snapshot.clone(),
                        });
                    }
                    background_tasks
                        .finish_delegate(&task_id_for_worker, output.snapshot)
                        .await;
                });
                return Ok(ToolResult::ok(format!(
                    "agent_id: {}\nname: {}\nkind: delegate\nstatus: running\nrun_index: {}\ncontext_mode: {}\nnext_step: Call WaitDelegate with this agent_id to wait for completion.",
                    snapshot.id.as_str(),
                    snapshot.display_name.as_str(),
                    snapshot.run_count,
                    context_mode_label(request.context),
                ))
                .with_details({
                    let mut details = agent_details(
                        "delegate",
                        &snapshot,
                        Some(request.context),
                        SummaryScope::CurrentRun,
                        true,
                        false,
                        false,
                    );
                    details["mode"] = json!("background");
                    details["task_id"] = json!(task_id);
                    details
                }));
            }

            // Foreground mode: run synchronously and return the result.
            ctx.emit_event(AgentEvent::DelegateStarted {
                turn,
                agent: snapshot.clone(),
            });
            let output = ctx
                .multi_agent
                .run_started_child_turn(deps, snapshot, request.context, |agent| {
                    ctx.emit_event(AgentEvent::DelegateUpdated { turn, agent });
                })
                .await;
            let completed = output.snapshot;
            ctx.emit_event(AgentEvent::DelegateFinished {
                turn,
                agent: completed.clone(),
            });
            Ok(
                ToolResult::ok(delegate_result_content(&completed, request.context)).with_details(
                    agent_details(
                        "delegate",
                        &completed,
                        Some(request.context),
                        SummaryScope::CurrentRun,
                        true,
                        true,
                        false,
                    ),
                ),
            )
        })
    }
}

pub struct DelegateSwarmTool;

impl Tool for DelegateSwarmTool {
    fn name(&self) -> &'static str {
        "DelegateSwarm"
    }

    fn description(&self) -> &'static str {
        "Run many related bounded tasks in subagents and return an ordered aggregate result. \
         Default mode is foreground; background returns immediately and exposes the same structured swarm result through WaitDelegate and TaskOutput. \
         Required: description, and either items with prompt_template containing {{item}}, resume_agent_ids, or both. \
         Optional {{description}} inserts the swarm description. Only {{item}} and {{description}} placeholders are supported."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema_with_role_guide::<DelegateSwarmRequest>()
    }

    #[allow(clippy::too_many_lines)]
    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let request = parse_delegate_swarm_input(self.name(), input)?;
            validate_swarm_request(self.name(), &request)?;
            let mut deps = child_runtime_deps(ctx)?;
            deps.role = request.role;
            let turn = ctx.current_turn.unwrap_or_default();
            let swarm_id = ctx.multi_agent.new_swarm_id();
            let total_children = request.items.len() + request.resume_agent_ids.len();
            let max_concurrency = request
                .max_concurrency
                .unwrap_or(1)
                .clamp(1, total_children);
            // Build initial children from both new items and resumed agents.
            let mut initial_children = Vec::new();
            let mut item_index = 0usize;
            for item in &request.items {
                let template = request.prompt_template.as_deref().unwrap_or("");
                let task =
                    apply_swarm_template(template, item.value.as_str(), &request.description);
                let snapshot = ctx.multi_agent.queue_delegate(
                    &task,
                    Some(item.title.as_str()),
                    request.role,
                    request.mode,
                    DelegateContext::None,
                    crate::multi_agent::AgentPathKind::SwarmChild(&swarm_id),
                );
                initial_children.push(SwarmChildSnapshot {
                    item_index,
                    item: item.value.clone(),
                    agent: snapshot,
                });
                item_index += 1;
            }
            // Resume existing agents as swarm children.
            for (agent_id, prompt) in &request.resume_agent_ids {
                let resume_request = DelegateRequest {
                    task: prompt.clone(),
                    resume: Some(agent_id.clone()),
                    title: None,
                    role: None,
                    mode: request.mode,
                    context: DelegateContext::None,
                };
                let agent_snapshot = ctx
                    .multi_agent
                    .start_resume_delegate(agent_id, &resume_request)
                    .map_err(|message| ToolError::InvalidInput {
                        tool: "DelegateSwarm".to_owned(),
                        message,
                    })?;
                initial_children.push(SwarmChildSnapshot {
                    item_index,
                    item: format!("resume:{agent_id}"),
                    agent: agent_snapshot,
                });
                item_index += 1;
            }
            let initial_aggregate =
                SwarmAggregate::from_states(initial_children.iter().map(|c| c.agent.state));
            let initial_snapshot = SwarmSnapshot {
                swarm_id: swarm_id.clone(),
                description: request.description.clone(),
                role: request.role,
                mode: request.mode,
                state: AgentLifecycleState::Queued,
                max_concurrency,
                aggregate: initial_aggregate,
                children: initial_children,
            };

            // Background mode: register in background task manager, emit start,
            // and return immediately.
            ctx.multi_agent.register_swarm(initial_snapshot.clone());
            if request.mode == AgentRunMode::Background {
                deps = deps.with_cancel_token(CancellationToken::new());
                ctx.emit_event(AgentEvent::DelegateSwarmStarted {
                    turn,
                    swarm: initial_snapshot.clone(),
                });
                let task_id = ctx
                    .background_tasks
                    .start_delegate_swarm(initial_snapshot.clone())
                    .await;
                let runtime = ctx.multi_agent.clone();
                let background_tasks = ctx.background_tasks.clone();
                let task_id_for_worker = task_id.clone();
                let event_callback = ctx.tool_event.clone();
                let initial_snapshot_for_worker = initial_snapshot.clone();
                tokio::spawn(async move {
                    let final_snapshot = run_swarm_children(
                        runtime.clone(),
                        deps,
                        initial_snapshot_for_worker,
                        max_concurrency,
                        turn,
                        event_callback,
                        Some((background_tasks.clone(), task_id_for_worker.clone())),
                    )
                    .await;
                    runtime.register_swarm(final_snapshot.clone());
                    background_tasks
                        .finish_delegate_swarm(&task_id_for_worker, final_snapshot)
                        .await;
                });
                return Ok(ToolResult::ok(format!(
                    "swarm_id: {swarm_id}\nkind: delegate-swarm\nstatus: running\nitems: {total_children}\nnext_step: Call WaitDelegate with this swarm_id to wait for completion."
                ))
                .with_details(json!({
                    "kind": "delegate_swarm",
                    "mode": "background",
                    "swarm": model_safe_swarm_snapshot(&initial_snapshot),
                    "task_id": task_id,
                })));
            }

            ctx.emit_event(AgentEvent::DelegateSwarmStarted {
                turn,
                swarm: initial_snapshot.clone(),
            });

            let final_snapshot = run_swarm_children(
                ctx.multi_agent.clone(),
                deps,
                initial_snapshot,
                max_concurrency,
                turn,
                ctx.tool_event.clone(),
                None,
            )
            .await;
            ctx.multi_agent.register_swarm(final_snapshot.clone());
            ctx.emit_event(AgentEvent::DelegateSwarmFinished {
                turn,
                swarm: final_snapshot.clone(),
            });
            Ok(ToolResult::ok(format!(
                "swarm_id: {}\nstatus: {}\nsummary_scope: swarm_items\naggregate: total={} queued={} running={} completed={} failed={} cancelled={} timed_out={}",
                final_snapshot.swarm_id,
                final_snapshot.state.as_str(),
                final_snapshot.aggregate.total,
                final_snapshot.aggregate.queued,
                final_snapshot.aggregate.running,
                final_snapshot.aggregate.completed,
                final_snapshot.aggregate.failed,
                final_snapshot.aggregate.cancelled,
                final_snapshot.aggregate.timed_out,
            ))
            .with_details(swarm_details(&final_snapshot)))
        })
    }
}

fn parse_delegate_swarm_input(
    tool: &str,
    input: serde_json::Value,
) -> Result<DelegateSwarmRequest, ToolError> {
    if let Some(items) = input.get("items").and_then(serde_json::Value::as_array) {
        for (index, item) in items.iter().enumerate() {
            if !item.is_object() {
                return Err(ToolError::InvalidInput {
                    tool: tool.to_owned(),
                    message: format!(
                        "items[{index}] must be an object with required string fields title and value, for example {{\"title\":\"addition\",\"value\":\"2 + 2\"}}"
                    ),
                });
            }
        }
    }
    parse_input(tool, input)
}

#[allow(clippy::too_many_lines)]
async fn run_swarm_children(
    runtime: crate::multi_agent::MultiAgentRuntime,
    deps: ChildRuntimeDeps,
    initial_snapshot: SwarmSnapshot,
    max_concurrency: usize,
    turn: u32,
    event_callback: Option<ToolEventCallback>,
    background: Option<(crate::BackgroundTaskManager, String)>,
) -> SwarmSnapshot {
    const PROGRESS_QUEUE_CAPACITY: usize = 64;
    let mut ordered_children: Vec<Option<SwarmChildSnapshot>> =
        vec![None; initial_snapshot.children.len()];
    let current_children =
        std::sync::Arc::new(std::sync::Mutex::new(initial_snapshot.children.clone()));
    let (progress_tx, mut progress_rx) = mpsc::channel(PROGRESS_QUEUE_CAPACITY);
    let overflow = Arc::new(Mutex::new(BTreeMap::<usize, SwarmProgressUpdate>::new()));
    let mut stream = stream::iter(initial_snapshot.children.clone())
        .map(|child| {
            let runtime = runtime.clone();
            let deps = deps.clone();
            let initial_snapshot = initial_snapshot.clone();
            let current_children = std::sync::Arc::clone(&current_children);
            let progress_tx = progress_tx.clone();
            let overflow = Arc::clone(&overflow);
            async move {
                let item_index = child.item_index;
                let item = child.item.clone();
                // Skip queued children that were cancelled by cancel_swarm
                // before they were polled.
                if let Some(current) = runtime.agent_snapshot(child.agent.id.as_str())
                    && current.state.is_terminal()
                {
                    return SwarmChildSnapshot {
                        item_index,
                        item,
                        agent: current,
                    };
                }
                let output = runtime
                    .run_started_swarm_child_turn(
                        deps,
                        child.agent,
                        &initial_snapshot.swarm_id,
                        &item,
                        DelegateContext::None,
                        |progress| {
                            let (aggregate, state) = {
                                let mut children = current_children
                                    .lock()
                                    .expect("swarm progress state poisoned");
                                if let Some(child) = children.get_mut(item_index) {
                                    let _ = apply_agent_progress(&mut child.agent, &progress);
                                }
                                let aggregate = SwarmAggregate::from_states(
                                    children.iter().map(|c| c.agent.state),
                                );
                                (aggregate, aggregate.status())
                            };
                            let update = (
                                SwarmChildProgress {
                                    item_index,
                                    progress,
                                },
                                aggregate,
                                state,
                            );
                            match progress_tx.try_send(update) {
                                Ok(()) => {
                                    overflow
                                        .lock()
                                        .expect("swarm progress overflow poisoned")
                                        .remove(&item_index);
                                }
                                Err(mpsc::error::TrySendError::Full(update)) => {
                                    overflow
                                        .lock()
                                        .expect("swarm progress overflow poisoned")
                                        .insert(item_index, update);
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {}
                            }
                        },
                    )
                    .await;
                SwarmChildSnapshot {
                    item_index,
                    item,
                    agent: output.snapshot,
                }
            }
        })
        .buffer_unordered(max_concurrency);

    let mut completed_count = 0;
    loop {
        tokio::select! {
            Some((child_progress, aggregate, state)) = progress_rx.recv() => {
                publish_swarm_progress(
                    event_callback.as_ref(),
                    background.as_ref(),
                    turn,
                    &initial_snapshot.swarm_id,
                    (child_progress, aggregate, state),
                ).await;
                while let Ok(update) = progress_rx.try_recv() {
                    publish_swarm_progress(
                        event_callback.as_ref(),
                        background.as_ref(),
                        turn,
                        &initial_snapshot.swarm_id,
                        update,
                    ).await;
                }
                let overflow_updates = std::mem::take(
                    &mut *overflow.lock().expect("swarm progress overflow poisoned"),
                );
                for (_, update) in overflow_updates {
                    publish_swarm_progress(
                        event_callback.as_ref(),
                        background.as_ref(),
                        turn,
                        &initial_snapshot.swarm_id,
                        update,
                    ).await;
                }
            }
            Some(completed_child) = stream.next() => {
        while let Ok(update) = progress_rx.try_recv() {
            publish_swarm_progress(
                event_callback.as_ref(),
                background.as_ref(),
                turn,
                &initial_snapshot.swarm_id,
                update,
            ).await;
        }
        let overflow_updates = std::mem::take(
            &mut *overflow.lock().expect("swarm progress overflow poisoned"),
        );
        for (_, update) in overflow_updates {
            publish_swarm_progress(
                event_callback.as_ref(),
                background.as_ref(),
                turn,
                &initial_snapshot.swarm_id,
                update,
            ).await;
        }
        let index = completed_child.item_index;
        {
            let mut children = current_children
                .lock()
                .expect("swarm progress state poisoned");
            if let Some(child) = children.get_mut(index) {
                *child = completed_child.clone();
            }
        }
        ordered_children[index] = Some(completed_child);
        let snapshot = {
            let children = current_children
                .lock()
                .expect("swarm progress state poisoned")
                .clone();
            let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
            SwarmSnapshot {
                swarm_id: initial_snapshot.swarm_id.clone(),
                description: initial_snapshot.description.clone(),
                role: initial_snapshot.role,
                mode: initial_snapshot.mode,
                state: aggregate.status(),
                max_concurrency: initial_snapshot.max_concurrency,
                aggregate,
                children,
            }
        };
        if let Some(callback) = &event_callback {
            callback(AgentEvent::DelegateSwarmUpdated {
                turn,
                swarm: snapshot.clone(),
            });
        }
        if let Some((manager, task_id)) = &background {
            manager
                .update_delegate_swarm(task_id, snapshot.clone())
                .await;
        }
        completed_count += 1;
        if completed_count == initial_snapshot.children.len() {
            break;
        }
            }
            else => break,
        }
    }

    // Prefer the runtime's terminal swarm snapshot if the swarm was
    // cancelled via InterruptDelegate. This prevents the worker's
    // locally-tracked progress from regressing a cancelled swarm.
    if let Some(current) = runtime.swarm_snapshot(&initial_snapshot.swarm_id)
        && current.state == crate::multi_agent::AgentLifecycleState::Cancelled
    {
        return current;
    }
    swarm_snapshot_from_progress(&initial_snapshot, &ordered_children, initial_snapshot.mode)
}

fn swarm_snapshot_from_progress(
    initial_snapshot: &SwarmSnapshot,
    completed: &[Option<SwarmChildSnapshot>],
    mode: AgentRunMode,
) -> SwarmSnapshot {
    let children: Vec<SwarmChildSnapshot> = initial_snapshot
        .children
        .iter()
        .enumerate()
        .map(|(index, child)| {
            completed
                .get(index)
                .and_then(Clone::clone)
                .unwrap_or_else(|| child.clone())
        })
        .collect();
    let aggregate = SwarmAggregate::from_states(children.iter().map(|c| c.agent.state));
    SwarmSnapshot {
        swarm_id: initial_snapshot.swarm_id.clone(),
        description: initial_snapshot.description.clone(),
        role: initial_snapshot.role,
        mode,
        state: aggregate.status(),
        max_concurrency: initial_snapshot.max_concurrency,
        aggregate,
        children,
    }
}

fn child_runtime_deps(ctx: &ToolContext) -> Result<ChildRuntimeDeps, ToolError> {
    let config = ctx
        .child_config
        .clone()
        .ok_or_else(|| ToolError::InvalidInput {
            tool: "Delegate".to_owned(),
            message: "Delegate requires runtime config in ToolContext".to_owned(),
        })?;
    let model = ctx
        .child_model
        .clone()
        .ok_or_else(|| ToolError::InvalidInput {
            tool: "Delegate".to_owned(),
            message: "Delegate requires model client in ToolContext".to_owned(),
        })?;
    let tools = ctx
        .child_tools
        .clone()
        .ok_or_else(|| ToolError::InvalidInput {
            tool: "Delegate".to_owned(),
            message: "Delegate requires tool registry in ToolContext".to_owned(),
        })?;
    let mut deps =
        ChildRuntimeDeps::new(config, model, tools).with_cancel_token(ctx.cancel_token.clone());
    if let Some(state) = &ctx.parent_instruction_state {
        deps = deps.with_parent_instruction_state(state.clone());
    }
    Ok(deps)
}

fn validate_delegate_request(tool: &str, request: &DelegateRequest) -> Result<(), ToolError> {
    if request.task.trim().is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "task must not be empty".to_owned(),
        });
    }
    if let Some(resume) = request.resume.as_deref() {
        if !resume.starts_with("agent_") {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message:
                    "resume must be an agent_id returned by Delegate, not a swarm_id or task id"
                        .to_owned(),
            });
        }
        if request.role.is_some() {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: "role must be omitted when resume is set; resumed agents keep their original role/profile".to_owned(),
            });
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_swarm_request(tool: &str, request: &DelegateSwarmRequest) -> Result<(), ToolError> {
    const MAX_SWARM_CHILDREN: usize = 8;
    const MAX_SWARM_DESCRIPTION_CHARS: usize = 256;
    const MAX_SWARM_ITEM_TITLE_CHARS: usize = 80;
    const MAX_SWARM_ITEM_VALUE_CHARS: usize = 512;
    if request.description.trim().is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "description must not be empty".to_owned(),
        });
    }
    if request.items.is_empty() && request.resume_agent_ids.is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "items or resume_agent_ids must contain at least one child".to_owned(),
        });
    }
    if request.items.len() + request.resume_agent_ids.len() > MAX_SWARM_CHILDREN {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!("swarm supports at most {MAX_SWARM_CHILDREN} children"),
        });
    }
    if request.description.chars().count() > MAX_SWARM_DESCRIPTION_CHARS {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!(
                "description must not exceed {MAX_SWARM_DESCRIPTION_CHARS} characters"
            ),
        });
    }
    if !request.items.is_empty()
        && request
            .prompt_template
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "prompt_template is required when items are provided".to_owned(),
        });
    }
    if let Some(template) = request.prompt_template.as_deref() {
        if !request.items.is_empty() && !template.contains("{{item}}") {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: "prompt_template must include {{item}}; only {{item}} and optional {{description}} are supported".to_owned(),
            });
        }
        reject_unknown_placeholders(tool, template)?;
    }
    for (index, item) in request.items.iter().enumerate() {
        if item.title.trim().is_empty() {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!("items[{index}].title must not be empty"),
            });
        }
        if item.value.trim().is_empty() {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!("items[{index}].value must not be empty"),
            });
        }
        if item.title.chars().count() > MAX_SWARM_ITEM_TITLE_CHARS {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!(
                    "items[{index}].title must not exceed {MAX_SWARM_ITEM_TITLE_CHARS} characters"
                ),
            });
        }
        if item.value.chars().count() > MAX_SWARM_ITEM_VALUE_CHARS {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!(
                    "items[{index}].value must not exceed {MAX_SWARM_ITEM_VALUE_CHARS} characters"
                ),
            });
        }
    }
    for (agent_id, prompt) in &request.resume_agent_ids {
        if !agent_id.starts_with("agent_") {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: "resume_agent_ids keys must be agent_id values".to_owned(),
            });
        }
        if prompt.trim().is_empty() {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!("resume_agent_ids[{agent_id}] must not be empty"),
            });
        }
        if prompt.chars().count() > MAX_SWARM_ITEM_VALUE_CHARS {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!(
                    "resume_agent_ids[{agent_id}] must not exceed {MAX_SWARM_ITEM_VALUE_CHARS} characters"
                ),
            });
        }
    }
    if request.max_concurrency == Some(0) {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "max_concurrency must be greater than 0 when provided".to_owned(),
        });
    }
    let mut expanded = std::collections::HashSet::new();
    if let Some(template) = request.prompt_template.as_deref() {
        for item in &request.items {
            let prompt = apply_swarm_template(template, item.value.as_str(), &request.description);
            if !expanded.insert(prompt.clone()) {
                return Err(ToolError::InvalidInput {
                    tool: tool.to_owned(),
                    message: format!("duplicate expanded child prompt: {prompt}"),
                });
            }
        }
    }
    for prompt in request.resume_agent_ids.values() {
        if !expanded.insert(prompt.clone()) {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!("duplicate expanded child prompt: {prompt}"),
            });
        }
    }
    Ok(())
}

fn reject_unknown_placeholders(tool: &str, template: &str) -> Result<(), ToolError> {
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: "template placeholder is missing closing }}".to_owned(),
            });
        };
        let name = after_start[..end].trim();
        if name != "item" && name != "description" {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: "only {{item}} and {{description}} are supported in prompt_template"
                    .to_owned(),
            });
        }
        rest = &after_start[end + 2..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegate_swarm_schema_describes_resume_agent_ids_as_object_map() {
        let schema = DelegateSwarmTool.input_schema();
        let resume = &schema["properties"]["resume_agent_ids"];
        let description = resume["description"]
            .as_str()
            .expect("resume_agent_ids description");

        assert!(description.contains("JSON object"));
        assert!(description.contains("agent_id"));
        assert!(description.contains("per-agent resume prompt"));
        assert_eq!(resume["type"], "object");
        assert_eq!(resume["additionalProperties"]["type"], "string");
    }

    #[test]
    fn delegate_swarm_schema_describes_items_as_required_title_value_objects() {
        let schema = DelegateSwarmTool.input_schema();
        let items = &schema["properties"]["items"];
        let description = items["description"].as_str().expect("items description");

        assert!(description.contains("object array"));
        assert!(description.contains("required string fields"));
        assert!(description.contains("title"));
        assert!(description.contains("value"));
        assert_eq!(items["type"], "array");
    }

    #[test]
    fn delegate_swarm_request_rejects_string_items_with_title_value_guidance() {
        let err = parse_delegate_swarm_input(
            "DelegateSwarm",
            serde_json::json!({
                "description": "math checks",
                "items": ["2 + 2"],
                "prompt_template": "Calculate {{item}}"
            }),
        )
        .expect_err("string items rejected");

        assert_eq!(
            err.to_string(),
            "invalid input for DelegateSwarm: items[0] must be an object with required string fields title and value, for example {\"title\":\"addition\",\"value\":\"2 + 2\"}"
        );
    }

    #[test]
    fn delegate_resume_rejects_swarm_id_without_rewriting_target() {
        let request: DelegateRequest = serde_json::from_value(serde_json::json!({
            "task": "continue this work",
            "resume": "swarm_abc123"
        }))
        .expect("request parses");

        let err = validate_delegate_request("Delegate", &request).expect_err("swarm id rejected");

        assert_eq!(
            err.to_string(),
            "invalid input for Delegate: resume must be an agent_id returned by Delegate, not a swarm_id or task id"
        );
        assert!(!err.to_string().contains("agent_abc123"));
    }

    #[test]
    fn delegate_swarm_request_rejects_empty_item_title() {
        let request: DelegateSwarmRequest = serde_json::from_value(serde_json::json!({
            "description": "math checks",
            "items": [
                { "title": "   ", "value": "2 + 2" }
            ],
            "prompt_template": "Calculate {{item}}"
        }))
        .expect("request parses");

        let err =
            validate_swarm_request("DelegateSwarm", &request).expect_err("empty title rejected");
        assert_eq!(
            err.to_string(),
            "invalid input for DelegateSwarm: items[0].title must not be empty"
        );
    }

    #[test]
    fn delegate_swarm_accepts_long_prompt_template() {
        let request: DelegateSwarmRequest = serde_json::from_value(serde_json::json!({
            "description": "long instructions",
            "items": [
                { "title": "check", "value": "neo-ai" }
            ],
            "prompt_template": "x".repeat(513) + " {{item}}"
        }))
        .expect("request parses");

        validate_swarm_request("DelegateSwarm", &request).expect("long template accepted");
    }

    #[test]
    fn delegate_swarm_titled_items_drive_child_titles_and_prompts() {
        let request: DelegateSwarmRequest = serde_json::from_value(serde_json::json!({
            "description": "math checks",
            "items": [
                { "title": "addition", "value": "2 + 2" },
                { "title": "multiplication", "value": "3 * 3" }
            ],
            "prompt_template": "Calculate {{item}} for {{description}}"
        }))
        .expect("request parses");

        assert_eq!(request.items[0].title, "addition");
        assert_eq!(request.items[0].value, "2 + 2");
        assert_eq!(
            apply_swarm_template(
                request.prompt_template.as_deref().unwrap(),
                request.items[0].value.as_str(),
                request.description.as_str()
            ),
            "Calculate 2 + 2 for math checks"
        );
    }
}
