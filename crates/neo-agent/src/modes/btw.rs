//! In-memory runtime runner for `/btw`-style sidecar dialogs.
//!
//! A `BtwRunner` projects inherited parent messages into a lightweight sidecar
//! context, attaches a deny-all before-tool hook, and streams model output as
//! [`BtwEvent`] values over an unbounded channel. It does not persist anything
//! to a JSONL session.

use std::sync::Arc;

use anyhow::Context;
use futures::StreamExt;
use neo_agent_core::runtime::AgentRuntimeError;
use neo_agent_core::sidecar::{deny_sidecar_tool_call, sidecar_projected_messages};
use neo_agent_core::skills::SkillStore;
use neo_agent_core::{AgentContext, AgentEvent, AgentMessage, AgentRuntime, StopReason};
use neo_ai::{ModelClient, ModelSpec};
use neo_tui::widgets::btw_panel::{BtwPanelState, BtwPhase, BtwTurn};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::modes::run::{agent_config_for_app, tool_registry_for_config};
use crate::resources;

/// Events emitted by a running `/btw` sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BtwEvent {
    /// The sidecar turn has started.
    Started {
        /// Stable identifier for this sidecar invocation.
        sidecar_id: String,
        /// The prompt text sent to the sidecar model.
        prompt: String,
    },
    /// A thinking/reasoning delta from the model.
    ThinkingDelta(String),
    /// A text delta from the model.
    TextDelta(String),
    /// A tool call was denied by the sidecar deny-all hook.
    ToolDenied { message: String },
    /// The sidecar turn finished normally.
    Finished,
    /// The sidecar turn was cancelled.
    Cancelled,
    /// The sidecar turn failed with an error.
    Failed(String),
}

/// In-memory runner for a `/btw` side question.
///
/// The runner owns the model, application config, and inherited parent
/// messages. Each [`run`](Self::run) call starts a fresh sidecar turn with its
/// own cancellation token; cancelling the runner only affects the currently
/// active sidecar turn, not any main turn.
pub struct BtwRunner {
    model: ModelSpec,
    client: Arc<dyn ModelClient>,
    config: AppConfig,
    context: Arc<Mutex<AgentContext>>,
    cancel_token: std::sync::Mutex<Option<CancellationToken>>,
}

impl BtwRunner {
    /// Create a new sidecar runner.
    #[must_use]
    pub fn new(
        model: ModelSpec,
        client: Arc<dyn ModelClient>,
        config: AppConfig,
        inherited_messages: &[AgentMessage],
    ) -> Self {
        let mut context = AgentContext::new();
        for message in sidecar_projected_messages(inherited_messages) {
            context.append_message(message);
        }

        Self {
            model,
            client,
            config,
            context: Arc::new(Mutex::new(context)),
            cancel_token: std::sync::Mutex::new(None),
        }
    }

    /// Cancel the currently active sidecar turn, if any.
    ///
    /// Cancellation is independent of any main-turn cancellation token.
    pub fn cancel(&self) {
        if let Ok(guard) = self.cancel_token.lock() {
            guard.as_ref().map(CancellationToken::cancel);
        }
    }

    /// Run a sidecar turn for the given prompt and return an event receiver.
    ///
    /// The returned receiver will receive [`BtwEvent::Started`] first, followed
    /// by zero or more [`BtwEvent::ThinkingDelta`] / [`BtwEvent::TextDelta`]
    /// events, and finally one of [`BtwEvent::Finished`],
    /// [`BtwEvent::Cancelled`], or [`BtwEvent::Failed`].
    ///
    /// # Errors
    ///
    /// Returns an error if the agent config, skill store, or tool registry
    /// cannot be built from the application config.
    pub async fn run(&self, prompt: String) -> anyhow::Result<mpsc::UnboundedReceiver<BtwEvent>> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let sidecar_id = Uuid::new_v4().to_string();

        let skill_store = self.load_skill_store()?;
        let agent_config = agent_config_for_app(self.model.clone(), &self.config, None, None)
            .context("failed to build agent config for sidecar")?
            .with_before_tool_call(deny_sidecar_tool_call);
        let tools = tool_registry_for_config(&self.config, Arc::clone(&agent_config.todos), None)
            .await
            .context("failed to build tool registry for sidecar")?;
        let runtime = AgentRuntime::with_tools_and_skills(
            agent_config,
            Arc::clone(&self.client),
            tools,
            skill_store,
        );

        let cancel_token = CancellationToken::new();
        if let Ok(mut guard) = self.cancel_token.lock() {
            *guard = Some(cancel_token.clone());
        }

        let _ = event_tx.send(BtwEvent::Started {
            sidecar_id,
            prompt: prompt.clone(),
        });

