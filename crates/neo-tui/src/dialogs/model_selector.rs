//! Model selector dialog — flat searchable model list with thinking toggle.

use crate::ansi::Color;
use crate::components::{truncate_width, visible_width};
use crate::searchable_list::SearchableList;
use crate::{InputEvent, InputResult, TuiTheme};

/// One model entry in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelEntry {
    pub alias: String,
    pub provider_id: String,
    pub display_name: String,
    pub model_id: String,
    pub capabilities: Vec<String>,
    pub max_context_tokens: Option<u32>,
}

/// The user's selection (alias + effective thinking flag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelection {
    pub alias: String,
    pub thinking: bool,
}

/// Options for the model selector.
pub struct ModelSelectorOptions {
    pub models: Vec<ModelEntry>,
    pub current_alias: String,
    pub selected_alias: Option<String>,
    pub current_thinking: bool,
    pub theme: TuiTheme,
}

/// Result of interacting with the model selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSelectorResult {
    Selected(ModelSelection),
    Cancelled,
}

const TITLE: &str = "Models";
const HINT: &str = "↑↓ navigate · ←→ thinking · / filter · Enter select · Esc cancel";

/// State for the flat model selector dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelectorState {
    list: SearchableList<ModelEntry>,
    theme: TuiTheme,
    current_alias: String,
    /// Per-index thinking draft. None = use default; Some(true/false) = toggled.
    thinking_drafts: Vec<Option<bool>>,
    current_thinking: bool,
    result: Option<ModelSelectorResult>,
}

fn entry_search_text(entry: &ModelEntry) -> String {
    format!("{} {}", entry.display_name, entry.provider_id)
}

impl ModelSelectorState {
    #[must_use]
    pub fn new(opts: ModelSelectorOptions) -> Self {
        let initial_index = opts
            .selected_alias
            .as_ref()
            .and_then(|alias| opts.models.iter().position(|m| &m.alias == alias))
            .or_else(|| {
                opts.models
                    .iter()
                    .position(|m| m.alias == opts.current_alias)
            });

        let thinking_drafts = vec![None; opts.models.len()];
        let list = SearchableList::new(crate::searchable_list::SearchableListOptions {
            items: opts.models,
            to_search_text: entry_search_text,
            page_size: Some(8),
            initial_index,
            searchable: true,
        });

        Self {
            list,
            theme: opts.theme,
            current_alias: opts.current_alias,
            thinking_drafts,
            current_thinking: opts.current_thinking,
            result: None,
        }
    }

    fn selected_entry(&self) -> Option<&ModelEntry> {
        self.list.selected()
    }

    /// Determine effective thinking for the selected model.
    fn effective_thinking(&self, entry: &ModelEntry) -> bool {
        let idx = self.list.selected_index();
        // Check for always_thinking capability
        if entry.capabilities.iter().any(|c| c == "always_thinking") {
            return true;
        }
        // Check for thinking capability at all
        if !entry
            .capabilities
            .iter()
            .any(|c| c == "thinking" || c == "reasoning")
        {
            return false;
        }
        // Use draft if set, else current default
        self.thinking_drafts
            .get(idx)
            .copied()
            .flatten()
            .unwrap_or(self.current_thinking)
    }

    fn toggle_thinking(&mut self) {
        let idx = self.list.selected_index();
        if let Some(Some(_)) = self.thinking_drafts.get(idx) {
            // Already drafted — toggle it
            if let Some(draft) = self.thinking_drafts.get_mut(idx) {
                *draft = Some(!draft.unwrap_or(false));
            }
        } else if let Some(draft) = self.thinking_drafts.get_mut(idx) {
            *draft = Some(true);
        }
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        let inner_w = width.saturating_sub(2).max(1);
        let mut lines = Vec::new();

        // Top border
        lines.push(border_line(
            width,
            BorderKind::Top,
            TITLE,
            self.theme.overlay_border,
        ));

        // Hint
        lines.push(style_line(
            &format!(" {HINT}"),
            inner_w,
            self.theme.text_muted,
            Color::Reset,
        ));

        let query = self.list.query();
        if !query.is_empty() {
            lines.push(style_line(
                &format!(" /{query}"),
                inner_w,
                self.theme.brand,
                Color::Reset,
            ));
        } else {
            lines.push(String::new());
        }

        // Model rows
        let view = self.list.view();
        let filtered: Vec<&ModelEntry> = self.list.filtered();
        let page_items = &filtered[view.start..view.end];

        for (offset, entry) in page_items.iter().enumerate() {
            let global_idx = view.start + offset;
            let is_selected = global_idx == self.list.selected_index();
            let is_current = entry.alias == self.current_alias;
            lines.push(self.render_model_row(entry, inner_w, is_selected, is_current));
        }

        // "more" indicator
        if view.end < self.list.total_filtered() {
            let remaining = self.list.total_filtered() - view.end;
            lines.push(style_line(
                &format!(" ▼ {remaining} more"),
                inner_w,
                self.theme.text_muted,
                Color::Reset,
            ));
        }

        // Thinking indicator
        if let Some(entry) = self.selected_entry() {
            let thinking = self.effective_thinking(entry);
            let indicator = if thinking {
                "◉ thinking"
            } else {
                "○ thinking"
            };
            let color = if thinking {
                self.theme.brand
            } else {
                self.theme.text_muted
            };
            lines.push(style_line(
                &format!(" {indicator}"),
                inner_w,
                color,
                Color::Reset,
            ));
        }

        // Bottom border
        lines.push(border_line(
            width,
            BorderKind::Bottom,
            "",
            self.theme.overlay_border,
        ));

        lines
    }

