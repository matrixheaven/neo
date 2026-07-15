use neo_agent_core::PermissionOperation;
use neo_agent_core::PlanSuggestion;

use crate::transcript::pane::TranscriptPane;
use crate::transcript::{ApprovalPromptData, TranscriptEntry};

struct ApprovalPromptSummary {
    title: String,
    details: Vec<String>,
    queued_label: String,
    plan_content: Option<String>,
    plan_path: Option<String>,
    plan_option_labels: Vec<String>,
    suggestions: Vec<PlanSuggestion>,
}

#[allow(clippy::too_many_lines)]
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
            plan_option_labels: Vec::new(),
            suggestions: Vec::new(),
        }
    } else if is_terminal {
        ApprovalPromptSummary {
            title: terminal_approval_title(arguments),
            details: terminal_approval_details(arguments, subject),
            queued_label: String::new(),
            plan_content: None,
            plan_path: None,
            plan_option_labels: Vec::new(),
            suggestions: Vec::new(),
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
            plan_option_labels: Vec::new(),
            suggestions: Vec::new(),
        }
    } else {
        match operation {
            PermissionOperation::Shell => ApprovalPromptSummary {
                title: "Run this command?".to_owned(),
                details: shell_approval_details(arguments, subject),
                queued_label: String::new(),
                plan_content: None,
                plan_path: None,
                plan_option_labels: Vec::new(),
                suggestions: Vec::new(),
            },
            PermissionOperation::FileWrite => ApprovalPromptSummary {
                title: "Write file?".to_owned(),
                details: compact_details([labeled_argument(arguments, "path")]),
                queued_label: String::new(),
                plan_content: None,
                plan_path: None,
                plan_option_labels: Vec::new(),
                suggestions: Vec::new(),
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
                plan_option_labels: Vec::new(),
                suggestions: Vec::new(),
            },
            PermissionOperation::Tool => ApprovalPromptSummary {
                title: "Run tool?".to_owned(),
                details: compact_details([Some(format!("tool: {subject}"))]),
                queued_label: String::new(),
                plan_content: None,
                plan_path: None,
                plan_option_labels: Vec::new(),
                suggestions: Vec::new(),
            },
            PermissionOperation::UserQuestion => ApprovalPromptSummary {
                title: "User question".to_owned(),
                details: compact_details([Some(subject.to_owned())]),
                queued_label: String::new(),
                plan_content: None,
                plan_path: None,
                plan_option_labels: Vec::new(),
                suggestions: Vec::new(),
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
                // Extract model-supplied option labels so the transcript
                // renders the same option list as the chrome. Without this
                // the transcript hardcoded a single "Approve once" while the
                // chrome had N custom options, causing the selected index to
                // be off by N-1 — selecting "Reject with feedback" in the
                // UI actually selected "Reject" in the chrome.
                let plan_option_labels = arguments
                    .get("options")
                    .and_then(serde_json::Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.get("label")?.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let suggestions = arguments
                    .get("suggestions")
                    .and_then(serde_json::Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| {
                                let label = item.get("label")?.as_str()?.to_owned();
                                let description = item
                                    .get("description")
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or(&label)
                                    .to_owned();
                                let feedback = item
                                    .get("feedback")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_owned)
                                    .or_else(|| Some(description.clone()));
                                Some(PlanSuggestion {
                                    label,
                                    description,
                                    feedback,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                ApprovalPromptSummary {
                    title: "Plan Review".to_owned(),
                    details: compact_details([Some("Ready to build with this plan?".to_owned())]),
                    queued_label: String::new(),
                    plan_content,
                    plan_path,
                    plan_option_labels,
                    suggestions,
                }
            }
            PermissionOperation::GoalTransition => ApprovalPromptSummary {
                title: "Goal mode transition".to_owned(),
                details: compact_details([Some(subject.to_owned())]),
                queued_label: String::new(),
                plan_content: None,
                plan_path: None,
                plan_option_labels: Vec::new(),
                suggestions: Vec::new(),
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
    pub fn select_approval(
        &mut self,
        id: &str,
        selected: usize,
        feedback_input: &str,
        selected_suggestion: Option<usize>,
        feedback_active: bool,
    ) {
        let changed = self.transcript.mutate_approval(id, |approval| {
            let changed = approval.selected != selected
                || approval.selected_suggestion != selected_suggestion
                || approval.feedback_active != feedback_active
                || approval.feedback_input != feedback_input;
            if !changed {
                return false;
            }
            approval.selected = selected;
            approval.selected_suggestion = selected_suggestion;
            approval.feedback_active = feedback_active;
            feedback_input.clone_into(&mut approval.feedback_input);
            true
        });
        if changed {
            self.mark_dirty();
        }
    }

    pub fn resolve_approval(&mut self, id: &str, label: impl Into<String>) {
        let label = label.into();
        let changed = self.transcript.mutate_approval(id, |approval| {
            if approval.resolved.as_deref() == Some(label.as_str()) && approval.queued_count == 0 {
                return false;
            }
            approval.resolved = Some(label);
            approval.queued_count = 0;
            true
        });
        if changed {
            self.advance_queued_approval();
            self.mark_dirty();
        }
    }

    pub fn resolve_unresolved_approvals(&mut self, label: impl Into<String>) {
        let label = label.into();
        let mut changed = false;
        for index in 0..self.transcript.entries().len() {
            let is_unresolved = matches!(
                &self.transcript.entries()[index],
                TranscriptEntry::ApprovalPrompt(data) if data.resolved.is_none()
            );
            if !is_unresolved {
                continue;
            }
            changed |= self.transcript.mutate_entry(index, |entry| {
                let TranscriptEntry::ApprovalPrompt(data) = entry else {
                    return false;
                };
                data.resolved = Some(label.clone());
                data.queued_count = 0;
                true
            });
        }
        if !self.queued_approvals.is_empty() {
            self.queued_approvals.clear();
            changed = true;
        }
        if changed {
            self.mark_dirty();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn upsert_approval(
        &mut self,
        id: String,
        operation: PermissionOperation,
        subject: &str,
        arguments: &serde_json::Value,
        session_option_label: Option<String>,
        prefix_option_label: Option<String>,
        suggestions: Vec<PlanSuggestion>,
    ) {
        let mut prompt = approval_prompt(operation, subject, arguments);
        if !suggestions.is_empty() {
            prompt.suggestions = suggestions;
        }

        if self
            .transcript
            .approval(&id)
            .is_some_and(|approval| approval.resolved.is_some())
        {
            return;
        }
        if self.transcript.approval(&id).is_some() {
            let queued_count = self.queued_approvals.len();
            self.transcript.mutate_approval(&id, |approval| {
                let changed = approval.title != prompt.title
                    || approval.details != prompt.details
                    || approval.queued_label != prompt.queued_label
                    || approval.plan_content != prompt.plan_content
                    || approval.plan_path != prompt.plan_path
                    || approval.suggestions != prompt.suggestions
                    || approval.plan_option_labels != prompt.plan_option_labels
                    || approval.selected_suggestion.is_some()
                    || approval.queued_count != queued_count
                    || approval.resolved.is_some()
                    || approval.session_option_label != session_option_label
                    || approval.prefix_option_label != prefix_option_label;
                if !changed {
                    return false;
                }
                approval.title = prompt.title;
                approval.details = prompt.details;
                approval.queued_label = prompt.queued_label;
                approval.plan_content = prompt.plan_content;
                approval.plan_path = prompt.plan_path;
                approval.suggestions = prompt.suggestions;
                approval.plan_option_labels = prompt.plan_option_labels;
                approval.selected_suggestion = None;
                approval.queued_count = queued_count;
                approval.resolved = None;
                approval.session_option_label = session_option_label;
                approval.prefix_option_label = prefix_option_label;
                true
            });
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
            feedback_active: false,
            resolved: None,
            session_option_label,
            prefix_option_label,
            plan_content: prompt.plan_content,
            plan_path: prompt.plan_path,
            plan_option_labels: prompt.plan_option_labels,
            suggestions: prompt.suggestions,
            selected_suggestion: None,
        };
        if self.active_approval_index().is_some() {
            self.queued_approvals.push_back(data);
            self.update_active_approval_queue_count();
            return;
        }

        self.finish_active_text_blocks();
        self.transcript.insert_approval_after_tool_or_push(data);
    }

    fn active_approval_index(&self) -> Option<usize> {
        self.transcript.entries().iter().rposition(|entry| {
            matches!(
                entry,
                TranscriptEntry::ApprovalPrompt(data) if data.resolved.is_none()
            )
        })
    }

    fn update_active_approval_queue_count(&mut self) {
        let queued_count = self.queued_approvals.len();
        let Some(index) = self.active_approval_index() else {
            return;
        };
        if self.transcript.mutate_entry(index, |entry| {
            let TranscriptEntry::ApprovalPrompt(approval) = entry else {
                return false;
            };
            if approval.queued_count == queued_count {
                return false;
            }
            approval.queued_count = queued_count;
            true
        }) {
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
