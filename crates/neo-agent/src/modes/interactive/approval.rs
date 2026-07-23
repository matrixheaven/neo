//! Extracted: tool-call approval resolution, dispatch, and pending-approval state.

use neo_agent_core::{ApprovalCancelReason, ApprovalResolution, ApprovalResponse};

use super::InteractiveController;
use crate::modes::run::PendingApproval;

impl InteractiveController {
    /// Cancel every live chrome approval and every registered responder with
    /// [`ApprovalCancelReason::Interrupt`].
    pub(super) fn reject_all_pending_approvals(&mut self) -> bool {
        let chrome_results = self.tui.chrome_mut().cancel_all_approvals();
        let had_pending = !chrome_results.is_empty() || !self.pending_approvals.is_empty();
        for response in chrome_results {
            self.resolve_approval(response);
        }
        for (request_id, pending) in std::mem::take(&mut self.pending_approvals) {
            let response = ApprovalResponse::Cancelled {
                request_id: request_id.clone(),
                reason: ApprovalCancelReason::Interrupt,
            };
            if let Ok(resolution) = pending.request.validate_response(&response) {
                self.tui
                    .transcript_mut()
                    .resolve_approval(&request_id, &resolution);
            }
            let _ = pending.response_tx.send(response);
        }
        self.tui
            .transcript_mut()
            .finalize_pending_approvals(&ApprovalResolution::Cancelled {
                reason: ApprovalCancelReason::Interrupt,
            });
        had_pending
    }

    /// Apply a chrome (or synthetic) response: update transcript, surface revise
    /// feedback, and complete the single responder for this request id.
    pub(super) fn resolve_approval(&mut self, response: ApprovalResponse) {
        let request_id = match &response {
            ApprovalResponse::Selected { request_id, .. }
            | ApprovalResponse::Cancelled { request_id, .. } => request_id.clone(),
        };
        self.workflow_approval_sessions.remove(&request_id);
        if let Some(pending) = self.pending_approvals.remove(&request_id) {
            if let Ok(resolution) = pending.request.validate_response(&response) {
                if let ApprovalResolution::Selected {
                    feedback: Some(feedback),
                    ..
                } = &resolution
                {
                    self.push_status(format!("Revision feedback: {feedback}"));
                }
                self.tui
                    .transcript_mut()
                    .resolve_approval(&request_id, &resolution);
            }
            let _ = pending.response_tx.send(response);
        }
    }

    /// Atomically register a live approval: store the responder, open chrome,
    /// and upsert the transcript card. Events never open the live modal.
    pub(super) fn register_pending_approval(&mut self, pending: PendingApproval) -> bool {
        if pending.response_tx.is_closed() {
            self.resolve_closed_approval_ui(&pending.request.id);
            return false;
        }
        let request = pending.request.clone();
        let id = request.id.clone();
        self.pending_approvals.insert(id, pending);
        self.tui.chrome_mut().push_approval(request.clone());
        self.tui
            .transcript_mut()
            .apply_agent_event(&neo_agent_core::AgentEvent::ApprovalRequested { request });
        true
    }

    pub(super) fn register_workflow_approval(
        &mut self,
        session_id: &str,
        pending: PendingApproval,
    ) -> bool {
        let request_id = pending.request.id.clone();
        if !self.register_pending_approval(pending) {
            return false;
        }
        self.workflow_approval_sessions
            .insert(request_id, session_id.to_owned());
        true
    }

    pub(super) fn park_workflow_approvals_for_session_change(
        &mut self,
        next_session_id: Option<&str>,
    ) {
        let request_ids = self
            .workflow_approval_sessions
            .iter()
            .filter(|(_, session_id)| next_session_id != Some(session_id.as_str()))
            .map(|(request_id, _)| request_id.clone())
            .collect::<Vec<_>>();
        for request_id in request_ids {
            let Some(session_id) = self.workflow_approval_sessions.remove(&request_id) else {
                continue;
            };
            let Some(pending) = self.pending_approvals.remove(&request_id) else {
                continue;
            };
            self.tui.chrome_mut().remove_approval(&request_id);
            self.workflow_approval_backlog
                .entry(session_id)
                .or_default()
                .push_back(pending);
        }
    }

    pub(super) fn activate_workflow_approvals_for_session(&mut self, session_id: &str) {
        let Some(mut pending) = self.workflow_approval_backlog.remove(session_id) else {
            return;
        };
        while let Some(approval) = pending.pop_front() {
            self.register_workflow_approval(session_id, approval);
        }
    }

    pub(super) fn prune_closed_pending_approvals(&mut self) -> bool {
        let mut closed_ids = self
            .pending_approvals
            .iter()
            .filter(|(_, pending)| pending.response_tx.is_closed())
            .map(|(request_id, _)| request_id.clone())
            .collect::<Vec<_>>();
        for pending in self.workflow_approval_backlog.values_mut() {
            pending.retain(|approval| {
                if approval.response_tx.is_closed() {
                    closed_ids.push(approval.request.id.clone());
                    false
                } else {
                    true
                }
            });
        }
        self.workflow_approval_backlog
            .retain(|_, pending| !pending.is_empty());
        if closed_ids.is_empty() {
            return false;
        }
        for request_id in closed_ids {
            self.pending_approvals.remove(&request_id);
            self.workflow_approval_sessions.remove(&request_id);
            self.resolve_closed_approval_ui(&request_id);
        }
        true
    }

    pub(super) fn drop_all_workflow_approvals(&mut self) {
        let request_ids = self
            .workflow_approval_sessions
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for request_id in request_ids {
            self.pending_approvals.remove(&request_id);
            self.tui.chrome_mut().remove_approval(&request_id);
        }
        self.workflow_approval_sessions.clear();
        self.workflow_approval_backlog.clear();
    }

    pub(super) fn resolve_closed_approval_ui(&mut self, request_id: &str) {
        self.tui.chrome_mut().remove_approval(request_id);
        self.tui.transcript_mut().resolve_approval(
            request_id,
            &ApprovalResolution::Cancelled {
                reason: ApprovalCancelReason::Interrupt,
            },
        );
    }
}