    fn render_model_row(
        &self,
        entry: &ModelEntry,
        width: usize,
        selected: bool,
        is_current: bool,
    ) -> String {
        let name_col: usize = width / 2;
        let prov_col: usize = width.saturating_sub(name_col);
        let name = truncate_width(&entry.display_name, name_col.saturating_sub(2), "…", false);
        let provider = truncate_width(&entry.provider_id, prov_col.saturating_sub(12), "…", false);

        let current_marker = if is_current { " ← current" } else { "" };

        let gap = name_col.saturating_sub(visible_width(&name));
        let row_content = format!("{name}{}  {provider}{current_marker}", " ".repeat(gap));

        let (fg, bg) = if selected {
            (self.theme.selected_fg, self.theme.selected_bg)
        } else if is_current {
            (self.theme.brand, Color::Reset)
        } else {
            (Color::Reset, Color::Reset)
        };

        let styled = format!("\x1b[{};{}m {row_content}\x1b[0m", fg.sgr_fg(), bg.sgr_bg());
        styled
    }

    pub fn handle_input(&mut self, input: InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Ignored;
        }
        match input {
            InputEvent::Submit => {
                if let Some(entry) = self.selected_entry().cloned() {
                    let thinking = self.effective_thinking(&entry);
                    self.result = Some(ModelSelectorResult::Selected(ModelSelection {
                        alias: entry.alias.clone(),
                        thinking,
                    }));
                    InputResult::Submitted
                } else {
                    InputResult::Ignored
                }
            }
            InputEvent::Cancel => {
                if self.list.clear_query() {
                    InputResult::Handled
                } else {
                    self.result = Some(ModelSelectorResult::Cancelled);
                    InputResult::Cancelled
                }
            }
            InputEvent::Backspace => {
                self.list.handle_key("backspace");
                InputResult::Handled
            }
            InputEvent::Insert(ch) => {
                self.list.handle_key(&ch.to_string());
                InputResult::Handled
            }
            InputEvent::ScrollUp(1) | InputEvent::MoveLeft => {
                self.toggle_thinking();
                InputResult::Handled
            }
            InputEvent::ScrollDown(1) | InputEvent::MoveRight => {
                self.toggle_thinking();
                InputResult::Handled
            }
            // Arrow up/down from keybindings
            InputEvent::Action(crate::KeybindingAction::SelectUp) => {
                self.list.move_up();
                InputResult::Handled
            }
            InputEvent::Action(crate::KeybindingAction::SelectDown) => {
                self.list.move_down();
                InputResult::Handled
            }
            InputEvent::Action(crate::KeybindingAction::SelectPageUp) => {
                self.list.page_up();
                InputResult::Handled
            }
            InputEvent::Action(crate::KeybindingAction::SelectPageDown) => {
                self.list.page_down();
                InputResult::Handled
            }
            InputEvent::Action(crate::KeybindingAction::SelectConfirm) => {
                if let Some(entry) = self.selected_entry().cloned() {
                    let thinking = self.effective_thinking(&entry);
                    self.result = Some(ModelSelectorResult::Selected(ModelSelection {
                        alias: entry.alias.clone(),
                        thinking,
                    }));
                    InputResult::Submitted
                } else {
                    InputResult::Ignored
                }
            }
            InputEvent::Action(crate::KeybindingAction::SelectCancel) => {
                if self.list.clear_query() {
                    InputResult::Handled
                } else {
                    self.result = Some(ModelSelectorResult::Cancelled);
                    InputResult::Cancelled
                }
            }
            _ => InputResult::Ignored,
        }
    }

    #[must_use]
    pub fn result(&self) -> Option<&ModelSelectorResult> {
        self.result.as_ref()
    }

    #[must_use]
    pub fn take_result(&mut self) -> Option<ModelSelectorResult> {
        self.result.take()
    }
}

enum BorderKind {
    Top,
    Bottom,
}

fn border_line(width: usize, kind: BorderKind, title: &str, color: Color) -> String {
    let left = match kind {
        BorderKind::Top => "╭",
        BorderKind::Bottom => "╰",
    };
    let right = match kind {
        BorderKind::Top => "╮",
        BorderKind::Bottom => "╯",
    };
    let fill = if matches!(kind, BorderKind::Top) && !title.is_empty() {
        let title_part = format!(" {title} ");
        let title_len = visible_width(&title_part);
        let remaining = width.saturating_sub(2 + title_len);
        format!("{title_part}{}", "─".repeat(remaining))
    } else {
        "─".repeat(width.saturating_sub(2))
    };
    format!("\x1b[38;2;{}m{left}{fill}{right}\x1b[0m", color.sgr_rgb())
}

