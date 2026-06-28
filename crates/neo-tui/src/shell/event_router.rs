use neo_agent_core::{AgentEvent, PermissionOperation};

use crate::primitive::theme::{ChromeMode, DevelopmentMode, GoalModeStatus};
use crate::widgets::{TodoDisplayItem, TodoDisplayStatus};
use crate::dialogs::{QuestionDisplayData, QuestionDisplayOption};

use super::approval::ApprovalRequestModal;
use super::state::NeoChromeState;
use super::stream::StreamUpdate;

impl NeoChromeState {
    pub fn apply_stream_update(&mut self, update: StreamUpdate) {
        match update {
            StreamUpdate::AssistantStarted { .. }
            | StreamUpdate::TextDelta { .. }
            | StreamUpdate::ToolStarted { .. }
            | StreamUpdate::ToolUpdated { .. }
            | StreamUpdate::ToolFinished { .. }
            | StreamUpdate::ThinkingStarted
            | StreamUpdate::ThinkingDelta { .. } => {
                self.mode = ChromeMode::Streaming;
            }
            StreamUpdate::Error { text } => {
                let _ = text;
                self.mode = self.overlay_mode();
            }
            StreamUpdate::TurnFinished | StreamUpdate::RunFinished { .. } => {
                self.mode = self.overlay_mode();
            }
            StreamUpdate::PlanModeChanged { active } => self.set_plan_mode(active),
            StreamUpdate::TodoUpdated { todos } => {
                self.todo_items = todos;
            }
            StreamUpdate::QuestionRequested { id, questions } => {
                self.push_question_overlay(id, questions);
            }
            StreamUpdate::ThinkingFinished | StreamUpdate::SkillActivated { .. } => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn apply_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::MessageStarted { .. }
            | AgentEvent::TextDelta { .. }
            | AgentEvent::ThinkingStarted { .. }
            | AgentEvent::ThinkingDelta { .. }
            | AgentEvent::ThinkingFinished { .. }
            | AgentEvent::ToolCallStarted { .. }
            | AgentEvent::ToolCallArgumentsDelta { .. }
            | AgentEvent::ToolCallFinished { .. }
            | AgentEvent::ToolExecutionStarted { .. }
            | AgentEvent::ToolExecutionUpdate { .. }
            | AgentEvent::ToolExecutionFinished { .. }
            | AgentEvent::ShellCommandStarted { .. }
            | AgentEvent::ShellCommandFinished { .. } => {
                self.mode = ChromeMode::Streaming;
            }
            AgentEvent::ApprovalRequested {
                id,
                operation,
                subject,
                arguments,
                session_scope,
                prefix_rule,
                ..
            } => {
                let is_review = matches!(
                    operation,
                    PermissionOperation::PlanTransition | PermissionOperation::GoalTransition
                );
                let body = if arguments.is_null() {
                    subject
                } else {
                    format!("{subject}\n{arguments}")
                };
                // Derive the dynamic option labels. Review transitions and
                // scope-less prompts omit both; prefix is offered only when the
                // runtime proposed a persistent rule.
                let mut session_label = if is_review {
                    None
                } else {
                    session_scope
                        .as_ref()
                        .filter(|scope| !scope.is_empty())
                        .map(|scope| scope.label.clone())
                };
                // Tool and shell approvals always offer a session-approval
                // option, even when no explicit session scope was derived.
                // Use the default label so the modal keeps its four-option
                // layout, matching the transcript pane.
                if session_label.is_none()
                    && matches!(
                        operation,
                        PermissionOperation::Tool | PermissionOperation::Shell
                    )
                {
                    session_label = Some("Approve for this session".to_owned());
                }
                let prefix_label = if is_review {
                    None
                } else {
                    prefix_rule
                        .as_ref()
                        .map(|rule| format!("Approve commands starting with {}", rule.label))
                };
                self.pending_approvals.push_back(
                    if operation == PermissionOperation::PlanTransition {
                        // ExitPlanMode carries `{plan_summary, options: [{label, description}]}`.
                        // Surface the model-supplied options as a real picker (mirrors
                        // kimi-code) instead of dumping the raw JSON into the body.
                        let (option_labels, options_body) = crate::primitive::theme::plan_review_options(&arguments);
                        let body = match arguments.get("plan_summary").and_then(|v| v.as_str()) {
                            Some(summary) if !summary.trim().is_empty() => {
                                if options_body.is_empty() {
                                    summary.to_owned()
                                } else {
                                    format!("{summary}\n\n{options_body}")
                                }
                            }
                            _ => options_body,
                        };
                        ApprovalRequestModal::new_plan_review(
                            id,
                            crate::primitive::theme::review_title(operation),
                            body,
                            option_labels,
                        )
                    } else if is_review {
                        ApprovalRequestModal::new_review(id, crate::primitive::theme::review_title(operation), body)
                    } else {
                        ApprovalRequestModal::new_with_options(
                            id,
                            format!("{operation:?} approval"),
                            body,
                            session_label,
                            prefix_label,
                        )
                    },
                );
                self.focused_overlay = None;
                self.mode = ChromeMode::Approval;
            }
            AgentEvent::ContextWindowUpdated { used_tokens, .. } => {
                if let Some(context_window) = &mut self.context_window {
                    *context_window = context_window.with_used_tokens(used_tokens);
                }
            }
            AgentEvent::TurnFinished { .. } => {
                self.apply_stream_update(StreamUpdate::TurnFinished);
            }
            AgentEvent::Error { message, .. } => {
                self.apply_stream_update(StreamUpdate::Error { text: message });
            }
            AgentEvent::RunFinished { turn, stop_reason } => {
                self.apply_stream_update(StreamUpdate::RunFinished { turn, stop_reason });
            }
            AgentEvent::SteeringQueued { message } => {
                self.pending_input.queue_steer(message.text());
            }
            AgentEvent::FollowUpQueued { message } => {
                self.pending_input.queue_follow_up(message.text());
            }
            AgentEvent::QueueDrained { kind, count } => {
                self.pending_input.drain(kind, count);
            }
            AgentEvent::CompactionStarted { .. }
            | AgentEvent::CompactionProgress { .. }
            | AgentEvent::CompactionApplied { .. }
            | AgentEvent::MessageAppended { .. }
            | AgentEvent::RunStarted { .. }
            | AgentEvent::TurnStarted { .. }
            | AgentEvent::MessageFinished { .. }
            | AgentEvent::TokenUsage { .. }
            | AgentEvent::TerminalSessionStarted { .. }
            | AgentEvent::TerminalSessionOutput { .. }
            | AgentEvent::TerminalSessionFinished { .. }
            | AgentEvent::SkillActivated { .. } => {}
            AgentEvent::GoalStarted { .. } | AgentEvent::GoalResumed { .. } => {
                self.set_development_mode(DevelopmentMode::Goal(GoalModeStatus::Active));
            }
            AgentEvent::GoalPaused { .. } => {
                self.set_development_mode(DevelopmentMode::Goal(GoalModeStatus::Paused));
            }
            AgentEvent::GoalBlocked { .. } => {
                self.set_development_mode(DevelopmentMode::Goal(GoalModeStatus::Blocked));
            }
            AgentEvent::GoalFinished { .. } => {
                self.set_development_mode(DevelopmentMode::Normal);
            }
            AgentEvent::PlanModeEntered { .. } => self.set_plan_mode(true),
            AgentEvent::PlanModeExited { .. } => {
                self.set_plan_mode(false);
            }
            AgentEvent::PlanUpdated { enabled, .. } => self.set_plan_mode(enabled),
            AgentEvent::TodoUpdated { todos, .. } => {
                let display: Vec<TodoDisplayItem> = todos
                    .iter()
                    .map(|t| TodoDisplayItem {
                        title: t.title.clone(),
                        status: match t.status.as_str() {
                            "in_progress" => TodoDisplayStatus::InProgress,
                            "done" => TodoDisplayStatus::Done,
                            _ => TodoDisplayStatus::Pending,
                        },
                    })
                    .collect();
                self.todo_items = display;
            }
            AgentEvent::QuestionRequested { id, questions, .. } => {
                let display: Vec<QuestionDisplayData> = questions
                    .iter()
                    .map(|q| QuestionDisplayData {
                        question: q.question.clone(),
                        header: q.header.clone(),
                        body: q.body.clone(),
                        options: q
                            .options
                            .iter()
                            .map(|o| QuestionDisplayOption {
                                label: o.label.clone(),
                                description: o.description.clone(),
                            })
                            .collect(),
                        multi_select: q.multi_select,
                    })
                    .collect();
                self.push_question_overlay(id, display);
            }
        }
    }
}
