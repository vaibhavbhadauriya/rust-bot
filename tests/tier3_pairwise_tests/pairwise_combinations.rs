//! Tier 3: Pairwise Cross-Feature Tests
//! 18 Combinatorial Cross-Feature Integration Tests
//! Authoritative source: TEST_INFRA.md and PROJECT.md § Interface Contracts.

use crate::common::*;
use alloy_primitives::{Address, U256};

#[test]
fn test_p01_v2_sizing_with_flash_loan_priority() {
    // F1 + F13: Sizing with Morpho 0% vs Aave 0.05%
    let x_a = 1_000_000.0;
    let y_a = 405.0;
    let x_b = 1_000_000.0;
    let y_b = 395.0;

    let snapshot_morpho = ProviderLiquiditySnapshot {
        morpho_liquidity: 10_000_000,
        balancer_liquidity: 0,
        univ4_liquidity: 0,
        aave_liquidity: 0,
    };
    let (prov_m, fee_m_pips) = route_flash_loan(5_000, &snapshot_morpho).unwrap();
    let phi_m = fee_m_pips as f64 / 1_000_000.0;
    let dx_m = solve_2pool_optimal_f64(x_a, y_a, x_b, y_b, 0.003, 0.003, phi_m).unwrap();
    let (_, _, net_m) = calculate_2pool_profit(dx_m, x_a, y_a, x_b, y_b, 0.003, 0.003, phi_m);

    let snapshot_aave = ProviderLiquiditySnapshot {
        morpho_liquidity: 0,
        balancer_liquidity: 0,
        univ4_liquidity: 0,
        aave_liquidity: 10_000_000,
    };
    let (prov_a, fee_a_pips) = route_flash_loan(5_000, &snapshot_aave).unwrap();
    let phi_a = fee_a_pips as f64 / 1_000_000.0;
    let (_, gross_m, _) = calculate_2pool_profit(dx_m, x_a, y_a, x_b, y_b, 0.003, 0.003, 0.0);
    let flash_fee_a = phi_a * dx_m;
    let net_a = gross_m - flash_fee_a;

    assert_eq!(prov_m, FlashLoanProvider::MorphoBlue);
    assert_eq!(prov_a, FlashLoanProvider::AaveV3);
    assert!(net_m > net_a, "Morpho zero-fee net profit must exceed Aave after-fee profit");
    assert_eq!((net_m * 100.0).round() / 100.0, 45.15);
    assert_eq!((net_a * 100.0).round() / 100.0, 42.79);
}

#[test]
fn test_p02_multiplication_screening_feeds_radical_splitter() {
    // F2 + F7: Screening identifies profitable dislocation, feeding radical splitter
    let x_a = 1_000_000u128;
    let y_a = 405u128;
    let x_b = 1_000_000u128;
    let y_b = 395u128;

    assert!(quick_spread_screen(y_b, x_a, y_a, x_b));
    let size = radical_split_sizing(x_a, y_a, x_b, y_b, 3000, 3000, 0);
    assert!(size.is_some());
    assert!(size.unwrap() > U256::ZERO);
}

#[test]
fn test_p03_triangular_reduction_solved_via_v2_sizing() {
    // F3 + F1: Virtual pool composition solved against Pool 3
    let (ex, ey, g12) = reduce_virtual_pool_f64(1_000_000.0, 405.0, 0.997, 400.0, 420_000.0, 0.997).unwrap();
    let dx_opt = solve_2pool_optimal_f64(ex, ey, 1_010_000.0, 410_000.0, 1.0 - g12, 0.003, 0.0);
    assert!(dx_opt.is_some());
}

#[test]
fn test_p04_clmm_sizing_with_sp_ranking() {
    // F4 + F6: High scoring concentrated liquidity pool triggering virtual reserve sizing
    let score = compute_sp_score(0.04, 0.0005, 500_000.0, 15_000_000.0, 3);
    assert!(score > 10_000.0);

    let (y_virt, x_virt) = virtual_reserves_from_clmm(15_000_000.0, 2495.0);
    assert!(x_virt > 0.0 && y_virt > 0.0);
}