fn style_line(text: &str, _width: usize, fg: Color, _bg: Color) -> String {
    format!("\x1b[38;2;{}m{text}\x1b[0m", fg.sgr_rgb())
}

// Extension trait for Color to get SGR codes
trait ColorSgr {
    fn sgr_fg(&self) -> String;
    fn sgr_bg(&self) -> String;
    fn sgr_rgb(&self) -> String;
}

impl ColorSgr for Color {
    fn sgr_fg(&self) -> String {
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
    fn sgr_rgb(&self) -> String {
        match self {
            Color::Rgb(r, g, b) => format!("{r};{g};{b}"),
            _ => "255;255;255".into(),
        }
    }
}

// Re-export SearchableList from crate root for convenience
// (dialog modules import it from here)

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> TuiTheme {
        TuiTheme::default()
    }

    fn models() -> Vec<ModelEntry> {
        vec![
            ModelEntry {
                alias: "openai/gpt-4o".into(),
                provider_id: "openai".into(),
                display_name: "GPT-4o".into(),
                model_id: "gpt-4o".into(),
                capabilities: vec!["thinking".into()],
                max_context_tokens: Some(128_000),
            },
            ModelEntry {
                alias: "anthropic/claude-sonnet".into(),
                provider_id: "anthropic".into(),
                display_name: "Claude Sonnet".into(),
                model_id: "claude-sonnet".into(),
                capabilities: vec!["always_thinking".into()],
                max_context_tokens: Some(200_000),
            },
            ModelEntry {
                alias: "google/gemini-flash".into(),
                provider_id: "google".into(),
                display_name: "Gemini Flash".into(),
                model_id: "gemini-flash".into(),
                capabilities: vec![],
                max_context_tokens: Some(1_000_000),
            },
        ]
    }

    #[test]
    fn renders_title_and_rows() {
        let state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_thinking: false,
            theme: theme(),
        });
        let lines = state.render_lines(60);
        let combined: String = lines.join("\n");
        assert!(combined.contains("Models"));
        assert!(combined.contains("GPT-4o"));
        assert!(combined.contains("Claude Sonnet"));
    }

    #[test]
    fn current_marker_shown() {
        let state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_thinking: false,
            theme: theme(),
        });
        let lines = state.render_lines(60);
        let combined: String = lines.join("\n");
        assert!(combined.contains("← current"));
    }

    #[test]
    fn fuzzy_filter_reduces_items() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_thinking: false,
            theme: theme(),
        });
        state.handle_input(InputEvent::Insert('c'));
        state.handle_input(InputEvent::Insert('l'));
        // Should match "Claude Sonnet"
        assert_eq!(state.list.total_filtered(), 1);
    }

    #[test]
    fn thinking_toggle_respects_capabilities() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_thinking: false,
            theme: theme(),
        });

        // First model (gpt-4o) supports thinking → toggles on
        let entry = state.selected_entry().cloned().unwrap();
        assert_eq!(entry.alias, "openai/gpt-4o");
        assert!(!state.effective_thinking(&entry)); // default off
        state.handle_input(InputEvent::MoveRight); // toggle
        let entry2 = state.selected_entry().cloned().unwrap();
        assert!(state.effective_thinking(&entry2)); // now on

        // Second model (claude) always_thinking → stays on regardless
        state.handle_input(InputEvent::Action(crate::KeybindingAction::SelectDown));
        let entry3 = state.selected_entry().cloned().unwrap();
        assert_eq!(entry3.alias, "anthropic/claude-sonnet");
        assert!(state.effective_thinking(&entry3));

        // Third model (gemini) no thinking → stays off
        state.handle_input(InputEvent::Action(crate::KeybindingAction::SelectDown));
        let entry4 = state.selected_entry().cloned().unwrap();
        assert_eq!(entry4.alias, "google/gemini-flash");
        assert!(!state.effective_thinking(&entry4));
    }

    #[test]
    fn enter_returns_selected() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_thinking: false,
            theme: theme(),
        });
        state.handle_input(InputEvent::Submit);
        let result = state.take_result().unwrap();
        match result {
            ModelSelectorResult::Selected(sel) => {
                assert_eq!(sel.alias, "openai/gpt-4o");
                assert!(!sel.thinking);
            }
            _ => panic!("expected Selected"),
        }
    }

    #[test]
    fn esc_clears_query_then_cancels() {
        let mut state = ModelSelectorState::new(ModelSelectorOptions {
            models: models(),
            current_alias: "openai/gpt-4o".into(),
            selected_alias: None,
            current_thinking: false,
            theme: theme(),
        });
        state.handle_input(InputEvent::Insert('a'));
        assert!(!state.list.query().is_empty());

        state.handle_input(InputEvent::Cancel);
        assert!(state.list.query().is_empty());
        assert!(state.result.is_none()); // first Esc just cleared

        state.handle_input(InputEvent::Cancel);
        assert!(matches!(
            state.take_result(),
            Some(ModelSelectorResult::Cancelled)
        ));
    }
}
