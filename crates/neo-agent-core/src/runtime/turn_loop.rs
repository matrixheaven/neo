use std::sync::Arc;

use neo_ai::{AiError, ModelClient};
use tokio_util::sync::CancellationToken;

use super::chat_request::{chat_request, validate_model_capabilities};
use super::compaction_controller::{
    CompactionController, CompactionDecision, DeferredCompaction, ToolGroupBudgetState,
};
use super::config::AgentConfig;
use super::error::AgentRuntimeError;
use super::events::{
    EventEmitter, emit_context_window_snapshot, emit_goal_event_from_result, emit_todo_event,
};
use super::instruction_context::{InstructionContextBridge, PendingEpochAdmission};
use super::permission::current_permission_mode;
use super::plan_orchestration::{
    attach_enter_plan_details, emit_plan_tool_event, enter_plan_mode_state,
};
use super::queue::{
    SteerInputHandle, drain_live_steer_input, drain_next_pending_queue, drain_steering_queue,
};
use super::retry::{retry_delay, wait_for_retry};
use super::stream_aggregator::run_model_attempt;
use super::tool_dispatch::{
    apply_reconciliation_decision, continues_after_terminating_batch, execute_tool_calls,
    terminates_tool_batch,
};
use crate::compaction::CompactionError;
use crate::compaction::projection::{ProjectionMode, ProjectionPlan};
use crate::compaction::summary::run_full_compaction;
use crate::goal::GoalManager;
use crate::instructions::{
    InstructionEpochData, InstructionReconcileKind, InstructionReconcileRequest,
};
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

