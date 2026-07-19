use std::path::Path;

use serde_json::Value;

use crate::markdown::{highlight_code_lines, wrap_spans};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Span, Style};
use crate::shell::ToolStatusKind;

use super::partial_json::extract_partial_string_field;
use super::tool_call::ToolCallState;
use super::tool_renderers::{make_workspace_relative, render_text_preview_themed};

pub(super) fn header_metadata(state: &ToolCallState, theme: &TuiTheme) -> Option<Vec<Span>> {
    let style = muted_style(theme);
    match state.name.as_str() {
        "Bash" => {
            let arguments = Arguments::new(state.arguments.as_deref());
            if is_backgrounded(state) {
                return Some(vec![Span::styled(" · background", style)]);
            }
            let result = state.result.as_deref().filter(|result| !result.is_empty());
            result.map_or_else(
                || {
                    let has_command = arguments
                        .string("command")
                        .is_some_and(|command| !command.is_empty());
                    let has_details = state
                        .details
                        .as_ref()
                        .and_then(Value::as_object)
                        .is_some_and(|details| !details.is_empty());
                    (has_command || has_details).then(Vec::new)
                },
                |result| {
                    Some(vec![Span::styled(
                        format!(" · {} lines", result.lines().count()),
                        style,
                    )])
                },
            )
        }
        "Terminal" => {
            let arguments = Arguments::new(state.arguments.as_deref());
            let mut spans = Vec::new();
            if let Some(mode) = arguments.string("mode").filter(|mode| !mode.is_empty()) {
                spans.push(Span::styled(
                    format!(" · {}", sanitize_terminal_display(&mode, false)),
                    style,
                ));
                let handle = match mode.as_str() {
                    "start" => detail_string(state, "handle"),
                    "write" | "read" | "resize" | "stop" => arguments.string("handle"),
                    _ => None,
                };
                if let Some(handle) = handle
                    .map(|handle| sanitize_terminal_display(&handle, false))
                    .filter(|handle| !handle.is_empty())
                {
                    spans.push(Span::styled(format!(" · {handle}"), style));
                }
            }
            (!spans.is_empty()).then_some(spans)
        }
        _ => None,
    }
}

pub(super) fn render_body(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
    workspace_dir: Option<&Path>,
) -> Option<Vec<Line>> {
    match state.name.as_str() {
        "Bash" => render_bash_body(state, expanded, width, theme, workspace_dir),
        "Terminal" => render_terminal_body(state, expanded, width, theme, workspace_dir),
        _ => None,
    }
}

fn render_bash_body(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
    workspace_dir: Option<&Path>,
) -> Option<Vec<Line>> {
    let arguments = Arguments::new(state.arguments.as_deref());
    let command = arguments.string("command")?;
    let mut rows = cwd_rows(
        arguments.string("cwd").as_deref(),
        width,
        theme,
        workspace_dir,
    );
    rows.extend(command_preview(&command, expanded, width, theme));

    if is_backgrounded(state) {
        rows.push(background_summary(state, &arguments, width, theme));
    } else if let Some(result) = state.result.as_deref().filter(|result| !result.is_empty()) {
        rows.extend(render_text_preview_themed(result, expanded, width, theme));
    }
    (!rows.is_empty()).then_some(rows)
}

