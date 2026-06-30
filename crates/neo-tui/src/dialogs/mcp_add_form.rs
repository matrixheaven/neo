//! Single-page form for adding an MCP server.

use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::Color;
use crate::primitive::InputResult;
use crate::primitive::theme::TuiTheme;
use crate::primitive::{truncate_width, visible_width};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAddFormOptions {
    pub title: String,
    pub transport: String,
}

/// Payload produced when the user submits the MCP add form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAddFormData {
    pub name: String,
    pub command: Option<String>,
    pub url: Option<String>,
    pub bearer_token: Option<String>,
    pub headers: Vec<String>,
    pub env: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpAddFormResult {
    Submitted(McpAddFormData),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    label: &'static str,
    buffer: String,
    optional: bool,
    masked: bool,
    multiline: bool,
}

// Field indices shared across transports. The field at index 2 means Env for
// stdio and Bearer Token for http/sse; index 3 is only present for http/sse.
const FIELD_NAME: usize = 0;
const FIELD_COMMAND_OR_URL: usize = 1;
const FIELD_OPTIONAL_1: usize = 2;
const FIELD_HEADERS: usize = 3;

/// Split a raw env/headers buffer into individual `KEY=value` entries.
/// Supports commas and newlines as separators.
fn split_key_value_entries(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c == '\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAddFormState {
    fields: Vec<Field>,
    active_field: usize,
    transport: String,
    title: String,
    theme: TuiTheme,
    result: Option<McpAddFormResult>,
}

impl McpAddFormState {
    #[must_use]
    pub fn new(opts: McpAddFormOptions, theme: TuiTheme) -> Self {
        let fields = match opts.transport.as_str() {
            "stdio" => vec![
                Field {
                    label: "Name",
                    buffer: String::new(),
                    optional: false,
                    masked: false,
                    multiline: false,
                },
                Field {
                    label: "Command",
                    buffer: String::new(),
                    optional: false,
                    masked: false,
                    multiline: false,
                },
                Field {
                    label: "Env",
                    buffer: String::new(),
                    optional: true,
                    masked: false,
                    multiline: true,
                },
            ],
            _ => vec![
                Field {
                    label: "Name",
                    buffer: String::new(),
                    optional: false,
                    masked: false,
                    multiline: false,
                },
                Field {
                    label: "URL",
                    buffer: String::new(),
                    optional: false,
                    masked: false,
                    multiline: false,
                },
                Field {
                    label: "Bearer Token",
                    buffer: String::new(),
                    optional: true,
                    masked: true,
                    multiline: false,
                },
                Field {
                    label: "Headers",
                    buffer: String::new(),
                    optional: true,
                    masked: false,
                    multiline: true,
                },
            ],
        };

        Self {
            fields,
            active_field: 0,
            transport: opts.transport,
            title: opts.title,
            theme,
            result: None,
        }
    }

    fn switch_field(&mut self, forward: bool) {
        let count = self.fields.len();
        if count == 0 {
            return;
        }
        if forward {
            self.active_field = (self.active_field + 1) % count;
        } else {
            self.active_field = (self.active_field + count - 1) % count;
        }
    }

    fn active_buffer_mut(&mut self) -> Option<&mut String> {
        self.fields
            .get_mut(self.active_field)
            .map(|f| &mut f.buffer)
    }

    fn push_allowed_char(&mut self, ch: char) -> bool {
        let multiline = self
            .fields
            .get(self.active_field)
            .is_some_and(|f| f.multiline);
        if ch.is_control() && !(multiline && ch == '\n') {
            return false;
        }
        if let Some(buffer) = self.active_buffer_mut() {
            buffer.push(ch);
            true
        } else {
            false
        }
    }

