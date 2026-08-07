//! preview behavior (moved from `theme_draft.rs`).

use super::super::*;
use super::*;
use serde_json::json;
use tempfile::TempDir;

#[tokio::test]
async fn preview_materializes_complete_independent_theme() {
    let temp = TempDir::new().expect("tempdir");
    let tool = tool_in(&temp);
    let ctx = context(&temp);

    let result = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
    assert!(!result.is_error, "preview failed: {}", result.content);

    let details = result_details(&result);
    assert_eq!(details["kind"], "theme_draft_preview");
    assert_eq!(details["display_name"], "Aurora Night");
    assert_eq!(details["candidate_theme_id"], "aurora-night.json");
    assert_eq!(details["applied"], false);
    assert!(details["draft_id"].as_str().unwrap().starts_with("draft-"));

    let colors = details["normalized_colors"].as_object().unwrap();
    assert_eq!(colors.len(), CANONICAL_TOKENS.len());
    assert_eq!(colors["brand"], "#58a6ff");
    // Uppercase input hex is normalized to lowercase canonical form.
    assert_eq!(colors["text_primary"], "#e6edf3");
    // Non-overridden tokens come from the built-in default.
    assert_eq!(colors["status_ok"], "#4ec87e");
    assert!(colors.contains_key("shell_mode"));
}

