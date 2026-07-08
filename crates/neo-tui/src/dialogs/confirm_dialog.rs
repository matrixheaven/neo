use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{InputResult, Style, paint, truncate_width, visible_width};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmDialogOptions {
    pub id: String,
    pub title: String,
    pub hint: String,
    pub lines: Vec<String>,
    pub theme: TuiTheme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmDialogResult {
    Approved { id: String },
    Cancelled { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmDialogState {
    id: String,
    title: String,
    hint: String,
    lines: Vec<String>,
    theme: TuiTheme,
    result: Option<ConfirmDialogResult>,
}

impl ConfirmDialogState {
    #[must_use]
    pub fn new(opts: ConfirmDialogOptions) -> Self {
        Self {
            id: opts.id,
            title: opts.title,
            hint: opts.hint,
            lines: opts.lines,
            theme: opts.theme,
            result: None,
        }
    }

    #[must_use]
    pub const fn result(&self) -> Option<&ConfirmDialogResult> {
        self.result.as_ref()
    }

    pub fn take_result(&mut self) -> Option<ConfirmDialogResult> {
        self.result.take()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        if width < 4 {
            return lines;
        }

        let inner_width = width.saturating_sub(2).max(1);
        let border_style = Style::default().fg(self.theme.overlay_border);
        let title_style = Style::default().fg(self.theme.brand).bold();

        lines.push(paint(
            &format!("┌{}┐", "─".repeat(inner_width)),
            border_style,
        ));
        lines.push(box_line(
            &format!(" {}", self.title),
            inner_width,
            title_style,
            border_style,
        ));
        lines.push(box_line_raw(
            &format!(" {}", render_hint(&self.hint, self.theme)),
            inner_width,
            border_style,
        ));
        lines.push(box_line("", inner_width, Style::default(), border_style));

        for line in &self.lines {
            lines.push(box_line_raw(
                &render_body_line(line, self.theme),
                inner_width,
                border_style,
            ));
        }

        lines.push(box_line("", inner_width, Style::default(), border_style));
        lines.push(paint(
            &format!("└{}┘", "─".repeat(inner_width)),
            border_style,
        ));
        lines
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Handled;
        }

        match input {
            InputEvent::Insert(character) if matches!(character, 'y' | 'Y') => {
                self.result = Some(ConfirmDialogResult::Approved {
                    id: self.id.clone(),
                });
                InputResult::Submitted
            }
            InputEvent::Insert(character) if matches!(character, 'n' | 'N') => self.cancel(),
            InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => {
                self.cancel()
            }
            _ => InputResult::Ignored,
        }
    }

    fn cancel(&mut self) -> InputResult {
        self.result = Some(ConfirmDialogResult::Cancelled {
            id: self.id.clone(),
        });
        InputResult::Cancelled
    }
}

fn box_line(
    content: &str,
    content_width: usize,
    content_style: Style,
    border_style: Style,
) -> String {
    let padded = truncate_width(content, content_width, "…", true);
    let styled_content = paint(&padded, content_style);
    let left = paint("│", border_style);
    let right = paint("│", border_style);
    format!("{left}{styled_content}{right}")
}

fn box_line_raw(content: &str, content_width: usize, border_style: Style) -> String {
    let visible = visible_width(&crate::primitive::strip_ansi(content));
    let content = if visible > content_width {
        truncate_width(content, content_width, "…", false)
    } else {
        content.to_owned()
    };
    let padding =
        content_width.saturating_sub(visible_width(&crate::primitive::strip_ansi(&content)));
    let left = paint("│", border_style);
    let right = paint("│", border_style);
    format!("{left}{content}{}{right}", " ".repeat(padding))
}

fn render_hint(hint: &str, theme: TuiTheme) -> String {
    let mut rendered = Vec::new();
    for part in hint.split(" · ") {
        let Some((key, rest)) = part.split_once(' ') else {
            rendered.push(paint(part, Style::default().fg(theme.text_muted)));
            continue;
        };
        let key = paint(
            key,
            Style::default()
                .fg(theme.selected_fg)
                .bg(theme.selection_bg)
                .bold(),
        );
        let rest = paint(&format!(" {rest}"), Style::default().fg(theme.text_muted));
        rendered.push(format!("{key}{rest}"));
    }
    rendered.join(&paint(" · ", Style::default().fg(theme.text_muted)))
}

fn render_body_line(line: &str, theme: TuiTheme) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let indent = line
        .chars()
        .take_while(|character| character.is_whitespace())
        .collect::<String>();
    let style = if matches!(trimmed, "Directory" | "Access") {
        Style::default().fg(theme.brand).bold()
    } else if is_pathish(trimmed) {
        Style::default().fg(theme.prompt).bold()
    } else if line.starts_with("   ") {
        Style::default().fg(theme.text_muted)
    } else {
        Style::default().fg(theme.text_primary)
    };
    format!("{indent}{}", paint(trimmed, style))
}

fn is_pathish(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("~/")
        || value.starts_with("./")
        || value.starts_with("../")
        || value.as_bytes().get(1).is_some_and(|byte| *byte == b':')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ConfirmDialogState {
        ConfirmDialogState::new(ConfirmDialogOptions {
            id: "toggle-write:/tmp/shared".to_owned(),
            title: "Confirm Write Access".to_owned(),
            hint: "Y approve · N cancel · Esc cancel".to_owned(),
            lines: vec![
                " Enable write access for this directory?".to_owned(),
                " /tmp/shared".to_owned(),
            ],
            theme: TuiTheme::default(),
        })
    }

    fn visible_lines(state: &ConfirmDialogState, width: usize) -> Vec<String> {
        state
            .render_lines(width)
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect()
    }

    #[test]
    fn renders_title_hint_and_body() {
        let joined = visible_lines(&state(), 80).join("\n");

        assert!(
            joined.contains("Confirm Write Access"),
            "title missing: {joined}"
        );
        assert!(joined.contains("Y approve"), "hint missing: {joined}");
        assert!(joined.contains("/tmp/shared"), "body missing: {joined}");
    }

    #[test]
    fn renders_colored_action_hint_and_body_accents() {
        let rendered = state().render_lines(80).join("\n");
        let theme = TuiTheme::default();

        assert!(rendered.contains(&crate::primitive::bg_to_ansi(theme.selection_bg)));
        assert!(rendered.contains(&crate::primitive::fg_to_ansi(theme.brand)));
        assert!(rendered.contains(&crate::primitive::fg_to_ansi(theme.prompt)));
    }

    #[test]
    fn y_approves() {
        let mut state = state();
        let result = state.handle_input(&InputEvent::Insert('Y'));

        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(
            state.result(),
            Some(ConfirmDialogResult::Approved { id }) if id == "toggle-write:/tmp/shared"
        ));
    }

    #[test]
    fn n_cancels() {
        let mut state = state();
        let result = state.handle_input(&InputEvent::Insert('N'));

        assert!(matches!(result, InputResult::Cancelled));
        assert!(matches!(
            state.result(),
            Some(ConfirmDialogResult::Cancelled { id }) if id == "toggle-write:/tmp/shared"
        ));
    }
}
