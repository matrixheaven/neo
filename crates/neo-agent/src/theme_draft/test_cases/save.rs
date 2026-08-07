//! save behavior (moved from `theme_draft.rs`).

use super::super::*;
use super::*;
use neo_agent_core::ToolAccess;
use neo_agent_core::tools::ToolContext;
use serde_json::json;
use tempfile::TempDir;

#[tokio::test]
async fn save_persists_previewed_draft_inside_theme_home() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    let tool = ThemeDraftTool::new(repo.clone(), Arc::new(Mutex::new(ThemeDraftStore::new())));
    let ctx = context(&temp);

    let preview = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
    assert!(!preview.is_error);
    let draft_id = result_details(&preview)["draft_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let expected_fingerprint = result_details(&preview)["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();

    let saved = tool
        .execute(&ctx, json!({"action": "save", "draft_id": draft_id}))
        .await
        .expect("save runs");
    assert!(!saved.is_error, "save failed: {}", saved.content);

    let details = result_details(&saved);
    assert_eq!(details["kind"], "theme_draft_saved");
    assert_eq!(details["theme_id"], "aurora-night.json");
    assert_eq!(details["fingerprint"], expected_fingerprint);
    assert_eq!(details["applied"], false);

    let entry = repo
        .resolve(&crate::themes::ThemeId::new("aurora-night.json").unwrap())
        .unwrap();
    assert!(entry.is_valid());
    assert_eq!(entry.name, "Aurora Night");
    let on_disk = std::fs::read_to_string(&entry.path).unwrap();
    assert_eq!(super::super::fingerprint_of(&on_disk), expected_fingerprint);
}

#[tokio::test]
async fn save_rejects_unknown_draft_and_extra_fields() {
    let temp = TempDir::new().expect("tempdir");
    let tool = tool_in(&temp);
    let ctx = context(&temp);

    let expired = tool
        .execute(&ctx, json!({"action": "save", "draft_id": "draft-missing"}))
        .await
        .expect("save runs");
    assert!(expired.is_error);
    assert_eq!(result_details(&expired)["error"], "expired_draft");

    let extra = tool
        .execute(
            &ctx,
            json!({"action": "save", "draft_id": "x", "colors": {"brand": "#fff"}}),
        )
        .await
        .expect("save runs");
    assert!(extra.is_error);
    assert_eq!(result_details(&extra)["error"], "invalid_input");
}

#[tokio::test]
async fn save_conflict_requires_explicit_overwrite() {
    let temp = TempDir::new().expect("tempdir");
    let repo = repository_in(&temp);
    let tool = ThemeDraftTool::new(repo.clone(), Arc::new(Mutex::new(ThemeDraftStore::new())));
    let ctx = context(&temp);

    let preview = run_preview(&tool, &ctx, preview_input("Aurora Night")).await;
    let draft_id = result_details(&preview)["draft_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let conflict = tool
        .execute(&ctx, json!({"action": "save", "draft_id": draft_id}))
        .await
        .expect("first save");
    assert!(!conflict.is_error, "{}", conflict.content);

    let conflict = tool
        .execute(&ctx, json!({"action": "save", "draft_id": draft_id}))
        .await
        .expect("conflicting save");
    assert!(conflict.is_error);
    assert_eq!(result_details(&conflict)["error"], "conflict");

    let overwritten = tool
        .execute(
            &ctx,
            json!({"action": "save", "draft_id": draft_id, "overwrite": true}),
        )
        .await
        .expect("overwrite save");
    assert!(!overwritten.is_error, "{}", overwritten.content);
    assert_eq!(result_details(&overwritten)["applied"], false);
}

#[tokio::test]
async fn save_is_denied_without_tool_access() {
    let temp = TempDir::new().expect("tempdir");
    let tool = tool_in(&temp);
    let workspace = TempDir::new().expect("tempdir");
    let denied_ctx = ToolContext::new(workspace.path())
        .expect("context")
        .with_access(ToolAccess::none());

    let error = tool
        .execute(&denied_ctx, preview_input("Aurora Night"))
        .await
        .expect_err("tool access required");
    assert!(matches!(
        error,
        ToolError::PermissionDenied { operation: "tool" }
    ));
}

#[test]
fn candidate_id_slugs_display_names_deterministically() {
    assert_eq!(
        candidate_theme_id("Aurora Night").unwrap().as_str(),
        "aurora-night.json"
    );
    assert_eq!(
        candidate_theme_id("  Aurora   Night  ").unwrap().as_str(),
        "aurora-night.json"
    );
    assert_eq!(
        candidate_theme_id("B.R.A.N.D.").unwrap().as_str(),
        "b-r-a-n-d.json"
    );
    assert_eq!(candidate_theme_id("主题").unwrap().as_str(), "主题.json");
    assert!(candidate_theme_id("").is_err());
}
