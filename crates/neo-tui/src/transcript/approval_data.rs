use neo_agent_core::PermissionOperation;

use crate::transcript::{
    ApprovalPromptData, TranscriptEntry,
};
use crate::transcript::pane::TranscriptPane;

struct ApprovalPromptSummary {
    title: String,
    details: Vec<String>,
    queued_label: String,
    plan_content: Option<String>,
    plan_path: Option<String>,
}

fn approval_prompt(
    operation: PermissionOperation,
    subject: &str,
    arguments: &serde_json::Value,
) -> ApprovalPromptSummary {
    let is_task_stop =
        operation == PermissionOperation::Shell && arguments.get("task_id").is_some();
    let is_terminal = operation == PermissionOperation::Shell && arguments.get("mode").is_some();
    let is_edit = operation == PermissionOperation::FileWrite
        && (arguments.get("old").is_some()
            || arguments.get("new").is_some()
            || arguments.get("replace_all").is_some());

    if is_task_stop {
        ApprovalPromptSummary {
            title: "Stop background task?".to_owned(),
            details: compact_details([
                labeled_argument(arguments, "task_id"),
                labeled_argument(arguments, "reason"),
            ]),
            queued_label: String::new(),
            plan_content: None,
            plan_path: None,
        }
    } else if is_terminal {
        ApprovalPromptSummary {
            title: terminal_approval_title(arguments),
            details: terminal_approval_details(arguments, subject),
            queued_label: String::new(),
            plan_content: None,
            plan_path: None,
        }
    } else if is_edit {
        ApprovalPromptSummary {
            title: "Edit file?".to_owned(),
            details: compact_details([
                labeled_argument(arguments, "path"),
                labeled_argument(arguments, "replace_all"),
            ]),
            queued_label: String::new(),
            plan_content: None,
            plan_path: None,
        }
    } else {
        match operation {
            PermissionOperation::Shell => ApprovalPromptSummary {
                title: "Run this command?".to_owned(),
                details: shell_approval_details(arguments, subject),
                queued_label: String::new(),
                plan_content: None,
                plan_path: None,
            },
            PermissionOperation::FileWrite => ApprovalPromptSummary {
                title: "Write file?".to_owned(),
                details: compact_details([labeled_argument(arguments, "path")]),
                queued_label: String::new(),
                plan_content: None,
                plan_path: None,
            },
            PermissionOperation::FileRead => ApprovalPromptSummary {
                title: "Read workspace data?".to_owned(),
                details: non_empty_details(
                    compact_details([
                        labeled_argument(arguments, "path"),
                        labeled_argument(arguments, "pattern"),
                    ]),
                    || vec![format!("target: {subject}")],
                ),
                queued_label: String::new(),
                plan_content: None,
                plan_path: None,
            },
            PermissionOperation::Tool => ApprovalPromptSummary {
                title: "Run tool?".to_owned(),
                details: compact_details([Some(format!("tool: {subject}"))]),
                queued_label: String::new(),
                plan_content: None,
                plan_path: None,
            },
            PermissionOperation::UserQuestion => ApprovalPromptSummary {
                title: "User question".to_owned(),
                details: compact_details([Some(subject.to_owned())]),
                queued_label: String::new(),
                plan_content: None,
                plan_path: None,
            },
            PermissionOperation::PlanTransition => {
                let plan_content = arguments
                    .get("plan_content")
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_owned);
                let plan_path = arguments
                    .get("plan_path")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                ApprovalPromptSummary {
                    title: "Plan Review".to_owned(),
                    details: compact_details([Some("Ready to build with this plan?".to_owned())]),
                    queued_label: String::new(),
                    plan_content,
                    plan_path,
                }
            }
            PermissionOperation::GoalTransition => ApprovalPromptSummary {
                title: "Goal mode transition".to_owned(),
                details: compact_details([Some(subject.to_owned())]),
                queued_label: String::new(),
                plan_content: None,
                plan_path: None,
            },
        }
    }
}

fn shell_approval_details(arguments: &serde_json::Value, subject: &str) -> Vec<String> {
    let mut details = Vec::new();
    if let Some(cwd) = arguments
        .get("cwd")
        .or_else(|| arguments.get("workdir"))
        .and_then(serde_json::Value::as_str)
    {
        details.push(format!("cwd: {cwd}"));
    }
    let command = arguments
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(subject);
    details.push(format!("$ {command}"));
    details
}

fn terminal_approval_title(arguments: &serde_json::Value) -> String {
    match arguments
        .get("mode")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
    {
        "start" => "Start terminal?".to_owned(),
        "write" => "Write to terminal?".to_owned(),
        "resize" => "Resize terminal?".to_owned(),
        "stop" => "Stop terminal?".to_owned(),
        _ => "Use terminal?".to_owned(),
    }
}

