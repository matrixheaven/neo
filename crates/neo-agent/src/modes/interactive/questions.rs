//! Extracted: pending question registration, resolution, and background
//! question follow-up turn handling.

use anyhow::Result;

use neo_agent_core::{
    AgentEvent, Content, PendingQuestion, QuestionResponse, format_collected_answers,
};

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
            });
        self.pending_questions
            .insert(id.clone(), pending.response_tx);
        self.pending_question_prompts.insert(id, questions);
    }

    /// Resolve a pending question by sending the user's answers through the
    /// stored oneshot channel.
    pub(super) async fn resolve_question(&mut self, id: &str, answers: Vec<String>) -> Result<()> {
        if let Some(questions) = self.pending_question_prompts.remove(id) {
            self.transcript_mut()
                .push_transcript(neo_tui::transcript::TranscriptEntry::status(
                    format_collected_answers(&questions, &answers),
                ));
        }
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
            self.start_turn_with_prompt(vec![Content::text(prompt)], None, false);
            self.drain_active_turn().await?;
        }
        Ok(())
    }
}
