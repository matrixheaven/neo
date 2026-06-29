use std::path::PathBuf;

use futures::{StreamExt, stream::FuturesUnordered};
use tokio_util::sync::CancellationToken;

use super::config::{AgentConfig, ToolExecutionMode};
use super::error::AgentRuntimeError;
use super::events::{
    EventEmitter, emit_shell_finished, emit_shell_started, emit_terminal_events,
    make_tool_update_callback,
};
use super::permission::{
    PermissionPreparation, current_permission_mode, permission_preparation_for_mode,
    prepare_tool_call,
};
use super::plan_orchestration::exit_plan_mode_has_reviewable_plan;
use super::skill_dispatch::execute_invoke_skill;
use crate::skills::SkillStore;
use crate::tools::execute_model_bash_for_runtime;
use crate::{
    AgentEvent, AgentToolCall, PermissionMode, ProcessSupervisor, ToolAccess, ToolContext,
    ToolError, ToolRegistry, ToolResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolSchedulingClass {
    ParallelSafe,
    Exclusive,
    BlockingDialog,
}

pub(super) struct PreparedToolCall {
    pub(super) result: PreparedToolCallResult,
    pub(super) access: ToolAccess,
}

pub(super) enum PreparedToolCallResult {
    Run,
    Skip(ToolResult),
}

pub(super) fn terminates_tool_batch(tool_results: &[(AgentToolCall, ToolResult)]) -> bool {
    !tool_results.is_empty() && tool_results.iter().all(|(_, result)| result.terminate)
}

pub(super) fn continues_after_terminating_batch(
    tool_results: &[(AgentToolCall, ToolResult)],
) -> bool {
    tool_results.iter().any(|(call, result)| {
        // Mode transitions terminate their batch (so the runtime can fire the
        // mode-switch side effects keyed off `result.terminate`), but the loop
        // generally keeps going so the model can act on the result: continue
        // planning after EnterPlanMode, execute the approved plan after
        // ExitPlanMode. Only the successful branch continues; a rejected/revised
        // ExitPlanMode returns a non-terminating synthesized result and never
        // reaches this predicate.
        //
        // ExitGoalMode is intentionally excluded: it starts the durable goal,
        // and goal continuation (`goal_continuation_messages`) drives subsequent
        // turns on the next `run_agent_turn` entry by design. Continuing inline
        // here would re-feed the continuation message every turn and spin.
        !result.is_error && matches!(call.name.as_str(), "EnterPlanMode" | "ExitPlanMode")
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_tool_calls(
    config: &AgentConfig,
    registry: &ToolRegistry,
    skills: Option<&SkillStore>,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    if matches!(config.tool_execution_mode, ToolExecutionMode::Sequential) {
        return execute_tool_calls_sequential(
            config,
            registry,
            skills,
            turn,
            tool_calls,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    if tool_calls.iter().any(|call| {
        let prep = permission_preparation_for_mode(config, call);
        scheduling_class_for_preparation(config, call, &prep) == ToolSchedulingClass::BlockingDialog
    }) {
        return execute_tool_calls_sequential(
            config,
            registry,
            skills,
            turn,
            tool_calls,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    if tool_calls.iter().any(|call| {
        let prep = permission_preparation_for_mode(config, call);
        scheduling_class_for_preparation(config, call, &prep) == ToolSchedulingClass::Exclusive
    }) {
        return execute_tool_calls_sequential(
            config,
            registry,
            skills,
            turn,
            tool_calls,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    execute_tool_calls_parallel(
        config,
        registry,
        skills,
        turn,
        tool_calls,
        emitter,
        cancel_token,
        process_supervisor,
    )
    .await
}

fn scheduling_class_for_preparation(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    preparation: &PermissionPreparation,
) -> ToolSchedulingClass {
    if matches!(preparation, PermissionPreparation::Ask { .. }) {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name == "AskUserQuestion" && !ask_user_runs_in_background(tool_call) {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name == "ExitPlanMode"
        && current_permission_mode(config) != PermissionMode::Auto
        && exit_plan_mode_has_reviewable_plan(config)
    {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name == "ExitGoalMode" && current_permission_mode(config) != PermissionMode::Auto {
        return ToolSchedulingClass::BlockingDialog;
    }
    if matches!(
        tool_call.name.as_str(),
        "Bash" | "Terminal" | "Write" | "Edit"
    ) {
        return ToolSchedulingClass::Exclusive;
    }
    ToolSchedulingClass::ParallelSafe
}

pub(super) fn ask_user_runs_in_background(tool_call: &AgentToolCall) -> bool {
    tool_call
        .arguments
        .get("background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls_sequential(
    config: &AgentConfig,
    registry: &ToolRegistry,
    skills: Option<&SkillStore>,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    let tool_context = default_tool_context(config, cancel_token, process_supervisor.clone())?;
    let mut results = Vec::new();
    for tool_call in tool_calls {
        emitter.emit(AgentEvent::ToolExecutionStarted {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: tool_call.arguments.clone(),
        });
        let mut result =
            if let Some(blocked) = before_tool_result(config, tool_call, cancel_token).await {
                blocked
            } else {
                prepare_and_run_tool(
                    config,
                    registry,
                    skills,
                    &tool_context,
                    turn,
                    tool_call,
                    emitter,
                    cancel_token,
                )
                .await?
            };
        if !cancel_token.is_cancelled() {
            result = after_tool_result(config, tool_call, result, cancel_token).await;
        }
        emit_shell_finished(turn, tool_call, &result, emitter);
        emit_terminal_events(turn, tool_call, &result, &tool_context, emitter);
        emitter.emit(AgentEvent::ToolExecutionFinished {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            result: result.clone(),
        });
        results.push((tool_call.clone(), result));
        if cancel_token.is_cancelled() {
            break;
        }
    }
    Ok(results)
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls_parallel(
    config: &AgentConfig,
    registry: &ToolRegistry,
    skills: Option<&SkillStore>,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    let tool_context = default_tool_context(config, cancel_token, process_supervisor.clone())?;
    let mut completed = Vec::with_capacity(tool_calls.len());
    let mut running = FuturesUnordered::new();

    for (index, tool_call) in tool_calls.iter().cloned().enumerate() {
        if cancel_token.is_cancelled() {
            break;
        }
        emitter.emit(AgentEvent::ToolExecutionStarted {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: tool_call.arguments.clone(),
        });
        if let Some(mut result) = before_tool_result(config, &tool_call, cancel_token).await {
            if !cancel_token.is_cancelled() {
                result = after_tool_result(config, &tool_call, result, cancel_token).await;
            }
            emit_shell_finished(turn, &tool_call, &result, emitter);
            emit_terminal_events(turn, &tool_call, &result, &tool_context, emitter);
            emitter.emit(AgentEvent::ToolExecutionFinished {
                turn,
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                result: result.clone(),
            });
            completed.push((index, tool_call, result));
            continue;
        }

        let prepared = prepare_tool_call(config, &tool_call, turn, emitter, cancel_token).await;
        if let PreparedToolCallResult::Skip(result) = prepared.result {
            if !cancel_token.is_cancelled() {
                let result = after_tool_result(config, &tool_call, result, cancel_token).await;
                emit_shell_finished(turn, &tool_call, &result, emitter);
                emit_terminal_events(turn, &tool_call, &result, &tool_context, emitter);
                emitter.emit(AgentEvent::ToolExecutionFinished {
                    turn,
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    result: result.clone(),
                });
                completed.push((index, tool_call, result));
            }
            continue;
        }

        let config = config.clone();
        let tool_context = tool_context.clone().with_access(prepared.access);
        let cancel_token = cancel_token.clone();
        let sink = emitter.sink();
        running.push(async move {
            let tool_context = tool_context.with_tool_update(make_tool_update_callback(
                sink.clone(),
                turn,
                tool_call.id.clone(),
                tool_call.name.clone(),
            ));
            let mut result =
                run_tool_with_cancel(skills, registry, &tool_call, &tool_context, &cancel_token)
                    .await;
            if !cancel_token.is_cancelled() {
                result = after_tool_result(&config, &tool_call, result, &cancel_token).await;
            }
            Ok::<_, AgentRuntimeError>((index, tool_call, result))
        });
    }

    while let Some(outcome) = running.next().await {
        let (index, tool_call, result) = outcome?;
        emit_shell_finished(turn, &tool_call, &result, emitter);
        emit_terminal_events(turn, &tool_call, &result, &tool_context, emitter);
        emitter.emit(AgentEvent::ToolExecutionFinished {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            result: result.clone(),
        });
        completed.push((index, tool_call, result));
    }

    completed.sort_by_key(|(index, _, _)| *index);
    Ok(completed
        .into_iter()
        .map(|(_, tool_call, result)| (tool_call, result))
        .collect())
}

async fn before_tool_result(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    cancel_token: &CancellationToken,
) -> Option<ToolResult> {
    if let Some(before_tool_call) = &config.before_tool_call
        && let Some(result) = before_tool_call(tool_call)
    {
        return Some(result);
    }
    let async_before_tool_call = config.async_before_tool_call.as_ref()?;
    tokio::select! {
        biased;
        result = async_before_tool_call(tool_call.clone(), cancel_token.clone()) => result,
        () = cancel_token.cancelled() => Some(cancelled_tool_result()),
    }
}

async fn after_tool_result(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    mut result: ToolResult,
    cancel_token: &CancellationToken,
) -> ToolResult {
    if let Some(after_tool_call) = &config.after_tool_call {
        result = after_tool_call(tool_call, result);
    }
    let Some(async_after_tool_call) = &config.async_after_tool_call else {
        return result;
    };
    tokio::select! {
        biased;
        result = async_after_tool_call(tool_call.clone(), result, cancel_token.clone()) => result,
        () = cancel_token.cancelled() => cancelled_tool_result(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn prepare_and_run_tool(
    config: &AgentConfig,
    registry: &ToolRegistry,
    skills: Option<&SkillStore>,
    tool_context: &ToolContext,
    turn: u32,
    tool_call: &AgentToolCall,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<ToolResult, AgentRuntimeError> {
    let prepared = prepare_tool_call(config, tool_call, turn, emitter, cancel_token).await;
    match prepared.result {
        PreparedToolCallResult::Skip(result) => Ok(result),
        PreparedToolCallResult::Run => {
            let context = tool_context
                .clone()
                .with_access(prepared.access)
                .with_tool_update(make_tool_update_callback(
                    emitter.sink(),
                    turn,
                    tool_call.id.clone(),
                    tool_call.name.clone(),
                ));
            if tool_call.name == "Bash" {
                emit_shell_started(turn, tool_call, &context, emitter);
            }
            let result =
                run_tool_with_cancel(skills, registry, tool_call, &context, cancel_token).await;
            if tool_call.name == "Skill" && !result.is_error {
                emitter.emit(AgentEvent::SkillActivated {
                    turn,
                    name: tool_call
                        .arguments
                        .get("skill")
                        .and_then(|value| value.as_str())
                        .unwrap_or("unknown")
                        .to_owned(),
                });
            }
            Ok(result)
        }
    }
}

async fn run_tool_with_cancel(
    skills: Option<&SkillStore>,
    registry: &ToolRegistry,
    tool_call: &AgentToolCall,
    tool_context: &ToolContext,
    cancel_token: &CancellationToken,
) -> ToolResult {
    if tool_call.name == "Skill" {
        return execute_invoke_skill(skills, tool_call);
    }
    if tool_call.name == "Bash" {
        return run_model_bash_with_cancel(tool_call, tool_context, cancel_token).await;
    }
    tokio::select! {
        biased;
        result = registry.run(&tool_call.name, tool_context, tool_call.arguments.clone()) => {
            result.unwrap_or_else(|err| ToolResult::error(err.to_string()))
        }
        () = cancel_token.cancelled() => cancelled_tool_result(),
    }
}

pub(super) fn cancelled_tool_result() -> ToolResult {
    ToolResult::error(ToolError::Cancelled.to_string())
}

async fn run_model_bash_with_cancel(
    tool_call: &AgentToolCall,
    tool_context: &ToolContext,
    cancel_token: &CancellationToken,
) -> ToolResult {
    tokio::select! {
        biased;
        result = execute_model_bash_for_runtime(tool_context, tool_call.arguments.clone()) => {
            result.unwrap_or_else(|err| ToolResult::error(err.to_string()))
        }
        () = cancel_token.cancelled() => cancelled_tool_result(),
    }
}

fn default_tool_context(
    config: &AgentConfig,
    cancel_token: &CancellationToken,
    process_supervisor: ProcessSupervisor,
) -> Result<ToolContext, AgentRuntimeError> {
    let workspace_root = if let Some(workspace_root) = &config.workspace_root {
        workspace_root.clone()
    } else {
        std::env::current_dir()?
    };
    ToolContext::new(workspace_root)
        .map(|context| {
            context
                .with_access(ToolAccess::none())
                .with_cancel_token(cancel_token.clone())
                .with_process_supervisor(process_supervisor)
                .with_background_tasks(config.background_tasks.clone())
        })
        .map(|context| {
            // The active plan file lives under the NEO_HOME sessions bucket
            // (outside the workspace). Whitelist it so Write/Edit can resolve
            // the path while plan mode is active; the plan-mode guard and the
            // permission layer still restrict writes to *only* that path.
            let plan_path = config
                .plan_mode
                .read()
                .ok()
                .and_then(|plan_mode| plan_mode.plan_file_path().map(PathBuf::from));
            match plan_path {
                Some(path) => context.with_allowed_external_write_paths([path]),
                None => context,
            }
        })
        .map_err(AgentRuntimeError::Tool)
}
