// Cast warnings intentionally suppressed: progress/weight math mixes `usize`
// counts and elapsed milliseconds with `f32` ratios, which is the entire point
// of the estimator.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Standalone estimator (stateless, used by scheduler / tests)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct SwarmProgressInput {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub running: usize,
    pub queued: usize,
    pub suspended: usize,
    pub median_completed_duration: Option<Duration>,
    /// Elapsed duration of each individual running agent.  The old API used a
    /// single `longest_running_duration` which over-credited freshly started
    /// agents; per-agent durations let the aggregate respect each agent's own
    /// elapsed time.
    pub running_durations: Vec<Duration>,
}

/// Estimate swarm progress using a Bayesian-style approach: terminal items
/// contribute their full weight, running items contribute a fractional credit
/// based on elapsed time vs. median completed duration (log-normal CDF), and
/// the aggregate is confidence-weighted. The estimate never claims completion
/// while items are still active.
#[must_use]
pub fn estimate_swarm_progress(input: &SwarmProgressInput) -> f32 {
    if input.total == 0 {
        return 1.0;
    }
    let terminal = input.completed + input.failed;
    if terminal >= input.total {
        return 1.0;
    }

    let cfg = SwarmEstimatorConfig::default();

    // Prior median: observed median completed duration, or the conservative
    // cold-start default.
    let prior_median_ms = input
        .median_completed_duration
        .unwrap_or_else(|| Duration::from_millis(cfg.cold_start_prior_ms as u64))
        .as_secs_f32()
        .max(1.0);

    // Weighted contributions.  Terminal agents are fully certain (weight 1.0).
    // Running agents get a per-agent weight derived from their log-normal
    // time-credit.  Queued / suspended agents contribute zero progress and
    // zero weight (no information).
    let mut weighted_sum = terminal as f32; // terminal agents at progress=1.0
    let mut weight_sum = terminal as f32;

    for dur in &input.running_durations {
        let elapsed_ms = dur.as_secs_f32().max(0.0);
        let time_credit = lognormal_cdf(elapsed_ms, prior_median_ms, cfg.prior_shape);
        // Confidence weight: low at start, approaches 1.0 as time_credit → 1.
        let weight = cfg.min_running_weight + (1.0 - cfg.min_running_weight) * time_credit;
        weighted_sum += time_credit * cfg.unfinished_progress_cap * weight;
        weight_sum += weight;
    }

    if weight_sum <= 0.0 {
        return 0.0;
    }

    (weighted_sum / weight_sum).min(cfg.aggregate_progress_cap)
}

// ---------------------------------------------------------------------------
// Phase enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwarmEstimatorPhase {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl SwarmEstimatorPhase {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::TimedOut
        )
    }
}

// ---------------------------------------------------------------------------
// Config (Fix 6 — all tunable constants in one place)
// ---------------------------------------------------------------------------

/// Internal configuration for [`SwarmProgressEstimator`].  Not exposed to end
/// users — centralised so the estimator's behaviour is documented and tunable
/// in one place rather than scattered as magic numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwarmEstimatorConfig {
    /// Hard cap on an individual non-terminal agent's raw progress (ticks).
    pub unfinished_progress_cap: f32,
    /// Hard cap on the aggregate swarm progress while any agent is still
    /// active.
    pub aggregate_progress_cap: f32,
    /// Lower bound on the confidence weight of a running agent in the
    /// weighted aggregate.  Ensures running agents always contribute *some*
    /// signal even at the start.
    pub min_running_weight: f32,
    /// Cold-start prior for expected task duration, in milliseconds, used
    /// when no completion samples exist yet.
    pub cold_start_prior_ms: f32,
    /// Shape parameter (σ) of the log-normal CDF used for time-credit.
    /// Larger → more spread (slower convergence to 1.0).
    pub prior_shape: f32,
    /// Multiplier applied to the observed median completed duration to derive
    /// the prior median for still-running tasks.  Accounts for the fact that
    /// running tasks tend to be longer-lived than already-completed ones
    /// (survivorship bias).
    pub workload_spread_factor: f32,
    /// Floor for `display_ticks` after the first tool call — gives the
    /// progress bar an immediate visible nudge when work begins.
    pub initial_tool_credit_floor: f32,
    /// Catch-up animation window (ms) — member is considered to have pending
    /// catch-up for this long after its last activity.
    pub catchup_time_ms: u64,
    /// If a running member has no fresh activity for this long, freeze its
    /// time-credit at the stale boundary so wall-clock time cannot create fake
    /// progress while the child is waiting on an external bottleneck.
    pub stale_activity_after_ms: u64,
}

