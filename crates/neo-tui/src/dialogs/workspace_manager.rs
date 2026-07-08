use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{InputResult, Style, paint, truncate_width, visible_width};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub path: String,
    pub enabled: bool,
    pub read: bool,
    pub write: bool,
    pub missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceManagerOptions {
    pub trusted: bool,
    pub rows: Vec<WorkspaceRow>,
    pub theme: TuiTheme,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceManagerAction {
    Add,
    ToggleEnabled(String),
    ToggleRead(String),
    ToggleWrite(String),
    Delete(String),
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceManagerState {
    trusted: bool,
    rows: Vec<Row>,
    selected_index: usize,
    theme: TuiTheme,
    action: Option<WorkspaceManagerAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Workspace(WorkspaceRow),
    Add,
}

const HEADER_HINT: &str =
    "↑↓ navigate · A add · E on/off · R read on/off · W write on/off · D delete · Esc close";
const NARROW_HINT: &str = "↑↓ · A add · E on/off · R read · W write · D delete · Esc";

impl WorkspaceManagerState {
    #[must_use]
    pub fn new(opts: &WorkspaceManagerOptions) -> Self {
        Self {
            trusted: opts.trusted,
            rows: build_rows(opts),
            selected_index: 0,
            theme: opts.theme,
            action: None,
        }
    }

    pub fn set_options(&mut self, opts: &WorkspaceManagerOptions) {
        let previous_path = self.selected_path();
        self.trusted = opts.trusted;
        self.rows = build_rows(opts);
        self.theme = opts.theme;
        self.action = None;
        self.selected_index = previous_path
            .and_then(|path| {
                self.rows
                    .iter()
                    .position(|row| row.path().is_some_and(|candidate| candidate == path))
            })
            .unwrap_or_else(|| self.selected_index.min(self.rows.len().saturating_sub(1)));
    }

    #[must_use]
    pub fn action(&self) -> Option<WorkspaceManagerAction> {
        self.action.clone()
    }

    pub fn take_action(&mut self) -> Option<WorkspaceManagerAction> {
        self.action.take()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        if width < 4 {
            return Vec::new();
        }

        let inner_width = width.saturating_sub(2).max(1);
        let border_style = Style::default().fg(self.theme.overlay_border);
        let title_style = Style::default().fg(self.theme.text_primary).bold();
        let hint_style = Style::default().fg(self.theme.text_muted);

        let mut lines = vec![
            paint(&format!("┌{}┐", "─".repeat(inner_width)), border_style),
            box_line(" Workspace Access", inner_width, title_style, border_style),
        ];

        if !self.trusted {
            lines.push(box_line(
                " Esc close",
                inner_width,
                hint_style,
                border_style,
            ));
            lines.push(box_line("", inner_width, Style::default(), border_style));
            lines.push(box_line(
                "  This project is not trusted.",
                inner_width,
                hint_style,
                border_style,
            ));
            lines.push(box_line("", inner_width, Style::default(), border_style));
            lines.push(box_line(
                "  Additional workspace directories can expose files outside this cwd.",
                inner_width,
                hint_style,
                border_style,
            ));
            lines.push(box_line(
                "  Trust this workspace before managing extra filesystem access.",
                inner_width,
                hint_style,
                border_style,
            ));
            lines.push(box_line("", inner_width, Style::default(), border_style));
            lines.push(paint(
                &format!("└{}┘", "─".repeat(inner_width)),
                border_style,
            ));
            return lines;
        }

        let hint = if inner_width >= 76 {
            HEADER_HINT
        } else {
            NARROW_HINT
        };
        lines.push(box_line(
            &format!(" {hint}"),
            inner_width,
            hint_style,
            border_style,
        ));
        lines.push(box_line("", inner_width, Style::default(), border_style));

        if self.rows.iter().all(|row| matches!(row, Row::Add)) {
            lines.push(box_line(
                "  No additional workspaces configured.",
                inner_width,
                hint_style,
                border_style,
            ));
            lines.push(box_line(
                "  Added directories become available to file tools for this trusted cwd.",
                inner_width,
                hint_style,
                border_style,
            ));
            lines.push(box_line("", inner_width, Style::default(), border_style));
        }

        for (index, row) in self.rows.iter().enumerate() {
            let row_lines = render_row(row, index == self.selected_index, inner_width, self.theme);
            for line in row_lines {
                lines.push(box_line_raw(&line, inner_width, border_style));
            }
        }

        lines.push(box_line("", inner_width, Style::default(), border_style));
        lines.push(paint(
            &format!("└{}┘", "─".repeat(inner_width)),
            border_style,
        ));
        lines
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.action.is_some() {
            return InputResult::Handled;
        }

        match input {
            InputEvent::Action(action) => self.handle_action_input(*action),
            InputEvent::Key(key) => self.handle_named_key(key.as_str()),
            InputEvent::Submit => self.confirm_row(),
            InputEvent::Cancel => self.close(),
            InputEvent::Insert(character) => self.handle_insert(*character),
            _ => InputResult::Ignored,
        }
    }

    fn handle_action_input(&mut self, action: KeybindingAction) -> InputResult {
        match action {
            KeybindingAction::SelectUp => self.move_selection_up(),
            KeybindingAction::SelectDown => self.move_selection_down(),
            KeybindingAction::SelectPageUp => self.move_selection_page_up(),
            KeybindingAction::SelectPageDown => self.move_selection_page_down(),
            KeybindingAction::SelectConfirm => self.confirm_row(),
            KeybindingAction::SelectCancel => self.close(),
            _ => InputResult::Ignored,
        }
    }

    fn handle_named_key(&mut self, key: &str) -> InputResult {
        match key {
            "up" => self.move_selection_up(),
            "down" => self.move_selection_down(),
            "pageup" => self.move_selection_page_up(),
            "pagedown" => self.move_selection_page_down(),
            _ => InputResult::Ignored,
        }
    }

    fn handle_insert(&mut self, character: char) -> InputResult {
        if !self.trusted {
            return InputResult::Ignored;
        }
        match character {
            'a' | 'A' => self.add(),
            'e' | 'E' => self.action_for_selected(WorkspaceManagerAction::ToggleEnabled),
            'r' | 'R' => self.action_for_selected(WorkspaceManagerAction::ToggleRead),
            'w' | 'W' => self.action_for_selected(WorkspaceManagerAction::ToggleWrite),
            'd' | 'D' => self.action_for_selected(WorkspaceManagerAction::Delete),
            _ => InputResult::Ignored,
        }
    }

    fn confirm_row(&mut self) -> InputResult {
        if !self.trusted {
            return InputResult::Ignored;
        }
        if matches!(self.rows.get(self.selected_index), Some(Row::Add)) {
            return self.add();
        }
        InputResult::Handled
    }

    fn add(&mut self) -> InputResult {
        self.action = Some(WorkspaceManagerAction::Add);
        InputResult::Submitted
    }

    fn close(&mut self) -> InputResult {
        self.action = Some(WorkspaceManagerAction::Close);
        InputResult::Cancelled
    }

    fn action_for_selected(
        &mut self,
        build: impl FnOnce(String) -> WorkspaceManagerAction,
    ) -> InputResult {
        let Some(path) = self.selected_path() else {
            return InputResult::Handled;
        };
        self.action = Some(build(path));
        InputResult::Submitted
    }

    fn selected_path(&self) -> Option<String> {
        self.rows
            .get(self.selected_index)
            .and_then(Row::path)
            .map(str::to_owned)
    }

    fn move_selection_up(&mut self) -> InputResult {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
        InputResult::Handled
    }

    fn move_selection_down(&mut self) -> InputResult {
        if self.selected_index + 1 < self.rows.len() {
            self.selected_index += 1;
        }
        InputResult::Handled
    }

    fn move_selection_page_up(&mut self) -> InputResult {
        const PAGE_SIZE: usize = 8;
        self.selected_index = self.selected_index.saturating_sub(PAGE_SIZE);
        InputResult::Handled
    }

    fn move_selection_page_down(&mut self) -> InputResult {
        const PAGE_SIZE: usize = 8;
        self.selected_index =
            (self.selected_index + PAGE_SIZE).min(self.rows.len().saturating_sub(1));
        InputResult::Handled
    }
}

impl Row {
    fn path(&self) -> Option<&str> {
        match self {
            Self::Workspace(row) => Some(&row.path),
            Self::Add => None,
        }
    }
}

fn build_rows(opts: &WorkspaceManagerOptions) -> Vec<Row> {
    if !opts.trusted {
        return Vec::new();
    }

    let mut rows: Vec<Row> = opts.rows.iter().cloned().map(Row::Workspace).collect();
    rows.push(Row::Add);
    rows
}

fn render_row(row: &Row, is_selected: bool, width: usize, theme: TuiTheme) -> Vec<String> {
    let pointer = if is_selected { "▸" } else { " " };
    match row {
        Row::Add => {
            let plain = truncate_width(
                &format!(" {pointer} + Add workspace directory"),
                width,
                "…",
                true,
            );
            if is_selected {
                vec![paint(
                    &plain,
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selection_bg),
                )]
            } else {
                vec![format!(
                    " {}{}",
                    paint(pointer, Style::default().fg(theme.text_muted)),
                    paint(
                        " + Add workspace directory",
                        Style::default().fg(theme.brand)
                    )
                )]
            }
        }
        Row::Workspace(row) => {
            let enabled = if row.enabled { "[on ]" } else { "[off]" };
            let read = if row.read { "[R ]" } else { "[R-]" };
            let write = if row.write { "[W ]" } else { "[W-]" };
            let (access, state) = if row.missing {
                ("missing", "ignored")
            } else if !row.enabled {
                ("disabled", "inactive")
            } else if row.write {
                ("read/write", "active")
            } else if row.read {
                ("read-only", "active")
            } else {
                ("no access", "inactive")
            };
            let plain_badges = format!("  [{access}] · [{state}]");
            let prefix = format!(" {pointer} {enabled} {read} {write} ");
            let path_width = width
                .saturating_sub(visible_width(&prefix))
                .saturating_sub(visible_width(&plain_badges))
                .max(1);
            let path = truncate_width(&row.path, path_width, "…", false);
            let plain = truncate_width(&format!("{prefix}{path}{plain_badges}"), width, "…", true);

            if is_selected {
                return vec![paint(
                    &plain,
                    Style::default()
                        .fg(theme.selected_fg)
                        .bg(theme.selection_bg),
                )];
            }

            let flag_on = Style::default().fg(theme.status_ok);
            let flag_off = Style::default().fg(theme.text_muted);
            let muted = Style::default().fg(theme.text_muted);
            let access_style = if row.missing {
                Style::default().fg(theme.status_error)
            } else if !row.enabled || !row.read {
                Style::default().fg(theme.status_warn)
            } else {
                Style::default().fg(theme.brand)
            };
            let state_style = if row.enabled && row.read && !row.missing {
                Style::default().fg(theme.status_ok)
            } else {
                Style::default().fg(theme.text_muted)
            };

            vec![format!(
                " {} {} {} {} {}  {}{}{}",
                paint(pointer, muted),
                paint(enabled, if row.enabled { flag_on } else { flag_off }),
                paint(read, if row.read { flag_on } else { flag_off }),
                paint(write, if row.write { flag_on } else { flag_off }),
                paint(&path, Style::default().fg(theme.prompt)),
                paint(&format!("[{access}]"), access_style),
                paint(" · ", muted),
                paint(&format!("[{state}]"), state_style),
            )]
        }
    }
}

fn box_line(
    content: &str,
    content_width: usize,
    content_style: Style,
    border_style: Style,
) -> String {
    let visible = visible_width(&crate::primitive::strip_ansi(content));
    let content = if visible > content_width {
        truncate_width(content, content_width, "…", false)
    } else {
        content.to_owned()
    };
    let padding =
        content_width.saturating_sub(visible_width(&crate::primitive::strip_ansi(&content)));
    let left = paint("│", border_style);
    let right = paint("│", border_style);
    format!(
        "{left}{}{padding}{right}",
        paint(&content, content_style),
        padding = " ".repeat(padding)
    )
}

fn box_line_raw(content: &str, content_width: usize, border_style: Style) -> String {
    let visible = visible_width(&crate::primitive::strip_ansi(content));
    let content = if visible > content_width {
        truncate_width(content, content_width, "…", false)
    } else {
        content.to_owned()
    };
    let padding =
        content_width.saturating_sub(visible_width(&crate::primitive::strip_ansi(&content)));
    let left = paint("│", border_style);
    let right = paint("│", border_style);
    format!("{left}{content}{}{right}", " ".repeat(padding))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> TuiTheme {
        TuiTheme::default()
    }

    fn row(path: &str, enabled: bool, read: bool, write: bool, missing: bool) -> WorkspaceRow {
        WorkspaceRow {
            path: path.to_owned(),
            enabled,
            read,
            write,
            missing,
        }
    }

    fn manager(rows: Vec<WorkspaceRow>) -> WorkspaceManagerState {
        WorkspaceManagerState::new(&WorkspaceManagerOptions {
            trusted: true,
            rows,
            theme: theme(),
        })
    }

    fn visible_lines(state: &WorkspaceManagerState, width: usize) -> Vec<String> {
        state
            .render_lines(width)
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect()
    }

    #[test]
    fn renders_empty_state() {
        let state = manager(Vec::new());
        let rendered = visible_lines(&state, 88).join("\n");
        assert!(rendered.contains("No additional workspaces configured."));
        assert!(rendered.contains("+ Add workspace directory"));
    }

    #[test]
    fn renders_list_state_with_access_flags() {
        let state = manager(vec![row("/tmp/shared", true, true, false, false)]);
        let rendered = visible_lines(&state, 88).join("\n");
        assert!(rendered.contains("[on ] [R ] [W-] /tmp/shared  [read-only] · [active]"));
        assert!(
            !rendered
                .lines()
                .any(|line| line.trim() == "read-only · active"),
            "{rendered}"
        );
    }

    #[test]
    fn selected_workspace_row_uses_selection_background() {
        let state = manager(vec![row("/tmp/shared", true, true, false, false)]);
        let rendered = state.render_lines(88).join("\n");

        assert!(rendered.contains(&crate::primitive::bg_to_ansi(theme().selection_bg)));
    }

    #[test]
    fn renders_untrusted_warning() {
        let state = WorkspaceManagerState::new(&WorkspaceManagerOptions {
            trusted: false,
            rows: Vec::new(),
            theme: theme(),
        });
        let rendered = visible_lines(&state, 88).join("\n");
        assert!(rendered.contains("This project is not trusted."));
        assert!(!rendered.contains("+ Add workspace directory"));
    }

    #[test]
    fn key_e_toggles_enabled_for_selected_row() {
        let mut state = manager(vec![row("/tmp/shared", true, true, false, false)]);
        let result = state.handle_input(&InputEvent::Insert('E'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(
            state.action(),
            Some(WorkspaceManagerAction::ToggleEnabled(path)) if path == "/tmp/shared"
        ));
    }

    #[test]
    fn key_r_toggles_read_for_selected_row() {
        let mut state = manager(vec![row("/tmp/shared", true, true, false, false)]);
        let result = state.handle_input(&InputEvent::Insert('R'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(
            state.action(),
            Some(WorkspaceManagerAction::ToggleRead(path)) if path == "/tmp/shared"
        ));
    }

    #[test]
    fn key_w_toggles_write_for_selected_row() {
        let mut state = manager(vec![row("/tmp/shared", true, true, false, false)]);
        let result = state.handle_input(&InputEvent::Insert('W'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(
            state.action(),
            Some(WorkspaceManagerAction::ToggleWrite(path)) if path == "/tmp/shared"
        ));
    }

    #[test]
    fn page_down_moves_selection_by_page() {
        let mut state = manager(
            (0..10)
                .map(|index| row(&format!("/tmp/shared-{index}"), true, true, false, false))
                .collect(),
        );

        let result = state.handle_input(&InputEvent::Action(KeybindingAction::SelectPageDown));
        assert!(matches!(result, InputResult::Handled));

        let result = state.handle_input(&InputEvent::Insert('E'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(
            state.action(),
            Some(WorkspaceManagerAction::ToggleEnabled(path)) if path == "/tmp/shared-8"
        ));
    }
}
