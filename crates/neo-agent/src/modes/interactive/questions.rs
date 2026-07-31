//! Extracted: pending question registration, resolution, and background
//! question follow-up turn handling.

use anyhow::Result;

use neo_agent_core::{AgentEvent, Content, MessageOrigin, PendingQuestion, QuestionResponse};
use neo_tui::dialogs::{QuestionDisplayData, QuestionDisplayOption};

use super::InteractiveController;

/// Build the follow-up prompt text for a resolved background question.
pub(super) fn background_question_followup_prompt(task_id: &str) -> String {
    format!(
        "Background question `{task_id}` has been answered. Use TaskOutput with task_id `{task_id}` to read the answer, then continue the current work."
    )
}

impl InteractiveController {
    pub(super) fn register_pending_question(&mut self, pending: PendingQuestion) {
        let id = pending.id.clone();
        let questions = pending.questions.clone();
        // Synthesize a QuestionRequested event for the TUI to display the dialog.
        // The TUI's apply_agent_event will push a question overlay (implemented by
        // the TUI subagent).
        self.tui
            .chrome_mut()
            .apply_agent_event(AgentEvent::QuestionRequested {
                turn: 0,
                id: id.clone(),
                questions: questions.clone(),
                workflow_origin: pending.workflow_origin.clone(),
            });
        let display = questions
            .iter()
            .map(|question| QuestionDisplayData {
                question: question.question.clone(),
                header: question.header.clone(),
                body: question.body.clone(),
                options: question
                    .options
                    .iter()
                    .map(|option| QuestionDisplayOption {
                        label: option.label.clone(),
                        description: option.description.clone(),
                    })
                    .collect(),
                multi_select: question.multi_select,
            })
            .collect();
        // The transcript card is the single visible owner of the question;
        // the chrome overlay keeps the runtime selection state.
        self.tui
            .transcript_mut()
            .upsert_question_prompt(&id, display);
        self.pending_questions
            .insert(id.clone(), pending.response_tx);
        self.pending_question_prompts.insert(id, questions);
    }

    /// Resolve a pending question by sending the user's answers through the
    /// stored oneshot channel and updating the transcript card in place.
    pub(super) async fn resolve_question(&mut self, id: &str, answers: Vec<String>) -> Result<()> {
        self.pending_question_prompts.remove(id);
        self.tui
            .transcript_mut()
            .resolve_question_prompt(id, answers.clone());
        if let Some(tx) = self.pending_questions.remove(id) {
            let _ = tx.send(QuestionResponse { answers });
        }
        if id.starts_with("question-") {
            self.pending_background_question_followups
                .push_back(background_question_followup_prompt(id));
            self.start_pending_background_question_followups().await?;
        }
        Ok(())
    }

    pub(super) async fn start_pending_background_question_followups(&mut self) -> Result<()> {
        while self.active_turn.is_none() {
            let Some(prompt) = self.pending_background_question_followups.pop_front() else {
                break;
            };
            self.start_turn_with_prompt_origin(
                vec![Content::text(prompt)],
                MessageOrigin::injection("background_question"),
            );
            self.drain_active_turn().await?;
        }
        Ok(())
    }
}
