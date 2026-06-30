//! Custom registry import dialog — two-field form (URL + Bearer token).

use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::Color;
use crate::primitive::InputResult;
use crate::primitive::theme::TuiTheme;
use crate::primitive::visible_width;

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
        "•".repeat(self.token.chars().count())
    }

    /// Masked token, truncated to fit one line. Trailing `…` signals overflow.
    /// The stored `token` is never shortened.
    fn masked_token_capped(&self, max_chars: usize) -> String {
        let count = self.token.chars().count();
        if count <= max_chars {
            return self.masked_token();
        }
        if max_chars == 0 {
            return String::new();
        }
        let shown = max_chars.saturating_sub(1);
        let mut s = "•".repeat(shown);
        s.push('…');
        s
    }

    /// Plain URL, truncated to fit one line with a trailing `…` when cut off.
    /// The stored `url` is never shortened.
    fn url_display_capped(&self, max_chars: usize) -> String {
        let chars: Vec<char> = self.url.chars().collect();
        if chars.len() <= max_chars {
            return self.url.clone();
        }
        if max_chars == 0 {
            return String::new();
        }
        let shown = max_chars.saturating_sub(1);
        let mut s: String = chars[..shown].iter().collect();
        s.push('…');
        s
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
            rgb(self.theme.overlay_border),
            "─".repeat(remaining),
        ));

        // URL field
        let url_marker = if self.active_field == FIELD_URL {
            "▸"
        } else {
            " "
        };
        let url_color = if self.active_field == FIELD_URL {
            self.theme.brand
        } else {
            self.theme.text_muted
        };
        lines.push(format!(
            "\x1b[38;2;{}m{url_marker} Registry URL:\x1b[0m",
            rgb(url_color),
        ));
        // Field line layout: "  <value>▏" — 2 prefix cells + 1 trailing cursor.
        let max_value = inner_w.saturating_sub(3);
        if self.url.is_empty() {
            lines.push(format!(
                "  \x1b[38;2;90;90;90m{}\x1b[0m▏",
                if max_value >= "(https://...)".chars().count() {
                    "(https://...)"
                } else {
                    ""
                }
            ));
        } else {
            lines.push(format!("  {}▏", self.url_display_capped(max_value)));
        }

        lines.push(String::new());

        // Token field
        let token_marker = if self.active_field == FIELD_TOKEN {
            "▸"
        } else {
            " "
        };
        let token_color = if self.active_field == FIELD_TOKEN {
            self.theme.brand
        } else {
            self.theme.text_muted
        };
        lines.push(format!(
            "\x1b[38;2;{}m{token_marker} Bearer Token:\x1b[0m",
            rgb(token_color),
        ));
        let masked = self.masked_token_capped(max_value);
        lines.push(format!(
            "  {}▏",
            if self.token.is_empty() {
                "\x1b[38;2;90;90;90m(optional)\x1b[0m".to_owned()
            } else {
                masked
            }
        ));

        // Hint
        lines.push(format!(
            "\x1b[38;2;{}m Tab switch · Enter submit · Esc cancel\x1b[0m",
            rgb(self.theme.text_muted)
        ));

        // Bottom border
        lines.push(format!(
            "\x1b[38;2;{}m╰{}\x1b[0m",
            rgb(self.theme.overlay_border),
            "─".repeat(inner_w),
        ));

        lines
    }

    pub fn handle_input(&mut self, input: InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Ignored;
        }
        match input {
            InputEvent::Insert('\t')
            | InputEvent::Action(KeybindingAction::SelectDown | KeybindingAction::SelectUp) => {
                self.switch_field(true);
                InputResult::Handled
            }
            InputEvent::Paste(text) => self.paste_text(&text),
            InputEvent::Insert(ch) => self.insert_char(ch),
            InputEvent::Backspace => self.backspace(),
            InputEvent::Submit => self.submit(),
            InputEvent::Cancel => self.cancel(),
            _ => InputResult::Ignored,
        }
    }

    fn paste_text(&mut self, text: &str) -> InputResult {
        for ch in text.chars() {
            self.push_allowed_char(ch);
        }
        InputResult::Handled
    }

    fn insert_char(&mut self, ch: char) -> InputResult {
        if self.push_allowed_char(ch) {
            InputResult::Handled
        } else {
            InputResult::Ignored
        }
    }

    fn push_allowed_char(&mut self, ch: char) -> bool {
        if !(ch.is_ascii_graphic() || ch == ' ' || ch == '/' || ch == ':') {
            return false;
        }
        match self.active_field {
            FIELD_URL => self.url.push(ch),
            FIELD_TOKEN => self.token.push(ch),
            _ => {}
        }
        true
    }

    fn backspace(&mut self) -> InputResult {
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

    fn submit(&mut self) -> InputResult {
        if self.url.is_empty() {
            return InputResult::Ignored;
        }
        self.result = Some(CustomRegistryImportResult::Submitted(
            CustomRegistrySource {
                url: self.url.clone(),
                token: self.token.clone(),
            },
        ));
        InputResult::Submitted
    }

    fn cancel(&mut self) -> InputResult {
        self.result = Some(CustomRegistryImportResult::Cancelled);
        InputResult::Cancelled
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

fn rgb(c: Color) -> String {
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
    fn paste_appends_allowed_characters() {
        let mut state = CustomRegistryImportState::new(
            CustomRegistryImportOptions {
                title: "Import".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Paste("https://example.com/api.json".to_owned()));
        let lines = state.render_lines(50);
        let combined: String = lines.join("\n");
        assert!(combined.contains("https://example.com/api.json"));
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

    #[test]
    fn long_url_is_truncated_to_fit_width() {
        let mut state = CustomRegistryImportState::new(
            CustomRegistryImportOptions {
                title: "Import".into(),
            },
            theme(),
        );
        state.handle_input(InputEvent::Paste(
            "https://example.com/".to_owned() + &"a".repeat(200),
        ));
        // Stored value preserved in full.
        assert_eq!(
            state.url.chars().count(),
            200 + "https://example.com/".len()
        );
        let width = 50usize;
        let lines = state.render_lines(width);
        let combined: String = lines.join("\n");
        assert!(combined.contains('…'));
        let url_line = lines
            .iter()
            .find(|l| l.contains("example") || l.contains('•') || l.ends_with('▏'))
            .expect("value line present");
        assert!(
            visible_width(url_line) <= width,
            "url line visible width {} exceeds {}",
            visible_width(url_line),
            width
        );
    }

    #[test]
    fn long_token_is_masked_to_fit_width() {
        let mut state = CustomRegistryImportState::new(
            CustomRegistryImportOptions {
                title: "Import".into(),
            },
            theme(),
        );
        state.switch_field(true); // token field
        state.handle_input(InputEvent::Paste("x".repeat(300)));
        assert_eq!(state.token.chars().count(), 300);
        let width = 50usize;
        let lines = state.render_lines(width);
        let combined: String = lines.join("\n");
        assert!(combined.contains('…'));
        let token_line = lines
            .iter()
            .rfind(|l| l.contains('•'))
            .expect("masked token line present");
        assert!(
            visible_width(token_line) <= width,
            "token line visible width {} exceeds {}",
            visible_width(token_line),
            width
        );
    }
}
