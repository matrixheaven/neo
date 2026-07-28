//! Workflow Operator state: selection, focus, scrolling, and pending request memory.
//!
//! This module owns the TUI-side view state for the /tasks Workflow Operator
//! overlay. It stores selection anchors, pane focus, scroll offsets, and
//! ephemeral dialog state (dismissed request IDs, answer drafts).

use neo_agent_core::workflow::{WorkflowChildKey, WorkflowOperatorSnapshot, WorkflowStepKey};

/// Focus target within the Workflow Operator overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorFocus {
    Steps,
    Agents,
}

impl Default for OperatorFocus {
    fn default() -> Self {
        Self::Steps
    }
}

/// Narrow-screen sequential page for small terminal widths (< 70 columns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NarrowPage {
    Summary,
    Steps,
    Agents,
    AgentDetails,
}

/// Ephemeral per-request dismissal memory so the same
/// dismissed request does not reopen on periodic refresh.
#[derive(Debug, Clone, Default)]
pub struct DismissalMemory {
    pub dismissed_request_id: Option<String>,
}

/// Workflow Operator TUI state, stored alongside the task browser overlay.
#[derive(Debug, Clone)]
pub struct WorkflowOperatorState {
    /// Selected step key (stable keyed selection, not numeric index).
    pub selected_step: Option<WorkflowStepKey>,
    /// Selected child key (stable keyed selection).
    pub selected_child: Option<WorkflowChildKey>,
    /// Current pane focus.
    pub focus: OperatorFocus,
    /// Whether the user has manually moved the step (pins step selection).
    pub manual_step_pin: bool,
    /// Active step is followed automatically until first manual navigation.
    pub follow_active_step: bool,
    /// Scroll offset in the Steps pane.
    pub steps_scroll: usize,
    /// Scroll offset in the Agents pane.
    pub agents_scroll: usize,
    /// Whether agent details overlay is open.
    pub details_open: bool,
    /// Ephemeral dismissal state.
    pub dismissals: DismissalMemory,
    /// Narrow-screen current page.
    pub narrow_page: NarrowPage,
}

impl Default for WorkflowOperatorState {
    fn default() -> Self {
        Self {
            selected_step: None,
            selected_child: None,
            focus: OperatorFocus::default(),
            manual_step_pin: false,
            follow_active_step: true,
            steps_scroll: 0,
            agents_scroll: 0,
            details_open: false,
            dismissals: DismissalMemory::default(),
            narrow_page: NarrowPage::Summary,
        }
    }
}

impl WorkflowOperatorState {
    /// Reset ephemeral state for a new (or reopened) operator instance.
    pub fn reset_for_new_snapshot(&mut self) {
        self.follow_active_step = true;
        self.manual_step_pin = false;
    }

    /// Reset when reopening the operator.
    pub fn on_reopen(&mut self) {
        self.follow_active_step = true;
        self.manual_step_pin = false;
        self.details_open = false;
    }

    /// Dismiss the current request; prevents auto-reopen on refresh.
    pub fn dismiss_request(&mut self, request_id: String) {
        self.dismissals.dismissed_request_id = Some(request_id);
    }

    /// Whether the given request_id is currently dismissed.
    pub fn is_dismissed(&self, request_id: &str) -> bool {
        self.dismissals
            .dismissed_request_id
            .as_deref()
            == Some(request_id)
    }

    /// Clear dismissal if the request changed.
    pub fn clear_dismissal_if_changed(&mut self, current_request_id: Option<&str>) {
        if self
            .dismissals
            .dismissed_request_id
            .as_deref()
            != current_request_id
        {
            self.dismissals.dismissed_request_id = None;
        }
    }
}
