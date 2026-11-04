//! Feature 6 Boundaries: Dynamic Sp Ranking BVA Test Suite

use crate::common::*;

#[test]
fn test_b06_01_zero_volatility() {
    assert_eq!(compute_sp_score(0.0, 0.003, 100_000.0, 500_000.0, 2), 0.0);
}

#[test]
fn test_b06_02_zero_volume() {
    // ln(1 + 0) = ln(1) = 0
    assert_eq!(compute_sp_score(0.02, 0.003, 0.0, 500_000.0, 2), 0.0);
}

#[test]
fn test_b06_03_zero_liquidity() {
    assert_eq!(compute_sp_score(0.02, 0.003, 100_000.0, 0.0, 2), 0.0);
}

#[test]
fn test_b06_04_exactly_two_venues() {
    let score = compute_sp_score(0.02, 0.003, 100_000.0, 500_000.0, 2);
    assert!(score > 0.0);
}

#[test]
fn test_b06_05_huge_volume_log_damping() {
    let score_1m = compute_sp_score(0.01, 0.003, 1_000_000.0, 100_000.0, 2);
    let score_100m = compute_sp_score(0.01, 0.003, 100_000_000.0, 100_000.0, 2);

    // 100x volume increase results in only ~1.33x score increase due to ln() damping
    let ratio = score_100m / score_1m;
    assert!(ratio < 2.0, "Log damping must prevent mega-cap volume monopoly");
}
