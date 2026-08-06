use super::*;

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

// -- Calibration regression tests (real swarm data from ~/.neo/sessions) --
//
// Scenarios replayed from swarm_d49361cda20a48efb0c5dd56d4248b57 (a real
// 3-agent swarm: children completed at 348s / 392s / 726s). With the old
// defaults the estimate raced ahead of reality — 71% displayed while 0%
// of items were done, ~0.81 mid-flight vs a true 0.67. The calibrated
// defaults must track the real completion fraction instead.
#[test]
fn calibrated_defaults_do_not_race_ahead_before_first_completion() {
    // 3 agents running at 300s elapsed, nothing completed yet; the first
    // real completion in this swarm landed at 348s.
    // Expected: a small but non-zero running credit (~1-2%).  The narrow
    // band also pins the unit of `cold_start_prior_ms`: if it were fed to
    // the standalone estimator as milliseconds (1000x too large) the
    // result would collapse to exactly 0.0 and fail the lower bound.
    let progress = estimate_swarm_progress(&SwarmProgressInput {
        total: 3,
        completed: 0,
        failed: 0,
        running: 3,
        queued: 0,
        suspended: 0,
        median_completed_duration: None,
        running_durations: vec![Duration::from_secs(300); 3],
    });
    assert!(
        (0.005..=0.05).contains(&progress),
        "early estimate out of expected band: {progress}"
    );
}

#[test]
fn calibrated_defaults_track_real_completion_fraction() {
    // Same swarm mid-flight: 2 of 3 done (348s / 392s), last agent elapsed
    // 700s of its eventual 726s. True fraction is 0.67. The band upper
    // bound 0.75 excludes the old uncalibrated defaults, which give ~0.81
    // on this exact input (the over-estimation this calibration fixes).
    let progress = estimate_swarm_progress(&SwarmProgressInput {
        total: 3,
        completed: 2,
        failed: 0,
        running: 1,
        queued: 0,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(370)),
        running_durations: vec![Duration::from_secs(700)],
    });
    assert!(
        (0.6..=0.75).contains(&progress),
        "mid-flight estimate drifted from reality: {progress}"
    );
}
