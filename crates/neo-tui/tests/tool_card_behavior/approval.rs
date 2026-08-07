use neo_tui::primitive::{Component, Expandable, Line};
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::{ToolCallComponent, ToolCallState};

fn plain(rows: Vec<Line>) -> Vec<String> {
    rows.into_iter()
        .map(|row| neo_tui::primitive::strip_ansi(&row.to_ansi()))
        .collect()
}

#[test]
fn edit_batch_approval_uses_global_expansion() {
    // Approval entry expansion is owned by global Ctrl+O; renderer accepts expanded flag.
    let details = serde_json::json!({
        "kind": "edit_prepared",
        "files": 2,
        "replacements": 2,
        "added": 2,
        "removed": 2,
        "changes": [
            {
                "path": "a.rs",
                "replacements": 1,
                "added": 1,
                "removed": 1,
                "diff": "--- a.rs\n+++ a.rs\n@@ -1 +1 @@\n-a\n+A\n"
            },
            {
                "path": "b.rs",
                "replacements": 1,
                "added": 1,
                "removed": 1,
                "diff": "--- b.rs\n+++ b.rs\n@@ -1 +1 @@\n-b\n+B\n"
            }
        ]
    });
    let mut card = ToolCallComponent::new(ToolCallState {
        id: "edit-prep".to_owned(),
        name: "Edit".to_owned(),
        arguments: None,
        result: None,
        details: Some(details),
        status: ToolStatusKind::Running,
        exit_code: None,
    });
    card.set_expanded(true);
    let rows = plain(card.render(80));
    assert!(rows.iter().any(|line| line.contains("a.rs")));
    assert!(rows.iter().any(|line| line.contains("b.rs")));
}
