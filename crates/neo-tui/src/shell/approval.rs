use crate::primitive::theme::TuiTheme;
use neo_agent_core::PlanSuggestion;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalChoice {
    Approve,
    Deny,
    AlwaysApprove,
    /// Revise — like Deny but the user provides feedback that gets sent to the model.
    /// Used for `ExitPlanMode` plan review.
    Revise,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalOption {
    pub choice: ApprovalChoice,
    pub label: String,
}

impl ApprovalOption {
    #[must_use]
    pub fn new(choice: ApprovalChoice, label: impl Into<String>) -> Self {
        Self {
            choice,
            label: label.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalModal {
    pub title: String,
    pub body: String,
    pub options: Vec<ApprovalOption>,
    pub selected: usize,
    pub theme: TuiTheme,
}

impl ApprovalModal {
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        body: impl Into<String>,
        options: impl IntoIterator<Item = ApprovalOption>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            options: options.into_iter().collect(),
            selected: 0,
            theme: TuiTheme::default(),
        }
    }

    #[must_use]
    pub const fn with_theme(mut self, theme: TuiTheme) -> Self {
        self.theme = theme;
        self
    }

    #[must_use]
    pub fn selected_choice(&self) -> Option<ApprovalChoice> {
        self.options.get(self.selected).map(|option| option.choice)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequestModal {
    pub request_id: String,
    pub modal: ApprovalModal,
    pub feedback_input: String,
    /// Explicit flag separate from `selected_choice == Revise`. Without this,
    /// navigating to "Reject with feedback" would immediately enter feedback
    /// input mode — a misclick or arrow-over would show the input box before
    /// the user intended to revise. The flag is only set after the user
    /// explicitly confirms (Enter or number key) while Revise is selected.
    collecting_feedback: bool,
    /// Model-supplied plan-review option labels, in the order they were rendered
    /// as the leading approve choices. Empty for non-plan-review approvals.
    /// `confirm_approval` reads the entry at the selected index to populate
    /// `ApprovalResult.selected_option_label`.
    pub plan_option_labels: Vec<String>,
    /// Preset revision suggestions for plan review. Each suggestion is rendered
    /// as an option immediately after the plan options and before Reject /
    /// Revise. Selecting a suggestion fills [`Self::feedback_input`] with its
    /// feedback text and behaves like [`ApprovalChoice::Revise`].
    pub suggestions: Vec<neo_agent_core::PlanSuggestion>,
}

impl ApprovalRequestModal {
    #[must_use]
    pub fn new(
        request_id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self::new_with_options(request_id, title, body, None, None)
    }

    /// Build a tool approval modal with dynamic session/prefix options.
    ///
    /// - `session_option_label`: when `Some`, the second option uses that label
    ///   (e.g. "Approve this exact command for this session"). When `None`, the
    ///   session-approval option is omitted.
    /// - `prefix_option_label`: when `Some`, a persistent prefix option is added
    ///   (Layer 2), e.g. "Approve commands starting with git". Also uses
    ///   `AlwaysApprove`; the runtime distinguishes the two by whether a
    ///   `prefix_rule` is attached to the request.
    #[must_use]
    pub fn new_with_options(
        request_id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
        session_option_label: Option<String>,
        prefix_option_label: Option<String>,
    ) -> Self {
        let mut options = vec![ApprovalOption::new(ApprovalChoice::Approve, "Approve once")];
        if let Some(label) = session_option_label {
            options.push(ApprovalOption::new(ApprovalChoice::AlwaysApprove, label));
        }
        if let Some(label) = prefix_option_label {
            options.push(ApprovalOption::new(ApprovalChoice::AlwaysApprove, label));
        }
        options.push(ApprovalOption::new(ApprovalChoice::Deny, "Reject"));
        options.push(ApprovalOption::new(
            ApprovalChoice::Revise,
            "Reject with feedback",
        ));
        Self {
            request_id: request_id.into(),
            feedback_input: String::new(),
            collecting_feedback: false,
            plan_option_labels: Vec::new(),
            suggestions: Vec::new(),
            modal: ApprovalModal::new(title, body, options),
        }
    }

    /// Create a review approval modal with Approve / Reject / Revise options.
    #[must_use]
    pub fn new_review(
        request_id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            feedback_input: String::new(),
            collecting_feedback: false,
            plan_option_labels: Vec::new(),
            suggestions: Vec::new(),
            modal: ApprovalModal::new(
                title,
                body,
                [
                    ApprovalOption::new(ApprovalChoice::Approve, "Approve"),
                    ApprovalOption::new(ApprovalChoice::Deny, "Reject"),
                    ApprovalOption::new(ApprovalChoice::Revise, "Reject with feedback"),
                ],
            ),
        }
    }

    /// Create a plan-review modal that renders the model-supplied options as
    /// leading approve choices (one per label), followed by preset revision
    /// suggestions, Reject and Revise.
    /// Mirrors kimi-code's plan-review picker. When `plan_option_labels` is
    /// empty, falls back to a single generic Approve choice (same as
    /// [`Self::new_review`]) so a plan with no alternatives still reviews.
    /// Each model option is an `ApprovalChoice::Approve`; the selected label is
    /// recovered by index in [`Self::confirm_approval`]-equivalent handling.
    #[must_use]
    pub fn new_plan_review(
        request_id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
        plan_option_labels: Vec<String>,
        suggestions: Vec<PlanSuggestion>,
    ) -> Self {
        let mut options: Vec<ApprovalOption> = plan_option_labels
            .iter()
            .map(|label| ApprovalOption::new(ApprovalChoice::Approve, format!("Approach: {label}")))
            .collect();
        let plan_option_labels = if plan_option_labels.is_empty() {
            options.push(ApprovalOption::new(ApprovalChoice::Approve, "Approve"));
            vec!["Approve".to_owned()]
        } else {
            plan_option_labels
        };
        for suggestion in &suggestions {
            options.push(ApprovalOption::new(
                ApprovalChoice::Revise,
                format!("Suggestion: {}", suggestion.label),
            ));
        }
        options.push(ApprovalOption::new(ApprovalChoice::Deny, "Reject"));
        options.push(ApprovalOption::new(
            ApprovalChoice::Revise,
            "Reject with feedback",
        ));
        Self {
            request_id: request_id.into(),
            feedback_input: String::new(),
            collecting_feedback: false,
            plan_option_labels,
            suggestions,
            modal: ApprovalModal::new(title, body, options),
        }
    }

    pub fn move_up(&mut self) {
        if self.modal.options.is_empty() {
            self.modal.selected = 0;
        } else if self.modal.selected == 0 {
            self.modal.selected = self.modal.options.len() - 1;
        } else {
            self.modal.selected -= 1;
        }
        self.apply_suggestion_feedback();
    }

    pub fn move_down(&mut self) {
        if self.modal.options.is_empty() {
            self.modal.selected = 0;
        } else {
            self.modal.selected = (self.modal.selected + 1) % self.modal.options.len();
        }
        self.apply_suggestion_feedback();
    }

    /// If the current option selection lands on a preset suggestion, populate
    /// the feedback input with that suggestion's feedback text. If it lands on
    /// a non-revise option, clear any feedback text so the user doesn't submit
    /// stale suggestion text by accident.
    pub(super) fn apply_suggestion_feedback(&mut self) {
        if let Some(suggestion) = self.selected_suggestion() {
            self.feedback_input = suggestion
                .feedback
                .clone()
                .unwrap_or_else(|| suggestion.description.clone());
            self.collecting_feedback = true;
            return;
        }
        self.collecting_feedback = false;
        self.feedback_input.clear();
    }

    /// Returns the preset suggestion currently selected, if any.
    fn selected_suggestion(&self) -> Option<&PlanSuggestion> {
        let plan_option_count = self.plan_option_labels.len();
        let suggestion_index = self.modal.selected.saturating_sub(plan_option_count);
        self.suggestions.get(suggestion_index)
    }

    #[must_use]
    pub fn is_collecting_feedback(&self) -> bool {
        self.collecting_feedback
    }

    /// Activates feedback collection mode. Called only when the user explicitly
    /// confirms (Enter / number key) while "Reject with feedback" is selected.
    /// Returns `false` (no-op) if the current selection isn't Revise, so callers
    /// can use the return value to decide whether to stay in the dialog or
    /// proceed to `confirm_approval`.
    pub fn begin_feedback_collection(&mut self) -> bool {
        if self.modal.selected_choice() != Some(ApprovalChoice::Revise) {
            return false;
        }
        self.collecting_feedback = true;
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalResult {
    pub request_id: String,
    pub choice: ApprovalChoice,
    /// Feedback text when the user picks Revise (`ExitPlanMode` plan review).
    pub feedback: Option<String>,
    /// True when the user picked the persistent prefix-approval option (Layer 2).
    /// Disambiguates from the session-approval option since both are
    /// `ApprovalChoice::AlwaysApprove`.
    pub picked_prefix: bool,
    /// When the user approved a specific model-supplied plan-review option,
    /// this carries that option's label so the runtime can tell the model to
    /// execute only the selected approach. `None` for non-plan-review approvals
    /// and for generic Approve/Reject/Revise choices.
    pub selected_option_label: Option<String>,
}

pub(super) fn approval_number(character: char) -> Option<usize> {
    match character {
        '1' => Some(1),
        '2' => Some(2),
        '3' => Some(3),
        '4' => Some(4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_review_renders_suggestion_options() {
        let suggestions = vec![PlanSuggestion {
            label: "Keep 85% window".to_owned(),
            description: "Keep compaction window at 85%.".to_owned(),
            feedback: Some("Keep compaction at 85%.".to_owned()),
        }];
        let modal = ApprovalRequestModal::new_plan_review(
            "id",
            "Plan Review",
            "Ready?",
            Vec::new(),
            suggestions,
        );
        let labels: Vec<_> = modal
            .modal
            .options
            .iter()
            .map(|opt| opt.label.clone())
            .collect();
        assert_eq!(
            labels,
            vec![
                "Approve",
                "Suggestion: Keep 85% window",
                "Reject",
                "Reject with feedback",
            ]
        );
    }

    #[test]
    fn selecting_suggestion_populates_feedback() {
        let suggestions = vec![PlanSuggestion {
            label: "Keep 85% window".to_owned(),
            description: "Keep compaction window at 85%.".to_owned(),
            feedback: Some("Keep compaction at 85%.".to_owned()),
        }];
        let mut modal = ApprovalRequestModal::new_plan_review(
            "id",
            "Plan Review",
            "Ready?",
            Vec::new(),
            suggestions,
        );
        // With one plan option (Approve) and one suggestion, the suggestion is at index 1.
        modal.modal.selected = 1;
        modal.apply_suggestion_feedback();
        assert_eq!(modal.feedback_input, "Keep compaction at 85%.");
        assert!(modal.is_collecting_feedback());
    }
}
