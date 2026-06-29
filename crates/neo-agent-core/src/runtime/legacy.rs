use std::sync::Arc;

use futures::{StreamExt, stream};
use neo_ai::ModelClient;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::chat_request::*;
use super::compaction_trigger::*;
use super::config::*;
use super::context::AgentContext;
use super::events::*;
use super::plan_orchestration::*;
use super::queue::*;
use super::skill_dispatch::*;
use super::stream_aggregator::*;
use super::tokens::*;
use super::tool_dispatch::*;
use crate::goal::GoalManager;
use crate::skills::SkillStore;
use crate::{
    AgentEvent, AgentMessage, AgentToolCall, Content, ProcessSupervisor, StopReason, ToolError,
    ToolRegistry, ToolResult, compaction,
};

#[derive(Debug, Error)]
pub enum AgentRuntimeError {
    #[error("model stream failed: {0}")]
    Model(#[from] neo_ai::AiError),
    #[error("tool execution failed: {0}")]
    Tool(#[from] ToolError),
    #[error("runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("compaction failed: {0}")]
    Compaction(#[from] compaction::CompactionError),
    #[error("turn cancelled")]
    Cancelled,
}

pub type AgentEventStream<'a> = stream::BoxStream<'a, Result<AgentEvent, AgentRuntimeError>>;

#[derive(Clone)]
pub struct AgentRuntime {
    config: AgentConfig,
    model: Arc<dyn ModelClient>,
    tools: Option<Arc<ToolRegistry>>,
    skills: Option<Arc<SkillStore>>,
    goal_manager: Option<Arc<GoalManager>>,
    steer_input: SteerInputHandle,
}

impl AgentRuntime {
    #[must_use]
    pub fn new(config: AgentConfig, model: Arc<dyn ModelClient>) -> Self {
        Self {
            config,
            model,
            tools: None,
            skills: None,
            goal_manager: None,
            steer_input: SteerInputHandle::new(),
        }
    }

    #[must_use]
    pub fn with_tools(
        config: AgentConfig,
        model: Arc<dyn ModelClient>,
        tools: ToolRegistry,
    ) -> Self {
        let mut config = config;
        config.tools = tools.specs();
        Self {
            config,
            model,
            tools: Some(Arc::new(tools)),
            skills: None,
            goal_manager: None,
            steer_input: SteerInputHandle::new(),
        }
    }

    #[must_use]
    pub fn with_tools_and_skills(
        mut config: AgentConfig,
        model: Arc<dyn ModelClient>,
        tools: ToolRegistry,
        skills: SkillStore,
    ) -> Self {
        let mut tool_specs = tools.specs();
        tool_specs.push(invoke_skill_tool_spec());
        config.tools = tool_specs;
        Self {
            config,
            model,
            tools: Some(Arc::new(tools)),
            skills: Some(Arc::new(skills)),
            goal_manager: None,
            steer_input: SteerInputHandle::new(),
        }
    }

    /// Attach a shared steer-input handle so the controller can push live
    /// input into a running turn. The runtime drains this handle at each
    /// step boundary and feeds it into the existing queue machinery.
    #[must_use]
    pub fn with_steer_input(mut self, steer_input: SteerInputHandle) -> Self {
        self.steer_input = steer_input;
        self
    }

    #[must_use]
    pub fn with_goal_manager(mut self, manager: &Arc<GoalManager>) -> Self {
        self.goal_manager = Some(Arc::clone(manager));
        self
    }

    pub fn tools_mut(&mut self) -> Option<&mut Arc<ToolRegistry>> {
        self.tools.as_mut()
    }

    #[must_use]
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Restore plan-mode state from a replayed context.
    pub fn restore_plan_mode(&self, context: &AgentContext) {
        if !context.is_plan_mode_active() {
            return;
        }
        let Some(id) = context.plan_mode_id() else {
            return;
        };
        let Some(plans_dir) = plan_mode_plans_dir(&self.config) else {
            return;
        };
        if let Ok(mut pm) = self.config.plan_mode.write() {
            pm.restore_enter(&plans_dir, id);
        }
    }

    pub fn run_turn<'a>(
        &'a self,
        context: &'a mut AgentContext,
        message: AgentMessage,
    ) -> AgentEventStream<'a> {
        self.run_turn_with_cancel(context, message, CancellationToken::new())
    }

    pub fn run_turn_with_cancel<'a>(
        &'a self,
        context: &'a mut AgentContext,
        message: AgentMessage,
        cancel_token: CancellationToken,
    ) -> AgentEventStream<'a> {
        if let Ok(mut todos) = self.config.todos.lock() {
            todos.clone_from(&context.todos);
        }

        let live_context = context.clone();
        let model = Arc::clone(&self.model);
        let tools = self.tools.clone();
        let skills = self.skills.clone();
        let goal_manager = self.goal_manager.clone();
        let config = self.config.clone();
        let steer_input = self.steer_input.clone();
        let process_supervisor = ProcessSupervisor::default();
        let (sender, receiver) = mpsc::unbounded_channel();
        let (final_sender, final_receiver) = oneshot::channel();

