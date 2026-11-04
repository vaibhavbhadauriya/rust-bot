//! Tier 4 Real-World Scenario 3: Morpho Blue Zero-Fee Arbitrage with Front-Running Competitor Revert
//! Features Exercised: F1, F10, F11, F13, F14, F17
//! Authoritative source: TEST_INFRA.md line 45, ORIGINAL_REQUEST.md lines 66, 88.

use crate::common::*;
use alloy_primitives::Address;

#[test]
pub fn test_scenario3_morpho_frontrun_fast_revert_end_to_end() {
    // 1. Initial State: Arbitrage detected between Pool 1 and Pool 2
    let expected_reserve0 = 1_000_000u128;
    let expected_reserve1 = 405u128;

    // Sized optimal trade
    let dx = 4732.42;

    // Priority 1 zero-fee Morpho Blue flash loan selected
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 10_000_000_000,
        balancer_liquidity: 10_000_000_000,
        univ4_liquidity: 10_000_000_000,
        aave_liquidity: 10_000_000_000,
    };
    let (provider, fee) = route_flash_loan((dx * 1e6) as u128, &snapshot).unwrap();
    assert_eq!(provider, FlashLoanProvider::MorphoBlue);
    assert_eq!(fee, 0);

    // Encode 100-byte calldata
    let params = ExecutionParams {
        weth_amount_to_first_market: (dx * 1e6) as u64,
        balance_before: 1_000,
        gas_cost: 200,
        target1: Address::from([0xAA; 20]),
        target2: Address::from([0xBB; 20]),
        token0_amount_out: 100,
        token0_amount_out_index: 0,
        token1_amount_out: 200,
        token1_amount_out_index: 1,
    };
    let calldata = encode_oracle_calldata(&params);
    assert_eq!(calldata.len(), 100);

    // 2. Race Condition: In the same 125ms Arbitrum PGA or 200ms Base Flashblock round,
    // a competitor bot submitted a higher priority fee transaction that mined immediately before us,
    // shifting Pool 1's reserves from (1,000,000, 405) to (1,010,000, 401).
    let actual_reserve0 = 1_010_000u128;
    let actual_reserve1 = 401u128;

    // 3. 30-Byte Fast-Revert Guard Execution in revm:
    // Before triggering flash loan borrowing or DEX approvals, the guard compares snapshot vs actual.
    let outcome = simulate_fast_revert_guard(expected_reserve0, expected_reserve1, actual_reserve0, actual_reserve1);

    match outcome {
        GuardOutcome::FastRevert { gas_used, gas_saved_percent } => {
            // Must spend < 2,500 gas
            assert!(gas_used < 2500, "Fast revert must consume < 2,500 gas");
            // Must save > 80% gas
            assert!(
                gas_saved_percent > 80.0,
                "Gas savings ({:.2}%) must exceed 80%",
                gas_saved_percent
            );
        }
        GuardOutcome::Passed => panic!("Expected FastRevert due to front-running competitor"),
    }

    // 4. Kill Switch Tracking:
    let ks = KillSwitchController::new();
    // Record first fast-revert: kill switch stays untripped
    assert!(!ks.record_trade_result(true));
    assert!(!ks.is_tripped());
}

#[test]
pub fn test_scenario_3() {
    test_scenario3_morpho_frontrun_fast_revert_end_to_end();
}
