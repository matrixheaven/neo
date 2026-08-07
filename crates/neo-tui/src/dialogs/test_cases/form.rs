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
    assert_eq!(state.active_field, 3);
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
    state.handle_input(InputEvent::Paste("npx".to_owned()));
    state.handle_input(InputEvent::Insert('\t'));
    state.handle_input(InputEvent::Paste(
        "\"-y\"\n\"  spaced  \"\n\"\"\n\"@server/filesystem\"".to_owned(),
    ));
    state.handle_input(InputEvent::Insert('\t'));
    state.handle_input(InputEvent::Paste("KEY=value".to_owned()));
    let result = state.handle_input(InputEvent::Submit);
    assert!(matches!(result, InputResult::Submitted));
    match state.take_result() {
        Some(McpAddFormResult::Submitted(data)) => {
            assert_eq!(data.name, "fs");
            assert_eq!(data.command, Some("npx".to_owned()));
            assert_eq!(
                data.args,
                vec!["-y", "  spaced  ", "", "@server/filesystem"]
            );
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
            assert!(data.args.is_empty());
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
