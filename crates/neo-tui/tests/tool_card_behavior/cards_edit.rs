use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Component, Expandable, Line};
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::diff_preview::render_diff_lines_clustered;
use neo_tui::transcript::{ToolCallComponent, ToolCallState, TranscriptPane};
use serde_json::json;

fn plain(rows: Vec<Line>) -> Vec<String> {
    rows.into_iter()
        .map(|row| neo_tui::primitive::strip_ansi(&row.to_ansi()))
        .collect()
}

#[test]
fn collapsed_edit_keeps_first_and_last_change_clusters_inside_frame() {
    let diff = "--- sample.rs\n+++ sample.rs\n@@ -1,12 +1,12 @@\n first\n-\told_first\n+\tnew_first\n c3\n c4\n c5\n c6\n c7\n c8\n c9\n c10\n-old_last\n+new_last\n tail\n";
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "edit-clusters".to_owned(),
        name: "Edit".to_owned(),
        arguments: None,
        result: Some("edited".to_owned()),
        details: Some(json!({
            "kind": "edit",
            "status": "committed",
            "files": 1,
            "replacements": 2,
            "added": 2,
            "removed": 2,
            "changes": [{
                "path": "sample.rs",
                "status": "committed",
                "replacements": 2,
                "added": 2,
                "removed": 2,
                "diff": diff
            }]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let collapsed = plain(card.render(64));
    assert!(
        collapsed.iter().any(|row| row.contains("old_first")),
        "{collapsed:?}"
    );
    assert!(
        collapsed.iter().any(|row| row.contains("old_last")),
        "{collapsed:?}"
    );
    let hidden = collapsed
        .iter()
        .position(|row| row.contains("diff lines hidden"))
        .expect("omission row");
    assert!(collapsed[hidden].starts_with("│ "), "{collapsed:?}");
    assert!(collapsed.iter().all(|row| !row.contains('\t')));
    assert!(
        collapsed
            .iter()
            .all(|row| neo_tui::primitive::visible_width(row) <= 64)
    );
    let collapsed_bottom = collapsed
        .iter()
        .rposition(|row| row.starts_with('╰'))
        .unwrap();

    card.set_expanded(true);
    let expanded = plain(card.render(64));
    assert!(!expanded.iter().any(|row| row.contains("diff lines hidden")));
    let expanded_bottom = expanded
        .iter()
        .rposition(|row| row.starts_with('╰'))
        .unwrap();
    assert!(expanded_bottom > collapsed_bottom);
}

#[test]
fn edit_and_write_file_frames_embed_semantic_headers_in_top_border() {
    // Write card: path embedded in top border.
    let mut write_card = ToolCallComponent::new(ToolCallState {
        id: "write-border".to_owned(),
        name: "Write".to_owned(),
        arguments: Some(json!({"path": "src/embedded.rs", "content": "hello"}).to_string()),
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
                "path": "src/embedded.rs",
                "operation": "created",
                "status": "committed",
                "line_count": 1,
                "added": 1,
                "removed": 0,
                "content": "hello"
            }]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });
    let write_rows = plain(write_card.render(80));
    let write_top = write_rows
        .iter()
        .find(|line| line.starts_with('╭'))
        .expect("write frame top border");
    assert!(
        write_top.contains("src/embedded.rs"),
        "path should be in top border: {write_top:?}"
    );
    assert!(
        write_top.contains("created")
            && write_top.contains("1 lines")
            && write_top.contains("+1 -0"),
        "Write semantic metadata should be in top border: {write_top:?}"
    );
    // The line after the top border should be body content (│), not a duplicate header.
    let top_idx = write_rows
        .iter()
        .position(|line| line.starts_with('╭'))
        .unwrap();
    if top_idx + 1 < write_rows.len() {
        let after_top = &write_rows[top_idx + 1];
        assert!(
            after_top.starts_with('│'),
            "line after border should be frame body: {after_top:?}"
        );
    }

    // Edit card: path embedded in top border.
    let mut edit_card = ToolCallComponent::new(ToolCallState {
        id: "edit-border".to_owned(),
        name: "Edit".to_owned(),
        arguments: None,
        result: Some("edited".to_owned()),
        details: Some(json!({
            "kind": "edit",
            "status": "committed",
            "files": 1,
            "replacements": 1,
            "added": 1,
            "removed": 1,
            "changes": [{
                "path": "src/edit_embedded.rs",
                "status": "committed",
                "replacements": 1,
                "added": 1,
                "removed": 1,
                "diff": "--- src/edit_embedded.rs\n+++ src/edit_embedded.rs\n@@ -1 +1 @@\n-old\n+new\n"
            }]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });
    let edit_rows = plain(edit_card.render(80));
    let edit_top = edit_rows
        .iter()
        .find(|line| line.starts_with('╭'))
        .expect("edit frame top border");
    assert!(
        edit_top.contains("src/edit_embedded.rs"),
        "path should be in edit top border: {edit_top:?}"
    );
    assert!(
        edit_top.contains("committed")
            && edit_top.contains("1 replacements")
            && edit_top.contains("+1 -1"),
        "Edit semantic metadata should be in top border: {edit_top:?}"
    );
    let edit_top_idx = edit_rows
        .iter()
        .position(|line| line.starts_with('╭'))
        .unwrap();
    if edit_top_idx + 1 < edit_rows.len() {
        let after_top = &edit_rows[edit_top_idx + 1];
        assert!(
            after_top.starts_with('│'),
            "line after edit border should be frame body: {after_top:?}"
        );
    }
}

#[test]
fn edit_and_write_frames_preserve_color_line_numbers_and_wrapped_tails() {
    let theme = TuiTheme::default();
    let long_path = "src/a_very_long_directory_name/tail.rs";
    let mut edit = ToolCallComponent::new(ToolCallState {
        id: "edit-frame".to_owned(),
        name: "Edit".to_owned(),
        arguments: None,
        result: Some("edited".to_owned()),
        details: Some(json!({
            "kind": "edit",
            "status": "committed",
            "files": 1,
            "replacements": 1,
            "added": 1,
            "removed": 1,
            "changes": [{
                "path": long_path,
                "status": "committed",
                "replacements": 1,
                "added": 1,
                "removed": 1,
                "diff": format!("--- {long_path}\n+++ {long_path}\n@@ -41 +41 @@\n-fn old() {{}}\n+fn ENDING_SENTINEL() {{}}\n")
            }]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });
    let edit_rows = edit.render_with_theme(60, &theme);
    let edit_text = edit_rows
        .iter()
        .map(Line::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        edit_text.contains('╭') && edit_text.contains('╰'),
        "{edit_text}"
    );
    assert!(
        edit_text.contains("tail.rs") && !edit_text.contains("src/a_very_long_directory_name"),
        "narrow frame should preserve the path tail: {edit_text}"
    );
    assert!(
        edit_text.contains("committed")
            && edit_text.contains("1 replacements")
            && edit_text.contains("+1 -1"),
        "narrow frame must preserve semantic suffix: {edit_text}"
    );
    assert!(edit_text.contains("41 - fn old()"), "{edit_text}");
    assert!(
        edit_text.contains("ENDING_SENTINEL"),
        "wrapped code tail lost: {edit_text}"
    );
    assert!(
        edit_rows
            .iter()
            .flat_map(neo_tui::primitive::Line::spans)
            .any(|span| { span.text() == "✓ " && span.style().fg == Some(theme.status_ok) })
    );
    let removed = edit_rows
        .iter()
        .find(|line| line.text().contains("41 - fn old()"))
        .expect("removed row");
    assert!(
        removed
            .spans()
            .iter()
            .any(|span| { span.text() == "41 " && span.style().fg == Some(theme.diff_removed) })
    );
    assert!(
        removed
            .spans()
            .iter()
            .any(|span| { span.text() == "- " && span.style().fg == Some(theme.diff_removed) })
    );
    assert!(
        removed.spans().iter().any(|span| {
            span.text().contains("fn") && span.style().fg != Some(theme.diff_removed)
        })
    );
    let added = edit_rows
        .iter()
        .find(|line| line.text().contains("41 + fn"))
        .expect("added row");
    assert!(
        added
            .spans()
            .iter()
            .any(|span| { span.text() == "41 " && span.style().fg == Some(theme.diff_added) })
    );
    assert!(
        added
            .spans()
            .iter()
            .any(|span| { span.text() == "+ " && span.style().fg == Some(theme.diff_added) })
    );

    let mut write = ToolCallComponent::new(ToolCallState {
        id: "write-frame".to_owned(),
        name: "Write".to_owned(),
        arguments: Some(
            json!({"path": long_path, "content": "fn main() { let value = ENDING_SENTINEL; }"})
                .to_string(),
        ),
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
                "path": long_path,
                "operation": "created",
                "status": "committed",
                "line_count": 1,
                "added": 1,
                "removed": 0,
                "content": "fn main() { let value = ENDING_SENTINEL; }"
            }]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });
    let write_text = plain(write.render(60)).join("\n");
    assert!(
        write_text.contains('╭') && write_text.contains('╰'),
        "{write_text}"
    );
    assert!(
        write_text.contains("tail.rs") && !write_text.contains("src/a_very_long_directory_name"),
        "narrow frame should preserve the path tail: {write_text}"
    );
    assert!(
        write_text.contains("created")
            && write_text.contains("1 lines")
            && write_text.contains("+1 -0")
            && write_text.contains("committed"),
        "narrow frame must preserve semantic suffix: {write_text}"
    );
    assert!(
        write_text.contains("ENDING_SENTINEL"),
        "wrapped code tail lost: {write_text}"
    );
    let too_narrow = plain(write.render(48)).join("\n");
    assert!(
        !too_narrow.contains('╭') && too_narrow.contains("tail.rs"),
        "a title that cannot retain a path tail must use the unframed fallback: {too_narrow}"
    );
}

#[test]
fn edit_batch_card_distinguishes_prepare_stale_partial_and_durability() {
    for (status, needle) in [
        ("prepare_failed", "zero writes"),
        ("stale", "zero writes"),
        ("partial_commit", "partial"),
        ("durability_uncertain", "durability"),
    ] {
        let mut card = ToolCallComponent::new(ToolCallState {
            id: format!("edit-{status}"),
            name: "Edit".to_owned(),
            arguments: None,
            result: Some("failed".to_owned()),
            details: Some(serde_json::json!({
                "kind": "edit",
                "status": status,
                "message": "diagnostic",
                "path": "src/a.rs",
                "changes": []
            })),
            status: ToolStatusKind::Failed,
            exit_code: None,
        });
        let rows = plain(card.render(80));
        assert!(
            rows.iter().any(|line| line.contains(needle)),
            "{status} missing {needle}: {rows:?}"
        );
    }

    let mut no_path = ToolCallComponent::new(ToolCallState {
        id: "edit-no-path".to_owned(),
        name: "Edit".to_owned(),
        arguments: None,
        result: Some("failed".to_owned()),
        details: Some(json!({
            "kind": "edit",
            "status": "prepare_failed",
            "message": "diagnostic without path"
        })),
        status: ToolStatusKind::Failed,
        exit_code: None,
    });
    let rows = plain(no_path.render(80));
    let diagnostic = rows
        .iter()
        .find(|line| line.contains("diagnostic without path"))
        .expect("diagnostic row");
    assert!(diagnostic.starts_with("│ "), "{rows:?}");
}

#[test]
fn edit_batch_card_renders_collapsed_expanded_and_narrow() {
    let details = serde_json::json!({
        "kind": "edit",
        "status": "committed",
        "files": 5,
        "replacements": 9,
        "added": 28,
        "removed": 17,
        "changes": (0..5).map(|i| serde_json::json!({
            "path": if i == 2 {
                format!("src/very/long/nested/path/file{i}.rs")
            } else {
                format!("src/file{i}.rs")
            },
            "status": "committed",
            "replacements": 1,
            "added": 2,
            "removed": 1,
            "diff": format!("--- src/file{i}.rs\n+++ src/file{i}.rs\n@@ -1 +1 @@\n-old{i}\n+new{i}\n")
        })).collect::<Vec<_>>()
    });
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "edit-batch".to_owned(),
        name: "Edit".to_owned(),
        arguments: Some(r#"{"path":"src/foo.rs","old":"foo","new":"bar"}"#.to_owned()),
        result: Some("edited".to_owned()),
        details: Some(details),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let collapsed = plain(card.render(100));
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("file2.rs") && line.contains("+2") && line.contains("-1")),
        "collapsed should show omitted file stats: {collapsed:?}"
    );
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("diff details hidden") && line.contains("ctrl+o")),
        "collapsed should retain expand hint: {collapsed:?}"
    );
    let narrow_collapsed = plain(card.render(30));
    assert!(
        narrow_collapsed
            .iter()
            .any(|line| line.contains("+2") && line.contains("-1")),
        "narrow collapsed rows should retain file stats: {narrow_collapsed:?}"
    );

    card.set_expanded(true);
    let expanded = plain(card.render(100));
    for i in 0..5 {
        assert!(
            expanded
                .iter()
                .any(|line| line.contains(&format!("file{i}.rs"))),
            "expanded missing file{i}: {expanded:?}"
        );
    }

    let narrow = plain(card.render(40));
    for line in &narrow {
        assert!(
            neo_tui::primitive::visible_width(line) <= 40,
            "row exceeds width: {line:?}"
        );
    }
}

#[test]
fn edit_batch_progress_details_survive_interruption() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "edit-int".to_owned(),
        name: "Edit".to_owned(),
        arguments: None,
        result: None,
        details: Some(serde_json::json!({
            "kind": "edit_progress",
            "committed": 2,
            "total": 5,
            "latest_path": "src/lib.rs",
            "added": 9,
            "removed": 4
        })),
        status: ToolStatusKind::Running,
        exit_code: None,
    });
    assert!(card.set_terminal_status(ToolStatusKind::Failed, Some("interrupted".to_owned())));
    assert!(card.state().details.is_some());
    let rows = plain(card.render(80));
    assert!(
        rows.iter()
            .any(|line| line.contains("unknown") || line.contains("interrupted")),
        "interruption should retain progress evidence: {rows:?}"
    );
    assert!(
        !rows.iter().any(|line| line.contains("committing")),
        "terminal state must outrank retained progress: {rows:?}"
    );
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
fn edit_streaming_preview_shows_flat_intent() {
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
        frame.iter().any(|line| line.contains("src/foo.rs"))
            && frame.iter().any(|line| line.contains("unverified intent")),
        "Edit streaming should show path and unverified intent: {frame:?}"
    );
}

