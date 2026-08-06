use super::*;

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

#[test]
fn queued_items_reduce_swarm_progress() {
    let without_queued = estimate_swarm_progress(&SwarmProgressInput {
        total: 2,
        completed: 1,
        failed: 0,
        running: 1,
        queued: 0,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(60)),
        running_durations: vec![Duration::from_secs(30)],
    });
    let with_queued = estimate_swarm_progress(&SwarmProgressInput {
        total: 3,
        completed: 1,
        failed: 0,
        running: 1,
        queued: 1,
        suspended: 0,
        median_completed_duration: Some(Duration::from_secs(60)),
        running_durations: vec![Duration::from_secs(30)],
    });

    assert!(with_queued < without_queued);
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
