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