impl Default for SwarmEstimatorConfig {
    fn default() -> Self {
        Self {
            unfinished_progress_cap: 0.85,
            aggregate_progress_cap: 0.95,
            min_running_weight: 0.3,
            cold_start_prior_ms: 180_000.0, // 3 min — more conservative than old 2 min
            prior_shape: 0.6,
            workload_spread_factor: 1.5,
            initial_tool_credit_floor: 0.12,
            catchup_time_ms: 1_500,
            stale_activity_after_ms: 45_000,
        }
    }
}

// ---------------------------------------------------------------------------
// Estimate result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwarmProgressEstimate {
    pub raw_ticks: f32,
    pub display_ticks: f32,
    pub progress: f32,
    /// Confidence weight in `[0, 1]` — how much this agent's estimate should
    /// influence the aggregate.  Terminal = 1.0, queued = 0.0, running scales
    /// with evidence.
    pub confidence: f32,
    pub boosted: bool,
}

// ---------------------------------------------------------------------------
// Stateful estimator (used by TUI)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SwarmProgressEstimator {
    members: BTreeMap<String, MemberProgressState>,
    completed_samples: Vec<CompletedSample>,
    config: SwarmEstimatorConfig,
}

impl Eq for SwarmProgressEstimator {}

#[derive(Debug, Clone, Default, PartialEq)]
struct MemberProgressState {
    started_at_ms: Option<u64>,
    terminal_at_ms: Option<u64>,
    last_activity_ms: Option<u64>,
    tool_call_ids: BTreeSet<String>,
    display_ticks: f32,
    catchup_until_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompletedSample {
    duration_ms: u64,
}

impl SwarmProgressEstimator {
    pub fn ensure_member(&mut self, member_id: &str, now_ms: u64) {
        self.members
            .entry(member_id.to_owned())
            .or_insert_with(|| MemberProgressState {
                last_activity_ms: Some(now_ms),
                ..MemberProgressState::default()
            });
    }

    pub fn mark_started(&mut self, member_id: &str, now_ms: u64) {
        self.ensure_member(member_id, now_ms);
        if let Some(member) = self.members.get_mut(member_id) {
            member.started_at_ms.get_or_insert(now_ms);
            member.last_activity_ms = Some(member.last_activity_ms.unwrap_or(now_ms).max(now_ms));
        }
    }

    pub fn note_activity(&mut self, member_id: &str, activity_ms: u64) {
        self.ensure_member(member_id, activity_ms);
        if let Some(member) = self.members.get_mut(member_id) {
            member.last_activity_ms = Some(
                member
                    .last_activity_ms
                    .unwrap_or(activity_ms)
                    .max(activity_ms),
            );
        }
    }

    pub fn record_tool_call(&mut self, member_id: &str, tool_call_id: &str, now_ms: u64) {
        self.ensure_member(member_id, now_ms);
        if let Some(member) = self.members.get_mut(member_id) {
            member.started_at_ms.get_or_insert(now_ms);
            if member.tool_call_ids.insert(tool_call_id.to_owned()) {
                member.last_activity_ms =
                    Some(member.last_activity_ms.unwrap_or(now_ms).max(now_ms));
                member.display_ticks = member
                    .display_ticks
                    .max(self.config.initial_tool_credit_floor);
            }
        }
    }

    pub fn mark_completed(&mut self, member_id: &str, now_ms: u64) {
        self.mark_terminal(member_id, now_ms, true);
    }

    pub fn mark_failed(&mut self, member_id: &str, now_ms: u64) {
        self.mark_terminal(member_id, now_ms, true);
    }

    pub fn mark_cancelled(&mut self, member_id: &str, now_ms: u64) {
        self.mark_terminal(member_id, now_ms, false);
    }

    fn mark_terminal(&mut self, member_id: &str, now_ms: u64, sample_duration: bool) {
        self.ensure_member(member_id, now_ms);
        if let Some(member) = self.members.get_mut(member_id) {
            let already_terminal = member.terminal_at_ms.is_some();
            member.terminal_at_ms.get_or_insert(now_ms);
            member.last_activity_ms = Some(now_ms);
            member.catchup_until_ms = Some(now_ms.saturating_add(self.config.catchup_time_ms));
            member.display_ticks = member.display_ticks.max(1.0);
            if sample_duration
                && !already_terminal
                && let Some(started_at) = member.started_at_ms
            {
                self.completed_samples.push(CompletedSample {
                    duration_ms: now_ms.saturating_sub(started_at).max(1),
                });
            }
        }
    }

