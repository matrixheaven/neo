use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SwarmItemState {
    Queued,
    Running,
    SuspendedRateLimit,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct SwarmSchedulerConfig {
    pub max_concurrency: usize,
    pub retry_base_delay: Duration,
    pub provider_quiet_window: Duration,
}

impl Default for SwarmSchedulerConfig {
    #[allow(clippy::duration_suboptimal_units)]
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            retry_base_delay: Duration::from_secs(3),
            provider_quiet_window: Duration::from_secs(180),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SwarmRetryState {
    pub attempts: usize,
    pub retry_after: Instant,
}

#[derive(Debug, Clone)]
pub struct SwarmScheduler {
    config: SwarmSchedulerConfig,
    effective_concurrency: usize,
}

impl SwarmScheduler {
    #[must_use]
    pub fn new(config: SwarmSchedulerConfig) -> Self {
        let effective_concurrency = config.max_concurrency;
        Self {
            config,
            effective_concurrency,
        }
    }

    #[must_use]
    pub fn effective_concurrency(&self) -> usize {
        self.effective_concurrency
    }

    pub fn record_rate_limit(&mut self) {
        self.effective_concurrency = self.effective_concurrency.saturating_sub(1).max(1);
    }

    pub fn record_recovery(&mut self) {
        let max = self.config.max_concurrency;
        self.effective_concurrency = (self.effective_concurrency + 1).min(max);
    }

    #[must_use]
    pub fn retry_delay(&self, attempts: usize) -> Duration {
        self.config.retry_base_delay * (1_u32 << attempts.min(5))
    }
}
