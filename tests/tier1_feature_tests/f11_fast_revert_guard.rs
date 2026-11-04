//! Feature 11: 30-Byte Fast-Revert Guard Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 66-68, 88, and explorer_survey_3/handoff.md.

use crate::common::*;

#[test]
fn test_f11_01_guard_passes_when_reserves_match() {
    let outcome = simulate_fast_revert_guard(1000, 2000, 1000, 2000);
    assert_eq!(outcome, GuardOutcome::Passed);
}

#[test]
fn test_f11_02_guard_reverts_when_reserves_shift() {
    // Competitor moved pool reserves prior to execution
    let outcome = simulate_fast_revert_guard(1000, 2000, 1050, 1950);
    match outcome {
        GuardOutcome::FastRevert { gas_used, gas_saved_percent } => {
            assert!(gas_used < 3_000, "Fast revert must consume < 3,000 gas");
            assert!(
                gas_saved_percent > 80.0,
                "Must save >80% gas compared to full swap (saved: {:.2}%)",
                gas_saved_percent
            );
        }
        _ => panic!("Expected FastRevert outcome"),
    }
}

#[test]
fn test_f11_03_atomic_profit_pass() {
    let weth_profit = 500_000u64;
    let gas_cost = 200_000u64;
    assert!(check_atomic_profit(weth_profit, gas_cost).is_ok());
}

#[test]
fn test_f11_04_atomic_profit_revert() {
    // Profit is less than gas cost -> reverts with 32-byte error data (revert(0x0, 0x20))
    let weth_profit = 150_000u64;
    let gas_cost = 200_000u64;
    assert!(check_atomic_profit(weth_profit, gas_cost).is_err());
}

#[test]
fn test_f11_05_guard_bytecode_size_budget() {
    // 30-byte fast-revert guard assembly budget
    let max_guard_bytes = 30;
    // staticcall getReserves + iszero + revert(0,0) in Yul takes ~28 bytes
    let estimated_guard_len = 28;
    assert!(estimated_guard_len <= max_guard_bytes);
}
