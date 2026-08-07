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
    AgentLifecycleState, AgentProfile, AgentRunMode, AgentTerminalReason, ChildPlan,
    ChildRuntimeDeps, DelegateContext, DelegateRequest, DelegateSwarmRequest, SwarmAggregate,
    SwarmChildProgress, SwarmChildSnapshot, SwarmResourceLimits, SwarmSnapshot,
    apply_agent_progress, child_plans_from_delegate_swarm, child_plans_serialized_bytes,
};
use crate::workflow::CompiledSchema;

type SwarmProgressUpdate = (SwarmChildProgress, SwarmAggregate, AgentLifecycleState);
struct SwarmRunOutput {
    snapshot: SwarmSnapshot,
    actual_usage: Option<crate::AgentTokenUsage>,
}

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
            workflow_origin: None,
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

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(execute_delegate(self.name(), ctx, input))
    }
}

async fn execute_delegate(
    tool: &str,
    ctx: &ToolContext,
    input: serde_json::Value,
) -> Result<ToolResult, ToolError> {
    let request: DelegateRequest = parse_input(tool, input)?;
    if let Err(err) = validate_delegate_request(tool, &request) {
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

    if let Some(origin) = deps.config.workflow_execution_origin.as_ref()
        && origin.invocation_id.is_some()
        && origin.swarm_item_id.is_none()
        && let Err(error) = ctx
            .workflow_runtime
            .bind_direct_delegate_agent(origin, &snapshot.id)
            .await
    {
        let message = error.to_string();
        let terminal = ctx
            .multi_agent
            .mark_background_terminal_reason(
                &snapshot.id,
                AgentLifecycleState::Failed,
                AgentTerminalReason::Error,
                Some(message.clone()),
            )
            .unwrap_or_else(|| snapshot.clone());
        let mut details = agent_details(
            "delegate",
            &terminal,
            Some(request.context),
            SummaryScope::CurrentRun,
            true,
            true,
            false,
        );
        details["binding_error"] = json!(message.clone());
        return Ok(ToolResult::error(message).with_details(details));
    }

    // Background mode: register the agent in the background task manager
    // and return immediately.
    if request.mode == AgentRunMode::Background {
        return Ok(start_background_delegate(ctx, deps, &request, &snapshot, turn).await);
    }

    // Foreground mode: run synchronously and return the result.
    ctx.emit_event(AgentEvent::DelegateStarted {
        turn,
        agent: snapshot.clone(),
        workflow_origin: None,
    });
    let output = ctx
        .multi_agent
        .run_started_child_turn_with_schema(
            deps.clone(),
            snapshot,
            request.context,
            request.output_schema.as_ref(),
            |progress| {
                ctx.emit_event(AgentEvent::DelegateProgressUpdated {
                    turn,
                    progress,
                    workflow_origin: None,
                });
            },
        )
        .await;
    let (result, details_extra) = apply_child_output_schema(&deps, &request, output).await?;
    let completed = result.snapshot;
    ctx.emit_event(AgentEvent::DelegateFinished {
        turn,
        agent: completed.clone(),
        workflow_origin: None,
    });
    let mut details = agent_details(
        "delegate",
        &completed,
        Some(request.context),
        SummaryScope::CurrentRun,
        true,
        true,
        false,
    );
    if let Some(obj) = details_extra.as_object() {
        for (k, v) in obj {
            details[k] = v.clone();
        }
    }
    let content = details.get("structured_output").map_or_else(
        || delegate_result_content(&completed, request.context),
        ToString::to_string,
    );
    Ok(ToolResult::ok(content).with_details(details))
}

async fn start_background_delegate(
    ctx: &ToolContext,
    deps: ChildRuntimeDeps,
    request: &DelegateRequest,
    snapshot: &crate::multi_agent::AgentSnapshot,
    turn: u32,
) -> ToolResult {
    let deps = deps.with_cancel_token(CancellationToken::new());
    ctx.emit_event(AgentEvent::DelegateStarted {
        turn,
        agent: snapshot.clone(),
        workflow_origin: None,
    });
    let task_id = ctx.background_tasks.start_delegate(snapshot.clone()).await;
    let runtime = ctx.multi_agent.clone();
    let background_tasks = ctx.background_tasks.clone();
    let request_for_worker = request.clone();
    let task_id_for_worker = task_id.clone();
    let snapshot_for_worker = snapshot.clone();
    let agent_id_for_worker = snapshot.id.as_str().to_owned();
    let event_callback = ctx.tool_event.clone();
    // MultiAgentRuntime owns panic terminalization: supervise the worker
    // JoinHandle, then mirror the resulting snapshot into BackgroundTaskManager.
    tokio::spawn(async move {
        let runtime_for_finish = runtime.clone();
        let callback = event_callback.clone();
        let runner = tokio::spawn(async move {
            runtime
                .run_started_child_turn_with_schema(
                    deps,
                    snapshot_for_worker,
                    request_for_worker.context,
                    request_for_worker.output_schema.as_ref(),
                    move |progress| {
                        if let Some(callback) = &callback {
                            callback(AgentEvent::DelegateProgressUpdated {
                                turn,
                                progress,
                                workflow_origin: None,
                            });
                        }
                    },
                )
                .await
        });
        let finished = match runner.await {
            Ok(output) => output.snapshot,
            Err(join_error) if join_error.is_panic() => runtime_for_finish
                .finish_delegate_worker_panicked(&agent_id_for_worker)
                .expect("panicked background delegate must exist"),
            Err(_) => runtime_for_finish
                .mark_background_terminal_reason(
                    &crate::multi_agent::AgentId::from_existing(&agent_id_for_worker),
                    AgentLifecycleState::Failed,
                    crate::multi_agent::AgentTerminalReason::Error,
                    Some("worker_task_cancelled".to_owned()),
                )
                .expect("cancelled background delegate must exist"),
        };
        if let Some(callback) = &event_callback {
            callback(AgentEvent::DelegateFinished {
                turn,
                agent: finished.clone(),
                workflow_origin: None,
            });
        }
        background_tasks
            .finish_delegate(&task_id_for_worker, finished)
            .await;
    });
    ToolResult::ok(format!(
        "agent_id: {}\nname: {}\nkind: delegate\nstatus: running\nrun_index: {}\ncontext_mode: {}\nnext_step: Call WaitDelegate with this agent_id to wait for completion.",
        snapshot.id.as_str(),
        snapshot.display_name.as_str(),
        snapshot.run_count,
        context_mode_label(request.context),
    ))
    .with_details({
        let mut details = agent_details(
            "delegate",
            snapshot,
            Some(request.context),
            SummaryScope::CurrentRun,
            true,
            false,
            false,
        );
        details["mode"] = json!("background");
        details["task_id"] = json!(task_id);
        details
    })
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

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(execute_delegate_swarm(self.name(), ctx, input))
    }
}

