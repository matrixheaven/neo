// Internal sub-module re-exports. Each `*` imports deliberately brings the
// full surface into scope (orchestrator file); refactoring each to a named
// import would be churn with no semantic benefit.
#![allow(clippy::wildcard_imports)]

use std::sync::Arc;

use futures::{Stream, StreamExt, stream};
use neo_ai::ModelClient;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::config::*;
use super::context::AgentContext;
use super::context_budget::ContextBudgetEstimator;
use super::error::AgentRuntimeError;
use super::events::*;
use super::plan_orchestration::*;
use super::queue::*;
use super::skill_dispatch::*;
use super::turn_loop::{
    AgentTurnRuntime, append_available_skills_snapshot, append_runtime_reminders,
    emit_run_finished, establish_instruction_baseline,
    rehydrate_instruction_context_after_compaction, run_agent_turn,
};
use crate::compaction::projection::ProjectionPlan;
use crate::compaction::summary::{FullCompactionInput, run_full_compaction};
use crate::goal::GoalManager;
use crate::skills::{SkillStore, SkillStoreHandle};
use crate::{AgentEvent, AgentMessage, ProcessSupervisor, StopReason, ToolRegistry};

pub struct AgentEventStream<'a> {
    inner: stream::BoxStream<'a, Result<AgentEvent, AgentRuntimeError>>,
    _workflow_event_drain_lease: Option<super::workflow_dispatch::WorkflowDispatchEventDrainLease>,
    cancel_token: CancellationToken,
    completed: bool,
}

struct SpawnedTurn {
    sender: mpsc::UnboundedSender<Result<AgentEvent, AgentRuntimeError>>,
    live_context: AgentContext,
    workflow_event_lease: Option<super::workflow_dispatch::WorkflowDispatchEventLease>,
    model: Arc<dyn ModelClient>,
    config: AgentConfig,
    tools: Option<Arc<ToolRegistry>>,
    skills: Option<SkillStoreHandle>,
    goal_manager: Option<Arc<GoalManager>>,
    steer_input: SteerInputHandle,
    message: AgentMessage,
    natural_user_turn: bool,
    process_supervisor: ProcessSupervisor,
    cancel_token: CancellationToken,
    final_sender: oneshot::Sender<AgentContext>,
}

impl Stream for AgentEventStream<'_> {
    type Item = Result<AgentEvent, AgentRuntimeError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(context) {
            std::task::Poll::Ready(None) => {
                self.completed = true;
                std::task::Poll::Ready(None)
            }
            other => other,
        }
    }
}

impl Drop for AgentEventStream<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.cancel_token.cancel();
        }
    }
}

#[derive(Clone)]
pub struct AgentRuntime {
    config: AgentConfig,
    model: Arc<dyn ModelClient>,
    tools: Option<Arc<ToolRegistry>>,
    skills: Option<SkillStoreHandle>,
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
    pub fn with_shared_tools(
        mut config: AgentConfig,
        model: Arc<dyn ModelClient>,
        tools: Arc<ToolRegistry>,
    ) -> Self {
        config.tools = tools.specs();
        Self::with_shared_tools_and_configured_specs(config, model, tools)
    }

    #[must_use]
    pub fn with_shared_tools_and_configured_specs(
        config: AgentConfig,
        model: Arc<dyn ModelClient>,
        tools: Arc<ToolRegistry>,
    ) -> Self {
        Self {
            config,
            model,
            tools: Some(tools),
            skills: None,
            goal_manager: None,
            steer_input: SteerInputHandle::new(),
        }
    }

    #[must_use]
    pub fn with_tools_and_skills(
        config: AgentConfig,
        model: Arc<dyn ModelClient>,
        tools: ToolRegistry,
        skills: SkillStore,
    ) -> Self {
        Self::with_tools_and_skill_handle(config, model, tools, SkillStoreHandle::new(skills))
    }

