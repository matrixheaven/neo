//! Transient theme manager overlay state.
//!
//! Owns the catalog snapshot, filter/focus/selection, the selected preview
//! value, delete/import/copy pending states, and a typed [`ThemeManagerAction`]
//! output queue. The state never writes files or config: it receives a snapshot
//! through [`ThemeManagerState::apply_snapshot`] and emits actions that the
//! controller (in `neo-agent`) executes against the canonical repository.
//!
//! Logical theme ids cross the crate boundary as opaque [`String`] values; the
//! repository remains the validator and source of truth.

use std::collections::VecDeque;
use std::fmt::Write as _;

use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{
    Color, InputResult, Line, Span, Style, paint, truncate_width, visible_width,
};
use crate::theme_preview::ThemePreviewRenderer;

/// One catalog row carried from the repository/controller into the TUI.
///
/// No absolute paths are carried; `id` is the opaque logical theme id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeCatalogEntrySnapshot {
    /// Logical theme id (relative path inside `$NEO_HOME/themes/`).
    pub id: String,
    /// User-facing display name (the theme JSON `name` or a derived name).
    pub display_name: String,
    /// Rendered theme value; `None` for invalid entries.
    pub theme: Option<TuiTheme>,
    /// Load error text for invalid entries.
    pub error: Option<String>,
    /// Whether this entry is the current session-active theme.
    pub active: bool,
    /// Whether this entry is the configured startup default.
    pub startup_default: bool,
}

impl ThemeCatalogEntrySnapshot {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.theme.is_some() && self.error.is_none()
    }

    #[must_use]
    pub fn label(&self) -> &str {
        if self.display_name.is_empty() {
            &self.id
        } else {
            &self.display_name
        }
    }
}

/// Focus target inside the manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeManagerFocus {
    #[default]
    List,
    Preview,
    Actions,
    Filter,
}

impl ThemeManagerFocus {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Preview => "Preview",
            Self::Actions => "Actions",
            Self::Filter => "Filter",
        }
    }
}

/// Pending state that gates further manager input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeManagerPending {
    /// Confirming deletion of the selected theme.
    Delete,
    /// Awaiting an import path from the controller's path dialog.
    ImportPath,
    /// Awaiting a display name for a duplicate from the controller's dialog.
    CopyName,
}

/// Conflict policy for an import whose destination already exists. The default
/// is to ask the user; the controller re-emits with an explicit policy after
/// the conflict dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeConflictPolicy {
    #[default]
    Ask,
    Overwrite,
    SaveAsNew,
}

/// A typed action the controller executes; the state never mutates files or
/// config itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeManagerAction {
    /// Apply for the current session only (no config write).
    ApplySession(String),
    /// Set the startup default; the current session stays unchanged.
    SetStartupDefault(String),
    /// Import an external theme file. The path is read-only input.
    Import {
        path: String,
        conflict_policy: ThemeConflictPolicy,
    },
    /// Duplicate the theme under a new display name.
    Duplicate {
        id: String,
        new_display_name: String,
    },
    /// Delete a managed theme after confirmation.
    Delete(String),
    /// Rescan and reparse the catalog.
    Refresh,
    /// Close the manager.
    Close,
}

/// Status/error message shown above the action bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeManagerStatus {
    pub text: String,
    pub is_error: bool,
}

impl ThemeManagerStatus {
    #[must_use]
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }

    #[must_use]
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }
}

/// Default page size for `PageUp`/`PageDown` navigation.
const DEFAULT_PAGE_SIZE: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeManagerState {
    entries: Vec<ThemeCatalogEntrySnapshot>,
    filter: String,
    filtered_indices: Vec<usize>,
    selected: Option<String>,
    focus: ThemeManagerFocus,
    preview_value: Option<TuiTheme>,
    preview_model: String,
    pending: Option<ThemeManagerPending>,
    status: Option<ThemeManagerStatus>,
    page_size: usize,
    pending_actions: VecDeque<ThemeManagerAction>,
}

