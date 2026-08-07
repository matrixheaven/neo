use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Component, Expandable, Line};
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
fn aggregated_write_card_renders_created_content_and_overwrite_diff() {
    let diff = "--- a/old.txt\n+++ b/old.txt\n@@ -1,2 +1,2 @@\n-old line\n+new line\n context\n";
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "batch-write-1".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: Some("wrote 2 files".to_owned()),
        details: Some(json!({
            "kind": "write",
            "status": "committed",
            "files": 2,
            "created": 1,
            "overwritten": 1,
            "added": 3,
            "removed": 1,
            "changes": [
                {
                    "path": "src/new_file.rs",
                    "operation": "created",
                    "status": "committed",
                    "line_count": 1,
                    "added": 1,
                    "removed": 0,
                    "content": "fn main() {}"
                },
                {
                    "path": "old.txt",
                    "operation": "overwritten",
                    "status": "committed",
                    "line_count": 2,
                    "added": 1,
                    "removed": 1,
                    "diff": diff
                }
            ]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    let joined = rows.join("\n");

    // Created file shows line-numbered content.
    assert!(
        rows.iter().any(|line| line.contains("fn main()")),
        "created content should appear: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|line| line.contains('1') && line.contains("fn main()")),
        "created content should have line number: {rows:?}"
    );

    // Overwritten file shows diff lines.
    assert!(
        rows.iter()
            .any(|line| line.contains("- old line") || line.contains("-old line")),
        "diff removed line should appear: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|line| line.contains("+ new line") || line.contains("+new line")),
        "diff added line should appear: {rows:?}"
    );

    // Both paths appear in frame borders.
    assert!(
        joined.contains("src/new_file.rs"),
        "created path in frame: {joined}"
    );
    assert!(
        joined.contains("old.txt"),
        "overwritten path in frame: {joined}"
    );
}

#[test]
fn batch_write_collapse_keeps_first_two_and_last_file() {
    let changes: Vec<serde_json::Value> = (0..5)
        .map(|i| {
            json!({
                "path": format!("src/file{i}.rs"),
                "operation": "created",
                "status": "committed",
                "line_count": 2,
                "added": 2,
                "removed": 0,
                "content": format!("fn file{i}() {{}}\n// end {i}")
            })
        })
        .collect();
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "batch-write-collapse".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: Some("wrote 5 files".to_owned()),
        details: Some(json!({
            "kind": "write",
            "status": "committed",
            "files": 5,
            "created": 5,
            "overwritten": 0,
            "added": 10,
            "removed": 0,
            "changes": changes
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    // Collapsed: files 0, 1, and 4 visible; files 2 and 3 omitted.
    let collapsed = plain(card.render(100));
    let collapsed_text = collapsed.join("\n");
    assert!(
        collapsed_text.contains("file0.rs"),
        "file0 visible collapsed: {collapsed:?}"
    );
    assert!(
        collapsed_text.contains("file1.rs"),
        "file1 visible collapsed: {collapsed:?}"
    );
    assert!(
        collapsed_text.contains("file4.rs"),
        "file4 visible collapsed: {collapsed:?}"
    );
    assert!(
        !collapsed_text.contains("file2.rs"),
        "file2 omitted collapsed: {collapsed:?}"
    );
    assert!(
        !collapsed_text.contains("file3.rs"),
        "file3 omitted collapsed: {collapsed:?}"
    );
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("hidden") && line.contains("ctrl+o")),
        "omission summary should appear: {collapsed:?}"
    );

    // Expanded: all 5 paths appear.
    card.set_expanded(true);
    let expanded = plain(card.render(100));
    let expanded_text = expanded.join("\n");
    for i in 0..5 {
        assert!(
            expanded_text.contains(&format!("file{i}.rs")),
            "file{i} visible expanded: {expanded:?}"
        );
    }
}

#[test]
fn batch_write_created_head_tail_and_failed_diagnostics_are_visible() {
    let content = (1..=15)
        .map(|line| format!("line_{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut created = ToolCallComponent::new(ToolCallState {
        id: "write-head-tail".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: Some("written".to_owned()),
        details: Some(json!({
            "kind": "write",
            "status": "committed",
            "files": 1,
            "created": 1,
            "overwritten": 0,
            "added": 15,
            "removed": 0,
            "changes": [{
                "path": "src/generated.rs",
                "operation": "created",
                "status": "committed",
                "line_count": 15,
                "added": 15,
                "removed": 0,
                "content": content
            }]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });
    let collapsed = plain(created.render(100)).join("\n");
    assert!(collapsed.contains("line_1") && collapsed.contains("line_15"));
    assert!(
        !collapsed.contains("line_8"),
        "middle must be collapsed: {collapsed}"
    );
    assert!(collapsed.contains("5 lines hidden"), "{collapsed}");
    created.set_expanded(true);
    let expanded = plain(created.render(100)).join("\n");
    assert!(expanded.contains("line_8"), "expanded body: {expanded}");

    let mut failed = ToolCallComponent::new(ToolCallState {
        id: "write-failed-diagnostic".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: Some("failed".to_owned()),
        details: Some(json!({
            "kind": "write",
            "status": "partial_commit",
            "files": 2,
            "created": 2,
            "overwritten": 0,
            "added": 1,
            "removed": 0,
            "created_directories": ["src/generated"],
            "changes": [
                {"path": "src/a.rs", "operation": "created", "status": "committed", "line_count": 1, "added": 1, "removed": 0, "content": "a"},
                {"path": "src/b.rs", "operation": "created", "status": "failed", "line_count": 1, "added": 0, "removed": 0, "message": "file install failed: permission denied"}
            ]
        })),
        status: ToolStatusKind::Failed,
        exit_code: None,
    });
    let failed_rows = plain(failed.render(100));
    let failed_text = failed_rows.join("\n");
    assert!(failed_rows[0].contains("partial commit · 1/2 committed"));
    assert!(failed_text.contains("file install failed: permission denied"));
    assert!(failed_text.contains("created directories:") && failed_text.contains("src/generated"));
}

#[test]
fn batch_write_frames_preserve_highlight_line_numbers_clusters_and_narrow_width() {
    let diff = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n first\n-old_alpha\n+new_alpha\n tail\n@@ -20,3 +20,3 @@\n ctx\n-old_beta\n+new_beta\n end\n";
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "batch-write-narrow".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: Some("written".to_owned()),
        details: Some(json!({
            "kind": "write",
            "status": "committed",
            "files": 1,
            "created": 0,
            "overwritten": 1,
            "added": 2,
            "removed": 2,
            "changes": [{
                "path": "src/lib.rs",
                "operation": "overwritten",
                "status": "committed",
                "line_count": 22,
                "added": 2,
                "removed": 2,
                "diff": diff
            }]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(40));

    // Line numbers present in diff output (e.g. "2 - old_alpha").
    assert!(
        rows.iter()
            .any(|line| { line.contains("old_alpha") || line.contains("new_alpha") })
            && rows.iter().any(|line| {
                (line.contains("old_alpha") || line.contains("new_alpha"))
                    && line.chars().any(|c| c.is_ascii_digit())
            }),
        "line numbers should appear with diff: {rows:?}"
    );

    // No border overflow: all lines <= 40 visible width.
    for line in &rows {
        assert!(
            neo_tui::primitive::visible_width(line) <= 40,
            "row exceeds width 40: {line:?}"
        );
    }

    // Diff clusters rendered (both hunks visible or cluster markers present).
    let joined = rows.join("\n");
    assert!(
        joined.contains("alpha") || joined.contains("beta") || joined.contains("changes hidden"),
        "diff clusters should be rendered: {rows:?}"
    );
}

#[test]
fn batch_write_live_headers_own_aggregate_summary_once() {
    let mut prepared = ToolCallComponent::new(ToolCallState {
        id: "write-prepared-summary".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: None,
        details: Some(json!({
            "kind": "write_prepared",
            "files": 2,
            "created": 2,
            "overwritten": 0,
            "added": 2,
            "removed": 0,
            "changes": []
        })),
        status: ToolStatusKind::Running,
        exit_code: None,
    });
    let prepared_text = plain(prepared.render(100)).join("\n");
    assert_eq!(prepared_text.matches("2 files · 2 created").count(), 1);
    assert!(!prepared_text.contains("verified · 2 files"));

    let mut progress = ToolCallComponent::new(ToolCallState {
        id: "write-progress-summary".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: None,
        details: Some(json!({
            "kind": "write_progress",
            "committed": 1,
            "total": 2,
            "latest_path": "src/a.rs",
            "added": 1,
            "removed": 0
        })),
        status: ToolStatusKind::Running,
        exit_code: None,
    });
    let progress_text = plain(progress.render(100)).join("\n");
    assert_eq!(progress_text.matches("committing 1/2 files").count(), 1);
    assert_eq!(progress_text.matches("+1 -0").count(), 1);
}

#[test]
fn batch_write_partial_header_uses_committed_totals_only() {
    let theme = TuiTheme::default();
    let state = ToolCallState {
        id: "batch-write-partial".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: Some("partial".to_owned()),
        details: Some(json!({
            "kind": "write",
            "status": "partial_commit",
            "files": 3,
            "created": 1,
            "overwritten": 1,
            "added": 5,
            "removed": 2,
            "changes": [
                {"path": "a.rs", "operation": "created", "status": "committed", "line_count": 3, "added": 3, "removed": 0, "content": "aaa"},
                {"path": "b.rs", "operation": "overwritten", "status": "committed", "line_count": 2, "added": 2, "removed": 2, "diff": "--- b.rs\n+++ b.rs\n@@ -1 +1 @@\n-x\n+y\n"},
                {"path": "c.rs", "operation": "created", "status": "not_attempted", "line_count": 10, "added": 10, "removed": 0, "content": "ccc"}
            ]
        })),
        status: ToolStatusKind::Failed,
        exit_code: None,
    };

    let header = plain(vec![Line::from_spans(tool_header_spans(
        &state, &theme, None, 80,
    ))])
    .remove(0);

    // The header chip shows the applied-only totals (+5 -2), not planned totals.
    assert!(
        header.contains("+5"),
        "header should show applied added: {header:?}"
    );
    assert!(
        header.contains("-2"),
        "header should show applied removed: {header:?}"
    );
    assert!(
        !header.contains("+15"),
        "header must not show planned totals: {header:?}"
    );
}

#[test]
fn narrow_write_frame_expands_tabs_without_extra_border_overflow() {
    for width in 1..=6 {
        let content = if width == 1 { "a\tb" } else { "中\tb" };
        let mut card = ToolCallComponent::new(ToolCallState {
            id: format!("write-{width}"),
            name: "Write".to_owned(),
            arguments: Some(json!({"path": "x.rs", "content": content}).to_string()),
            result: Some("written".to_owned()),
            details: Some(json!({
                "kind": "write",
                "status": "committed",
                "files": 1,
                "created": 1,
                "overwritten": 0,
                "added": 1,
                "removed": 0,
                "changes": [{
                    "path": "x.rs",
                    "operation": "created",
                    "status": "committed",
                    "line_count": 1,
                    "added": 1,
                    "removed": 0,
                    "content": content
                }]
            })),
            status: ToolStatusKind::Succeeded,
            exit_code: None,
        });
        let rows = plain(card.render(width));
        let bound = width.max(if content.contains('中') { 2 } else { 1 });
        assert!(
            rows.iter()
                .all(|row| neo_tui::primitive::visible_width(row) <= bound),
            "width={width}: {rows:?}"
        );
        assert!(
            !rows
                .iter()
                .any(|row| row.contains('╭') || row.contains('│'))
        );
        assert!(
            !rows.iter().any(|row| row.contains('\t')),
            "width={width}: {rows:?}"
        );
    }
}

#[test]
fn streaming_batch_write_uses_unverified_content_preview_without_raw_json() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "stream-batch".to_owned(),
        name: "Write".to_owned(),
        arguments: Some(r#"{"path":"new.rs","content":"fn main() {}"}"#.to_owned()),
        result: None,
        details: None,
        status: ToolStatusKind::Running,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    let joined = rows.join("\n");

    // Shows unverified intent indicator or file path.
    assert!(
        joined.contains("unverified intent") || joined.contains("new.rs"),
        "streaming should show unverified intent or path: {rows:?}"
    );

    // Does NOT show raw JSON braces.
    assert!(
        !joined.contains("{\"files\""),
        "streaming must not show raw JSON: {rows:?}"
    );
    assert!(
        !joined.contains("\"content\""),
        "streaming must not show raw JSON keys: {rows:?}"
    );
}

#[test]
fn streaming_write_tool_card_does_not_panic_on_trailing_blank_lines() {
    let theme = TuiTheme::default();
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: None,
        details: None,
        status: ToolStatusKind::Pending,
        exit_code: None,
    });

    card.update_call(Some(
        r#"{"path":"design.md","content":"---\nrole: technical-design\n---\n\n# Design\n\n"}"#
            .to_owned(),
    ));

    let rows = card.render_with_theme(100, &theme);
    assert!(
        rows.iter().any(|line| line.text().contains("design.md")),
        "preview should show file path without panicking: {rows:?}"
    );
}

#[test]
fn streaming_write_tool_card_highlights_content_before_path_arrives() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: None,
        details: None,
        status: ToolStatusKind::Pending,
        exit_code: None,
    });

    card.update_call(Some(
        r#"{"path":"service.go","content":"package service\n\nfunc main() {\n\tfmt.Println(\"ok\")\n}\n"}"#.to_owned(),
    ));

    let rows = plain(card.render(100));
    assert!(
        rows.iter().any(|line| line.contains("service.go")),
        "streaming intent should show file path: {rows:?}"
    );
    assert!(
        rows.iter().any(|line| line.contains("unverified intent")),
        "streaming intent should be marked unverified: {rows:?}"
    );
}

#[test]
fn streaming_write_tool_card_renders_line_numbered_preview_from_partial_json() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: None,
        details: None,
        status: ToolStatusKind::Pending,
        exit_code: None,
    });

    card.update_call(Some(
        r#"{"path":"/workspace/sample_service.go","content":"// sample_service.go\n\npackage service\n\nimport (\n\t\"context\"\n\t\"fmt\"\n)\n"#.to_owned(),
    ));

    let rows = plain(card.render(100));
    assert!(
        rows.iter()
            .any(|line| line.contains("receiving structured changes")),
        "partial JSON should show receiving indicator: {rows:?}"
    );

    card.update_call(Some(
        r#"{"path":"/workspace/sample_service.go","content":"// sample_service.go\n\npackage service\n\nimport (\n\t\"context\"\n\t\"fmt\"\n)\n"}"#.to_owned(),
    ));

    let rows = plain(card.render(100));
    assert!(
        rows.iter().any(|line| line.contains("sample_service.go")),
        "complete JSON should show file path: {rows:?}"
    );
    assert!(
        rows.iter().any(|line| line.contains("unverified intent")),
        "streaming intent should be marked unverified: {rows:?}"
    );
}

#[test]
fn write_and_edit_success_headers_omit_paths_and_color_stats() {
    let theme = TuiTheme::default();

    // Write card header.
    let write_state = ToolCallState {
        id: "write-header".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: Some("wrote".to_owned()),
        details: Some(json!({
            "kind": "write",
            "status": "committed",
            "files": 2,
            "created": 1,
            "overwritten": 1,
            "added": 10,
            "removed": 3,
            "changes": [
                {"path": "src/alpha.rs", "operation": "created", "status": "committed", "line_count": 5, "added": 5, "removed": 0, "content": "alpha"},
                {"path": "src/beta.rs", "operation": "overwritten", "status": "committed", "line_count": 5, "added": 5, "removed": 3, "diff": "--- src/beta.rs\n+++ src/beta.rs\n@@ -1 +1 @@\n-a\n+b\n"}
            ]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    };
    let write_header = plain(vec![Line::from_spans(tool_header_spans(
        &write_state,
        &theme,
        None,
        80,
    ))])
    .remove(0);

    assert!(
        write_header.contains("Write"),
        "header names tool: {write_header:?}"
    );
    assert!(
        write_header.contains("2 files"),
        "header shows file count: {write_header:?}"
    );
    assert!(
        write_header.contains("1 created"),
        "header shows created: {write_header:?}"
    );
    assert!(
        write_header.contains("1 overwritten"),
        "header shows overwritten: {write_header:?}"
    );
    assert!(
        write_header.contains("+10"),
        "header shows added: {write_header:?}"
    );
    assert!(
        write_header.contains("-3"),
        "header shows removed: {write_header:?}"
    );
    assert!(
        !write_header.contains("alpha.rs"),
        "header must not contain path: {write_header:?}"
    );
    assert!(
        !write_header.contains("beta.rs"),
        "header must not contain path: {write_header:?}"
    );

    // Edit card header.
    let edit_state = ToolCallState {
        id: "edit-header".to_owned(),
        name: "Edit".to_owned(),
        arguments: None,
        result: Some("edited".to_owned()),
        details: Some(json!({
            "kind": "edit",
            "status": "committed",
            "files": 2,
            "replacements": 3,
            "added": 7,
            "removed": 4,
            "changes": [
                {"path": "src/gamma.rs", "status": "committed", "replacements": 2, "added": 4, "removed": 2, "diff": "--- src/gamma.rs\n+++ src/gamma.rs\n@@ -1 +1 @@\n-x\n+y\n"},
                {"path": "src/delta.rs", "status": "committed", "replacements": 1, "added": 3, "removed": 2, "diff": "--- src/delta.rs\n+++ src/delta.rs\n@@ -1 +1 @@\n-a\n+b\n"}
            ]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    };
    let edit_header = plain(vec![Line::from_spans(tool_header_spans(
        &edit_state,
        &theme,
        None,
        80,
    ))])
    .remove(0);

    assert!(
        edit_header.contains("Edit"),
        "header names tool: {edit_header:?}"
    );
    assert!(
        !edit_header.contains("gamma.rs"),
        "edit header must not contain path: {edit_header:?}"
    );
    assert!(
        !edit_header.contains("delta.rs"),
        "edit header must not contain path: {edit_header:?}"
    );
}

#[test]
fn write_frame_shrinks_to_content_width() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "write-compact-frame".to_owned(),
        name: "Write".to_owned(),
        arguments: Some(json!({"path": "x.rs", "content": "ok"}).to_string()),
        result: Some("written".to_owned()),
        details: Some(json!({
            "kind": "write",
            "status": "committed",
            "files": 1,
            "created": 1,
            "overwritten": 0,
            "added": 1,
            "removed": 0,
            "changes": [{
                "path": "x.rs",
                "operation": "created",
                "status": "committed",
                "line_count": 1,
                "added": 1,
                "removed": 0,
                "content": "ok"
            }]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    let top = rows
        .iter()
        .position(|row| row.starts_with('╭'))
        .expect("frame top");
    let bottom = rows
        .iter()
        .position(|row| row.starts_with('╰'))
        .expect("frame bottom");
    let frame = &rows[top..=bottom];
    let frame_width = neo_tui::primitive::visible_width(&frame[0]);

    assert!(frame_width < 80, "{frame:?}");
    assert!(
        frame
            .iter()
            .all(|row| neo_tui::primitive::visible_width(row) == frame_width),
        "{frame:?}"
    );
}

#[test]
fn write_prepare_failure_bounds_diagnostics_and_omits_recovery_prompt() {
    let message = format!(
        "invalid Write arguments: {}TAIL_SENTINEL",
        "payload ".repeat(1_000)
    );
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "write-invalid-arguments".to_owned(),
        name: "Write".to_owned(),
        arguments: None,
        result: Some(message.clone()),
        details: Some(json!({
            "kind": "write",
            "status": "prepare_failed",
            "message": message
        })),
        status: ToolStatusKind::Failed,
        exit_code: None,
    });

    let rendered = plain(card.render(80)).join("\n");

    assert!(rendered.contains("invalid Write arguments"), "{rendered}");
    assert!(rendered.contains("error details omitted"), "{rendered}");
    assert!(!rendered.contains("TAIL_SENTINEL"), "{rendered}");
    assert!(!rendered.contains("Re-read affected files"), "{rendered}");
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
        json_fragment:
            r#"{"path":"src/foo.rs","content":"use std::collections::HashMap;\n\npub f"}"#
                .to_owned(),
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
        frame.iter().any(|line| line.contains("src/foo.rs")),
        "streaming content should show file path: {frame:?}"
    );
    assert!(
        frame.iter().any(|line| line.contains("unverified intent")),
        "streaming intent should be marked unverified: {frame:?}"
    );
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
    assert!(
        !body_text.contains("Preparing changes"),
        "streaming preview should not show progress line"
    );
    assert!(
        body_text.contains("test.md"),
        "streaming content should show file path"
    );
}

#[test]
fn write_tool_card_renders_finalized_diff_from_details() {
    let content = (1..=20)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Write".to_owned(),
        arguments: Some(
            serde_json::json!({"path": "src/generated.rs", "content": content}).to_string(),
        ),
        result: Some("wrote 1 files".to_owned()),
        details: Some(serde_json::json!({
            "kind": "write",
            "status": "committed",
            "files": 1,
            "created": 1,
            "overwritten": 0,
            "added": 20,
            "removed": 0,
            "changes": [{
                "path": "src/generated.rs",
                "operation": "created",
                "status": "committed",
                "line_count": 20,
                "added": 20,
                "removed": 0,
                "content": content,
            }]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    assert!(
        rows.iter().any(|line| line.contains("src/generated.rs")),
        "path should appear in frame header: {rows:?}"
    );
    assert!(rows.iter().any(|line| line.contains("ctrl+o to expand")));
    assert!(rows.iter().any(|line| line.contains("line 1")));
    assert!(!rows.iter().any(|line| line.contains("line 11")));

    card.set_expanded(true);
    let expanded = plain(card.render(80));
    assert!(expanded.iter().any(|line| line.contains("line 20")));
}
