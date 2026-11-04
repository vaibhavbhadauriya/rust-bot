//! Tier 4 Real-World Scenario 1: Arbitrum WETH-USDC-USDT Triangular Arbitrage
//! Features Exercised: F1, F3, F7, F8, F10, F13, F14
//! Authoritative source: TEST_INFRA.md line 43 and ORIGINAL_REQUEST.md.

use crate::common::*;
use alloy_primitives::Address;

#[test]
fn test_scenario1_arbitrum_triangular_arbitrage_end_to_end() {
    // 1. Ingest Arbitrum Sequencer Feed message with monotonic sequence number
    let dedup = SequencerDeduplicator::new(10_000);
    assert!(dedup.check_and_update(10_001));

    // Nested batch unpacking delivers swap transaction targeting WETH-USDC pool
    let tx = MockTx {
        tx_type: ArbitrumTxType::Eip1559,
        pool_target: [0x11; 20],
        calldata_len: 128,
    };
    let batch = BatchPayload::NestedBatch(vec![BatchPayload::Direct(tx)]);
    let unpacked = unpack_arbitrum_batch(&batch);
    assert_eq!(unpacked.len(), 1);

    // 2. Targeted DFS identifies 3-hop triangular cycle: WETH -> USDC -> USDT -> WETH
    // Pool 1 (WETH/USDC): x1 = 1,000 WETH, y1 = 2,500,000 USDC, fee = 0.05% (500 PIPs)
    let x1 = 1_000.0;
    let y1 = 2_500_000.0;
    let g1 = 0.9995;

    // Pool 2 (USDC/USDT): x2 = 5,000,000 USDC, y2 = 5,020,000 USDT, fee = 0.01% (100 PIPs)
    let x2 = 5_000_000.0;
    let y2 = 5_020_000.0;
    let g2 = 0.9999;

    // Pool 3 (USDT/WETH): x3 = 3,000,000 USDT, y3 = 1,220 WETH, fee = 0.05% (500 PIPs)
    let x3 = 3_000_000.0;
    let y3 = 1_220.0;
    let g3 = 0.9995;

    // 3. Virtual Pool Reduction: Compose Pool 1 and Pool 2 into Virtual Pool (Ex, Ey)
    let (ex, ey, g12) = reduce_virtual_pool_f64(x1, y1, g1, x2, y2, g2).expect("Virtual reduction failed");
    assert!(ex > 0.0 && ey > 0.0);

    // 4. Closed-form sizing on (Ex, Ey) against Pool 3
    let f12 = 1.0 - g12;
    let f3 = 1.0 - g3;
    let dx_opt = solve_2pool_optimal_f64(ex, ey, y3, x3, f12, f3, 0.0);
    assert!(dx_opt.is_some(), "3-hop opportunity must be profitable");
    let dx = dx_opt.unwrap();
    assert!(dx > 0.0, "Optimal input size must be positive");

    let (dx_out, gross_profit, _) = calculate_2pool_profit(dx, ex, ey, y3, x3, f12, f3, 0.0);
    assert!(gross_profit > 0.0, "Triangular trade must deliver positive gross profit");
    assert!(dx_out > dx);

    // 5. Zero-Fee Flash Loan Routing: Borrow dx WETH via Morpho Blue
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 100_000_000_000_000_000_000, // 100 ETH
        balancer_liquidity: 50_000_000_000_000_000_000,
        univ4_liquidity: 50_000_000_000_000_000_000,
        aave_liquidity: 100_000_000_000_000_000_000,
    };
    let (provider, fee_pips) = route_flash_loan((dx * 1e18) as u128, &snapshot).unwrap();
    assert_eq!(provider, FlashLoanProvider::MorphoBlue);
    assert_eq!(fee_pips, 0, "Morpho Blue provides 0% fee flash loan");

    // 6. 100-Byte Calldata Encoding
    let params = ExecutionParams {
        weth_amount_to_first_market: (dx * 1e18) as u64,
        balance_before: 1_000_000_000,
        gas_cost: 200_000,
        target1: Address::from([0x11; 20]),
        target2: Address::from([0x22; 20]),
        token0_amount_out: 100_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: (dx_out * 1e18) as u128,
        token1_amount_out_index: 1,
    };
    let calldata = encode_oracle_calldata(&params);
    assert_eq!(calldata.len(), 100);

    // 7. Verify In-Memory State Simulation
    let decoded = decode_oracle_calldata(&calldata).unwrap();
    assert_eq!(decoded, params);
}
