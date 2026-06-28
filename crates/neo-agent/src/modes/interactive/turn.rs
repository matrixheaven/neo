//! Extracted: turn lifecycle — start, drain, cancel, wait, abort, and queue
//! draining after turn completion.

use std::time::Duration;

use anyhow::Result;

use neo_agent_core::{Content, PendingQuestion};

use neo_tui::shell::{DevelopmentMode, GoalModeStatus, StreamUpdate};

use tokio_util::sync::CancellationToken;

use super::InteractiveController;
use super::{RunningTurn, TurnChannels, TurnRequest};

impl InteractiveController {
    pub(super) fn start_turn_with_prompt(
        &mut self,
        prompt: Vec<Content>,
        model_override: Option<super::SelectedModel>,
        show_user_message: bool,
    ) {
        if self.active_turn.is_some() {
            self.push_status("A turn is already running");
            return;
        }
        if show_user_message {
            self.tui
                .transcript_mut()
                .push_user_message(super::content_to_display_text(&prompt));
        }
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let (approval_tx, approval_rx) = tokio::sync::mpsc::unbounded_channel();
        let (session_id_tx, session_id_rx) = tokio::sync::mpsc::unbounded_channel();
        let (question_tx, question_rx) = tokio::sync::mpsc::unbounded_channel::<PendingQuestion>();
        let cancel_token = CancellationToken::new();
        let steer_input = neo_agent_core::SteerInputHandle::new();
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
            model_override.or_else(|| self.active_model.clone()),
            if self.current_thinking {
                Some(neo_ai::ReasoningEffort::High)
            } else {
                None
            },
        );
        request.permission_mode = self.permission_mode;
        request.live_permission_mode = std::sync::Arc::clone(&self.live_permission_mode);
        request.plan_mode = std::sync::Arc::clone(&self.plan_mode);
        request.goal_mode_authoring = matches!(
            self.tui.chrome().development_mode(),
            DevelopmentMode::Goal(GoalModeStatus::Pending)
        );
        request.plan_review_feedback = std::mem::take(&mut self.pending_plan_review_feedback);
        request.mcp_manager.clone_from(&self.mcp_manager);
        request.base_config.clone_from(&self.local_config);
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
        self.resolved_approvals.clear();
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

    pub(super) async fn drain_active_turn(&mut self) -> Result<()> {
        let Some(mut turn) = self.active_turn.take() else {
            return Ok(());
        };

        while let Ok(session_id) = turn.session_ids.try_recv() {
            self.set_active_session_id(session_id);
        }
        while let Ok(approval) = turn.approvals.try_recv() {
            self.register_pending_approval(approval);
        }
        while let Ok(pending) = turn.questions.try_recv() {
            self.register_pending_question(pending);
        }
        while let Ok(event) = turn.events.try_recv() {
            match event {
                Ok(event) => self.apply_turn_event(event),
                Err(error) => {
                    self.push_status(format!("Error: {error}"));
                }
            }
        }

        if turn.task.is_finished() {
            let turn_result = turn
                .task
                .await
                .map_err(|error| anyhow::anyhow!("interactive turn task failed: {error}"))?;
            while let Ok(session_id) = turn.session_ids.try_recv() {
                self.set_active_session_id(session_id);
            }
            while let Ok(approval) = turn.approvals.try_recv() {
                self.register_pending_approval(approval);
            }
            while let Ok(pending) = turn.questions.try_recv() {
                self.register_pending_question(pending);
            }
            while let Ok(event) = turn.events.try_recv() {
                match event {
                    Ok(event) => self.apply_turn_event(event),
                    Err(error) => {
                        self.push_status(format!("Error: {error}"));
                    }
                }
            }
            // Turn-driver errors are already forwarded through the event channel
            // and rendered into the transcript. Keep the interactive shell alive.
            match turn_result {
                Ok(outcome) => {
                    if let Some(session_id) = outcome.session_id {
                        self.set_active_session_id(session_id);
                    }
                }
                Err(error) => {
                    self.tui
                        .chrome_mut()
                        .apply_stream_update(StreamUpdate::Error {
                            text: error.to_string(),
                        });
                }
            }
            self.refresh_git_status_now();
            self.start_next_queued_after_turn().await?;
        } else {
            self.active_turn = Some(turn);
        }
        Ok(())
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
        Ok(())
    }

    pub(super) fn abort_active_turn(&mut self) {
        if let Some(turn) = self.active_turn.take() {
            turn.cancel_token.cancel();
            turn.task.abort();
        }
        self.pending_approvals.clear();
        self.resolved_approvals.clear();
        self.pending_questions.clear();
        self.pending_question_prompts.clear();
        self.pending_background_question_followups.clear();
        self.clear_interrupted_turn_state();
    }
}
