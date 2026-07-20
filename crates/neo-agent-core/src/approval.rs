use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{PermissionOperation, PrefixApprovalRule, SessionApprovalScope};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanSelection {
    pub label: String,
    pub description: Option<String>,
}

/// Presentation-only projection of a prepared Edit batch. Never carries full
/// original or staged file bodies — only paths, counts, stats, and diffs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct EditApprovalPresentation {
    pub files: usize,
    pub replacements: usize,
    pub added: usize,
    pub removed: usize,
    pub changes: Vec<EditApprovalChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct EditApprovalChange {
    pub path: PathBuf,
    pub replacements: usize,
    pub added: usize,
    pub removed: usize,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalPresentation {
    Command {
        title: String,
        command: String,
        cwd: Option<PathBuf>,
    },
    Tool {
        title: String,
        details: Vec<String>,
    },
    Edit {
        title: String,
        edit: EditApprovalPresentation,
    },
    Plan {
        title: String,
        path: Option<PathBuf>,
        markdown: String,
        summary: Option<String>,
    },
    Goal {
        title: String,
        objective: String,
        completion_criterion: Option<String>,
        phases: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalAction {
    PermitOnce,
    PermitForSession { scope: SessionApprovalScope },
    PermitForPrefix { rule: PrefixApprovalRule },
    Reject,
    ApprovePlan { selection: Option<PlanSelection> },
    RevisePlan { preset_feedback: Option<String> },
    RejectPlan,
    StartGoal,
    ReviseGoal { preset_feedback: Option<String> },
    RejectGoal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalOption {
    pub label: String,
    pub description: Option<String>,
    pub action: ApprovalAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalRequest {
    pub turn: u32,
    pub id: String,
    pub operation: PermissionOperation,
    pub presentation: ApprovalPresentation,
    pub options: Vec<ApprovalOption>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalCancelReason {
    Escape,
    Interrupt,
    SessionEnded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalResponse {
    Selected {
        request_id: String,
        action: ApprovalAction,
        feedback: Option<String>,
    },
    Cancelled {
        request_id: String,
        reason: ApprovalCancelReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalResolution {
    Selected {
        action: ApprovalAction,
        label: String,
        feedback: Option<String>,
    },
    Cancelled {
        reason: ApprovalCancelReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalProtocolError {
    RequestIdMismatch,
    ActionNotOffered,
    FeedbackRequired,
    UnexpectedFeedback,
}

impl ApprovalRequest {
    pub fn validate_response(
        &self,
        response: &ApprovalResponse,
    ) -> Result<ApprovalResolution, ApprovalProtocolError> {
        match response {
            ApprovalResponse::Cancelled { request_id, reason } => {
                if request_id != &self.id {
                    return Err(ApprovalProtocolError::RequestIdMismatch);
                }
                Ok(ApprovalResolution::Cancelled { reason: *reason })
            }
            ApprovalResponse::Selected {
                request_id,
                action,
                feedback,
            } => {
                if request_id != &self.id {
                    return Err(ApprovalProtocolError::RequestIdMismatch);
                }
                let option = self
                    .options
                    .iter()
                    .find(|option| &option.action == action)
                    .ok_or(ApprovalProtocolError::ActionNotOffered)?;
                let revises = matches!(
                    action,
                    ApprovalAction::RevisePlan { .. } | ApprovalAction::ReviseGoal { .. }
                );
                if !revises && feedback.is_some() {
                    return Err(ApprovalProtocolError::UnexpectedFeedback);
                }
                let feedback = feedback
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                if revises && feedback.is_none() {
                    return Err(ApprovalProtocolError::FeedbackRequired);
                }
                Ok(ApprovalResolution::Selected {
                    action: action.clone(),
                    label: option.label.clone(),
                    feedback,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PermissionOperation;

    fn plan_request() -> ApprovalRequest {
        ApprovalRequest {
            turn: 1,
            id: "approval-1".to_owned(),
            operation: PermissionOperation::PlanTransition,
            presentation: ApprovalPresentation::Plan {
                title: "Plan Review".to_owned(),
                path: None,
                markdown: "# Plan".to_owned(),
                summary: None,
            },
            options: vec![
                ApprovalOption {
                    label: "Approve".to_owned(),
                    description: None,
                    action: ApprovalAction::ApprovePlan { selection: None },
                },
                ApprovalOption {
                    label: "Reject with feedback".to_owned(),
                    description: None,
                    action: ApprovalAction::RevisePlan {
                        preset_feedback: None,
                    },
                },
            ],
        }
    }

    #[test]
    fn validate_response_enforces_request_action_and_feedback_contract() {
        let request = plan_request();
        let wrong_request = ApprovalResponse::Selected {
            request_id: "other".to_owned(),
            action: ApprovalAction::ApprovePlan { selection: None },
            feedback: None,
        };
        assert_eq!(
            request.validate_response(&wrong_request),
            Err(ApprovalProtocolError::RequestIdMismatch)
        );

        let unoffered = ApprovalResponse::Selected {
            request_id: request.id.clone(),
            action: ApprovalAction::PermitOnce,
            feedback: None,
        };
        assert_eq!(
            request.validate_response(&unoffered),
            Err(ApprovalProtocolError::ActionNotOffered)
        );

        let blank = ApprovalResponse::Selected {
            request_id: request.id.clone(),
            action: ApprovalAction::RevisePlan {
                preset_feedback: None,
            },
            feedback: Some("  ".to_owned()),
        };
        assert_eq!(
            request.validate_response(&blank),
            Err(ApprovalProtocolError::FeedbackRequired)
        );

        let unexpected = ApprovalResponse::Selected {
            request_id: request.id.clone(),
            action: ApprovalAction::ApprovePlan { selection: None },
            feedback: Some("not allowed".to_owned()),
        };
        assert_eq!(
            request.validate_response(&unexpected),
            Err(ApprovalProtocolError::UnexpectedFeedback)
        );

        let approved = ApprovalResponse::Selected {
            request_id: request.id.clone(),
            action: ApprovalAction::ApprovePlan { selection: None },
            feedback: None,
        };
        assert_eq!(
            request.validate_response(&approved),
            Ok(ApprovalResolution::Selected {
                action: ApprovalAction::ApprovePlan { selection: None },
                label: "Approve".to_owned(),
                feedback: None,
            })
        );
    }
}
