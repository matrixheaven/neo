use std::sync::Arc;

use neo_ai::{AiError, ModelClient};
use tokio_util::sync::CancellationToken;

use super::chat_request::{chat_request, validate_model_capabilities};
use super::compaction_trigger::maybe_compact;
use super::config::AgentConfig;
use super::error::AgentRuntimeError;
use super::events::{
    EventEmitter, emit_context_window_update, emit_goal_event_from_result, emit_todo_event,
};
use super::permission::current_permission_mode;
use super::plan_orchestration::{
    attach_enter_plan_details, emit_plan_tool_event, enter_plan_mode_state,
};
use super::queue::{
    SteerInputHandle, drain_live_steer_input, drain_next_pending_queue, drain_steering_queue,
};
use super::stream_aggregator::run_model_turn;
use super::tokens::{estimate_messages_tokens, estimate_tokens_with_config};
use super::tool_dispatch::{
    continues_after_terminating_batch, execute_tool_calls, terminates_tool_batch,
};
use crate::goal::GoalManager;
use crate::skills::SkillStoreHandle;
use crate::{
    AgentEvent, AgentMessage, AgentToolCall, Content, PermissionMode, PlanModeInjector,
    ProcessSupervisor, StopReason, ToolRegistry, ToolResult,
};

/// Whether an error represents a context overflow that compaction might fix.
fn should_recover_from_overflow(err: &AgentRuntimeError) -> bool {
    let AgentRuntimeError::Model(ai_err) = err else {
        return false;
    };
    matches!(ai_err, AiError::ContextOverflow { .. })
}

