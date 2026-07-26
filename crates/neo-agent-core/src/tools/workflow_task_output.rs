//! Workflow TaskOutput adapter (design §35).
//!
//! BackgroundTaskManager remains a projection/control adapter only. This module
//! owns the workflow-specific view routing, cursor binding, and ToolResult cap
//! for summary / journal / result / artifacts / artifact_content.

use super::{ToolError, ToolResult};
use crate::workflow::{
    TaskOutputPage, TaskOutputRequest, TaskOutputView, WorkflowError, WorkflowHandle,
    page_to_tool_result,
};

/// Execute a paged workflow TaskOutput request against a live handle.
pub async fn workflow_task_output(
    handle: &WorkflowHandle,
    request: TaskOutputRequest,
) -> Result<ToolResult, ToolError> {
    let max_output_bytes = request.max_output_bytes;
    let page = handle
        .task_output(request)
        .await
        .map_err(|error| map_workflow_error("TaskOutput", error))?;
    page_into_tool_result(&page, max_output_bytes)
}

pub fn page_into_tool_result(
    page: &TaskOutputPage,
    max_output_bytes: u64,
) -> Result<ToolResult, ToolError> {
    let (content, details) =
        page_to_tool_result(page, max_output_bytes).map_err(|error| ToolError::InvalidInput {
            tool: "TaskOutput".to_owned(),
            message: error.to_string(),
        })?;
    let failed = matches!(
        page.state,
        crate::workflow::WorkflowState::Failed
            | crate::workflow::WorkflowState::Cancelled
            | crate::workflow::WorkflowState::ResourceLimited
    );
    let result = if failed {
        ToolResult::error(content)
    } else {
        ToolResult::ok(content)
    };
    Ok(result.with_details(details))
}

/// Parse tool-level view string; default is summary.
pub fn parse_view(raw: Option<&str>) -> Result<TaskOutputView, ToolError> {
    match raw {
        None => Ok(TaskOutputView::Summary),
        Some(value) => TaskOutputView::parse(value).map_err(|error| ToolError::InvalidInput {
            tool: "TaskOutput".to_owned(),
            message: error.to_string(),
        }),
    }
}

fn map_workflow_error(tool: &str, error: WorkflowError) -> ToolError {
    ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: error.to_string(),
    }
}
