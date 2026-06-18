use crate::ansi::{Style, paint};
use crate::chrome::TuiTheme;
use crate::components::{truncate_width, visible_width};
use crate::core::InputResult;
use crate::input::{InputEvent, KeybindingAction};

/// A group of providers treated as a single row in the provider manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSource {
    /// Provider ids belonging to this source.
    pub provider_ids: Vec<String>,
    /// Display label for the source row.
    pub label: String,
    /// Origin classification for rendering/ordering hints.
    pub kind: ProviderSourceKind,
}

/// Classification of a [`ProviderSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSourceKind {
    BuiltIn,
    ApiJson,
    Inline,
}

/// Options used to create or refresh a [`ProviderManagerState`].
pub struct ProviderManagerOptions {
    pub sources: Vec<ProviderSource>,
    pub active_provider_id: Option<String>,
    pub theme: TuiTheme,
}

/// User-facing action produced by the provider manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderManagerAction {
    /// User chose to add a new provider platform.
    Add,
    /// User confirmed deletion of a source and all its providers.
    DeleteSource(Vec<String>),
    /// User chose to close the dialog.
    Close,
}

/// Synthetic row type used internally for rendering and navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Source {
        label: String,
        provider_ids: Vec<String>,
        is_active: bool,
    },
    Add,
}

/// Provider list/manager dialog matching Kimi Code's `/provider` UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderManagerState {
    rows: Vec<Row>,
    selected_index: usize,
    theme: TuiTheme,
    confirm: Option<ConfirmState>,
    action: Option<ProviderManagerAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfirmState {
    label: String,
    provider_ids: Vec<String>,
}

const ADD_ROW_LABEL: &str = "[ Add New Platform ]";
const HEADER_HINT: &str = "↑↓ navigate · D delete · Enter add · Esc close";

impl ProviderManagerState {
    /// Create a new provider manager with the given options.
    #[must_use]
    pub fn new(opts: ProviderManagerOptions) -> Self {
        let rows = build_rows(&opts);
        let selected_index = initial_selection(&rows, &opts.active_provider_id);
        Self {
            rows,
            selected_index,
            theme: opts.theme,
            confirm: None,
            action: None,
        }
    }

    /// Replace the options while preserving the current selection when possible.
    ///
    /// Selection is preserved by row id/label or first provider id; if no match
    /// is found the old index is clamped to the new row count. Any in-flight
    /// delete confirmation is cleared.
    pub fn set_options(&mut self, opts: ProviderManagerOptions) {
        let previous_row = self.rows.get(self.selected_index);
        let previous_label = previous_row.map(Row::label);
        let previous_first_id = previous_row.and_then(Row::first_provider_id);

        self.rows = build_rows(&opts);
        self.theme = opts.theme;
        self.confirm = None;
        self.action = None;

        let mut new_index = None;
        if let Some(label) = previous_label {
            new_index = self.rows.iter().position(|row| row.label() == label);
        }
        if new_index.is_none() {
            if let Some(id) = previous_first_id {
                new_index = self
                    .rows
                    .iter()
                    .position(|row| row.provider_ids().contains(&id));
            }
        }

        self.selected_index =
            new_index.unwrap_or_else(|| self.selected_index.min(self.rows.len().saturating_sub(1)));
    }

    /// Render the dialog as ANSI-styled lines inside a bordered box.
    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        if width < 4 {
            return lines;
        }

        let inner_width = width.saturating_sub(2).max(1);
        let border_style = Style::default().fg(self.theme.overlay_border);
        let title_style = Style::default().fg(self.theme.text_primary).bold();
        let hint_style = Style::default().fg(self.theme.text_muted);

        // Top border.
        lines.push(paint(
            &format!("┌{}┐", "─".repeat(inner_width)),
            border_style,
        ));

        // Title.
        lines.push(box_line(
            " Providers",
            inner_width,
            title_style,
            border_style,
        ));

