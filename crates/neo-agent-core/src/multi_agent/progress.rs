use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwarmProgressInput {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub running: usize,
    pub queued: usize,
    pub suspended: usize,
    pub median_completed_duration: Option<Duration>,
    pub longest_running_duration: Duration,
}

/// Estimate swarm progress using a Bayesian-style approach: terminal items
/// contribute their full weight, running items contribute a fractional credit
/// based on elapsed time vs. median completed duration. The estimate never
/// claims completion while items are still active.
#[must_use]
pub fn estimate_swarm_progress(input: SwarmProgressInput) -> f32 {
    if input.total == 0 {
        return 1.0;
    }
    let terminal = input.completed + input.failed;
    if terminal >= input.total {
        return 1.0;
    }

    let base = terminal as f32 / input.total as f32;
    let running_credit = if input.running == 0 {
        0.0
    } else {
        let median = input
            .median_completed_duration
            .unwrap_or_else(|| Duration::from_secs(120));
        let ratio = input.longest_running_duration.as_secs_f32() / median.as_secs_f32().max(1.0);
        ratio.clamp(0.0, 0.85)
    };
    let unfinished_weight = input.running as f32 / input.total as f32;
    (base + running_credit * unfinished_weight).min(0.95)
}
