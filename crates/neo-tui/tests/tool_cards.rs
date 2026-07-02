use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Component, Expandable, Finalization, Line};
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::diff_preview::render_diff_lines_clustered;
use neo_tui::transcript::tool_renderers::tool_header_spans;
use neo_tui::transcript::{ToolCallComponent, ToolCallState, TranscriptPane};
use std::fmt::Write as _;

fn plain(rows: Vec<Line>) -> Vec<String> {
    rows.into_iter()
        .map(|row| neo_tui::primitive::strip_ansi(&row.to_ansi()))
        .collect()
}

#[test]
fn running_tool_header_uses_finished_status_color() {
    let theme = TuiTheme::default();
    let running = ToolCallState {
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: None,
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    };
    let used = ToolCallState {
        status: ToolStatusKind::Succeeded,
        ..running.clone()
    };

    assert_eq!(
        tool_header_spans(&running, &theme, None)[0].to_ansi(),
        tool_header_spans(&used, &theme, None)[0].to_ansi()
    );
}

#[test]
fn tool_call_renders_running_header_and_key_arg() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: Some(r#"{"path":"crates/neo-tui/src/app.rs"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    assert!(
        rows.iter()
            .any(|line| line.contains("● Using Read (crates/neo-tui/src/app.rs)"))
    );
    assert_eq!(card.finalization(), Finalization::Live);
}

#[test]
fn tool_call_updates_in_place_to_finished_state() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Read".to_owned(),
        arguments: Some(r#"{"path":"README.md"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });

    card.set_result(Some("line one\nline two".to_owned()), None, false, None);

    let rows = plain(card.render(80));
    assert!(
        rows.iter()
            .any(|line| line.contains("● Used Read (README.md)"))
    );
    assert!(rows.iter().any(|line| line.contains("2 lines")));
    assert_eq!(card.finalization(), Finalization::Finalized);
}

#[test]
fn unrecognized_json_keys_omit_parens_in_header() {
    use neo_tui::primitive::visible_width;
    use neo_tui::transcript::frame_content_width;

    const WIDTH: usize = 80;
    let args = serde_json::json!({
        "questions": [{
            "question": "1 + 2 × 3 = ?",
            "header": "单选题",
            "options": [
                {"label": "7", "description": "先乘除后加减：2×3=6，1+6=7"},
                {"label": "9", "description": "从左到右：(1+2)×3=9"},
                {"label": "6", "description": "1+2+3=6"},
                {"label": "Other"}
            ],
            "multi_select": false
        }]
    });
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "question-1".to_owned(),
        name: "AskUserQuestion".to_owned(),
        arguments: Some(args.to_string()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });

    let rows = plain(card.render(WIDTH));
    let content_width = frame_content_width(WIDTH);

    assert!(
        rows.iter()
            .any(|line| line.contains("Using AskUserQuestion"))
    );
    assert_eq!(rows.len(), 1, "header should stay compact: {rows:?}");
    // Unrecognized-key JSON no longer leaks as a raw-arg suffix, so the
    // header is short and carries no `(...)` parens.
    assert!(
        !rows[0].contains('('),
        "header must not show raw-args parens: {rows:?}"
    );
    assert!(
        rows.iter().all(|line| visible_width(line) <= content_width),
        "all rows must fit content width {content_width}: {rows:?}"
    );
}

#[test]
fn successful_todo_list_tool_card_hides_redundant_result_body() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "todo-1".to_owned(),
        name: "TodoList".to_owned(),
        arguments: Some(r#"{"todos":[{"title":"ship","status":"in_progress"}]}"#.to_owned()),
        result: Some("Current todo list:\n  [in_progress] ship".to_owned()),
        details: None,
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(80));

    assert!(rows.iter().any(|line| line.contains("Used TodoList")));
    assert!(!rows.iter().any(|line| line.contains("[in_progress] ship")));
}