async fn execute_delegate_swarm(
    tool: &str,
    ctx: &ToolContext,
    input: serde_json::Value,
) -> Result<ToolResult, ToolError> {
    let request = parse_delegate_swarm_input(tool, input)?;
    validate_swarm_request(tool, &request)?;
    let mut deps = child_runtime_deps(ctx)?;
    deps.role = request.role;
    let turn = ctx.current_turn.unwrap_or_default();
    let swarm_id = ctx.multi_agent.new_swarm_id();
    let initial_snapshot =
        ctx.multi_agent
            .prepare_swarm(&swarm_id, &request)
            .map_err(|message| ToolError::InvalidInput {
                tool: "DelegateSwarm".to_owned(),
                message,
            })?;
    let total_children = initial_snapshot.children.len();
    let max_concurrency = initial_snapshot.max_concurrency;

    // Background mode: register in background task manager, emit start,
    // and return immediately.

    if request.mode == AgentRunMode::Background {
        deps = deps.with_cancel_token(CancellationToken::new());
        ctx.emit_event(AgentEvent::DelegateSwarmStarted {
            turn,
            swarm: initial_snapshot.clone(),
            workflow_origin: None,
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
        let swarm_id_for_worker = swarm_id.clone();
        tokio::spawn(async move {
            let runtime_for_finish = runtime.clone();
            let background_for_finish = background_tasks.clone();
            let task_id_for_finish = task_id_for_worker.clone();
            let runner = tokio::spawn({
                let runtime = runtime.clone();
                let background_tasks = background_tasks.clone();
                let task_id_for_worker = task_id_for_worker.clone();
                async move {
                    run_swarm_children(
                        runtime,
                        deps,
                        initial_snapshot_for_worker,
                        max_concurrency,
                        turn,
                        event_callback,
                        Some((background_tasks, task_id_for_worker)),
                    )
                    .await
                }
            });
            let final_snapshot = match runner.await {
                Ok(output) => {
                    let final_snapshot = output.snapshot;
                    runtime_for_finish.register_swarm(final_snapshot.clone());
                    final_snapshot
                }
                Err(join_error) if join_error.is_panic() => runtime_for_finish
                    .finish_delegate_swarm_worker_panicked(&swarm_id_for_worker)
                    .expect("panicked background swarm must exist"),
                Err(_) => runtime_for_finish
                    .finish_delegate_swarm_worker_failed(
                        &swarm_id_for_worker,
                        "worker_task_cancelled",
                    )
                    .expect("cancelled background swarm must exist"),
            };
            background_for_finish
                .finish_delegate_swarm(&task_id_for_finish, final_snapshot)
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
        workflow_origin: None,
    });

    let output = run_swarm_children(
        ctx.multi_agent.clone(),
        deps,
        initial_snapshot,
        max_concurrency,
        turn,
        ctx.tool_event.clone(),
        None,
    )
    .await;
    let final_snapshot = output.snapshot;
    ctx.multi_agent.register_swarm(final_snapshot.clone());
    ctx.emit_event(AgentEvent::DelegateSwarmFinished {
        turn,
        swarm: final_snapshot.clone(),
        workflow_origin: None,
    });
    Ok(swarm_run_result(final_snapshot, output.actual_usage))
}

fn swarm_run_result(
    final_snapshot: SwarmSnapshot,
    actual_usage: Option<crate::AgentTokenUsage>,
) -> ToolResult {
    let mut details = swarm_details(&final_snapshot);
    if let Some(usage) = actual_usage {
        details["actual_usage"] = json!(usage);
    }
    ToolResult::ok(format!(
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
            .with_details(details)
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

async fn run_swarm_children(
    runtime: crate::multi_agent::MultiAgentRuntime,
    deps: ChildRuntimeDeps,
    initial_snapshot: SwarmSnapshot,
    max_concurrency: usize,
    turn: u32,
    event_callback: Option<ToolEventCallback>,
    background: Option<(crate::BackgroundTaskManager, String)>,
) -> SwarmRunOutput {
    const PROGRESS_QUEUE_CAPACITY: usize = 64;
    let mut ordered_children: Vec<Option<SwarmChildSnapshot>> =
        vec![None; initial_snapshot.children.len()];
    let current_children =
        std::sync::Arc::new(std::sync::Mutex::new(initial_snapshot.children.clone()));
    let (progress_tx, mut progress_rx) = mpsc::channel(PROGRESS_QUEUE_CAPACITY);
    let overflow = Arc::new(Mutex::new(BTreeMap::<usize, SwarmProgressUpdate>::new()));
    let mut stream = swarm_child_runs(
        runtime.clone(),
        deps,
        &initial_snapshot,
        Arc::clone(&current_children),
        progress_tx.clone(),
        Arc::clone(&overflow),
        max_concurrency,
    );

    let mut completed_count = 0;
    let mut actual_usage: Option<crate::AgentTokenUsage> = None;
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
                drain_swarm_progress(
                    &mut progress_rx, &overflow, event_callback.as_ref(), background.as_ref(),
                    turn, &initial_snapshot.swarm_id,
                ).await;
            }
            Some((completed_child, child_usage)) = stream.next() => {
        if let Some(child_usage) = child_usage {
            actual_usage = Some(actual_usage.map_or(child_usage, |total| {
                total.saturating_add(child_usage)
            }));
        }
        drain_swarm_progress(
            &mut progress_rx, &overflow, event_callback.as_ref(), background.as_ref(),
            turn, &initial_snapshot.swarm_id,
        ).await;
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
                workflow_origin: None,
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

    final_swarm_output(&runtime, &initial_snapshot, &ordered_children, actual_usage)
}

fn final_swarm_output(
    runtime: &crate::multi_agent::MultiAgentRuntime,
    initial_snapshot: &SwarmSnapshot,
    ordered_children: &[Option<SwarmChildSnapshot>],
    actual_usage: Option<crate::AgentTokenUsage>,
) -> SwarmRunOutput {
    if let Some(current) = runtime.swarm_snapshot(&initial_snapshot.swarm_id)
        && current.state == crate::multi_agent::AgentLifecycleState::Cancelled
    {
        return SwarmRunOutput {
            snapshot: current,
            actual_usage,
        };
    }
    SwarmRunOutput {
        snapshot: swarm_snapshot_from_progress(
            initial_snapshot,
            ordered_children,
            initial_snapshot.mode,
        ),
        actual_usage,
    }
}

fn swarm_child_runs(
    runtime: crate::multi_agent::MultiAgentRuntime,
    deps: ChildRuntimeDeps,
    initial_snapshot: &SwarmSnapshot,
    current_children: Arc<std::sync::Mutex<Vec<SwarmChildSnapshot>>>,
    progress_tx: mpsc::Sender<SwarmProgressUpdate>,
    overflow: Arc<Mutex<BTreeMap<usize, SwarmProgressUpdate>>>,
    max_concurrency: usize,
) -> impl futures::Stream<Item = (SwarmChildSnapshot, Option<crate::AgentTokenUsage>)> {
    let initial_snapshot = initial_snapshot.clone();
    stream::iter(initial_snapshot.children.clone())
        .map(move |child| {
            let runtime = runtime.clone();
            let deps = deps.clone();
            let initial_snapshot = initial_snapshot.clone();
            let current_children = Arc::clone(&current_children);
            let progress_tx = progress_tx.clone();
            let overflow = Arc::clone(&overflow);
            async move {
                let item_index = child.item_index;
                let item = child.item.clone();
                if let Some(current) = runtime.agent_snapshot(child.agent.id.as_str())
                    && current.state.is_terminal()
                {
                    return (
                        SwarmChildSnapshot {
                            item_index,
                            item,
                            agent: current,
                        },
                        None,
                    );
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
                                    children.iter().map(|child| child.agent.state),
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
                let actual_usage = output.actual_usage;
                (
                    SwarmChildSnapshot {
                        item_index,
                        item,
                        agent: output.snapshot,
                    },
                    actual_usage,
                )
            }
        })
        .buffer_unordered(max_concurrency)
}

async fn drain_swarm_progress(
    progress_rx: &mut mpsc::Receiver<SwarmProgressUpdate>,
    overflow: &Mutex<BTreeMap<usize, SwarmProgressUpdate>>,
    event_callback: Option<&ToolEventCallback>,
    background: Option<&(crate::BackgroundTaskManager, String)>,
    turn: u32,
    swarm_id: &str,
) {
    while let Ok(update) = progress_rx.try_recv() {
        publish_swarm_progress(event_callback, background, turn, swarm_id, update).await;
    }
    let overflow_updates =
        std::mem::take(&mut *overflow.lock().expect("swarm progress overflow poisoned"));
    for update in overflow_updates.into_values() {
        publish_swarm_progress(event_callback, background, turn, swarm_id, update).await;
    }
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

/// When `output_schema` is set, attempt one best-effort structured projection of
/// the completed child's text. A mismatch only makes the projection unavailable:
/// the child lifecycle, original text, and observed usage are preserved, and no
/// repair model turn is ever started.
async fn apply_child_output_schema(
    _deps: &ChildRuntimeDeps,
    request: &DelegateRequest,
    output: crate::multi_agent::ChildRunOutput,
) -> Result<(crate::multi_agent::ChildRunOutput, serde_json::Value), ToolError> {
    let Some(schema_doc) = request.output_schema.as_ref() else {
        // The schema is optional: without it the ordinary child result is
        // returned unchanged with its observed usage.
        let usage = output.actual_usage;
        let mut extra = json!({});
        if let Some(usage) = usage {
            extra["actual_usage"] = json!(usage);
        }
        return Ok((output, extra));
    };
    // A child turn that failed at provider, authentication, rate-limit,
    // cancellation, or runtime level is not structured-output text: preserve
    // the original output and observed usage; never enter schema parsing.
    let child_succeeded = output.snapshot.state == AgentLifecycleState::Completed
        && !output
            .snapshot
            .outcome
            .as_ref()
            .is_some_and(|outcome| outcome.is_error);
    if !child_succeeded {
        let usage = output.actual_usage;
        let mut extra = json!({});
        if let Some(usage) = usage {
            extra["actual_usage"] = json!(usage);
        }
        return Ok((output, extra));
    }
    let schema = CompiledSchema::compile(schema_doc).map_err(|err| ToolError::InvalidInput {
        tool: "Delegate".to_owned(),
        message: format!("output_schema compile failed: {err}"),
    })?;

    // One deterministic projection attempt; a mismatch is data, not failure.
    let projection = crate::workflow::project_child_structured_output(&schema, &output);
    let mut extra = json!({
        "first_raw": projection.first_raw,
    });
    if let Some(usage) = projection.actual_usage {
        extra["actual_usage"] = json!(usage);
    }
    if let Some(value) = projection.value {
        extra["structured_output"] = value;
    } else {
        extra["projection_error"] = json!(projection.summary);
    }
    Ok((output, extra))
}

pub(crate) fn validate_delegate_request(
    tool: &str,
    request: &DelegateRequest,
) -> Result<(), ToolError> {
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

pub(crate) fn validate_swarm_request(
    tool: &str,
    request: &DelegateSwarmRequest,
) -> Result<(), ToolError> {
    validate_swarm_request_with_limits(tool, request, SwarmResourceLimits::default())
}

pub(crate) fn validate_swarm_request_with_limits(
    tool: &str,
    request: &DelegateSwarmRequest,
    limits: SwarmResourceLimits,
) -> Result<(), ToolError> {
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
    if request.description.chars().count() > limits.max_description_chars {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!(
                "description must not exceed {} characters",
                limits.max_description_chars
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
        if template.len() > limits.max_item_field_bytes {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!(
                    "prompt_template exceeds resource limit of {} bytes",
                    limits.max_item_field_bytes
                ),
            });
        }
    }
    validate_swarm_items(
        tool,
        &request.items,
        limits.max_title_chars,
        limits.max_item_field_bytes,
    )?;
    validate_resume_agents(tool, &request.resume_agent_ids, limits.max_item_field_bytes)?;
    if request.max_concurrency == Some(0) {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "max_concurrency must be greater than 0 when provided".to_owned(),
        });
    }
    let plans =
        child_plans_from_delegate_swarm(request).map_err(|message| ToolError::InvalidInput {
            tool: tool.to_owned(),
            message,
        })?;
    validate_child_plans(tool, &request.description, &plans, limits)?;
    Ok(())
}

/// Validate canonical child plans against byte/field resource limits.
///
/// There is intentionally no total child-count ceiling. Oversized serialized
/// batches and per-item fields fail; large but within-limit arrays succeed.
pub(crate) fn validate_child_plans(
    tool: &str,
    description: &str,
    plans: &[ChildPlan],
    limits: SwarmResourceLimits,
) -> Result<(), ToolError> {
    if plans.is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "items or resume_agent_ids must contain at least one child".to_owned(),
        });
    }
    if description.trim().is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "description must not be empty".to_owned(),
        });
    }
    if description.chars().count() > limits.max_description_chars {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!(
                "description must not exceed {} characters",
                limits.max_description_chars
            ),
        });
    }
    let mut expanded = std::collections::HashSet::new();
    for (index, plan) in plans.iter().enumerate() {
        if plan.task.trim().is_empty() {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!("children[{index}].task must not be empty"),
            });
        }
        if plan.task.len() > limits.max_item_field_bytes {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!(
                    "children[{index}].task exceeds resource limit of {} bytes",
                    limits.max_item_field_bytes
                ),
            });
        }
        if let Some(title) = plan.title.as_deref() {
            if title.trim().is_empty() {
                return Err(ToolError::InvalidInput {
                    tool: tool.to_owned(),
                    message: format!("children[{index}].title must not be empty when present"),
                });
            }
            if title.chars().count() > limits.max_title_chars {
                return Err(ToolError::InvalidInput {
                    tool: tool.to_owned(),
                    message: format!(
                        "children[{index}].title must not exceed {} characters",
                        limits.max_title_chars
                    ),
                });
            }
        }
        if let Some(schema) = &plan.output_schema {
            let schema_bytes =
                serde_json::to_vec(schema).map_err(|error| ToolError::InvalidInput {
                    tool: tool.to_owned(),
                    message: format!("children[{index}].output_schema serialize failed: {error}"),
                })?;
            if schema_bytes.len() > limits.max_item_schema_bytes {
                return Err(ToolError::InvalidInput {
                    tool: tool.to_owned(),
                    message: format!(
                        "children[{index}].output_schema exceeds resource limit of {} bytes",
                        limits.max_item_schema_bytes
                    ),
                });
            }
        }
        if let Some(resume) = plan.resume.as_deref()
            && !resume.starts_with("agent_")
        {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!("children[{index}].resume must be an agent_id value"),
            });
        }
        if !expanded.insert(plan.task.clone()) {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message: format!("duplicate expanded child prompt: {}", plan.task),
            });
        }
    }
    let total_bytes = child_plans_serialized_bytes(description, plans).map_err(|message| {
        ToolError::InvalidInput {
            tool: tool.to_owned(),
            message,
        }
    })?;
    if total_bytes > limits.max_request_bytes {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!(
                "swarm request exceeds resource limit of {} bytes (observed {total_bytes})",
                limits.max_request_bytes
            ),
        });
    }
    Ok(())
}

