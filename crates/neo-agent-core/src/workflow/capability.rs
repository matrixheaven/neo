use std::sync::{Arc, Mutex, MutexGuard};

/// Session-scoped one-shot workflow launch capability.
///
/// Only an exact `/workflow` slash-parser action creates a capability.
/// Ordinary text, model inference, Auto/Yolo mode, or AGENTS.md guidance
/// cannot create or forge it.
#[derive(Clone, Debug, Default)]
pub struct WorkflowCapability {
    inner: Arc<Mutex<WorkflowCapabilityState>>,
}

#[derive(Debug, Default)]
struct WorkflowCapabilityState {
    generation: u64,
    status: WorkflowCapabilityStatus,
}

#[derive(Debug, Default)]
enum WorkflowCapabilityStatus {
    #[default]
    Unavailable,
    Available,
    Reserved(u64),
}

/// An in-process reservation. Its generation is never serialized or exposed
/// to the model/Lua, and dropping it rolls the capability back.
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
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Grant one capability. Replaces any unreserved capability.
    pub async fn grant(&self) {
        let mut state = self.state();
        state.generation = state.generation.wrapping_add(1).max(1);
        state.status = WorkflowCapabilityStatus::Available;
    }

    /// Check whether a capability currently exists and is not reserved.
    pub async fn is_available(&self) -> bool {
        self.inspect()
    }

    /// Synchronous authorization-time inspection. This never reserves or
    /// consumes the capability.
    #[must_use]
    pub fn inspect(&self) -> bool {
        matches!(self.state().status, WorkflowCapabilityStatus::Available)
    }

    /// Reserve the one available capability for an atomic durable launch.
    pub fn reserve(&self) -> Option<WorkflowCapabilityReservation> {
        let mut state = self.state();
        if !matches!(state.status, WorkflowCapabilityStatus::Available) {
            return None;
        }
        let generation = state.generation;
        state.status = WorkflowCapabilityStatus::Reserved(generation);
        Some(WorkflowCapabilityReservation {
            capability: self.clone(),
            generation,
            active: true,
        })
    }

    /// Revoke the capability. No-op if none exists.
    pub async fn revoke(&self) {
        self.revoke_now();
    }

    /// Synchronous cancellation path used by typed approval resolution.
    pub fn revoke_now(&self) {
        let mut state = self.state();
        state.generation = state.generation.wrapping_add(1).max(1);
        state.status = WorkflowCapabilityStatus::Unavailable;
    }
}

impl WorkflowCapabilityReservation {
    /// Consume the reserved capability after durable create and task
    /// registration both succeed.
    pub fn commit(mut self) -> bool {
        let mut state = self.capability.state();
        let committed = if matches!(state.status, WorkflowCapabilityStatus::Reserved(generation) if generation == self.generation)
        {
            state.status = WorkflowCapabilityStatus::Unavailable;
            true
        } else {
            false
        };
        drop(state);
        self.active = !committed;
        committed
    }

    fn rollback(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self.capability.state();
        if matches!(state.status, WorkflowCapabilityStatus::Reserved(generation) if generation == self.generation)
        {
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
        capability.grant().await;
        let reservation = capability.reserve().expect("reserve");
        assert!(!capability.inspect());
        drop(reservation);
        assert!(capability.inspect());

        assert!(capability.reserve().expect("reserve again").commit());
        assert!(!capability.inspect());
    }
}