    #[must_use]
    pub fn with_tools_and_skill_handle(
        mut config: AgentConfig,
        model: Arc<dyn ModelClient>,
        tools: ToolRegistry,
        skills: SkillStoreHandle,
    ) -> Self {
        let mut tool_specs = tools.specs();
        tool_specs.push(invoke_skill_tool_spec());
        config.tools = tool_specs;
        Self {
            config,
            model,
            tools: Some(Arc::new(tools)),
            skills: Some(skills),
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

    /// Check a minimal post-compaction turn against the existing request budget.
    #[must_use]
    pub fn turn_messages_fit_after_compaction(
        &self,
        context: &AgentContext,
        messages: &[AgentMessage],
    ) -> bool {
        let (sender, _receiver) = mpsc::unbounded_channel();
        let mut projected_emitter = EventEmitter::new(sender, context.clone());
        if let Some(skill_context) = projected_emitter.context.take_skill_context() {
            projected_emitter.emit(AgentEvent::MessageAppended {
                message: skill_context,
            });
        }
        append_available_skills_snapshot(self.skills.as_ref(), &mut projected_emitter);
        for message in messages {
            projected_emitter.emit(AgentEvent::MessageAppended {
                message: message.clone(),
            });
        }
        append_runtime_reminders(&self.config, &mut projected_emitter);
        context_fits_after_compaction(&self.config, &projected_emitter.context)
    }

    /// Publish this runtime's canonical dependencies for recovered workflows.
    ///
    /// This prepares dispatch only; it does not start a model turn or workflow.
    pub fn refresh_workflow_dispatch(
        &self,
        context: &AgentContext,
    ) -> Result<(), AgentRuntimeError> {
        let tools = self.tools.as_ref().ok_or_else(|| {
            AgentRuntimeError::Tool(crate::ToolError::InvalidInput {
                tool: "Workflow".to_owned(),
                message: "workflow dispatch requires a configured tool registry".to_owned(),
            })
        })?;
        self.config
            .workflow_dispatch_resolver
            .refresh(super::workflow_dispatch::WorkflowDispatchSnapshot {
                config: self.config.clone(),
                model_client: Arc::clone(&self.model),
                registry: Arc::clone(tools),
                skills: self.skills.clone(),
                process_supervisor: ProcessSupervisor::default(),
                context: context.clone(),
            })
            .map_err(std::io::Error::other)?;
        Ok(())
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

        let instruction_registry = context
            .instruction_registry()
            .or_else(|| self.config.instruction_registry.clone());
        if let Some(registry) = &instruction_registry {
            registry.restore_generation(context.instruction_state().visible_generation);
            context.attach_instruction_registry(Arc::clone(registry));
        }
        let live_context = context.clone();
        let model = Arc::clone(&self.model);
        let tools = self.tools.clone();
        let skills = self.skills.clone();
        let goal_manager = self.goal_manager.clone();
        let mut config = self.config.clone();
        config.instruction_registry = instruction_registry;
        let steer_input = self.steer_input.clone();
        let natural_user_turn = matches!(
            &message,
            AgentMessage::User { origin, .. } if origin.is_user()
        );
        let process_supervisor = ProcessSupervisor::default();
        let (sender, receiver) = mpsc::unbounded_channel();
        let (final_sender, final_receiver) = oneshot::channel();
        let workflow_event_leases = config
            .workflow_dispatch_resolver
            .lease_event_route(
                config.session_directory.as_deref(),
                context.turns.saturating_add(1),
                make_tool_event_callback(EventSink {
                    sender: sender.clone(),
                }),
            )
            .ok();
        let (workflow_event_lease, workflow_event_drain_lease) =
            workflow_event_leases.map_or((None, None), |(lease, drain)| (Some(lease), Some(drain)));
        let stream_cancel_token = cancel_token.clone();

        tokio::spawn(Self::run_turn_spawned(SpawnedTurn {
            sender,
            live_context,
            workflow_event_lease,
            model,
            config,
            tools,
            skills,
            goal_manager,
            steer_input,
            message,
            natural_user_turn,
            process_supervisor,
            cancel_token,
            final_sender,
        }));

        let inner = stream::unfold(
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
        .boxed();
        AgentEventStream {
            inner,
            _workflow_event_drain_lease: workflow_event_drain_lease,
            cancel_token: stream_cancel_token,
            completed: false,
        }
    }

    async fn run_turn_spawned(turn: SpawnedTurn) {
        let SpawnedTurn {
            sender,
            live_context,
            workflow_event_lease,
            model,
            mut config,
            tools,
            skills,
            goal_manager,
            steer_input,
            message,
            natural_user_turn,
            process_supervisor,
            cancel_token,
            final_sender,
        } = turn;
        let mut emitter = EventEmitter::new(sender, live_context);
        let _workflow_event_lease = workflow_event_lease;
        emitter.emit(AgentEvent::RunStarted {
            turn: emitter.context.turns.saturating_add(1),
        });
        if let Some(skill_context) = emitter.context.take_skill_context() {
            emitter.emit(AgentEvent::MessageAppended {
                message: skill_context,
            });
        }
        // Baseline-before-user: new sessions and pre-feature resumes
        // (visible_generation == 0) establish one durable instruction
        // epoch before the first user message is appended.
        if let Err(err) =
            establish_instruction_baseline(&model, &config, &mut emitter, &cancel_token).await
        {
            process_supervisor.cleanup_all().await;
            emitter.emit(AgentEvent::MessageAppended { message });
            emitter.emit(AgentEvent::RunFinished {
                turn: emitter.context.turns.saturating_add(1),
                stop_reason: StopReason::Error,
            });
            let _ = emitter.send_error(err);
            let _ = final_sender.send(emitter.context);
            return;
        }
        append_available_skills_snapshot(skills.as_ref(), &mut emitter);
        if let Some(turn_injection) = config.turn_injection.take() {
            append_runtime_reminders(&config, &mut emitter);
            rehydrate_instruction_context_after_compaction(&mut emitter, false).await;

            let workflow_injection = AgentMessage::injection_text(
                format!(
                    "<workflow_turn_context applies_to=\"next_model_request_only\">\n{turn_injection}\n</workflow_turn_context>"
                ),
                "workflow_turn_context",
            );

            let (projection_sender, _projection_receiver) = mpsc::unbounded_channel();
            let mut projected_emitter =
                EventEmitter::new(projection_sender, emitter.context.clone());
            projected_emitter.emit(AgentEvent::MessageAppended {
                message: message.clone(),
            });
            if natural_user_turn {
                append_pending_workflow_notifications(&config, &mut projected_emitter);
            }
            projected_emitter.emit(AgentEvent::MessageAppended {
                message: workflow_injection.clone(),
            });
            if !context_fits_after_compaction(&config, &projected_emitter.context) {
                process_supervisor.cleanup_all().await;
                emitter.emit(AgentEvent::RunFinished {
                    turn: emitter.context.turns.saturating_add(1),
                    stop_reason: StopReason::Error,
                });
                let _ = emitter.send_error(AgentRuntimeError::WorkflowContextTooLarge);
                let _ = final_sender.send(emitter.context);
                return;
            }
            emitter.emit(AgentEvent::MessageAppended { message });
            if natural_user_turn {
                append_pending_workflow_notifications(&config, &mut emitter);
            }
            emitter.emit(AgentEvent::MessageAppended {
                message: workflow_injection,
            });
        } else {
            emitter.emit(AgentEvent::MessageAppended { message });
            if natural_user_turn {
                append_pending_workflow_notifications(&config, &mut emitter);
            }
        }
        if let Err(err) = run_agent_turn(
            AgentTurnRuntime {
                model,
                config,
                tools,
                skills,
                goal_manager,
                steer_input,
                cancel_token,
                process_supervisor: process_supervisor.clone(),
            },
            &mut emitter,
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
    }

    /// Run a compaction-only turn.  This does not append a user message and does
    /// not call the model afterwards; it simply executes any pending compaction
    /// (manual or automatic) and finishes.  Used by the TUI's `/compact` slash
    /// command when the session is idle.
    pub fn run_manual_compaction_turn<'a>(
        &'a self,
        context: &'a mut AgentContext,
    ) -> AgentEventStream<'a> {
        self.run_manual_compaction_turn_with_cancel(context, CancellationToken::new())
    }

    /// Run a compaction-only turn with an external cancellation token.
    pub fn run_manual_compaction_turn_with_cancel<'a>(
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
        let stream_cancel_token = cancel_token.clone();

        tokio::spawn(async move {
            let mut emitter = EventEmitter::new(sender, live_context);
            let turn = emitter.context.turns.saturating_add(1);
            emitter.emit(AgentEvent::RunStarted { turn });
            let instruction = config.manual_compact_request.lock().map_or_else(
                |poisoned| poisoned.into_inner().take(),
                |mut guard| guard.take(),
            );
            let snapshot = ContextBudgetEstimator::snapshot(
                &config,
                &emitter.context,
                ProjectionPlan::disabled(),
            );
            let compaction_sink = emitter.sink();
            let result = run_full_compaction(
                &model,
                &config,
                &mut emitter.context,
                FullCompactionInput {
                    reason: crate::CompactionReason::Manual,
                    snapshot,
                    custom_instruction: instruction.as_deref(),
                },
                &cancel_token,
                move |event| compaction_sink.emit_event(event),
            )
            .await;
            process_supervisor.cleanup_all().await;
            let stop_reason = if result.is_ok() {
                StopReason::EndTurn
            } else {
                StopReason::Error
            };
            emit_run_finished(&config, &mut emitter, turn, stop_reason).await;
            if let Err(err) = result {
                let _ = emitter.send_error(AgentRuntimeError::Compaction(err));
            }
            let _ = final_sender.send(emitter.context);
        });

        let inner = stream::unfold(
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
        .boxed();
        AgentEventStream {
            inner,
            _workflow_event_drain_lease: None,
            cancel_token: stream_cancel_token,
            completed: false,
        }
    }
}

pub(super) fn context_fits_after_compaction(config: &AgentConfig, context: &AgentContext) -> bool {
    let snapshot = ContextBudgetEstimator::snapshot(
        config,
        context,
        crate::compaction::projection::ProjectionPlan::disabled(),
    );
    let Some(cap) = [
        snapshot.effective_max_context_tokens,
        snapshot.absolute_max_tokens,
    ]
    .into_iter()
    .flatten()
    .min() else {
        return true;
    };
    snapshot
        .projected_tokens
        .saturating_add(snapshot.reserved_headroom_tokens)
        <= cap
}

struct SpawnedRun<'a> {
    receiver: mpsc::UnboundedReceiver<Result<AgentEvent, AgentRuntimeError>>,
    final_receiver: Option<oneshot::Receiver<AgentContext>>,
    context: &'a mut AgentContext,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Content;
    use futures::TryStreamExt;

    fn fake_compaction_config() -> AgentConfig {
        let mut config = AgentConfig::for_model(neo_ai::ModelSpec {
            provider: neo_ai::ProviderId("fake".to_owned()),
            model: "fake".to_owned(),
            api: neo_ai::ApiKind::OpenAi,
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
            snip_enabled: false,
            snip_min_tokens: 1_000,
            snip_keep_recent: 16,
            snip_trigger_ratio: 0.6,
            max_rounds: 5,
            max_retry_attempts: 5,
        });
        config
    }

    #[test]
    fn turn_messages_fit_after_compaction_uses_existing_budget_and_headroom() {
        let mut config = fake_compaction_config();
        config.turn_injection = Some("workflow catalog ".repeat(400_000));
        let runtime = AgentRuntime::new(
            config,
            Arc::new(neo_ai::providers::fake::FakeModelClient::default()),
        );

        assert!(!runtime.turn_messages_fit_after_compaction(
            &AgentContext::new(),
            &[AgentMessage::user_text("run this workflow")],
        ));

        let mut small_config = fake_compaction_config();
        small_config.turn_injection = Some("complete workflow catalog".to_owned());
        let small_runtime = AgentRuntime::new(
            small_config,
            Arc::new(neo_ai::providers::fake::FakeModelClient::default()),
        );
        assert!(small_runtime.turn_messages_fit_after_compaction(
            &AgentContext::new(),
            &[AgentMessage::user_text("run this workflow")],
        ));

        let mut existing_context = AgentContext::new();
        existing_context.append_message(AgentMessage::user_text("history ".repeat(300_000)));
        assert!(!small_runtime.turn_messages_fit_after_compaction(
            &existing_context,
            &[AgentMessage::user_text("run this workflow")],
        ));
    }

    #[test]
    fn turn_messages_fit_after_compaction_includes_runtime_skill_catalog() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skill_dir = temp.path().join("large-skill");
        std::fs::create_dir_all(&skill_dir).expect("skill directory");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: large-skill\ndescription: {}\n---\nBody\n",
                "large ".repeat(60_000)
            ),
        )
        .expect("skill file");

        let mut config = fake_compaction_config();
        config.turn_injection = Some("complete workflow catalog".to_owned());
        let runtime = AgentRuntime::with_tools_and_skills(
            config,
            Arc::new(neo_ai::providers::fake::FakeModelClient::default()),
            ToolRegistry::new(),
            SkillStore::load(&[], &[temp.path().to_path_buf()], Vec::new()),
        );

        assert!(!runtime.turn_messages_fit_after_compaction(
            &AgentContext::new(),
            &[AgentMessage::user_text("run this workflow")],
        ));
    }

    #[tokio::test]
    async fn workflow_context_capacity_guard_runs_before_provider_request() {
        let mut config = fake_compaction_config();
        config.turn_injection = Some("complete workflow catalog".to_owned());
        let runtime = AgentRuntime::new(
            config,
            Arc::new(neo_ai::providers::fake::FakeModelClient::default()),
        );
        let mut context = AgentContext::new();
        context.append_message(AgentMessage::user_text("history ".repeat(300_000)));

        let events = runtime
            .run_turn(&mut context, AgentMessage::user_text("run this workflow"))
            .collect::<Vec<_>>()
            .await;

        assert!(matches!(
            events.last(),
            Some(Err(AgentRuntimeError::WorkflowContextTooLarge))
        ));
        assert!(!events.iter().any(|event| matches!(
            event,
            Ok(AgentEvent::MessageAppended {
                message: AgentMessage::User { .. }
            })
        )));
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
            .run_manual_compaction_turn(&mut context)
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
    async fn failed_compaction_emits_error_terminal_event_before_stream_error() {
        let runtime = AgentRuntime::new(
            fake_compaction_config(),
            Arc::new(neo_ai::providers::fake::FakeModelClient::default()),
        );
        let mut context = AgentContext::new();

        let events = runtime
            .run_manual_compaction_turn(&mut context)
            .collect::<Vec<_>>()
            .await;

        let terminal_events = events
            .iter()
            .filter_map(|event| match event {
                Ok(AgentEvent::RunFinished { stop_reason, .. }) => Some(*stop_reason),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(terminal_events, [StopReason::Error]);
        assert!(matches!(
            events.last(),
            Some(Err(AgentRuntimeError::Compaction(_)))
        ));
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
            .run_manual_compaction_turn(&mut context)
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
