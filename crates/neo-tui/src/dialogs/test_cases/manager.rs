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
