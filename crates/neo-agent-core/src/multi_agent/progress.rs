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
pub struct SwarmProgressEstimate {
    pub raw_ticks: f32,
    pub display_ticks: f32,
    pub progress: f32,
    pub boosted: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SwarmProgressEstimator {
    members: BTreeMap<String, MemberProgressState>,
    completed_samples: Vec<CompletedSample>,
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

const DEFAULT_UNFINISHED_PROGRESS_CAP: f32 = 0.85;
const DEFAULT_CATCHUP_TIME_MS: u64 = 1_500;
const DEFAULT_WORKLOAD_SPREAD_FACTOR: f32 = 1.5;

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
            member.last_activity_ms = Some(now_ms);
        }
    }

    pub fn record_tool_call(&mut self, member_id: &str, tool_call_id: &str, now_ms: u64) {
        self.mark_started(member_id, now_ms);
        if let Some(member) = self.members.get_mut(member_id)
            && member.tool_call_ids.insert(tool_call_id.to_owned())
        {
            member.last_activity_ms = Some(now_ms);
            member.display_ticks = member.display_ticks.max(0.12);
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
            member.catchup_until_ms = Some(now_ms.saturating_add(DEFAULT_CATCHUP_TIME_MS));
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
        let raw_ticks = self.raw_ticks(member_id, phase, capacity_ticks, now_ms);
        let member = self
            .members
            .get_mut(member_id)
            .expect("member just ensured");
        let target = if phase.is_terminal() {
            capacity_ticks.max(1.0)
        } else {
            raw_ticks.min(capacity_ticks * DEFAULT_UNFINISHED_PROGRESS_CAP)
        };
        let previous = member.display_ticks;
        member.display_ticks = member.display_ticks.max(target);
        let display_ticks = member.display_ticks;
        let progress = if phase.is_terminal() {
            1.0
        } else if capacity_ticks <= f32::EPSILON {
            0.0
        } else {
            (display_ticks / capacity_ticks).clamp(0.0, 0.95)
        };
        SwarmProgressEstimate {
            raw_ticks,
            display_ticks,
            progress,
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
    ) -> f32 {
        if phase.is_terminal() {
            return capacity_ticks.max(1.0);
        }
        let Some(member) = self.members.get(member_id) else {
            return 0.0;
        };
        let Some(started_at) = member.started_at_ms else {
            return 0.0;
        };
        let elapsed_ms = now_ms.saturating_sub(started_at) as f32;
        let expected_ms = self.expected_duration_ms() as f32;
        let time_credit = (elapsed_ms / expected_ms.max(1.0)).clamp(0.0, 1.0);
        let tool_credit = (member.tool_call_ids.len() as f32 * 0.08).min(0.35);
        (capacity_ticks * (time_credit + tool_credit).min(DEFAULT_UNFINISHED_PROGRESS_CAP))
            .max(member.display_ticks)
    }

    fn expected_duration_ms(&self) -> u64 {
        if self.completed_samples.is_empty() {
            return 120_000;
        }
        let mut samples = self
            .completed_samples
            .iter()
            .map(|sample| sample.duration_ms)
            .collect::<Vec<_>>();
        samples.sort_unstable();
        let median = samples[samples.len() / 2];
        ((median as f32) * DEFAULT_WORKLOAD_SPREAD_FACTOR) as u64
    }
}