#[test]
fn empty_args_tool_header_omits_parens() {
    let theme = TuiTheme::default();
    let state = ToolCallState {
        id: "plan-1".to_owned(),
        name: "EnterPlanMode".to_owned(),
        arguments: Some("{}".to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    };
    let rows = plain(vec![Line::from_spans(tool_header_spans(
        &state, &theme, None,
    ))]);
    let header = &rows[0];
    assert!(
        header.contains("Used EnterPlanMode"),
        "header should name the tool: {header:?}"
    );
    assert!(
        !header.contains("({})"),
        "header must not show empty-args parens: {header:?}"
    );
}

#[test]
fn failed_todo_list_tool_card_keeps_error_body() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "todo-1".to_owned(),
        name: "TodoList".to_owned(),
        arguments: Some(r#"{"todos":[{"title":"ship","status":"wip"}]}"#.to_owned()),
        result: Some("invalid status".to_owned()),
        details: None,
        status: ToolStatusKind::Failed,
        exit_code: None,
    });

    let rows = plain(card.render(80));

    assert!(rows.iter().any(|line| line.contains("TodoList")));
    assert!(rows.iter().any(|line| line.contains("invalid status")));
}

#[test]
fn ctrl_o_expansion_switches_preview_limit() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: Some(r#"{"command":"printf many"}"#.to_owned()),
        result: Some("1\n2\n3\n4\n5\n6\n7\n8".to_owned()),
        details: None,
        status: ToolStatusKind::Succeeded,
        exit_code: Some(0),
    });

    let collapsed = plain(card.render(80));
    assert!(collapsed.iter().any(|line| line.contains("more lines")));

    card.set_expanded(true);
    let expanded = plain(card.render(80));
    assert!(expanded.iter().any(|line| line.trim() == "8"));
}

