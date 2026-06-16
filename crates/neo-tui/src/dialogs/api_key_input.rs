//! API key input dialog — masked single-field input.

use crate::ansi::Color;
use crate::components::visible_width;
use crate::{InputEvent, InputResult, TuiTheme};

pub struct ApiKeyInputOptions {
    pub title: String,
    pub provider_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiKeyInputResult {
    Submitted(String),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyInputState {
    value: String,
    theme: TuiTheme,
    title: String,
    provider_name: String,
    result: Option<ApiKeyInputResult>,
}

impl ApiKeyInputState {
    #[must_use]
    pub fn new(opts: ApiKeyInputOptions, theme: TuiTheme) -> Self {
        Self {
            value: String::new(),
            theme,
            title: opts.title,
            provider_name: opts.provider_name,
            result: None,
        }
    }

    fn masked_display(&self) -> String {
        "•".repeat(self.value.len())
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        let inner_w = width.saturating_sub(2).max(1);
        let mut lines = Vec::new();

        // Top border
        let title_str = format!(" {} ", self.title);
        let remaining = inner_w.saturating_sub(visible_width(&title_str));
        lines.push(format!(
            "\x1b[38;2;{}m╭{title_str}{}\x1b[0m",
            rgb(&self.theme.overlay_border),
            "─".repeat(remaining),
        ));

        // Provider name hint
        lines.push(format!(
            "\x1b[38;2;{}m Provider: {}\x1b[0m",
            rgb(&self.theme.muted),
            self.provider_name
        ));
        lines.push(String::new());

        // Input field
        let masked = self.masked_display();
        lines.push(format!(
            "\x1b[38;2;{}m API Key: \x1b[0m{masked}▏",
            rgb(&self.theme.header)
        ));

        // Hint
        lines.push(format!(
            "\x1b[38;2;{}m Enter submit · Esc cancel\x1b[0m",
            rgb(&self.theme.muted)
        ));

        // Bottom border
        lines.push(format!(
            "\x1b[38;2;{}m╰{}\x1b[0m",
            rgb(&self.theme.overlay_border),
            "─".repeat(inner_w),
        ));

        lines
    }

    pub fn handle_input(&mut self, input: InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Ignored;
        }
        match input {
            InputEvent::Insert(ch) if ch.is_ascii_graphic() || ch == ' ' => {
                self.value.push(ch);
                InputResult::Handled
            }
            InputEvent::Backspace => {
                self.value.pop();
                InputResult::Handled
            }
            InputEvent::Submit => {
                if !self.value.is_empty() {
                    self.result = Some(ApiKeyInputResult::Submitted(self.value.clone()));
                    InputResult::Submitted
                } else {
                    InputResult::Ignored
                }
            }
            InputEvent::Cancel => {
                self.result = Some(ApiKeyInputResult::Cancelled);
                InputResult::Cancelled
            }
            _ => InputResult::Ignored,
        }
    }

    #[must_use]
    pub fn result(&self) -> Option<&ApiKeyInputResult> {
        self.result.as_ref()
    }

    #[must_use]
    pub fn take_result(&mut self) -> Option<ApiKeyInputResult> {
        self.result.take()
    }
}

fn rgb(c: &Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("{r};{g};{b}"),
        _ => "255;255;255".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> TuiTheme {
        TuiTheme::default()
    }

    #[test]
    fn typing_appends_and_masks() {
        let mut state = ApiKeyInputState::new(
            ApiKeyInputOptions {
                title: "API Key".into(),
                provider_name: "OpenAI".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Insert('s'));
        state.handle_input(InputEvent::Insert('k'));
        let lines = state.render_lines(40);
        let combined: String = lines.join("\n");
        assert!(combined.contains("••")); // masked
        assert!(!combined.contains("sk")); // raw value not shown
    }

    #[test]
    fn backspace_removes_last() {
        let mut state = ApiKeyInputState::new(
            ApiKeyInputOptions {
                title: "T".into(),
                provider_name: "P".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Insert('a'));
        state.handle_input(InputEvent::Insert('b'));
        state.handle_input(InputEvent::Backspace);
        let lines = state.render_lines(40);
        let combined: String = lines.join("\n");
        assert!(combined.contains("•") && !combined.contains("••"));
    }

    #[test]
    fn enter_submits() {
        let mut state = ApiKeyInputState::new(
            ApiKeyInputOptions {
                title: "T".into(),
                provider_name: "P".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Insert('k'));
        state.handle_input(InputEvent::Submit);
        match state.take_result().unwrap() {
            ApiKeyInputResult::Submitted(v) => assert_eq!(v, "k"),
            ApiKeyInputResult::Cancelled => panic!("expected Submitted"),
        }
    }

    #[test]
    fn esc_cancels() {
        let mut state = ApiKeyInputState::new(
            ApiKeyInputOptions {
                title: "T".into(),
                provider_name: "P".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Cancel);
        assert!(matches!(
            state.take_result(),
            Some(ApiKeyInputResult::Cancelled)
        ));
    }

    #[test]
    fn enter_on_empty_does_nothing() {
        let mut state = ApiKeyInputState::new(
            ApiKeyInputOptions {
                title: "T".into(),
                provider_name: "P".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Submit);
        assert!(state.result.is_none());
    }
}