    #[must_use]
    pub fn estimate(
        &mut self,
        member_id: &str,
        phase: SwarmEstimatorPhase,
        capacity_ticks: f32,
        now_ms: u64,
    ) -> SwarmProgressEstimate {
        self.ensure_member(member_id, now_ms);
        let (raw_ticks, time_credit) = self.raw_ticks(member_id, phase, capacity_ticks, now_ms);
        let member = self
            .members
            .get_mut(member_id)
            .expect("member just ensured");
        let target = if phase.is_terminal() {
            capacity_ticks.max(1.0)
        } else {
            raw_ticks.min(capacity_ticks * self.config.unfinished_progress_cap)
        };
        let previous = member.display_ticks;
        member.display_ticks = member.display_ticks.max(target);
        let display_ticks = member.display_ticks;
        let progress = if phase.is_terminal() {
            1.0
        } else if capacity_ticks <= f32::EPSILON {
            0.0
        } else {
            (display_ticks / capacity_ticks).clamp(0.0, self.config.aggregate_progress_cap)
        };

        // Confidence: terminal = 1.0, queued/no-start = 0.0, running scales
        // with time_credit (evidence of progress).
        let confidence = if phase.is_terminal() {
            1.0
        } else if phase == SwarmEstimatorPhase::Queued {
            0.0
        } else {
            self.config.min_running_weight + (1.0 - self.config.min_running_weight) * time_credit
        };

        SwarmProgressEstimate {
            raw_ticks,
            display_ticks,
            progress,
            confidence,
            boosted: display_ticks > previous,
        }
    }

    #[must_use]
    pub fn has_pending_catchup(&self) -> bool {
        self.members.values().any(|member| {
            member
                .catchup_until_ms
                .zip(member.last_activity_ms)
                .is_some_and(|(until, last)| last < until)
        })
    }

    /// Compute raw ticks and the time-credit component for a member.
    /// Returns `(raw_ticks, time_credit)` so the caller can derive confidence.
    fn raw_ticks(
        &self,
        member_id: &str,
        phase: SwarmEstimatorPhase,
        capacity_ticks: f32,
        now_ms: u64,
    ) -> (f32, f32) {
        if phase.is_terminal() {
            return (capacity_ticks.max(1.0), 1.0);
        }
        let Some(member) = self.members.get(member_id) else {
            return (0.0, 0.0);
        };
        let Some(started_at) = member.started_at_ms else {
            return (0.0, 0.0);
        };
        let effective_now_ms = member.last_activity_ms.map_or(now_ms, |last_activity_ms| {
            now_ms.min(last_activity_ms.saturating_add(self.config.stale_activity_after_ms))
        });
        let elapsed_ms = effective_now_ms.saturating_sub(started_at) as f32;
        let (prior_median_ms, shape) = self.prior_duration();
        // Log-normal CDF: smooth S-curve from 0 → 1.  No hard stall at any
        // fixed threshold; growth decelerates naturally.
        let time_credit = lognormal_cdf(elapsed_ms, prior_median_ms, shape);
        // Fix 5: logarithmic diminishing returns on tool count.
        let tool_count = member.tool_call_ids.len() as f32;
        let tool_credit = (0.15 * (1.0 + tool_count).ln()).min(0.35);
        let combined = (time_credit + tool_credit).min(self.config.unfinished_progress_cap);
        let ticks = (capacity_ticks * combined).max(member.display_ticks);
        (ticks, time_credit)
    }

