use neo_agent_core::PermissionOperation;

use crate::primitive::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuiTheme {
    pub background: Color,
    pub surface: Color,
    pub surface_border: Color,
    pub brand: Color,
    pub status_ok: Color,
    pub status_error: Color,
    pub status_warn: Color,
    pub text_muted: Color,
    pub text_primary: Color,
    pub prompt: Color,
    pub composer_bg: Color,
    pub user_message: Color,
    pub user_bg: Color,
    pub diff_added: Color,
    pub diff_removed: Color,
    pub diff_hunk: Color,
    pub diff_context: Color,
    pub selection_bg: Color,
    pub status_pending: Color,
    pub status_cancelled: Color,
    pub approval_bg: Color,
    pub approval_border: Color,
    pub approval_title: Color,
    pub selected_fg: Color,
    pub selected_bg: Color,
    pub overlay_border: Color,
    pub footer_permission_allow: Color,
    pub footer_permission_ask: Color,
    pub footer_permission_deny: Color,
    pub footer_working: Color,
    pub footer_context_ok: Color,
    pub footer_context_warn: Color,
    pub footer_context_critical: Color,
    pub pending_input_header: Color,
    pub pending_input_text: Color,
    pub pending_input_steer_prefix: Color,
}

impl Default for TuiTheme {
    fn default() -> Self {
        Self {
            background: Color::Reset,
            surface: Color::Rgb(31, 35, 43),
            surface_border: Color::Rgb(75, 88, 104),
            brand: Color::Rgb(198, 120, 221),
            status_ok: Color::Rgb(78, 200, 126),
            status_error: Color::Rgb(232, 84, 84),
            status_warn: Color::Rgb(232, 168, 56),
            text_muted: Color::Rgb(139, 148, 158),
            // Soft white body text instead of pure terminal white.
            text_primary: Color::Rgb(198, 208, 245),
            prompt: Color::Rgb(198, 208, 245),
            composer_bg: Color::Reset,
            user_message: Color::Rgb(229, 200, 144),
            user_bg: Color::Reset,
            diff_added: Color::Rgb(78, 200, 126),
            diff_removed: Color::Rgb(232, 84, 84),
            diff_hunk: Color::Rgb(232, 168, 56),
            diff_context: Color::Rgb(139, 148, 158),
            selection_bg: Color::DarkGray,
            status_pending: Color::Rgb(139, 148, 158),
            status_cancelled: Color::DarkGray,
            approval_bg: Color::Reset,
            approval_border: Color::Rgb(75, 88, 104),
            approval_title: Color::Rgb(232, 168, 56),
            selected_fg: Color::Black,
            // Selection / overlay track the magenta brand color.
            selected_bg: Color::Rgb(198, 120, 221),
            overlay_border: Color::Rgb(198, 120, 221),
            footer_permission_allow: Color::Rgb(78, 200, 126),
            footer_permission_ask: Color::Rgb(198, 120, 221),
            footer_permission_deny: Color::Rgb(232, 84, 84),
            footer_working: Color::Rgb(198, 120, 221),
            footer_context_ok: Color::Rgb(139, 148, 158),
            footer_context_warn: Color::Rgb(232, 168, 56),
            footer_context_critical: Color::Rgb(232, 84, 84),
            pending_input_header: Color::Rgb(139, 148, 158),
            pending_input_text: Color::Rgb(139, 148, 158),
            pending_input_steer_prefix: Color::Rgb(198, 120, 221),
        }
    }
}

impl TuiTheme {
    #[must_use]
    pub const fn with_background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    #[must_use]
    pub const fn with_surface(mut self, color: Color) -> Self {
        self.surface = color;
        self.composer_bg = color;
        self.approval_bg = color;
        self
    }

    #[must_use]
    pub const fn with_surface_border(mut self, color: Color) -> Self {
        self.surface_border = color;
        self.overlay_border = color;
        self.approval_border = color;
        self
    }

    #[must_use]
    pub const fn with_brand(mut self, color: Color) -> Self {
        self.brand = color;
        self.overlay_border = color;
        self
    }