#[test]
fn write_tool_card_renders_finalized_diff_from_details() {
    let content = (1..=20)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut diff_body = String::new();
    for line_number in 1..=20 {
        writeln!(diff_body, "+line {line_number}").expect("write diff line");
    }
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Write".to_owned(),
        arguments: Some(
            serde_json::json!({
                "path": "src/generated.rs",
                "content": content,
            })
            .to_string(),
        ),
        result: Some("wrote src/generated.rs".to_owned()),
        details: Some(serde_json::json!({
            "path": "src/generated.rs",
            "operation": "created",
            "added": 20,
            "removed": 0,
            "line_count": 20,
            "diff": format!("--- src/generated.rs\n+++ src/generated.rs\n@@ -0,0 +1,20 @@\n{diff_body}")
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    assert!(
        rows.iter()
            .any(|line| line.contains("Used Write (src/generated.rs) · 1 lines"))
    );
    assert!(rows.iter().any(|line| line.contains("ctrl+o to expand")));
    // New files show a syntax-highlighted preview, not an all-green diff.
    assert!(rows.iter().any(|line| line.contains(" 1 line 1")));
    assert!(!rows.iter().any(|line| line.contains("20 line 20")));

    card.set_expanded(true);
    let expanded = plain(card.render(80));
    assert!(expanded.iter().any(|line| line.contains("20 line 20")));
}

#[test]
fn bash_running_card_shows_live_output_tail() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: Some(r#"{"command":"cargo test"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });

    for n in 1..=10 {
        card.append_live_output(format!("line {n}"));
    }

    let rows = plain(card.render(80));
    assert!(rows.iter().any(|line| line.contains("cargo test")));
    assert!(rows.iter().any(|line| line.contains("line 10")));
    assert!(rows.iter().any(|line| line.contains("earlier lines")));
    assert!(!rows.iter().any(|line| line.trim() == "line 1"));
}

#[test]
fn edit_diff_preview_clusters_changes_with_context_and_hidden_footer() {
    let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
    let new = "a\nb changed\nc\nd\ne\nf\ng changed\nh\ni\nj\n";

    let rows = render_diff_lines_clustered(old, new, "src/lib.rs", 1, Some(4));
    let plain: Vec<String> = rows
        .into_iter()
        .map(|row| neo_tui::primitive::strip_ansi(&row.to_ansi()))
        .collect();

    assert!(plain[0].contains("+2 -2 src/lib.rs"));
    assert!(plain.iter().any(|line| line.contains("- b")));
    assert!(plain.iter().any(|line| line.contains("+ b changed")));
    assert!(
        plain
            .iter()
            .any(|line| line.contains("more changes hidden"))
    );
}

#[test]
fn edit_tool_card_renders_finalized_real_line_diff_from_details() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Edit".to_owned(),
        arguments: Some(
            serde_json::json!({
                "path": "src/lib.rs",
                "old": "old",
                "new": "new"
            })
            .to_string(),
        ),
        result: Some("edited src/lib.rs".to_owned()),
        details: Some(serde_json::json!({
            "path": "src/lib.rs",
            "old": "old",
            "new": "new",
            "replace_all": false,
            "diff": "--- src/lib.rs\n+++ src/lib.rs\n@@ -40,3 +40,3 @@\n context\n-old\n+new\n tail\n"
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    assert!(
        rows.iter()
            .any(|line| line.contains("Used Edit (src/lib.rs) · +1 -1"))
    );
    assert!(rows.iter().any(|line| line.contains("41 - old")));
    assert!(rows.iter().any(|line| line.contains("41 + new")));
    assert!(
        !rows
            .iter()
            .any(|line| line.contains(" 1 - old") || line.contains(" 1 + new")),
        "finalized Edit must not use args-local line numbers: {rows:?}"
    );
}

#[test]
fn transcript_pane_expansion_state_is_instance_local() {
    let mut expanded_pane = TranscriptPane::new(80, 12);
    let collapsed_pane = TranscriptPane::new(80, 12);

    expanded_pane.set_tool_output_expanded(true);

    assert!(expanded_pane.tool_output_expanded());
    assert!(!collapsed_pane.tool_output_expanded());
}

#[test]
fn transcript_pane_expansion_reaches_rendered_bash_tool_body() {
    use neo_agent_core::AgentEvent;
    use neo_tui::primitive::strip_ansi;

    let mut runtime = TranscriptPane::new(80, 20);
    runtime.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "bash-1".to_owned(),
        json_fragment: r#"{"command":"printf many"}"#.to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok("1\n2\n3\n4\n5\n6\n7\n8"),
    });

    let collapsed = runtime
        .render_frame(80, 20)
        .expect("collapsed frame")
        .iter()
        .map(|line| strip_ansi(line).clone())
        .collect::<Vec<_>>();
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("ctrl+o to expand")),
        "collapsed frame should show expansion hint: {collapsed:?}"
    );
    assert!(
        !collapsed.iter().any(|line| line.trim() == "8"),
        "collapsed frame should not show final result line: {collapsed:?}"
    );

    runtime.set_tool_output_expanded(true);
    let expanded = runtime
        .render_frame(80, 20)
        .expect("expanded frame")
        .iter()
        .map(|line| strip_ansi(line).clone())
        .collect::<Vec<_>>();
    assert!(
        expanded.iter().any(|line| line.trim() == "8"),
        "expanded frame should show final result line: {expanded:?}"
    );
    assert!(
        !expanded
            .iter()
            .any(|line| line.contains("ctrl+o to expand")),
        "expanded frame should hide expansion hint: {expanded:?}"
    );
}

#[test]
fn tool_card_lines_do_not_exceed_terminal_width_after_gutter() {
    // Regression for the post-turn duplicate/right-shift bug: tool-card rows
    // were rendered at the full terminal width, then the TUI applied a 1-col
    // gutter, pushing them one column past the edge. The terminal wrapped the
    // extra column and the differential renderer lost track of cursor rows.
    use neo_agent_core::AgentEvent;
    use neo_tui::primitive::{strip_ansi, visible_width};
    use neo_tui::transcript::{apply_gutter, frame_content_width};

    const WIDTH: usize = 40;
    let mut runtime = TranscriptPane::new(WIDTH, 20);

    runtime.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "read-0".to_owned(),
        name: "Read".to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "read-0".to_owned(),
        json_fragment: r#"{"path":"src/lib.rs"}"#.to_owned(),
    });
    // Result line is intentionally wider than the terminal so the wrapped body
    // would have hit the right edge before the fix.
    runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "read-0".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("x".repeat(200)),
    });

    let frame = runtime
        .render_frame(WIDTH, 20)
        .expect("frame renders")
        .iter()
        .map(|line| strip_ansi(line).clone())
        .collect::<Vec<_>>();

    // Sanity-check the invariant that makes the gutter safe: the body was
    // composed at content_width, not full terminal width.
    let content_width = frame_content_width(WIDTH);
    assert!(
        frame
            .iter()
            .filter(|line| line.contains("Used Read"))
            .all(|line| visible_width(line) <= content_width),
        "header should fit in content width {content_width}"
    );

    let mut frame_with_gutter = frame.clone();
    apply_gutter(&mut frame_with_gutter);

    let mut tool_card_header_count = 0;
    for line in &frame_with_gutter {
        if line.is_empty() {
            continue;
        }
        let w = visible_width(line);
        assert!(
            w < WIDTH,
            "line reaches terminal autowrap column ({w} >= {WIDTH}): {line:?}"
        );
        if line.contains("Used Read") {
            tool_card_header_count += 1;
        }
    }
    assert_eq!(tool_card_header_count, 1, "tool card header rendered once");
    assert!(
        frame_with_gutter
            .iter()
            .any(|line| line.contains("ctrl+o to expand")),
        "overflow hint present: {frame_with_gutter:?}"
    );
}

