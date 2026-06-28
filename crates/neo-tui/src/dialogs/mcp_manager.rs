use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::InputResult;
use crate::primitive::{Style, paint};
use crate::primitive::{truncate_width, visible_width};
use crate::primitive::theme::TuiTheme;

/// Row data passed in from the controller for one configured MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerRow {
    pub id: String,
    pub transport_label: String,
    pub enabled: bool,
    pub endpoint_summary: String,
    pub cwd_summary: Option<String>,
    pub env_keys: Vec<String>,
    pub header_keys: Vec<String>,
    pub tool_status: McpToolStatus,
}

/// Tool discovery state shown in a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpToolStatus {
    NotDiscovered,
    Discovering,
    Discovered(Vec<String>),
    Failed(String),
    SkippedDisabled,
}

impl McpToolStatus {
    fn summary(&self) -> String {
        match self {
            Self::NotDiscovered => "tools: not discovered".to_owned(),
            Self::Discovering => "tools: discovering...".to_owned(),
            Self::Discovered(names) => format!("tools: {} discovered", names.len()),
            Self::Failed(reason) => format!("tools: {reason}"),
            Self::SkippedDisabled => "tools: disabled".to_owned(),
        }
    }
}

/// Options used to create or refresh a [`McpManagerState`].
pub struct McpManagerOptions {
    pub servers: Vec<McpServerRow>,
    pub theme: TuiTheme,
}

/// User-facing action produced by the MCP manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpManagerAction {
    Add,
    Test(String),
    Refresh(String),
    ToggleEnabled(String),
    Delete(String),
    Auth(String),
    Close,
}

/// Synthetic row type used internally for rendering and navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Server { row: McpServerRow },
    Add,
}

/// MCP list/manager dialog matching Neo's `/mcp` UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpManagerState {
    rows: Vec<Row>,
    selected_index: usize,
    theme: TuiTheme,
    confirm: Option<ConfirmState>,
    action: Option<McpManagerAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfirmState {
    label: String,
    server_id: String,
}

const ADD_ROW_LABEL: &str = "+ Add MCP server";
const HEADER_HINT: &str =
    "↑↓ navigate · Enter test · A add · E toggle · D delete · R refresh · O auth · Esc close";
const NARROW_HINT: &str =
    "↑↓ · Enter test · A add · E toggle · D delete · R refresh · O auth · Esc";

impl McpManagerState {
    /// Create a new MCP manager with the given options.
    #[must_use]
    pub fn new(opts: &McpManagerOptions) -> Self {
        let rows = build_rows(opts);
        Self {
            rows,
            selected_index: 0,
            theme: opts.theme,
            confirm: None,
            action: None,
        }
    }