impl ThemeManagerState {
    #[must_use]
    pub fn new(preview_model: impl Into<String>) -> Self {
        Self {
            entries: Vec::new(),
            filter: String::new(),
            filtered_indices: Vec::new(),
            selected: None,
            focus: ThemeManagerFocus::List,
            preview_value: None,
            preview_model: preview_model.into(),
            pending: None,
            status: None,
            page_size: DEFAULT_PAGE_SIZE,
            pending_actions: VecDeque::new(),
        }
    }

    /// Replace the catalog and restore a stable selection: the previous id is
    /// kept when it still exists; otherwise the entry nearest the previous
    /// position is selected (never a jump to the first item without cause).
    pub fn apply_snapshot(&mut self, entries: Vec<ThemeCatalogEntrySnapshot>) {
        let mut entries = entries;
        entries.sort_by(|a, b| {
            a.display_name
                .to_lowercase()
                .cmp(&b.display_name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        let previous_position = self
            .selected
            .as_deref()
            .and_then(|id| self.entries.iter().position(|entry| entry.id == id));
        self.selected = if let Some(id) = &self.selected {
            if entries.iter().any(|entry| &entry.id == id) {
                Some(id.clone())
            } else if let Some(position) = previous_position {
                entries
                    .get(position.min(entries.len().saturating_sub(1)))
                    .map(|entry| entry.id.clone())
            } else {
                entries.first().map(|entry| entry.id.clone())
            }
        } else {
            entries.first().map(|entry| entry.id.clone())
        };
        self.entries = entries;
        self.recompute_filter();
    }

    /// Select a specific id (used after import/copy). Clears the filter when
    /// the id would otherwise be hidden. Returns `false` for an unknown id.
    pub fn select_id(&mut self, id: &str) -> bool {
        if !self.entries.iter().any(|entry| entry.id == id) {
            return false;
        }
        self.selected = Some(id.to_owned());
        if !self.id_matches_filter(id) {
            self.filter.clear();
        }
        self.recompute_filter();
        true
    }

    /// Feed an external path-dialog result back into the manager. Returns the
    /// emitted `Import` action when accepted; `None` on cancel or empty input.
    pub fn submit_import_path(&mut self, path: Option<String>) -> Option<ThemeManagerAction> {
        if !matches!(self.pending, Some(ThemeManagerPending::ImportPath)) {
            return None;
        }
        self.pending = None;
        self.status = None;
        let path = path?.trim().to_owned();
        if path.is_empty() {
            return None;
        }
        let action = ThemeManagerAction::Import {
            path,
            conflict_policy: ThemeConflictPolicy::Ask,
        };
        self.emit(action.clone());
        Some(action)
    }

    /// Feed an external copy-dialog result back into the manager. Returns the
    /// emitted `Duplicate` action when accepted; `None` on cancel or empty
    /// input or when the selected entry cannot be duplicated.
    pub fn submit_copy_name(&mut self, name: Option<String>) -> Option<ThemeManagerAction> {
        if !matches!(self.pending, Some(ThemeManagerPending::CopyName)) {
            return None;
        }
        let selected = self
            .selected_entry()
            .map(|entry| (entry.id.clone(), entry.is_valid()));
        self.pending = None;
        self.status = None;
        let (id, valid) = selected?;
        let name = name?.trim().to_owned();
        if name.is_empty() || !valid {
            return None;
        }
        let action = ThemeManagerAction::Duplicate {
            id,
            new_display_name: name,
        };
        self.emit(action.clone());
        Some(action)
    }

    /// Take the next emitted action (FIFO).
    #[must_use]
    pub fn take_action(&mut self) -> Option<ThemeManagerAction> {
        self.pending_actions.pop_front()
    }

    #[must_use]
    pub fn has_action(&self) -> bool {
        !self.pending_actions.is_empty()
    }

    #[must_use]
    pub fn entries(&self) -> &[ThemeCatalogEntrySnapshot] {
        &self.entries
    }

    #[must_use]
    pub fn filter(&self) -> &str {
        &self.filter
    }

    #[must_use]
    pub const fn focus(&self) -> ThemeManagerFocus {
        self.focus
    }

    #[must_use]
    pub fn selected_id(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    #[must_use]
    pub fn selected_entry(&self) -> Option<&ThemeCatalogEntrySnapshot> {
        let id = self.selected.as_deref()?;
        self.filtered_indices
            .iter()
            .find_map(|index| self.entries.get(*index).filter(|entry| entry.id == id))
    }

    #[must_use]
    pub fn filtered_count(&self) -> usize {
        self.filtered_indices.len()
    }

    /// The previewed theme value; selection changes update this and nothing
    /// else (the runtime chrome is untouched until an action is executed).
    #[must_use]
    pub const fn preview(&self) -> Option<TuiTheme> {
        self.preview_value
    }

    #[must_use]
    pub const fn pending(&self) -> Option<ThemeManagerPending> {
        self.pending
    }

    #[must_use]
    pub const fn status(&self) -> Option<&ThemeManagerStatus> {
        self.status.as_ref()
    }

    pub fn set_page_size(&mut self, page_size: usize) {
        self.page_size = page_size.max(1);
    }

    // -- Input --------------------------------------------------------------

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.pending.is_some() {
            return self.handle_pending_input(input);
        }
        match input {
            InputEvent::Insert(character)
                if self.focus == ThemeManagerFocus::Filter && *character != '\t' =>
            {
                self.filter.push(*character);
                self.recompute_filter();
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectUp) | InputEvent::Insert('k') => {
                self.move_selection(-1);
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectDown) | InputEvent::Insert('j') => {
                self.move_selection(1);
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageUp) => {
                self.page_selection(false);
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectPageDown) => {
                self.page_selection(true);
                InputResult::Handled
            }
            InputEvent::MoveHome => {
                self.select_boundary(true);
                InputResult::Handled
            }
            InputEvent::MoveEnd => {
                self.select_boundary(false);
                InputResult::Handled
            }
            InputEvent::Insert('\t') => {
                self.cycle_focus(true);
                InputResult::Handled
            }
            InputEvent::Key(key) if key.as_str() == "shift+tab" => {
                self.cycle_focus(false);
                InputResult::Handled
            }
            InputEvent::Insert('/') => {
                self.focus = ThemeManagerFocus::Filter;
                InputResult::Handled
            }
            InputEvent::Submit => self.handle_submit(),
            InputEvent::Backspace => {
                if self.focus == ThemeManagerFocus::Filter {
                    self.filter.pop();
                    self.recompute_filter();
                    InputResult::Handled
                } else {
                    InputResult::Ignored
                }
            }
            InputEvent::Cancel => self.handle_cancel(),
            InputEvent::Insert('D' | 'd') => self.set_startup_default(),
            InputEvent::Insert('I' | 'i') => self.begin_import(),
            InputEvent::Insert('C' | 'c') => self.begin_copy(),
            InputEvent::Insert('X' | 'x') => self.begin_delete(),
            InputEvent::Insert('R' | 'r') => {
                self.emit(ThemeManagerAction::Refresh);
                InputResult::Handled
            }
            _ => InputResult::Ignored,
        }
    }

    fn handle_pending_input(&mut self, input: &InputEvent) -> InputResult {
        match (self.pending, input) {
            (
                Some(ThemeManagerPending::Delete),
                InputEvent::Submit | InputEvent::Insert('Y' | 'y'),
            ) => {
                self.pending = None;
                self.status = None;
                if let Some(id) = self.selected.clone() {
                    self.emit(ThemeManagerAction::Delete(id));
                }
                InputResult::Handled
            }
            (
                Some(
                    ThemeManagerPending::Delete
                    | ThemeManagerPending::ImportPath
                    | ThemeManagerPending::CopyName,
                ),
                InputEvent::Cancel | InputEvent::Insert('N' | 'n'),
            ) => {
                self.pending = None;
                self.status = None;
                InputResult::Handled
            }
            _ => InputResult::Ignored,
        }
    }

    fn handle_submit(&mut self) -> InputResult {
        match self.focus {
            ThemeManagerFocus::Filter => {
                self.focus = ThemeManagerFocus::List;
                InputResult::Handled
            }
            ThemeManagerFocus::List | ThemeManagerFocus::Preview | ThemeManagerFocus::Actions => {
                self.apply_selected()
            }
        }
    }

    fn handle_cancel(&mut self) -> InputResult {
        if self.focus == ThemeManagerFocus::Filter && !self.filter.is_empty() {
            // Esc clears the filter first; a second Esc closes.
            self.filter.clear();
            self.recompute_filter();
            return InputResult::Handled;
        }
        // Esc closes via the chrome; no Close action is queued on this path.
        InputResult::Cancelled
    }

    fn apply_selected(&mut self) -> InputResult {
        let Some(entry) = self.selected_entry() else {
            self.set_status(ThemeManagerStatus::error("No theme selected."));
            return InputResult::Handled;
        };
        if !entry.is_valid() {
            self.set_status(ThemeManagerStatus::error(format!(
                "Cannot apply invalid theme {}.",
                entry.label()
            )));
            return InputResult::Handled;
        }
        // A single action per keystroke; the chrome closes the overlay on
        // `Submitted`, so a later poll cannot re-apply the same theme.
        self.emit(ThemeManagerAction::ApplySession(entry.id.clone()));
        InputResult::Submitted
    }

    fn set_startup_default(&mut self) -> InputResult {
        let Some(entry) = self.selected_entry() else {
            self.set_status(ThemeManagerStatus::error("No theme selected."));
            return InputResult::Handled;
        };
        if !entry.is_valid() {
            self.set_status(ThemeManagerStatus::error(format!(
                "Cannot set invalid theme {} as startup default.",
                entry.label()
            )));
            return InputResult::Handled;
        }
        self.emit(ThemeManagerAction::SetStartupDefault(entry.id.clone()));
        InputResult::Handled
    }

    fn begin_import(&mut self) -> InputResult {
        self.pending = Some(ThemeManagerPending::ImportPath);
        self.set_status(ThemeManagerStatus::info(
            "Import: enter the theme file path.",
        ));
        InputResult::Handled
    }

    fn begin_copy(&mut self) -> InputResult {
        let Some(entry) = self.selected_entry() else {
            self.set_status(ThemeManagerStatus::error("No theme selected."));
            return InputResult::Handled;
        };
        if !entry.is_valid() {
            self.set_status(ThemeManagerStatus::error(format!(
                "Cannot duplicate invalid theme {}.",
                entry.label()
            )));
            return InputResult::Handled;
        }
        self.pending = Some(ThemeManagerPending::CopyName);
        self.set_status(ThemeManagerStatus::info(
            "Copy: enter a display name for the duplicate.",
        ));
        InputResult::Handled
    }

    fn begin_delete(&mut self) -> InputResult {
        let Some(entry) = self.selected_entry() else {
            self.set_status(ThemeManagerStatus::error("No theme selected."));
            return InputResult::Handled;
        };
        if entry.active {
            self.set_status(ThemeManagerStatus::error(
                "The active theme cannot be deleted.",
            ));
            return InputResult::Handled;
        }
        if entry.startup_default {
            self.set_status(ThemeManagerStatus::error(
                "The startup default cannot be deleted.",
            ));
            return InputResult::Handled;
        }
        let label = entry.label().to_owned();
        self.pending = Some(ThemeManagerPending::Delete);
        self.set_status(ThemeManagerStatus::info(format!(
            "Delete {label}?  Enter confirm · Esc cancel"
        )));
        InputResult::Handled
    }

    fn move_selection(&mut self, delta: i64) {
        let len = self.filtered_indices.len();
        if len == 0 {
            return;
        }
        let current = self.selected_index().unwrap_or(0);
        let next = match delta {
            -1 if current == 0 => len - 1,
            -1 => current - 1,
            1 => (current + 1) % len,
            _ => return,
        };
        self.select_filtered_index(next);
    }

    fn page_selection(&mut self, forward: bool) {
        let len = self.filtered_indices.len();
        if len == 0 {
            return;
        }
        let current = self.selected_index().unwrap_or(0);
        let next = if forward {
            (current + self.page_size).min(len - 1)
        } else {
            current.saturating_sub(self.page_size)
        };
        self.select_filtered_index(next);
    }

    fn select_boundary(&mut self, home: bool) {
        let len = self.filtered_indices.len();
        if len == 0 {
            return;
        }
        self.select_filtered_index(if home { 0 } else { len - 1 });
    }

    fn cycle_focus(&mut self, forward: bool) {
        const ORDER: [ThemeManagerFocus; 4] = [
            ThemeManagerFocus::List,
            ThemeManagerFocus::Preview,
            ThemeManagerFocus::Actions,
            ThemeManagerFocus::Filter,
        ];
        let current = ORDER
            .iter()
            .position(|focus| *focus == self.focus)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % ORDER.len()
        } else if current == 0 {
            ORDER.len() - 1
        } else {
            current - 1
        };
        self.focus = ORDER[next];
    }

    fn select_filtered_index(&mut self, index: usize) {
        let Some(source_index) = self.filtered_indices.get(index) else {
            return;
        };
        let Some(entry) = self.entries.get(*source_index) else {
            return;
        };
        self.selected = Some(entry.id.clone());
        self.refresh_preview();
    }

    fn selected_index(&self) -> Option<usize> {
        let id = self.selected.as_deref()?;
        self.filtered_indices
            .iter()
            .position(|index| self.entries.get(*index).is_some_and(|entry| entry.id == id))
    }

    fn recompute_filter(&mut self) {
        let filter = self.filter.to_lowercase();
        self.filtered_indices = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                filter.is_empty()
                    || entry.display_name.to_lowercase().contains(&filter)
                    || entry.id.to_lowercase().contains(&filter)
            })
            .map(|(index, _)| index)
            .collect();
        let selected_visible = self.selected.as_ref().is_some_and(|id| {
            self.filtered_indices.iter().any(|index| {
                self.entries
                    .get(*index)
                    .is_some_and(|entry| &entry.id == id)
            })
        });
        if !selected_visible {
            self.selected = self
                .filtered_indices
                .first()
                .and_then(|index| self.entries.get(*index))
                .map(|entry| entry.id.clone());
        }
        self.refresh_preview();
    }

