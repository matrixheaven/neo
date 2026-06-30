use futures::{StreamExt, stream};
use serde_json::json;

use super::{
    Tool, ToolContext, ToolError, ToolEventCallback, ToolFuture, ToolResult, parse_input, schema,
};
use crate::AgentEvent;
use crate::multi_agent::{
    AgentRunMode, ChildRuntimeDeps, DelegateContext, DelegateRequest, DelegateSwarmRequest,
    SwarmChildSnapshot, SwarmSnapshot, apply_swarm_template,
};

pub struct DelegateTool;

impl Tool for DelegateTool {
    fn name(&self) -> &'static str {
        "Delegate"
    }

    fn description(&self) -> &'static str {
        "Run one bounded task in a foreground subagent by default. Use background mode only when explicit parallel collaboration is needed."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<DelegateRequest>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let request: DelegateRequest = parse_input(self.name(), input)?;
            validate_delegate_request(self.name(), &request)?;
            let deps = child_runtime_deps(ctx)?;
            let turn = ctx.current_turn.unwrap_or_default();

            // Background mode: start the agent, register it in the background
            // task manager, and return immediately.
            if request.mode == AgentRunMode::Background {
                let running = ctx.multi_agent.start_delegate(
                    &request.task,
                    request.role,
                    AgentRunMode::Background,
                    crate::multi_agent::AgentPathKind::Root,
                );
                ctx.emit_event(AgentEvent::DelegateStarted {
                    turn,
                    agent: running.clone(),
                });
                let task_id = ctx.background_tasks.start_delegate(running.clone()).await;
                let runtime = ctx.multi_agent.clone();
                let background_tasks = ctx.background_tasks.clone();
                let request_for_worker = request.clone();
                let task_id_for_worker = task_id.clone();
                let running_for_worker = running.clone();
                let event_callback = ctx.tool_event.clone();
                tokio::spawn(async move {
                    let callback = event_callback.clone();
                    let output = runtime
                        .run_started_child_turn(
                            deps,
                            running_for_worker,
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
                        .complete_delegate(&task_id_for_worker, output.snapshot)
                        .await;
                });
                return Ok(ToolResult::ok(format!(
                    "agent_id: {}\nname: {}\nkind: delegate\nstatus: running\ntask: {}\nnext_step: Use WaitDelegate to wait for completion.\nnext_step: Use /tasks to inspect progress.",
                    running.id.as_str(),
                    running.display_name.as_str(),
                    request.task,
                ))
                .with_details(json!({
                    "kind": "delegate",
                    "mode": "background",
                    "agent": running,
                    "task_id": task_id,
                })));
            }

            // Foreground mode: run synchronously and return the result.
            let snapshot = ctx.multi_agent.start_delegate(
                &request.task,
                request.role,
                AgentRunMode::Foreground,
                crate::multi_agent::AgentPathKind::Root,
            );
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
            let summary = completed
                .outcome
                .as_ref()
                .map_or("", |outcome| outcome.summary.as_str());
            Ok(ToolResult::ok(format!(
                "agent_id: {}\nname: {}\nstatus: completed\nsummary: {}",
                completed.id.as_str(),
                completed.display_name.as_str(),
                summary
            ))
            .with_details(json!({
                "kind": "delegate",
                "mode": "foreground",
                "agent": completed,
            })))
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
         Required: prompt_template must contain {{item}}; Neo replaces {{item}} with each value in items. \
         Optional: {{description}} inserts the swarm description. Do not use {task}, {item}, %s, {}, \
         TASK_PLACEHOLDER, or any other placeholder."
    }

    fn input_schema(&self) -> serde_json::Value {
        schema::<DelegateSwarmRequest>()
    }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let request: DelegateSwarmRequest = parse_input(self.name(), input)?;
            validate_swarm_request(self.name(), &request)?;
            let deps = child_runtime_deps(ctx)?;
            let turn = ctx.current_turn.unwrap_or_default();
            let swarm_id = ctx.multi_agent.new_swarm_id();
            let max_concurrency = request
                .max_concurrency
                .unwrap_or(1)
                .clamp(1, request.items.len());
            // Emit Orchestrating phase — children haven't started yet.
            let mut initial_children = Vec::new();
            for (index, item) in request.items.iter().enumerate() {
                let task =
                    apply_swarm_template(&request.prompt_template, item, &request.description);
                let snapshot = ctx.multi_agent.queue_delegate(
                    &task,
                    request.role,
                    request.mode,
                    crate::multi_agent::AgentPathKind::SwarmChild(&swarm_id),
                );
                initial_children.push(SwarmChildSnapshot {
                    item_index: index,
                    item: item.clone(),
                    agent: snapshot,
                });
            }
            let initial_snapshot = SwarmSnapshot {
                swarm_id: swarm_id.clone(),
                description: request.description.clone(),
                mode: request.mode,
                max_concurrency,
                children: initial_children,
            };

            // Background mode: register in background task manager, emit start,
            // and return immediately.
            if request.mode == AgentRunMode::Background {
                ctx.multi_agent.register_swarm(initial_snapshot.clone());
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
                        runtime,
                        deps,
                        initial_snapshot_for_worker,
                        max_concurrency,
                        turn,
                        event_callback,
                        Some((background_tasks.clone(), task_id_for_worker.clone())),
                    )
                    .await;
                    background_tasks
                        .complete_delegate_swarm(&task_id_for_worker, final_snapshot)
                        .await;
                });
                return Ok(ToolResult::ok(format!(
                    "swarm_id: {}\nkind: delegate-swarm\nstatus: running\nitems: {}\nnext_step: Use WaitDelegate to wait for completion.\nnext_step: Use /tasks to inspect progress.",
                    swarm_id,
                    request.items.len(),
                ))
                .with_details(json!({
                    "kind": "delegate_swarm",
                    "mode": "background",
                    "swarm": initial_snapshot,
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
            ctx.emit_event(AgentEvent::DelegateSwarmFinished {
                turn,
                swarm: final_snapshot.clone(),
            });
            Ok(ToolResult::ok(format!(
                "swarm_id: {}\nstatus: completed\nitems: {}\nsummary: Foreground swarm completed.\n{}",
                final_snapshot.swarm_id,
                request.items.len(),
                format_swarm_children(&final_snapshot)
            ))
            .with_details(json!({
                "kind": "delegate_swarm",
                "mode": "foreground",
                "description": request.description,
                "items": request.items,
                "swarm": final_snapshot,
            })))
        })
    }
}

