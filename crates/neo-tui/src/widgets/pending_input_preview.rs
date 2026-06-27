use std::collections::VecDeque;

use crate::primitive::{Style, paint};
use crate::primitive::{truncate_width, visible_width, wrap_width};
use crate::shell::TuiTheme;

/// Maximum content lines shown per message before an ellipsis row.
const PREVIEW_LINE_LIMIT: usize = 3;

/// Renders pending steers and queued follow-ups in a compact panel above the
/// composer. Modeled after Codex/Kimi Code's pending input preview.
pub struct PendingInputPreview<'a> {
    pending_steers: &'a VecDeque<String>,
    queued_follow_ups: &'a VecDeque<String>,
    queued_shell_commands: &'a VecDeque<String>,
    theme: TuiTheme,
}

impl<'a> PendingInputPreview<'a> {
    #[must_use]
    pub fn new(
        pending_steers: &'a VecDeque<String>,
        queued_follow_ups: &'a VecDeque<String>,
        queued_shell_commands: &'a VecDeque<String>,
    ) -> Self {
        Self {
            pending_steers,
            queued_follow_ups,
            queued_shell_commands,
            theme: TuiTheme::default(),
        }
    }

    #[must_use]
    pub const fn with_theme(mut self, theme: TuiTheme) -> Self {
        self.theme = theme;
        self
    }

    /// Compute the rendered height of the panel for a given terminal width.
    #[must_use]
    pub fn height(&self, width: u16) -> u16 {
        u16::try_from(self.render(usize::from(width)).len()).unwrap_or(u16::MAX)
    }

    /// Render the panel as ANSI lines. Returns an empty vec when there is
    /// nothing pending.
    #[must_use]
    pub fn render(&self, width: usize) -> Vec<String> {
        if self.pending_steers.is_empty()
            && self.queued_follow_ups.is_empty()
            && self.queued_shell_commands.is_empty()
        {
            return Vec::new();
        }

        let mut lines = Vec::new();
        if !self.pending_steers.is_empty() {
            lines.extend(self.render_section(
                "Messages to be submitted after next tool call",
                self.pending_steers,
                true,
                width,
            ));
        }

        if !self.queued_follow_ups.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.extend(self.render_section(
                "Queued follow-up inputs",
                self.queued_follow_ups,
                false,
                width,
            ));
            lines.push(self.render_hint("Alt+↑ edit last queued message", width));
        }