    fn refresh_preview(&mut self) {
        self.preview_value = self.selected_entry().and_then(|entry| entry.theme);
    }

    fn id_matches_filter(&self, id: &str) -> bool {
        let filter = self.filter.to_lowercase();
        if filter.is_empty() {
            return true;
        }
        self.entries.iter().any(|entry| {
            entry.id == id
                && (entry.display_name.to_lowercase().contains(&filter)
                    || entry.id.to_lowercase().contains(&filter))
        })
    }

    fn emit(&mut self, action: ThemeManagerAction) {
        self.pending_actions.push_back(action);
    }

    fn set_status(&mut self, status: ThemeManagerStatus) {
        self.status = Some(status);
    }

    // -- Rendering ----------------------------------------------------------

    /// Render the full-screen manager surface. Every row is truncated to
    /// `width` visible columns and the result is padded to exactly `height`
    /// rows, so narrow and very-short terminals can never overflow.
    #[must_use]
    pub fn render_lines(&self, width: usize, height: usize, chrome: &TuiTheme) -> Vec<String> {
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let layout = Layout::for_size(width, height);
        let mut lines = Vec::new();
        lines.push(self.header_line(width, chrome, layout));
        let status_rows = usize::from(layout != Layout::VeryShort);
        let body_height = height.saturating_sub(2 + status_rows);
        match layout {
            Layout::Wide => {
                let list_width = list_pane_width(width);
                let preview_width = width.saturating_sub(list_width + 1);
                let left = self.list_pane(list_width, body_height, chrome);
                let right = self.preview_pane(preview_width, body_height, chrome);
                for row in 0..body_height {
                    let l = left.get(row).map_or("", String::as_str);
                    let r = right.get(row).map_or("", String::as_str);
                    lines.push(format!("{l} {r}"));
                }
            }
            Layout::Medium => {
                let list_height = (body_height / 2).max(1).min(body_height);
                lines.extend(self.list_pane(width, list_height, chrome));
                lines.extend(self.preview_pane(
                    width,
                    body_height.saturating_sub(list_height),
                    chrome,
                ));
            }
            Layout::Narrow | Layout::VeryShort => match self.focus {
                ThemeManagerFocus::List | ThemeManagerFocus::Filter => {
                    lines.extend(self.list_pane(width, body_height, chrome));
                }
                ThemeManagerFocus::Preview | ThemeManagerFocus::Actions => {
                    lines.extend(self.preview_pane(width, body_height, chrome));
                }
            },
        }
        if status_rows == 1 {
            lines.push(self.status_line(width, chrome));
        }
        lines.push(self.action_bar(width, chrome));
        pad_height(lines, height)
    }