        tokio::spawn(async move {
            let mut emitter = EventEmitter::new(sender, live_context);
            emitter.emit(AgentEvent::RunStarted {
                turn: emitter.context.turns.saturating_add(1),
            });
            if let Some(skill_context) = emitter.context.take_skill_context() {
                emitter.emit(AgentEvent::MessageAppended {
                    message: skill_context,
                });
            }
            emitter.emit(AgentEvent::MessageAppended { message });
            if let Err(err) = run_agent_turn(
                model,
                config,
                tools,
                skills,
                goal_manager,
                steer_input,
                &mut emitter,
                cancel_token,
                process_supervisor.clone(),
            )
            .await
            {
                process_supervisor.cleanup_all().await;
                emitter.emit(AgentEvent::RunFinished {
                    turn: emitter.context.turns.saturating_add(1),
                    stop_reason: StopReason::Error,
                });
                let _ = emitter.send_error(err);
            }
            let _ = final_sender.send(emitter.context);
        });

        stream::unfold(
            SpawnedRun {
                receiver,
                final_receiver: Some(final_receiver),
                context,
            },
            |mut state| async move {
                if let Some(event) = state.receiver.recv().await {
                    if let Ok(event) = &event {
                        EventEmitter::apply_to_context(state.context, event);
                    }
                    return Some((event, state));
                }
                if let Some(final_receiver) = state.final_receiver.take()
                    && let Ok(final_context) = final_receiver.await
                {
                    *state.context = final_context;
                }
                None
            },
        )
        .boxed()
    }

    /// Run a compaction-only turn.  This does not append a user message and does
    /// not call the model afterwards; it simply executes any pending compaction
    /// (manual or automatic) and finishes.  Used by the TUI's `/compact` slash
    /// command when the session is idle.
    pub fn run_compaction_turn<'a>(
        &'a self,
        context: &'a mut AgentContext,
    ) -> AgentEventStream<'a> {
        self.run_compaction_turn_with_cancel(context, CancellationToken::new())
    }

    /// Run a compaction-only turn with an external cancellation token.
    pub fn run_compaction_turn_with_cancel<'a>(
        &'a self,
        context: &'a mut AgentContext,
        cancel_token: CancellationToken,
    ) -> AgentEventStream<'a> {
        let live_context = context.clone();
        let model = Arc::clone(&self.model);
        let config = self.config.clone();
        let process_supervisor = ProcessSupervisor::default();
        let (sender, receiver) = mpsc::unbounded_channel();
        let (final_sender, final_receiver) = oneshot::channel();

        tokio::spawn(async move {
            let mut emitter = EventEmitter::new(sender, live_context);
            let turn = emitter.context.turns.saturating_add(1);
            emitter.emit(AgentEvent::RunStarted { turn });
            maybe_compact(&model, &config, &mut emitter, &cancel_token).await;
            process_supervisor.cleanup_all().await;
            emit_run_finished(&config, &mut emitter, turn, StopReason::EndTurn).await;
            let _ = final_sender.send(emitter.context);
        });

        stream::unfold(
            SpawnedRun {
                receiver,
                final_receiver: Some(final_receiver),
                context,
            },
            |mut state| async move {
                if let Some(event) = state.receiver.recv().await {
                    if let Ok(event) = &event {
                        EventEmitter::apply_to_context(state.context, event);
                    }
                    return Some((event, state));
                }
                if let Some(final_receiver) = state.final_receiver.take()
                    && let Ok(final_context) = final_receiver.await
                {
                    *state.context = final_context;
                }
                None
            },
        )
        .boxed()
    }
}

struct SpawnedRun<'a> {
    receiver: mpsc::UnboundedReceiver<Result<AgentEvent, AgentRuntimeError>>,
    final_receiver: Option<oneshot::Receiver<AgentContext>>,
    context: &'a mut AgentContext,
}

/// Compute the session-scoped plans directory (`<session_dir>/plans`).
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn run_agent_turn(
    model: Arc<dyn ModelClient>,
    config: AgentConfig,
    tools: Option<Arc<ToolRegistry>>,
    skills: Option<Arc<SkillStore>>,
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
            estimate_chat_messages_tokens(&request.messages),
        );
        validate_model_capabilities(&request)?;
        let assistant = run_model_turn(
            Arc::clone(&model),
            &config,
            request,
            turn,
            emitter,
            cancel_token.clone(),
        )
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

        let Some(registry) = &tools else {
            break;
        };
        let mut tool_results = execute_tool_calls(
            &config,
            registry,
            skills.as_deref(),
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
        // Attach plan details + the selected-option prefix BEFORE appending the
        // tool results to the context so the next model turn sees the prefix,
        // and before the side-effect events flip plan mode off.
        attach_exit_plan_details(&config, &mut tool_results);
        // For EnterPlanMode: create the plan file and inject its path into the
        // tool result so the model knows where to write. Must happen before
        // append_tool_result_messages and before the duplicate enter in
        // emit_tool_side_effect_events.
        let has_enter_plan_mode = tool_results
            .iter()
            .any(|(tc, _)| tc.name == "EnterPlanMode");
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
        emit_plan_tool_event(turn, config, tool_call.name.as_str(), result, emitter);
        emit_todo_event(turn, config, tool_call.name.as_str(), result, emitter);
        emit_goal_event_from_result(turn, tool_call.name.as_str(), result, emitter);
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
    Some(vec![AgentMessage::system_text(format!(
        "Goal still active: {objective}. Continue making progress using the goal artifacts.\n\n\
         Artifact directory: {artifact}\n\
         Current phase: {phase}\n\n\
         Work phase by phase. On repeated failures, retry once, write a focused fix spec on the second failure, and report blocked with handoff details on the third. Run a final audit before marking complete. \
         Use `UpdateGoalStatus` when the goal is complete or blocked, or `GetGoalStatus` to check current state."
    ))])
}