        // Hint.
        let hint = if let Some(confirm) = &self.confirm {
            format!(" [y/N] delete {}?", confirm.label)
        } else {
            format!(" {HEADER_HINT}")
        };
        lines.push(box_line(&hint, inner_width, hint_style, border_style));

        // Blank separator.
        lines.push(box_line("", inner_width, Style::default(), border_style));

        // Rows.
        if self.rows.is_empty() {
            lines.push(box_line(
                "  No providers configured.",
                inner_width,
                hint_style,
                border_style,
            ));
        } else {
            for (index, row) in self.rows.iter().enumerate() {
                let is_selected = index == self.selected_index;
                let row_lines = render_row(row, is_selected, inner_width, self.theme);
                for line in row_lines {
                    lines.push(box_line_raw(&line, inner_width, border_style));
                }
            }
        }

        // Blank separator before bottom border.
        lines.push(box_line("", inner_width, Style::default(), border_style));

        // Bottom border.
        lines.push(paint(
            &format!("└{}┘", "─".repeat(inner_width)),
            border_style,
        ));

        lines
    }

    /// Handle keyboard input.
    ///
    /// Delete confirmation is armed with `d`/`D` on a source row, confirmed with
    /// `y`/`Y`, and cancelled with `n`/`N` or Esc. Enter on the add row returns
    /// [`InputResult::Submitted`] and sets [`ProviderManagerAction::Add`]. Esc
    /// otherwise returns [`InputResult::Cancelled`] and sets
    /// [`ProviderManagerAction::Close`].
    pub fn handle_input(&mut self, input: InputEvent) -> InputResult {
        if self.action.is_some() {
            return InputResult::Handled;
        }

        if self.confirm.is_some() {
            return self.handle_confirm_input(input);
        }

        match input {
            InputEvent::Action(KeybindingAction::SelectUp) => {
                self.move_up();
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectDown) => {
                self.move_down();
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageUp) => {
                self.move_page_up();
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageDown) => {
                self.move_page_down();
                InputResult::Handled
            }
            InputEvent::Key(key) => match key.as_str() {
                "up" => {
                    self.move_up();
                    InputResult::Handled
                }
                "down" => {
                    self.move_down();
                    InputResult::Handled
                }
                "pageup" => {
                    self.move_page_up();
                    InputResult::Handled
                }
                "pagedown" => {
                    self.move_page_down();
                    InputResult::Handled
                }
                _ => InputResult::Ignored,
            },
            InputEvent::Action(KeybindingAction::SelectConfirm) | InputEvent::Submit => {
                if matches!(self.rows.get(self.selected_index), Some(Row::Add)) {
                    self.action = Some(ProviderManagerAction::Add);
                    InputResult::Submitted
                } else {
                    InputResult::Handled
                }
            }
            InputEvent::Action(KeybindingAction::SelectCancel) | InputEvent::Cancel => {
                self.action = Some(ProviderManagerAction::Close);
                InputResult::Cancelled
            }
            InputEvent::Insert('d' | 'D') => {
                self.arm_delete();
                InputResult::Handled
            }
            _ => InputResult::Ignored,
        }
    }

    /// Return the action selected by the user, if any.
    #[must_use]
    pub fn action(&self) -> Option<ProviderManagerAction> {
        self.action.clone()
    }

    fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.selected_index + 1 < self.rows.len() {
            self.selected_index += 1;
        }
    }

    fn move_page_up(&mut self) {
        const PAGE_SIZE: usize = 8;
        self.selected_index = self.selected_index.saturating_sub(PAGE_SIZE);
    }

    fn move_page_down(&mut self) {
        const PAGE_SIZE: usize = 8;
        self.selected_index =
            (self.selected_index + PAGE_SIZE).min(self.rows.len().saturating_sub(1));
    }

    fn arm_delete(&mut self) {
        let Some(Row::Source {
            label,
            provider_ids,
            ..
        }) = self.rows.get(self.selected_index)
        else {
            return;
        };
        self.confirm = Some(ConfirmState {
            label: label.clone(),
            provider_ids: provider_ids.clone(),
        });
    }

    fn handle_confirm_input(&mut self, input: InputEvent) -> InputResult {
        match input {
            InputEvent::Insert('y' | 'Y') => {
                if let Some(confirm) = self.confirm.take() {
                    self.action = Some(ProviderManagerAction::DeleteSource(confirm.provider_ids));
                    return InputResult::Submitted;
                }
                InputResult::Handled
            }
            InputEvent::Insert('n' | 'N')
            | InputEvent::Action(KeybindingAction::SelectCancel)
            | InputEvent::Cancel => {
                self.confirm = None;
                InputResult::Handled
            }
            _ => InputResult::Ignored,
        }
    }
}

