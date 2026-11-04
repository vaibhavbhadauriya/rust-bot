//! Feature 11 Boundaries: Fast-Revert Guard BVA Test Suite

use crate::common::*;

#[test]
fn test_b11_01_one_wei_reserve_shift_triggers_revert() {
    let outcome = simulate_fast_revert_guard(1_000_000, 2_000_000, 1_000_001, 2_000_000);
    assert_ne!(outcome, GuardOutcome::Passed);
}

#[test]
fn test_b11_02_zero_reserve_edge_case() {
    let outcome = simulate_fast_revert_guard(0, 0, 0, 0);
    assert_eq!(outcome, GuardOutcome::Passed);
}

#[test]
fn test_b11_03_profit_exactly_equal_gas_cost() {
    // wethProfit == gasCost passes (breakeven)
    assert!(check_atomic_profit(200_000, 200_000).is_ok());
}

#[test]
fn test_b11_04_profit_one_wei_below_gas_cost() {
    // wethProfit == gasCost - 1 reverts
    assert!(check_atomic_profit(199_999, 200_000).is_err());
}

#[test]
fn test_b11_05_zero_gas_cost_boundary() {
    assert!(check_atomic_profit(0, 0).is_ok());
}