    fn paste_text(&mut self, text: &str) -> InputResult {
        for ch in text.chars() {
            let _ = self.push_allowed_char(ch);
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

    fn backspace(&mut self) -> InputResult {
        if let Some(buffer) = self.active_buffer_mut() {
            buffer.pop();
        }
        InputResult::Handled
    }

    fn can_submit(&self) -> bool {
        if self
            .fields
            .get(FIELD_NAME)
            .is_none_or(|f| f.buffer.is_empty())
        {
            return false;
        }
        if self
            .fields
            .get(FIELD_COMMAND_OR_URL)
            .is_none_or(|f| f.buffer.is_empty())
        {
            return false;
        }
        true
    }

    fn submit(&mut self) -> InputResult {
        if !self.can_submit() {
            return InputResult::Ignored;
        }

        let name = self.fields[FIELD_NAME].buffer.clone();
        let second = self.fields[FIELD_COMMAND_OR_URL].buffer.clone();
        let optional = |idx: usize| {
            self.fields
                .get(idx)
                .filter(|f| !f.buffer.is_empty())
                .map(|f| f.buffer.clone())
        };
        let optional_vec = |idx: usize| {
            self.fields
                .get(idx)
                .map(|f| split_key_value_entries(&f.buffer))
                .unwrap_or_default()
        };

        let (command, url, bearer_token, headers, env) = match self.transport.as_str() {
            "stdio" => (
                Some(second),
                None,
                None,
                Vec::new(),
                optional_vec(FIELD_OPTIONAL_1),
            ),
            _ => (
                None,
                Some(second),
                optional(FIELD_OPTIONAL_1),
                optional_vec(FIELD_HEADERS),
                Vec::new(),
            ),
        };

        self.result = Some(McpAddFormResult::Submitted(McpAddFormData {
            name,
            command,
            url,
            bearer_token,
            headers,
            env,
        }));
        InputResult::Submitted
    }

    fn cancel(&mut self) -> InputResult {
        self.result = Some(McpAddFormResult::Cancelled);
        InputResult::Cancelled
    }

    /// Render the form as a sequence of ANSI strings, each fitting within `width` columns.
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

        let max_value = inner_w.saturating_sub(3);

        for (index, field) in self.fields.iter().enumerate() {
            let marker = if index == self.active_field {
                "▸"
            } else {
                " "
            };
            let color = if index == self.active_field {
                self.theme.brand
            } else {
                self.theme.text_muted
            };
            lines.push(format!(
                "\x1b[38;2;{}m{marker} {}:\x1b[0m",
                rgb(color),
                field.label,
            ));

            let value_line = if field.buffer.is_empty() {
                let placeholder = if field.optional { "(optional)" } else { "" };
                if placeholder.is_empty() || max_value == 0 {
                    format!("  {}▏", truncate_width(placeholder, max_value, "…", false))
                } else {
                    format!(
                        "  \x1b[38;2;90;90;90m{}\x1b[0m▏",
                        truncate_width(placeholder, max_value, "…", false)
                    )
                }
            } else if field.masked {
                let masked = masked_value(&field.buffer, max_value);
                format!("  {masked}▏")
            } else {
                let display = truncate_width(&field.buffer, max_value, "…", false);
                format!("  {display}▏")
            };
            lines.push(value_line);

            // Blank line between fields, but not after the last one.
            if index + 1 < self.fields.len() {
                lines.push(String::new());
            }
        }

        // Hint
        lines.push(format!(
            "\x1b[38;2;{}m Tab · ↑↓ switch · Enter submit · Esc cancel\x1b[0m",
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
            InputEvent::Insert('\t') | InputEvent::Action(KeybindingAction::SelectDown) => {
                self.switch_field(true);
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectUp) => {
                self.switch_field(false);
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

    #[must_use]
    pub fn result(&self) -> Option<&McpAddFormResult> {
        self.result.as_ref()
    }

    #[must_use]
    pub fn take_result(&mut self) -> Option<McpAddFormResult> {
        self.result.take()
    }
}

fn masked_value(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars || max_chars == 0 {
        return "•".repeat(count.min(max_chars));
    }
    let shown = max_chars.saturating_sub(1);
    let mut s = "•".repeat(shown);
    s.push('…');
    s
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

    fn stdio_state() -> McpAddFormState {
        McpAddFormState::new(
            McpAddFormOptions {
                title: "Add MCP Server".into(),
                transport: "stdio".into(),
            },
            theme(),
        )
    }

    fn http_state() -> McpAddFormState {
        McpAddFormState::new(
            McpAddFormOptions {
                title: "Add MCP Server".into(),
                transport: "http".into(),
            },
            theme(),
        )
    }

    #[test]
    fn stdio_starts_on_name_field() {
        let state = stdio_state();
        assert_eq!(state.active_field, 0);
        let lines = state.render_lines(60);
        assert!(lines.join("\n").contains("▸ Name:"));
    }

    #[test]
    fn tab_switches_fields_forward() {
        let mut state = stdio_state();
        state.handle_input(InputEvent::Insert('\t'));
        assert_eq!(state.active_field, 1);
        state.handle_input(InputEvent::Insert('\t'));
        assert_eq!(state.active_field, 2);
        state.handle_input(InputEvent::Insert('\t'));
        assert_eq!(state.active_field, 0);
    }

    #[test]
    fn arrow_keys_switch_fields() {
        let mut state = http_state();
        assert_eq!(state.active_field, 0);
        state.handle_input(InputEvent::Action(KeybindingAction::SelectDown));
        assert_eq!(state.active_field, 1);
        state.handle_input(InputEvent::Action(KeybindingAction::SelectUp));
        assert_eq!(state.active_field, 0);
        // Wrap up from first field to last.
        state.handle_input(InputEvent::Action(KeybindingAction::SelectUp));
        assert_eq!(state.active_field, 3);
    }

    #[test]
    fn typing_appends_to_active_field() {
        let mut state = stdio_state();
        for ch in "my-server".chars() {
            state.handle_input(InputEvent::Insert(ch));
        }
        let lines = state.render_lines(60);
        assert!(lines.join("\n").contains("my-server"));
    }

    #[test]
    fn paste_inserts_text_into_active_field() {
        let mut state = stdio_state();
        state.handle_input(InputEvent::Paste("npx -y @server/filesystem".to_owned()));
        let lines = state.render_lines(80);
        assert!(lines.join("\n").contains("npx -y @server/filesystem"));
    }

    #[test]
    fn backspace_removes_last_character() {
        let mut state = stdio_state();
        state.handle_input(InputEvent::Paste("abc".to_owned()));
        state.handle_input(InputEvent::Backspace);
        let lines = state.render_lines(60);
        assert!(!lines.join("\n").contains("abc"));
        assert!(lines.join("\n").contains("ab"));
    }

    #[test]
    fn optional_fields_show_placeholder_when_empty() {
        let mut state = stdio_state();
        state.handle_input(InputEvent::Insert('\t'));
        state.handle_input(InputEvent::Insert('\t'));
        let lines = state.render_lines(60);
        assert!(lines.join("\n").contains("(optional)"));
    }

    #[test]
    fn bearer_token_is_masked() {
        let mut state = http_state();
        state.handle_input(InputEvent::Insert('\t')); // URL
        state.handle_input(InputEvent::Insert('\t')); // Bearer Token
        state.handle_input(InputEvent::Paste("secret-token".to_owned()));
        let lines = state.render_lines(60);
        let combined = lines.join("\n");
        assert!(combined.contains("••••••••••••"));
        assert!(!combined.contains("secret-token"));
    }

    #[test]
    fn submit_stdio_returns_correct_data() {
        let mut state = stdio_state();
        state.handle_input(InputEvent::Paste("fs".to_owned()));
        state.handle_input(InputEvent::Insert('\t'));
        state.handle_input(InputEvent::Paste("npx -y @server/filesystem".to_owned()));
        state.handle_input(InputEvent::Insert('\t'));
        state.handle_input(InputEvent::Paste("KEY=value".to_owned()));
        let result = state.handle_input(InputEvent::Submit);
        assert!(matches!(result, InputResult::Submitted));
        match state.take_result() {
            Some(McpAddFormResult::Submitted(data)) => {
                assert_eq!(data.name, "fs");
                assert_eq!(data.command, Some("npx -y @server/filesystem".to_owned()));
                assert!(data.url.is_none());
                assert_eq!(data.env, vec!["KEY=value".to_owned()]);
                assert!(data.headers.is_empty());
            }
            other => panic!("expected submitted result, got {other:?}"),
        }
    }

    #[test]
    fn submit_http_returns_correct_data() {
        let mut state = http_state();
        state.handle_input(InputEvent::Paste("linear".to_owned()));
        state.handle_input(InputEvent::Insert('\t'));
        state.handle_input(InputEvent::Paste("https://example.invalid/mcp".to_owned()));
        state.handle_input(InputEvent::Insert('\t'));
        state.handle_input(InputEvent::Paste("tok".to_owned()));
        state.handle_input(InputEvent::Insert('\t'));
        state.handle_input(InputEvent::Paste("Authorization=bearer".to_owned()));
        assert!(matches!(
            state.handle_input(InputEvent::Submit),
            InputResult::Submitted
        ));
        match state.take_result() {
            Some(McpAddFormResult::Submitted(data)) => {
                assert_eq!(data.name, "linear");
                assert_eq!(data.url, Some("https://example.invalid/mcp".to_owned()));
                assert_eq!(data.bearer_token, Some("tok".to_owned()));
                assert_eq!(data.headers, vec!["Authorization=bearer".to_owned()]);
                assert!(data.command.is_none());
                assert!(data.env.is_empty());
            }
            other => panic!("expected submitted result, got {other:?}"),
        }
    }

    #[test]
    fn submit_splits_env_and_headers_by_comma_and_newline() {
        let mut state = http_state();
        state.handle_input(InputEvent::Paste("linear".to_owned()));
        state.handle_input(InputEvent::Insert('\t'));
        state.handle_input(InputEvent::Paste("https://example.invalid/mcp".to_owned()));
        state.handle_input(InputEvent::Insert('\t'));
        state.handle_input(InputEvent::Insert('\t'));
        state.handle_input(InputEvent::Paste("A=1,B=2\nC=3".to_owned()));
        assert!(matches!(
            state.handle_input(InputEvent::Submit),
            InputResult::Submitted
        ));
        match state.take_result() {
            Some(McpAddFormResult::Submitted(data)) => {
                assert_eq!(
                    data.headers,
                    vec!["A=1".to_owned(), "B=2".to_owned(), "C=3".to_owned()]
                );
            }
            other => panic!("expected submitted result, got {other:?}"),
        }
    }

    #[test]
    fn submit_requires_name_and_command() {
        let mut state = stdio_state();
        assert!(matches!(
            state.handle_input(InputEvent::Submit),
            InputResult::Ignored
        ));
        state.handle_input(InputEvent::Paste("name".to_owned()));
        assert!(matches!(
            state.handle_input(InputEvent::Submit),
            InputResult::Ignored
        ));
    }

    #[test]
    fn cancel_cancels() {
        let mut state = stdio_state();
        state.handle_input(InputEvent::Paste("name".to_owned()));
        assert!(matches!(
            state.handle_input(InputEvent::Cancel),
            InputResult::Cancelled
        ));
        assert!(matches!(
            state.take_result(),
            Some(McpAddFormResult::Cancelled)
        ));
    }

    #[test]
    fn masked_token_fits_width() {
        let mut state = http_state();
        state.handle_input(InputEvent::Insert('\t'));
        state.handle_input(InputEvent::Insert('\t'));
        state.handle_input(InputEvent::Paste("x".repeat(200)));
        let width = 40usize;
        let lines = state.render_lines(width);
        let token_line = lines
            .iter()
            .find(|l| l.contains('•'))
            .expect("masked token line present");
        assert!(
            visible_width(token_line) <= width,
            "token line width {} exceeds {}",
            visible_width(token_line),
            width
        );
    }
}