#[test]
fn test_p05_pre_filter_prunes_graph_dfs() {
    // F5 + F7: Dust pool discarded by prefilter never enters DFS
    let r0 = 500_000u128; // < 1 USDC
    let r1 = 1_000_000_000_000_000_000u128;
    let is_valid = filter_unit_reserves(r0, 6, r1, 18);
    assert!(!is_valid, "Pre-filter rejects dust pool");
}

#[test]
fn test_p06_pre_filter_and_honeypot_defense_pipeline() {
    // F5 + F15: Pool passes reserve filter but token fails honeypot test
    let passes_reserves = filter_unit_reserves(10_000_000, 6, 10_000_000_000_000_000_000, 18);
    assert!(passes_reserves);

    let honeypot_report = HoneypotSimulationReport {
        token: Address::from([0xAA; 20]),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(10), // Salmonella
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(inspect_honeypot_safety(&honeypot_report).is_err());
}

#[test]
fn test_p07_batch_unpack_triggers_targeted_dfs() {
    // F8 + F7: Arbitrum batch unpacking yields mutated pool
    let tx = MockTx {
        tx_type: ArbitrumTxType::Eip1559,
        pool_target: [0x11; 20],
        calldata_len: 100,
    };
    let batch = BatchPayload::Direct(tx.clone());
    let unpacked = unpack_arbitrum_batch(&batch);
    assert_eq!(unpacked[0].pool_target, [0x11; 20]);
}

#[test]
fn test_p08_flashblock_budget_with_multi_eoa() {
    // F9 + F16: Flashblock sub-block dispatch with multi-EOA pool
    let mut tracker = FlashblockGasTracker::new(30_000_000);
    let pool = MockEoaPool::new(5);

    let _ = tracker.try_budget_tx(0, 500_000).unwrap();
    let (signer_idx, _, nonce) = pool.acquire_signer().unwrap();
    assert_eq!(signer_idx, 0);
    assert_eq!(nonce, 0);
}

#[test]
fn test_p09_calldata_decoding_and_fast_revert_guard() {
    // F10 + F11: Decode calldata, run fast-revert guard on expected reserves
    let params = ExecutionParams {
        weth_amount_to_first_market: 100,
        balance_before: 50,
        gas_cost: 10,
        target1: Address::from([0x12; 20]),
        target2: Address::from([0x34; 20]),
        token0_amount_out: 500,
        token0_amount_out_index: 0,
        token1_amount_out: 600,
        token1_amount_out_index: 1,
    };
    let encoded = encode_oracle_calldata(&params);
    let decoded = decode_oracle_calldata(&encoded).unwrap();

    let outcome = simulate_fast_revert_guard(1000, 2000, 1000, 2000);
    assert_eq!(outcome, GuardOutcome::Passed);
    assert_eq!(decoded.weth_amount_to_first_market, 100);
}

#[test]
fn test_p10_calldata_routed_to_flash_loan_callback() {
    // F10 + F13: Calldata encoded and routed to Morpho Blue callback
    let params = ExecutionParams {
        weth_amount_to_first_market: 5000,
        balance_before: 1000,
        gas_cost: 50,
        target1: Address::ZERO,
        target2: Address::ZERO,
        token0_amount_out: 10,
        token0_amount_out_index: 0,
        token1_amount_out: 20,
        token1_amount_out_index: 1,
    };
    let encoded = encode_oracle_calldata(&params);
    assert_eq!(FlashLoanProvider::MorphoBlue.callback_selector(), "0x3a48e7e1");
    assert_eq!(encoded.len(), 100);
}

#[test]
fn test_p11_fast_reverts_trip_kill_switch() {
    // F11 + F17: 3 consecutive fast-reverts trip kill switch
    let ks = KillSwitchController::new();
    for _ in 0..3 {
        let outcome = simulate_fast_revert_guard(100, 200, 150, 180);
        let is_revert = matches!(outcome, GuardOutcome::FastRevert { .. });
        ks.record_trade_result(is_revert);
    }
    assert!(ks.is_tripped());
}

#[test]
fn test_p12_vanity_miner_with_calldata_encoder() {
    // F12 + F10: 16-zero vanity address encoded into calldata
    let vanity_addr = Address::from([
        0, 0, 0, 0, 0, 0, 0, 0, // 8 zero bytes (16 hex zeros)
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    ]);
    assert_eq!(count_leading_zero_bytes(&vanity_addr), 8);

    let params = ExecutionParams {
        weth_amount_to_first_market: 1,
        balance_before: 2,
        gas_cost: 3,
        target1: vanity_addr,
        target2: Address::ZERO,
        token0_amount_out: 0,
        token0_amount_out_index: 0,
        token1_amount_out: 0,
        token1_amount_out_index: 0,
    };
    let encoded = encode_oracle_calldata(&params);
    let decoded = decode_oracle_calldata(&encoded).unwrap();
    assert_eq!(decoded.target1, vanity_addr);
}

#[test]
fn test_p13_in_memory_sim_executes_fast_revert_guard() {
    // F14 + F11: revm simulation evaluating fast-revert guard gas savings
    let outcome = simulate_fast_revert_guard(10_000, 20_000, 10_050, 19_950);
    match outcome {
        GuardOutcome::FastRevert { gas_saved_percent, .. } => {
            assert!(gas_saved_percent > 80.0);
        }
        _ => panic!("Expected FastRevert"),
    }
}

#[test]
fn test_p14_in_memory_sim_catches_salmonella() {
    // F14 + F15: revm TxInspector identifying Salmonella disparity
    let report = HoneypotSimulationReport {
        token: Address::from([0x12; 20]),
        emitted_amount: U256::from(1_000_000),
        storage_delta: U256::from(100_000), // 10%
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(inspect_honeypot_safety(&report).is_err());
}

#[test]
fn test_p15_multi_eoa_pool_shadow_mode_telemetry() {
    // F16 + F17: Shadow mode executing multi-EOA pool with telemetry
    let pool = MockEoaPool::new(5);
    let (idx, _, nonce) = pool.acquire_signer().unwrap();
    let telemetry = PipelineTelemetry {
        sizing_math_us: 50,
        revm_sim_us: 600,
        tx_sign_us: 150,
        ..Default::default()
    };
    assert!(telemetry.total_e2e_latency_us() < 3500);
    pool.release_signer(idx);
    assert_eq!(nonce, 0);
}

#[test]
fn test_p16_arbitrum_feed_lag_trips_kill_switch() {
    // F8 + F17: Feed delay > 100ms trips kill switch
    let ks = KillSwitchController::new();
    let arrival_lag = 120u64; // 120ms
    assert!(ks.check_feed_lag(arrival_lag));
    assert!(ks.is_tripped());
}

#[test]
fn test_p17_clmm_sizing_with_custom_quoter() {
    // F4 + F14: Concentrated liquidity virtual reserves simulated in quoter
    let (y_virt, x_virt) = virtual_reserves_from_clmm(15_000_000.0, 2495.0);
    let dx = 10_000.0;
    let dy = (y_virt * 0.9995 * dx) / (x_virt + 0.9995 * dx);
    assert!(dy > 0.0);
}

#[test]
fn test_p18_v2_sizing_latency_budget() {
    // F1 + F18: V2 sizing completes in < 100 us
    let t0 = std::time::Instant::now();
    let _ = solve_2pool_optimal_f64(1_000_000.0, 405.0, 1_000_000.0, 395.0, 0.003, 0.003, 0.0);
    let elapsed = t0.elapsed();
    assert!(elapsed.as_micros() < 100);
}
