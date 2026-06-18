use neo_tui::tool_diff::{DiffModel, DiffRenderState};

#[test]
fn diff_model_parses_edit_tool_details_into_files_hunks_and_stats() {
    let details = serde_json::json!({
        "path": "src/lib.rs",
        "diff": "--- src/lib.rs\n+++ src/lib.rs\n@@ -1,3 +1,3 @@\n unchanged\n-old\n+new\n@@ -8,2 +8,3 @@\n context\n+extra\n"
    });

    let model = DiffModel::from_tool_details(&details).expect("diff details parse");

    assert_eq!(model.files().len(), 1);
    assert_eq!(model.files()[0].old_path, "src/lib.rs");
    assert_eq!(model.files()[0].new_path, "src/lib.rs");
    assert_eq!(model.files()[0].hunks.len(), 2);
    assert_eq!(model.stats().files_changed, 1);
    assert_eq!(model.stats().added, 2);
    assert_eq!(model.stats().removed, 1);
}

#[test]
fn diff_render_state_navigates_and_folds_hunks() {
    let model = DiffModel::parse_unified(
        "--- src/a.rs\n+++ src/a.rs\n@@\n-a\n+b\n@@\n-c\n+d\n--- src/b.rs\n+++ src/b.rs\n@@\n-old\n+new\n",
    )
    .expect("diff parses");
    let mut state = DiffRenderState::new(model);

    assert_eq!(state.active_file_index(), 0);
    assert_eq!(state.active_hunk_index(), 0);
    assert_eq!(state.stats().added, 3);
    assert_eq!(state.stats().removed, 3);

    state.next_hunk();
    assert_eq!(state.active_file_index(), 0);
    assert_eq!(state.active_hunk_index(), 1);
    state.next_hunk();
    assert_eq!(state.active_file_index(), 1);
    assert_eq!(state.active_hunk_index(), 0);
    state.previous_hunk();
    assert_eq!(state.active_file_index(), 0);
    assert_eq!(state.active_hunk_index(), 1);

    assert!(!state.is_active_hunk_folded());
    state.toggle_active_hunk_fold();
    assert!(state.is_active_hunk_folded());

    let rendered = state.render_lines(80);
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("@@ folded 2 changes"))
    );
    assert!(!rendered.iter().any(|line| line == "-c"));
    assert!(!rendered.iter().any(|line| line == "+d"));

    state.unfold_active_hunk();
    assert!(!state.is_active_hunk_folded());
}

#[test]
fn diff_render_state_groups_files_and_collapses_active_file() {
    let model = DiffModel::parse_unified(
        "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n-before\n+after\n",
    )
    .expect("diff parses");
    let mut state = DiffRenderState::new(model);

    let rendered = state.render_lines(80);
    assert!(rendered.iter().any(|line| line.contains("src/a.rs")));
    assert!(rendered.iter().any(|line| line.contains("1 hunk")));

    state.toggle_active_file_fold();
    assert!(state.is_active_file_folded());
    let folded = state.render_lines(80);
    assert!(folded.iter().any(|line| {
        line.contains("src/a.rs") && line.contains("folded") && line.contains("2 changes")
    }));
    assert!(!folded.iter().any(|line| line == "-old"));
    assert!(!folded.iter().any(|line| line == "+new"));

    state.next_file();
    assert_eq!(state.active_file_index(), 1);
    assert_eq!(state.active_hunk_index(), 0);
    state.previous_file();
    assert_eq!(state.active_file_index(), 0);
}

#[test]
fn diff_render_state_copies_active_hunk_and_file_as_unified_diff() {
    let model = DiffModel::parse_unified(
        "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\n@@ -8 +8 @@\n-before\n+after\n",
    )
    .expect("diff parses");
    let mut state = DiffRenderState::new(model);

    assert_eq!(
        state.copy_active_hunk().as_deref(),
        Some("--- src/a.rs\n+++ src/a.rs\n@@ -1 +1 @@\n-old\n+new\n")
    );

    state.next_hunk();
    assert_eq!(
        state.copy_active_hunk().as_deref(),
        Some("--- src/a.rs\n+++ src/a.rs\n@@ -8 +8 @@\n-before\n+after\n")
    );
    assert_eq!(
        state.copy_active_file().as_deref(),
        Some("--- src/a.rs\n+++ src/a.rs\n@@ -1 +1 @@\n-old\n+new\n@@ -8 +8 @@\n-before\n+after\n")
    );
}
