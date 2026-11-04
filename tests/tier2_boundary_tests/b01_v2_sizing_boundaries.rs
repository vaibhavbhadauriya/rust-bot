//! Feature 1 Boundaries: V2 Constant-Product Sizing BVA Test Suite

use crate::common::*;

#[test]
fn test_b01_01_zero_reserve_returns_none() {
    assert!(solve_2pool_optimal_f64(0.0, 405.0, 1_000_000.0, 395.0, 0.003, 0.003, 0.0).is_none());
    assert!(solve_2pool_optimal_f64(1_000_000.0, 0.0, 1_000_000.0, 395.0, 0.003, 0.003, 0.0).is_none());
}

#[test]
fn test_b01_02_extreme_asymmetry_no_overflow() {
    let res = solve_2pool_optimal_f64(1e12, 1.0, 1e12, 0.9, 0.003, 0.003, 0.0);
    assert!(res.is_some());
    assert!(res.unwrap().is_finite());
}

#[test]
fn test_b01_03_inverted_spread_no_trade() {
    // Pool A price = 2500, Pool B price = 2400 (unfavorable for arbitrage)
    let res = solve_2pool_optimal_f64(1_000_000.0, 400.0, 1_000_000.0, 416.6, 0.003, 0.003, 0.0);
    assert!(res.is_none(), "Unfavorable spread must return None");
}

#[test]
fn test_b01_04_fee_100_percent() {
    // 100% fee (f = 1.0, gamma = 0.0) -> impossible to profit
    let res = solve_2pool_optimal_f64(1_000_000.0, 405.0, 1_000_000.0, 395.0, 1.0, 1.0, 0.0);
    assert!(res.is_none());
}

#[test]
fn test_b01_05_microscopic_divergence_below_fees() {
    // 0.01% price dislocation but 0.3% fees in both pools (round-trip fee ~0.6%)
    // Gross dislocation is eaten by fees -> yields None
    let res = solve_2pool_optimal_f64(1_000_000.0, 400.0, 1_000_000.0, 399.96, 0.003, 0.003, 0.0);
    assert!(res.is_none(), "Dislocation below round-trip fees must return None");
}
