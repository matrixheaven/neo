use neo_agent_core::{
    ApprovalAction, ApprovalCancelReason, ApprovalOption, ApprovalRequest, ApprovalResponse,
};

/// Live chrome state for one canonical approval request.
///
/// The request (including its option list and actions) is immutable once stored.
/// Only selection index and feedback editing state are mutable UI fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequestModal {
    pub request: ApprovalRequest,
    pub selected: usize,
    pub feedback_input: String,
    /// Explicit flag separate from "selection is a revise action". Navigation
    /// alone never sets this — only an explicit confirm (Enter / number key)
    /// while a revision action is selected enters feedback editing.
    collecting_feedback: bool,
}

impl ApprovalRequestModal {
    #[must_use]
    pub fn new(request: ApprovalRequest) -> Self {
        Self {
            request,
            selected: 0,
            feedback_input: String::new(),
            collecting_feedback: false,
        }
    }

    #[must_use]
    pub fn selected_option(&self) -> Option<&ApprovalOption> {
        self.request.options.get(self.selected)
    }

    #[must_use]
    pub fn selected_action(&self) -> Option<&ApprovalAction> {
        self.selected_option().map(|option| &option.action)
    }

    #[must_use]
    pub fn is_collecting_feedback(&self) -> bool {
        self.collecting_feedback
    }

    #[must_use]
    pub fn is_revision_selected(&self) -> bool {
        matches!(
            self.selected_action(),
            Some(ApprovalAction::RevisePlan { .. } | ApprovalAction::ReviseGoal { .. })
        )
    }

    pub fn move_up(&mut self) {
        if self.request.options.is_empty() {
            self.selected = 0;
        } else if self.selected == 0 {
            self.selected = self.request.options.len() - 1;
        } else {
            self.selected -= 1;
        }
        // Navigation alone never *enters* editing. While already collecting,
        // retarget or exit the editor so presets cannot stick across options.
        self.sync_feedback_editor_after_selection_change();
    }

    pub fn move_down(&mut self) {
        if self.request.options.is_empty() {
            self.selected = 0;
        } else {
            self.selected = (self.selected + 1) % self.request.options.len();
        }
        self.sync_feedback_editor_after_selection_change();
    }

    /// After selection changes while collecting feedback: re-seed from the new
    /// revise option's `preset_feedback` (or clear if none), or exit the editor
    /// entirely when the new option is not a revise action.
    fn sync_feedback_editor_after_selection_change(&mut self) {
        if !self.collecting_feedback {
            return;
        }
        if let Some(
            ApprovalAction::RevisePlan { preset_feedback }
            | ApprovalAction::ReviseGoal { preset_feedback },
        ) = self.selected_action()
        {
            self.feedback_input = preset_feedback.clone().unwrap_or_default();
        } else {
            self.collecting_feedback = false;
            self.feedback_input.clear();
        }
    }

    /// Activates feedback collection for a selected revision action.
    /// Initializes `feedback_input` from the action's `preset_feedback` when present.
    /// Returns `false` if the current selection is not a revision action.
    pub fn begin_feedback_collection(&mut self) -> bool {
        let preset = match self.selected_action() {
            Some(
                ApprovalAction::RevisePlan { preset_feedback }
                | ApprovalAction::ReviseGoal { preset_feedback },
            ) => preset_feedback.clone(),
            _ => return false,
        };
        self.collecting_feedback = true;
        if self.feedback_input.is_empty()
            && let Some(preset) = preset
        {
            self.feedback_input = preset;
        }
        true
    }

    pub fn insert_feedback(&mut self, text: &str) {
        if self.is_collecting_feedback() {
            self.feedback_input.push_str(text);
        }
    }

    pub fn backspace_feedback(&mut self) {
        if self.is_collecting_feedback() {
            self.feedback_input.pop();
        }
    }

    /// Build a response for the currently selected option, or `None` when a
    /// revision action still needs non-empty feedback.
    #[must_use]
    pub fn response_for_selected(&self) -> Option<ApprovalResponse> {
        response_for_selected(self)
    }

    #[must_use]
    pub fn cancelled(&self, reason: ApprovalCancelReason) -> ApprovalResponse {
        ApprovalResponse::Cancelled {
            request_id: self.request.id.clone(),
            reason,
        }
    }
}