fn render_terminal_body(
    state: &ToolCallState,
    expanded: bool,
    width: usize,
    theme: &TuiTheme,
    workspace_dir: Option<&Path>,
) -> Option<Vec<Line>> {
    let arguments = Arguments::new(state.arguments.as_deref());
    let mode = arguments.string("mode");
    let details = state.details.as_ref();
    let structured_output = details
        .and_then(|details| details.get("output"))
        .and_then(Value::as_str);
    let output = structured_output
        .map(|output| sanitize_terminal_display(output, true))
        .filter(|output| !output.is_empty());
    let structured_status = details
        .and_then(|details| details.get("status"))
        .and_then(Value::as_str);
    let structured_error = structured_status.is_some_and(|status| {
        matches!(
            status,
            "failed" | "cancelled" | "error" | "timed_out" | "resource_limited" | "parent_exited"
        )
    });
    let failed = matches!(
        state.status,
        ToolStatusKind::Failed | ToolStatusKind::Cancelled
    ) || structured_error;
    let result = state
        .result
        .as_deref()
        .filter(|result| !result.is_empty())
        .map(|result| sanitize_terminal_display(result, true));
    let mut rows = Vec::new();
    let mut needs_result = false;
    let mut handled_empty_body = false;
    match mode.as_deref() {
        Some("start") => {
            if let Some(command) = arguments.string("command") {
                rows.extend(cwd_rows(
                    arguments.string("cwd").as_deref(),
                    width,
                    theme,
                    workspace_dir,
                ));
                rows.extend(command_preview(&command, expanded, width, theme));
            } else {
                needs_result = true;
            }
            if let Some(output) = &output {
                rows.extend(render_text_preview_themed(output, expanded, width, theme));
            } else if state.status == ToolStatusKind::Succeeded
                && structured_output.is_some()
                && !failed
            {
                rows.push(
                    Line::styled("  Terminal started.", muted_style(theme))
                        .truncate_to_width(width),
                );
            } else if structured_output.is_none() {
                needs_result = true;
            }
        }
        Some("write") => {
            if arguments.string("handle").is_some()
                && let Some(input) = arguments.terminal_input()
            {
                rows.extend(prefixed_rows(
                    &input,
                    "  stdin › ",
                    "          ",
                    Style::default().fg(theme.text_primary),
                    width,
                ));
            } else {
                needs_result = true;
            }
            if let Some(output) = &output {
                rows.extend(render_text_preview_themed(output, expanded, width, theme));
            } else if structured_output.is_none() {
                needs_result = true;
            }
        }
        Some("read") => {
            if arguments.string("handle").is_some() && structured_output.is_some() {
                handled_empty_body = true;
                if let Some(output) = &output {
                    rows.extend(render_text_preview_themed(output, expanded, width, theme));
                }
            } else {
                needs_result = true;
            }
        }
        Some("resize") => {
            if let (Some(_), Some(cols), Some(rows_count)) = (
                arguments.string("handle"),
                arguments.u64("cols"),
                arguments.u64("rows"),
            ) {
                rows.extend(prefixed_rows(
                    &format!("{cols} × {rows_count}"),
                    "  size ",
                    "       ",
                    muted_style(theme),
                    width,
                ));
            } else {
                needs_result = true;
            }
        }
        Some("stop") => {
            if arguments.string("handle").is_some()
                && structured_status.is_some()
                && structured_output.is_some()
            {
                if let Some(output) = &output {
                    rows.extend(render_text_preview_themed(output, expanded, width, theme));
                }
                if state.status == ToolStatusKind::Succeeded && !failed {
                    rows.push(
                        Line::styled("  Process tree stopped.", muted_style(theme))
                            .truncate_to_width(width),
                    );
                }
            } else {
                needs_result = true;
            }
        }
        Some(_) | None => needs_result = true,
    }

    if (failed || needs_result)
        && let Some(result) = &result
    {
        rows.extend(render_text_preview_themed(result, expanded, width, theme));
    }
    if rows.is_empty() {
        return handled_empty_body.then(Vec::new);
    }
    Some(
        rows.into_iter()
            .map(|row| row.truncate_to_width(width))
            .collect(),
    )
}

