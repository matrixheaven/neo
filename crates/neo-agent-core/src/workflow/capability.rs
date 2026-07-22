use std::sync::Arc;
use tokio::sync::Mutex;

/// Session-scoped one-shot workflow launch capability.
///
/// Only an exact `/workflow` slash-parser action creates a capability.
/// Ordinary text, model inference, Auto/Yolo mode, or AGENTS.md guidance
/// cannot create or forge it.
///
/// The capability is runtime state, never a model-visible token value. It
/// expires when one workflow is durably launched, the user cancels, `/new`
/// resets the session, or the process exits.
#[derive(Clone, Debug, Default)]
pub struct WorkflowCapability {
    inner: Arc<Mutex<WorkflowCapabilityState>>,
}

#[derive(Debug, Default)]
struct WorkflowCapabilityState {
    available: bool,
}

impl WorkflowCapability {
    /// Grant one capability. Replaces any existing one.
    pub async fn grant(&self) {
        self.inner.lock().await.available = true;
    }

    /// Check whether a capability currently exists.
    pub async fn is_available(&self) -> bool {
        self.inner.lock().await.available
    }

    /// Consume the capability (returns true if one was consumed).
    pub async fn consume_if_available(&self) -> bool {
        let mut state = self.inner.lock().await;
        if state.available {
            state.available = false;
            true
        } else {
            false
        }
    }

    /// Revoke the capability (cancel). No-op if none exists.
    pub async fn revoke(&self) {
        self.inner.lock().await.available = false;
    }
}