/// Run one logical model turn, recovering from `ContextOverflow` via forced
/// compaction while sharing one network retry policy across both requests.
async fn run_model_turn_with_recovery(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    request: neo_ai::ChatRequest,
    turn: u32,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<Option<AgentMessage>, AgentRuntimeError> {
    emitter.emit(AgentEvent::TurnStarted { turn });
    let mut retries_used = 0;
    let result = match run_model_request_with_retries(
        model,
        config,
        &request,
        turn,
        emitter,
        cancel_token,
        &mut retries_used,
    )
    .await
    {
        Err(error) if should_recover_from_overflow(&error) => {
            recover_from_overflow(
                model,
                config,
                emitter,
                cancel_token,
                turn,
                &mut retries_used,
            )
            .await
        }
        result => result,
    }?;

    let stop_reason = match &result {
        Some(AgentMessage::Assistant { stop_reason, .. }) => *stop_reason,
        _ => StopReason::EndTurn,
    };
    emit_effective_context_window(config, emitter, turn).await;
    emitter.emit(AgentEvent::TurnFinished { turn, stop_reason });
    Ok(result)
}

async fn run_model_request_with_retries(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    request: &neo_ai::ChatRequest,
    turn: u32,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    retries_used: &mut u32,
) -> Result<Option<AgentMessage>, AgentRuntimeError> {
    loop {
        match run_model_attempt(
            Arc::clone(model),
            config,
            request.clone(),
            turn,
            emitter,
            cancel_token,
            (*retries_used > 0).then_some(*retries_used),
        )
        .await
        {
            Ok(message) => {
                if *retries_used > 0 {
                    emitter.emit(AgentEvent::RetrySucceeded {
                        turn,
                        retries_used: *retries_used,
                    });
                }
                return Ok(message);
            }
            Err(AgentRuntimeError::Model(error)) if error.is_retryable() => {
                if *retries_used >= config.max_retries {
                    emitter.emit(AgentEvent::RetryExhausted {
                        turn,
                        retries_used: *retries_used,
                        error_code: error.code().to_owned(),
                        message: error.to_string(),
                    });
                    return Err(error.into());
                }

                let next_retry = retries_used.saturating_add(1);
                let delay = retry_delay(&error, next_retry);
                emitter.emit(AgentEvent::RetryScheduled {
                    turn,
                    retry: next_retry,
                    max_retries: config.max_retries,
                    delay_ms: delay.as_millis().try_into().unwrap_or(u64::MAX),
                    error_code: error.code().to_owned(),
                    message: error.to_string(),
                });
                wait_for_retry(delay, cancel_token).await?;
                *retries_used = next_retry;
                emitter.emit(AgentEvent::RetryStarted {
                    turn,
                    retry: next_retry,
                    max_retries: config.max_retries,
                });
            }
            Err(error) => return Err(error),
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
    retries_used: &mut u32,
) -> Result<Option<AgentMessage>, AgentRuntimeError> {
    let snapshot = super::context_budget::ContextBudgetEstimator::snapshot(
        config,
        &emitter.context,
        ProjectionPlan::disabled(),
    );
    super::config::observe_context_overflow(config, snapshot.raw_effective_tokens);
    let snapshot = super::context_budget::ContextBudgetEstimator::snapshot(
        config,
        &emitter.context,
        ProjectionPlan::disabled(),
    );
    let mut compaction_events = Vec::new();
    run_full_compaction(
        model,
        config,
        &mut emitter.context,
        crate::CompactionReason::Threshold,
        snapshot,
        None,
        cancel_token,
        |event| compaction_events.push(event),
    )
    .await?;
    for event in compaction_events {
        emitter.emit(event);
    }

    // Rebuild request with compacted context and retry once.
    let retry_request = chat_request(config, &emitter.context, &ProjectionPlan::disabled()).await;
    run_model_request_with_retries(
        model,
        config,
        &retry_request,
        turn,
        emitter,
        cancel_token,
        retries_used,
    )
    .await
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
    let mut pending_compaction_debt: Option<DeferredCompaction> = None;
    drain_live_steer_input(&steer_input, emitter);
    let mut pending_messages = drain_steering_queue(&config, emitter);

    loop {
        if !pending_messages.is_empty() {
            append_queued_messages(emitter, pending_messages);
        }

        append_runtime_reminders(&config, emitter);
        rehydrate_instruction_context_after_compaction(emitter).await;

        if let Some((turn, stop_reason)) = terminal_pre_model_stop(emitter, &cancel_token) {
            final_turn = turn;
            final_stop_reason = stop_reason;
            break;
        }

        let turn = emitter.context.turns.saturating_add(1);
        let projection_plan = prepare_model_request(
            &model,
            &config,
            emitter,
            &cancel_token,
            pending_compaction_debt.take(),
        )
        .await?;
        let request = chat_request(&config, &emitter.context, &projection_plan).await;
        validate_model_capabilities(&request)?;
        let assistant = match run_model_turn_with_recovery(
            &model,
            &config,
            request,
            turn,
            emitter,
            &cancel_token,
        )
        .await
        {
            Ok(assistant) => assistant,
            Err(AgentRuntimeError::Model(AiError::Cancelled)) => {
                emitter.emit(AgentEvent::TurnFinished {
                    turn,
                    stop_reason: StopReason::Cancelled,
                });
                final_turn = turn;
                final_stop_reason = StopReason::Cancelled;
                break;
            }
            Err(AgentRuntimeError::Model(error)) => {
                let retry_after = match &error {
                    AiError::RateLimit { retry_after, .. }
                    | AiError::Server { retry_after, .. } => {
                        retry_after.as_ref().map(std::time::Duration::as_secs)
                    }
                    _ => None,
                };
                emitter.emit(AgentEvent::Error {
                    turn,
                    message: error.to_string(),
                    code: Some(error.code().to_owned()),
                    retry_after,
                });
                emitter.emit(AgentEvent::TurnFinished {
                    turn,
                    stop_reason: StopReason::Error,
                });
                final_turn = turn;
                final_stop_reason = StopReason::Error;
                break;
            }
            Err(error) => return Err(error),
        };
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
        let outcome = execute_tool_calls(
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
        let super::tool_dispatch::ToolBatchOutcome {
            results: mut tool_results,
            pending_epoch,
            executed_any,
            preflight_targets,
        } = outcome;
        let synthesized = pending_epoch.is_some();
        // For EnterPlanMode: create the plan file and inject its path into the
        // tool result so the model knows where to write. Must happen before
        // append_tool_result_messages and before the duplicate enter in
        // emit_tool_side_effect_events. Synthesized (deferred/blocked) results
        // carry no side effects, so mode transitions are skipped for them.
        if !synthesized {
            let has_enter_plan_mode = tool_results
                .iter()
                .any(|(tc, _)| tc.name.as_ref() == "EnterPlanMode");
            if has_enter_plan_mode {
                enter_plan_mode_state(&config);
                attach_enter_plan_details(&config, &mut tool_results);
            }
        }
        append_tool_result_messages(&tool_results, emitter);
        // Deferred/blocked batches emit exactly one instruction epoch after
        // every tool result, then continue to the next model request without
        // TurnFinished, a new user message, or automatic tool replay. The
        // epoch is admitted compact-first: when it would cross the existing
        // compaction threshold, ordinary history is compacted (instruction
        // bodies excluded) and rehydrated BEFORE the epoch lands — never
        // inject a large epoch and immediately summarize it.
        if let Some(pending) = pending_epoch {
            let fingerprint = pending.fingerprint;
            if let Err(error) = admit_pending_epoch_compact_first(
                &model,
                &config,
                emitter,
                &cancel_token,
                pending.epoch,
            )
            .await
            {
                emitter.emit(AgentEvent::Error {
                    turn,
                    message: error.to_string(),
                    code: error.code().map(str::to_owned),
                    retry_after: None,
                });
                emitter.emit(AgentEvent::TurnFinished {
                    turn,
                    stop_reason: StopReason::Error,
                });
                final_turn = turn;
                final_stop_reason = StopReason::Error;
                break;
            }
            super::tool_dispatch::record_decision_fingerprint(&mut emitter.context, &fingerprint);
        }
        pending_compaction_debt =
            observe_tool_group_debt(&config, emitter, turn, tool_results.len());
        emit_effective_context_window(&config, emitter, turn).await;
        if !synthesized {
            emit_tool_side_effect_events(turn, &config, &tool_results, emitter);
        }
        drain_live_steer_input(&steer_input, emitter);
        // After any actual tool result and before the next provider request,
        // reconcile the touched scopes: a tool that edited or removed an
        // instruction source produces an Updated/Removed epoch, and a newly
        // created nested AGENTS.md is picked up before any later batch.
        if executed_any {
            reconcile_after_tool_execution(&config, emitter, preflight_targets).await;
        }
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

/// Establish the baseline instruction epoch for contexts that have never
/// seen one (new sessions and pre-feature resumes with
/// `visible_generation == 0`). Emitted before the first user message so the
/// baseline is durable and instruction-before-request ordering is
/// unambiguous. No-op without an attached registry or after any epoch.
pub(super) async fn establish_instruction_baseline(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
) {
    let Some(registry) = emitter.context.instruction_registry() else {
        return;
    };
    if emitter.context.instruction_state().visible_generation != 0 {
        return;
    }
    let request = InstructionReconcileRequest {
        agent_id: config
            .agent_id
            .clone()
            .unwrap_or_else(|| crate::session::MAIN_AGENT_ID.to_owned()),
        kind: InstructionReconcileKind::Baseline,
        target_directories: Vec::new(),
        budget: InstructionContextBridge::budget(config, &emitter.context),
        deferred_tool_ids: Vec::new(),
    };
    let decision = registry
        .reconcile(request, emitter.context.instruction_state())
        .await;
    apply_reconciliation_decision(config, emitter, decision, &[]);
}

/// Repair pinned instruction content at a live provider boundary when it is
/// missing: after a replayed or live full compaction, replay drops pinned
/// instruction messages without reconstructing the context-only rehydration,
/// so the first live boundary re-runs rehydration against current disk. The
/// guard (epoch admitted, visible revisions, no pinned instruction message)
/// keeps an already-intact context untouched.
async fn rehydrate_instruction_context_after_compaction(emitter: &mut EventEmitter) {
    let Some(registry) = emitter.context.instruction_registry() else {
        return;
    };
    {
        let state = emitter.context.instruction_state();
        if state.visible_generation == 0 || state.visible_revisions.is_empty() {
            return;
        }
    }
    if emitter
        .context
        .messages()
        .iter()
        .any(|message| matches!(message, AgentMessage::Instruction { .. }))
    {
        return;
    }
    match InstructionContextBridge::rehydrate_after_compaction(&registry, &mut emitter.context)
        .await
    {
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "instruction rehydration failed; the next preflight surfaces it");
        }
    }
}

/// Admit one pending defer/block epoch with compact-first ordering. When the
/// bridge reports the epoch would cross the existing compaction threshold,
/// ordinary history is compacted FIRST (pinned instruction bodies are
/// excluded from the summary input), the current instruction chain is
/// rehydrated byte-exactly, and the admission decision is re-taken on a fresh
/// snapshot — never inject a large epoch and immediately summarize it.
///
/// When even compaction cannot create enough safe capacity, this returns the
/// typed context-overflow error instead of admitting: the epoch's own budget
/// clamp already applied deterministic whole-bundle omission at reconcile
/// time, so what remains cannot fit without overflowing the request. The
/// decision fingerprint is left unrecorded, so a later turn re-preflights and
/// re-derives the epoch.
async fn admit_pending_epoch_compact_first(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    epoch: InstructionEpochData,
) -> Result<(), AgentRuntimeError> {
    if InstructionContextBridge::prepare_pending_epoch(config, &emitter.context, &epoch)
        == PendingEpochAdmission::CompactFirst
    {
        let snapshot = super::context_budget::ContextBudgetEstimator::snapshot(
            config,
            &emitter.context,
            ProjectionPlan::disabled(),
        );
        let mut compaction_events = Vec::new();
        match run_full_compaction(
            model,
            config,
            &mut emitter.context,
            crate::CompactionReason::Threshold,
            snapshot,
            None,
            cancel_token,
            |event| compaction_events.push(event),
        )
        .await
        {
            Ok(_) => {
                for event in compaction_events {
                    emitter.emit(event);
                }
            }
            // Nothing safe to compact: fall through to the capacity re-check.
            Err(CompactionError::NoBoundary) => {}
            Err(error) => return Err(error.into()),
        }
        // Post-compaction rehydrate, then admit: restore the exact current
        // instruction chain dropped by compaction before the pending bundle
        // lands. Failure is non-fatal; the next preflight re-derives it as a
        // typed Blocked epoch.
        rehydrate_instruction_context_after_compaction(emitter).await;
        if InstructionContextBridge::prepare_pending_epoch(config, &emitter.context, &epoch)
            == PendingEpochAdmission::CompactFirst
        {
            return Err(AgentRuntimeError::Model(AiError::ContextOverflow {
                message: format!(
                    "pending instruction epoch (generation {}) still exceeds the context budget \
                     after compaction",
                    epoch.generation
                ),
            }));
        }
    }
    emitter.emit(AgentEvent::InstructionEpoch { epoch });
    Ok(())
}

/// Reconcile the scopes a just-executed batch touched, before the next
/// provider request. A tool that edited or removed an instruction source was
/// governed by the old revision for that tool; this appends the resulting
/// `Updated`/`Removed` epoch. A newly created nested `AGENTS.md` is picked
/// up here before any later batch in its scope.
async fn reconcile_after_tool_execution(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    preflight_targets: Option<Vec<std::path::PathBuf>>,
) {
    let Some(targets) = preflight_targets else {
        return;
    };
    let Some(registry) = emitter.context.instruction_registry() else {
        return;
    };
    let request = InstructionReconcileRequest {
        agent_id: config
            .agent_id
            .clone()
            .unwrap_or_else(|| crate::session::MAIN_AGENT_ID.to_owned()),
        kind: InstructionReconcileKind::ToolPreflight,
        target_directories: targets.clone(),
        budget: InstructionContextBridge::budget(config, &emitter.context),
        deferred_tool_ids: Vec::new(),
    };
    let decision = registry
        .reconcile(request, emitter.context.instruction_state())
        .await;
    apply_reconciliation_decision(config, emitter, decision, &targets);
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

async fn prepare_model_request(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    pending_debt: Option<DeferredCompaction>,
) -> Result<ProjectionPlan, AgentRuntimeError> {
    let manual_requested = take_manual_compact_request(config).is_some();
    let request_projection = request_projection_plan(config, &emitter.context);
    let mut snapshot = super::context_budget::ContextBudgetEstimator::snapshot(
        config,
        &emitter.context,
        request_projection,
    );

    let projection_plan = match CompactionController::decide_before_model_call(
        snapshot.clone(),
        pending_debt.as_ref(),
        manual_requested,
    ) {
        CompactionDecision::NoAction { snapshot: decided } => {
            snapshot = decided;
            ProjectionPlan::disabled()
        }
        CompactionDecision::UseProjectionOnly {
            snapshot: decided,
            plan,
        } => {
            snapshot = decided;
            plan
        }
        CompactionDecision::RunFullCompaction {
            snapshot: decided,
            reason,
            ..
        } => {
            let mut compaction_events = Vec::new();
            let compaction_result = run_full_compaction(
                model,
                config,
                &mut emitter.context,
                reason,
                decided,
                None,
                cancel_token,
                |event| compaction_events.push(event),
            )
            .await;
            if matches!(compaction_result, Err(CompactionError::NoBoundary))
                && reason == crate::CompactionReason::Threshold
            {
                snapshot = super::context_budget::ContextBudgetEstimator::snapshot(
                    config,
                    &emitter.context,
                    request_projection_plan(config, &emitter.context),
                );
                snapshot.projection
            } else {
                compaction_result?;
                for event in compaction_events {
                    emitter.emit(event);
                }
                snapshot = super::context_budget::ContextBudgetEstimator::snapshot(
                    config,
                    &emitter.context,
                    request_projection_plan(config, &emitter.context),
                );
                snapshot.projection
            }
        }
        CompactionDecision::ForceAfterOverflow {
            snapshot: decided, ..
        } => {
            let mut compaction_events = Vec::new();
            run_full_compaction(
                model,
                config,
                &mut emitter.context,
                crate::CompactionReason::Threshold,
                decided,
                None,
                cancel_token,
                |event| compaction_events.push(event),
            )
            .await?;
            for event in compaction_events {
                emitter.emit(event);
            }
            snapshot = super::context_budget::ContextBudgetEstimator::snapshot(
                config,
                &emitter.context,
                request_projection_plan(config, &emitter.context),
            );
            snapshot.projection
        }
        CompactionDecision::StopWithContextError { message, .. } => {
            return Err(AgentRuntimeError::Model(AiError::ContextOverflow {
                message,
            }));
        }
    };

    emit_context_window_snapshot(emitter, &snapshot);
    Ok(projection_plan)
}

fn observe_tool_group_debt(
    config: &AgentConfig,
    emitter: &EventEmitter,
    turn: u32,
    tool_result_count: usize,
) -> Option<DeferredCompaction> {
    if tool_result_count == 0 {
        return None;
    }
    let snapshot = super::context_budget::ContextBudgetEstimator::snapshot(
        config,
        &emitter.context,
        request_projection_plan(config, &emitter.context),
    );
    let mut group = ToolGroupBudgetState::new(turn, tool_result_count, snapshot.clone());
    group.observe_completed_result(tool_result_count.saturating_sub(1), snapshot)
}

fn request_projection_plan(
    config: &AgentConfig,
    context: &super::context::AgentContext,
) -> ProjectionPlan {
    let Some(settings) = config.compaction else {
        return ProjectionPlan::disabled();
    };
    if !settings.micro_enabled {
        return ProjectionPlan::disabled();
    }
    ProjectionPlan {
        enabled: true,
        cutoff_index: context
            .messages()
            .len()
            .saturating_sub(settings.micro_keep_recent),
        min_tool_result_tokens: 1_000,
        keep_recent_messages: settings.micro_keep_recent,
        mode: ProjectionMode::Request,
    }
}

fn take_manual_compact_request(config: &AgentConfig) -> Option<String> {
    config.manual_compact_request.lock().map_or_else(
        |poisoned| poisoned.into_inner().take(),
        |mut guard| guard.take(),
    )
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
    let mut snapshot = super::context_budget::ContextBudgetEstimator::snapshot(
        config,
        &emitter.context,
        ProjectionPlan::disabled(),
    );
    snapshot.turn = turn;
    emit_context_window_snapshot(emitter, &snapshot);
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
