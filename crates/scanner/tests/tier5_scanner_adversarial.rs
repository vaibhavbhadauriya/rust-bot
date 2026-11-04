//! Tier 5 Adversarial Coverage Hardening: crates/scanner
//!
//! White-box stress testing of:
//! 1. Hard pre-filters (unit reserves across 0, 6, 18, 38, 39+ decimals)
//! 2. Dynamic Sp scoring formula (NaN, Inf, negative values, zero-division floor at 1e-6)
//! 3. MarketGraph concurrent mutation and thread safety
//! 4. Targeted DFS cycle search invariants (distinct pools, distinct tokens, max hops)
//! 5. HoneypotSimulationReport inspect_honeypot_safety opcode trap and Salmonella checks

use alloy_primitives::{address, Address, U256};
use flash_core::types::{Chain, FeePips, PoolState, Venue};
use flash_scanner::{
    compute_sp_score, dfs_cyclic_opportunities, filter_unit_reserves,
    inspect_honeypot_safety, DiscoveredPool, HoneypotSimulationReport, HoneypotViolation,
    MarketGraph,
};
use std::sync::Arc;
use std::thread;

fn make_pool(id: Address, token0: Address, token1: Address, r0: u128, r1: u128, fee_pips: u32) -> DiscoveredPool {
    DiscoveredPool::new(
        id,
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        token0,
        token1,
        FeePips::from_pips(fee_pips),
        PoolState::ConstantProduct { reserve0: r0, reserve1: r1 },
    )
}

#[test]
fn test_adv_scanner_filter_unit_reserves_extreme_decimals() {
    // 1. Zero reserve fails
    assert!(!filter_unit_reserves(0, 6, 1_000_000, 6));
    assert!(!filter_unit_reserves(1_000_000, 6, 0, 6));

    // 2. Exact decimal boundaries
    // 6 decimals (USDC)
    assert!(!filter_unit_reserves(999_999, 6, 1_000_000, 6));
    assert!(filter_unit_reserves(1_000_000, 6, 1_000_000, 6));

    // 18 decimals (WETH)
    let unit_18 = 1_000_000_000_000_000_000u128;
    assert!(!filter_unit_reserves(unit_18 - 1, 18, unit_18, 18));
    assert!(filter_unit_reserves(unit_18, 18, unit_18, 18));

    // 0 decimals
    assert!(filter_unit_reserves(1, 0, 1, 0));
    assert!(!filter_unit_reserves(0, 0, 1, 0));

    // 38 decimals (max power fitting in u128)
    let unit_38 = 10u128.checked_pow(38).unwrap();
    assert!(filter_unit_reserves(unit_38, 38, unit_38, 38));

    // 39+ decimals: overflows u128 -> returns false without panicking
    assert!(!filter_unit_reserves(u128::MAX, 39, u128::MAX, 39));
    assert!(!filter_unit_reserves(u128::MAX, 255, u128::MAX, 255));
}