async fn run_swarm_children(
    runtime: crate::multi_agent::MultiAgentRuntime,
    deps: ChildRuntimeDeps,
    initial_snapshot: SwarmSnapshot,
    max_concurrency: usize,
    turn: u32,
    event_callback: Option<ToolEventCallback>,
    background: Option<(crate::BackgroundTaskManager, String)>,
) -> SwarmSnapshot {
    let mut ordered_children: Vec<Option<SwarmChildSnapshot>> =
        vec![None; initial_snapshot.children.len()];
    let current_children =
        std::sync::Arc::new(std::sync::Mutex::new(initial_snapshot.children.clone()));
    let mut stream = stream::iter(initial_snapshot.children.clone())
        .map(|child| {
            let runtime = runtime.clone();
            let deps = deps.clone();
            let event_callback = event_callback.clone();
            let background = background.clone();
            let initial_snapshot = initial_snapshot.clone();
            let current_children = std::sync::Arc::clone(&current_children);
            async move {
                let item_index = child.item_index;
                let item = child.item.clone();
                let output = runtime
                    .run_started_swarm_child_turn(
                        deps,
                        child.agent,
                        DelegateContext::None,
                        |agent| {
                            let snapshot = {
                                let mut children = current_children
                                    .lock()
                                    .expect("swarm progress state poisoned");
                                if let Some(child) = children.get_mut(item_index) {
                                    child.agent = agent;
                                }
                                SwarmSnapshot {
                                    swarm_id: initial_snapshot.swarm_id.clone(),
                                    description: initial_snapshot.description.clone(),
                                    mode: initial_snapshot.mode,
                                    max_concurrency: initial_snapshot.max_concurrency,
                                    children: children.clone(),
                                }
                            };
                            if let Some(callback) = &event_callback {
                                callback(AgentEvent::DelegateSwarmUpdated {
                                    turn,
                                    swarm: snapshot.clone(),
                                });
                            }
                            if let Some((manager, task_id)) = &background {
                                let manager = manager.clone();
                                let task_id = task_id.clone();
                                tokio::spawn(async move {
                                    manager.update_delegate_swarm(&task_id, snapshot).await;
                                });
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

    while let Some(completed_child) = stream.next().await {
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
            SwarmSnapshot {
                swarm_id: initial_snapshot.swarm_id.clone(),
                description: initial_snapshot.description.clone(),
                mode: initial_snapshot.mode,
                max_concurrency: initial_snapshot.max_concurrency,
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
    }

    swarm_snapshot_from_progress(&initial_snapshot, &ordered_children, initial_snapshot.mode)
}

fn swarm_snapshot_from_progress(
    initial_snapshot: &SwarmSnapshot,
    completed: &[Option<SwarmChildSnapshot>],
    mode: AgentRunMode,
) -> SwarmSnapshot {
    let children = initial_snapshot
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
    SwarmSnapshot {
        swarm_id: initial_snapshot.swarm_id.clone(),
        description: initial_snapshot.description.clone(),
        mode,
        max_concurrency: initial_snapshot.max_concurrency,
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
    Ok(ChildRuntimeDeps::new(config, model, tools))
}

fn validate_delegate_request(tool: &str, request: &DelegateRequest) -> Result<(), ToolError> {
    if request.task.trim().is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "task must not be empty".to_owned(),
        });
    }
    Ok(())
}

fn validate_swarm_request(tool: &str, request: &DelegateSwarmRequest) -> Result<(), ToolError> {
    if request.description.trim().is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "description must not be empty".to_owned(),
        });
    }
    if request.items.is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "items must contain at least one item".to_owned(),
        });
    }
    if let Some(index) = request.items.iter().position(|item| item.trim().is_empty()) {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: format!("items[{index}] must not be empty"),
        });
    }
    if request.prompt_template.trim().is_empty() {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "prompt_template must not be empty".to_owned(),
        });
    }
    if !request.prompt_template.contains("{{item}}") {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "prompt_template must include {{item}}; only {{item}} and optional {{description}} are supported".to_owned(),
        });
    }
    if request.max_concurrency == Some(0) {
        return Err(ToolError::InvalidInput {
            tool: tool.to_owned(),
            message: "max_concurrency must be greater than 0 when provided".to_owned(),
        });
    }
    Ok(())
}

fn format_swarm_children(snapshot: &SwarmSnapshot) -> String {
    snapshot
        .children
        .iter()
        .map(|child| {
            let status = format!("{:?}", child.agent.state).to_lowercase();
            let summary = child
                .agent
                .outcome
                .as_ref()
                .map_or("", |outcome| outcome.summary.as_str());
            format!(
                "- item_index: {} | item: {} | agent_id: {} | name: {} | status: {} | tools: {} | elapsed_ms: {} | summary: {}",
                child.item_index,
                child.item,
                child.agent.id.as_str(),
                child.agent.display_name.as_str(),
                status,
                child.agent.tool_count,
                child.agent.elapsed.as_millis(),
                summary,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