    fn header_line(&self, width: usize, chrome: &TuiTheme, layout: Layout) -> String {
        let mut text = format!(
            " THEME MANAGER  focus {}  {} themes",
            self.focus.label(),
            self.filtered_count(),
        );
        if !self.filter.is_empty() || self.focus == ThemeManagerFocus::Filter {
            let _ = write!(text, "  filter \"{}\"", self.filter);
        }
        if layout == Layout::VeryShort
            && let Some(status) = &self.status
        {
            let _ = write!(text, "  {}", status.text);
        }
        let style = Style::default().fg(chrome.text_primary);
        paint(&truncate_width(&text, width, "", false), style)
    }

    fn list_pane(&self, width: usize, height: usize, chrome: &TuiTheme) -> Vec<String> {
        let body = if self.filtered_indices.is_empty() {
            if self.filter.is_empty() {
                vec!["No themes installed.".to_owned()]
            } else {
                vec![format!("No themes match \"{}\".", self.filter)]
            }
        } else {
            self.filtered_indices
                .iter()
                .filter_map(|index| self.entries.get(*index))
                .map(|entry| self.list_row(entry, width.saturating_sub(4), chrome))
                .collect()
        };
        pane(" themes ", width, height, &body, chrome.overlay_border)
    }

    fn list_row(
        &self,
        entry: &ThemeCatalogEntrySnapshot,
        width: usize,
        chrome: &TuiTheme,
    ) -> String {
        let selected = self.selected.as_deref() == Some(entry.id.as_str());
        let pointer = if selected { "> " } else { "  " };
        let name_style = if selected {
            Style::default()
                .fg(chrome.selected_fg)
                .bg(chrome.selected_bg)
                .bold()
        } else if !entry.is_valid() {
            Style::default().fg(chrome.status_error)
        } else {
            Style::default().fg(chrome.text_primary)
        };
        let pointer_style = if selected {
            name_style
        } else {
            Style::default().fg(chrome.text_muted)
        };
        let mut spans = vec![
            Span::styled(pointer, pointer_style),
            Span::styled(entry.label(), name_style),
        ];
        if entry.active {
            spans.push(Span::styled(
                "  ● active",
                Style::default().fg(chrome.status_ok),
            ));
        }
        if entry.startup_default {
            spans.push(Span::styled(
                "  ★ default",
                Style::default().fg(chrome.brand),
            ));
        }
        if !entry.is_valid() {
            spans.push(Span::styled(
                format!("  ✗ {}", entry.error.as_deref().unwrap_or("invalid")),
                Style::default().fg(chrome.status_error),
            ));
        }
        Line::from_spans(spans).truncate_to_width(width).to_ansi()
    }

