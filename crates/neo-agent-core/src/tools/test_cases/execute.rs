use super::*;
use crate::ToolContext;

#[tokio::test]
async fn enter_plan_mode_returns_terminate() {
    let ctx = ToolContext::new(".").expect("context");
    let result = EnterPlanModeTool
        .execute(&ctx, json!({}))
        .await
        .expect("execute");
    assert!(result.terminate);
    assert!(!result.is_error);
    assert!(result.content.contains("plan mode"));
}

#[tokio::test]
async fn exit_plan_mode_returns_terminate() {
    let ctx = ToolContext::new(".").expect("context");
    let result = ExitPlanModeTool
        .execute(&ctx, json!({"plan_summary": "Refactor module X"}))
        .await
        .expect("execute");
    assert!(result.terminate);
    assert!(!result.is_error);
    assert!(result.content.contains("Refactor module X"));
}

#[tokio::test]
async fn exit_plan_mode_allows_no_summary() {
    let ctx = ToolContext::new(".").expect("context");
    let result = ExitPlanModeTool
        .execute(&ctx, json!({}))
        .await
        .expect("execute");
    assert!(result.terminate);
    assert!(result.content.contains("No summary provided"));
}

#[tokio::test]
async fn exit_plan_mode_accepts_options() {
    let ctx = ToolContext::new(".").expect("context");
    let result = ExitPlanModeTool
        .execute(
            &ctx,
            json!({
                "plan_summary": "Add feature",
                "options": [
                    {"label": "Approach A", "description": "Simple"},
                    {"label": "Approach B (Recommended)", "description": "Fast"}
                ]
            }),
        )
        .await
        .expect("execute");
    assert!(result.terminate);
    assert!(result.content.contains("Add feature"));
}

#[tokio::test]
async fn exit_plan_mode_rejects_reserved_label() {
    let ctx = ToolContext::new(".").expect("context");
    let result = ExitPlanModeTool
        .execute(
            &ctx,
            json!({
                "options": [{"label": "Approve"}]
            }),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn exit_plan_mode_rejects_duplicate_label() {
    let ctx = ToolContext::new(".").expect("context");
    let result = ExitPlanModeTool
        .execute(
            &ctx,
            json!({
                "options": [
                    {"label": "Same"},
                    {"label": "same"}
                ]
            }),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn exit_plan_mode_rejects_too_many_options() {
    let ctx = ToolContext::new(".").expect("context");
    let result = ExitPlanModeTool
        .execute(
            &ctx,
            json!({
                "options": [
                    {"label": "A"},
                    {"label": "B"},
                    {"label": "C"},
                    {"label": "D"}
                ]
            }),
        )
        .await;
    assert!(result.is_err());
}
