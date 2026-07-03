use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Tool, ToolContext, ToolError, ToolFuture, ToolResult, parse_input};

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
        "Use this tool proactively when you are about to start a non-trivial implementation task. \
         Getting user sign-off on your approach via ExitPlanMode before writing code prevents wasted effort.\n\n\
         Use it when ANY of these conditions apply:\n\
         1. New Feature Implementation - e.g. \"Add a caching layer to the API\".\n\
         2. Multiple Valid Approaches - e.g. \"Optimize database queries\" (indexing vs rewrite vs caching).\n\
         3. Code Modifications - e.g. \"Refactor parser module to support streaming\".\n\
         4. Architectural Decisions - e.g. \"Add WebSocket support\".\n\
         5. Multi-File Changes - involves more than 2-3 files.\n\
         6. Unclear Requirements - need exploration to understand scope.\n\
         7. User Preferences Matter - if user input would materially change the implementation approach.\n\n\
         Permission mode notes:\n\
         - EnterPlanMode enters plan mode automatically without an approval prompt in all permission modes.\n\
         - In yolo and ask modes, ExitPlanMode still presents the plan to the user for approval.\n\
         - In auto permission mode, do not use AskUserQuestion; make the best decision from available context.\n\
         - In auto permission mode, ExitPlanMode exits plan mode without asking the user.\n\
         - Use EnterPlanMode only when planning itself adds value.\n\n\
         When NOT to use:\n\
         - Single-line or few-line fixes (typos, obvious bugs, small tweaks).\n\
         - User gave very specific, detailed instructions.\n\
         - Pure research/exploration tasks.\n\n\
         What happens in plan mode:\n\
         1. Identify 2-3 key questions about the codebase that are critical to your plan. If you are not confident about the codebase structure or relevant code paths, use read-only exploration tools first.\n\
         2. Explore the codebase using Glob, Grep, Read, and other read-only tools. Use Bash only when needed; Bash follows the normal permission mode and rules.\n\
         3. Design a concrete, step-by-step plan.\n\
         4. Write your plan to the current plan file with Write or Edit.\n\
         5. Present your plan to the user via ExitPlanMode for approval."
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

/// A single user-selectable option surfaced at plan approval time.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExitPlanModeOption {
    /// Short name for this option (1-8 words). Append "(Recommended)" if you recommend this option.
    #[schemars(
        description = "Short name for this option (1-8 words). Append \"(Recommended)\" if you recommend this option."
    )]
    pub label: String,
    /// Brief summary of this approach and its trade-offs.
    #[schemars(description = "Brief summary of this approach and its trade-offs.")]
    pub description: Option<String>,
}

/// A preset revision suggestion offered during plan review.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExitPlanModeSuggestion {
    /// Short label shown as the suggestion title.
    #[schemars(description = "Short label shown as the suggestion title.")]
    pub label: String,
    /// Longer explanation shown under the label.
    #[schemars(description = "Longer explanation shown under the label.")]
    pub description: String,
    /// Feedback text to populate when the user selects this suggestion.
    #[schemars(
        description = "Feedback text to populate when the user selects this suggestion. Defaults to description."
    )]
    #[serde(default)]
    pub feedback: Option<String>,
}

/// Input payload for [`ExitPlanModeTool`].
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExitPlanModeInput {
    /// Optional brief summary of the plan for the user to review. The canonical plan content should already be written to the plan file.
    #[schemars(
        description = "Optional brief summary of the plan for the user to review. The plan itself should already be written to the plan file."
    )]
    pub plan_summary: Option<String>,
    /// Optional alternative approaches for the user to choose from.
    #[schemars(
        description = "When the plan contains multiple alternative approaches, list them here so the user can choose which one to execute. Provide up to 3 options; 2-3 distinct approaches work best when the plan offers a real choice. Passing a single option is allowed and is equivalent to a plain plan approval."
    )]
    pub options: Option<Vec<ExitPlanModeOption>>,
    /// Optional preset revision suggestions shown below the plan box.
    #[schemars(
        description = "Optional preset revision suggestions shown below the plan box. Each suggestion has a label, description, and optional feedback text that is populated when the user selects it."
    )]
    #[serde(default)]
    pub suggestions: Option<Vec<ExitPlanModeSuggestion>>,
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
        "Use this tool when you are in plan mode and have finished writing your plan to the plan file and are ready for user approval.\n\n\
         How this tool works:\n\
         - You should have already written your plan to the plan file specified in the plan mode reminder.\n\
         - This tool does NOT take the plan content as a parameter - it reads the plan from the file you wrote.\n\
         - The user will see the contents of your plan file when they review it. In auto permission mode, the tool reads the file and exits plan mode without asking the user.\n\n\
         When to use:\n\
         Only use this tool for tasks that require planning implementation steps. For research tasks (searching files, reading code, understanding the codebase), do NOT use this tool.\n\n\
         Multiple approaches:\n\
         - If your plan contains multiple alternative approaches, pass them via the `options` parameter so the user can choose which approach to execute.\n\
         - Each option should have a concise label and a brief description of trade-offs.\n\
         - If you recommend one option, list it first and append \"(Recommended)\" to its label.\n\
         - In yolo and ask modes, the user will see all options alongside Reject and Revise choices.\n\
         - Provide up to 3 options; the host adds the standard rejection and revision controls.\n\
         - Passing a single option is allowed and is equivalent to a plain plan approval.\n\
         - Do NOT use \"Reject\", \"Reject and Exit\", \"Revise\", or \"Approve\" as option labels - these are reserved by the system.\n\n\
         Before using:\n\
         - In auto permission mode, do NOT use AskUserQuestion; make the best decision from available context.\n\
         - In auto permission mode, this tool exits plan mode without asking the user.\n\
         - In yolo and ask modes, this tool still presents the plan to the user for approval.\n\
         - If auto permission mode is not active and you have unresolved questions, use AskUserQuestion first.\n\
         - If auto permission mode is not active and you have multiple approaches and have not narrowed down yet, consider using AskUserQuestion first to let the user choose, then write a plan for the chosen approach only.\n\
         - Once your plan is finalized, use THIS tool to request approval.\n\
         - Do NOT use AskUserQuestion to ask \"Is this plan OK?\" or \"Should I proceed?\" - that is exactly what ExitPlanMode does.\n\
         - If rejected, revise based on feedback and call ExitPlanMode again.
         - You may include preset revision suggestions in the `suggestions` parameter to help the user quickly request common changes."
    }

    fn input_schema(&self) -> serde_json::Value {
        neo_ai::tool_schema::schema_for::<ExitPlanModeInput>()
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: ExitPlanModeInput = parse_input(self.name(), input)?;

            if let Some(ref options) = input.options {
                validate_exit_plan_mode_options(options)?;
            }
            if let Some(ref suggestions) = input.suggestions {
                validate_exit_plan_mode_suggestions(suggestions)?;
            }

            let summary = input
                .plan_summary
                .as_deref()
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or("No summary provided");
            Ok(ToolResult::ok(format!("Exiting plan mode. Plan summary: {summary}")).terminate())
        })
    }
}

