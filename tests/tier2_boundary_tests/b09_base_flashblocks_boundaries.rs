//! Feature 9 Boundaries: Base Flashblocks BVA Test Suite

use crate::common::*;

#[test]
fn test_b09_01_sub_block_0_minimal_budget() {
    let tracker = FlashblockGasTracker::new(10_000_000);
    // j = 0: budget = 1,000,000
    assert_eq!(tracker.sub_block_gas_budget(0), 1_000_000);
}

#[test]
fn test_b09_02_sub_block_9_maximal_budget() {
    let tracker = FlashblockGasTracker::new(10_000_000);
    // j = 9: budget = 10,000,000
    assert_eq!(tracker.sub_block_gas_budget(9), 10_000_000);
}

#[test]
fn test_b09_03_exact_budget_exhaustion() {
    let mut tracker = FlashblockGasTracker::new(10_000_000);
    // Exactly 1,000,000 in sub-block 0
    let res = tracker.try_budget_tx(0, 1_000_000);
    assert!(res.is_ok());

    // Even 1 gas more fails in sub-block 0
    let fail = tracker.try_budget_tx(0, 1);
    assert!(fail.is_err());
}

#[test]
fn test_b09_04_zero_gas_tx() {
    let mut tracker = FlashblockGasTracker::new(10_000_000);
    let res = tracker.try_budget_tx(0, 0);
    assert!(res.is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 0);
}

#[test]
fn test_b09_05_sub_block_boundary_clamping() {
    let tracker = FlashblockGasTracker::new(10_000_000);
    // j > 9 is clamped to total block gas limit
    assert_eq!(tracker.sub_block_gas_budget(15), 10_000_000);
}