#[test]
fn ask_user_question_header_does_not_exceed_terminal_width_after_gutter() {
    use neo_agent_core::AgentEvent;
    use neo_tui::primitive::{strip_ansi, visible_width};
    use neo_tui::transcript::apply_gutter;

    const WIDTH: usize = 80;
    let args = serde_json::json!({
        "questions": [{
            "question": "1 + 2 × 3 = ?",
            "header": "单选题",
            "options": [
                {"label": "7", "description": "先乘除后加减：2×3=6，1+6=7"},
                {"label": "9", "description": "从左到右：(1+2)×3=9"},
                {"label": "6", "description": "1+2+3=6"},
                {"label": "Other"}
            ],
            "multi_select": false
        }]
    });
    let mut runtime = TranscriptPane::new(WIDTH, 20);

    runtime.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "question-1".to_owned(),
        name: "AskUserQuestion".to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "question-1".to_owned(),
        json_fragment: args.to_string(),
    });

    let mut frame = runtime
        .render_frame(WIDTH, 20)
        .expect("frame renders")
        .iter()
        .map(|line| strip_ansi(line).clone())
        .collect::<Vec<_>>();
    apply_gutter(&mut frame);

    assert!(
        frame
            .iter()
            .any(|line| line.contains("Queued AskUserQuestion")),
        "tool header present: {frame:?}"
    );
    for line in &frame {
        let width = visible_width(line);
        assert!(
            width < WIDTH,
            "line reaches terminal autowrap column ({width} >= {WIDTH}): {line:?}"
        );
    }
}

#[test]
fn grouped_read_lines_do_not_exceed_terminal_width_after_gutter() {
    use neo_agent_core::AgentEvent;
    use neo_tui::primitive::{strip_ansi, visible_width};
    use neo_tui::transcript::{apply_gutter, frame_content_width};

    const WIDTH: usize = 30;
    let mut runtime = TranscriptPane::new(WIDTH, 20);

    for (idx, path) in ["very/long/path/to/alpha.rs", "very/long/path/to/beta.rs"]
        .into_iter()
        .enumerate()
    {
        let id = format!("read-{idx}");
        runtime.apply_agent_event(AgentEvent::ToolCallStarted {
            turn: 1,
            id: id.clone(),
            name: "Read".to_owned(),
        });
        runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
            turn: 1,
            id: id.clone(),
            json_fragment: format!(r#"{{"path":"{path}"}}"#),
        });
        runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id,
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("ok"),
        });
    }

    let frame = runtime
        .render_frame(WIDTH, 20)
        .expect("frame renders")
        .iter()
        .map(|line| strip_ansi(line).clone())
        .collect::<Vec<_>>();

    // Grouped rows should be truncated to content_width, not full width.
    let content_width = frame_content_width(WIDTH);
    assert!(
        frame
            .iter()
            .filter(|line| line.contains("Read 2 files") || line.contains("very/long"))
            .all(|line| visible_width(line) <= content_width),
        "grouped rows must fit in content width {content_width}"
    );

    let mut frame_with_gutter = frame.clone();
    apply_gutter(&mut frame_with_gutter);

    assert!(
        frame_with_gutter
            .iter()
            .any(|line| line.contains("Read 2 files")),
        "group header present: {frame_with_gutter:?}"
    );

    for line in &frame_with_gutter {
        if line.is_empty() {
            continue;
        }
        let w = visible_width(line);
        assert!(
            w < WIDTH,
            "grouped tool line reaches terminal autowrap column ({w} >= {WIDTH}): {line:?}"
        );
    }
}

