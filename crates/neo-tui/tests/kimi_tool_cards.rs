use neo_tui::ToolStatusKind;
use neo_tui::core::{Component, Expandable, Finalization, Line};
use neo_tui::transcript::diff_preview::render_diff_lines_clustered;
use neo_tui::transcript::{ToolCallComponent, ToolCallState};

fn plain(rows: Vec<Line>) -> Vec<String> {
    rows.into_iter()
        .map(|row| neo_tui::ansi::strip_ansi(&row.to_ansi()))
        .collect()
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
            .any(|line| line.contains("✓ Used Read (README.md)"))
    );
    assert!(rows.iter().any(|line| line.contains("2 lines")));
    assert_eq!(card.finalization(), Finalization::Finalized);
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
fn write_tool_card_caps_finalized_content_preview() {
    let content = (1..=20)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
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
        details: None,
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    assert!(
        rows.iter()
            .any(|line| line.contains("src/generated.rs · 20 lines"))
    );
    assert!(rows.iter().any(|line| line.contains("ctrl+o to expand")));
    assert!(!rows.iter().any(|line| line.contains("line 20")));

    card.set_expanded(true);
    let expanded = plain(card.render(80));
    assert!(expanded.iter().any(|line| line.contains("line 20")));
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
        .map(|row| neo_tui::ansi::strip_ansi(&row.to_ansi()))
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
fn edit_tool_card_renders_finalized_clustered_diff_from_args() {
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "tool-1".to_owned(),
        name: "Edit".to_owned(),
        arguments: Some(
            serde_json::json!({
                "path": "src/lib.rs",
                "old_string": "old\nline\n",
                "new_string": "new\nline\nextra\n"
            })
            .to_string(),
        ),
        result: Some("edited src/lib.rs".to_owned()),
        details: None,
        status: ToolStatusKind::Succeeded,
        exit_code: None,
    });

    let rows = plain(card.render(80));
    assert!(rows.iter().any(|line| line.contains("+2 -1 src/lib.rs")));
    assert!(rows.iter().any(|line| line.contains("- old")));
    assert!(rows.iter().any(|line| line.contains("+ new")));
}

#[test]
fn runtime_expansion_state_is_instance_local() {
    let mut expanded_runtime = neo_tui::NeoTuiRuntime::new(80, 12);
    let collapsed_runtime = neo_tui::NeoTuiRuntime::new(80, 12);

    expanded_runtime.set_tool_output_expanded(true);

    assert!(expanded_runtime.tool_output_expanded());
    assert!(!collapsed_runtime.tool_output_expanded());
}