fn command_preview(command: &str, expanded: bool, width: usize, theme: &TuiTheme) -> Vec<Line> {
    let command = sanitize_terminal_display(command, true);
    let mut rows = Vec::new();
    for logical_line in highlight_code_lines(&command, "command.sh", theme) {
        for visual_line in wrap_spans(&logical_line, width.saturating_sub(4).max(1)) {
            let prefix = if rows.is_empty() { "  $ " } else { "    " };
            let mut spans = vec![Span::styled(prefix, muted_style(theme))];
            spans.extend(visual_line);
            rows.push(Line::from_spans(spans).truncate_to_width(width));
        }
    }

    if expanded || rows.len() <= 4 {
        return rows;
    }

    let hidden_chars = rows[3..rows.len() - 1]
        .iter()
        .flat_map(|line| line.spans().iter().skip(1))
        .map(|span| span.text().chars().count())
        .sum::<usize>();
    let last = rows.pop().expect("more than four command rows");
    rows.truncate(3);
    rows.push(
        Line::styled(
            format!("    ... {hidden_chars} characters hidden · ctrl+o to expand"),
            muted_style(theme),
        )
        .truncate_to_width(width),
    );
    rows.push(last);
    rows
}

fn cwd_rows(
    cwd: Option<&str>,
    width: usize,
    theme: &TuiTheme,
    workspace_dir: Option<&Path>,
) -> Vec<Line> {
    let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) else {
        return Vec::new();
    };
    let cwd = sanitize_terminal_display(&make_workspace_relative(cwd, workspace_dir), false);
    prefixed_rows(&cwd, "  cwd ", "      ", muted_style(theme), width)
}

fn prefixed_rows(
    text: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    style: Style,
    width: usize,
) -> Vec<Line> {
    let mut rows = Vec::new();
    for logical_line in text.lines() {
        let prefix = if rows.is_empty() {
            first_prefix
        } else {
            continuation_prefix
        };
        let prefix_width = crate::primitive::visible_width(prefix);
        for visual_line in wrap_spans(
            &[Span::styled(logical_line, style)],
            width.saturating_sub(prefix_width).max(1),
        ) {
            let prefix = if rows.is_empty() {
                first_prefix
            } else {
                continuation_prefix
            };
            let mut spans = vec![Span::styled(prefix, style)];
            spans.extend(visual_line);
            rows.push(Line::from_spans(spans).truncate_to_width(width));
        }
    }
    rows
}

fn background_summary(
    state: &ToolCallState,
    arguments: &Arguments<'_>,
    width: usize,
    theme: &TuiTheme,
) -> Line {
    let task_id = detail_string(state, "task_id");
    let description = detail_string(state, "description")
        .or_else(|| arguments.string("description"))
        .filter(|description| !description.is_empty());
    let text = match (task_id, description) {
        (Some(task_id), Some(description)) => format!("  task {task_id} · {description}"),
        (Some(task_id), None) => format!("  task {task_id}"),
        (None, Some(description)) => format!("  background · {description}"),
        (None, None) => "  background".to_owned(),
    };
    Line::styled(text, muted_style(theme)).truncate_to_width(width)
}

fn is_backgrounded(state: &ToolCallState) -> bool {
    state
        .details
        .as_ref()
        .and_then(|details| details.get("outcome"))
        .and_then(Value::as_str)
        == Some("backgrounded")
}

fn detail_string(state: &ToolCallState, field: &str) -> Option<String> {
    state
        .details
        .as_ref()?
        .get(field)?
        .as_str()
        .map(ToOwned::to_owned)
}

fn sanitize_terminal_display(text: &str, preserve_newlines: bool) -> String {
    let mut sanitized = String::new();
    let mut index = 0;
    while index < text.len() {
        if let Some(sequence) = crate::primitive::next_sequence(text, index)
            && sequence
                .chars()
                .nth(1)
                .is_some_and(|character| ('\x20'..='\x7e').contains(&character))
        {
            index += sequence.len();
            continue;
        }
        let character = text[index..].chars().next().expect("valid UTF-8 boundary");
        match character {
            '\n' if preserve_newlines => sanitized.push('\n'),
            '\n' => sanitized.push_str("\\n"),
            '\r' => sanitized.push_str("\\r"),
            '\t' => sanitized.push_str("\\t"),
            '\u{1b}' => sanitized.push_str("\\x1b"),
            character if character.is_control() => sanitized.extend(character.escape_default()),
            character => sanitized.push(character),
        }
        index += character.len_utf8();
    }
    sanitized
}