/// Validates that option labels are unique and do not collide with reserved approval labels.
fn validate_exit_plan_mode_options(options: &[ExitPlanModeOption]) -> Result<(), ToolError> {
    if options.len() > 3 {
        return Err(ToolError::InvalidInput {
            tool: "ExitPlanMode".to_owned(),
            message: format!(
                "options must contain at most 3 items, got {}",
                options.len()
            ),
        });
    }

    let reserved: &[&str] = &["approve", "reject", "revise", "reject and exit"];
    let mut seen = std::collections::HashSet::new();
    for option in options {
        let normalized = option.label.trim().to_lowercase();
        if normalized.is_empty() {
            return Err(ToolError::InvalidInput {
                tool: "ExitPlanMode".to_owned(),
                message: "option label must not be empty".to_owned(),
            });
        }
        if reserved.contains(&normalized.as_str()) {
            return Err(ToolError::InvalidInput {
                tool: "ExitPlanMode".to_owned(),
                message: format!("option label `{}` is reserved", option.label),
            });
        }
        if !seen.insert(normalized.clone()) {
            return Err(ToolError::InvalidInput {
                tool: "ExitPlanMode".to_owned(),
                message: format!("duplicate option label `{}`", option.label),
            });
        }
    }
    Ok(())
}

fn validate_exit_plan_mode_suggestions(
    suggestions: &[ExitPlanModeSuggestion],
) -> Result<(), ToolError> {
    if suggestions.len() > 5 {
        return Err(ToolError::InvalidInput {
            tool: "ExitPlanMode".to_owned(),
            message: format!(
                "suggestions must contain at most 5 items, got {}",
                suggestions.len()
            ),
        });
    }
    let mut seen = std::collections::HashSet::new();
    for suggestion in suggestions {
        let normalized = suggestion.label.trim().to_lowercase();
        if normalized.is_empty() {
            return Err(ToolError::InvalidInput {
                tool: "ExitPlanMode".to_owned(),
                message: "suggestion label must not be empty".to_owned(),
            });
        }
        if !seen.insert(normalized.clone()) {
            return Err(ToolError::InvalidInput {
                tool: "ExitPlanMode".to_owned(),
                message: format!("duplicate suggestion label `{}`", suggestion.label),
            });
        }
    }
    Ok(())
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

    #[test]
    fn enter_plan_mode_schema_is_valid() {
        let schema = EnterPlanModeTool.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn exit_plan_mode_schema_does_not_require_summary() {
        let schema = ExitPlanModeTool.input_schema();
        assert_eq!(schema["type"], "object");
        let plan_summary = resolve_schema_ref(&schema, &schema["properties"]["plan_summary"]);
        assert!(
            plan_summary["type"].is_string()
                || plan_summary["type"].as_array().is_some_and(|types| {
                    types.iter().any(|t| t == "string") && types.iter().any(|t| t == "null")
                })
                || plan_summary.get("anyOf").is_some()
                || plan_summary.get("oneOf").is_some(),
            "plan_summary schema should be a string or optional string, got: {plan_summary}"
        );
        let required = schema["required"].as_array();
        assert!(!required.is_some_and(|arr| { arr.iter().any(|v| v == "plan_summary") }));
    }

    #[test]
    fn exit_plan_mode_schema_has_options() {
        let schema = ExitPlanModeTool.input_schema();
        let options = resolve_schema_ref(&schema, &schema["properties"]["options"]);
        assert!(
            options["type"] == "array"
                || options["type"].as_array().is_some_and(|types| {
                    types.iter().any(|t| t == "array") && types.iter().any(|t| t == "null")
                })
                || options.get("anyOf").is_some()
                || options.get("oneOf").is_some(),
            "options schema should be an array or optional array, got: {options}"
        );
    }

    fn resolve_schema_ref<'schema>(root: &'schema Value, node: &'schema Value) -> &'schema Value {
        if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
            let defs = root
                .get("$defs")
                .or_else(|| root.get("definitions"))
                .expect("schema defs");
            let name = reference.split('/').next_back().expect("ref name");
            return &defs[name];
        }
        node
    }

    #[test]
    fn tool_names() {
        assert_eq!(EnterPlanModeTool.name(), "EnterPlanMode");
        assert_eq!(ExitPlanModeTool.name(), "ExitPlanMode");
    }
}
