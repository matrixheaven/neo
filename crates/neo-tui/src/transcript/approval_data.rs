use neo_agent_core::{ApprovalRequest, ApprovalResolution};

use crate::transcript::pane::TranscriptPane;
use crate::transcript::{ApprovalDisplayState, ApprovalPromptData, TranscriptEntry};

impl TranscriptPane {
    pub fn select_approval(
        &mut self,
        id: &str,
        selected: usize,
        feedback_input: &str,
        feedback_active: bool,
    ) {
        let changed = self.transcript.mutate_approval(id, |approval| {
            let changed = approval.selected != selected
                || approval.feedback_active != feedback_active
                || approval.feedback_input != feedback_input;
            if !changed {
                return false;
            }
            approval.selected = selected;
            approval.feedback_active = feedback_active;
            feedback_input.clone_into(&mut approval.feedback_input);
            true
        });
        if changed {
            self.mark_dirty();
        }
    }

    pub fn resolve_approval(&mut self, id: &str, resolution: &ApprovalResolution) {
        let changed = self.transcript.mutate_approval(id, |approval| {
            let already_resolved = matches!(
                &approval.state,
                ApprovalDisplayState::Resolved(existing) if existing == resolution
            );
            let feedback_cleared = !approval.feedback_active && approval.feedback_input.is_empty();
            if already_resolved && feedback_cleared {
                return false;
            }
            approval.state = ApprovalDisplayState::Resolved(resolution.clone());
            // Interactive feedback is live-only; historical cards keep the
            // canonical resolution label/action without editor state.
            approval.feedback_active = false;
            approval.feedback_input.clear();
            true
        });
        if changed {
            self.mark_dirty();
        }
    }

    pub fn finalize_pending_approvals(&mut self, resolution: &ApprovalResolution) {
        let mut changed = false;
        for index in 0..self.transcript.entries().len() {
            let is_unresolved = matches!(
                &self.transcript.entries()[index],
                TranscriptEntry::ApprovalPrompt(data) if data.is_pending()
            );
            if !is_unresolved {
                continue;
            }
            changed |= self.transcript.mutate_entry(index, |entry| {
                let TranscriptEntry::ApprovalPrompt(data) = entry else {
                    return false;
                };
                data.state = ApprovalDisplayState::Resolved(resolution.clone());
                true
            });
        }
        if changed {
            self.mark_dirty();
        }
    }

    /// Upsert a canonical approval request exactly as provided. Never appends
    /// session/prefix options or reconstructs labels from raw tool JSON.
    ///
    /// Every approval gets its transcript position on arrival; the
    /// presentation selects the earliest unresolved blocking entry as the
    /// active focus while later entries remain present but deferred.
    pub(super) fn upsert_approval(&mut self, request: ApprovalRequest) {
        let id = request.id.clone();
        if self
            .transcript
            .approval(&id)
            .is_some_and(|approval| !approval.is_pending())
        {
            return;
        }
        if self.transcript.approval(&id).is_some() {
            self.transcript.mutate_approval(&id, |approval| {
                if approval.request == request {
                    return false;
                }
                approval.request = request;
                true
            });
            return;
        }

        let data = ApprovalPromptData {
            request,
            selected: 0,
            feedback_input: String::new(),
            feedback_active: false,
            expanded: self.tool_output_expanded(),
            state: ApprovalDisplayState::Pending,
        };
        self.finish_active_text_blocks();
        self.transcript.insert_approval_after_tool_or_push(data);
    }
}