    fn preview_pane(&self, width: usize, height: usize, chrome: &TuiTheme) -> Vec<String> {
        let body = if let Some(theme) = self.preview_value {
            ThemePreviewRenderer::new(
                theme,
                width.saturating_sub(4),
                height.saturating_sub(2),
                self.preview_model.clone(),
            )
            .render()
        } else {
            let message = if self.filtered_indices.is_empty() {
                "No theme selected.".to_owned()
            } else {
                "Select a valid theme to preview.".to_owned()
            };
            vec![
                paint(&message, Style::default().fg(chrome.text_muted)),
                paint(
                    "Invalid entries can be inspected and removed.",
                    Style::default().fg(chrome.text_muted),
                ),
            ]
        };
        pane(" preview ", width, height, &body, chrome.overlay_border)
    }

    fn status_line(&self, width: usize, chrome: &TuiTheme) -> String {
        let Some(status) = &self.status else {
            return String::new();
        };
        let marker = if status.is_error { "⚠" } else { "·" };
        let style = if status.is_error {
            Style::default().fg(chrome.status_error)
        } else {
            Style::default().fg(chrome.text_muted)
        };
        paint(
            &truncate_width(&format!(" {marker} {}", status.text), width, "", false),
            style,
        )
    }

    fn action_bar(&self, width: usize, chrome: &TuiTheme) -> String {
        let actions =
            " Enter apply · D default · I import · C copy · X delete · R refresh · Esc close";
        let style = if self.focus == ThemeManagerFocus::Actions {
            Style::default()
                .fg(chrome.selected_fg)
                .bg(chrome.selected_bg)
                .bold()
        } else {
            Style::default().fg(chrome.text_muted)
        };
        paint(&truncate_width(actions, width, "", false), style)
    }
}

