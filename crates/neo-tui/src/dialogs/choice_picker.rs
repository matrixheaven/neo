//! Choice picker dialog — simple single-select list.

use std::fmt::Write as _;

use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::Color;
use crate::primitive::InputResult;
use crate::primitive::visible_width;
use crate::shell::TuiTheme;

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
    /// Maximum number of items visible at once. `0` means use the default.
    pub page_size: usize,
    /// Item id that represents the current value, if any. Its label will have
    /// ` ← current` appended when rendered.
    pub current_id: Option<String>,
}

const DEFAULT_PAGE_SIZE: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceResult {
    Selected(ChoiceItem),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoicePickerState {
    items: Vec<ChoiceItem>,
    selected: usize,
    scroll_offset: usize,
    page_size: usize,
    theme: TuiTheme,
    title: String,
    result: Option<ChoiceResult>,
    current_id: Option<String>,
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
        let page_size = if opts.page_size == 0 {
            DEFAULT_PAGE_SIZE
        } else {
            opts.page_size
        };
        let mut state = Self {
            items: opts.items,
            selected,
            scroll_offset: 0,
            page_size,
            theme: opts.theme,
            title: opts.title,
            result: None,
            current_id: opts.current_id,
        };
        state.ensure_selected_visible();
        state
    }

    fn ensure_selected_visible(&mut self) {
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + self.page_size {
            self.scroll_offset = self.selected.saturating_sub(self.page_size - 1);
        }
    }

    fn total_pages(&self) -> usize {
        self.items.len().div_ceil(self.page_size)
    }

    fn current_page(&self) -> usize {
        self.selected / self.page_size + 1
    }

    fn page_start_for(index: usize, page_size: usize) -> usize {
        (index / page_size) * page_size
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
            dialog_rgb(self.theme.overlay_border),
            "─".repeat(remaining),
        ));

        // Items (paginated)
        let end = (self.scroll_offset + self.page_size).min(self.items.len());
        for i in self.scroll_offset..end {
            let item = &self.items[i];
            let is_selected = i == self.selected;
            let marker = if is_selected { "▸" } else { " " };
            let is_current = self.current_id.as_ref().is_some_and(|id| id == &item.id);
            let label = if is_current {
                format!("{} ← current", item.label)
            } else {
                item.label.clone()
            };

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
                dialog_sgr_fg(fg),
                dialog_sgr_bg(bg)
            ));
        }

        // Hint
        let mut hint = "↑↓ navigate · Enter select · Esc cancel".to_owned();
        if self.items.len() > self.page_size {
            let _ = write!(
                hint,
                " · page {}/{} · ←/→ · PgUp/PgDn",
                self.current_page(),
                self.total_pages()
            );
        }
        lines.push(format!(
            "\x1b[38;2;{}m {hint}\x1b[0m",
            dialog_rgb(self.theme.text_muted)
        ));

        // Bottom border
        lines.push(format!(
            "\x1b[38;2;{}m╰{}\x1b[0m",
            dialog_rgb(self.theme.overlay_border),
            "─".repeat(inner_w),
        ));

        lines
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Ignored;
        }
        match input {
            InputEvent::Action(KeybindingAction::SelectUp) | InputEvent::ScrollUp(1) => {
                if !self.items.is_empty() && self.selected == 0 {
                    self.selected = self.items.len() - 1;
                } else {
                    self.selected = self.selected.saturating_sub(1);
                }
                self.ensure_selected_visible();
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectDown) | InputEvent::ScrollDown(1) => {
                if !self.items.is_empty() {
                    self.selected = (self.selected + 1) % self.items.len();
                }
                self.ensure_selected_visible();
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageUp) | InputEvent::MoveLeft => {
                if !self.items.is_empty() {
                    self.selected = self.selected.saturating_sub(self.page_size);
                    self.scroll_offset = Self::page_start_for(self.selected, self.page_size);
                }
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageDown) | InputEvent::MoveRight => {
                if !self.items.is_empty() {
                    self.selected = (self.selected + self.page_size).min(self.items.len() - 1);
                    self.scroll_offset = Self::page_start_for(self.selected, self.page_size);
                }
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectConfirm) | InputEvent::Submit => {
                if let Some(item) = self.items.get(self.selected).cloned() {
                    self.result = Some(ChoiceResult::Selected(item));
                    InputResult::Submitted
                } else {
                    InputResult::Ignored
                }
            }
            InputEvent::Action(KeybindingAction::SelectCancel) | InputEvent::Cancel => {
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

pub(super) fn dialog_sgr_fg(color: Color) -> String {
    dialog_sgr(color, DialogSgrLayer::Foreground)
}

pub(super) fn dialog_sgr_bg(color: Color) -> String {
    dialog_sgr(color, DialogSgrLayer::Background)
}

fn dialog_sgr(color: Color, layer: DialogSgrLayer) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", layer.rgb_prefix()),
        Color::Indexed(i) => format!("{};{i}", layer.indexed_prefix()),
        _ => named_dialog_sgr(color, layer)
            .unwrap_or_default()
            .to_owned(),
    }
}

