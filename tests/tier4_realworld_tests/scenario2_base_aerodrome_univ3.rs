//! Tier 4 Real-World Scenario 2: Base Aerodrome Slipstream vs Uniswap V3 Arbitrage
//! Features Exercised: F4, F6, F7, F9, F10, F13, F14
//! Authoritative source: TEST_INFRA.md line 44, ORIGINAL_REQUEST.md lines 42, 79.

use crate::common::*;
use alloy_primitives::Address;

#[test]
fn test_scenario2_base_aerodrome_slipstream_vs_univ3_end_to_end() {
    // 1. Ingest Base Flashblocks 200ms sub-block stream (sub-block j = 2)
    let block_limit = 30_000_000u64;
    let mut gas_tracker = FlashblockGasTracker::new(block_limit);
    let sub_block_idx = 2; // 3rd flashblock
    assert!(gas_tracker.try_budget_tx(sub_block_idx, 350_000).is_ok());

    // 2. Dynamic Sp Scoring prioritizes ETH-USDC pair across Slipstream & Uniswap V3
    let score = compute_sp_score(0.035, 0.0003, 2_000_000.0, 12_000_000.0, 2);
    assert!(score > 50_000.0, "High volatility pair must rank into top tier");

    // 3. Concentrated Liquidity Virtual Reserves Calculation (AC1 Test Vector)
    // Uniswap V3 (Pool A): fA = 0.05%, PA = 2495, LA = 15,000,000
    let l_a = 15_000_000.0;
    let p_a = 2495.0;
    let f_a = 0.0005;
    let (y_a, x_a) = virtual_reserves_from_clmm(l_a, p_a);

    // Aerodrome Slipstream (Pool B): fB = 0.01%, PB = 2505, LB = 12,000,000
    let l_b = 12_000_000.0;
    let p_b = 2505.0;
    let f_b = 0.0001;
    let (y_b, x_b) = virtual_reserves_from_clmm(l_b, p_b);

    // 4. Closed-Form Optimal Sizing on Virtual Reserves
    let dx_opt = solve_2pool_optimal_f64(x_a, y_a, x_b, y_b, f_a, f_b, 0.0);
    assert!(dx_opt.is_some());
    let dx = dx_opt.unwrap();

    // Verify verbatim AC1 vector: dx* = 566,245.53 USDC (+- 0.05)
    let rounded_dx = (dx * 100.0).round() / 100.0;
    assert!(
        (rounded_dx - 566245.53).abs() <= 0.05,
        "dx* {} must match 566245.53 verbatim",
        rounded_dx
    );

    let (dx_out, gross_profit, _) = calculate_2pool_profit(dx, x_a, y_a, x_b, y_b, f_a, f_b, 0.0);
    let rounded_gross = (gross_profit * 100.0).round() / 100.0;
    assert_eq!(rounded_gross, 963.40, "Gross profit must match 963.40 USDC verbatim");

    // 5. Zero-Fee Flash Loan Routing: 566,245 USDC borrowed
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 1_000_000_000_000, // 1M USDC (6 decimals)
        balancer_liquidity: 1_000_000_000_000,
        univ4_liquidity: 1_000_000_000_000,
        aave_liquidity: 1_000_000_000_000,
    };
    let (provider, fee_pips) = route_flash_loan((dx * 1e6) as u128, &snapshot).unwrap();
    assert_eq!(provider, FlashLoanProvider::MorphoBlue);
    assert_eq!(fee_pips, 0);

    // 6. Encode 100-byte calldata
    let params = ExecutionParams {
        weth_amount_to_first_market: (dx * 1e6) as u64,
        balance_before: 500_000,
        gas_cost: 150_000,
        target1: Address::from([0x33; 20]),
        target2: Address::from([0x44; 20]),
        token0_amount_out: (dx_out * 1e6) as u128,
        token0_amount_out_index: 0,
        token1_amount_out: 0,
        token1_amount_out_index: 1,
    };
    let calldata = encode_oracle_calldata(&params);
    assert_eq!(calldata.len(), 100);

    let decoded = decode_oracle_calldata(&calldata).unwrap();
    assert_eq!(decoded, params);
}
