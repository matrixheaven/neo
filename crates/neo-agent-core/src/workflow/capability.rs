use std::sync::{Arc, Mutex, MutexGuard};

use uuid::Uuid;

use super::error::{WorkflowError, WorkflowErrorCode};

/// Session-scoped workflow launch capability.
///
/// State machine (design §13):
/// ```text
/// Unavailable
/// -> Available { generation, launch_nonce }
/// -> Bound { generation, launch_nonce, intent_digest }
/// -> Consumed | Revoked  (both map to Unavailable)
/// ```
///
/// Only an exact `/workflow` slash-parser action creates a capability via
/// [`Self::grant`]. Ordinary text, model inference, Auto/Yolo mode, or
/// AGENTS.md guidance cannot create or forge it.
///
/// Untyped [`Self::reserve`] remains for linked-run paths that hold a
/// generation-scoped reservation until durable create commits.
#[derive(Clone, Debug, Default)]
pub struct WorkflowCapability {
    inner: Arc<Mutex<WorkflowCapabilityState>>,
}

#[derive(Debug, Default)]
struct WorkflowCapabilityState {
    generation: u64,
    launch_nonce: Option<String>,
    status: WorkflowCapabilityStatus,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
enum WorkflowCapabilityStatus {
    #[default]
    Unavailable,
    Available,
    /// Generation-scoped lock used by linked-run create (RAII rollback on drop).
    Reserved,
    /// Exact intent binding; survives create failure for same-intent retry.
    Bound {
        intent_digest: String,
    },
}

/// An in-process untyped reservation. Its generation is never serialized or
/// exposed to the model/Lua, and dropping it rolls the capability back to
/// [`WorkflowCapabilityStatus::Available`].
#[derive(Debug)]
pub struct WorkflowCapabilityReservation {
    capability: WorkflowCapability,
    generation: u64,
    active: bool,
}

impl WorkflowCapability {
    fn state(&self) -> MutexGuard<'_, WorkflowCapabilityState> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Grant one capability. Replaces any unreserved capability with a fresh
    /// generation and launch nonce.
    pub fn grant(&self) {
        let mut state = self.state();
        state.generation = state.generation.wrapping_add(1).max(1);
        state.launch_nonce = Some(Uuid::new_v4().as_simple().to_string());
        state.status = WorkflowCapabilityStatus::Available;
    }

    /// Current launch nonce when a capability exists (Available / Bound / Reserved).
    #[must_use]
    pub fn launch_nonce(&self) -> Option<String> {
        let state = self.state();
        match state.status {
            WorkflowCapabilityStatus::Unavailable => None,
            _ => state.launch_nonce.clone(),
        }
    }

    /// Bound intent digest when status is Bound.
    #[must_use]
    pub fn bound_digest(&self) -> Option<String> {
        let state = self.state();
        match &state.status {
            WorkflowCapabilityStatus::Bound { intent_digest } => Some(intent_digest.clone()),
            _ => None,
        }
    }

    /// Check whether a capability currently exists and is not fully consumed.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.inspect()
    }

    /// Synchronous authorization-time inspection. This never reserves,
    /// binds, or consumes the capability.
    #[must_use]
    pub fn inspect(&self) -> bool {
        !matches!(self.state().status, WorkflowCapabilityStatus::Unavailable)
    }

    /// True only when the capability is Available (not Bound or Reserved).
    #[must_use]
    pub fn is_unbound(&self) -> bool {
        matches!(self.state().status, WorkflowCapabilityStatus::Available)
    }

    /// Bind the available (or already-matching Bound) capability to an exact
    /// intent digest. Mismatch leaves the valid Bound state unchanged.
    pub fn bind(&self, intent_digest: &str) -> Result<(), WorkflowError> {
        let mut state = self.state();
        match &state.status {
            WorkflowCapabilityStatus::Available => {
                state.status = WorkflowCapabilityStatus::Bound {
                    intent_digest: intent_digest.to_owned(),
                };
                Ok(())
            }
            WorkflowCapabilityStatus::Bound {
                intent_digest: existing,
            } if existing == intent_digest => Ok(()),
            WorkflowCapabilityStatus::Bound { .. } => Err(WorkflowError::coded(
                WorkflowErrorCode::LaunchAuthorizationMismatch,
                "launch authorization is bound to a different intent",
            )),
            WorkflowCapabilityStatus::Reserved => Err(WorkflowError::coded(
                WorkflowErrorCode::LaunchAuthorizationMismatch,
                "launch authorization is reserved for another launch",
            )),
            WorkflowCapabilityStatus::Unavailable => Err(WorkflowError::coded(
                WorkflowErrorCode::LaunchAuthorizationMissing,
                "RunWorkflow requires a launch capability. Use the exact /workflow slash command first.",
            )),
        }
    }

