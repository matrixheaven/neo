mod approval;
mod command_palette;
mod context;
mod dialog_dispatch;
mod dialog_factory;
mod event_router;
mod image_cache;
mod input_dispatch;
mod overlay;
mod pending_input;
mod pickers;
mod prompt;
mod select_list;
mod session_picker;
mod state;
mod stream;

pub use crate::primitive::theme::{ChromeMode, DevelopmentMode, GoalModeStatus, TuiTheme};

pub use approval::{
    ApprovalChoice, ApprovalModal, ApprovalOption, ApprovalRequestModal, ApprovalResult,
};
pub use command_palette::{CommandPaletteState, CommandSpec};
pub use context::{ContextWindow, MainAgentTokenUsage};
pub use image_cache::InlineImageRenderCache;
pub use overlay::{Overlay, OverlayId, OverlayKind};
pub use pending_input::PendingInputState;
pub use pickers::{
    ModelPickerState, PickerItem, PickerState, PromptCompletionPrefix, PromptCompletionState,
};
pub use prompt::{PromptEdit, PromptState};
pub use select_list::{SelectItem, SelectListState, VisibleSelectItem};
pub use session_picker::{SessionPickerItem, SessionPickerScope, SessionPickerState};
pub use state::NeoChromeState;
pub use stream::{StreamUpdate, ToolStatusKind};

use crate::dialogs::{QuestionDisplayData, QuestionStateMachine};
use crate::tasks_browser::TaskBrowserState;

/// Maximum number of visible content lines in the composer input box.
pub(crate) const MAX_PROMPT_VISIBLE_LINES: usize = 8;

impl NeoChromeState {
    pub fn push_overlay(&mut self, mut overlay: Overlay) -> OverlayId {
        self.next_overlay_id = self.next_overlay_id.next();
        overlay.id = self.next_overlay_id;
        let id = overlay.id;
        self.overlays.push(overlay);
        self.focused_overlay = Some(id);
        self.mode = self.overlay_mode();
        id
    }

    /// Search `self.overlays` for an existing overlay whose kind matches `predicate`.
    /// If found, returns its `OverlayId`; otherwise `None`.
    pub fn find_overlay_by_kind(
        &self,
        predicate: impl Fn(&OverlayKind) -> bool,
    ) -> Option<OverlayId> {
        self.overlays
            .iter()
            .find(|o| predicate(&o.kind))
            .map(|o| o.id)
    }

    pub fn focus_overlay(&mut self, id: OverlayId) -> bool {
        if self.overlays.iter().any(|overlay| overlay.id == id) {
            self.focused_overlay = Some(id);
            self.mode = self.overlay_mode();
            true
        } else {
            false
        }
    }

    pub fn close_overlay(&mut self, id: OverlayId) -> Option<Overlay> {
        let index = self.overlays.iter().position(|overlay| overlay.id == id)?;
        let overlay = self.overlays.remove(index);
        if self.focused_overlay == Some(id) {
            self.focused_overlay = self.overlays.last().map(|overlay| overlay.id);
        }
        self.mode = self.overlay_mode();
        Some(overlay)
    }

    pub fn close_focused_overlay(&mut self) -> Option<Overlay> {
        self.focused_overlay.and_then(|id| self.close_overlay(id))
    }

    pub fn close_question_overlay(&mut self, question_id: &str) -> Option<Overlay> {
        let id = self.overlays.iter().find_map(|overlay| {
            let OverlayKind::QuestionDialog(state) = &overlay.kind else {
                return None;
            };
            (state.id == question_id).then_some(overlay.id)
        })?;
        self.close_overlay(id)
    }

    pub fn clear_interrupted_turn_state(&mut self) -> Vec<String> {
        let mut question_ids = Vec::new();
        self.overlays.retain(|overlay| {
            let OverlayKind::QuestionDialog(state) = &overlay.kind else {
                return true;
            };
            question_ids.push(state.id.clone());
            false
        });
        if self
            .focused_overlay
            .is_some_and(|id| !self.overlays.iter().any(|overlay| overlay.id == id))
        {
            self.focused_overlay = self.overlays.last().map(|overlay| overlay.id);
        }
        self.mode = self.overlay_mode();
        question_ids
    }