#[tokio::test]
async fn preview_is_non_mutating_and_store_is_shared_with_runtime() {
    let temp = TempDir::new().expect("tempdir");
    let store = Arc::new(Mutex::new(ThemeDraftStore::new()));
    let tool = ThemeDraftTool::new(repository_in(&temp), Arc::clone(&store));
    let ctx = context(&temp);

    let result = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
    assert!(!result.is_error);
    let draft_id = result_details(&result)["draft_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(store.lock().unwrap().get(&draft_id).is_some());
    // No theme files were written by a preview.
    assert_eq!(repository_in(&temp).catalog().unwrap().entries.len(), 0);
    assert_eq!(store.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn preview_then_save_across_tool_instances_shares_the_session_store() {
    // The interactive session owns one bounded store threaded through every
    // turn's runtime. A preview issued by instance A (turn N) must be
    // savable by instance B (turn N+1) built from the same Arc; a fresh
    // store (a different session) must reject the draft as expired.
    let temp = TempDir::new().expect("tempdir");
    let store = Arc::new(Mutex::new(ThemeDraftStore::new()));
    let repo = repository_in(&temp);
    let turn_a = ThemeDraftTool::new(repo.clone(), Arc::clone(&store));
    let turn_b = ThemeDraftTool::new(repo.clone(), Arc::clone(&store));
    let ctx = context(&temp);

    let preview = turn_a
        .execute(&ctx, preview_input("Aurora Night"))
        .await
        .expect("preview runs");
    assert!(!preview.is_error, "{}", preview.content);
    let draft_id = result_details(&preview)["draft_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let saved = turn_b
        .execute(&ctx, json!({"action": "save", "draft_id": draft_id}))
        .await
        .expect("save runs");
    assert!(
        !saved.is_error,
        "save across turns must succeed: {}",
        saved.content
    );
    assert_eq!(result_details(&saved)["applied"], false);

    // A different Arc (fresh session) cannot see the draft.
    let other_session = ThemeDraftTool::new(repo, Arc::new(Mutex::new(ThemeDraftStore::new())));
    let expired = other_session
        .execute(&ctx, json!({"action": "save", "draft_id": draft_id}))
        .await
        .expect("save runs");
    assert!(expired.is_error);
    assert_eq!(result_details(&expired)["error"], "expired_draft");
}

#[tokio::test]
async fn preview_rejects_unknown_tokens_and_invalid_colors() {
    let temp = TempDir::new().expect("tempdir");
    let tool = tool_in(&temp);
    let ctx = context(&temp);

    let unknown = run_preview(
        &tool,
        &ctx,
        json!({
            "action": "preview",
            "name": "Bad",
            "colors": {"accent": "#ff0000"},
        }),
    )
    .await;
    assert!(unknown.is_error);
    assert_eq!(result_details(&unknown)["error"], "invalid_input");

    let bad_color = run_preview(
        &tool,
        &ctx,
        json!({
            "action": "preview",
            "name": "Bad",
            "colors": {"brand": "not-a-color"},
        }),
    )
    .await;
    assert!(bad_color.is_error);
    assert_eq!(result_details(&bad_color)["error"], "invalid_input");
}

#[tokio::test]
async fn preview_rejects_unknown_json_fields() {
    let temp = TempDir::new().expect("tempdir");
    let tool = tool_in(&temp);
    let ctx = context(&temp);

    let result = tool
        .execute(
            &ctx,
            json!({
                "action": "preview",
                "name": "Aurora",
                "bogus_field": true,
            }),
        )
        .await
        .expect("tool runs");
    assert!(result.is_error);
    assert_eq!(result_details(&result)["error"], "invalid_input");
}

#[tokio::test]
async fn preview_validates_display_name_bounds() {
    let temp = TempDir::new().expect("tempdir");
    let tool = tool_in(&temp);
    let ctx = context(&temp);

    for (name, needle) in [
        ("", "empty"),
        ("Aurora/ Night", "separator"),
        ("bad\u{1}name", "control"),
        (&"x".repeat(MAX_DISPLAY_NAME_CHARS + 1), "at most"),
        ("CON", "cannot be used"),
    ] {
        let result = run_preview(&tool, &ctx, preview_input(name)).await;
        assert!(result.is_error, "accepted name {name:?}");
        assert_eq!(
            result_details(&result)["error"],
            "invalid_input",
            "name {name:?}"
        );
        assert!(
            result.content.contains(needle),
            "name {name:?} message {:?} missing {needle:?}",
            result.content
        );
    }
}

#[tokio::test]
async fn preview_base_theme_resolution_and_missing_base() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    let tool = ThemeDraftTool::new(repo.clone(), Arc::new(Mutex::new(ThemeDraftStore::new())));
    let ctx = context(&temp);

    let base_id = crate::themes::ThemeId::new("base.json").unwrap();
    let path = base_id.path_under(repo.root());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r##"{"name": "Base", "colors": {"brand": "#123456"}}"##,
    )
    .unwrap();

    let result = run_preview(
        &tool,
        &ctx,
        json!({
            "action": "preview",
            "name": "Derived",
            "base_theme": "base.json",
            "colors": {"brand": "#ff0000"},
        }),
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    let details = result_details(&result);
    assert_eq!(details["base_theme_id"], "base.json");
    assert_eq!(details["normalized_colors"]["brand"], "#ff0000");

    let missing = run_preview(
        &tool,
        &ctx,
        json!({
            "action": "preview",
            "name": "Derived",
            "base_theme": "nope.json",
        }),
    )
    .await;
    assert!(missing.is_error);
    assert_eq!(result_details(&missing)["error"], "missing_base");
}

#[tokio::test]
async fn preview_fingerprint_is_stable_and_content_driven() {
    let temp = TempDir::new().expect("tempdir");
    let tool = tool_in(&temp);
    let ctx = context(&temp);

    let first = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
    let second = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
    let third = run_preview(&tool, &ctx, preview_input("Different Name")).await;

    let first_fp = result_details(&first)["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(first_fp, result_details(&second)["fingerprint"]);
    assert_eq!(&first_fp[..7], "sha256:");
    assert_ne!(first_fp, result_details(&third)["fingerprint"]);
}

#[tokio::test]
async fn preview_store_is_bounded_and_evicts_oldest_first() {
    let temp = TempDir::new().expect("tempdir");
    let tool = tool_in(&temp);
    let ctx = context(&temp);

    let mut draft_ids = Vec::new();
    for index in 0..(DRAFT_STORE_CAPACITY + 3) {
        let result = run_preview(&tool, &ctx, preview_input(&format!("Theme {index}"))).await;
        assert!(!result.is_error, "{}", result.content);
        draft_ids.push(
            result_details(&result)["draft_id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }

    let store = tool.store();
    let store = store.lock().unwrap();
    assert_eq!(store.len(), DRAFT_STORE_CAPACITY);
    // The three oldest drafts were evicted deterministically.
    for id in &draft_ids[..3] {
        assert!(store.get(id).is_none(), "oldest draft {id} must be evicted");
    }
    for id in &draft_ids[3..] {
        assert!(store.get(id).is_some(), "recent draft {id} must be kept");
    }
}

#[test]
fn contrast_warnings_flag_low_contrast_pairs() {
    let theme = TuiTheme {
        text_primary: Color::Rgb(20, 20, 20), // near-black on default surface
        selection_bg: Color::Rgb(31, 35, 43),
        ..Default::default()
    };
    let warnings = contrast_warnings_for(&theme);
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("text_primary vs selection_bg")),
        "expected a low-contrast warning: {warnings:?}"
    );
}
