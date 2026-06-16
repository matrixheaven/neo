//! Choice picker dialog — simple single-select list.

use crate::ansi::Color;
use crate::components::visible_width;
use crate::{InputEvent, InputResult, TuiTheme};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceItem {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

impl ChoiceItem {
    #[must_use]
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
        }
    }

    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }
}

pub struct ChoicePickerOptions {
    pub title: String,
    pub items: Vec<ChoiceItem>,
    pub initial_id: Option<String>,
    pub theme: TuiTheme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceResult {
    Selected(ChoiceItem),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoicePickerState {
    items: Vec<ChoiceItem>,
    selected: usize,
    theme: TuiTheme,
    title: String,
    result: Option<ChoiceResult>,
}

impl ChoicePickerState {
    #[must_use]
    pub fn new(opts: ChoicePickerOptions) -> Self {
        let selected = opts
            .initial_id
            .as_ref()
            .and_then(|id| opts.items.iter().position(|i| &i.id == id))
            .unwrap_or(0)
            .min(opts.items.len().saturating_sub(1));
        Self {
            items: opts.items,
            selected,
            theme: opts.theme,
            title: opts.title,
            result: None,
        }
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        let inner_w = width.saturating_sub(2).max(1);
        let mut lines = Vec::new();

        // Top border with title
        let title_str = format!(" {} ", self.title);
        let title_len = visible_width(&title_str);
        let remaining = inner_w.saturating_sub(title_len);
        lines.push(format!(
            "\x1b[38;2;{}m╭{title_str}{}\x1b[0m",
            rgb(&self.theme.overlay_border),
            "─".repeat(remaining),
        ));

        // Items
        for (i, item) in self.items.iter().enumerate() {
            let is_selected = i == self.selected;
            let marker = if is_selected { "▸" } else { " " };
            let label = &item.label;

            let (fg, bg) = if is_selected {
                (self.theme.selected_fg, self.theme.selected_bg)
            } else {
                (Color::Reset, Color::Reset)
            };

            let desc_str = item
                .description
                .as_ref()
                .map(|d| format!(" — {d}"))
                .unwrap_or_default();

            lines.push(format!(
                "\x1b[{};{}m {marker} {label}{desc_str}\x1b[0m",
                fg.sgr(),
                bg.sgr_bg()
            ));
        }

        // Hint
        lines.push(format!(
            "\x1b[38;2;{}m ↑↓ navigate · Enter select · Esc cancel\x1b[0m",
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
            InputEvent::Action(crate::KeybindingAction::SelectUp) | InputEvent::ScrollUp(1) => {
                if !self.items.is_empty() && self.selected == 0 {
                    self.selected = self.items.len() - 1;
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                InputResult::Handled
            }
            InputEvent::Action(crate::KeybindingAction::SelectDown) | InputEvent::ScrollDown(1) => {
                if !self.items.is_empty() {
                    self.selected = (self.selected + 1) % self.items.len();
                }
                InputResult::Handled
            }
            InputEvent::Action(crate::KeybindingAction::SelectConfirm) | InputEvent::Submit => {
                if let Some(item) = self.items.get(self.selected).cloned() {
                    self.result = Some(ChoiceResult::Selected(item));
                    InputResult::Submitted
                } else {
                    InputResult::Ignored
                }
            }
            InputEvent::Action(crate::KeybindingAction::SelectCancel) | InputEvent::Cancel => {
                self.result = Some(ChoiceResult::Cancelled);
                InputResult::Cancelled
            }
            _ => InputResult::Ignored,
        }
    }

    #[must_use]
    pub fn result(&self) -> Option<&ChoiceResult> {
        self.result.as_ref()
    }

    #[must_use]
    pub fn take_result(&mut self) -> Option<ChoiceResult> {
        self.result.take()
    }
}

trait ColorSgr {
    fn sgr(&self) -> String;
    fn sgr_bg(&self) -> String;
}

impl ColorSgr for Color {
    fn sgr(&self) -> String {
        match self {
            Color::Black => "30".into(),
            Color::Red => "31".into(),
            Color::Green => "32".into(),
            Color::Yellow => "33".into(),
            Color::Blue => "34".into(),
            Color::Magenta => "35".into(),
            Color::Cyan => "36".into(),
            Color::White => "37".into(),
            Color::Gray => "90".into(),
            Color::DarkGray => "90".into(),
            Color::LightRed => "91".into(),
            Color::LightGreen => "92".into(),
            Color::LightYellow => "93".into(),
            Color::LightBlue => "94".into(),
            Color::LightMagenta => "95".into(),
            Color::LightCyan => "96".into(),
            Color::Reset => "39".into(),
            Color::Rgb(r, g, b) => format!("38;2;{r};{g};{b}"),
            Color::Indexed(i) => format!("5;{i}"),
        }
    }
    fn sgr_bg(&self) -> String {
        match self {
            Color::Black => "40".into(),
            Color::Red => "41".into(),
            Color::Green => "42".into(),
            Color::Yellow => "43".into(),
            Color::Blue => "44".into(),
            Color::Magenta => "45".into(),
            Color::Cyan => "46".into(),
            Color::White => "47".into(),
            Color::Gray => "100".into(),
            Color::DarkGray => "100".into(),
            Color::LightRed => "101".into(),
            Color::LightGreen => "102".into(),
            Color::LightYellow => "103".into(),
            Color::LightBlue => "104".into(),
            Color::LightMagenta => "105".into(),
            Color::LightCyan => "106".into(),
            Color::Reset => "49".into(),
            Color::Rgb(r, g, b) => format!("48;2;{r};{g};{b}"),
            Color::Indexed(i) => format!("6;{i}"),
        }
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
    fn renders_title_and_items() {
        let state = ChoicePickerState::new(ChoicePickerOptions {
            title: "Choose".into(),
            items: vec![
                ChoiceItem::new("a", "Option A"),
                ChoiceItem::new("b", "Option B"),
            ],
            initial_id: None,
            theme: theme(),
        });
        let lines = state.render_lines(40);
        let combined: String = lines.join("\n");
        assert!(combined.contains("Choose"));
        assert!(combined.contains("Option A"));
        assert!(combined.contains("Option B"));
    }

    #[test]
    fn initial_selection_works() {
        let state = ChoicePickerState::new(ChoicePickerOptions {
            title: "T".into(),
            items: vec![ChoiceItem::new("a", "A"), ChoiceItem::new("b", "B")],
            initial_id: Some("b".into()),
            theme: theme(),
        });
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn moving_selection_changes_highlight() {
        let mut state = ChoicePickerState::new(ChoicePickerOptions {
            title: "T".into(),
            items: vec![ChoiceItem::new("a", "A"), ChoiceItem::new("b", "B")],
            initial_id: None,
            theme: theme(),
        });
        assert_eq!(state.selected, 0);
        state.handle_input(InputEvent::Action(crate::KeybindingAction::SelectDown));
        assert_eq!(state.selected, 1);
        state.handle_input(InputEvent::Action(crate::KeybindingAction::SelectDown));
        assert_eq!(state.selected, 0); // wraps
    }

    #[test]
    fn enter_returns_selected_item() {
        let mut state = ChoicePickerState::new(ChoicePickerOptions {
            title: "T".into(),
            items: vec![ChoiceItem::new("a", "A"), ChoiceItem::new("b", "B")],
            initial_id: None,
            theme: theme(),
        });
        state.handle_input(InputEvent::Submit);
        match state.take_result().unwrap() {
            ChoiceResult::Selected(item) => assert_eq!(item.id, "a"),
            ChoiceResult::Cancelled => panic!("expected Selected"),
        }
    }

    #[test]
    fn esc_returns_cancelled() {
        let mut state = ChoicePickerState::new(ChoicePickerOptions {
            title: "T".into(),
            items: vec![ChoiceItem::new("a", "A")],
            initial_id: None,
            theme: theme(),
        });
        state.handle_input(InputEvent::Cancel);
        assert!(matches!(state.take_result(), Some(ChoiceResult::Cancelled)));
    }
}
