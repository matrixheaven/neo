//! Custom registry import dialog — two-field form (URL + Bearer token).

use crate::ansi::Color;
use crate::components::visible_width;
use crate::{InputEvent, InputResult, TuiTheme};

pub struct CustomRegistryImportOptions {
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRegistrySource {
    pub url: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomRegistryImportResult {
    Submitted(CustomRegistrySource),
    Cancelled,
}

const FIELD_URL: usize = 0;
const FIELD_TOKEN: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomRegistryImportState {
    url: String,
    token: String,
    active_field: usize,
    theme: TuiTheme,
    title: String,
    result: Option<CustomRegistryImportResult>,
}

impl CustomRegistryImportState {
    #[must_use]
    pub fn new(opts: CustomRegistryImportOptions, theme: TuiTheme) -> Self {
        Self {
            url: String::new(),
            token: String::new(),
            active_field: FIELD_URL,
            theme,
            title: opts.title,
            result: None,
        }
    }

    fn masked_token(&self) -> String {
        "•".repeat(self.token.len())
    }

    fn switch_field(&mut self, forward: bool) {
        if forward {
            self.active_field = (self.active_field + 1) % 2;
        } else if self.active_field == 0 {
            self.active_field = FIELD_TOKEN;
        } else {
            self.active_field = FIELD_URL;
        }
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

        // URL field
        let url_marker = if self.active_field == FIELD_URL {
            "▸"
        } else {
            " "
        };
        let url_color = if self.active_field == FIELD_URL {
            self.theme.accent
        } else {
            self.theme.muted
        };
        lines.push(format!(
            "\x1b[38;2;{}m{url_marker} Registry URL:\x1b[0m",
            rgb(&url_color),
        ));
        lines.push(format!(
            "  {}▏",
            if self.url.is_empty() {
                "\x1b[38;2;90;90;90m(https://...)\x1b[0m".to_owned()
            } else {
                self.url.clone()
            }
        ));

        lines.push(String::new());

        // Token field
        let token_marker = if self.active_field == FIELD_TOKEN {
            "▸"
        } else {
            " "
        };
        let token_color = if self.active_field == FIELD_TOKEN {
            self.theme.accent
        } else {
            self.theme.muted
        };
        lines.push(format!(
            "\x1b[38;2;{}m{token_marker} Bearer Token:\x1b[0m",
            rgb(&token_color),
        ));
        let masked = self.masked_token();
        lines.push(format!(
            "  {}▏",
            if masked.is_empty() {
                "\x1b[38;2;90;90;90m(optional)\x1b[0m".to_owned()
            } else {
                masked
            }
        ));

        // Hint
        lines.push(format!(
            "\x1b[38;2;{}m Tab switch · Enter submit · Esc cancel\x1b[0m",
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
            InputEvent::Insert('\t') => {
                self.switch_field(true);
                InputResult::Handled
            }
            InputEvent::Insert(ch)
                if ch.is_ascii_graphic() || ch == ' ' || ch == '/' || ch == ':' =>
            {
                match self.active_field {
                    FIELD_URL => self.url.push(ch),
                    FIELD_TOKEN => self.token.push(ch),
                    _ => {}
                }
                InputResult::Handled
            }
            InputEvent::Backspace => {
                match self.active_field {
                    FIELD_URL => {
                        self.url.pop();
                    }
                    FIELD_TOKEN => {
                        self.token.pop();
                    }
                    _ => {}
                }
                InputResult::Handled
            }
            InputEvent::Action(crate::KeybindingAction::SelectDown)
            | InputEvent::Action(crate::KeybindingAction::SelectUp) => {
                self.switch_field(true);
                InputResult::Handled
            }
            InputEvent::Submit => {
                if !self.url.is_empty() {
                    self.result = Some(CustomRegistryImportResult::Submitted(
                        CustomRegistrySource {
                            url: self.url.clone(),
                            token: self.token.clone(),
                        },
                    ));
                    InputResult::Submitted
                } else {
                    InputResult::Ignored
                }
            }
            InputEvent::Cancel => {
                self.result = Some(CustomRegistryImportResult::Cancelled);
                InputResult::Cancelled
            }
            _ => InputResult::Ignored,
        }
    }

    #[must_use]
    pub fn result(&self) -> Option<&CustomRegistryImportResult> {
        self.result.as_ref()
    }

    #[must_use]
    pub fn take_result(&mut self) -> Option<CustomRegistryImportResult> {
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
    fn typing_url_appends() {
        let mut state = CustomRegistryImportState::new(
            CustomRegistryImportOptions {
                title: "Import".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Insert('h'));
        state.handle_input(InputEvent::Insert('t'));
        let lines = state.render_lines(50);
        let combined: String = lines.join("\n");
        assert!(combined.contains("ht"));
    }

    #[test]
    fn typing_token_is_masked() {
        let mut state = CustomRegistryImportState::new(
            CustomRegistryImportOptions {
                title: "Import".into(),
            },
            theme(),
        );
        state.switch_field(true); // go to token
        state.handle_input(InputEvent::Insert('a'));
        state.handle_input(InputEvent::Insert('b'));
        let lines = state.render_lines(50);
        let combined: String = lines.join("\n");
        assert!(combined.contains("••"));
        assert!(!combined.contains("ab") || combined.contains("••"));
    }

    #[test]
    fn tab_switches_fields() {
        let mut state = CustomRegistryImportState::new(
            CustomRegistryImportOptions {
                title: "Import".into(),
            },
            theme(),
        );
        assert_eq!(state.active_field, FIELD_URL);
        state.handle_input(InputEvent::Insert('\t'));
        assert_eq!(state.active_field, FIELD_TOKEN);
        state.handle_input(InputEvent::Insert('\t'));
        assert_eq!(state.active_field, FIELD_URL);
    }

    #[test]
    fn enter_submits_with_url() {
        let mut state = CustomRegistryImportState::new(
            CustomRegistryImportOptions {
                title: "Import".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Insert('h'));
        state.handle_input(InputEvent::Submit);
        match state.take_result().unwrap() {
            CustomRegistryImportResult::Submitted(src) => {
                assert_eq!(src.url, "h");
            }
            CustomRegistryImportResult::Cancelled => panic!("expected Submitted"),
        }
    }

    #[test]
    fn enter_on_empty_url_does_nothing() {
        let mut state = CustomRegistryImportState::new(
            CustomRegistryImportOptions {
                title: "Import".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Submit);
        assert!(state.result.is_none());
    }

    #[test]
    fn esc_cancels() {
        let mut state = CustomRegistryImportState::new(
            CustomRegistryImportOptions {
                title: "Import".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Cancel);
        assert!(matches!(
            state.take_result(),
            Some(CustomRegistryImportResult::Cancelled)
        ));
    }

    #[test]
    fn backspace_works_on_active_field() {
        let mut state = CustomRegistryImportState::new(
            CustomRegistryImportOptions {
                title: "Import".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Insert('a'));
        state.handle_input(InputEvent::Insert('b'));
        state.handle_input(InputEvent::Backspace);
        assert_eq!(state.url, "a");
    }
}
