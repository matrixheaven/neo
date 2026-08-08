//! Interactive test fixtures: approval requests, options, and scopes (moved from `mod.rs`).

use neo_agent_core::{
    ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResponse,
    FileWriteApprovalOperation, PermissionOperation, PrefixApprovalRule, SessionApprovalKey,
    SessionApprovalScope,
};
use tokio::sync::oneshot;

use super::fixtures_sessions::*;

pub fn ordinary_approval_options(
    session_scope: Option<SessionApprovalScope>,
    prefix_rule: Option<PrefixApprovalRule>,
) -> Vec<ApprovalOption> {
    let mut options = vec![ApprovalOption {
        label: "Approve once".to_owned(),
        description: None,
        action: ApprovalAction::PermitOnce,
    }];
    if let Some(scope) = session_scope.filter(|scope| !scope.is_empty()) {
        options.push(ApprovalOption {
            label: scope.label.clone(),
            description: Some(scope.detail.clone()),
            action: ApprovalAction::PermitForSession { scope },
        });
    }
    if let Some(rule) = prefix_rule {
        options.push(ApprovalOption {
            label: format!("Approve commands starting with {}", rule.label),
            description: None,
            action: ApprovalAction::PermitForPrefix { rule },
        });
    }
    options.push(ApprovalOption {
        label: "Reject".to_owned(),
        description: None,
        action: ApprovalAction::Reject,
    });
    options
}

pub fn ordinary_tool_request(
    id: &str,
    subject: &str,
    path: &str,
    session_scope: Option<SessionApprovalScope>,
) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::Tool,
        presentation: ApprovalPresentation::Tool {
            title: "Run tool?".to_owned(),
            details: vec![format!("tool: {subject}"), format!("path: {path}")],
        },
        options: ordinary_approval_options(session_scope, None),
        workflow_origin: None,
    }
}

pub fn ordinary_shell_request(
    id: &str,
    command: &str,
    session_scope: Option<SessionApprovalScope>,
    prefix_rule: Option<PrefixApprovalRule>,
) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: command.to_owned(),
            cwd: None,
        },
        options: ordinary_approval_options(session_scope, prefix_rule),
        workflow_origin: None,
    }
}

pub fn background_bash_request() -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: "background-bash".to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: "sleep 5".to_owned(),
            cwd: None,
        },
        options: vec![
            ApprovalOption {
                label: "Approve once".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::Reject,
            },
        ],
        workflow_origin: None,
    }
}

pub fn plan_review_request(id: &str) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::PlanTransition,
        presentation: ApprovalPresentation::Plan {
            title: "Plan Review".to_owned(),
            path: None,
            markdown: "Ready to build with this plan?".to_owned(),
            summary: Some("Ready to build with this plan?".to_owned()),
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
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::RejectPlan,
            },
        ],
        workflow_origin: None,
    }
}

pub fn make_pending_approval(
    request: ApprovalRequest,
) -> (
    crate::modes::run::PendingApproval,
    oneshot::Receiver<ApprovalResponse>,
) {
    let (response_tx, response_rx) = oneshot::channel();
    (
        crate::modes::run::PendingApproval {
            request,
            response_tx,
        },
        response_rx,
    )
}

pub fn file_write_session_scope(path: &str) -> SessionApprovalScope {
    SessionApprovalScope {
        keys: vec![SessionApprovalKey::FileWrite {
            workspace: test_workspace_root().display().to_string(),
            path: test_workspace_root().join(path).display().to_string(),
            operation: FileWriteApprovalOperation::Write,
        }],
        label: "Approve writes to this file for this session".to_owned(),
        detail: path.to_owned(),
    }
}

pub fn shell_session_scope(command: &[&str]) -> SessionApprovalScope {
    SessionApprovalScope {
        keys: vec![SessionApprovalKey::Shell {
            workspace: test_workspace_root().display().to_string(),
            cwd: test_workspace_root().display().to_string(),
            command: command.iter().map(|part| (*part).to_owned()).collect(),
        }],
        label: "Approve this exact command for this session".to_owned(),
        detail: test_workspace_root().display().to_string(),
    }
}

pub fn replay_background_bash_request() -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: "background-bash".to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: "sleep 5".to_owned(),
            cwd: None,
        },
        options: vec![
            ApprovalOption {
                label: "Approve once".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::Reject,
            },
        ],
        workflow_origin: None,
    }
}

pub fn replay_workflow_request() -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: "workflow-replay".to_owned(),
        operation: PermissionOperation::WorkflowLaunch,
        presentation: ApprovalPresentation::Workflow {
            title: "Launch workflow?".to_owned(),
            workflow: neo_agent_core::WorkflowApprovalPresentation {
                name: "reviewed".to_owned(),
                description: "A reviewed workflow".to_owned(),
                phases: vec!["work: Do the work".to_owned()],
                args: "{}".to_owned(),
                line_count: 2,
                byte_count: 27,
                source: "neo.phase('work')\nreturn {}".to_owned(),
                warning: "Launch approval authorizes orchestration only.".to_owned(),
            },
        },
        options: vec![ApprovalOption {
            label: "Launch".to_owned(),
            description: None,
            action: ApprovalAction::LaunchWorkflow,
        }],
        workflow_origin: None,
    }
}