#[must_use]
pub(super) fn response_for_selected(modal: &ApprovalRequestModal) -> Option<ApprovalResponse> {
    let action = modal.selected_action()?.clone();
    let revises = matches!(
        action,
        ApprovalAction::RevisePlan { .. } | ApprovalAction::ReviseGoal { .. }
    );
    if revises {
        let feedback = modal.feedback_input.trim();
        if !modal.collecting_feedback || feedback.is_empty() {
            return None;
        }
        return Some(ApprovalResponse::Selected {
            request_id: modal.request.id.clone(),
            action,
            feedback: Some(feedback.to_owned()),
        });
    }
    Some(ApprovalResponse::Selected {
        request_id: modal.request.id.clone(),
        action,
        feedback: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_agent_core::{
        ApprovalAction, ApprovalOption, ApprovalPresentation, PermissionOperation,
    };

    fn revise_request() -> ApprovalRequest {
        ApprovalRequest {
            turn: 1,
            id: "plan-1".to_owned(),
            operation: PermissionOperation::PlanTransition,
            presentation: ApprovalPresentation::Plan {
                title: "Plan Review".to_owned(),
                path: None,
                markdown: "Ready?".to_owned(),
                summary: Some("Ready?".to_owned()),
            },
            options: vec![
                ApprovalOption {
                    label: "Approve".to_owned(),
                    description: None,
                    action: ApprovalAction::ApprovePlan { selection: None },
                },
                ApprovalOption {
                    label: "Suggestion: Keep 85%".to_owned(),
                    description: Some("Keep compaction at 85%.".to_owned()),
                    action: ApprovalAction::RevisePlan {
                        preset_feedback: Some("Keep compaction at 85%.".to_owned()),
                    },
                },
                ApprovalOption {
                    label: "Reject".to_owned(),
                    description: None,
                    action: ApprovalAction::RejectPlan,
                },
            ],
        }
    }

    #[test]
    fn navigation_does_not_enter_feedback_editing() {
        let mut modal = ApprovalRequestModal::new(revise_request());
        modal.move_down();
        assert!(modal.is_revision_selected());
        assert!(!modal.is_collecting_feedback());
        assert!(modal.feedback_input.is_empty());
    }

    #[test]
    fn confirming_revision_loads_preset_feedback() {
        let mut modal = ApprovalRequestModal::new(revise_request());
        modal.selected = 1;
        assert!(modal.begin_feedback_collection());
        assert!(modal.is_collecting_feedback());
        assert_eq!(modal.feedback_input, "Keep compaction at 85%.");
    }

    #[test]
    fn navigation_while_collecting_reseeds_or_exits_editor() {
        let mut modal = ApprovalRequestModal::new(ApprovalRequest {
            turn: 1,
            id: "plan-2".to_owned(),
            operation: PermissionOperation::PlanTransition,
            presentation: ApprovalPresentation::Plan {
                title: "Plan Review".to_owned(),
                path: None,
                markdown: "Ready?".to_owned(),
                summary: Some("Ready?".to_owned()),
            },
            options: vec![
                ApprovalOption {
                    label: "Suggestion A".to_owned(),
                    description: None,
                    action: ApprovalAction::RevisePlan {
                        preset_feedback: Some("A".to_owned()),
                    },
                },
                ApprovalOption {
                    label: "Suggestion B".to_owned(),
                    description: None,
                    action: ApprovalAction::RevisePlan {
                        preset_feedback: Some("B".to_owned()),
                    },
                },
                ApprovalOption {
                    label: "Reject".to_owned(),
                    description: None,
                    action: ApprovalAction::RejectPlan,
                },
            ],
        });
        assert!(modal.begin_feedback_collection());
        modal.feedback_input.push_str(" typed");
        modal.move_down();
        assert!(modal.is_collecting_feedback());
        assert_eq!(modal.feedback_input, "B");
        modal.move_down();
        assert!(!modal.is_collecting_feedback());
        assert!(modal.feedback_input.is_empty());
    }

    #[test]
    fn response_clones_selected_action() {
        let mut modal = ApprovalRequestModal::new(revise_request());
        let response = modal.response_for_selected().expect("approve");
        assert!(matches!(
            response,
            ApprovalResponse::Selected {
                action: ApprovalAction::ApprovePlan { selection: None },
                feedback: None,
                ..
            }
        ));
        modal.selected = 1;
        assert!(modal.response_for_selected().is_none());
        assert!(modal.begin_feedback_collection());
        modal.feedback_input.push_str(" please");
        let response = modal.response_for_selected().expect("revise");
        assert!(matches!(
            response,
            ApprovalResponse::Selected {
                action: ApprovalAction::RevisePlan { .. },
                feedback: Some(ref text),
                ..
            } if text == "Keep compaction at 85%. please"
        ));
    }
}
