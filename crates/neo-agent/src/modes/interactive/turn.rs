//! Extracted: turn lifecycle — start, drain, cancel, wait, abort, and queue
//! draining after turn completion.

use std::time::Duration;

use anyhow::Result;

use neo_agent_core::{Content, MessageOrigin, PendingQuestion};

use neo_tui::shell::{DevelopmentMode, GoalModeStatus, StreamUpdate};

use tokio_util::sync::CancellationToken;

use super::InteractiveController;
use super::{FrameRequest, RunningTurn, TurnChannels, TurnRequest};

impl InteractiveController {
    pub(super) fn start_turn_with_prompt(&mut self, prompt: Vec<Content>) {
        self.start_turn_with_prompt_projection(prompt, MessageOrigin::User, None);
    }

    pub(super) fn start_turn_with_prompt_display(
        &mut self,
        prompt: Vec<Content>,
        display_text: String,
    ) {
        self.start_turn_with_prompt_projection(prompt, MessageOrigin::User, Some(display_text));
    }

    pub(super) fn start_turn_with_prompt_origin(
        &mut self,
        prompt: Vec<Content>,
        prompt_origin: MessageOrigin,
    ) {
        self.start_turn_with_prompt_projection(prompt, prompt_origin, None);
    }

    fn start_turn_with_prompt_projection(
        &mut self,
        prompt: Vec<Content>,
        prompt_origin: MessageOrigin,
        prompt_display_text: Option<String>,
    ) {
        if self.active_turn.is_some() {
            self.push_status("A turn is already running");
            return;
        }
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let (approval_tx, approval_rx) = tokio::sync::mpsc::unbounded_channel();
        let (session_id_tx, session_id_rx) = tokio::sync::mpsc::unbounded_channel();
        let (question_tx, question_rx) = tokio::sync::mpsc::unbounded_channel::<PendingQuestion>();
        let cancel_token = CancellationToken::new();
        let steer_input = neo_agent_core::SteerInputHandle::new();
        let instruction_registry = match self.instruction_registry_for_turn() {
            Ok(registry) => registry,
            Err(error) => {
                self.push_status(format!("Failed to load project instructions: {error}"));
                return;
            }
        };
        let channels = TurnChannels {
            events: event_tx.clone(),
            approvals: approval_tx,
            session_ids: session_id_tx,
            cancel_token: cancel_token.clone(),
            questions: question_tx,
            steer_input: steer_input.clone(),
        };
        let mut request = TurnRequest::new(
            prompt,
            self.active_session_id.clone(),
            self.active_model.clone(),
            self.current_reasoning.clone(),
        );
        request.prompt_display_text = prompt_display_text;
        request.prompt_origin = prompt_origin;
        request.permission_mode = self.permission_mode;
        request.live_permission_mode = std::sync::Arc::clone(&self.live_permission_mode);
        request.workspace_policy = std::sync::Arc::clone(&self.workspace_policy);
        request.plan_mode = std::sync::Arc::clone(&self.plan_mode);
        request.goal_mode_authoring = matches!(
            self.tui.chrome().development_mode(),
            DevelopmentMode::Goal(GoalModeStatus::Pending)
        );
        request.mcp_manager.clone_from(&self.mcp_manager);
        request.base_config.clone_from(&self.local_config);
        request
            .instruction_registry
            .clone_from(&instruction_registry);
        request.manual_compact_request = std::sync::Arc::clone(&self.manual_compact_request);
        let request = if let Some(skill_context) = self.pending_skill_context.take() {
            request.with_skill_context(skill_context)
        } else {
            request
        };
        let future = (self.run_turn)(request, channels);
        let task = tokio::spawn(async move {
            let result = future.await;
            if let Err(error) = &result {
                let _ = event_tx.send(Err(anyhow::anyhow!(error.to_string())));
            }
            result
        });
        self.active_turn = Some(RunningTurn {
            events: event_rx,
            approvals: approval_rx,
            session_ids: session_id_rx,
            task,
            cancel_token,
            questions: question_rx,
            steer_input,
            instruction_registry,
        });
    }

