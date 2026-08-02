// Standalone verification harness for the swarm progress estimator.
//
// neo-agent-core cannot be compiled right now because a parallel agent's
// in-progress `neo-ai` changes (new `phase` field) break the dependency
// graph. progress.rs itself only depends on `std`, so we compile it
// standalone and run the calibration regression scenarios here.
//
// Build:  rustc -O tools/progress_check/main.rs -o tools/progress_check/check
// Run:    tools/progress_check/check
//
// When the workspace compiles again, the same scenarios are enforced by the
// real tests in crates/neo-agent-core/src/multi_agent/progress.rs.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

// ---------- verbatim copy of progress.rs (minus #[cfg(test)] module) ----------

#[derive(Debug, Clone, PartialEq)]
pub struct SwarmProgressInput {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub running: usize,
    pub queued: usize,
    pub suspended: usize,
    pub median_completed_duration: Option<Duration>,
    pub running_durations: Vec<Duration>,
}

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

    // Prior median: observed median completed duration scaled by the workload
    // spread factor (running tasks tend to be longer-lived than completed
    // ones — survivorship bias), or the conservative cold-start default when
    // no completion samples exist yet. Mirrors `SwarmProgressEstimator::prior_duration`.
    let prior_median_ms = input
        .median_completed_duration
        .map(|duration| duration.as_secs_f32() * cfg.workload_spread_factor)
        .unwrap_or(cfg.cold_start_prior_ms)
        .max(1.0);

    let mut weighted_sum = terminal as f32;
    let weight_sum = input.total as f32;

    for dur in &input.running_durations {
        let elapsed_ms = dur.as_secs_f32().max(0.0);
        let time_credit = lognormal_cdf(elapsed_ms, prior_median_ms, cfg.prior_shape);
        let weight = cfg.min_running_weight + (1.0 - cfg.min_running_weight) * time_credit;
        weighted_sum += time_credit * cfg.unfinished_progress_cap * weight;
    }

    if weight_sum <= 0.0 {
        return 0.0;
    }

    (weighted_sum / weight_sum).min(cfg.aggregate_progress_cap)
}

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwarmEstimatorConfig {
    pub unfinished_progress_cap: f32,
    pub aggregate_progress_cap: f32,
    pub min_running_weight: f32,
    pub cold_start_prior_ms: f32,
    pub prior_shape: f32,
    pub workload_spread_factor: f32,
    pub tool_credit_cap: f32,
    pub initial_tool_credit_floor: f32,
    pub catchup_time_ms: u64,
    pub stale_activity_after_ms: u64,
}

