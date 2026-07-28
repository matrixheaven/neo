//! Workflow Operator render logic.
//!
//! Renders the Steps / Agents / Details layout for wide, stacked, and
//! sequential terminal sizes.

use neo_agent_core::workflow::{StepRowState, WorkflowOperatorSnapshot};

/// Terminal width threshold for the wide layout (>= 100 columns).
pub const WIDE_THRESHOLD: u16 = 100;
/// Terminal width threshold for stacked layout (70-99 columns).
pub const STACKED_THRESHOLD: u16 = 70;
/// Minimum terminal height for the operator overlay.
pub const MIN_HEIGHT: u16 = 12;

/// Layout kind determined by terminal dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorLayout {
    Wide,
    Stacked,
    Sequential,
}

impl OperatorLayout {
    /// Determine layout from terminal dimensions.
    #[must_use]
    pub fn from_dimensions(cols: u16, _rows: u16) -> Self {
        if cols >= WIDE_THRESHOLD {
            Self::Wide
        } else if cols >= STACKED_THRESHOLD {
            Self::Stacked
        } else {
            Self::Sequential
        }
    }
}

/// Render a compact status marker for a workflow step state.
#[must_use]
pub fn step_status_marker(state: StepRowState) -> &'static str {
    match state {
        StepRowState::Completed => " ✓",
        StepRowState::Active => " ●",
        StepRowState::Failed => " ✗",
        StepRowState::Pending => "  ",
        StepRowState::Paused => " ⏸",
    }
}

/// Render a needs-input banner text.
#[must_use]
pub fn needs_input_banner(snapshot: &WorkflowOperatorSnapshot) -> Option<String> {
    snapshot.pending_user.as_ref().map(|pending| {
        format!(
            "Needs your input: {}  Enter answer",
            pending.prompt.chars().take(50).collect::<String>()
        )
    })
}