        let context = Arc::clone(&self.context);
        tokio::spawn(async move {
            let user_message = AgentMessage::user_text(prompt);
            let mut context = context.lock().await;
            let mut stream = runtime.run_turn_with_cancel(&mut context, user_message, cancel_token);

            let mut terminal_sent = false;
            while let Some(event) = stream.next().await {
                if terminal_sent {
                    break;
                }
                terminal_sent = forward_event(&event_tx, event);
            }
        });

        Ok(event_rx)
    }

    fn load_skill_store(&self) -> anyhow::Result<SkillStore> {
        resources::load_skill_store(
            crate::config::neo_home().as_deref(),
            &self.config.extra_skill_dirs,
            &self.config.skill_path,
        )
        .context("failed to load skill store for sidecar")
    }
}

/// Apply a [`BtwEvent`] to a [`BtwPanelState`].
///
/// This mapping lives in `neo-agent` because `BtwEvent` is private to this
/// crate and `BtwPanelState` lives in `neo-tui`.
pub fn update_btw_panel_state(state: &mut BtwPanelState, event: BtwEvent) {
    match event {
        BtwEvent::Started { prompt, .. } => {
            state
                .sidecar
                .turns
                .push(BtwTurn::new(prompt).with_phase(BtwPhase::Running));
            state.sidecar.phase = BtwPhase::Running;
            state.status_message = None;
        }
        BtwEvent::ThinkingDelta(delta) => {
            if let Some(turn) = state.sidecar.turns.last_mut() {
                turn.thinking.push_str(&delta);
            }
        }
        BtwEvent::TextDelta(delta) => {
            if let Some(turn) = state.sidecar.turns.last_mut() {
                turn.answer.push_str(&delta);
            }
        }
        BtwEvent::ToolDenied { message } => {
            if let Some(turn) = state.sidecar.turns.last_mut() {
                turn.error = Some(message);
                turn.phase = BtwPhase::Failed;
            } else {
                state.status_message = Some(message);
            }
        }
        BtwEvent::Finished => {
            if let Some(turn) = state.sidecar.turns.last_mut() {
                turn.phase = BtwPhase::Done;
            }
            state.sidecar.phase = BtwPhase::Done;
        }
        BtwEvent::Cancelled => {
            if let Some(turn) = state.sidecar.turns.last_mut() {
                if turn.answer.is_empty() && turn.thinking.is_empty() && turn.error.is_none() {
                    state.sidecar.turns.pop();
                } else {
                    turn.phase = BtwPhase::Cancelled;
                }
            }
            state.sidecar.phase = BtwPhase::Cancelled;
        }
        BtwEvent::Failed(message) => {
            if let Some(turn) = state.sidecar.turns.last_mut() {
                turn.error = Some(message.clone());
                turn.phase = BtwPhase::Failed;
            } else {
                state.status_message = Some(message);
            }
            state.sidecar.phase = BtwPhase::Failed;
        }
    }
}

fn forward_event(
    event_tx: &mpsc::UnboundedSender<BtwEvent>,
    event: Result<AgentEvent, AgentRuntimeError>,
) -> bool {
    match event {
        Ok(AgentEvent::ThinkingDelta { text, .. }) => {
            let _ = event_tx.send(BtwEvent::ThinkingDelta(text));
            false
        }
        Ok(AgentEvent::TextDelta { text, .. }) => {
            let _ = event_tx.send(BtwEvent::TextDelta(text));
            false
        }
        Ok(AgentEvent::ToolExecutionFinished { result, .. }) if result.is_error => {
            let _ = event_tx.send(BtwEvent::ToolDenied {
                message: result.content,
            });
            false
        }
        Ok(
            AgentEvent::MessageFinished {
                stop_reason: StopReason::Cancelled,
                ..
            }
            | AgentEvent::TurnFinished {
                stop_reason: StopReason::Cancelled,
                ..
            },
        )
        | Err(AgentRuntimeError::Cancelled) => {
            let _ = event_tx.send(BtwEvent::Cancelled);
            true
        }
        Ok(AgentEvent::MessageFinished { stop_reason, .. }) => {
            if stop_reason == StopReason::Error {
                // The matching `AgentEvent::Error` already emits `Failed`.
                return false;
            }
            let _ = event_tx.send(BtwEvent::Finished);
            true
        }
        Ok(AgentEvent::Error { message, .. }) => {
            let _ = event_tx.send(BtwEvent::Failed(message));
            true
        }
        Err(error) => {
            let _ = event_tx.send(BtwEvent::Failed(error.to_string()));
            true
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "test_cases/btw.rs"]
mod tests;