fn terminal_approval_details(arguments: &serde_json::Value, subject: &str) -> Vec<String> {
    let mut details = compact_details([
        labeled_argument(arguments, "mode"),
        labeled_argument(arguments, "handle"),
    ]);
    if let Some(command) = arguments.get("command").and_then(serde_json::Value::as_str) {
        details.push(format!("$ {command}"));
    } else if !subject.is_empty() && details.is_empty() {
        details.push(format!("target: {subject}"));
    }
    details.extend(compact_details([
        labeled_argument(arguments, "input"),
        labeled_argument(arguments, "cols"),
        labeled_argument(arguments, "rows"),
    ]));
    details
}

fn labeled_argument(arguments: &serde_json::Value, key: &str) -> Option<String> {
    let value = arguments.get(key)?;
    match value {
        serde_json::Value::String(value) if !value.is_empty() => Some(format!("{key}: {value}")),
        serde_json::Value::Bool(value) => Some(format!("{key}: {value}")),
        serde_json::Value::Number(value) => Some(format!("{key}: {value}")),
        _ => None,
    }
}

fn compact_details(lines: impl IntoIterator<Item = Option<String>>) -> Vec<String> {
    lines.into_iter().flatten().collect()
}

fn non_empty_details(details: Vec<String>, fallback: impl FnOnce() -> Vec<String>) -> Vec<String> {
    if details.is_empty() {
        fallback()
    } else {
        details
    }
}

impl TranscriptPane {
    pub fn select_approval(&mut self, id: &str, selected: usize, feedback_input: &str) {
        if let Some(approval) = self.transcript.approval_mut(id) {
            approval.selected = selected;
            feedback_input.clone_into(&mut approval.feedback_input);
            self.mark_dirty();
        }
    }

    pub fn resolve_approval(&mut self, id: &str, label: impl Into<String>) {
        if let Some(approval) = self.transcript.approval_mut(id) {
            approval.resolved = Some(label.into());
            approval.queued_count = 0;
            self.advance_queued_approval();
            self.mark_dirty();
        }
    }

    pub fn resolve_unresolved_approvals(&mut self, label: impl Into<String>) {
        let label = label.into();
        let mut changed = false;
        for entry in self.transcript.entries_mut() {
            if let TranscriptEntry::ApprovalPrompt(data) = entry
                && data.resolved.is_none()
            {
                data.resolved = Some(label.clone());
                data.queued_count = 0;
                changed = true;
            }
        }
        if !self.queued_approvals.is_empty() {
            self.queued_approvals.clear();
            changed = true;
        }
        if changed {
            self.mark_dirty();
        }
    }

    pub(super) fn upsert_approval(
        &mut self,
        id: String,
        operation: PermissionOperation,
        subject: &str,
        arguments: &serde_json::Value,
        session_option_label: Option<String>,
        prefix_option_label: Option<String>,
    ) {
        let prompt = approval_prompt(operation, subject, arguments);

        if let Some(approval) = self.transcript.approval_mut(&id) {
            approval.title = prompt.title;
            approval.details = prompt.details;
            approval.queued_label = prompt.queued_label;
            approval.plan_content = prompt.plan_content;
            approval.plan_path = prompt.plan_path;
            approval.queued_count = self.queued_approvals.len();
            approval.resolved = None;
            approval
                .session_option_label
                .clone_from(&session_option_label);
            approval
                .prefix_option_label
                .clone_from(&prefix_option_label);
            return;
        }

        let data = ApprovalPromptData {
            id,
            title: prompt.title,
            details: prompt.details,
            queued_label: prompt.queued_label,
            queued_count: 0,
            selected: 0,
            feedback_input: String::new(),
            resolved: None,
            session_option_label,
            prefix_option_label,
            plan_content: prompt.plan_content,
            plan_path: prompt.plan_path,
        };
        if self.active_approval_mut().is_some() {
            self.queued_approvals.push_back(data);
            self.update_active_approval_queue_count();
            return;
        }

        self.finish_active_text_blocks();
        self.transcript.insert_approval_after_tool_or_push(data);
    }

    fn active_approval_mut(&mut self) -> Option<&mut ApprovalPromptData> {
        self.transcript
            .entries_mut()
            .iter_mut()
            .rev()
            .find_map(|entry| {
                if let TranscriptEntry::ApprovalPrompt(data) = entry
                    && data.resolved.is_none()
                {
                    return Some(data);
                }
                None
            })
    }

    fn update_active_approval_queue_count(&mut self) {
        let queued_count = self.queued_approvals.len();
        if let Some(approval) = self.active_approval_mut() {
            approval.queued_count = queued_count;
            self.mark_dirty();
        }
    }

    fn advance_queued_approval(&mut self) {
        let Some(mut next) = self.queued_approvals.pop_front() else {
            return;
        };
        next.queued_count = self.queued_approvals.len();
        self.transcript.insert_approval_after_tool_or_push(next);
    }
}