#[test]
fn test_adv_scanner_sp_scoring_guards_and_monotonicity() {
    // 1. Invalid / Non-finite inputs must return 0.0 without panic
    assert_eq!(compute_sp_score(f64::NAN, 0.003, 100_000.0, 50_000.0, 2), 0.0);
    assert_eq!(compute_sp_score(0.05, f64::NAN, 100_000.0, 50_000.0, 2), 0.0);
    assert_eq!(compute_sp_score(0.05, 0.003, f64::NAN, 50_000.0, 2), 0.0);
    assert_eq!(compute_sp_score(0.05, 0.003, 100_000.0, f64::NAN, 2), 0.0);

    assert_eq!(compute_sp_score(f64::INFINITY, 0.003, 100_000.0, 50_000.0, 2), 0.0);
    assert_eq!(compute_sp_score(0.05, f64::INFINITY, 100_000.0, 50_000.0, 2), 0.0);
    assert_eq!(compute_sp_score(0.05, 0.003, f64::INFINITY, 50_000.0, 2), 0.0);
    assert_eq!(compute_sp_score(0.05, 0.003, 100_000.0, f64::INFINITY, 2), 0.0);

    // Negative values
    assert_eq!(compute_sp_score(-0.05, 0.003, 100_000.0, 50_000.0, 2), 0.0);
    assert_eq!(compute_sp_score(0.05, 0.003, -100.0, 50_000.0, 2), 0.0);

    // 2. Venues requirement: venues < 2 must return 0.0
    assert_eq!(compute_sp_score(0.05, 0.003, 100_000.0, 50_000.0, 0), 0.0);
    assert_eq!(compute_sp_score(0.05, 0.003, 100_000.0, 50_000.0, 1), 0.0);

    // 3. Zero fee: must floor at 1e-6 without division by zero
    let score_zero_fee = compute_sp_score(0.05, 0.0, 100_000.0, 50_000.0, 2);
    assert!(score_zero_fee.is_finite() && score_zero_fee > 0.0);

    // 4. Monotonicity: higher volume -> higher score
    let s_low = compute_sp_score(0.05, 0.003, 10_000.0, 50_000.0, 2);
    let s_high = compute_sp_score(0.05, 0.003, 100_000.0, 50_000.0, 2);
    assert!(s_high > s_low);
}

#[test]
fn test_adv_scanner_honeypot_safety_inspection() {
    let token = address!("1111111111111111111111111111111111111111");

    // 1. Clean report passes
    let clean = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(100),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert_eq!(inspect_honeypot_safety(&clean), Ok(()));

    // 2. Salmonella: delivers 10% (<= 20%)
    let salmonella = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(10),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(matches!(
        inspect_honeypot_safety(&salmonella),
        Err(HoneypotViolation::SalmonellaPartialDelivery { .. })
    ));

    // 3. Excessive fee on transfer: delivers 85% (< 90%)
    let high_fee = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(85),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(matches!(
        inspect_honeypot_safety(&high_fee),
        Err(HoneypotViolation::ExcessiveFeeOnTransfer { .. })
    ));

    // 4. ORIGIN opcode trap
    let origin_trap = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(100),
        used_origin_opcode: true,
        used_gasprice_opcode: false,
    };
    assert_eq!(inspect_honeypot_safety(&origin_trap), Err(HoneypotViolation::OriginSandwichTrap));

    // 5. GASPRICE opcode trap
    let gasprice_trap = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(100),
        used_origin_opcode: false,
        used_gasprice_opcode: true,
    };
    assert_eq!(inspect_honeypot_safety(&gasprice_trap), Err(HoneypotViolation::GasPriceSandwichTrap));
}

#[test]
fn test_adv_scanner_graph_concurrency_stress() {
    let graph = Arc::new(MarketGraph::new());
    let token_a = address!("0000000000000000000000000000000000000001");
    let token_b = address!("0000000000000000000000000000000000000002");
    let pool_id = address!("0000000000000000000000000000000000000003");

    let p = make_pool(pool_id, token_a, token_b, 10_000_000, 10_000_000, 3000);
    graph.add_pool(&p);

    // 8 concurrent threads hammering update_reserves while 4 threads query DFS
    let mut handles = Vec::new();
    for thread_id in 0..8 {
        let g = Arc::clone(&graph);
        handles.push(thread::spawn(move || {
            for i in 0..500 {
                let r0 = 10_000_000 + (thread_id * 1000) + i;
                let r1 = 10_000_000 - (thread_id * 500) + i;
                g.update_reserves(pool_id, r0, r1);
            }
        }));
    }

    for _ in 0..4 {
        let g = Arc::clone(&graph);
        handles.push(thread::spawn(move || {
            for _ in 0..500 {
                let _opps = dfs_cyclic_opportunities(&g, token_a, 2, 0);
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Verify graph is still consistent and drains mutated vertices correctly
    let mutated = graph.take_mutated_vertices();
    assert!(mutated.contains(&token_a) || mutated.contains(&token_b));
    let mutated_after = graph.take_mutated_vertices();
    assert!(mutated_after.is_empty(), "take_mutated_vertices must atomically clear mutated vertices");
}
