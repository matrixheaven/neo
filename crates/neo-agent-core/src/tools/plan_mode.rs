use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolFuture, ToolResult};

/// Tool that requests entering plan mode.
///
/// When the model calls this tool, the runtime intercepts the `terminate`
/// flag on the [`ToolResult`] to switch into plan mode. The actual mode
/// switching is handled at the runtime/interactive layer.
pub struct EnterPlanModeTool;

impl Tool for EnterPlanModeTool {
    fn name(&self) -> &'static str {
        "EnterPlanMode"
    }

    fn description(&self) -> &'static str {
        "Enter plan mode. In plan mode you can read and explore code but cannot make edits \
         except to the plan file. Use this when you need to investigate and plan before \
         making changes."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: Value) -> ToolFuture<'a> {
        Box::pin(async move {
            Ok(ToolResult::ok(
                "Entering plan mode. You can now read and explore code, and write to the plan \
                 file. Other edits and shell commands are blocked until you exit plan mode."
                    .to_owned(),
            )
            .terminate())
        })
    }
}

/// Tool that requests exiting plan mode.
///
/// When the model calls this tool, the runtime intercepts the `terminate`
/// flag to present the plan for user approval and switch back to default
/// mode.
pub struct ExitPlanModeTool;

impl Tool for ExitPlanModeTool {
    fn name(&self) -> &'static str {
        "ExitPlanMode"
    }

    fn description(&self) -> &'static str {
        "Exit plan mode and present the plan for approval. \
         The user will review the plan and decide whether to proceed."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "plan_summary": {
                    "type": "string",
                    "description": "A brief summary of the plan for the user to review"
                },
                "options": {
                    "type": "array",
                    "description": "Optional 1-3 custom approval options (e.g. different approaches). Labels must be unique (case-insensitive). Reserved labels (Approve, Reject, Revise, 'Reject and Exit') are not allowed.",
                    "maxItems": 3,
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": {
                                "type": "string",
                                "maxLength": 80,
                                "description": "Short label for this option"
                            },
                            "description": {
                                "type": "string",
                                "description": "Optional description explaining this option"
                            }
                        },
                        "required": ["label"]
                    }
                }
            },
            "required": ["plan_summary"]
        })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let summary = input
                .get("plan_summary")
                .and_then(Value::as_str)
                .unwrap_or("No summary provided");
            Ok(ToolResult::ok(format!("Exiting plan mode. Plan summary: {summary}")).terminate())
        })
    }
}

#[cfg(test)]
mod tests {
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
    async fn exit_plan_mode_handles_missing_summary() {
        let ctx = ToolContext::new(".").expect("context");
        let result = ExitPlanModeTool
            .execute(&ctx, json!({}))
            .await
            .expect("execute");
        assert!(result.terminate);
        assert!(result.content.contains("No summary provided"));
    }

    #[test]
    fn enter_plan_mode_schema_is_valid() {
        let schema = EnterPlanModeTool.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn exit_plan_mode_schema_requires_summary() {
        let schema = ExitPlanModeTool.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["plan_summary"]["type"].is_string());
        assert!(
            schema["required"]
                .as_array()
                .is_some_and(|arr| { arr.iter().any(|v| v == "plan_summary") })
        );
    }

    #[test]
    fn tool_names() {
        assert_eq!(EnterPlanModeTool.name(), "EnterPlanMode");
        assert_eq!(ExitPlanModeTool.name(), "ExitPlanMode");
    }
}
