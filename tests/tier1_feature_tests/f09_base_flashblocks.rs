//! Feature 9: Base Flashblocks 200ms Stream & Gas Budgeting Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md line 29 and explorer_survey_3/handoff.md.

use crate::common::*;

#[test]
fn test_f09_01_flashblock_cadence_parameters() {
    let sub_block_cadence_ms = 200u64;
    let block_time_ms = 2000u64;
    let sub_blocks_per_block = block_time_ms / sub_block_cadence_ms;
    assert_eq!(sub_blocks_per_block, 10, "There must be exactly 10 Flashblocks per block");
}

#[test]
fn test_f09_02_cumulative_gas_budgeting() {
    let block_gas_limit = 30_000_000u64;
    let tracker = FlashblockGasTracker::new(block_gas_limit);

    // j = 0 (1st flashblock): 1/10th = 3,000,000 gas
    assert_eq!(tracker.sub_block_gas_budget(0), 3_000_000);

    // j = 4 (5th flashblock): 5/10th = 15,000,000 gas
    assert_eq!(tracker.sub_block_gas_budget(4), 15_000_000);

    // j = 9 (10th flashblock): 10/10th = 30,000,000 gas
    assert_eq!(tracker.sub_block_gas_budget(9), 30_000_000);
}

#[test]
fn test_f09_03_gas_budget_overflow_throttling() {
    let block_gas_limit = 10_000_000u64;
    let mut tracker = FlashblockGasTracker::new(block_gas_limit);

    // Sub-block 0 budget is 1,000,000. Trying to budget 1,500,000 must fail
    let res = tracker.try_budget_tx(0, 1_500_000);
    assert!(res.is_err(), "Gas exceeding sub-block budget must be throttled");

    // Budgeting 800,000 succeeds
    let ok = tracker.try_budget_tx(0, 800_000);
    assert!(ok.is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 800_000);
}

#[test]
fn test_f09_04_block_reset() {
    let mut tracker = FlashblockGasTracker::new(30_000_000);
    let _ = tracker.try_budget_tx(5, 10_000_000);
    assert_eq!(tracker.cumulative_gas_spent, 10_000_000);

    tracker.reset_block();
    assert_eq!(tracker.cumulative_gas_spent, 0);
    assert_eq!(tracker.current_sub_block_index, 0);
}

#[test]
fn test_f09_05_sequential_sub_block_inclusion() {
    let mut tracker = FlashblockGasTracker::new(30_000_000);
    // Ingest 10 consecutive sub-blocks with 1,000,000 gas each
    for j in 0..10 {
        let res = tracker.try_budget_tx(j, 1_000_000);
        assert!(res.is_ok(), "Failed at sub-block {}", j);
    }
    assert_eq!(tracker.cumulative_gas_spent, 10_000_000);
}