#[test]
fn exit_plan_mode_header_shows_approved_without_label() {
    use neo_tui::transcript::tool_renderers::exit_plan_mode_header_spans;

    let theme = TuiTheme::default();
    let state = ToolCallState {
        id: "plan-1".to_owned(),
        name: "ExitPlanMode".to_owned(),
        arguments: Some("{}".to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    };

    let rows = plain(vec![Line::from_spans(exit_plan_mode_header_spans(
        &state, &theme,
    ))]);
    let header = &rows[0];
    assert!(
        header.contains("Current plan"),
        "header should say 'Current plan': {header:?}"
    );
    assert!(
        header.contains("Approved"),
        "header should show 'Approved' on success: {header:?}"
    );
    assert!(
        !header.contains("ExitPlanMode"),
        "header should not show generic tool name: {header:?}"
    );
}

#[test]
fn exit_plan_mode_header_shows_approved_with_label() {
    use neo_tui::transcript::tool_renderers::exit_plan_mode_header_spans;

    let theme = TuiTheme::default();
    let state = ToolCallState {
        id: "plan-1".to_owned(),
        name: "ExitPlanMode".to_owned(),
        arguments: Some("{}".to_owned()),
        result: None,
        details: Some(serde_json::json!({
            "plan_selected_label": "incremental",
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    };

    let rows = plain(vec![Line::from_spans(exit_plan_mode_header_spans(
        &state, &theme,
    ))]);
    let header = &rows[0];
    assert!(
        header.contains("Current plan"),
        "header should say 'Current plan': {header:?}"
    );
    assert!(
        header.contains("Approved: incremental"),
        "header should show 'Approved: incremental': {header:?}"
    );
}

#[test]
fn exit_plan_mode_header_shows_rejected_on_failure() {
    use neo_tui::transcript::tool_renderers::exit_plan_mode_header_spans;

    let theme = TuiTheme::default();
    let state = ToolCallState {
        id: "plan-1".to_owned(),
        name: "ExitPlanMode".to_owned(),
        arguments: Some("{}".to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Failed,
        exit_code: None,
    };

    let rows = plain(vec![Line::from_spans(exit_plan_mode_header_spans(
        &state, &theme,
    ))]);
    let header = &rows[0];
    assert!(
        header.contains("Current plan"),
        "header should say 'Current plan': {header:?}"
    );
    assert!(
        header.contains("Rejected"),
        "header should show 'Rejected' on failure: {header:?}"
    );
    assert!(
        !header.contains("Approved"),
        "header should not show 'Approved' on failure: {header:?}"
    );
}

#[test]
fn long_command_header_keeps_closing_paren() {
    let theme = TuiTheme::default();
    let state = ToolCallState {
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: Some(
            serde_json::json!({
                "command": "cargo nextest run -p neo-agent-core --test runtime_turn runtime_turn_and_then_some_more_stuff",
            })
            .to_string(),
        ),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    };
    let rows = plain(vec![Line::from_spans(tool_header_spans(
        &state, &theme, None,
    ))]);
    let header = &rows[0];
    assert!(
        header.contains(')'),
        "closing parenthesis should survive truncation: {header:?}"
    );
    assert!(
        header.contains("..."),
        "long argument should be truncated: {header:?}"
    );
}

#[test]
fn long_path_header_preserves_tail() {
    let theme = TuiTheme::default();
    let state = ToolCallState {
        id: "read-1".to_owned(),
        name: "Read".to_owned(),
        arguments: Some(
            serde_json::json!({
                "path": "crates/neo-agent-core/src/tools/something/very/deep/terminal.rs",
            })
            .to_string(),
        ),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    };
    let rows = plain(vec![Line::from_spans(tool_header_spans(
        &state, &theme, None,
    ))]);
    let header = &rows[0];
    assert!(
        header.contains("…"),
        "long path should be truncated: {header:?}"
    );
    assert!(
        header.contains("terminal.rs"),
        "filename tail should be preserved: {header:?}"
    );
    assert!(
        header.contains(')'),
        "closing parenthesis should be visible: {header:?}"
    );
}

#[test]
fn write_streaming_preview_reuses_final_format() {
    use neo_agent_core::AgentEvent;
    use neo_tui::primitive::strip_ansi;

    let mut runtime = TranscriptPane::new(80, 20);
    runtime.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "write-1".to_owned(),
        name: "Write".to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "write-1".to_owned(),
        json_fragment: r#"{"path":"src/foo.rs","content":"use std::collections::HashMap;\n\npub f""#.to_owned(),
    });

    let frame = runtime
        .render_frame(80, 20)
        .expect("frame renders")
        .iter()
        .map(|line| strip_ansi(line).clone())
        .collect::<Vec<_>>();

    // Should NOT contain the old progress line format.
    assert!(
        !frame.iter().any(|line| line.contains("Preparing changes")),
        "streaming preview should not show old progress line: {frame:?}"
    );
    // Content should be rendered with the final preview format (line numbers).
    assert!(
        frame
            .iter()
            .any(|line| line.contains("use std::collections::HashMap")),
        "streaming content should be visible: {frame:?}"
    );
    assert!(
        frame
            .iter()
            .any(|line| line.contains("src/foo.rs") && line.contains("lines")),
        "streaming preview should show path header: {frame:?}"
    );
}

#[test]
fn edit_streaming_preview_shows_progress() {
    use neo_agent_core::AgentEvent;
    use neo_tui::primitive::strip_ansi;

    let mut runtime = TranscriptPane::new(80, 20);
    runtime.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "edit-1".to_owned(),
        name: "Edit".to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "edit-1".to_owned(),
        json_fragment: r#"{"path":"src/foo.rs","old":"foo","new":"bar"}"#.to_owned(),
    });

    let frame = runtime
        .render_frame(80, 20)
        .expect("frame renders")
        .iter()
        .map(|line| strip_ansi(line).clone())
        .collect::<Vec<_>>();

    assert!(
        !frame.iter().any(|line| line.contains("Preparing changes")),
        "streaming preview should not show old progress line: {frame:?}"
    );
    assert!(
        frame.iter().any(|line| line.contains("Editing src/foo.rs")),
        "Edit progress line should show path: {frame:?}"
    );
    assert!(
        frame.iter().any(|line| line.contains("tok")),
        "streaming preview should show token count: {frame:?}"
    );
}

#[test]
fn key_argument_ignores_legacy_file_path_alias() {
    let argument =
        neo_tui::transcript::tool_renderers::key_argument(Some(r#"{"file_path":"src/legacy.rs"}"#));

    assert!(argument.is_empty());
}

#[test]
fn write_streaming_uses_preview_format() {
    use neo_tui::transcript::ToolCallComponent;

    let state = ToolCallState {
        id: "stream-1".to_string(),
        name: "Write".to_string(),
        arguments: Some(
            r##"{"path":"/tmp/test.md","content":"# Title\nLine 2\nLine 3"}"##.to_string(),
        ),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    };
    let mut comp = ToolCallComponent::new(state);
    let lines = comp.render_with_theme(80, &TuiTheme::default());
    let body_text = lines.iter().map(Line::to_ansi).collect::<String>();
    // Should NOT contain the old progress line
    assert!(
        !body_text.contains("Preparing changes"),
        "streaming preview should not show progress line"
    );
    // Should contain line numbers (same format as final preview)
    assert!(
        body_text.contains("Title"),
        "streaming content should be rendered"
    );
}