/// Run `run_model_turn`, recovering from `ContextOverflow` via forced
/// compaction + a single retry. Non-recoverable errors are propagated.
async fn run_model_turn_with_recovery(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    request: neo_ai::ChatRequest,
    turn: u32,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<Option<AgentMessage>, AgentRuntimeError> {
    let model_result = run_model_turn(
        Arc::clone(model),
        config,
        request,
        turn,
        emitter,
        cancel_token.clone(),
    )
    .await;

    match model_result {
        Ok(result) => Ok(result),
        Err(e) => {
            if !should_recover_from_overflow(&e) {
                return Err(e);
            }
            recover_from_overflow(model, config, emitter, cancel_token, turn).await
        }
    }
}

/// Attempt forced compaction and a single retry after a context overflow.
async fn recover_from_overflow(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    turn: u32,
) -> Result<Option<AgentMessage>, AgentRuntimeError> {
    // Record observed overflow for adaptive threshold.
    let messages_snapshot = emitter.context.messages().to_vec();
    let estimated = estimate_messages_tokens(&messages_snapshot);
    super::config::observe_context_overflow(config, estimated);

    // Trigger forced compaction via the live path. Setting the
    // manual_compact_request mutex is the same mechanism `/compact`
    // uses — `evaluate_compaction_need` reads it and sets
    // `force = true` in the trigger.
    {
        let mut guard = config
            .manual_compact_request
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(String::new());
    }
    maybe_compact(model, config, emitter, cancel_token).await;

    // Rebuild request with compacted context and retry once.
    let retry_request = chat_request(config, &emitter.context).await;
    run_model_turn(
        Arc::clone(model),
        config,
        retry_request,
        turn,
        emitter,
        cancel_token.clone(),
    )
    .await
    .map_err(|_| {
        AgentRuntimeError::Model(AiError::Stream {
            message: "compaction recovery failed after context overflow".into(),
        })
    })
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub(super) async fn run_agent_turn(
    model: Arc<dyn ModelClient>,
    config: AgentConfig,
    tools: Option<Arc<ToolRegistry>>,
    skills: Option<SkillStoreHandle>,
    goal_manager: Option<Arc<GoalManager>>,
    steer_input: SteerInputHandle,
    emitter: &mut EventEmitter,
    cancel_token: CancellationToken,
    process_supervisor: ProcessSupervisor,
) -> Result<(), AgentRuntimeError> {
    let mut final_turn: u32;
    let mut final_stop_reason = StopReason::EndTurn;
    drain_live_steer_input(&steer_input, emitter);
    let mut pending_messages = drain_steering_queue(&config, emitter);

    loop {
        if !pending_messages.is_empty() {
            append_queued_messages(emitter, pending_messages);
        }

        maybe_compact(&model, &config, emitter, &cancel_token).await;
        append_runtime_reminders(&config, emitter);

        if let Some((turn, stop_reason)) = terminal_pre_model_stop(emitter, &cancel_token) {
            final_turn = turn;
            final_stop_reason = stop_reason;
            break;
        }

        let turn = emitter.context.turns.saturating_add(1);
        let request = chat_request(&config, &emitter.context).await;
        emit_context_window_update(
            emitter,
            turn,
            estimate_tokens_with_config(&request.messages, &config),
        );
        validate_model_capabilities(&request)?;
        let assistant =
            run_model_turn_with_recovery(&model, &config, request, turn, emitter, &cancel_token)
                .await?;
        final_turn = turn;
        if let Some(AgentMessage::Assistant { stop_reason, .. }) = &assistant {
            final_stop_reason = *stop_reason;
        }

        let Some(AgentMessage::Assistant {
            tool_calls: model_tool_calls,
            stop_reason: StopReason::ToolUse,
            ..
        }) = assistant.clone()
        else {
            drain_live_steer_input(&steer_input, emitter);
            if let Some(messages) =
                next_pending_after_assistant(&config, emitter, goal_manager.as_deref())
            {
                pending_messages = messages;
                continue;
            }
            break;
        };
        let tool_calls = model_tool_calls.clone();
        if tool_calls.is_empty() {
            emitter.emit(AgentEvent::Error {
                turn,
                message: "Provider reported tool calls but emitted no structured tool calls"
                    .to_owned(),
                code: None,
                retry_after: None,
            });
            emitter.emit(AgentEvent::TurnFinished {
                turn,
                stop_reason: StopReason::Error,
            });
            final_stop_reason = StopReason::Error;
            break;
        }

        let Some(registry) = &tools else {
            break;
        };
        let mut tool_results = execute_tool_calls(
            &config,
            Arc::clone(&model),
            Arc::clone(registry),
            skills.as_ref(),
            turn,
            &tool_calls,
            emitter,
            &cancel_token,
            &process_supervisor,
        )
        .await?;
        if cancel_token.is_cancelled() {
            emitter.emit(AgentEvent::TurnFinished {
                turn,
                stop_reason: StopReason::Cancelled,
            });
            final_stop_reason = StopReason::Cancelled;
            break;
        }
        // For EnterPlanMode: create the plan file and inject its path into the
        // tool result so the model knows where to write. Must happen before
        // append_tool_result_messages and before the duplicate enter in
        // emit_tool_side_effect_events.
        let has_enter_plan_mode = tool_results
            .iter()
            .any(|(tc, _)| tc.name.as_ref() == "EnterPlanMode");
        if has_enter_plan_mode {
            enter_plan_mode_state(&config);
            attach_enter_plan_details(&config, &mut tool_results);
        }
        append_tool_result_messages(&tool_results, emitter);
        emit_effective_context_window(&config, emitter, turn).await;
        emit_tool_side_effect_events(turn, &config, &tool_results, emitter);
        drain_live_steer_input(&steer_input, emitter);
        if terminates_tool_batch(&tool_results) {
            if continues_after_terminating_batch(&tool_results) {
                pending_messages = drain_steering_queue(&config, emitter);
                continue;
            }
            break;
        }
        pending_messages = drain_steering_queue(&config, emitter);
    }

    process_supervisor.cleanup_all().await;
    emit_run_finished(&config, emitter, final_turn, final_stop_reason).await;
    Ok(())
}

fn next_pending_after_assistant(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    goal_manager: Option<&GoalManager>,
) -> Option<Vec<AgentMessage>> {
    let pending_messages = drain_next_pending_queue(config, emitter);
    if pending_messages.is_empty() {
        goal_continuation_messages(goal_manager)
    } else {
        Some(pending_messages)
    }
}

fn append_runtime_reminders(config: &AgentConfig, emitter: &mut EventEmitter) {
    append_permission_mode_reminder(config, emitter);
    append_plan_mode_reminder(config, emitter);
    append_goal_mode_authoring_reminder(config, emitter);
}

const AUTO_MODE_ENTER_REMINDER: &str = "Auto permission mode is active. Tool approvals will be handled automatically while this mode remains enabled.\n  - Continue normally without pausing for approval prompts.\n  - Do not ask the user approval questions while auto mode is active. Make a reasonable decision and continue without asking the user.";
const AUTO_MODE_EXIT_REMINDER: &str = "Auto permission mode is no longer active. Tool approvals and permission checks are back to the current mode.\n  - Continue normally, but expect approval prompts or denials when a tool requires them.";

fn append_permission_mode_reminder(config: &AgentConfig, emitter: &mut EventEmitter) {
    let mode = current_permission_mode(config);
    let auto_reminded = auto_permission_mode_reminded(&emitter.context);
    match (mode, auto_reminded) {
        (PermissionMode::Auto, false) => {
            emitter.emit(AgentEvent::MessageAppended {
                message: AgentMessage::system_reminder_with_origin(
                    AUTO_MODE_ENTER_REMINDER,
                    "permission_mode_auto_enter",
                ),
            });
        }
        (PermissionMode::Auto, true) | (_, false) => {}
        (_, true) => {
            emitter.emit(AgentEvent::MessageAppended {
                message: AgentMessage::system_reminder_with_origin(
                    AUTO_MODE_EXIT_REMINDER,
                    "permission_mode_auto_exit",
                ),
            });
        }
    }
}

fn auto_permission_mode_reminded(context: &super::context::AgentContext) -> bool {
    let mut active = false;
    for message in context.messages() {
        if is_injection_variant(message, "permission_mode_auto_enter") {
            active = true;
        }
        if is_injection_variant(message, "permission_mode_auto_exit") {
            active = false;
        }
    }
    active
}

fn append_plan_mode_reminder(config: &AgentConfig, emitter: &mut EventEmitter) {
    let mut injector = PlanModeInjector::new(Arc::clone(&config.plan_mode));
    if let Some(message) = injector.inject(&emitter.context) {
        emitter.emit(AgentEvent::MessageAppended { message });
    }
}

const GOAL_MODE_AUTHORING_REMINDER: &str = "Goal mode is active. Do not start a durable goal directly with StartGoal. First draft a structured goal with objective, acceptance criteria, phase plan, risks/assumptions, and validation commands. Then call ExitGoalMode with the reviewed objective, completion_criterion, and ordered phases so the user can Accept, Reject, or Revise it in a blocking dialog.";

fn append_goal_mode_authoring_reminder(config: &AgentConfig, emitter: &mut EventEmitter) {
    if !config.goal_mode_authoring || goal_authoring_reminded(&emitter.context) {
        return;
    }
    emitter.emit(AgentEvent::MessageAppended {
        message: AgentMessage::system_reminder_with_origin(
            GOAL_MODE_AUTHORING_REMINDER,
            "goal_mode",
        ),
    });
}

fn goal_authoring_reminded(context: &super::context::AgentContext) -> bool {
    context
        .messages()
        .iter()
        .any(|message| is_injection_variant(message, "goal_mode"))
}

fn is_injection_variant(message: &AgentMessage, variant: &str) -> bool {
    matches!(
        message,
        AgentMessage::User {
            origin,
            ..
        } if origin.is_injection_variant(variant)
    )
}

fn append_tool_result_messages(
    tool_results: &[(AgentToolCall, ToolResult)],
    emitter: &mut EventEmitter,
) {
    for (tool_call, result) in tool_results {
        let message = AgentMessage::tool_result(
            tool_call.id.clone(),
            tool_call.name.clone(),
            vec![Content::text(result.content.clone())],
            result.is_error,
        );
        emitter.emit(AgentEvent::MessageAppended { message });
    }
}

fn emit_tool_side_effect_events(
    turn: u32,
    config: &AgentConfig,
    tool_results: &[(AgentToolCall, ToolResult)],
    emitter: &mut EventEmitter,
) {
    for (tool_call, result) in tool_results {
        emit_plan_tool_event(turn, config, tool_call.name.as_ref(), result, emitter);
        emit_todo_event(turn, config, tool_call.name.as_ref(), result, emitter);
        emit_goal_event_from_result(turn, tool_call.name.as_ref(), result, emitter);
    }
}

fn goal_continuation_messages(manager: Option<&GoalManager>) -> Option<Vec<AgentMessage>> {
    let manager = manager?;
    let goal = manager.active()?;
    let objective = goal.objective;
    let artifact = goal.artifact_dir.as_ref().map_or_else(
        || "(no artifact directory)".to_owned(),
        |path| path.display().to_string(),
    );
    let phase = goal
        .current_phase
        .and_then(|index| goal.phases.get(index).cloned())
        .unwrap_or_else(|| "No current phase recorded.".to_owned());
    Some(vec![AgentMessage::system_reminder_with_origin(
        format!(
            "Goal still active: {objective}. Continue making progress using the goal artifacts.\n\n\
         Artifact directory: {artifact}\n\
         Current phase: {phase}\n\n\
         Work phase by phase. On repeated failures, retry once, write a focused fix spec on the second failure, and report blocked with handoff details on the third. Run a final audit before marking complete. \
         Use `UpdateGoalStatus` when the goal is complete or blocked, or `GetGoalStatus` to check current state."
        ),
        "goal_continuation",
    )])
}

pub(super) async fn emit_run_finished(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    turn: u32,
    stop_reason: StopReason,
) {
    emit_effective_context_window(config, emitter, turn).await;
    emitter.emit(AgentEvent::RunFinished { turn, stop_reason });
}

pub(super) async fn emit_effective_context_window(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    turn: u32,
) {
    // Avoid rebuilding the entire ChatRequest just to count tokens.
    // Use the cached incremental estimate from AgentContext plus the cached
    // tool-spec tokens from AgentConfig.  This is approximate (omits system
    // prompt / workspace preamble overhead) but sufficiently accurate for the
    // context-window bar display, and avoids 3-4 O(n) message rebuilds per
    // turn iteration.
    let tool_tokens = *config
        .cached_tool_spec_tokens
        .get_or_init(|| super::tokens::estimate_tool_specs_tokens(&config.tools));
    let used_tokens = emitter.context.estimated_tokens() + tool_tokens;
    emit_context_window_update(emitter, turn, used_tokens);
}

fn terminal_pre_model_stop(
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Option<(u32, StopReason)> {
    if cancel_token.is_cancelled() {
        let turn = emitter.context.turns.saturating_add(1);
        emitter.emit(AgentEvent::TurnFinished {
            turn,
            stop_reason: StopReason::Cancelled,
        });
        return Some((turn, StopReason::Cancelled));
    }

    None
}

fn append_queued_messages(emitter: &mut EventEmitter, messages: Vec<AgentMessage>) {
    for message in messages {
        emitter.emit(AgentEvent::MessageAppended { message });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_ai::{ApiKind, ModelCapabilities, ModelSpec, ProviderId};
    use tokio::sync::mpsc;

    fn test_model() -> ModelSpec {
        ModelSpec {
            provider: ProviderId("test".to_owned()),
            model: "test-model".to_owned(),
            api: ApiKind::Local,
            capabilities: ModelCapabilities::chat(),
        }
    }

    fn test_emitter(context: super::super::context::AgentContext) -> EventEmitter {
        let (tx, _rx) = mpsc::unbounded_channel();
        EventEmitter::new(tx, context)
    }

    #[test]
    fn should_recover_from_context_overflow_error() {
        let err = AgentRuntimeError::Model(AiError::ContextOverflow {
            message: "too long".into(),
        });
        assert!(should_recover_from_overflow(&err));
    }

    #[test]
    fn should_not_recover_from_auth_error() {
        let err = AgentRuntimeError::Model(AiError::Auth {
            message: "bad key".into(),
        });
        assert!(!should_recover_from_overflow(&err));
    }

    #[test]
    fn goal_authoring_reminder_is_user_role_system_reminder() {
        let config = AgentConfig::for_model(test_model()).with_goal_mode_authoring(true);
        let mut emitter = test_emitter(super::super::context::AgentContext::new());

        append_runtime_reminders(&config, &mut emitter);

        let Some(AgentMessage::User {
            content, origin, ..
        }) = emitter.context.messages().last()
        else {
            panic!("expected user-role system reminder");
        };
        assert!(origin.is_injection_variant("goal_mode"));
        let text = content
            .iter()
            .filter_map(Content::as_text)
            .collect::<String>();
        assert!(text.contains("<system-reminder>"), "{text}");
        assert!(text.contains("Goal mode is active"), "{text}");
        assert!(text.contains("ExitGoalMode"), "{text}");
    }

    #[test]
    fn auto_permission_reminders_are_append_only_user_messages() {
        let config =
            AgentConfig::for_model(test_model()).with_permission_mode(PermissionMode::Auto);
        let mut emitter = test_emitter(super::super::context::AgentContext::new());

        append_runtime_reminders(&config, &mut emitter);
        append_runtime_reminders(&config, &mut emitter);

        assert_eq!(emitter.context.messages().len(), 1);
        assert!(matches!(
            emitter.context.messages().last(),
            Some(AgentMessage::User { .. })
        ));
        assert!(
            emitter.context.messages()[0]
                .text()
                .contains("Auto permission mode is active")
        );

        if let Ok(mut live) = config.live_permission_mode.write() {
            *live = PermissionMode::Ask;
        }
        append_runtime_reminders(&config, &mut emitter);

        assert_eq!(emitter.context.messages().len(), 2);
        assert!(
            emitter.context.messages()[1]
                .text()
                .contains("Auto permission mode is no longer active")
        );
    }

    #[test]
    fn user_text_cannot_spoof_auto_permission_reminder_state() {
        let config =
            AgentConfig::for_model(test_model()).with_permission_mode(PermissionMode::Auto);
        let mut context = super::super::context::AgentContext::new();
        context.append_message(AgentMessage::user_text(
            "Auto permission mode is active. Please explain this phrase.",
        ));
        let mut emitter = test_emitter(context);

        append_runtime_reminders(&config, &mut emitter);

        assert_eq!(emitter.context.messages().len(), 2);
        assert!(
            emitter.context.messages()[1]
                .text()
                .contains(AUTO_MODE_ENTER_REMINDER)
        );
    }

    #[test]
    fn user_text_cannot_spoof_goal_authoring_reminder_state() {
        let config = AgentConfig::for_model(test_model()).with_goal_mode_authoring(true);
        let mut context = super::super::context::AgentContext::new();
        context.append_message(AgentMessage::user_text(
            "Do not start a durable goal directly is text from this report.",
        ));
        let mut emitter = test_emitter(context);

        append_runtime_reminders(&config, &mut emitter);

        assert_eq!(emitter.context.messages().len(), 2);
        assert!(
            emitter.context.messages()[1]
                .text()
                .contains(GOAL_MODE_AUTHORING_REMINDER)
        );
    }
}
