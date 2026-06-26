use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::InputResult;
use crate::primitive::{Style, paint};
use crate::primitive::{truncate_width, visible_width};
use crate::shell::TuiTheme;

/// Options used to create a [`TextInputState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInputOptions {
    pub title: String,
    pub prompt: String,
    pub submit_label: String,
}

/// Result produced by a [`TextInputState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputResult {
    Submitted(String),
    Cancelled,
}

/// Single-line text input dialog for non-secret strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInputState {
    value: String,
    theme: TuiTheme,
    title: String,
    prompt: String,
    submit_label: String,
    result: Option<TextInputResult>,
}

impl TextInputState {
    #[must_use]
    pub fn new(opts: TextInputOptions, theme: TuiTheme) -> Self {
        Self {
            value: String::new(),
            theme,
            title: opts.title,
            prompt: opts.prompt,
            submit_label: opts.submit_label,
            result: None,
        }
    }

    #[must_use]
    pub fn result(&self) -> Option<&TextInputResult> {
        self.result.as_ref()
    }

    #[must_use]
    pub fn take_result(&mut self) -> Option<TextInputResult> {
        self.result.take()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        if width < 4 {
            return lines;
        }
        let inner_w = width.saturating_sub(2).max(1);
        let border_style = Style::default().fg(self.theme.overlay_border);
        let title_style = Style::default().fg(self.theme.text_primary).bold();
        let muted_style = Style::default().fg(self.theme.text_muted);
        let prompt_style = Style::default().fg(self.theme.text_primary);
        let value_style = Style::default().fg(self.theme.prompt);

        lines.push(paint(&format!("┌{}┐", "─".repeat(inner_w)), border_style));
        lines.push(box_line(
            &format!(" {}", self.title),
            inner_w,
            title_style,
            border_style,
        ));
        lines.push(box_line("", inner_w, Style::default(), border_style));

        // Prompt and value on one line, capped.
        let prompt_text = format!(" {}: ", self.prompt);
        let prompt_visible = visible_width(&prompt_text);
        let max_value = inner_w.saturating_sub(prompt_visible).saturating_sub(1);
        let value_display = truncate_width(&self.value, max_value, "…", false);
        let value_line = format!(
            "{}{}▏",
            paint(&prompt_text, prompt_style),
            paint(&value_display, value_style)
        );
        lines.push(box_line_raw(&value_line, inner_w, border_style));

        lines.push(box_line("", inner_w, Style::default(), border_style));
        let hint = format!(" {} · Esc cancel", self.submit_label);
        lines.push(box_line(&hint, inner_w, muted_style, border_style));
        lines.push(paint(&format!("└{}┘", "─".repeat(inner_w)), border_style));

        lines
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Handled;
        }
        match input {
            InputEvent::Insert(ch) => {
                if ch.is_ascii_graphic() || matches!(ch, ' ' | '-' | '_' | '.' | '/' | ':' | '=') {
                    self.value.push(*ch);
                    InputResult::Handled
                } else {
                    InputResult::Ignored
                }
            }
            InputEvent::Paste(text) => {
                let cleaned: String = text
                    .chars()
                    .filter(|c| {
                        c.is_ascii_graphic() || matches!(c, ' ' | '-' | '_' | '.' | '/' | ':' | '=')
                    })
                    .collect();
                if cleaned.is_empty() {
                    return InputResult::Ignored;
                }
                self.value.push_str(&cleaned);
                InputResult::Handled
            }
            InputEvent::Backspace => {
                self.value.pop();
                InputResult::Handled
            }
            InputEvent::Submit => {
                if self.value.trim().is_empty() {
                    InputResult::Handled
                } else {
                    self.result = Some(TextInputResult::Submitted(self.value.trim().to_owned()));
                    InputResult::Submitted
                }
            }
            InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => {
                self.result = Some(TextInputResult::Cancelled);
                InputResult::Cancelled
            }
            _ => InputResult::Ignored,
        }
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
    let padded = truncate_width(content, content_width, "…", true);
    let left = paint("│", border_style);
    let right = paint("│", border_style);
    format!("{left}{padded}{right}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> TuiTheme {
        TuiTheme::default()
    }

    fn input(title: &str, prompt: &str) -> TextInputState {
        TextInputState::new(
            TextInputOptions {
                title: title.to_owned(),
                prompt: prompt.to_owned(),
                submit_label: "Enter".to_owned(),
            },
            theme(),
        )
    }

    fn visible_lines(state: &TextInputState, width: usize) -> Vec<String> {
        state
            .render_lines(width)
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect()
    }

    #[test]
    fn render_shows_title_prompt_and_hint() {
        let state = input("Server id", "id");
        let visible = visible_lines(&state, 40);
        let joined = visible.join("\n");
        assert!(joined.contains("Server id"), "title missing: {joined}");
        assert!(joined.contains("id:"), "prompt missing: {joined}");
        assert!(
            joined.contains("Enter · Esc cancel"),
            "hint missing: {joined}"
        );
    }

    #[test]
    fn typing_updates_value() {
        let mut state = input("Server id", "id");
        state.handle_input(&InputEvent::Insert('f'));
        state.handle_input(&InputEvent::Insert('s'));
        let visible = visible_lines(&state, 40);
        assert!(visible.join("\n").contains("fs"));
    }

    #[test]
    fn submit_returns_value() {
        let mut state = input("Server id", "id");
        state.handle_input(&InputEvent::Insert('f'));
        let result = state.handle_input(&InputEvent::Submit);
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(state.result(), Some(TextInputResult::Submitted(v)) if v == "f"));
    }

    #[test]
    fn cancel_returns_cancelled() {
        let mut state = input("Server id", "id");
        state.handle_input(&InputEvent::Insert('f'));
        let result = state.handle_input(&InputEvent::Cancel);
        assert!(matches!(result, InputResult::Cancelled));
        assert!(matches!(state.result(), Some(TextInputResult::Cancelled)));
    }

    #[test]
    fn empty_submit_is_ignored() {
        let mut state = input("Server id", "id");
        let result = state.handle_input(&InputEvent::Submit);
        assert!(matches!(result, InputResult::Handled));
        assert!(state.result().is_none());
    }
}
