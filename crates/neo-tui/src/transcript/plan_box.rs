use crate::ansi::{Color, Style};
use crate::app::TuiTheme;
use crate::core::Line;

/// Renders plan content inside a bordered box, displayed within the
/// `ExitPlanMode` tool card.
#[derive(Debug, Clone)]
pub struct PlanBoxComponent {
    content: String,
    path: Option<String>,
    status: Option<String>,
}

impl PlanBoxComponent {
    #[must_use]
    pub fn new(content: impl Into<String>, path: Option<String>) -> Self {
        Self {
            content: content.into(),
            path,
            status: None,
        }
    }

    /// Set a status suffix (e.g. "Rejected") shown in the title bar.
    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    /// Render the plan box as styled lines, fitting within `width` columns.
    #[must_use]
    pub fn render(&self, width: usize, theme: &TuiTheme) -> Vec<Line> {
        if width < 10 {
            return vec![];
        }

        let border_color = theme.success;
        let content_color = theme.assistant;
        let muted_color = theme.muted;

        let content_width = width.saturating_sub(4).max(1); // │ + space + content + space + │

        // Title
        let basename = self
            .path
            .as_ref()
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or("plan");
        let title = if let Some(status) = &self.status {
            format!(" plan: {basename} · {status} ")
        } else {
            format!(" plan: {basename} ")
        };

        let mut lines = vec![Self::titled_border(&title, width, border_color)];

        // Content lines
        for raw_line in self.content.lines() {
            for chunk in Self::wrap_text(raw_line, content_width) {
                let padded = Self::pad_to(&chunk, content_width);
                lines.push(Line::styled(
                    format!(" \u{2502} {padded} \u{2502}"),
                    Style::default().fg(content_color),
                ));
            }
        }

        // Empty placeholder if no content
        if self.content.trim().is_empty() {
            let padded = " ".repeat(content_width);
            lines.push(Line::styled(
                format!(" \u{2502} {padded} \u{2502}"),
                Style::default().fg(muted_color),
            ));
        }

        // Bottom border
        let bottom = format!("\u{2514}{}", "\u{2500}".repeat(width.saturating_sub(1)));
        lines.push(Line::styled(bottom, Style::default().fg(border_color)));

        lines
    }

    fn titled_border(title: &str, width: usize, color: Color) -> Line {
        let title_display: String = title.chars().take(width.saturating_sub(2)).collect();
        let remaining = width
            .saturating_sub(2)
            .saturating_sub(title_display.chars().count());
        let line = format!("\u{250c}{title_display}{}", "\u{2500}".repeat(remaining));
        Line::styled(line, Style::default().fg(color))
    }

    fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
        if text.is_empty() {
            return vec![String::new()];
        }
        let mut result = Vec::new();
        let mut current = String::new();
        let mut current_len = 0usize;
        for word in text.split_whitespace() {
            let word_len = word.chars().count();
            if current_len == 0 {
                current = word.to_string();
                current_len = word_len;
            } else if current_len + 1 + word_len <= max_width {
                current.push(' ');
                current.push_str(word);
                current_len += 1 + word_len;
            } else {
                result.push(std::mem::take(&mut current));
                current = word.to_string();
                current_len = word_len;
            }
        }
        if !current.is_empty() {
            result.push(current);
        }
        if result.is_empty() {
            result.push(String::new());
        }
        result
    }

    fn pad_to(text: &str, width: usize) -> String {
        let char_count = text.chars().count();
        if char_count >= width {
            text.chars().take(width).collect()
        } else {
            let mut result = text.to_string();
            result.push_str(&" ".repeat(width - char_count));
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_basic_box() {
        let comp = PlanBoxComponent::new("# Plan\n- Step 1", Some("/tmp/abc.md".to_string()));
        let lines = comp.render(40, &TuiTheme::default());
        assert!(lines.len() >= 3); // top border + content lines + bottom border
        let top = lines[0].to_ansi();
        assert!(top.contains("plan: abc.md"));
    }

    #[test]
    fn render_with_status() {
        let comp =
            PlanBoxComponent::new("plan", Some("/tmp/x.md".to_string())).with_status("Rejected");
        let lines = comp.render(40, &TuiTheme::default());
        let top = lines[0].to_ansi();
        assert!(top.contains("Rejected"));
    }

    #[test]
    fn render_empty_content() {
        let comp = PlanBoxComponent::new("", None);
        let lines = comp.render(20, &TuiTheme::default());
        assert!(lines.len() >= 3);
    }

    #[test]
    fn wrap_text_long_line() {
        let wrapped = PlanBoxComponent::wrap_text("aaaa bbbb cccc dddd", 10);
        assert!(wrapped.len() > 1);
    }
}
