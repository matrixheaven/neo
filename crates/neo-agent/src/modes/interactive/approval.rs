//! Extracted: tool-call approval resolution, dispatch, and pending-approval state.

use tokio::sync::oneshot;

use neo_agent_core::PermissionApprovalDecision;
use neo_tui::shell::{ApprovalChoice, ApprovalResult};

use super::InteractiveController;

/// Pending approval response channels, stored per request id until the user
/// resolves the approval overlay.
pub(super) struct PendingApprovalResponse {
    pub(super) decision_tx: oneshot::Sender<PermissionApprovalDecision>,
    pub(super) feedback_tx: Option<oneshot::Sender<Option<String>>>,
    /// Returns the model-supplied plan-review option label the user picked.
    pub(super) selected_label_tx: Option<oneshot::Sender<Option<String>>>,
    /// Display label for the session-approval option, used for the resolved
    /// transcript line.
    pub(super) session_option_label: Option<String>,
    /// Display label for the prefix-approval option.
    pub(super) prefix_option_label: Option<String>,
}

fn approval_result_label(choice: ApprovalChoice) -> &'static str {
    match choice {
        ApprovalChoice::Approve => "Approved",
        ApprovalChoice::AlwaysApprove => "Approved for this session",
        ApprovalChoice::Deny => "Rejected",
        ApprovalChoice::Revise => "Rejected with feedback",
    }
}

/// Build the resolved-transcript label for an `AlwaysApprove` choice from the
/// saved scope/prefix label. The prefix option says "Approve commands starting
/// with X" → resolved shows "Approved commands starting with X". The session
/// option says "Approve this exact command for this session" → resolved shows
/// "Approved this exact command for this session".
fn session_approval_resolved_label(
    choice: ApprovalChoice,
    session_option_label: Option<&str>,
    prefix_option_label: Option<&str>,
    picked_prefix: bool,
) -> String {
    match choice {
        ApprovalChoice::AlwaysApprove => {
            if picked_prefix && let Some(label) = prefix_option_label {
                return label.replacen("Approve", "Approved", 1);
            }
            match session_option_label {
                Some(label) if label.starts_with("Approve ") => {
                    format!("Approved{}", &label["Approve".len()..])
                }
                Some(label) => format!("Approved: {label}"),
                None => approval_result_label(choice).to_owned(),
            }
        }
        other => approval_result_label(other).to_owned(),
    }
}

impl InteractiveController {
    pub(super) fn reject_all_pending_approvals(&mut self) -> bool {
        let chrome_results = self.tui.chrome_mut().cancel_all_approvals();
        let had_pending = !chrome_results.is_empty() || !self.pending_approvals.is_empty();
        for result in chrome_results {
            self.resolve_approval(&result);
        }
        for (request_id, pending) in std::mem::take(&mut self.pending_approvals) {
            self.tui
                .transcript_mut()
                .resolve_approval(&request_id, "Rejected");
            if let Some(tx) = pending.feedback_tx {
                let _ = tx.send(None);
            }
            if let Some(tx) = pending.selected_label_tx {
                let _ = tx.send(None);
            }
            let _ = pending.decision_tx.send(PermissionApprovalDecision::Reject);
        }
        self.tui
            .transcript_mut()
            .resolve_unresolved_approvals("Rejected");
        had_pending
    }

    pub(super) fn resolve_approval(&mut self, result: &ApprovalResult) {
        // Peek the pending labels before dispatch consumes the entry, so the
        // resolved transcript line reflects the exact saved scope (or prefix
        // rule). `picked_prefix` comes from chrome's ApprovalResult, which
        // detects the prefix option by its label.
        let (session_label, prefix_label) =
            self.pending_approvals
                .get(&result.request_id)
                .map_or((None, None), |pending| {
                    (
                        pending.session_option_label.clone(),
                        pending.prefix_option_label.clone(),
                    )
                });
        let label = session_approval_resolved_label(
            result.choice,
            session_label.as_deref(),
            prefix_label.as_deref(),
            result.picked_prefix,
        );
        self.tui
            .transcript_mut()
            .resolve_approval(&result.request_id, label);
        let decision = Self::approval_decision(result);
        let feedback = Self::approval_feedback(result);
        self.push_revision_feedback_status(feedback.as_deref());
        self.dispatch_approval_response(result, decision, feedback);
    }

    fn approval_decision(result: &ApprovalResult) -> PermissionApprovalDecision {
        match result.choice {
            ApprovalChoice::Approve => PermissionApprovalDecision::AllowOnce,
            ApprovalChoice::AlwaysApprove if result.picked_prefix => {
                PermissionApprovalDecision::AllowForPrefix
            }
            ApprovalChoice::AlwaysApprove => PermissionApprovalDecision::AllowForSession,
            ApprovalChoice::Deny | ApprovalChoice::Revise => PermissionApprovalDecision::Reject,
        }
    }

    fn approval_feedback(result: &ApprovalResult) -> Option<String> {
        (result.choice == ApprovalChoice::Revise)
            .then(|| result.feedback.clone())
            .flatten()
    }

    fn push_revision_feedback_status(&mut self, feedback: Option<&str>) {
        if let Some(feedback) = feedback {
            self.push_status(format!("Revision feedback: {feedback}"));
        }
    }

    fn dispatch_approval_response(
        &mut self,
        result: &ApprovalResult,
        decision: PermissionApprovalDecision,
        feedback: Option<String>,
    ) {
        if let Some(pending) = self.pending_approvals.remove(&result.request_id) {
            if let Some(tx) = pending.feedback_tx {
                let _ = tx.send(feedback);
            }
            if let Some(tx) = pending.selected_label_tx {
                let _ = tx.send(result.selected_option_label.clone());
            }
            let _ = pending.decision_tx.send(decision);
        } else {
            self.resolved_approvals
                .insert(result.request_id.clone(), decision);
        }
    }
}