fn escape_terminal_input(input: &str) -> String {
    let mut escaped = String::new();
    for character in input.chars() {
        match character {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{1b}' => escaped.push_str("\\x1b"),
            '\\' => escaped.push_str("\\\\"),
            character if character.is_control() => escaped.extend(character.escape_default()),
            character => escaped.push(character),
        }
    }
    escaped
}

fn muted_style(theme: &TuiTheme) -> Style {
    Style {
        fg: Some(theme.text_muted),
        ..Style::default()
    }
}

struct Arguments<'a> {
    raw: Option<&'a str>,
    parsed: Option<Value>,
}

impl<'a> Arguments<'a> {
    fn new(raw: Option<&'a str>) -> Self {
        Self {
            raw,
            parsed: raw.and_then(|raw| serde_json::from_str(raw).ok()),
        }
    }

    fn string(&self, field: &str) -> Option<String> {
        if let Some(parsed) = &self.parsed {
            return parsed.get(field)?.as_str().map(ToOwned::to_owned);
        }
        extract_partial_string_field(self.raw?, field)
    }

    fn u64(&self, field: &str) -> Option<u64> {
        self.parsed.as_ref()?.get(field)?.as_u64()
    }

    fn terminal_input(&self) -> Option<String> {
        if let Some(parsed) = &self.parsed {
            let parts = parsed.get("input")?.as_array()?;
            if parts.is_empty() {
                return None;
            }
            let mut rendered = String::new();
            for part in parts {
                let part = part.as_object()?;
                match (part.get("text"), part.get("control")) {
                    (Some(text), None) if part.len() == 1 => {
                        rendered.push_str(&escape_terminal_input(text.as_str()?));
                    }
                    (None, Some(control)) if part.len() == 1 => {
                        let control = u8::try_from(control.as_u64()?).ok()?;
                        if !matches!(control, 0..=31 | 127) {
                            return None;
                        }
                        rendered.extend(std::ascii::escape_default(control).map(char::from));
                    }
                    _ => return None,
                }
            }
            return Some(rendered);
        }

        let raw = self.raw?;
        if raw.matches("\"text\"").count() != 1 || raw.contains("\"control\"") {
            return None;
        }
        extract_partial_string_field(raw, "text").map(|text| escape_terminal_input(&text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_preview_preserves_text_head_tail_and_width() {
        let command = concat!(
            "printf 'first line with quote'\n",
            "echo \"second 世界\"\n",
            "\x1b[31mecho ansi-red\x1b[0m\n",
            "echo fourth\n",
            "echo fifth\n",
            "very_long_unbroken_token_abcdefghijklmnopqrstuvwxyz_0123456789_TAIL"
        );
        let sanitized = crate::primitive::strip_ansi(command);
        let theme = TuiTheme::default();

        for width in [60, 100] {
            let collapsed = command_preview(command, false, width, &theme);
            let collapsed_text = collapsed
                .iter()
                .map(Line::text)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(collapsed_text.starts_with("  $ printf 'first line with quote'"));
            let expected_hidden = if width == 60 { 77 } else { 21 };
            assert!(collapsed_text.contains(&format!("{expected_hidden} characters hidden")));
            assert!(collapsed_text.ends_with("TAIL"));
            assert!(collapsed.iter().all(|line| line.visible_width() <= width));
            assert!(
                collapsed
                    .iter()
                    .flat_map(Line::spans)
                    .all(|span| !span.text().contains('\u{1b}'))
            );

            let expanded = command_preview(command, true, width, &theme);
            let expanded_text = expanded
                .iter()
                .flat_map(|line| line.text().chars().skip(4).collect::<Vec<_>>())
                .collect::<String>();
            assert_eq!(expanded_text, sanitized.lines().collect::<String>());
            assert!(expanded.iter().all(|line| line.visible_width() <= width));
            assert!(expanded.iter().all(|line| !line.text().contains('\n')));
        }

        let unsafe_command = "printf audit\t\x1b[31mred\x1b[0m\x03\nprintf tail";
        let unsafe_rows = command_preview(unsafe_command, true, 40, &theme);
        let unsafe_text = unsafe_rows.iter().map(Line::text).collect::<String>();
        assert!(
            unsafe_text.contains(r"printf audit\tred\u{3}"),
            "{unsafe_text}"
        );
        assert!(unsafe_text.contains("printf tail"), "{unsafe_text}");
        assert!(unsafe_rows.iter().all(|line| line.visible_width() <= 40));
        assert!(
            unsafe_rows
                .iter()
                .flat_map(Line::spans)
                .all(|span| { span.text().chars().all(|character| !character.is_control()) })
        );

        let terminal_write = ToolCallState {
            id: "terminal-write".to_owned(),
            name: "Terminal".to_owned(),
            arguments: Some(
                serde_json::json!({
                    "mode": "write",
                    "handle": "term-1",
                    "input": [
                        {"text": "alpha\n世界 literal \\x03:"},
                        {"control": 3},
                        {"text": "beta"},
                        {"control": 27},
                        {"control": 127}
                    ]
                })
                .to_string(),
            ),
            result: None,
            details: None,
            status: crate::shell::ToolStatusKind::Running,
            exit_code: None,
        };
        let terminal_rows = render_body(&terminal_write, false, 100, &theme, None).unwrap();
        let terminal_text = terminal_rows.iter().map(Line::text).collect::<String>();
        assert!(terminal_text.contains(r"alpha\n世界 literal \\x03:\x03beta\x1b\x7f"));
        assert!(
            terminal_text
                .chars()
                .all(|character| !character.is_control())
        );

        let boilerplate = "handle: term-1\nstatus: running\noutput:\n";
        let mut successful_write = terminal_write.clone();
        successful_write.status = ToolStatusKind::Succeeded;
        successful_write.result = Some(boilerplate.to_owned());
        successful_write.details = Some(serde_json::json!({
            "handle": "term-1",
            "status": "running",
            "output": ""
        }));
        let successful_start = ToolCallState {
            id: "terminal-start".to_owned(),
            name: "Terminal".to_owned(),
            arguments: Some(
                serde_json::json!({"mode": "start", "command": "printf ready"}).to_string(),
            ),
            result: Some(boilerplate.to_owned()),
            details: Some(serde_json::json!({
                "handle": "term-1",
                "status": "running",
                "output": ""
            })),
            status: ToolStatusKind::Succeeded,
            exit_code: None,
        };
        let successful_resize = ToolCallState {
            id: "terminal-resize".to_owned(),
            name: "Terminal".to_owned(),
            arguments: Some(
                serde_json::json!({
                    "mode": "resize",
                    "handle": "term-1",
                    "cols": 80,
                    "rows": 24
                })
                .to_string(),
            ),
            result: Some(boilerplate.to_owned()),
            details: Some(serde_json::json!({
                "handle": "term-1",
                "status": "running",
                "output": ""
            })),
            status: ToolStatusKind::Running,
            exit_code: None,
        };
        for (state, audit_text) in [
            (successful_start, "printf ready"),
            (successful_write, r"literal \\x03"),
            (successful_resize, "size 80 × 24"),
        ] {
            let text = render_body(&state, false, 100, &theme, None)
                .unwrap()
                .iter()
                .map(Line::text)
                .collect::<String>();
            assert!(text.contains(audit_text));
            assert!(!text.contains("handle:"));
            assert!(!text.contains("status:"));
            assert!(!text.contains("output:"));
        }

        let mut failed_write = terminal_write.clone();
        failed_write.status = ToolStatusKind::Failed;
        failed_write.result = Some("terminal write failed: unknown handle".to_owned());
        let failed_write_rows = render_body(&failed_write, false, 100, &theme, None).unwrap();
        let failed_write_text = failed_write_rows.iter().map(Line::text).collect::<String>();
        assert!(failed_write_text.contains(r"literal \\x03:\x03beta"));
        assert!(failed_write_text.contains("terminal write failed: unknown handle"));

        let mut parent_exited_write = terminal_write.clone();
        parent_exited_write.result = Some("terminal guardian parent exited".to_owned());
        parent_exited_write.details = Some(serde_json::json!({
            "status": "parent_exited",
            "output": ""
        }));
        let parent_exited_text = render_body(&parent_exited_write, false, 100, &theme, None)
            .unwrap()
            .iter()
            .map(Line::text)
            .collect::<String>();
        assert!(parent_exited_text.contains("terminal guardian parent exited"));

        for mode in ["read", "stop"] {
            let state = ToolCallState {
                id: mode.to_owned(),
                name: "Terminal".to_owned(),
                arguments: Some(serde_json::json!({"mode": mode, "handle": "term-1"}).to_string()),
                result: Some("human-readable result must remain generic".to_owned()),
                details: Some(serde_json::json!({"handle": "term-1"})),
                status: crate::shell::ToolStatusKind::Succeeded,
                exit_code: None,
            };
            let text = render_body(&state, false, 100, &theme, None)
                .unwrap()
                .iter()
                .map(Line::text)
                .collect::<String>();
            assert!(text.contains("human-readable result must remain generic"));
        }

        let bash_background = ToolCallState {
            id: "bash-background".to_owned(),
            name: "Bash".to_owned(),
            arguments: Some(
                serde_json::json!({
                    "command": "cargo test",
                    "run_in_background": true,
                    "description": "run focused tests"
                })
                .to_string(),
            ),
            result: None,
            details: None,
            status: crate::shell::ToolStatusKind::Running,
            exit_code: None,
        };
        let request_rows = render_body(&bash_background, false, 100, &theme, None).unwrap();
        let request_text = request_rows.iter().map(Line::text).collect::<String>();
        assert!(!request_text.contains("background"));
        assert_eq!(header_metadata(&bash_background, &theme), Some(Vec::new()));

        let mut confirmed_background = bash_background.clone();
        confirmed_background.status = ToolStatusKind::Succeeded;
        confirmed_background.details = Some(serde_json::json!({
            "outcome": "backgrounded",
            "task_id": "bash-1"
        }));
        let background_rows = render_body(&confirmed_background, false, 100, &theme, None).unwrap();
        let background_text = background_rows.iter().map(Line::text).collect::<String>();
        assert!(background_text.contains("task bash-1 · run focused tests"));
        assert_eq!(
            header_metadata(&confirmed_background, &theme)
                .unwrap()
                .iter()
                .map(Span::text)
                .collect::<String>(),
            " · background"
        );

        let mut failed_background = bash_background.clone();
        failed_background.status = ToolStatusKind::Failed;
        failed_background.result = Some("background launch failed".to_owned());
        failed_background.details = Some(serde_json::json!({"outcome": "failed"}));
        let failed_background_rows =
            render_body(&failed_background, false, 100, &theme, None).unwrap();
        let failed_background_text = failed_background_rows
            .iter()
            .map(Line::text)
            .collect::<String>();
        assert!(failed_background_text.contains("background launch failed"));
        assert!(!failed_background_text.contains("background · run focused tests"));

        let mut bash_foreground = bash_background.clone();
        bash_foreground.id = "bash-foreground".to_owned();
        bash_foreground.arguments =
            Some(serde_json::json!({"command": "cargo test --lib focused"}).to_string());
        assert_eq!(header_metadata(&bash_foreground, &theme), Some(Vec::new()));

        let empty_terminal = ToolCallState {
            id: "empty-terminal".to_owned(),
            name: "Terminal".to_owned(),
            arguments: None,
            result: None,
            details: None,
            status: crate::shell::ToolStatusKind::Pending,
            exit_code: None,
        };
        assert!(header_metadata(&empty_terminal, &theme).is_none());
    }
}
