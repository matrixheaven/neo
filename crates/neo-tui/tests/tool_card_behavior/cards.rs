use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Component, Expandable, Finalization, Line};
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::tool_renderers::tool_header_spans;
use neo_tui::transcript::{ToolCallComponent, ToolCallState, TranscriptPane};
use serde_json::json;

fn plain(rows: Vec<Line>) -> Vec<String> {
    rows.into_iter()
        .map(|row| neo_tui::primitive::strip_ansi(&row.to_ansi()))
        .collect()
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
            .any(|line| line.contains("Preparing AskUserQuestion")),
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
fn completed_tool_expansion_reads_output_beyond_live_preview_limits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = neo_agent_core::session::ToolOutputStore::new(dir.path().to_owned());
    let output = (0..40)
        .map(|index| format!("line {index:02}\n"))
        .collect::<String>();
    store
        .append("main", "long-output", &output)
        .expect("append");
    let output_ref = store.finish("main", "long-output").expect("finish");
    assert!(output_ref.complete);

    let mut card = ToolCallComponent::new(ToolCallState {
        id: "long-output".to_owned(),
        name: "Bash".to_owned(),
        arguments: Some(r#"{"command":"produce long output"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });
    assert!(card.attach_output_ref(Some(output_ref.clone())));
    // Completion finalizes the six-line live preview but must not erase
    // access to the output reference.
    assert!(card.set_result(Some("bounded result".to_owned()), None, false, None,));

    let theme = TuiTheme::default();
    let mut cache = neo_tui::transcript::ExpandedOutputCache::default();
    let rows = card.render_complete_output_range(80, &theme, &store, &mut cache, 18);
    let rendered = plain(rows.clone());
    assert!(
        rendered.len() > 6,
        "expansion must read beyond the six-line live preview: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|row| row.contains("line 12")),
        "rows beyond the preview limit are served from the complete source: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|row| row.contains("22 lines remain")),
        "bounded visible range footer: {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|row| row.contains("output incomplete")),
        "finished artifact is complete: {rendered:?}"
    );

    // The derived wrap mapping is cached per width: a second render neither
    // re-reads the file nor changes the rows.
    let again = card.render_complete_output_range(80, &theme, &store, &mut cache, 18);
    assert_eq!(again, rows, "cached range is stable across renders");
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
        &state,
        &theme,
        None,
        usize::MAX,
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
fn expanded_output_cache_invalidates_when_artifact_completes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = neo_agent_core::session::ToolOutputStore::new(dir.path().to_owned());
    store
        .append("main", "flip-task", "alpha\nbeta\n")
        .expect("append");

    let mut card = ToolCallComponent::new(ToolCallState {
        id: "flip-task".to_owned(),
        name: "Bash".to_owned(),
        arguments: Some(r#"{"command":"long running"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });
    let live_ref = store.metadata("main", "flip-task").expect("live metadata");
    assert!(!live_ref.complete);
    card.attach_output_ref(Some(live_ref));

    let theme = TuiTheme::default();
    let mut cache = neo_tui::transcript::ExpandedOutputCache::default();
    let before = plain(card.render_complete_output_range(80, &theme, &store, &mut cache, 10));
    assert!(
        before.iter().any(|row| row.contains("output incomplete")),
        "{before:?}"
    );

    // The artifact completes without any line growth: the width-keyed cache
    // must not serve the stale incomplete footer.
    let finished_ref = store.finish("main", "flip-task").expect("finish");
    assert!(finished_ref.complete);
    let after = plain(card.render_complete_output_range(80, &theme, &store, &mut cache, 10));
    assert!(
        !after.iter().any(|row| row.contains("output incomplete")),
        "stale incomplete footer after completion: {after:?}"
    );
    assert_eq!(after, ["  alpha", "  beta"], "{after:?}");
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
fn generic_pending_tool_is_not_called_queued() {
    let mut component = ToolCallComponent::new(ToolCallState {
        id: "call-1".to_owned(),
        name: "Read".to_owned(),
        arguments: None,
        result: None,
        details: None,
        status: ToolStatusKind::Pending,
        exit_code: None,
    });
    assert!(
        plain(component.render(80))
            .join("\n")
            .contains("Preparing Read")
    );
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

            workflow_origin: None,
            output_ref: None,
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
fn key_argument_ignores_legacy_file_path_alias() {
    let argument =
        neo_tui::transcript::tool_renderers::key_argument(Some(r#"{"file_path":"src/legacy.rs"}"#));

    assert!(argument.is_empty());
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
        &state,
        &theme,
        None,
        usize::MAX,
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
        tool_header_spans(&running, &theme, None, usize::MAX)[0].to_ansi(),
        tool_header_spans(&used, &theme, None, usize::MAX)[0].to_ansi()
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
fn theme_draft_preview_card_never_overflows_narrow_widths() {
    let details = serde_json::json!({
        "kind": "theme_draft_preview",
        "draft_id": "draft-abc123",
        "fingerprint": "sha256:deadbeef",
        "display_name": "Aurora Night",
        "candidate_theme_id": "aurora-night.json",
        "base_theme_id": "default.json",
        "normalized_colors": {"brand": "#58a6ff", "text_primary": "#e6edf3"},
        "overridden_tokens": ["brand"],
        "contrast_warnings": ["text_muted vs selection_bg: contrast 2.4 is below 3.0"],
        "applied": false
    });
    for width in [10, 24, 40, 60, 80, 120] {
        let mut card = ToolCallComponent::new(ToolCallState {
            id: "theme-draft-2".to_owned(),
            name: "ThemeDraft".to_owned(),
            arguments: Some(serde_json::json!({"action": "preview"}).to_string()),
            result: Some("Preview ready.".to_owned()),
            details: Some(details.clone()),
            status: ToolStatusKind::Succeeded,
            exit_code: None,
        });
        let rows = card.render(width);
        for row in rows {
            assert!(
                neo_tui::primitive::visible_width(&row.to_ansi()) <= width,
                "width {width} overflow: {:?}",
                row.text()
            );
        }
    }
}

#[test]
fn theme_draft_preview_card_shows_name_status_samples_and_warnings_without_apply() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "theme-draft-1".to_owned(),
        name: "ThemeDraft".to_owned(),
        arguments: Some(serde_json::json!({"action": "preview"}).to_string()),
        result: Some("Preview ready.".to_owned()),
        details: Some(serde_json::json!({
            "kind": "theme_draft_preview",
            "draft_id": "draft-abc123",
            "fingerprint": "sha256:deadbeef",
            "display_name": "Aurora Night",
            "candidate_theme_id": "aurora-night.json",
            "base_theme_id": null,
            "normalized_colors": {
                "brand": "#58a6ff",
                "text_primary": "#e6edf3",
                "text_muted": "#8b949e",
                "status_ok": "#3fb950",
                "status_error": "#f85149",
                "selection_bg": "#1f232b",
                "shell_mode": "#56b4c2"
            },
            "overridden_tokens": ["brand", "text_primary"],
            "contrast_warnings": [
                "text_muted vs selection_bg: contrast 2.4 is below 3.0"
            ],
            "applied": false
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(100));
    let joined = rows.join("\n");
    assert!(
        joined.contains("Aurora Night"),
        "display name should appear: {joined:?}"
    );
    assert!(
        joined.contains("preview"),
        "status should appear: {joined:?}"
    );
    assert!(
        joined.contains("draft-abc123"),
        "opaque draft id should appear: {joined:?}"
    );
    assert!(
        joined.contains("aurora-night.json"),
        "candidate theme id should appear: {joined:?}"
    );
    assert!(
        joined.contains("sha256:deadbeef"),
        "fingerprint should appear: {joined:?}"
    );
    assert!(
        joined.contains("brand") && joined.contains("#58a6ff"),
        "color samples should appear: {joined:?}"
    );
    assert!(
        joined.contains("Welcome back"),
        "representative TUI sample should appear: {joined:?}"
    );
    assert!(
        joined.contains("text_muted vs selection_bg"),
        "contrast warnings should appear: {joined:?}"
    );
    assert!(
        !joined.to_lowercase().contains("apply"),
        "preview card must not offer an Apply action: {joined:?}"
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

        workflow_origin: None,
        output_ref: None,
    });
    runtime.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-0".to_owned(),
        name: "Bash".to_owned(),
        arguments: json!({"command": "y".repeat(200)}),

        workflow_origin: None,
        output_ref: None,
    });
    runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "bash-0".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok(""),

        workflow_origin: None,
        output_ref: None,
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
    assert_eq!(
        frame_with_gutter
            .iter()
            .filter(|line| line.contains("Used Bash"))
            .count(),
        1,
        "Bash tool card header rendered once"
    );
    assert!(
        frame_with_gutter
            .iter()
            .any(|line| line.contains("ctrl+o to expand")),
        "overflow hint present: {frame_with_gutter:?}"
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
fn wait_delegate_card_renders_running_and_final_outcomes() {
    const WIDTH: usize = 120;
    let arguments = json!({
        "ids": ["agent_a", "agent_b", "swarm_c", "agent_d"],
        "timeout_ms": 30_000
    })
    .to_string();
    let mut running = ToolCallComponent::new(ToolCallState {
        id: "wait-running".to_owned(),
        name: "WaitDelegate".to_owned(),
        arguments: Some(arguments.clone()),
        result: None,
        details: None,
        status: ToolStatusKind::Pending,
        exit_code: None,
    });
    assert!(running.update_call_state(
        "WaitDelegate".to_owned(),
        Some(arguments.clone()),
        ToolStatusKind::Running,
    ));
    assert!(running.has_visible_animation());

    let rows = plain(running.render(WIDTH));
    assert_eq!(rows.len(), 1, "collapsed running card: {rows:?}");
    assert!(
        rows[0].contains("Waiting for 4 delegates · timeout 30s · elapsed"),
        "running header: {rows:?}"
    );

    running.set_expanded(true);
    let rows = plain(running.render(WIDTH));
    for id in ["agent_a", "agent_b", "swarm_c", "agent_d"] {
        assert!(
            rows.iter()
                .any(|row| row.contains(&format!("{id} · waiting"))),
            "missing {id}: {rows:?}"
        );
    }

    let mut completed = ToolCallComponent::new(ToolCallState {
        id: "wait-completed".to_owned(),
        name: "WaitDelegate".to_owned(),
        arguments: Some(arguments.clone()),
        result: Some("kind: delegate_wait\noutcome: all_terminal".to_owned()),
        details: Some(json!({
            "kind": "delegate_wait",
            "outcome": "all_terminal",
            "aggregate": { "total": 4, "terminal": 4, "pending": 0, "not_found": 0 },
            "items": [
                { "id": "agent_a", "title": "Registry lifetime", "status": "completed" },
                { "id": "agent_b", "title": "Provider retry", "status": "failed" },
                { "id": "swarm_c", "description": "Shell audit", "status": "cancelled" },
                { "id": "agent_d", "title": "Smoke test", "status": "timed_out" }
            ]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });
    let rows = plain(completed.render(WIDTH));
    assert!(rows[0].contains("Wait complete · 4 terminal · 1 failed · 1 cancelled · 1 timed out"));
    assert!(
        rows.iter()
            .any(|row| row.contains("✓ Registry lifetime · completed"))
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("✗ Provider retry · failed"))
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("⊘ Shell audit · cancelled"))
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("1 more targets, ctrl+o to expand"))
    );
    assert!(!rows.iter().any(|row| row.contains("kind: delegate_wait")));

    completed.set_expanded(true);
    let rows = plain(completed.render(WIDTH));
    assert!(
        rows.iter()
            .any(|row| row.contains("✗ Smoke test · timed_out"))
    );

    let timeout = ToolCallState {
        id: "wait-timeout".to_owned(),
        name: "WaitDelegate".to_owned(),
        arguments: Some(arguments.clone()),
        result: Some("outcome: wait_timed_out".to_owned()),
        details: Some(json!({
            "kind": "delegate_wait",
            "outcome": "wait_timed_out",
            "aggregate": { "total": 4, "terminal": 2, "pending": 2, "not_found": 0 },
            "items": []
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    };
    let header = plain(vec![Line::from_spans(tool_header_spans(
        &timeout,
        &TuiTheme::default(),
        None,
        WIDTH,
    ))])
    .remove(0);
    assert_eq!(header, "◷ Wait timed out · 2/4 terminal · 2 still running");

    let not_found = ToolCallState {
        id: "wait-missing".to_owned(),
        details: Some(json!({
            "kind": "delegate_wait",
            "outcome": "not_found",
            "aggregate": { "total": 2, "terminal": 1, "pending": 0, "not_found": 1 },
            "items": []
        })),
        ..timeout
    };
    let header = plain(vec![Line::from_spans(tool_header_spans(
        &not_found,
        &TuiTheme::default(),
        None,
        40,
    ))])
    .remove(0);
    assert_eq!(header, "? Target not found · 1 unknown");
    assert!(neo_tui::primitive::visible_width(&header) <= 40);

    let rows = plain(completed.render(32));
    assert!(
        rows.iter()
            .all(|row| neo_tui::primitive::visible_width(row) <= 30),
        "narrow card overflowed: {rows:?}"
    );
}