    /// Consume a Bound capability after durable create and task registration.
    /// Returns false without changing state when the digest does not match.
    #[must_use]
    pub fn consume_bound(&self, intent_digest: &str) -> bool {
        let mut state = self.state();
        match &state.status {
            WorkflowCapabilityStatus::Bound {
                intent_digest: existing,
            } if existing == intent_digest => {
                state.generation = state.generation.wrapping_add(1).max(1);
                state.launch_nonce = None;
                state.status = WorkflowCapabilityStatus::Unavailable;
                true
            }
            _ => false,
        }
    }

    /// Discard a Bound binding and return to Available with the same
    /// generation and nonce (Ask Revise path).
    pub fn unbind(&self) {
        let mut state = self.state();
        if matches!(state.status, WorkflowCapabilityStatus::Bound { .. }) {
            state.status = WorkflowCapabilityStatus::Available;
        }
    }

    /// Reserve the one available capability for an atomic durable linked-run
    /// launch (generation-scoped; drop rolls back).
    #[must_use]
    pub fn reserve(&self) -> Option<WorkflowCapabilityReservation> {
        let mut state = self.state();
        if !matches!(state.status, WorkflowCapabilityStatus::Available) {
            return None;
        }
        let generation = state.generation;
        state.status = WorkflowCapabilityStatus::Reserved;
        Some(WorkflowCapabilityReservation {
            capability: self.clone(),
            generation,
            active: true,
        })
    }

    /// Revoke the capability. No-op if none exists.
    pub fn revoke(&self) {
        self.revoke_now();
    }

    /// Synchronous cancellation path used by typed approval resolution.
    pub fn revoke_now(&self) {
        let mut state = self.state();
        state.generation = state.generation.wrapping_add(1).max(1);
        state.launch_nonce = None;
        state.status = WorkflowCapabilityStatus::Unavailable;
    }
}

impl WorkflowCapabilityReservation {
    /// Consume the reserved capability after durable create succeeds.
    #[must_use]
    pub fn commit(mut self) -> bool {
        let mut state = self.capability.state();
        let committed = matches!(
            state.status,
            WorkflowCapabilityStatus::Reserved if state.generation == self.generation
        );
        if committed {
            state.generation = state.generation.wrapping_add(1).max(1);
            state.launch_nonce = None;
            state.status = WorkflowCapabilityStatus::Unavailable;
        }
        drop(state);
        self.active = !committed;
        committed
    }

    fn rollback(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self.capability.state();
        if matches!(
            state.status,
            WorkflowCapabilityStatus::Reserved if state.generation == self.generation
        ) {
            state.status = WorkflowCapabilityStatus::Available;
        }
        self.active = false;
    }
}

impl Drop for WorkflowCapabilityReservation {
    fn drop(&mut self) {
        self.rollback();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reservation_commit_is_one_shot_and_drop_rolls_back() {
        let capability = WorkflowCapability::default();
        capability.grant();
        let reservation = capability.reserve().expect("reserve");
        assert!(!capability.is_unbound());
        assert!(capability.inspect());
        drop(reservation);
        assert!(capability.is_unbound());
        assert!(capability.inspect());

        assert!(capability.reserve().expect("reserve again").commit());
        assert!(!capability.inspect());
    }

    #[test]
    fn bind_is_exact_and_mismatch_preserves_prior_binding() {
        let capability = WorkflowCapability::default();
        capability.grant();
        assert!(capability.launch_nonce().is_some());
        capability.bind("digest-a").expect("bind a");
        assert_eq!(capability.bound_digest().as_deref(), Some("digest-a"));
        let err = capability.bind("digest-b").expect_err("mismatch");
        assert_eq!(err.code(), WorkflowErrorCode::LaunchAuthorizationMismatch);
        assert_eq!(capability.bound_digest().as_deref(), Some("digest-a"));
        assert!(capability.consume_bound("digest-a"));
        assert!(!capability.inspect());
    }

    #[test]
    fn unbind_returns_to_available_with_same_nonce() {
        let capability = WorkflowCapability::default();
        capability.grant();
        let nonce = capability.launch_nonce();
        capability.bind("digest-a").unwrap();
        capability.unbind();
        assert!(capability.is_unbound());
        assert_eq!(capability.launch_nonce(), nonce);
    }
}