        if !self.queued_shell_commands.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.extend(self.render_shell_section(width));
            lines.push(self.render_hint("Alt+↑ edit · will run after current task", width));
        }

        lines
            .into_iter()
            .map(|line| truncate_width(&line, width, "", false))
            .collect()
    }

    fn render_shell_section(&self, width: usize) -> Vec<String> {
        let mut lines = vec![paint(
            "• Queued shell commands",
            Style::default().fg(self.theme.pending_input_header),
        )];
        let prefix = "  ❯ ";
        let prefix_width = visible_width(prefix);
        let body_width = width.saturating_sub(prefix_width).max(1);
        let continuation = " ".repeat(prefix_width);
        let text_style = Style::default().fg(self.theme.pending_input_text).italic();
        let shell_style = Style::default().fg(self.theme.shell_mode);

        for command in self.queued_shell_commands {
            let command = format!("$ {command}");
            let wrapped = wrap_width(&command, body_width);
            for (index, line) in wrapped.iter().enumerate().take(PREVIEW_LINE_LIMIT) {
                if index == 0 {
                    lines.push(format!(
                        "{} {}",
                        paint(prefix.trim_end(), shell_style),
                        paint(line, shell_style)
                    ));
                } else {
                    lines.push(paint(&format!("{continuation}{line}"), text_style));
                }
            }
            if wrapped.len() > PREVIEW_LINE_LIMIT {
                lines.push(paint(&format!("{continuation}…"), text_style));
            }
        }
        lines
    }

    fn render_section(
        &self,
        header: &str,
        messages: &VecDeque<String>,
        is_steer: bool,
        width: usize,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(paint(
            &format!("• {header}"),
            Style::default().fg(self.theme.pending_input_header),
        ));

        let prefix = "  ↳ ";
        let prefix_width = visible_width(prefix);
        let body_width = width.saturating_sub(prefix_width).max(1);
        let continuation = " ".repeat(prefix_width);
        let text_style = Style::default().fg(self.theme.pending_input_text).italic();
        let prefix_style = if is_steer {
            Style::default().fg(self.theme.pending_input_steer_prefix)
        } else {
            text_style
        };

        for message in messages {
            let wrapped = wrap_width(message, body_width);
            for (i, line) in wrapped.iter().enumerate().take(PREVIEW_LINE_LIMIT) {
                if i == 0 {
                    let colored_prefix = paint(prefix.trim_end(), prefix_style);
                    let colored_body = paint(line, text_style);
                    lines.push(format!("{colored_prefix} {colored_body}"));
                } else {
                    lines.push(paint(&format!("{continuation}{line}"), text_style));
                }
            }
            if wrapped.len() > PREVIEW_LINE_LIMIT {
                lines.push(paint(&format!("{continuation}…"), text_style));
            }
        }

        lines
    }

    fn render_hint(&self, text: &str, width: usize) -> String {
        let indent = "    ";
        let prefix_width = visible_width(indent);
        let body_width = width.saturating_sub(prefix_width).max(1);
        let truncated = if text.chars().count() > body_width {
            let mut truncated = text.chars().take(body_width).collect::<String>();
            truncated.push('…');
            truncated
        } else {
            text.to_owned()
        };
        paint(
            &format!("{indent}{truncated}"),
            Style::default().fg(self.theme.pending_input_header),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_panel_renders_nothing() {
        let steers: VecDeque<String> = VecDeque::new();
        let follow_ups: VecDeque<String> = VecDeque::new();
        let shell_commands: VecDeque<String> = VecDeque::new();
        let panel = PendingInputPreview::new(&steers, &follow_ups, &shell_commands);
        assert!(panel.render(40).is_empty());
        assert_eq!(panel.height(40), 0);
    }

    #[test]
    fn steer_section_renders_with_brand_prefix() {
        let steers: VecDeque<String> = VecDeque::from(["Please continue.".to_owned()]);
        let follow_ups: VecDeque<String> = VecDeque::new();
        let shell_commands: VecDeque<String> = VecDeque::new();
        let panel = PendingInputPreview::new(&steers, &follow_ups, &shell_commands);
        let lines = panel.render(60);
        let plain: Vec<String> = lines
            .iter()
            .map(|l| crate::primitive::strip_ansi(l))
            .collect();
        assert_eq!(plain[0], "• Messages to be submitted after next tool call");
        assert_eq!(plain[1], "  ↳ Please continue.");
    }

    #[test]
    fn follow_up_section_renders_hint() {
        let steers: VecDeque<String> = VecDeque::new();
        let follow_ups: VecDeque<String> = VecDeque::from(["Hello?".to_owned()]);
        let shell_commands: VecDeque<String> = VecDeque::new();
        let panel = PendingInputPreview::new(&steers, &follow_ups, &shell_commands);
        let lines = panel.render(40);
        let plain: Vec<String> = lines
            .iter()
            .map(|l| crate::primitive::strip_ansi(l))
            .collect();
        assert_eq!(plain[0], "• Queued follow-up inputs");
        assert_eq!(plain[1], "  ↳ Hello?");
        assert!(plain[2].contains("Alt+↑ edit last queued message"));
    }

    #[test]
    fn multi_message_panel_separates_sections() {
        let steers: VecDeque<String> = VecDeque::from(["Steer one".to_owned()]);
        let follow_ups: VecDeque<String> =
            VecDeque::from(["Follow one".to_owned(), "Follow two".to_owned()]);
        let shell_commands: VecDeque<String> = VecDeque::new();
        let panel = PendingInputPreview::new(&steers, &follow_ups, &shell_commands);
        let lines = panel.render(60);
        let plain: Vec<String> = lines
            .iter()
            .map(|l| crate::primitive::strip_ansi(l))
            .collect();
        assert!(plain.contains(&"• Messages to be submitted after next tool call".to_owned()));
        assert!(plain.contains(&"  ↳ Steer one".to_owned()));
        assert!(plain.contains(&"• Queued follow-up inputs".to_owned()));
        assert!(plain.contains(&"  ↳ Follow one".to_owned()));
        assert!(plain.contains(&"  ↳ Follow two".to_owned()));
    }

    #[test]
    fn long_message_wraps_and_truncates() {
        let steers: VecDeque<String> = VecDeque::from(["a".repeat(200)]);
        let follow_ups: VecDeque<String> = VecDeque::new();
        let shell_commands: VecDeque<String> = VecDeque::new();
        let panel = PendingInputPreview::new(&steers, &follow_ups, &shell_commands);
        let lines = panel.render(40);
        let plain: Vec<String> = lines
            .iter()
            .map(|l| crate::primitive::strip_ansi(l))
            .collect();
        // Header + up to 3 wrapped lines + ellipsis row.
        assert_eq!(plain.len(), 5);
        assert!(plain.last().unwrap().contains('…'));
    }
}