fn validate_swarm_items(
    tool: &str,
    items: &[crate::multi_agent::DelegateSwarmItem],
    max_title_chars: usize,
    max_field_bytes: usize,
) -> Result<(), ToolError> {
    for (index, item) in items.iter().enumerate() {
        let message = if item.title.trim().is_empty() {
            Some(format!("items[{index}].title must not be empty"))
        } else if item.value.trim().is_empty() {
            Some(format!("items[{index}].value must not be empty"))
        } else if item.title.chars().count() > max_title_chars {
            Some(format!(
                "items[{index}].title must not exceed {max_title_chars} characters"
            ))
        } else if item.value.len() > max_field_bytes {
            Some(format!(
                "items[{index}].value exceeds resource limit of {max_field_bytes} bytes"
            ))
        } else {
            None
        };
        if let Some(message) = message {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message,
            });
        }
    }
    Ok(())
}

fn validate_resume_agents(
    tool: &str,
    resume_agents: &std::collections::BTreeMap<String, String>,
    max_field_bytes: usize,
) -> Result<(), ToolError> {
    for (agent_id, prompt) in resume_agents {
        let message = if !agent_id.starts_with("agent_") {
            Some("resume_agent_ids keys must be agent_id values".to_owned())
        } else if prompt.trim().is_empty() {
            Some(format!("resume_agent_ids[{agent_id}] must not be empty"))
        } else if prompt.len() > max_field_bytes {
            Some(format!(
                "resume_agent_ids[{agent_id}] exceeds resource limit of {max_field_bytes} bytes"
            ))
        } else {
            None
        };
        if let Some(message) = message {
            return Err(ToolError::InvalidInput {
                tool: tool.to_owned(),
                message,
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
    use crate::multi_agent::apply_swarm_template;

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
    fn delegate_swarm_accepts_long_child_instructions() {
        let request: DelegateSwarmRequest = serde_json::from_value(serde_json::json!({
            "description": "long instructions",
            "items": [
                { "title": "check", "value": "i".repeat(513) }
            ],
            "prompt_template": "x".repeat(513) + " {{item}}",
            "resume_agent_ids": {
                "agent_existing": "r".repeat(513)
            }
        }))
        .expect("request parses");

        validate_swarm_request("DelegateSwarm", &request)
            .expect("long child instructions accepted");
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