    pub fn request_approval(
        &mut self,
        request_id: impl Into<String>,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> OverlayId {
        self.pending_approvals
            .push_back(ApprovalRequestModal::new(request_id, title, body));
        self.focused_overlay = None;
        self.mode = ChromeMode::Approval;
        OverlayId::default()
    }

    #[must_use]
    pub fn approval_choice(&self) -> Option<ApprovalChoice> {
        if let Some(approval) = self.pending_approvals.front() {
            return approval.modal.selected_choice();
        }
        let OverlayKind::Approval(modal) = &self.focused_overlay()?.kind else {
            return None;
        };
        modal.modal.selected_choice()
    }

    #[must_use]
    pub fn approval_is_pending(&self) -> bool {
        !self.pending_approvals.is_empty()
    }

    #[must_use]
    pub fn approval_selection(&self) -> Option<(&str, usize, &str)> {
        self.pending_approvals.front().map(|approval| {
            (
                approval.request_id.as_str(),
                approval.modal.selected,
                approval.feedback_input.as_str(),
            )
        })
    }

    pub fn choose_approval_number(&mut self, number: usize) -> Option<ApprovalResult> {
        let approval = self.pending_approvals.front_mut()?;
        if number == 0 || number > approval.modal.options.len() {
            return None;
        }
        approval.modal.selected = number - 1;
        if approval.is_collecting_feedback() {
            return None;
        }
        self.confirm_approval()
    }

    pub fn deny_approval(&mut self) -> Option<ApprovalResult> {
        if let Some(approval) = self.pending_approvals.front_mut() {
            if let Some(index) = approval
                .modal
                .options
                .iter()
                .position(|option| option.choice == ApprovalChoice::Deny)
            {
                approval.modal.selected = index;
            }
            return self.confirm_approval();
        }

        let id = self.focused_overlay;
        let overlay = self.focused_overlay()?;
        let OverlayKind::Approval(modal) = &overlay.kind else {
            return None;
        };
        let result = ApprovalResult {
            request_id: modal.request_id.clone(),
            choice: ApprovalChoice::Deny,
            feedback: None,
            picked_prefix: false,
            selected_option_label: None,
        };
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(result)
    }

    pub fn cancel_all_approvals(&mut self) -> Vec<ApprovalResult> {
        let results = self
            .pending_approvals
            .drain(..)
            .map(|modal| ApprovalResult {
                request_id: modal.request_id,
                choice: ApprovalChoice::Deny,
                feedback: None,
                picked_prefix: false,
                selected_option_label: None,
            })
            .collect();
        self.mode = self.overlay_mode();
        results
    }

    pub fn confirm_approval(&mut self) -> Option<ApprovalResult> {
        if let Some(modal) = self.pending_approvals.pop_front() {
            let selected = modal.modal.selected;
            let selected_label = modal
                .modal
                .options
                .get(selected)
                .map(|opt| opt.label.clone());
            let choice = modal.modal.selected_choice()?;
            // The prefix option (Layer 2) is rendered as
            // "Approve commands starting with …" and uses AlwaysApprove. Detect
            // it by label so the controller persists a prefix rule instead of a
            // session key.
            let picked_prefix = choice == ApprovalChoice::AlwaysApprove
                && selected_label
                    .as_deref()
                    .is_some_and(|label| label.starts_with("Approve commands starting with"));
            // Plan-review approve choices occupy the leading indices, one per
            // model-supplied label. Recover the chosen approach label only when
            // the user actually picked one of those entries.
            let selected_option_label = (choice == ApprovalChoice::Approve
                && selected < modal.plan_option_labels.len())
            .then(|| modal.plan_option_labels[selected].clone());
            let result = ApprovalResult {
                request_id: modal.request_id,
                choice,
                feedback: (choice == ApprovalChoice::Revise)
                    .then_some(modal.feedback_input)
                    .filter(|feedback| !feedback.is_empty()),
                picked_prefix,
                selected_option_label,
            };
            self.mode = self.overlay_mode();
            return Some(result);
        }

        let id = self.focused_overlay;
        let overlay = self.focused_overlay()?;
        let OverlayKind::Approval(modal) = &overlay.kind else {
            return None;
        };
        let result = ApprovalResult {
            request_id: modal.request_id.clone(),
            choice: modal.modal.selected_choice()?,
            feedback: None,
            picked_prefix: false,
            selected_option_label: None,
        };
        if let Some(id) = id {
            let _ = self.close_overlay(id);
        }
        Some(result)
    }

    // -- Question dialog overlay ---------------------------------------------

    pub fn push_question_overlay(
        &mut self,
        id: impl Into<String>,
        questions: Vec<QuestionDisplayData>,
    ) -> OverlayId {
        let state = QuestionStateMachine::new(id, questions);
        self.push_overlay(Overlay::new(
            "questions",
            OverlayKind::QuestionDialog(state),
        ))
    }

    #[must_use]
    pub fn question_dialog_state(&self) -> Option<&QuestionStateMachine> {
        let overlay = self.focused_overlay()?;
        match &overlay.kind {
            OverlayKind::QuestionDialog(state) => Some(state),
            _ => None,
        }
    }

    #[must_use]
    pub fn question_dialog_is_focused(&self) -> bool {
        self.focused_overlay()
            .is_some_and(|o| matches!(o.kind, OverlayKind::QuestionDialog(_)))
    }

    fn overlay_mode(&self) -> ChromeMode {
        if !self.pending_approvals.is_empty() {
            return ChromeMode::Approval;
        }
        if let Some(overlay) = self.focused_overlay() {
            if matches!(
                overlay.kind,
                OverlayKind::Approval(_) | OverlayKind::QuestionDialog(_)
            ) {
                ChromeMode::Approval
            } else {
                ChromeMode::Overlay
            }
        } else {
            ChromeMode::Editing
        }
    }

    #[must_use]
    pub fn focused_overlay_blocks_prompt(&self) -> bool {
        if !self.pending_approvals.is_empty() {
            return true;
        }
        let Some(overlay) = self.focused_overlay() else {
            return false;
        };
        matches!(
            overlay.kind,
            OverlayKind::SessionPicker(_)
                | OverlayKind::ModelPicker(_)
                | OverlayKind::ModelSelector(_)
                | OverlayKind::TabbedModelSelector(_)
                | OverlayKind::ProviderManager(_)
                | OverlayKind::McpManager(_)
                | OverlayKind::McpAddForm(_)
                | OverlayKind::ChoicePicker(_)
                | OverlayKind::ApiKeyInput(_)
                | OverlayKind::TextInput(_)
                | OverlayKind::CustomRegistryImport(_)
                | OverlayKind::QuestionDialog(_)
                | OverlayKind::Approval(_)
                | OverlayKind::TrustDialog(_)
                | OverlayKind::HelpPanel(_)
                | OverlayKind::TaskBrowser(_)
        )
    }

    #[must_use]
    pub fn task_browser_state(&self) -> Option<&TaskBrowserState> {
        let OverlayKind::TaskBrowser(state) = &self.focused_overlay()?.kind else {
            return None;
        };
        Some(state)
    }

    pub fn task_browser_state_mut(&mut self) -> Option<&mut TaskBrowserState> {
        let id = self.focused_overlay?;
        let overlay = self.overlays.iter_mut().find(|overlay| overlay.id == id)?;
        let OverlayKind::TaskBrowser(state) = &mut overlay.kind else {
            return None;
        };
        Some(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backspace_selects_marker_first_then_deletes() {
        let mut prompt = PromptState::new("hello [paste #1 +5 lines]");
        prompt.cursor = prompt.char_len();
        // First backspace selects marker, text unchanged.
        assert!(prompt.apply_edit(PromptEdit::Backspace).is_none());
        assert!(prompt.text.contains("[paste #1 +5 lines]"));
        assert!(prompt.selected_marker().is_some());
        // Second backspace deletes marker.
        assert!(prompt.apply_edit(PromptEdit::Backspace).is_some());
        assert!(!prompt.text.contains("[paste #1 +5 lines]"));
        assert_eq!(prompt.text, "hello ");
    }

    #[test]
    fn delete_selects_marker_first_then_deletes() {
        let mut prompt = PromptState::new("[paste #1 +5 lines] world");
        prompt.cursor = 0;
        assert!(prompt.apply_edit(PromptEdit::Delete).is_none());
        assert!(prompt.text.contains("[paste #1 +5 lines]"));
        assert!(prompt.apply_edit(PromptEdit::Delete).is_some());
        assert!(!prompt.text.contains("[paste #1 +5 lines]"));
        assert_eq!(prompt.text, " world");
    }

    #[test]
    fn cursor_movement_clears_marker_selection() {
        let mut prompt = PromptState::new("hello [paste #1 +5 lines]");
        prompt.cursor = prompt.char_len();
        prompt.apply_edit(PromptEdit::Backspace);
        assert!(prompt.selected_marker().is_some());
        prompt.apply_edit(PromptEdit::MoveLeft);
        assert!(prompt.selected_marker().is_none());
    }

    #[test]
    fn normal_backspace_still_deletes_single_character() {
        let mut prompt = PromptState::new("hello");
        prompt.cursor = prompt.char_len();
        prompt.apply_edit(PromptEdit::Backspace);
        assert_eq!(prompt.text, "hell");
    }

    #[test]
    fn mcp_add_form_overlay_opens_and_has_no_result_initially() {
        let mut chrome = NeoChromeState::new("title", "session", "model", "/tmp");
        let opts = crate::dialogs::McpAddFormOptions {
            title: "Add MCP Server".into(),
            transport: "stdio".into(),
        };
        let id = chrome.open_mcp_add_form(opts);

        assert_eq!(chrome.focused_overlay_id(), Some(id));
        assert!(chrome.focused_overlay_is_rich_dialog());
        assert!(chrome.focused_overlay_blocks_prompt());
        assert!(chrome.mcp_add_form_result().is_none());

        let lines = chrome.focused_overlay_lines(80);
        assert!(!lines.is_empty());
        // The dialog reserves 16 rows so the larger http/sse form fits; the
        // actual rendered lines may be fewer (e.g. 11 for stdio).
        assert_eq!(chrome.focused_overlay_height(), 16);
    }

    #[test]
    fn help_panel_overlay_opens_as_rich_dialog_and_blocks_prompt() {
        let mut chrome = NeoChromeState::new("title", "session", "model", "/tmp");
        let id = chrome.open_help_panel(vec![
            crate::dialogs::HelpPanelCommand::new("/help", Some("Show help information")),
            crate::dialogs::HelpPanelCommand::new("/skill:refactor", Some("Refactor safely")),
        ]);

        assert_eq!(chrome.focused_overlay_id(), Some(id));
        assert!(chrome.focused_overlay_is_rich_dialog());
        assert!(chrome.focused_overlay_blocks_prompt());
        assert_eq!(chrome.focused_overlay_height(), 16);

        let visible = chrome
            .focused_overlay_lines(80)
            .into_iter()
            .map(|line| crate::primitive::strip_ansi(&line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            visible.contains("help · Esc / Enter / q close"),
            "{visible}"
        );
        assert!(visible.contains("/help"), "{visible}");
        assert!(visible.contains("/skill:refactor"), "{visible}");

        assert_eq!(
            chrome.handle_focused_dialog_input(crate::input::InputEvent::Insert('q')),
            crate::primitive::InputResult::Cancelled
        );
        assert!(chrome.focused_overlay().is_none());
    }
}