fn named_dialog_sgr(color: Color, layer: DialogSgrLayer) -> Option<&'static str> {
    const FG: &[(Color, &str)] = &[
        (Color::Black, "30"),
        (Color::Red, "31"),
        (Color::Green, "32"),
        (Color::Yellow, "33"),
        (Color::Blue, "34"),
        (Color::Magenta, "35"),
        (Color::Cyan, "36"),
        (Color::White, "37"),
        (Color::Gray, "90"),
        (Color::DarkGray, "90"),
        (Color::LightRed, "91"),
        (Color::LightGreen, "92"),
        (Color::LightYellow, "93"),
        (Color::LightBlue, "94"),
        (Color::LightMagenta, "95"),
        (Color::LightCyan, "96"),
        (Color::Reset, "39"),
    ];
    const BG: &[(Color, &str)] = &[
        (Color::Black, "40"),
        (Color::Red, "41"),
        (Color::Green, "42"),
        (Color::Yellow, "43"),
        (Color::Blue, "44"),
        (Color::Magenta, "45"),
        (Color::Cyan, "46"),
        (Color::White, "47"),
        (Color::Gray, "100"),
        (Color::DarkGray, "100"),
        (Color::LightRed, "101"),
        (Color::LightGreen, "102"),
        (Color::LightYellow, "103"),
        (Color::LightBlue, "104"),
        (Color::LightMagenta, "105"),
        (Color::LightCyan, "106"),
        (Color::Reset, "49"),
    ];

    let table = match layer {
        DialogSgrLayer::Foreground => FG,
        DialogSgrLayer::Background => BG,
    };
    table
        .iter()
        .find_map(|(candidate, code)| (*candidate == color).then_some(*code))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogSgrLayer {
    Foreground,
    Background,
}

impl DialogSgrLayer {
    const fn rgb_prefix(self) -> &'static str {
        match self {
            Self::Foreground => "38",
            Self::Background => "48",
        }
    }

    const fn indexed_prefix(self) -> &'static str {
        match self {
            Self::Foreground => "5",
            Self::Background => "6",
        }
    }
}

pub(super) fn dialog_rgb(c: Color) -> String {
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
            page_size: 0,
            current_id: None,
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
            page_size: 0,
            current_id: None,
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
            page_size: 0,
            current_id: None,
            theme: theme(),
        });
        assert_eq!(state.selected, 0);
        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        assert_eq!(state.selected, 1);
        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        assert_eq!(state.selected, 0); // wraps
    }

    #[test]
    fn enter_returns_selected_item() {
        let mut state = ChoicePickerState::new(ChoicePickerOptions {
            title: "T".into(),
            items: vec![ChoiceItem::new("a", "A"), ChoiceItem::new("b", "B")],
            initial_id: None,
            page_size: 0,
            current_id: None,
            theme: theme(),
        });
        state.handle_input(&InputEvent::Submit);
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
            page_size: 0,
            current_id: None,
            theme: theme(),
        });
        state.handle_input(&InputEvent::Cancel);
        assert!(matches!(state.take_result(), Some(ChoiceResult::Cancelled)));
    }

    #[test]
    fn pagination_pages_and_indicator_match_selection() {
        let items: Vec<_> = (0..25)
            .map(|i| ChoiceItem::new(i.to_string(), format!("Item {i}")))
            .collect();
        let mut state = ChoicePickerState::new(ChoicePickerOptions {
            title: "T".into(),
            items,
            initial_id: None,
            page_size: 10,
            current_id: None,
            theme: theme(),
        });
        assert_eq!(state.current_page(), 1);
        assert_eq!(state.total_pages(), 3);

        state.handle_input(&InputEvent::Action(KeybindingAction::SelectPageDown));
        assert_eq!(state.selected, 10);
        assert_eq!(state.scroll_offset, 10);
        assert_eq!(state.current_page(), 2);

        state.handle_input(&InputEvent::Action(KeybindingAction::SelectPageDown));
        assert_eq!(state.selected, 20);
        assert_eq!(state.scroll_offset, 20);
        assert_eq!(state.current_page(), 3);

        state.handle_input(&InputEvent::Action(KeybindingAction::SelectPageUp));
        assert_eq!(state.selected, 10);
        assert_eq!(state.scroll_offset, 10);
        assert_eq!(state.current_page(), 2);

        state.handle_input(&InputEvent::MoveRight);
        assert_eq!(state.selected, 20);
        assert_eq!(state.current_page(), 3);

        state.handle_input(&InputEvent::MoveLeft);
        assert_eq!(state.selected, 10);
        assert_eq!(state.current_page(), 2);

        // Page indicator follows selected item even when scroll offset is unaligned.
        for _ in 0..12 {
            state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        }
        assert_eq!(state.selected, 22);
        assert_eq!(state.current_page(), 3);
    }
}
