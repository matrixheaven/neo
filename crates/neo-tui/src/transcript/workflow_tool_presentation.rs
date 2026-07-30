use std::fmt::Write as _;

pub(super) fn workflow_tool_result_summary(
    name: &str,
    details: &serde_json::Value,
) -> Option<String> {
    match name {
        "Workflow" => {
            let action = details.get("action")?.as_str()?;
            if let Some(message) = details
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(serde_json::Value::as_str)
            {
                return Some(message.to_owned());
            }
            let workflow_name = details
                .get("workflow")
                .and_then(|workflow| workflow.get("name"))
                .and_then(serde_json::Value::as_str);
            match action {
                "list" => Some(format!(
                    "Listed {} workflow(s).",
                    details
                        .pointer("/items/entries")
                        .and_then(serde_json::Value::as_array)
                        .map_or(0, Vec::len)
                )),
                "show" => Some(format!("Showing workflow `{}`.", workflow_name?)),
                "validate_inline" | "validate_saved" => {
                    Some(format!("Workflow `{}` is valid.", workflow_name?))
                }
                "save" => Some(format!("Saved workflow `{}`.", workflow_name?)),
                "run_inline" | "run_saved" => {
                    let task = details.get("task")?;
                    Some(format!(
                        "Workflow '{}' started as task {}. Completion arrives automatically; use TaskOutput for optional details.",
                        task.get("display_name")?.as_str()?,
                        task.get("task_id")?.as_str()?
                    ))
                }
                _ => None,
            }
        }
        "TaskOutput"
            if details.get("kind").and_then(serde_json::Value::as_str) == Some("workflow") =>
        {
            let task_id = details.get("run_id")?.as_str()?;
            let status = details.get("status")?.as_str()?;
            let view = details.get("view")?.as_str()?;
            let mut summary = match view {
                "summary" => format!(
                    "task_id: {task_id}\nkind: workflow\nstatus: {status}\nview: summary\ninvocations: {}\nfailures: {}",
                    details
                        .get("invocation_count")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    details
                        .get("failure_count")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                ),
                "journal" => format!(
                    "task_id: {task_id}\nkind: workflow\nstatus: {status}\nview: journal\nhas_more: {}\nrecords: {}",
                    details
                        .get("has_more")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    details
                        .get("journal")
                        .and_then(serde_json::Value::as_array)
                        .map_or(0, Vec::len),
                ),
                "result" => format!(
                    "task_id: {task_id}\nkind: workflow\nstatus: {status}\nview: result\nhas_result: {}",
                    !details.get("result").is_none_or(serde_json::Value::is_null),
                ),
                "artifacts" => format!(
                    "task_id: {task_id}\nkind: workflow\nstatus: {status}\nview: artifacts\ncount: {}\nhas_more: {}",
                    details
                        .get("artifacts")
                        .and_then(serde_json::Value::as_array)
                        .map_or(0, Vec::len),
                    details
                        .get("has_more")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                ),
                "artifact_content" => format!(
                    "task_id: {task_id}\nkind: workflow\nstatus: {status}\nview: artifact_content\noffset: {}\ncontent_bytes: {}\nhas_more: {}",
                    details
                        .pointer("/artifact_content/offset")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    details
                        .pointer("/artifact_content/content_bytes")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    details
                        .get("has_more")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                ),
                _ => return None,
            };
            if let Some(pending) = details.get("pending_user")
                && !pending.is_null()
            {
                let request_id = pending.get("request_id")?.as_str()?;
                let _ = write!(
                    summary,
                    "\npending_request_id: {request_id}\nprompt: {}\nanswer_policy: {}\nanswer_schema: {}",
                    pending.get("prompt")?.as_str()?,
                    pending.get("answer_policy")?.as_str()?,
                    pending.get("answer_schema")?,
                );
                if let Some(default) = pending.get("default").filter(|value| !value.is_null()) {
                    let _ = write!(summary, "\ndefault_answer: {default}");
                }
                let next_action = pending
                    .get("next_action")
                    .and_then(serde_json::Value::as_str)?;
                if next_action == "TaskAnswer" {
                    let _ = write!(
                        summary,
                        "\nnext_action: TaskAnswer(task_id=\"{task_id}\", request_id=\"{request_id}\", answer=<JSON matching answer_schema>)"
                    );
                } else {
                    let _ = write!(summary, "\nnext_action: {next_action}");
                }
            }
            Some(summary)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::workflow_tool_result_summary;

    #[test]
    fn workflow_tool_result_uses_human_summary_not_model_json() {
        let details = json!({
            "ok": true,
            "action": "run_saved",
            "status": "started",
            "task": {"task_id": "workflow_123", "display_name": "Review"}
        });

        let summary = workflow_tool_result_summary("Workflow", &details).expect("summary");

        assert_eq!(
            summary,
            "Workflow 'Review' started as task workflow_123. Completion arrives automatically; use TaskOutput for optional details."
        );
        assert!(!summary.contains("{\"ok\""));
    }

    #[test]
    fn workflow_task_output_result_uses_human_summary_not_model_json() {
        let details = json!({
            "view": "result",
            "run_id": "workflow_123",
            "kind": "workflow",
            "status": "completed",
            "result": {"body": {"inline": {"value": {"ok": true}}}},
            "pending_user": {
                "request_id": "request_1",
                "prompt": "Choose a branch",
                "answer_policy": "human_or_model",
                "answer_schema": {"type": "string"},
                "default": null,
                "next_action": "TaskAnswer"
            }
        });

        let summary = workflow_tool_result_summary("TaskOutput", &details).expect("summary");

        assert_eq!(
            summary,
            "task_id: workflow_123\nkind: workflow\nstatus: completed\nview: result\nhas_result: true\npending_request_id: request_1\nprompt: Choose a branch\nanswer_policy: human_or_model\nanswer_schema: {\"type\":\"string\"}\nnext_action: TaskAnswer(task_id=\"workflow_123\", request_id=\"request_1\", answer=<JSON matching answer_schema>)"
        );
        assert!(!summary.contains("{\"view\""));
    }
}
