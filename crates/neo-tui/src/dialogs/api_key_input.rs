//! API key input dialog — masked single-field input.

use crate::ansi::Color;
use crate::chrome::TuiTheme;
use crate::components::visible_width;
use crate::core::InputResult;
use crate::input::InputEvent;

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
        "•".repeat(self.value.chars().count())
    }

    /// Masked value, truncated to fit one line of the input field.
    ///
    /// The stored `value` is never shortened — only the masked glyphs shown to
    /// the user are capped so a long pasted key does not overflow the fixed
    /// dialog height. When truncation happens a trailing `…` signals that more
    /// characters are held than are displayed.
    fn masked_display_capped(&self, max_chars: usize) -> String {
        let count = self.value.chars().count();
        if count <= max_chars {
            return self.masked_display();
        }
        if max_chars == 0 {
            return String::new();
        }
        // Reserve one cell for the ellipsis indicator.
        let shown = max_chars.saturating_sub(1);
        let mut s = "•".repeat(shown);
        s.push('…');
        s
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

        // Provider name hint
        lines.push(format!(
            "\x1b[38;2;{}m Provider: {}\x1b[0m",
            rgb(self.theme.text_muted),
            self.provider_name
        ));
        lines.push(String::new());

        // Input field. The masked value is capped to the available width so a
        // long pasted key does not blow past the fixed dialog height; the
        // stored value itself is untouched.
        // Layout: " API Key: <masked>▏" — 10 prefix cells + 1 trailing cursor.
        let max_masked = inner_w.saturating_sub(11);
        let masked = self.masked_display_capped(max_masked);
        lines.push(format!(
            "\x1b[38;2;{}m API Key: \x1b[0m{masked}▏",
            rgb(self.theme.text_primary)
        ));

        // Hint
        lines.push(format!(
            "\x1b[38;2;{}m Enter submit · Esc cancel\x1b[0m",
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

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Ignored;
        }
        match input {
            InputEvent::Insert(ch) if ch.is_ascii_graphic() || *ch == ' ' => {
                self.value.push(*ch);
                InputResult::Handled
            }
            InputEvent::Paste(text) => self.paste_text(text),
            InputEvent::Backspace => {
                self.value.pop();
                InputResult::Handled
            }
            InputEvent::Submit => {
                if self.value.is_empty() {
                    InputResult::Ignored
                } else {
                    self.result = Some(ApiKeyInputResult::Submitted(self.value.clone()));
                    InputResult::Submitted
                }
            }
            InputEvent::Cancel => {
                self.result = Some(ApiKeyInputResult::Cancelled);
                InputResult::Cancelled
            }
            _ => InputResult::Ignored,
        }
    }

    fn paste_text(&mut self, text: &str) -> InputResult {
        let trimmed = text.trim_matches(|c: char| c.is_whitespace());
        if trimmed.is_empty() {
            return InputResult::Ignored;
        }
        for ch in trimmed.chars() {
            if ch.is_ascii_graphic() {
                self.value.push(ch);
            }
        }
        InputResult::Handled
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
    fn typing_appends_and_masks() {
        let mut state = ApiKeyInputState::new(
            ApiKeyInputOptions {
                title: "API Key".into(),
                provider_name: "OpenAI".into(),
            },
            theme(),
        );
        state.handle_input(&InputEvent::Insert('s'));
        state.handle_input(&InputEvent::Insert('k'));
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
        state.handle_input(&InputEvent::Insert('a'));
        state.handle_input(&InputEvent::Insert('b'));
        state.handle_input(&InputEvent::Backspace);
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
        state.handle_input(&InputEvent::Insert('k'));
        state.handle_input(&InputEvent::Submit);
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
        state.handle_input(&InputEvent::Cancel);
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
        state.handle_input(&InputEvent::Submit);
        assert!(state.result.is_none());
    }

    #[test]
    fn paste_inserts_trimmed_value() {
        let mut state = ApiKeyInputState::new(
            ApiKeyInputOptions {
                title: "API Key".into(),
                provider_name: "OpenAI".into(),
            },
            theme(),
        );
        state.handle_input(&InputEvent::Paste("  sk-abcd1234  \n".to_owned()));
        if let Some(_) = state.take_result() {
            panic!("pasting should not submit");
        }
        // Verify the value got the trimmed, non-whitespace characters.
        let mut submit_state = state;
        submit_state.handle_input(&InputEvent::Submit);
        match submit_state
            .take_result()
            .expect("should submit after paste")
        {
            ApiKeyInputResult::Submitted(v) => assert_eq!(v, "sk-abcd1234"),
            ApiKeyInputResult::Cancelled => panic!("expected Submitted"),
        }
    }

    #[test]
    fn paste_strips_newlines_and_whitespace_only_ignored() {
        let mut state = ApiKeyInputState::new(
            ApiKeyInputOptions {
                title: "T".into(),
                provider_name: "P".into(),
            },
            theme(),
        );
        // Whitespace-only paste is ignored.
        assert_eq!(
            state.handle_input(&InputEvent::Paste("   \n\t".to_owned())),
            InputResult::Ignored
        );
        // Paste with embedded newlines keeps only the ascii_graphic chars.
        state.handle_input(&InputEvent::Paste("sk\n-secret".to_owned()));
        let mut submit_state = state;
        submit_state.handle_input(&InputEvent::Submit);
        match submit_state.take_result().unwrap() {
            ApiKeyInputResult::Submitted(v) => assert_eq!(v, "sk-secret"),
            ApiKeyInputResult::Cancelled => panic!("expected Submitted"),
        }
    }

    #[test]
    fn long_pasted_value_is_masked_to_fit_width() {
        let mut state = ApiKeyInputState::new(
            ApiKeyInputOptions {
                title: "API Key".into(),
                provider_name: "OpenAI".into(),
            },
            theme(),
        );
        // Simulate pasting a 200-char key.
        state.handle_input(&InputEvent::Paste("a".repeat(200)));
        // Render at a narrow width: inner = width - 2, prefix " API Key: " = 10,
        // trailing cursor = 1, so masked cap = width - 2 - 11.
        let width = 40usize;
        let lines = state.render_lines(width);
        let combined: String = lines.join("\n");
        // The value is preserved in full even though display is capped.
        assert_eq!(state.value.chars().count(), 200);
        // The displayed line contains an ellipsis indicator and never exceeds
        // the inner width.
        assert!(combined.contains('…'));
        let field_line = lines
            .iter()
            .find(|l| l.contains("API Key:"))
            .expect("field line present");
        let visible = strip_ansi(field_line);
        assert!(
            visible_width(&visible) <= width,
            "field line visible width {} exceeds terminal width {}",
            visible_width(&visible),
            width
        );
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut in_esc = false;
        for ch in s.chars() {
            if in_esc {
                if ch.is_alphabetic() {
                    in_esc = false;
                }
                continue;
            }
            if ch == '\x1b' {
                in_esc = true;
                continue;
            }
            out.push(ch);
        }
        out
    }
}
