use std::sync::{Arc, Mutex};

/// Records host API calls made by a Lua workflow for inspection and
/// model-facing result reporting. Thread-safe so it can be shared into
/// Lua closures.
#[derive(Debug, Clone, Default)]
pub struct WorkflowHostRecorder {
    calls: Arc<Mutex<Vec<String>>>,
    steps: Arc<Mutex<Vec<super::WorkflowStepRecord>>>,
    reports: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl WorkflowHostRecorder {
    pub fn record(&self, call: impl Into<String>) {
        self.calls
            .lock()
            .expect("workflow recorder poisoned")
            .push(call.into());
    }

    pub fn record_step(&self, name: impl Into<String>, summary: Option<String>) {
        self.record_step_state(name, super::WorkflowState::Completed, summary);
    }

    pub fn record_step_state(
        &self,
        name: impl Into<String>,
        state: super::WorkflowState,
        summary: Option<String>,
    ) {
        let mut steps = self.steps.lock().expect("workflow steps poisoned");
        let index = steps.len();
        steps.push(super::WorkflowStepRecord {
            index,
            name: name.into(),
            state,
            summary,
            details: None,
            agent: None,
            swarm: None,
            has_failures: None,
        });
    }

    pub fn record_report(&self, value: serde_json::Value) {
        self.reports
            .lock()
            .expect("workflow reports poisoned")
            .push(value);
    }

    pub fn push_step(&self, mut step: super::WorkflowStepRecord) {
        let mut steps = self.steps.lock().expect("workflow steps poisoned");
        step.index = steps.len();
        steps.push(step);
    }

    #[must_use]
    pub fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .expect("workflow recorder poisoned")
            .clone()
    }

    #[must_use]
    pub fn steps(&self) -> Vec<super::WorkflowStepRecord> {
        self.steps.lock().expect("workflow steps poisoned").clone()
    }

    #[must_use]
    pub fn reports(&self) -> Vec<serde_json::Value> {
        self.reports
            .lock()
            .expect("workflow reports poisoned")
            .clone()
    }
}