    #[must_use]
    pub const fn with_status_ok(mut self, color: Color) -> Self {
        self.status_ok = color;
        self
    }

    #[must_use]
    pub const fn with_status_error(mut self, color: Color) -> Self {
        self.status_error = color;
        self
    }

    #[must_use]
    pub const fn with_status_warn(mut self, color: Color) -> Self {
        self.status_warn = color;
        self.approval_title = color;
        self
    }

    #[must_use]
    pub const fn with_text_muted(mut self, color: Color) -> Self {
        self.text_muted = color;
        self
    }

    #[must_use]
    pub const fn with_text_primary(mut self, color: Color) -> Self {
        self.text_primary = color;
        self
    }

    #[must_use]
    pub const fn with_prompt(mut self, color: Color) -> Self {
        self.prompt = color;
        self
    }

    #[must_use]
    pub const fn with_composer_bg(mut self, color: Color) -> Self {
        self.composer_bg = color;
        self
    }

    #[must_use]
    pub const fn with_user_message(mut self, color: Color) -> Self {
        self.user_message = color;
        self
    }

    #[must_use]
    pub const fn with_footer_permission_allow(mut self, color: Color) -> Self {
        self.footer_permission_allow = color;
        self
    }

    #[must_use]
    pub const fn with_footer_permission_ask(mut self, color: Color) -> Self {
        self.footer_permission_ask = color;
        self
    }

    #[must_use]
    pub const fn with_footer_permission_deny(mut self, color: Color) -> Self {
        self.footer_permission_deny = color;
        self
    }

    #[must_use]
    pub const fn with_footer_working(mut self, color: Color) -> Self {
        self.footer_working = color;
        self
    }

    #[must_use]
    pub const fn with_footer_context_ok(mut self, color: Color) -> Self {
        self.footer_context_ok = color;
        self
    }

    #[must_use]
    pub const fn with_footer_context_warn(mut self, color: Color) -> Self {
        self.footer_context_warn = color;
        self
    }

    #[must_use]
    pub const fn with_footer_context_critical(mut self, color: Color) -> Self {
        self.footer_context_critical = color;
        self
    }

    #[must_use]
    pub const fn with_pending_input_header(mut self, color: Color) -> Self {
        self.pending_input_header = color;
        self
    }

    #[must_use]
    pub const fn with_pending_input_text(mut self, color: Color) -> Self {
        self.pending_input_text = color;
        self
    }

    #[must_use]
    pub const fn with_pending_input_steer_prefix(mut self, color: Color) -> Self {
        self.pending_input_steer_prefix = color;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeMode {
    Editing,
    Streaming,
    Overlay,
    Approval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DevelopmentMode {
    #[default]
    Normal,
    Plan,
    Goal(GoalModeStatus),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GoalModeStatus {
    #[default]
    Pending,
    Active,
    Paused,
    Blocked,
}

pub(super) fn format_token_count(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{}m", tokens / 1_000_000)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

pub(super) fn review_title(operation: PermissionOperation) -> &'static str {
    match operation {
        PermissionOperation::GoalTransition => "Goal Review",
        _ => "Plan Review",
    }
}

/// Extract the model-supplied option labels and a human-readable summary from
/// an `ExitPlanMode` approval request's arguments. Returns `(labels, body)`
/// where `labels` is the ordered list of approach labels (empty when the model
/// supplied no alternatives) and `body` is a rendered list of
/// `label — description` lines for the dialog body.
pub(super) fn plan_review_options(arguments: &serde_json::Value) -> (Vec<String>, String) {
    let Some(options) = arguments.get("options").and_then(|v| v.as_array()) else {
        return (Vec::new(), String::new());
    };
    let mut labels = Vec::new();
    let mut lines = Vec::new();
    for option in options {
        let Some(label) = option.get("label").and_then(|v| v.as_str()) else {
            continue;
        };
        labels.push(label.to_owned());
        match option.get("description").and_then(|v| v.as_str()) {
            Some(desc) if !desc.trim().is_empty() => lines.push(format!("• {label} — {desc}")),
            _ => lines.push(format!("• {label}")),
        }
    }
    (labels, lines.join("\n"))
}