/// Responsive layout buckets from the approved spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// `width >= 100` and `height >= 18`: list and preview side by side.
    Wide,
    /// `width 68..99`: list and preview stacked vertically.
    Medium,
    /// `width < 68`: one focused panel at a time.
    Narrow,
    /// `height < 8`: title/focus/status/action preserved, content clipped.
    VeryShort,
}

impl Layout {
    fn for_size(width: usize, height: usize) -> Self {
        if height < 8 {
            return Self::VeryShort;
        }
        if width >= 100 && height >= 18 {
            Self::Wide
        } else if width >= 68 {
            Self::Medium
        } else {
            Self::Narrow
        }
    }
}

fn list_pane_width(width: usize) -> usize {
    (width * 34 / 100).clamp(28, 48)
}

/// Draw a bordered pane (`┌ title ─┐` … `└──┘`) with `body` rows. Rows are
/// truncated to the inner width and the pane pads to exactly `height` rows.
fn pane(title: &str, width: usize, height: usize, body: &[String], color: Color) -> Vec<String> {
    if height == 0 {
        return Vec::new();
    }
    if width < 3 {
        return vec![truncate_width(title, width, "", false); height];
    }
    if height == 1 {
        return vec![truncate_width(title.trim(), width, "...", false)];
    }
    let inner = width.saturating_sub(2);
    let content_width = inner.saturating_sub(2);
    let style = Style::default().fg(color);
    let mut lines = vec![paint(&titled_top(title, inner), style)];
    for row in 0..height.saturating_sub(2) {
        let text = body.get(row).map_or("", String::as_str);
        // `truncate_width` measures ANSI-styled rows by visible width and
        // preserves escape sequences while clipping.
        let text = truncate_width(text, content_width, "", false);
        let visible = visible_width(&text);
        let padding = content_width.saturating_sub(visible);
        lines.push(format!(
            "{} {}{} {}",
            paint("│", style),
            text,
            " ".repeat(padding),
            paint("│", style),
        ));
    }
    lines.push(paint(&format!("└{}┘", "─".repeat(inner)), style));
    lines
}

fn titled_top(title: &str, inner: usize) -> String {
    let title = truncate_width(title, inner, "", false);
    format!(
        "┌{title}{}┐",
        "─".repeat(inner.saturating_sub(visible_width(&title)))
    )
}

fn pad_height(mut lines: Vec<String>, height: usize) -> Vec<String> {
    lines.truncate(height);
    while lines.len() < height {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
#[path = "test_cases/manager.rs"]
mod manager;