    /// Replace the options while preserving the current selection when possible.
    pub fn set_options(&mut self, opts: &McpManagerOptions) {
        let previous_id = self.rows.get(self.selected_index).and_then(Row::server_id);

        self.rows = build_rows(opts);
        self.theme = opts.theme;
        self.confirm = None;
        self.action = None;

        let new_index = previous_id
            .and_then(|id| {
                self.rows
                    .iter()
                    .position(|row| row.server_id().as_deref() == Some(&id))
            })
            .unwrap_or_else(|| self.selected_index.min(self.rows.len().saturating_sub(1)));
        self.selected_index = new_index;
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
        let status_ok = Style::default().fg(self.theme.status_ok);
        let status_warn = Style::default().fg(self.theme.status_warn);
        let status_error = Style::default().fg(self.theme.status_error);

        // Top border.
        lines.push(paint(
            &format!("┌{}┐", "─".repeat(inner_width)),
            border_style,
        ));

        // Title.
        lines.push(box_line(
            " MCP Servers",
            inner_width,
            title_style,
            border_style,
        ));

        // Hint.
        let hint = if let Some(confirm) = &self.confirm {
            format!(" [y/N] delete {}?", confirm.label)
        } else if inner_width >= 70 {
            format!(" {HEADER_HINT}")
        } else {
            format!(" {NARROW_HINT}")
        };
        lines.push(box_line(&hint, inner_width, hint_style, border_style));

        // Blank separator.
        lines.push(box_line("", inner_width, Style::default(), border_style));

        // Rows.
        let server_rows: Vec<&Row> = self
            .rows
            .iter()
            .filter(|row| matches!(row, Row::Server { .. }))
            .collect();
        if server_rows.is_empty() {
            lines.push(box_line(
                "  No MCP servers configured.",
                inner_width,
                hint_style,
                border_style,
            ));
            lines.push(box_line(
                "  Add a server to expose external tools to Neo.",
                inner_width,
                hint_style,
                border_style,
            ));
        }
        for (index, row) in self.rows.iter().enumerate() {
            let is_selected = index == self.selected_index;
            let row_lines = render_row(
                row,
                is_selected,
                inner_width,
                self.theme,
                status_ok,
                status_warn,
                status_error,
            );
            for line in row_lines {
                lines.push(box_line_raw(&line, inner_width, border_style));
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
    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.action.is_some() {
            return InputResult::Handled;
        }
        if self.confirm.is_some() {
            return self.handle_confirm_input(input);
        }
        self.handle_open_input(input)
    }

    fn handle_open_input(&mut self, input: &InputEvent) -> InputResult {
        match input {
            InputEvent::Action(action) => self.handle_action_input(*action),
            InputEvent::Key(key) => self.handle_named_key(key),
            InputEvent::Submit => self.confirm_row(),
            InputEvent::Cancel => self.close(),
            InputEvent::Insert(character) => self.handle_insert(*character),
            _ => InputResult::Ignored,
        }
    }

    fn handle_named_key(&mut self, key: &crate::input::KeyId) -> InputResult {
        match key.as_str() {
            "up" | "down" => self.handle_vertical_key(key.as_str()),
            "pageup" | "pagedown" => self.handle_page_key(key.as_str()),
            _ => InputResult::Ignored,
        }
    }

    fn handle_vertical_key(&mut self, key: &str) -> InputResult {
        if key == "up" {
            self.move_up();
        } else {
            self.move_down();
        }
        InputResult::Handled
    }

    fn handle_page_key(&mut self, key: &str) -> InputResult {
        if key == "pageup" {
            self.move_page_up();
        } else {
            self.move_page_down();
        }
        InputResult::Handled
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

    fn handle_insert(&mut self, character: char) -> InputResult {
        match character {
            'a' | 'A' => self.add(),
            'e' | 'E' => self.toggle_enabled(),
            'r' | 'R' => self.refresh(),
            'o' | 'O' => self.auth(),
            'd' | 'D' => {
                self.arm_delete();
                InputResult::Handled
            }
            _ => InputResult::Ignored,
        }
    }

    fn move_selection_up(&mut self) -> InputResult {
        self.move_up();
        InputResult::Handled
    }

    fn move_selection_down(&mut self) -> InputResult {
        self.move_down();
        InputResult::Handled
    }

    fn move_selection_page_up(&mut self) -> InputResult {
        self.move_page_up();
        InputResult::Handled
    }

    fn move_selection_page_down(&mut self) -> InputResult {
        self.move_page_down();
        InputResult::Handled
    }

    fn confirm_row(&mut self) -> InputResult {
        let id = self.rows.get(self.selected_index).and_then(Row::server_id);
        match id {
            Some(id) => self.test(&id),
            None => self.add(),
        }
    }

    fn add(&mut self) -> InputResult {
        self.action = Some(McpManagerAction::Add);
        InputResult::Submitted
    }

    fn test(&mut self, id: &str) -> InputResult {
        self.action = Some(McpManagerAction::Test(id.to_owned()));
        InputResult::Submitted
    }

    fn refresh(&mut self) -> InputResult {
        let Some(Row::Server { row }) = self.rows.get(self.selected_index) else {
            return InputResult::Handled;
        };
        self.action = Some(McpManagerAction::Refresh(row.id.clone()));
        InputResult::Submitted
    }

    fn toggle_enabled(&mut self) -> InputResult {
        let Some(Row::Server { row }) = self.rows.get(self.selected_index) else {
            return InputResult::Handled;
        };
        self.action = Some(McpManagerAction::ToggleEnabled(row.id.clone()));
        InputResult::Submitted
    }

    fn auth(&mut self) -> InputResult {
        let Some(Row::Server { row }) = self.rows.get(self.selected_index) else {
            return InputResult::Handled;
        };
        if row.transport_label != "remote-http" && row.transport_label != "remote-sse" {
            return InputResult::Handled;
        }
        self.action = Some(McpManagerAction::Auth(row.id.clone()));
        InputResult::Submitted
    }

    fn close(&mut self) -> InputResult {
        self.action = Some(McpManagerAction::Close);
        InputResult::Cancelled
    }

    fn arm_delete(&mut self) {
        let Some(Row::Server { row }) = self.rows.get(self.selected_index) else {
            return;
        };
        self.confirm = Some(ConfirmState {
            label: row.id.clone(),
            server_id: row.id.clone(),
        });
    }

    fn handle_confirm_input(&mut self, input: &InputEvent) -> InputResult {
        match input {
            InputEvent::Insert('y' | 'Y') => {
                if let Some(confirm) = self.confirm.take() {
                    self.action = Some(McpManagerAction::Delete(confirm.server_id));
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

    /// Return the action selected by the user, if any.
    #[must_use]
    pub fn action(&self) -> Option<McpManagerAction> {
        self.action.clone()
    }

    /// Take the pending action, clearing it from the dialog state.
    pub fn take_action(&mut self) -> Option<McpManagerAction> {
        self.action.take()
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
}

fn build_rows(opts: &McpManagerOptions) -> Vec<Row> {
    let mut rows: Vec<Row> = opts
        .servers
        .iter()
        .map(|server| Row::Server {
            row: server.clone(),
        })
        .collect();
    rows.push(Row::Add);
    rows
}

fn render_row(
    row: &Row,
    is_selected: bool,
    width: usize,
    theme: TuiTheme,
    status_ok: Style,
    status_warn: Style,
    status_error: Style,
) -> Vec<String> {
    let pointer = if is_selected { "❯" } else { " " };
    let pointer_style = if is_selected {
        Style::default().fg(theme.brand)
    } else {
        Style::default().fg(theme.text_muted)
    };

    match row {
        Row::Add => {
            let label_style = Style::default().fg(theme.brand);
            let pointer_text = paint(&format!("{pointer} "), pointer_style);
            let label_text = paint(ADD_ROW_LABEL, label_style);
            vec![format!("  {pointer_text}{label_text}")]
        }
        Row::Server { row } => {
            let id_style = if is_selected {
                Style::default().fg(theme.brand).bold()
            } else {
                Style::default().fg(theme.prompt)
            };
            let muted_style = Style::default().fg(theme.text_muted);

            let status_glyph = if row.enabled { "●" } else { "◌" };
            let status_style = if row.enabled { status_ok } else { status_warn };

            let tool_status = row.tool_status.summary();
            let tool_style = match &row.tool_status {
                McpToolStatus::Failed(_) => status_error,
                _ => muted_style,
            };

            // Main row: pointer status id transport tools
            let pointer_text = paint(&format!("{pointer} "), pointer_style);
            let status_text = paint(&format!("{status_glyph} "), status_style);
            let id_text = paint(&row.id, id_style);
            let transport_text = paint(&row.transport_label, muted_style);
            let tool_text = paint(&tool_status, tool_style);

            let main =
                format!("  {pointer_text}{status_text}{id_text}  {transport_text}  {tool_text}");
            let main_visible = visible_width(&crate::primitive::strip_ansi(&main));
            let available = width.saturating_sub(main_visible).saturating_sub(2);
            let endpoint = if available > 8 {
                let endpoint_text = truncate_width(&row.endpoint_summary, available, "…", false);
                format!("{main}  {}", paint(&endpoint_text, muted_style))
            } else {
                main
            };

            let mut lines = vec![endpoint];

            // Optional detail lines.
            let indent = "    ";
            let detail_width = width.saturating_sub(visible_width(indent));

            if let Some(cwd) = &row.cwd_summary {
                let text = format!("cwd: {cwd}");
                lines.push(format!(
                    "{indent}{}",
                    paint(
                        &truncate_width(&text, detail_width, "…", false),
                        muted_style
                    )
                ));
            }
            if !row.env_keys.is_empty() {
                let text = format!("env: {}", row.env_keys.join(", "));
                lines.push(format!(
                    "{indent}{}",
                    paint(
                        &truncate_width(&text, detail_width, "…", false),
                        muted_style
                    )
                ));
            }
            if !row.header_keys.is_empty() {
                let text = format!("headers: {}", row.header_keys.join(", "));
                lines.push(format!(
                    "{indent}{}",
                    paint(
                        &truncate_width(&text, detail_width, "…", false),
                        muted_style
                    )
                ));
            }

            lines
        }
    }
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
    fn server_id(&self) -> Option<String> {
        match self {
            Self::Server { row } => Some(row.id.clone()),
            Self::Add => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> TuiTheme {
        TuiTheme::default()
    }

    fn row(id: &str, transport: &str, enabled: bool, tool_status: McpToolStatus) -> McpServerRow {
        let transport_label = match transport {
            "stdio" => "studio",
            "http" => "remote-http",
            "sse" => "remote-sse",
            other => other,
        };
        McpServerRow {
            id: id.to_owned(),
            transport_label: transport_label.to_owned(),
            enabled,
            endpoint_summary: format!("endpoint-{id}"),
            cwd_summary: None,
            env_keys: vec![],
            header_keys: vec![],
            tool_status,
        }
    }

    fn manager(servers: Vec<McpServerRow>) -> McpManagerState {
        McpManagerState::new(&McpManagerOptions {
            servers,
            theme: theme(),
        })
    }

    fn visible_lines(state: &McpManagerState, width: usize) -> Vec<String> {
        state
            .render_lines(width)
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect()
    }

    #[test]
    fn render_shows_title_rows_and_add_row() {
        let state = manager(vec![
            row(
                "fs",
                "studio",
                true,
                McpToolStatus::Discovered(vec!["read".to_owned()]),
            ),
            row("linear", "remote-http", true, McpToolStatus::NotDiscovered),
        ]);
        let visible = visible_lines(&state, 80);
        let joined = visible.join("\n");
        assert!(joined.contains("MCP Servers"), "title missing: {joined}");
        assert!(joined.contains("fs"), "fs row missing: {joined}");
        assert!(joined.contains("linear"), "linear row missing: {joined}");
        assert!(
            joined.contains("+ Add MCP server"),
            "add row missing: {joined}"
        );
    }

    #[test]
    fn render_shows_empty_state() {
        let state = manager(vec![]);
        let visible = visible_lines(&state, 60);
        let joined = visible.join("\n");
        assert!(
            joined.contains("No MCP servers configured"),
            "empty state missing: {joined}"
        );
        assert!(
            joined.contains("+ Add MCP server"),
            "add row missing: {joined}"
        );
    }

    #[test]
    fn render_shows_enabled_and_disabled() {
        let state = manager(vec![
            row("fs", "studio", true, McpToolStatus::NotDiscovered),
            row("old", "remote-http", false, McpToolStatus::SkippedDisabled),
        ]);
        let visible = visible_lines(&state, 80);
        let joined = visible.join("\n");
        assert!(joined.contains('●'), "enabled marker missing: {joined}");
        assert!(joined.contains('◌'), "disabled marker missing: {joined}");
    }

    #[test]
    fn action_add_on_key_a() {
        let mut state = manager(vec![]);
        let result = state.handle_input(&InputEvent::Insert('a'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(state.action(), Some(McpManagerAction::Add)));
    }

    #[test]
    fn action_test_on_enter_for_server() {
        let mut state = manager(vec![row(
            "fs",
            "studio",
            true,
            McpToolStatus::NotDiscovered,
        )]);
        let result = state.handle_input(&InputEvent::Submit);
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(state.action(), Some(McpManagerAction::Test(id)) if id == "fs"));
    }

    #[test]
    fn action_add_on_enter_for_add_row() {
        let mut state = manager(vec![]);
        let result = state.handle_input(&InputEvent::Submit);
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(state.action(), Some(McpManagerAction::Add)));
    }

    #[test]
    fn action_toggle_enabled() {
        let mut state = manager(vec![row(
            "fs",
            "studio",
            true,
            McpToolStatus::NotDiscovered,
        )]);
        let result = state.handle_input(&InputEvent::Insert('E'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(state.action(), Some(McpManagerAction::ToggleEnabled(id)) if id == "fs"));
    }

    #[test]
    fn action_refresh() {
        let mut state = manager(vec![row(
            "fs",
            "studio",
            true,
            McpToolStatus::NotDiscovered,
        )]);
        let result = state.handle_input(&InputEvent::Insert('r'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(state.action(), Some(McpManagerAction::Refresh(id)) if id == "fs"));
    }

    #[test]
    fn delete_confirmation_flow() {
        let mut state = manager(vec![row(
            "fs",
            "studio",
            true,
            McpToolStatus::NotDiscovered,
        )]);
        let _ = state.handle_input(&InputEvent::Insert('d'));
        assert!(state.confirm.is_some());
        let result = state.handle_input(&InputEvent::Insert('y'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(state.action(), Some(McpManagerAction::Delete(id)) if id == "fs"));
    }

    #[test]
    fn delete_confirmation_cancelled() {
        let mut state = manager(vec![row(
            "fs",
            "studio",
            true,
            McpToolStatus::NotDiscovered,
        )]);
        let _ = state.handle_input(&InputEvent::Insert('d'));
        assert!(state.confirm.is_some());
        let result = state.handle_input(&InputEvent::Insert('n'));
        assert!(matches!(result, InputResult::Handled));
        assert!(state.action().is_none());
        assert!(state.confirm.is_none());
    }

    #[test]
    fn esc_closes() {
        let mut state = manager(vec![row(
            "fs",
            "studio",
            true,
            McpToolStatus::NotDiscovered,
        )]);
        let result = state.handle_input(&InputEvent::Cancel);
        assert!(matches!(result, InputResult::Cancelled));
        assert!(matches!(state.action(), Some(McpManagerAction::Close)));
    }

    #[test]
    fn action_is_cleared_after_take() {
        let mut state = manager(vec![row(
            "fs",
            "remote-http",
            true,
            McpToolStatus::NotDiscovered,
        )]);
        let result = state.handle_input(&InputEvent::Submit);
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(
            state.action(),
            Some(McpManagerAction::Test(id)) if id == "fs"
        ));

        assert!(matches!(
            state.take_action(),
            Some(McpManagerAction::Test(id)) if id == "fs"
        ));
        assert!(state.action().is_none());

        // After the action is consumed, Esc should close rather than re-issue
        // the previous action.
        let result = state.handle_input(&InputEvent::Cancel);
        assert!(matches!(result, InputResult::Cancelled));
        assert!(matches!(state.action(), Some(McpManagerAction::Close)));
    }

    #[test]
    fn set_options_preserves_selection_by_id() {
        let mut state = manager(vec![
            row("a", "studio", true, McpToolStatus::NotDiscovered),
            row("b", "remote-http", true, McpToolStatus::NotDiscovered),
        ]);
        state.move_down();
        assert_eq!(state.selected_index, 1);
        state.set_options(&McpManagerOptions {
            servers: vec![
                row("b", "remote-http", true, McpToolStatus::NotDiscovered),
                row("a", "studio", true, McpToolStatus::NotDiscovered),
            ],
            theme: theme(),
        });
        assert_eq!(state.selected_index, 0);
    }

    #[test]
    fn tool_status_summary_formats_counts() {
        assert_eq!(
            McpToolStatus::Discovered(vec!["a".to_owned(), "b".to_owned()]).summary(),
            "tools: 2 discovered"
        );
        assert_eq!(
            McpToolStatus::NotDiscovered.summary(),
            "tools: not discovered"
        );
        assert_eq!(
            McpToolStatus::Failed("timeout".to_owned()).summary(),
            "tools: timeout"
        );
    }

    #[test]
    fn skipped_disabled_status_renders() {
        let state = manager(vec![row(
            "old",
            "remote-http",
            false,
            McpToolStatus::SkippedDisabled,
        )]);
        let visible = visible_lines(&state, 80);
        let joined = visible.join("\n");
        assert!(joined.contains("old"), "row missing: {joined}");
    }

    #[test]
    fn action_auth_on_key_o_for_remote_http() {
        let mut state = manager(vec![row(
            "linear",
            "remote-http",
            true,
            McpToolStatus::NotDiscovered,
        )]);
        let result = state.handle_input(&InputEvent::Insert('O'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(state.action(), Some(McpManagerAction::Auth(id)) if id == "linear"));
    }

    #[test]
    fn action_auth_on_key_o_for_remote_sse() {
        let mut state = manager(vec![row(
            "linear",
            "remote-sse",
            true,
            McpToolStatus::NotDiscovered,
        )]);
        let result = state.handle_input(&InputEvent::Insert('O'));
        assert!(matches!(result, InputResult::Submitted));
        assert!(matches!(state.action(), Some(McpManagerAction::Auth(id)) if id == "linear"));
    }

    #[test]
    fn action_auth_ignored_for_stdio() {
        let mut state = manager(vec![row(
            "fs",
            "studio",
            true,
            McpToolStatus::NotDiscovered,
        )]);
        let result = state.handle_input(&InputEvent::Insert('O'));
        assert!(matches!(result, InputResult::Handled));
        assert!(state.action().is_none());
    }

    #[test]
    fn render_hint_includes_auth_key() {
        let state = manager(vec![row(
            "linear",
            "remote-http",
            true,
            McpToolStatus::NotDiscovered,
        )]);
        let visible = visible_lines(&state, 100);
        let joined = visible.join("\n");
        assert!(joined.contains("O auth"), "auth hint missing: {joined}");
    }
}