    /// Returns `(prior_median_ms, shape)` for the log-normal time-credit CDF.
    /// When we have completion samples we use the observed median scaled by
    /// the workload spread factor; otherwise we fall back to the conservative
    /// cold-start prior.
    fn prior_duration(&self) -> (f32, f32) {
        if self.completed_samples.is_empty() {
            return (self.config.cold_start_prior_ms, self.config.prior_shape);
        }
        let mut samples = self
            .completed_samples
            .iter()
            .map(|sample| sample.duration_ms)
            .collect::<Vec<_>>();
        samples.sort_unstable();
        let median = samples[samples.len() / 2] as f32;
        let adjusted = (median * self.config.workload_spread_factor).max(1.0);
        (adjusted, self.config.prior_shape)
    }
}

// ---------------------------------------------------------------------------
// Math helpers
// ---------------------------------------------------------------------------

/// Log-normal CDF evaluated at `x` with the given `median` and log-space
/// standard deviation `sigma`.
///
/// `CDF(x) = 0.5 * (1 + erf(ln(x / median) / (sigma * sqrt(2))))`
///
/// This gives a smooth S-curve: at `x = median` the CDF is 0.5; it
/// accelerates before the median and decelerates after, avoiding the
/// "rush-then-stall" behaviour of a linear clamp.
fn lognormal_cdf(x: f32, median: f32, sigma: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let sigma_safe = sigma.max(0.01);
    let z = (x / median.max(1.0)).ln() / (sigma_safe * core::f32::consts::SQRT_2);
    0.5 * (1.0 + erf(z))
}

/// Abramowitz & Stegun 7.1.26 rational approximation of the error function.
/// Max absolute error ≈ 1.5e-7, more than sufficient for progress estimation.
fn erf(x: f32) -> f32 {
    // Constants from Abramowitz & Stegun 7.1.26 (truncated to f32 precision).
    const A1: f32 = 0.254_829_6;
    const A2: f32 = -0.284_496_7;
    const A3: f32 = 1.421_413_8;
    const A4: f32 = -1.453_152;
    const A5: f32 = 1.061_405_4;
    const P: f32 = 0.327_591_1;
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x_abs = x.abs();
    let t = 1.0 / (1.0 + P * x_abs);
    let y = 1.0 - (((((A5 * t + A4) * t) + A3) * t + A2) * t + A1) * t * (-x_abs * x_abs).exp();
    sign * y
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- lognormal_cdf / erf --

    #[test]
    fn erf_at_zero_is_zero() {
        assert!(erf(0.0).abs() < 1e-5);
    }

    #[test]
    fn erf_symmetry() {
        assert!((erf(0.5) + erf(-0.5)).abs() < 1e-5);
    }

    #[test]
    fn lognormal_cdf_at_median_is_half() {
        let cdf = lognormal_cdf(100.0, 100.0, 0.6);
        assert!(
            (cdf - 0.5).abs() < 1e-3,
            "CDF at median should be ~0.5, got {cdf}"
        );
    }

    #[test]
    fn lognormal_cdf_monotone_increasing() {
        let median = 100.0_f32;
        let sigma = 0.6;
        let mut prev = -1.0;
        // At 10x the median (1000), the CDF should be very close to 1.0.
        for i in 0..1000 {
            let x = i as f32;
            let cdf = lognormal_cdf(x, median, sigma);
            assert!(cdf >= prev, "not monotone at x={x}: {cdf} < {prev}");
            prev = cdf;
        }
        assert!(prev > 0.99);
    }

    // -- standalone estimate_swarm_progress --

    #[test]
    fn progress_estimate_never_claims_completion_while_items_are_active() {
        let progress = estimate_swarm_progress(&SwarmProgressInput {
            total: 4,
            completed: 3,
            failed: 0,
            running: 1,
            queued: 0,
            suspended: 0,
            median_completed_duration: Some(Duration::from_secs(10)),
            running_durations: vec![Duration::from_secs(100)],
        });
        assert!(progress < 1.0);
        assert!(progress <= 0.95);
    }

    #[test]
    fn progress_estimate_returns_full_when_all_terminal() {
        let progress = estimate_swarm_progress(&SwarmProgressInput {
            total: 3,
            completed: 2,
            failed: 1,
            running: 0,
            queued: 0,
            suspended: 0,
            median_completed_duration: None,
            running_durations: vec![],
        });
        assert!((progress - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn progress_estimate_increases_with_running_duration() {
        let early = estimate_swarm_progress(&SwarmProgressInput {
            total: 4,
            completed: 0,
            failed: 0,
            running: 1,
            queued: 3,
            suspended: 0,
            median_completed_duration: Some(Duration::from_secs(60)),
            running_durations: vec![Duration::from_secs(5)],
        });
        let late = estimate_swarm_progress(&SwarmProgressInput {
            total: 4,
            completed: 0,
            failed: 0,
            running: 1,
            queued: 3,
            suspended: 0,
            median_completed_duration: Some(Duration::from_secs(60)),
            running_durations: vec![Duration::from_secs(50)],
        });
        assert!(late > early, "late={late} should be > early={early}");
    }

    // -- Fix 4: per-agent durations --

    #[test]
    fn progress_estimate_uses_individual_durations_not_longest() {
        // Two running agents: one very early (5s), one very late (600s).
        // With the old `longest_running_duration` the aggregate would be near
        // the cap.  With per-agent durations the early agent drags the
        // aggregate down.
        let mixed = estimate_swarm_progress(&SwarmProgressInput {
            total: 2,
            completed: 0,
            failed: 0,
            running: 2,
            queued: 0,
            suspended: 0,
            median_completed_duration: Some(Duration::from_secs(120)),
            running_durations: vec![Duration::from_secs(5), Duration::from_secs(600)],
        });
        // A hypothetical "both at 600s" scenario for comparison.
        let both_late = estimate_swarm_progress(&SwarmProgressInput {
            total: 2,
            completed: 0,
            failed: 0,
            running: 2,
            queued: 0,
            suspended: 0,
            median_completed_duration: Some(Duration::from_secs(120)),
            running_durations: vec![Duration::from_secs(600), Duration::from_secs(600)],
        });
        assert!(
            mixed < both_late,
            "mixed durations should produce lower estimate: mixed={mixed}, both_late={both_late}"
        );
    }

    // -- Fix 3: cold-start smoothness --

    #[test]
    fn cold_start_progress_is_not_linear() {
        let cfg = SwarmEstimatorConfig::default();
        let prior = cfg.cold_start_prior_ms;
        // At 50% of prior elapsed, a linear model gives exactly 0.5.
        // The log-normal CDF at the median gives 0.5 but here we're at 50% of
        // the prior, so it should be significantly below 0.5.
        let cdf_half = lognormal_cdf(prior * 0.5, prior, cfg.prior_shape);
        assert!(
            cdf_half < 0.5,
            "CDF below median should be < 0.5, got {cdf_half}"
        );
        // At the prior median, CDF should be ~0.5
        let cdf_median = lognormal_cdf(prior, prior, cfg.prior_shape);
        assert!((cdf_median - 0.5).abs() < 1e-3);
    }

    // -- Fix 1: cross-frame monotonicity (stateful estimator) --

    #[test]
    fn estimator_display_ticks_never_decrease_across_calls() {
        let mut est = SwarmProgressEstimator::default();
        est.mark_started("a", 0);
        // First call at t=10s — no samples, cold-start prior is 180s.
        let r1 = est.estimate("a", SwarmEstimatorPhase::Running, 1.0, 10_000);
        // Simulate a sample completing, which raises the prior median.
        est.mark_completed("helper", 5_000);
        // Second call at t=15s — prior may have changed, but display_ticks
        // must not go backwards.
        let r2 = est.estimate("a", SwarmEstimatorPhase::Running, 1.0, 15_000);
        assert!(
            r2.display_ticks >= r1.display_ticks,
            "display_ticks went backwards: {} → {}",
            r1.display_ticks,
            r2.display_ticks
        );
    }

    // -- Fix 2: confidence --

    #[test]
    fn estimator_confidence_increases_with_time_credit() {
        let mut est = SwarmProgressEstimator::default();
        est.mark_started("a", 0);
        let early = est.estimate("a", SwarmEstimatorPhase::Running, 1.0, 5_000);
        let late = est.estimate("a", SwarmEstimatorPhase::Running, 1.0, 600_000);
        assert!(
            late.confidence > early.confidence,
            "confidence should increase: early={}, late={}",
            early.confidence,
            late.confidence
        );
        // Terminal should be 1.0.
        est.mark_completed("a", 700_000);
        let terminal = est.estimate("a", SwarmEstimatorPhase::Completed, 1.0, 700_000);
        assert!((terminal.confidence - 1.0).abs() < 1e-5);
    }

    // -- Fix 5: tool credit diminishing returns --

    #[test]
    fn tool_credit_has_diminishing_returns() {
        let credit = |n: f32| 0.15 * (1.0 + n).ln();
        let delta_early = credit(2.0) - credit(1.0);
        let delta_late = credit(10.0) - credit(9.0);
        assert!(
            delta_early > delta_late,
            "early delta ({delta_early}) should > late delta ({delta_late})"
        );
    }
}