async fn emit_run_finished(
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
    let request = chat_request_for_context_estimate(config, &emitter.context).await;
    emit_context_window_update(
        emitter,
        turn,
        estimate_chat_messages_tokens(&request.messages),
    );
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
    use futures::TryStreamExt;

    fn fake_compaction_config() -> AgentConfig {
        let mut config = AgentConfig::for_model(neo_ai::ModelSpec {
            provider: neo_ai::ProviderId("fake".to_owned()),
            model: "fake".to_owned(),
            api: neo_ai::ApiKind::OpenAiChatCompletions,
            capabilities: neo_ai::ModelCapabilities::chat()
                .with_max_context_tokens(100_000)
                .with_max_output_tokens(4_096),
        });
        config = config.with_compaction(CompactionSettings {
            enabled: true,
            max_estimated_tokens: 100_000,
            keep_recent_messages: 4,
            trigger_ratio: 0.85,
            reserved_context_tokens: 50_000,
            max_recent_messages: 4,
            micro_enabled: false,
            micro_keep_recent: 20,
        });
        config
    }

    #[tokio::test]
    async fn compaction_only_turn_runs_compaction_without_model_reply() {
        let fake = neo_ai::providers::fake::FakeModelClient::new(vec![
            neo_ai::AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            },
            neo_ai::AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ]);
        let mut config = fake_compaction_config();
        config.manual_compact_request = Arc::new(std::sync::Mutex::new(Some(String::new())));
        let runtime = AgentRuntime::new(config, Arc::new(fake));
        let mut context = AgentContext::new();
        context.append_message(AgentMessage::user_text("hello"));
        context.append_message(AgentMessage::assistant(
            vec![Content::text("hi")],
            Vec::new(),
            StopReason::EndTurn,
        ));
        context.append_message(AgentMessage::user_text("world"));
        context.append_message(AgentMessage::assistant(
            vec![Content::text("yes")],
            Vec::new(),
            StopReason::EndTurn,
        ));

        let events: Vec<AgentEvent> = runtime
            .run_compaction_turn(&mut context)
            .try_collect()
            .await
            .expect("compaction turn succeeds");

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::CompactionStarted { .. })),
            "missing CompactionStarted"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::CompactionApplied { .. })),
            "missing CompactionApplied"
        );
        assert!(
            !events.iter().any(|e| matches!(
                e,
                AgentEvent::MessageAppended {
                    message: AgentMessage::Assistant { .. },
                    ..
                }
            )),
            "compaction-only turn must not produce an assistant reply"
        );
        assert!(context.compaction_summary().is_some());
    }

    #[tokio::test]
    async fn compaction_turn_passes_custom_instruction_to_summary_llm() {
        let fake = neo_ai::providers::fake::FakeModelClient::new(vec![
            neo_ai::AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            },
            neo_ai::AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ]);
        let mut config = fake_compaction_config();
        config.manual_compact_request =
            Arc::new(std::sync::Mutex::new(Some("keep the todo list".to_owned())));
        let runtime = AgentRuntime::new(config, Arc::new(fake.clone()));
        let mut context = AgentContext::new();
        context.append_message(AgentMessage::user_text("hello"));
        context.append_message(AgentMessage::assistant(
            vec![Content::text("hi")],
            Vec::new(),
            StopReason::EndTurn,
        ));
        context.append_message(AgentMessage::user_text("world"));
        context.append_message(AgentMessage::assistant(
            vec![Content::text("yes")],
            Vec::new(),
            StopReason::EndTurn,
        ));

        let _events: Vec<AgentEvent> = runtime
            .run_compaction_turn(&mut context)
            .try_collect()
            .await
            .expect("compaction turn succeeds");

        let requests = fake.requests();
        let request = requests.first().expect("summary LLM was called");
        let last_message_text = request
            .messages
            .last()
            .and_then(|message| match message {
                neo_ai::ChatMessage::User { content } => Some(
                    content
                        .iter()
                        .filter_map(|part| match part {
                            neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<String>(),
                ),
                _ => None,
            })
            .unwrap_or_default();
        assert!(
            last_message_text.contains("keep the todo list"),
            "instruction not in compaction prompt: {last_message_text}"
        );
    }
}
