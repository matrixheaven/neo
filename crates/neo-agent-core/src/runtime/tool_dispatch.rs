use std::path::PathBuf;
use std::sync::Arc;

use futures::{StreamExt, stream::FuturesUnordered};
use neo_ai::ModelClient;
use tokio_util::sync::CancellationToken;

use super::config::{AgentConfig, ToolExecutionMode};
use super::error::AgentRuntimeError;
use super::events::{
    EventEmitter, emit_shell_finished, emit_shell_started, emit_terminal_events,
    make_tool_event_callback, make_tool_update_callback,
};
use super::permission::{
    PermissionPreparation, current_permission_mode, permission_preparation_for_mode,
    prepare_tool_call,
};
use super::plan_orchestration::exit_plan_mode_has_reviewable_plan;
use super::skill_dispatch::execute_invoke_skill;
use super::tool_arguments::prepare_tool_arguments;
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

/// Parse raw arguments for every tool call up front, returning a vec of
/// `(tool_call, parsed_arguments_or_error_result)`. Invalid arguments produce
/// a `ToolResult` error that short-circuits execution for that call.
fn prepare_tool_calls_for_execution<'a>(
    tool_calls: &'a [AgentToolCall],
    tool_specs: &[neo_ai::ToolSpec],
) -> Vec<(
    &'a AgentToolCall,
    Result<super::tool_arguments::PreparedToolCall, ToolResult>,
)> {
    tool_calls
        .iter()
        .map(|tool_call| {
            let prepared = prepare_tool_arguments(tool_call, tool_specs);
            if let Ok(ref parsed) = prepared {
                if let Some(warning) = &parsed.warning {
                    eprintln!(
                        "[warn] tool call '{}' arguments repaired: {}",
                        parsed.name, warning
                    );
                }
            }
            (tool_call, prepared)
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_tool_calls(
    config: &AgentConfig,
    model: Arc<dyn ModelClient>,
    registry: Arc<ToolRegistry>,
    skills: Option<&SkillStore>,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    let tool_specs = registry.specs();
    let prepared = prepare_tool_calls_for_execution(tool_calls, &tool_specs);

    if matches!(config.tool_execution_mode, ToolExecutionMode::Sequential) {
        return execute_tool_calls_sequential(
            config,
            Arc::clone(&model),
            Arc::clone(&registry),
            skills,
            turn,
            &prepared,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    if prepared.iter().any(|(tool_call, parsed)| {
        let prep = match parsed {
            Ok(prepared) => permission_preparation_for_mode(config, tool_call, &prepared.arguments),
            Err(_) => return false, // invalid args bypass scheduling — handled inline
        };
        scheduling_class_for_preparation(
            config,
            tool_call,
            &prep,
            parsed_prepared_arguments(parsed),
        ) == ToolSchedulingClass::BlockingDialog
    }) {
        return execute_tool_calls_sequential(
            config,
            Arc::clone(&model),
            Arc::clone(&registry),
            skills,
            turn,
            &prepared,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    if prepared.iter().any(|(tool_call, parsed)| {
        let prep = match parsed {
            Ok(prepared) => permission_preparation_for_mode(config, tool_call, &prepared.arguments),
            Err(_) => return false,
        };
        scheduling_class_for_preparation(
            config,
            tool_call,
            &prep,
            parsed_prepared_arguments(parsed),
        ) == ToolSchedulingClass::Exclusive
    }) {
        return execute_tool_calls_sequential(
            config,
            Arc::clone(&model),
            Arc::clone(&registry),
            skills,
            turn,
            &prepared,
            emitter,
            cancel_token,
            process_supervisor,
        )
        .await;
    }

    execute_tool_calls_parallel(
        config,
        model,
        registry,
        skills,
        turn,
        &prepared,
        emitter,
        cancel_token,
        process_supervisor,
    )
    .await
}

/// Helper to extract the parsed arguments reference for scheduling decisions.
fn parsed_prepared_arguments(
    parsed: &Result<super::tool_arguments::PreparedToolCall, ToolResult>,
) -> &serde_json::Value {
    match parsed {
        Ok(prepared) => &prepared.arguments,
        // Invalid args never reach scheduling (returned false above); this
        // fallback is never used but must be valid.
        Err(_) => &serde_json::Value::Null,
    }
}

fn scheduling_class_for_preparation(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    preparation: &PermissionPreparation,
    arguments: &serde_json::Value,
) -> ToolSchedulingClass {
    if matches!(preparation, PermissionPreparation::Ask { .. }) {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name == "AskUserQuestion" && !ask_user_runs_in_background(arguments) {
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

pub(super) fn ask_user_runs_in_background(arguments: &serde_json::Value) -> bool {
    arguments
        .get("background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls_sequential(
    config: &AgentConfig,
    model: Arc<dyn ModelClient>,
    registry: Arc<ToolRegistry>,
    skills: Option<&SkillStore>,
    turn: u32,
    prepared: &[(
        &AgentToolCall,
        Result<super::tool_arguments::PreparedToolCall, ToolResult>,
    )],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    let tool_context = default_tool_context(
        config,
        model,
        Arc::clone(&registry),
        turn,
        cancel_token,
        process_supervisor.clone(),
    )?;
    let mut results = Vec::new();
    for (tool_call, parsed) in prepared {
        // Invalid arguments: emit a finished error without starting execution.
        let prepared_call = match parsed {
            Ok(prepared) => prepared,
            Err(error_result) => {
                let result = error_result.clone();
                emitter.emit(AgentEvent::ToolExecutionFinished {
                    turn,
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    result: result.clone(),
                });
                results.push(((*tool_call).clone(), result));
                continue;
            }
        };
        emitter.emit(AgentEvent::ToolExecutionStarted {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: prepared_call.arguments.clone(),
        });
        let mut result =
            if let Some(blocked) = before_tool_result(config, tool_call, cancel_token).await {
                blocked
            } else {
                prepare_and_run_tool(
                    config,
                    registry.as_ref(),
                    skills,
                    &tool_context,
                    turn,
                    tool_call,
                    &prepared_call.arguments,
                    emitter,
                    cancel_token,
                )
                .await?
            };
        if !cancel_token.is_cancelled() {
            result = after_tool_result(config, tool_call, result, cancel_token).await;
        }
        emit_shell_finished(turn, tool_call, &result, emitter);
        emit_terminal_events(
            turn,
            &prepared_call.arguments,
            tool_call,
            &result,
            &tool_context,
            emitter,
        );
        emitter.emit(AgentEvent::ToolExecutionFinished {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            result: result.clone(),
        });
        results.push(((*tool_call).clone(), result));
        if cancel_token.is_cancelled() {
            break;
        }
    }
    Ok(results)
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls_parallel(
    config: &AgentConfig,
    model: Arc<dyn ModelClient>,
    registry: Arc<ToolRegistry>,
    skills: Option<&SkillStore>,
    turn: u32,
    prepared: &[(
        &AgentToolCall,
        Result<super::tool_arguments::PreparedToolCall, ToolResult>,
    )],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError> {
    let tool_context = default_tool_context(
        config,
        model,
        Arc::clone(&registry),
        turn,
        cancel_token,
        process_supervisor.clone(),
    )?;
    let mut completed = Vec::with_capacity(prepared.len());
    let mut running = FuturesUnordered::new();

    for (index, (tool_call, parsed)) in prepared.iter().cloned().enumerate() {
        if cancel_token.is_cancelled() {
            break;
        }

        // Invalid arguments: emit a finished error without starting execution.
        let prepared_call = match parsed {
            Ok(prepared) => prepared,
            Err(error_result) => {
                let result = error_result;
                emitter.emit(AgentEvent::ToolExecutionFinished {
                    turn,
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    result: result.clone(),
                });
                completed.push((index, (*tool_call).clone(), result));
                continue;
            }
        };

        emitter.emit(AgentEvent::ToolExecutionStarted {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: prepared_call.arguments.clone(),
        });
        if let Some(mut result) = before_tool_result(config, tool_call, cancel_token).await {
            if !cancel_token.is_cancelled() {
                result = after_tool_result(config, tool_call, result, cancel_token).await;
            }
            emit_shell_finished(turn, tool_call, &result, emitter);
            emit_terminal_events(
                turn,
                &prepared_call.arguments,
                tool_call,
                &result,
                &tool_context,
                emitter,
            );
            emitter.emit(AgentEvent::ToolExecutionFinished {
                turn,
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                result: result.clone(),
            });
            completed.push((index, (*tool_call).clone(), result));
            continue;
        }

        let permission_prep = prepare_tool_call(
            config,
            tool_call,
            &prepared_call.arguments,
            turn,
            emitter,
            cancel_token,
        )
        .await;
        if let PreparedToolCallResult::Skip(result) = permission_prep.result {
            if !cancel_token.is_cancelled() {
                let result = after_tool_result(config, tool_call, result, cancel_token).await;
                emit_shell_finished(turn, tool_call, &result, emitter);
                emit_terminal_events(
                    turn,
                    &prepared_call.arguments,
                    tool_call,
                    &result,
                    &tool_context,
                    emitter,
                );
                emitter.emit(AgentEvent::ToolExecutionFinished {
                    turn,
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    result: result.clone(),
                });
                completed.push((index, (*tool_call).clone(), result));
            }
            continue;
        }

        let config = config.clone();
        let registry = Arc::clone(&registry);
        let tool_context = tool_context.clone().with_access(permission_prep.access);
        let cancel_token = cancel_token.clone();
        let sink = emitter.sink();
        let arguments = prepared_call.arguments.clone();
        running.push(async move {
            let tool_context = tool_context
                .with_tool_update(make_tool_update_callback(
                    sink.clone(),
                    turn,
                    tool_call.id.clone(),
                    tool_call.name.clone(),
                ))
                .with_tool_event(make_tool_event_callback(sink));
            let mut result = run_tool_with_cancel(
                skills,
                registry.as_ref(),
                tool_call,
                &arguments,
                &tool_context,
                &cancel_token,
            )
            .await;
            if !cancel_token.is_cancelled() {
                result = after_tool_result(&config, tool_call, result, &cancel_token).await;
            }
            Ok::<_, AgentRuntimeError>((index, (*tool_call).clone(), result))
        });
    }

    while let Some(outcome) = running.next().await {
        let (index, tool_call, result) = outcome?;
        // Re-derive parsed arguments for shell/terminal event emission.
        let arguments = match &prepared[index].1 {
            Ok(p) => p.arguments.clone(),
            Err(_) => serde_json::Value::Null,
        };
        emit_shell_finished(turn, &tool_call, &result, emitter);
        emit_terminal_events(
            turn,
            &arguments,
            &tool_call,
            &result,
            &tool_context,
            emitter,
        );
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
    arguments: &serde_json::Value,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<ToolResult, AgentRuntimeError> {
    let prepared =
        prepare_tool_call(config, tool_call, arguments, turn, emitter, cancel_token).await;
    match prepared.result {
        PreparedToolCallResult::Skip(result) => Ok(result),
        PreparedToolCallResult::Run => {
            let sink = emitter.sink();
            let context = tool_context
                .clone()
                .with_access(prepared.access)
                .with_tool_update(make_tool_update_callback(
                    sink.clone(),
                    turn,
                    tool_call.id.clone(),
                    tool_call.name.clone(),
                ))
                .with_tool_event(make_tool_event_callback(sink));
            if tool_call.name == "Bash" {
                emit_shell_started(turn, arguments, tool_call, &context, emitter);
            }
            let result = run_tool_with_cancel(
                skills,
                registry,
                tool_call,
                arguments,
                &context,
                cancel_token,
            )
            .await;
            if tool_call.name == "Skill" && !result.is_error {
                emitter.emit(AgentEvent::SkillActivated {
                    turn,
                    name: arguments
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
    arguments: &serde_json::Value,
    tool_context: &ToolContext,
    cancel_token: &CancellationToken,
) -> ToolResult {
    if tool_call.name == "Skill" {
        return execute_invoke_skill(skills, arguments);
    }
    if tool_call.name == "Bash" {
        return run_model_bash_with_cancel(arguments, tool_context, cancel_token).await;
    }
    tokio::select! {
        biased;
        result = registry.run(&tool_call.name, tool_context, arguments.clone()) => {
            result.unwrap_or_else(|err| ToolResult::error(err.to_string()))
        }
        () = cancel_token.cancelled() => cancelled_tool_result(),
    }
}

pub(super) fn cancelled_tool_result() -> ToolResult {
    ToolResult::error(ToolError::Cancelled.to_string())
}

async fn run_model_bash_with_cancel(
    arguments: &serde_json::Value,
    tool_context: &ToolContext,
    cancel_token: &CancellationToken,
) -> ToolResult {
    tokio::select! {
        biased;
        result = execute_model_bash_for_runtime(tool_context, arguments.clone()) => {
            result.unwrap_or_else(|err| ToolResult::error(err.to_string()))
        }
        () = cancel_token.cancelled() => cancelled_tool_result(),
    }
}

fn default_tool_context(
    config: &AgentConfig,
    model: Arc<dyn ModelClient>,
    registry: Arc<ToolRegistry>,
    turn: u32,
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
                .with_multi_agent(config.multi_agent.clone())
                .with_child_runtime(config.clone(), model, registry, turn)
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