fn build_rows(opts: &ProviderManagerOptions) -> Vec<Row> {
    let mut rows: Vec<Row> = opts
        .sources
        .iter()
        .map(|source| {
            let is_active = opts
                .active_provider_id
                .as_ref()
                .is_some_and(|active| source.provider_ids.iter().any(|id| id == active));
            Row::Source {
                label: source.label.clone(),
                provider_ids: source.provider_ids.clone(),
                is_active,
            }
        })
        .collect();
    rows.push(Row::Add);
    rows
}

fn initial_selection(rows: &[Row], active_provider_id: &Option<String>) -> usize {
    if let Some(active) = active_provider_id {
        if let Some(index) = rows.iter().position(|row| match row {
            Row::Source { provider_ids, .. } => provider_ids.contains(active),
            Row::Add => false,
        }) {
            return index;
        }
    }
    0
}

fn render_row(row: &Row, is_selected: bool, width: usize, theme: TuiTheme) -> Vec<String> {
    let pointer = if is_selected { "❯" } else { " " };
    let pointer_style = if is_selected {
        Style::default().fg(theme.brand)
    } else {
        Style::default().fg(theme.text_muted)
    };

    let label_style = if is_selected {
        Style::default().fg(theme.brand).bold()
    } else {
        match row {
            Row::Add => Style::default().fg(theme.brand),
            Row::Source { .. } => Style::default().fg(theme.prompt),
        }
    };

    let marker = match row {
        Row::Source {
            is_active: true, ..
        } => " ← current",
        _ => "",
    };
    let marker_style = Style::default().fg(theme.status_ok);

    let label = row.label();
    let marker_width = visible_width(marker);
    let available = width.saturating_sub(4).saturating_sub(marker_width);
    let label_text = truncate_width(&label, available, "…", false);

    let pointer_text = paint(&format!("{pointer} "), pointer_style);
    let label_text = paint(&label_text, label_style);
    let marker_text = if marker.is_empty() {
        String::new()
    } else {
        paint(marker, marker_style)
    };

    vec![format!("  {pointer_text}{label_text}{marker_text}")]
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

impl Row {
    fn label(&self) -> String {
        match self {
            Row::Source { label, .. } => label.clone(),
            Row::Add => ADD_ROW_LABEL.to_owned(),
        }
    }

    fn first_provider_id(&self) -> Option<String> {
        match self {
            Row::Source { provider_ids, .. } => provider_ids.first().cloned(),
            Row::Add => None,
        }
    }

    fn provider_ids(&self) -> &[String] {
        match self {
            Row::Source { provider_ids, .. } => provider_ids,
            Row::Add => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(label: &str, ids: &[&str]) -> ProviderSource {
        ProviderSource {
            provider_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
            label: label.to_owned(),
            kind: ProviderSourceKind::BuiltIn,
        }
    }

    fn theme() -> TuiTheme {
        TuiTheme::default()
    }

    fn manager(sources: Vec<ProviderSource>, active: Option<&str>) -> ProviderManagerState {
        ProviderManagerState::new(ProviderManagerOptions {
            sources,
            active_provider_id: active.map(std::borrow::ToOwned::to_owned),
            theme: theme(),
        })
    }

    fn visible_lines(state: &ProviderManagerState, width: usize) -> Vec<String> {
        state
            .render_lines(width)
            .iter()
            .map(|line| crate::ansi::strip_ansi(line))
            .collect()
    }

    #[test]
    fn render_shows_title_source_rows_and_add_row() {
        let state = manager(
            vec![
                source("OpenAI", &["openai"]),
                source("Anthropic", &["anthropic"]),
            ],
            Some("openai"),
        );
        let visible = visible_lines(&state, 60);
        let joined = visible.join("\n");

        assert!(joined.contains("Providers"), "title missing: {joined}");
        assert!(joined.contains("OpenAI"), "OpenAI row missing: {joined}");
        assert!(
            joined.contains("Anthropic"),
            "Anthropic row missing: {joined}"
        );
        assert!(
            joined.contains("[ Add New Platform ]"),
            "add row missing: {joined}"
        );
        assert!(
            joined.contains("← current"),
            "current marker missing: {joined}"
        );
    }

    #[test]
    fn render_shows_hint() {
        let state = manager(vec![source("OpenAI", &["openai"])], None);
        let visible = visible_lines(&state, 80);
        let joined = visible.join("\n");
        assert!(
            joined.contains("↑↓ navigate · D delete · Enter add · Esc close"),
            "hint missing: {joined}"
        );
    }

    #[test]
    fn render_has_borders() {
        let state = manager(vec![source("OpenAI", &["openai"])], None);
        let visible = visible_lines(&state, 40);
        let first = visible.first().unwrap();
        let last = visible.last().unwrap();
        assert!(first.starts_with('┌') && first.ends_with('┐'));
        assert!(last.starts_with('└') && last.ends_with('┘'));
    }

    #[test]
    fn d_arms_delete_confirmation() {
        let mut state = manager(vec![source("OpenAI", &["openai"])], None);
        let result = state.handle_input(InputEvent::Insert('D'));
        assert_eq!(result, InputResult::Handled);

        let visible = visible_lines(&state, 60);
        let joined = visible.join("\n");
        assert!(
            joined.contains("[y/N] delete OpenAI?"),
            "confirmation prompt missing: {joined}"
        );
        assert!(state.confirm.is_some());
    }

    #[test]
    fn y_confirms_delete_source() {
        let mut state = manager(vec![source("OpenAI", &["openai"])], None);
        state.handle_input(InputEvent::Insert('D'));
        let result = state.handle_input(InputEvent::Insert('Y'));
        assert_eq!(result, InputResult::Submitted);
        assert_eq!(
            state.action(),
            Some(ProviderManagerAction::DeleteSource(vec![
                "openai".to_owned()
            ]))
        );
        assert!(state.confirm.is_none());
    }

    #[test]
    fn n_cancels_delete_confirmation() {
        let mut state = manager(vec![source("OpenAI", &["openai"])], None);
        state.handle_input(InputEvent::Insert('D'));
        let result = state.handle_input(InputEvent::Insert('n'));
        assert_eq!(result, InputResult::Handled);
        assert!(state.action().is_none());
        assert!(state.confirm.is_none());

        let visible = visible_lines(&state, 60);
        let joined = visible.join("\n");
        assert!(
            !joined.contains("[y/N] delete"),
            "confirmation prompt should be gone: {joined}"
        );
    }

    #[test]
    fn enter_on_add_row_returns_add() {
        let mut state = manager(
            vec![
                source("OpenAI", &["openai"]),
                source("Anthropic", &["anthropic"]),
            ],
            None,
        );
        // Move selection to the synthetic add row.
        state.handle_input(InputEvent::Action(KeybindingAction::SelectDown));
        state.handle_input(InputEvent::Action(KeybindingAction::SelectDown));
        let result = state.handle_input(InputEvent::Submit);
        assert_eq!(result, InputResult::Submitted);
        assert_eq!(state.action(), Some(ProviderManagerAction::Add));
    }

    #[test]
    fn enter_on_source_row_does_not_submit() {
        let mut state = manager(vec![source("OpenAI", &["openai"])], None);
        let result = state.handle_input(InputEvent::Submit);
        assert_eq!(result, InputResult::Handled);
        assert!(state.action().is_none());
    }

    #[test]
    fn esc_returns_close() {
        let mut state = manager(vec![source("OpenAI", &["openai"])], None);
        let result = state.handle_input(InputEvent::Cancel);
        assert_eq!(result, InputResult::Cancelled);
        assert_eq!(state.action(), Some(ProviderManagerAction::Close));
    }

    #[test]
    fn esc_cancels_delete_confirmation() {
        let mut state = manager(vec![source("OpenAI", &["openai"])], None);
        state.handle_input(InputEvent::Insert('D'));
        let result = state.handle_input(InputEvent::Cancel);
        assert_eq!(result, InputResult::Handled);
        assert!(state.confirm.is_none());
        assert!(state.action().is_none());
    }

    #[test]
    fn set_options_preserves_selection_by_label() {
        let mut state = manager(
            vec![
                source("OpenAI", &["openai"]),
                source("Anthropic", &["anthropic"]),
            ],
            None,
        );
        state.handle_input(InputEvent::Action(KeybindingAction::SelectDown));
        assert_eq!(state.selected_index, 1);

        state.set_options(ProviderManagerOptions {
            sources: vec![
                source("OpenAI", &["openai"]),
                source("Anthropic", &["anthropic"]),
                source("Google", &["google"]),
            ],
            active_provider_id: None,
            theme: theme(),
        });

        assert_eq!(state.selected_index, 1);
    }

    #[test]
    fn set_options_preserves_selection_by_provider_id() {
        let mut state = manager(
            vec![
                source("OpenAI", &["openai"]),
                source("Anthropic", &["anthropic"]),
            ],
            None,
        );
        state.handle_input(InputEvent::Action(KeybindingAction::SelectDown));
        assert_eq!(state.selected_index, 1);

        state.set_options(ProviderManagerOptions {
            sources: vec![source("Anthropic Renamed", &["anthropic"])],
            active_provider_id: None,
            theme: theme(),
        });

        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn set_options_clamps_index_when_rows_shrink() {
        let mut state = manager(
            vec![
                source("OpenAI", &["openai"]),
                source("Anthropic", &["anthropic"]),
            ],
            None,
        );
        state.handle_input(InputEvent::Action(KeybindingAction::SelectDown));
        state.handle_input(InputEvent::Action(KeybindingAction::SelectDown));
        // add row at index 2
        assert_eq!(state.selected_index, 2);

        state.set_options(ProviderManagerOptions {
            sources: vec![source("OpenAI", &["openai"])],
            active_provider_id: None,
            theme: theme(),
        });

        assert_eq!(state.selected_index, 1);
    }

    #[test]
    fn initial_selection_defaults_to_active_provider() {
        let state = manager(
            vec![
                source("OpenAI", &["openai"]),
                source("Anthropic", &["anthropic"]),
            ],
            Some("anthropic"),
        );
        assert_eq!(state.selected_index, 1);
    }

    #[test]
    fn move_down_and_up_changes_selection() {
        let mut state = manager(
            vec![
                source("OpenAI", &["openai"]),
                source("Anthropic", &["anthropic"]),
            ],
            None,
        );
        assert_eq!(state.selected_index, 0);
        state.handle_input(InputEvent::Action(KeybindingAction::SelectDown));
        assert_eq!(state.selected_index, 1);
        state.handle_input(InputEvent::Action(KeybindingAction::SelectUp));
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn delete_on_add_row_is_ignored() {
        let mut state = manager(
            vec![
                source("OpenAI", &["openai"]),
                source("Anthropic", &["anthropic"]),
            ],
            None,
        );
        state.handle_input(InputEvent::Action(KeybindingAction::SelectDown));
        state.handle_input(InputEvent::Action(KeybindingAction::SelectDown));
        assert!(matches!(
            state.rows.get(state.selected_index),
            Some(Row::Add)
        ));

        let result = state.handle_input(InputEvent::Insert('D'));
        assert_eq!(result, InputResult::Handled);
        assert!(state.confirm.is_none());
    }
}
