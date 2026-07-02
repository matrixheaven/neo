//! Help panel dialog for keyboard shortcuts and slash commands.

use std::cmp::Ordering;

use crate::input::InputEvent;
use crate::input::KeybindingAction;
use crate::primitive::theme::TuiTheme;
use crate::primitive::visible_width;
use crate::primitive::Color;
use crate::primitive::InputResult;

use super::choice_picker::dialog_rgb;

const DEFAULT_VIEWPORT_HEIGHT: usize = 14;
const MIN_VIEWPORT_HEIGHT: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpPanelCommand {
    pub value: String,
    pub description: Option<String>,
}

impl HelpPanelCommand {
    #[must_use]
    pub fn new(value: impl Into<String>, description: Option<impl Into<String>>) -> Self {
        Self {
            value: value.into(),
            description: description.map(Into::into),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HelpPanelOptions {
    pub commands: Vec<HelpPanelCommand>,
    pub theme: TuiTheme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpPanelState {
    commands: Vec<HelpPanelCommand>,
    theme: TuiTheme,
    scroll_offset: usize,
    viewport_height: usize,
}

impl HelpPanelState {
    #[must_use]
    pub fn new(opts: HelpPanelOptions) -> Self {
        let mut commands = opts.commands;
        commands.sort_by(compare_commands);
        Self {
            commands,
            theme: opts.theme,
            scroll_offset: 0,
            viewport_height: DEFAULT_VIEWPORT_HEIGHT,
        }
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        let inner_w = width.saturating_sub(2).max(1);
        let mut lines = Vec::new();
        let content = self.content_lines();
        let viewport_height = self.viewport_height.max(MIN_VIEWPORT_HEIGHT);
        let max_scroll = content.len().saturating_sub(viewport_height);
        let scroll_offset = self.scroll_offset.min(max_scroll);
        let visible_end = (scroll_offset + viewport_height).min(content.len());

        let title = " help ";
        let remaining = inner_w.saturating_sub(visible_width(title));
        lines.push(format!(
            "\x1b[38;2;{}m╭{title}{}╮\x1b[0m",
            dialog_rgb(self.theme.overlay_border),
            "─".repeat(remaining),
        ));

        for line in &content[scroll_offset..visible_end] {
            lines.push(self.bordered_line(line, inner_w));
        }

        if content.len() > viewport_height {
            let hint = format!(
                "{} {}/{}",
                paint("scroll", self.theme.text_muted),
                scroll_offset + 1,
                max_scroll + 1
            );
            lines.push(self.bordered_line(&hint, inner_w));
        }

        lines.push(format!(
            "\x1b[38;2;{}m╰{}╯\x1b[0m",
            dialog_rgb(self.theme.overlay_border),
            "─".repeat(inner_w),
        ));
        lines
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        match input {
            InputEvent::Action(KeybindingAction::SelectUp) => {
                self.scroll_up(1);
                InputResult::Handled
            }
            InputEvent::ScrollUp(rows) => {
                self.scroll_up(*rows);
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectDown) => {
                self.scroll_down(1);
                InputResult::Handled
            }
            InputEvent::ScrollDown(rows) => {
                self.scroll_down(*rows);
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageUp) | InputEvent::MoveLeft => {
                self.scroll_up(self.page_size());
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageDown) | InputEvent::MoveRight => {
                self.scroll_down(self.page_size());
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectConfirm) | InputEvent::Submit => {
                InputResult::Submitted
            }
            InputEvent::Action(KeybindingAction::SelectCancel)
            | InputEvent::Cancel
            | InputEvent::Insert('q' | 'Q') => InputResult::Cancelled,
            _ => InputResult::Ignored,
        }
    }

    #[must_use]
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    #[must_use]
    pub fn viewport_height(&self) -> usize {
        self.viewport_height
    }

    fn page_size(&self) -> usize {
        self.viewport_height.max(MIN_VIEWPORT_HEIGHT)
    }

    fn scroll_up(&mut self, rows: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(rows);
    }

    fn scroll_down(&mut self, rows: usize) {
        self.scroll_offset = self
            .scroll_offset
            .saturating_add(rows)
            .min(self.max_scroll_offset());
    }

    fn max_scroll_offset(&self) -> usize {
        self.content_lines().len().saturating_sub(self.page_size())
    }

    fn content_lines(&self) -> Vec<String> {
        let mut lines = vec![
            paint(
                "help · Esc / Enter / q close · ↑↓ scroll",
                self.theme.text_muted,
            ),
            String::new(),
            paint("Keyboard", self.theme.brand),
            shortcut_line("Shift+Tab", "Cycle development mode", &self.theme),
            shortcut_line(
                "Ctrl+S",
                "Steer or queue while a turn is running",
                &self.theme,
            ),
            shortcut_line("Alt+Up", "Edit the last queued message", &self.theme),
            shortcut_line("Esc", "Close dialog or cancel selection", &self.theme),
            String::new(),
            paint("Slash Commands", self.theme.brand),
        ];

        if self.commands.is_empty() {
            lines.push(format!(
                "  {}",
                paint("No slash commands available", self.theme.text_muted)
            ));
        } else {
            lines.extend(
                self.commands
                    .iter()
                    .map(|command| command_line(command, &self.theme)),
            );
        }

        lines
    }

    fn bordered_line(&self, line: &str, inner_w: usize) -> String {
        let clipped = truncate_styled_to_width(line, inner_w.saturating_sub(2));
        let padding = inner_w.saturating_sub(visible_width(&clipped) + 1);
        format!(
            "\x1b[38;2;{}m│\x1b[0m {clipped}{}\x1b[38;2;{}m│\x1b[0m",
            dialog_rgb(self.theme.overlay_border),
            " ".repeat(padding),
            dialog_rgb(self.theme.overlay_border),
        )
    }
}

fn compare_commands(left: &HelpPanelCommand, right: &HelpPanelCommand) -> Ordering {
    match (
        left.value.starts_with("/skill:"),
        right.value.starts_with("/skill:"),
    ) {
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => left.value.cmp(&right.value),
    }
}

fn command_line(command: &HelpPanelCommand, theme: &TuiTheme) -> String {
    let label = label_cell(&command.value, theme.text_primary);
    match command
        .description
        .as_deref()
        .filter(|description| !description.is_empty())
    {
        Some(description) => format!("  {} {}", label, paint(description, theme.text_muted)),
        None => format!("  {label}"),
    }
}

fn shortcut_line(key: &str, description: &str, theme: &TuiTheme) -> String {
    let label = label_cell(key, theme.text_primary);
    format!("  {} {}", label, paint(description, theme.text_muted))
}

fn label_cell(label: &str, color: Color) -> String {
    let padding = 18usize.saturating_sub(visible_width(label));
    format!("{}{}", paint(label, color), " ".repeat(padding))
}

fn paint(text: &str, color: Color) -> String {
    format!("\x1b[38;2;{}m{text}\x1b[0m", dialog_rgb(color))
}

fn truncate_styled_to_width(text: &str, max_width: usize) -> String {
    if visible_width(text) <= max_width {
        return text.to_owned();
    }

    let mut out = String::new();
    let mut visible = 0usize;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            out.push(ch);
            out.push(chars.next().expect("peeked CSI introducer"));
            for code_ch in chars.by_ref() {
                out.push(code_ch);
                if code_ch == 'm' {
                    break;
                }
            }
            continue;
        }

        let ch_width = visible_width(&ch.to_string());
        if visible + ch_width > max_width.saturating_sub(1) {
            out.push('…');
            break;
        }
        out.push(ch);
        visible += ch_width;
    }

    if !out.ends_with("\x1b[0m") {
        out.push_str("\x1b[0m");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::KeybindingAction;

    fn theme() -> TuiTheme {
        TuiTheme::default()
    }

    fn strip_ansi(input: &str) -> String {
        let mut stripped = String::new();
        let mut chars = input.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' && chars.peek() == Some(&'[') {
                let _ = chars.next();
                for code_ch in chars.by_ref() {
                    if code_ch == 'm' {
                        break;
                    }
                }
            } else {
                stripped.push(ch);
            }
        }
        stripped
    }

    #[test]
    fn help_panel_renders_shortcuts_commands_and_skill_commands() {
        let state = HelpPanelState::new(HelpPanelOptions {
            commands: vec![
                HelpPanelCommand::new("/model", Some("Choose model")),
                HelpPanelCommand::new("/empty", None::<String>),
                HelpPanelCommand::new("/skill:rust", Some("Use Rust skill")),
            ],
            theme: theme(),
        });

        let rendered = strip_ansi(&state.render_lines(72).join("\n"));

        assert!(rendered.contains("help · Esc / Enter / q close · ↑↓ scroll"));
        assert!(rendered.contains("Keyboard"));
        assert!(rendered.contains("Shift+Tab"));
        assert!(rendered.contains("Ctrl+S"));
        assert!(rendered.contains("Alt+Up"));
        assert!(rendered.contains("Esc"));
        assert!(rendered.contains("Slash Commands"));
        assert!(rendered.contains("/model"));
        assert!(rendered.contains("Choose model"));
        assert!(rendered.contains("/empty"));
        assert!(rendered.contains("/skill:rust"));
        assert!(rendered.contains("Use Rust skill"));
    }

    #[test]
    fn help_panel_sorts_skill_commands_after_regular_commands() {
        let state = HelpPanelState::new(HelpPanelOptions {
            commands: vec![
                HelpPanelCommand::new("/skill:z", Some("Zed")),
                HelpPanelCommand::new("/yolo", Some("Yolo")),
                HelpPanelCommand::new("/ask", Some("Ask")),
                HelpPanelCommand::new("/skill:a", Some("Aye")),
            ],
            theme: theme(),
        });

        let rendered = strip_ansi(&state.render_lines(72).join("\n"));
        let ask = rendered.find("/ask").expect("/ask should render");
        let yolo = rendered.find("/yolo").expect("/yolo should render");
        let skill_a = rendered.find("/skill:a").expect("/skill:a should render");
        let skill_z = rendered.find("/skill:z").expect("/skill:z should render");

        assert!(ask < yolo);
        assert!(yolo < skill_a);
        assert!(skill_a < skill_z);
    }

    #[test]
    fn help_panel_frame_lines_have_consistent_width() {
        let state = HelpPanelState::new(HelpPanelOptions {
            commands: vec![
                HelpPanelCommand::new("/help", Some("Show help information")),
                HelpPanelCommand::new("/skill:rust", Some("Use Rust skill")),
            ],
            theme: theme(),
        });

        let rendered = state.render_lines(52);
        let widths = rendered
            .iter()
            .map(|line| visible_width(&strip_ansi(line)))
            .collect::<Vec<_>>();
        let Some(expected_width) = widths.first() else {
            panic!("help panel should render lines");
        };

        assert!(
            widths.iter().all(|width| width == expected_width),
            "expected every help panel line to be {expected_width} cols, got {widths:?}\n{}",
            strip_ansi(&rendered.join("\n"))
        );
    }

    #[test]
    fn help_panel_scrolls_and_closes() {
        let commands = (0..16)
            .map(|index| {
                HelpPanelCommand::new(format!("/cmd{index:02}"), Some(format!("Command {index}")))
            })
            .collect();
        let mut state = HelpPanelState::new(HelpPanelOptions {
            commands,
            theme: theme(),
        });

        let first = strip_ansi(&state.render_lines(52).join("\n"));
        assert!(first.contains("/cmd00"));
        assert!(!first.contains("/cmd15"));

        for _ in 0..10 {
            assert_eq!(
                state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown)),
                InputResult::Handled
            );
        }
        assert_eq!(state.scroll_offset(), 10);
        let scrolled = strip_ansi(&state.render_lines(52).join("\n"));
        assert!(scrolled.contains("scroll"));
        assert!(!scrolled.contains("/cmd00"));

        assert_eq!(
            state.handle_input(&InputEvent::ScrollUp(4)),
            InputResult::Handled
        );
        assert_eq!(state.scroll_offset(), 6);
        assert_eq!(
            state.handle_input(&InputEvent::ScrollDown(3)),
            InputResult::Handled
        );
        assert_eq!(state.scroll_offset(), 9);

        assert_eq!(
            state.handle_input(&InputEvent::Action(KeybindingAction::SelectPageDown)),
            InputResult::Handled
        );
        let paged = strip_ansi(&state.render_lines(52).join("\n"));
        assert!(paged.contains("/cmd15"));

        assert_eq!(
            state.handle_input(&InputEvent::Submit),
            InputResult::Submitted
        );

        let mut state = HelpPanelState::new(HelpPanelOptions {
            commands: Vec::new(),
            theme: theme(),
        });
        assert_eq!(
            state.handle_input(&InputEvent::Action(KeybindingAction::SelectCancel)),
            InputResult::Cancelled
        );

        let mut state = HelpPanelState::new(HelpPanelOptions {
            commands: Vec::new(),
            theme: theme(),
        });
        assert_eq!(
            state.handle_input(&InputEvent::Insert('q')),
            InputResult::Cancelled
        );
        let mut state = HelpPanelState::new(HelpPanelOptions {
            commands: Vec::new(),
            theme: theme(),
        });
        assert_eq!(
            state.handle_input(&InputEvent::Insert('Q')),
            InputResult::Cancelled
        );
    }
}