impl Default for SwarmEstimatorConfig {
    fn default() -> Self {
        // Calibrated against real swarm data replayed from `~/.neo/sessions`
        // (80 historical swarms, 154 child-duration samples; 15 real swarms
        // with child duration > 60s used for replay evaluation).  Real child
        // durations cluster around 3-20 minutes (median ≈ 6-7 min) with a
        // heavy tail beyond 40 min, and the slowest child in a swarm runs
        // ~2-3x the completed median (excluding stuck agents).
        //
        // The previous defaults (cold-start 180s, spread 1.5, min weight 0.3,
        // cap 0.85) over-credited fresh agents: replay showed the estimate
        // racing ahead of reality (e.g. 71% displayed while 0% of items were
        // done; MAE 0.27, mean over-estimate +0.30 in the first half).
        // Current values keep the estimate close to the true completion
        // fraction (replay MAE 0.02, over-estimate +0.03 early / +0.00 late).
        Self {
            unfinished_progress_cap: 0.7,
            aggregate_progress_cap: 0.95,
            min_running_weight: 0.1,
            cold_start_prior_ms: 600_000.0, // 10 min — real median is ~6-7 min
            prior_shape: 0.5,
            workload_spread_factor: 3.0,
            tool_credit_cap: 0.2,
            initial_tool_credit_floor: 0.12,
            catchup_time_ms: 1_500,
            stale_activity_after_ms: 45_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwarmProgressEstimate {
    pub raw_ticks: f32,
    pub display_ticks: f32,
    pub progress: f32,
    pub confidence: f32,
    pub boosted: bool,
}

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
        let time_credit = lognormal_cdf(elapsed_ms, prior_median_ms, shape);
        let tool_count = member.tool_call_ids.len() as f32;
        let tool_credit = (0.15 * (1.0 + tool_count).ln()).min(self.config.tool_credit_cap);
        let combined = (time_credit + tool_credit).min(self.config.unfinished_progress_cap);
        let ticks = (capacity_ticks * combined).max(member.display_ticks);
        (ticks, time_credit)
    }

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

fn lognormal_cdf(x: f32, median: f32, sigma: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let sigma_safe = sigma.max(0.01);
    let z = (x / median.max(1.0)).ln() / (sigma_safe * core::f32::consts::SQRT_2);
    0.5 * (1.0 + erf(z))
}

fn erf(x: f32) -> f32 {
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

// ---------- calibration regression scenarios (mirror the #[cfg(test)] tests) ----------

fn main() {
    let mut failures = 0;
    let mut check = |name: &str, ok: bool, detail: String| {
        if ok {
            println!("PASS {name}");
        } else {
            failures += 1;
            println!("FAIL {name}: {detail}");
        }
    };

    // Scenario 1 (real swarm d49361 at t=300s): 3 agents running, nothing
    // completed; old defaults displayed 71%. Calibrated defaults must stay low.
    let early = estimate_swarm_progress(&SwarmProgressInput {
        total: 3,
        completed: 0,
        failed: 0,
        running: 3,
        queued: 0,
        suspended: 0,
        median_completed_duration: None,
        running_durations: vec![Duration::from_secs(300); 3],
    });
    check(
        "no-race-before-first-completion",
        early < 0.15,
        format!("early estimate too optimistic: {early}"),
    );
    println!("    early estimate = {:.1}% (old defaults: ~71%)", early * 100.0);

    // Scenario 2 (same swarm mid-flight): 2 of 3 done, last agent 700s of 726s.
    let mid = estimate_swarm_progress(&SwarmProgressInput {
        total: 3,
        completed: 2,
        failed: 0,
        running: 1,
        queued: 0,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(370)),
        running_durations: vec![Duration::from_secs(700)],
    });
    check(
        "mid-flight-tracks-reality",
        (0.6..=0.85).contains(&mid),
        format!("mid-flight estimate drifted from reality: {mid}"),
    );
    println!("    mid-flight estimate = {:.1}% (true 67%, old defaults: ~81%)", mid * 100.0);

    // Existing unit-test invariants that must survive the calibration:
    // 1. never claim completion while items are active
    let p1 = estimate_swarm_progress(&SwarmProgressInput {
        total: 4,
        completed: 3,
        failed: 0,
        running: 1,
        queued: 0,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(10)),
        running_durations: vec![Duration::from_secs(100)],
    });
    check("never-claims-completion", p1 < 1.0 && p1 <= 0.95, format!("p1={p1}"));

    // 2. full when all terminal
    let p2 = estimate_swarm_progress(&SwarmProgressInput {
        total: 3,
        completed: 2,
        failed: 1,
        running: 0,
        queued: 0,
        suspended: 0,
        median_completed_duration: None,
        running_durations: vec![],
    });
    check("full-when-all-terminal", (p2 - 1.0).abs() < f32::EPSILON, format!("p2={p2}"));

    // 3. monotonically increases with running duration
    let early3 = estimate_swarm_progress(&SwarmProgressInput {
        total: 4, completed: 0, failed: 0, running: 1, queued: 3, suspended: 0,
        median_completed_duration: Some(Duration::from_secs(60)),
        running_durations: vec![Duration::from_secs(5)],
    });
    let late3 = estimate_swarm_progress(&SwarmProgressInput {
        total: 4, completed: 0, failed: 0, running: 1, queued: 3, suspended: 0,
        median_completed_duration: Some(Duration::from_secs(60)),
        running_durations: vec![Duration::from_secs(50)],
    });
    check("monotone-in-running-duration", late3 > early3, format!("{early3} -> {late3}"));

    // 4. queued items reduce progress
    let noq = estimate_swarm_progress(&SwarmProgressInput {
        total: 2, completed: 1, failed: 0, running: 1, queued: 0, suspended: 0,
        median_completed_duration: Some(Duration::from_secs(60)),
        running_durations: vec![Duration::from_secs(30)],
    });
    let wq = estimate_swarm_progress(&SwarmProgressInput {
        total: 3, completed: 1, failed: 0, running: 1, queued: 1, suspended: 0,
        median_completed_duration: Some(Duration::from_secs(60)),
        running_durations: vec![Duration::from_secs(30)],
    });
    check("queued-reduces-progress", wq < noq, format!("{wq} vs {noq}"));

    // 5. stateful estimator: display_ticks never decrease; confidence increases
    let mut est = SwarmProgressEstimator::default();
    est.mark_started("a", 0);
    let r1 = est.estimate("a", SwarmEstimatorPhase::Running, 1.0, 10_000);
    est.mark_completed("helper", 5_000);
    let r2 = est.estimate("a", SwarmEstimatorPhase::Running, 1.0, 15_000);
    check("display-ticks-monotone", r2.display_ticks >= r1.display_ticks, format!("{} -> {}", r1.display_ticks, r2.display_ticks));

    let mut est2 = SwarmProgressEstimator::default();
    est2.mark_started("a", 0);
    let e1 = est2.estimate("a", SwarmEstimatorPhase::Running, 1.0, 5_000);
    let e2 = est2.estimate("a", SwarmEstimatorPhase::Running, 1.0, 600_000);
    check("confidence-increases", e2.confidence > e1.confidence, format!("{} -> {}", e1.confidence, e2.confidence));

    // 6. stateful estimator mid-flight on the real swarm shape:
    // members started at 0/54s, completed at 348s/392s; third still running at 700s
    let mut est3 = SwarmProgressEstimator::default();
    est3.mark_started("c1", 0);
    est3.mark_started("c2", 54_000);
    est3.mark_started("c3", 60_000);
    for i in 0..10 {
        est3.record_tool_call("c1", &format!("t{i}"), i as u64 * 30_000);
    }
    est3.mark_completed("c1", 348_000);
    est3.mark_completed("c2", 392_000);
    est3.note_activity("c3", 700_000);
    let c3 = est3.estimate("c3", SwarmEstimatorPhase::Running, 1.0, 700_000);
    let weighted = (1.0 + 1.0 + c3.progress * c3.confidence) / 3.0;
    check(
        "stateful-mid-flight-tracks-reality",
        (0.6..=0.85).contains(&weighted),
        format!("weighted={weighted} (c3 progress={}, conf={})", c3.progress, c3.confidence),
    );
    println!("    stateful mid-flight weighted = {:.1}% (true 67%)", weighted * 100.0);

    if failures > 0 {
        println!("\n{failures} check(s) FAILED");
        std::process::exit(1);
    }
    println!("\nall checks passed");
}
