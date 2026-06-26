use neo_tui::diff_model::{DiffModel, DiffRenderState};

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
            .any(|line| line.contains("folded 2 changes"))
    );
    assert!(!rendered.iter().any(|line| line.contains("- c")));
    assert!(!rendered.iter().any(|line| line.contains("+ d")));

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
    assert!(rendered.iter().any(|line| line.contains("+1 -1 src/a.rs")));

    state.toggle_active_file_fold();
    assert!(state.is_active_file_folded());
    let folded = state.render_lines(80);
    assert!(folded.iter().any(|line| {
        line.contains("src/a.rs") && line.contains("folded") && line.contains("+1 -1")
    }));
    assert!(!folded.iter().any(|line| line.contains("- old")));
    assert!(!folded.iter().any(|line| line.contains("+ new")));

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

#[test]
fn diff_render_state_renders_real_line_number_gutter_and_hunk_separator() {
    let model = DiffModel::parse_unified(
        "--- src/lib.rs\n+++ src/lib.rs\n@@ -10,3 +10,3 @@\n keep\n-old\n+new\n@@ -42,2 +42,3 @@\n context\n+extra\n",
    )
    .expect("diff parses");
    let state = DiffRenderState::new(model);

    let rendered = state.render_lines(80);

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("+2 -1 src/lib.rs"))
    );
    assert!(rendered.iter().any(|line| line == " 11 - old"));
    assert!(rendered.iter().any(|line| line == " 11 + new"));
    assert!(rendered.iter().any(|line| line == " 42   context"));
    assert!(rendered.iter().any(|line| line == " 43 + extra"));
    assert!(rendered.iter().any(|line| line.trim() == "⋮"));
    assert!(
        !rendered.iter().any(|line| line.starts_with("@@")),
        "hunk headers should not be the primary transcript UI: {rendered:?}"
    );
}

#[test]
fn diff_render_state_wraps_long_lines_under_content_column() {
    let model = DiffModel::parse_unified(
        "--- src/lib.rs\n+++ src/lib.rs\n@@ -7,1 +7,1 @@\n-short\n+abcdefghijklmnopqrstuvwxyz\n",
    )
    .expect("diff parses");
    let state = DiffRenderState::new(model);

    let rendered = state.render_lines(16);

    let first = rendered
        .iter()
        .find(|line| line.contains("+ abcdefgh"))
        .expect("first added row");
    let continuation = rendered
        .iter()
        .find(|line| line.contains("lmnop"))
        .expect("wrapped continuation row");

    let first_content_col = first.find("abcdefgh").expect("first content column");
    let continuation_col = continuation.find("lmnop").expect("continuation column");
    assert_eq!(first_content_col, continuation_col);
    assert!(rendered.iter().all(|line| line.chars().count() <= 16));
}
