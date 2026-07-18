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
                    .resolve_approval(&request_id, resolution);
            }
            let _ = pending.response_tx.send(response);
        }
        self.tui
            .transcript_mut()
            .finalize_pending_approvals(ApprovalResolution::Cancelled {
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
                    .resolve_approval(&request_id, resolution);
            }
            let _ = pending.response_tx.send(response);
        }
    }

    /// Atomically register a live approval: store the responder, open chrome,
    /// and upsert the transcript card. Events never open the live modal.
    pub(super) fn register_pending_approval(&mut self, pending: PendingApproval) {
        let request = pending.request.clone();
        let id = request.id.clone();
        self.pending_approvals.insert(id, pending);
        self.tui.chrome_mut().push_approval(request.clone());
        self.tui
            .transcript_mut()
            .apply_agent_event(&neo_agent_core::AgentEvent::ApprovalRequested { request });
    }
}