    pub(super) async fn wait_for_active_turn(&mut self) -> Result<()> {
        while self.active_turn.is_some() {
            self.drain_active_turn().await?;
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        Ok(())
    }

    pub(super) async fn cancel_active_turn(&mut self) -> Result<()> {
        if let Some(turn) = &self.active_turn {
            turn.cancel_token.cancel();
        }
        self.pending_approvals.clear();

        self.pending_questions.clear();
        self.pending_question_prompts.clear();
        self.pending_background_question_followups.clear();
        let result = if let Ok(result) =
            tokio::time::timeout(Duration::from_secs(2), self.wait_for_active_turn()).await
        {
            result
        } else {
            self.abort_active_turn();
            Ok(())
        };
        self.clear_interrupted_turn_state();
        result
    }

    pub(super) async fn drain_active_turn(&mut self) -> Result<FrameRequest> {
        let Some(mut turn) = self.active_turn.take() else {
            return Ok(FrameRequest::None);
        };
        let mut frame_request = self.drain_turn_channels(&mut turn);

        if turn.task.is_finished() {
            let turn_result = turn
                .task
                .await
                .map_err(|error| anyhow::anyhow!("interactive turn task failed: {error}"))?;
            // Turn-driver errors are already forwarded through the event channel
            // and rendered into the transcript. Keep the interactive shell alive.
            match turn_result {
                Ok(outcome) => {
                    if let Some(session_id) = outcome.session_id {
                        // `turn.task` was moved by `.await`; remaining fields are accessible.
                        self.bind_instruction_registry_to_session(
                            &session_id,
                            turn.instruction_registry.as_ref(),
                        );
                        self.set_active_session_id(session_id.clone());
                        self.refresh_terminal_title_for_session(&session_id);
                    }
                }
                Err(error) => {
                    self.tui
                        .chrome_mut()
                        .apply_stream_update(StreamUpdate::Error {
                            text: error.to_string(),
                        });
                    frame_request = frame_request.merge(FrameRequest::Coalesced);
                }
            }
            if self.refresh_git_status_now() {
                frame_request = frame_request.merge(FrameRequest::Coalesced);
            }
            self.start_next_queued_after_turn().await?;
            frame_request = frame_request.merge(FrameRequest::Coalesced);
        } else {
            self.active_turn = Some(turn);
        }
        Ok(frame_request)
    }

    pub(super) fn drain_workflow_events(&mut self) -> FrameRequest {
        let mut frame_request = FrameRequest::None;
        while let Ok(delivery) = self.workflow_events.try_recv() {
            match delivery {
                crate::modes::run::PersistedSessionWorkflowEvent::Event(envelope)
                    if self.active_session_id.as_deref() == Some(envelope.session_id.as_str())
                        && self.workflow_event_generation == envelope.generation =>
                {
                    frame_request = frame_request.merge(self.apply_turn_event(envelope.event));
                }
                crate::modes::run::PersistedSessionWorkflowEvent::Error {
                    session_id,
                    generation,
                    message,
                } if self.active_session_id.as_deref() == Some(session_id.as_str())
                    && self.workflow_event_generation == generation =>
                {
                    self.push_status(format!("Failed to persist workflow event: {message}"));
                    frame_request = frame_request.merge(FrameRequest::Coalesced);
                }
                crate::modes::run::PersistedSessionWorkflowEvent::Event(_)
                | crate::modes::run::PersistedSessionWorkflowEvent::Error { .. } => {}
            }
        }
        frame_request
    }

    pub(super) fn drain_workflow_approvals(&mut self) -> FrameRequest {
        let mut frame_request = FrameRequest::None;
        while let Ok(delivery) = self.workflow_approvals.try_recv() {
            if self.active_session_id.as_deref() == Some(delivery.session_id.as_str()) {
                if self.register_workflow_approval(&delivery.session_id, delivery.pending) {
                    frame_request = frame_request.merge(FrameRequest::Immediate);
                }
            } else if !delivery.pending.response_tx.is_closed() {
                self.workflow_approval_backlog
                    .entry(delivery.session_id)
                    .or_default()
                    .push_back(delivery.pending);
            } else {
                self.resolve_closed_approval_ui(&delivery.pending.request.id);
                frame_request = frame_request.merge(FrameRequest::Immediate);
            }
        }
        if self.prune_closed_pending_approvals() {
            frame_request = frame_request.merge(FrameRequest::Immediate);
        }
        frame_request
    }

    fn drain_turn_channels(&mut self, turn: &mut RunningTurn) -> FrameRequest {
        let mut frame_request = FrameRequest::None;

        while let Ok(session_id) = turn.session_ids.try_recv() {
            self.bind_instruction_registry_to_session(
                &session_id,
                turn.instruction_registry.as_ref(),
            );
            self.set_active_session_id(session_id);
            frame_request = frame_request.merge(FrameRequest::Coalesced);
        }
        while let Ok(approval) = turn.approvals.try_recv() {
            self.register_pending_approval(approval);
        }
        while let Ok(pending) = turn.questions.try_recv() {
            neo_tui::notify::notify_event(
                self.question_notification,
                neo_tui::notify::EventKind::Question,
            );
            self.register_pending_question(pending);
            frame_request = frame_request.merge(FrameRequest::Immediate);
        }
        while let Ok(event) = turn.events.try_recv() {
            match event {
                Ok(event) => {
                    self.notify_for_event(&event);
                    frame_request = frame_request.merge(self.apply_turn_event(event));
                }
                Err(error) => {
                    self.push_status(format!("Error: {error}"));
                    frame_request = frame_request.merge(FrameRequest::Coalesced);
                }
            }
        }
        frame_request
    }

    pub(super) async fn start_next_queued_after_turn(&mut self) -> Result<()> {
        if self.active_shell_command.is_none()
            && let Some(command) = self
                .tui
                .chrome_mut()
                .pending_input_mut()
                .drain_next_shell_command()
        {
            self.start_shell_command(command).await?;
        }
        if self.active_shell_command.is_none() {
            self.start_next_mcp_startup_prompt()?;
        }
        Ok(())
    }

    pub(super) fn start_next_mcp_startup_prompt(&mut self) -> Result<()> {
        if self.active_turn.is_some() || self.tui.chrome().mcp_startup_active() {
            return Ok(());
        }
        let Some(prompt) = self
            .tui
            .chrome_mut()
            .pending_input_mut()
            .drain_next_follow_up()
        else {
            return Ok(());
        };
        self.start_turn_from_submitted_prompt(prompt, true)?;
        Ok(())
    }

    pub(super) fn abort_active_turn(&mut self) {
        if let Some(turn) = self.active_turn.take() {
            turn.cancel_token.cancel();
            turn.task.abort();
        }
        self.pending_approvals.clear();

        self.pending_questions.clear();
        self.pending_question_prompts.clear();
        self.pending_background_question_followups.clear();
        self.clear_interrupted_turn_state();
    }
}