#[test]
fn edit_streaming_token_count_ignores_original_content() {
    use neo_tui::transcript::tool_renderers::estimate_tool_tokens;

    let original = "original file content ".repeat(1_000);
    let before_new = format!(r#"{{"path":"src/foo.rs","old":"{original}""#);
    assert_eq!(estimate_tool_tokens("Edit", &before_new), 0);

    let with_new = format!(r#"{before_new},"new":"small replacement"#);
    assert_eq!(
        estimate_tool_tokens("Edit", &with_new),
        estimate_tool_tokens("Write", "small replacement")
    );
}

#[test]
fn edit_tool_card_renders_finalized_real_line_diff_from_details() {
    let theme = TuiTheme::default();
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Edit".to_owned(),
        arguments: Some(
            serde_json::json!({ "path": "src/lib.rs", "old": "old", "new": "new" }).to_string(),
        ),
        result: Some("edited 1 files".to_owned()),
        details: Some(serde_json::json!({
            "kind": "edit",
            "status": "committed",
            "files": 1,
            "replacements": 1,
            "added": 1,
            "removed": 1,
            "changes": [{
                "path": "src/lib.rs",
                "status": "committed",
                "replacements": 1,
                "added": 1,
                "removed": 1,
                "diff": "--- src/lib.rs\n+++ src/lib.rs\n@@ -40,3 +40,3 @@\n context\n-old\n+new\n tail\n"
            }]
        })),
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rendered = card.render_with_theme(80, &theme);
    let rows = plain(rendered.clone());
    assert!(
        rows[0].contains("Used Edit · 1 files · 1 replacements · +1 -1"),
        "batch summary missing: {rows:?}"
    );
    assert_eq!(
        rows.iter()
            .filter(|line| line.contains("1 files · 1 replacements"))
            .count(),
        1,
        "batch summary should only appear in the header: {rows:?}"
    );
    assert!(
        rendered[0]
            .spans()
            .iter()
            .any(|span| span.text() == "+1" && span.style().fg == Some(theme.diff_added))
    );
    assert!(
        rendered[0]
            .spans()
            .iter()
            .any(|span| span.text() == "-1" && span.style().fg == Some(theme.diff_removed))
    );
    assert!(rows.iter().any(|line| line.contains("src/lib.rs")));
    assert!(
        rows.iter()
            .any(|line| line.contains("-old") || line.contains("- old"))
    );
    assert!(
        rows.iter()
            .any(|line| line.contains("+new") || line.contains("+ new"))
    );
}

#[test]
fn partial_edit_header_uses_committed_totals_only() {
    let theme = TuiTheme::default();
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "edit-partial-chip".to_owned(),
        name: "Edit".to_owned(),
        arguments: None,
        result: Some("partial".to_owned()),
        details: Some(json!({
            "kind": "edit",
            "status": "partial_commit",
            "files": 2,
            "replacements": 2,
            "added": 1,
            "removed": 1,
            "changes": [
                {"path": "done.rs", "status": "committed", "added": 1, "removed": 1, "diff": "--- done.rs\n+++ done.rs\n@@ -1 +1 @@\n-a\n+b\n"},
                {"path": "pending.rs", "status": "not_attempted", "added": 20, "removed": 20, "diff": "--- pending.rs\n+++ pending.rs\n@@ -1 +1 @@\n-a\n+b\n"}
            ]
        })),
        status: ToolStatusKind::Failed,
        exit_code: None,
    });

    let themed = card.render_with_theme(80, &theme);
    let rows = plain(themed.clone());
    assert!(rows[0].contains("+1 -1"), "header: {}", rows[0]);
    assert!(!rows[0].contains("+21 -21"), "header: {}", rows[0]);
    assert!(
        rows.iter().any(|row| row.contains("pending.rs")),
        "{rows:?}"
    );
    let pending = themed
        .iter()
        .find(|line| line.text().contains("pending.rs"))
        .expect("pending header");
    assert!(
        pending
            .spans()
            .iter()
            .any(|span| { span.text() == "+20" && span.style().fg == Some(theme.text_muted) })
    );
    assert!(
        pending
            .spans()
            .iter()
            .any(|span| { span.text() == "-20" && span.style().fg == Some(theme.text_muted) })
    );
}
