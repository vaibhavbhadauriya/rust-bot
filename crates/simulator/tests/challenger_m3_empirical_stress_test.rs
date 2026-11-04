//! Adversarial Empirical Stress Test Suite for Milestone M3 (Requirement R3)
//!
//! Evaluates:
//! 1. When post_balance <= trade_size, simulate_arbitrage_route MUST return Err(SimError::Unprofitable).
//! 2. When gas cost in token exceeds actual profit, it MUST return Err(SimError::Unprofitable).
//! 3. Gas consumed by 2-hop routes is >= 150,000 (~180,000) and 3-hop routes is >= 220,000 (~250,000).
//! 4. Decimal normalization on USDC (6 dec), WBTC (8 dec), WETH (18 dec) across extreme & microscopic values.
//! 5. GasPrices::total_gas_cost_wei arithmetic safety without overflow or underflow.

use alloy_primitives::{address, Address, U256};
use flash_core::constants::{arbitrum, base};
use flash_core::types::{Chain, GasPrices, Venue};
use flash_math::TradeSizeOutput;
use flash_scanner::dfs::RouteCandidate;
use flash_scanner::graph::{DirectedEdge, MarketGraph};
use flash_simulator::cache::BytecodeCache;
use flash_simulator::engine::SimulationEngine;
use flash_simulator::types::SimError;
use std::sync::Arc;

const USDC_ADDR: Address = arbitrum::USDC_NATIVE;
const WETH_ADDR: Address = arbitrum::WETH;
const WBTC_ADDR: Address = arbitrum::WBTC;

fn create_test_engine(chain: Chain, cache_subdir: &str) -> SimulationEngine {
    let cache_dir = format!("./target/challenger_m3_cache_{}", cache_subdir);
    let cache = Arc::new(BytecodeCache::new(&cache_dir, 100));
    SimulationEngine::new(chain, cache)
}

// ==============================================================================
// 1. Post-balance <= trade_size MUST return Err(SimError::Unprofitable)
// ==============================================================================

#[test]
fn test_adversarial_post_balance_less_or_equal_to_trade_size_rejection() {
    let engine = create_test_engine(Chain::ArbitrumOne, "unprofitable_delta");
    let pool1 = address!("1000000000000000000000000000000000000001");
    let pool2 = address!("2000000000000000000000000000000000000002");
    let trade_size = U256::from(1_000_000_000_000_000_000u128); // 1.0 ETH

    // Test Case A: Exactly break-even (output == trade_size) -> zero profit
    let route_breakeven = RouteCandidate {
        borrow_token: WETH_ADDR,
        hops: vec![
            DirectedEdge {
                pool_id: pool1,
                venue: Venue::UniswapV2,
                token_in: WETH_ADDR,
                token_out: USDC_ADDR,
                reserve_in: 100_000_000_000_000_000_000,
                reserve_out: 300_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
            DirectedEdge {
                pool_id: pool2,
                venue: Venue::UniswapV2,
                token_in: USDC_ADDR,
                token_out: WETH_ADDR,
                reserve_in: 300_000_000_000,
                reserve_out: 100_000_000_000_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
        ],
        sizing: TradeSizeOutput {
            optimal_input: trade_size,
            expected_intermediate: U256::from(3_000_000_000u64),
            expected_output: trade_size, // 1.0 ETH back -> 0 gross profit
            gross_profit: U256::ZERO,
            flash_fee: U256::ZERO,
            net_profit: U256::ZERO,
        },
        net_profit: U256::ZERO,
    };

    let res_breakeven = engine.simulate_arbitrage_route(&route_breakeven, trade_size);
    assert!(
        res_breakeven.is_err(),
        "Break-even trade must return Err(SimError::Unprofitable)"
    );
    match res_breakeven.unwrap_err() {
        SimError::Unprofitable { profit, gas_cost } => {
            assert_eq!(profit, U256::ZERO);
            assert!(gas_cost > U256::ZERO);
        }
        err => panic!("Expected SimError::Unprofitable, got {:?}", err),
    }

    // Test Case B: Direct loss (output < trade_size) -> 0.9 ETH return
    let route_loss = RouteCandidate {
        borrow_token: WETH_ADDR,
        hops: vec![
            DirectedEdge {
                pool_id: pool1,
                venue: Venue::UniswapV2,
                token_in: WETH_ADDR,
                token_out: USDC_ADDR,
                reserve_in: 100_000_000_000_000_000_000,
                reserve_out: 300_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
            DirectedEdge {
                pool_id: pool2,
                venue: Venue::UniswapV2,
                token_in: USDC_ADDR,
                token_out: WETH_ADDR,
                reserve_in: 300_000_000_000,
                reserve_out: 100_000_000_000_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
        ],
        sizing: TradeSizeOutput {
            optimal_input: trade_size,
            expected_intermediate: U256::from(2_700_000_000u64),
            expected_output: U256::from(900_000_000_000_000_000u128), // 0.9 ETH
            gross_profit: U256::ZERO,
            flash_fee: U256::ZERO,
            net_profit: U256::ZERO,
        },
        net_profit: U256::ZERO,
    };

    let res_loss = engine.simulate_arbitrage_route(&route_loss, trade_size);
    assert!(res_loss.is_err(), "Net loss trade must be rejected");
    match res_loss.unwrap_err() {
        SimError::Unprofitable { profit, gas_cost } => {
            assert_eq!(profit, U256::ZERO);
            assert!(gas_cost > U256::ZERO);
        }
        err => panic!("Expected SimError::Unprofitable, got {:?}", err),
    }

    // Test Case C: Fictitious sizing claim with 0 real return (facade elimination check)
    let route_fictitious = RouteCandidate {
        borrow_token: WETH_ADDR,
        hops: vec![
            DirectedEdge {
                pool_id: pool1,
                venue: Venue::UniswapV2,
                token_in: WETH_ADDR,
                token_out: USDC_ADDR,
                reserve_in: 100_000_000_000_000_000_000,
                reserve_out: 300_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
            DirectedEdge {
                pool_id: pool2,
                venue: Venue::UniswapV2,
                token_in: USDC_ADDR,
                token_out: WETH_ADDR,
                reserve_in: 300_000_000_000,
                reserve_out: 100_000_000_000_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
        ],
        sizing: TradeSizeOutput {
            optimal_input: trade_size,
            expected_intermediate: U256::from(3_000_000_000u64),
            expected_output: U256::ZERO, // 0 returned in EVM
            gross_profit: U256::from(100_000_000_000_000_000u128), // Fictitious theoretical profit claimed
            flash_fee: U256::ZERO,
            net_profit: U256::from(100_000_000_000_000_000u128),
        },
        net_profit: U256::from(100_000_000_000_000_000u128),
    };

    let res_fictitious = engine.simulate_arbitrage_route(&route_fictitious, trade_size);
    assert!(
        res_fictitious.is_err(),
        "Must NOT substitute theoretical sizing profit when real balance delta is zero"
    );
    match res_fictitious.unwrap_err() {
        SimError::Unprofitable { profit, .. } => {
            assert_eq!(profit, U256::ZERO, "Profit must be derived strictly from EVM balance delta");
        }
        err => panic!("Expected SimError::Unprofitable, got {:?}", err),
    }
}

// ==============================================================================
// 2. Gas cost in token exceeds actual profit -> Err(SimError::Unprofitable)
// ==============================================================================

#[test]
fn test_adversarial_profit_below_gas_cost_strict_rejection() {
    let engine = create_test_engine(Chain::ArbitrumOne, "profit_below_gas");
    let pool1 = address!("1000000000000000000000000000000000000001");
    let pool2 = address!("2000000000000000000000000000000000000002");
    let trade_size = U256::from(1_000_000_000_000_000_000u128); // 1.0 ETH

    // Test Case A: Microscopic profit (10,000 wei), whereas gas cost is ~50,000,000,000,000 wei (~0.00005 ETH)
    let tiny_profit = U256::from(10_000u128);
    let route_micro_profit = RouteCandidate {
        borrow_token: WETH_ADDR,
        hops: vec![
            DirectedEdge {
                pool_id: pool1,
                venue: Venue::UniswapV2,
                token_in: WETH_ADDR,
                token_out: USDC_ADDR,
                reserve_in: 100_000_000_000_000_000_000,
                reserve_out: 300_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
            DirectedEdge {
                pool_id: pool2,
                venue: Venue::UniswapV2,
                token_in: USDC_ADDR,
                token_out: WETH_ADDR,
                reserve_in: 300_000_000_000,
                reserve_out: 100_000_000_000_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
        ],
        sizing: TradeSizeOutput {
            optimal_input: trade_size,
            expected_intermediate: U256::from(3_000_000_000u64),
            expected_output: trade_size + tiny_profit,
            gross_profit: tiny_profit,
            flash_fee: U256::ZERO,
            net_profit: tiny_profit,
        },
        net_profit: tiny_profit,
    };

    let res_micro = engine.simulate_arbitrage_route(&route_micro_profit, trade_size);
    assert!(
        res_micro.is_err(),
        "Micro-profit below gas cost must return Err(SimError::Unprofitable)"
    );
    match res_micro.unwrap_err() {
        SimError::Unprofitable { profit, gas_cost } => {
            assert_eq!(profit, tiny_profit);
            assert!(gas_cost > profit, "Gas cost {} must exceed profit {}", gas_cost, profit);
        }
        err => panic!("Expected SimError::Unprofitable, got {:?}", err),
    }

    // Test Case B: Dynamic Gas Spike flips profitable trade into unprofitable
    // Initial moderate profit of 0.001 ETH (1_000_000_000_000_000 wei)
    let moderate_profit = U256::from(1_000_000_000_000_000u128);
    let route_moderate = RouteCandidate {
        borrow_token: WETH_ADDR,
        hops: vec![
            DirectedEdge {
                pool_id: pool1,
                venue: Venue::UniswapV2,
                token_in: WETH_ADDR,
                token_out: USDC_ADDR,
                reserve_in: 100_000_000_000_000_000_000,
                reserve_out: 300_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
            DirectedEdge {
                pool_id: pool2,
                venue: Venue::UniswapV2,
                token_in: USDC_ADDR,
                token_out: WETH_ADDR,
                reserve_in: 300_000_000_000,
                reserve_out: 100_000_000_000_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
        ],
        sizing: TradeSizeOutput {
            optimal_input: trade_size,
            expected_intermediate: U256::from(3_000_000_000u64),
            expected_output: trade_size + moderate_profit,
            gross_profit: moderate_profit,
            flash_fee: U256::ZERO,
            net_profit: moderate_profit,
        },
        net_profit: moderate_profit,
    };

    // Low gas environment (gas cost ~ 52,000,000,000,000 wei = ~0.000052 ETH < 0.001 ETH)
    engine.set_gas_prices(GasPrices {
        l2_base_fee_wei: 100_000_000,
        l2_priority_fee_wei: 10_000_000,
        l1_data_fee_wei: 2_000_000_000_000,
    });
    let res_low_gas = engine.simulate_arbitrage_route(&route_moderate, trade_size);
    assert!(
        res_low_gas.is_ok(),
        "Trade must succeed when profit (0.001 ETH) exceeds low gas cost"
    );

    // Spiked gas environment (L1 fee spikes to 0.005 ETH = 5_000_000_000_000_000 wei > 0.001 ETH profit)
    engine.set_gas_prices(GasPrices {
        l2_base_fee_wei: 100_000_000,
        l2_priority_fee_wei: 10_000_000,
        l1_data_fee_wei: 5_000_000_000_000_000,
    });
    let res_spiked_gas = engine.simulate_arbitrage_route(&route_moderate, trade_size);
    assert!(
        res_spiked_gas.is_err(),
        "Trade must immediately reject when L1 gas spike exceeds profit"
    );
    match res_spiked_gas.unwrap_err() {
        SimError::Unprofitable { profit, gas_cost } => {
            assert_eq!(profit, moderate_profit);
            assert!(gas_cost > profit);
        }
        err => panic!("Expected SimError::Unprofitable, got {:?}", err),
    }
}

// ==============================================================================
// 3. Realistic Gas Consumption: 2-Hop >= 150,000 and 3-Hop >= 220,000
// ==============================================================================

#[test]
fn test_adversarial_realistic_gas_consumption_2hop_and_3hop() {
    let engine = create_test_engine(Chain::ArbitrumOne, "gas_scaling");
    let pool1 = address!("1000000000000000000000000000000000000001");
    let pool2 = address!("2000000000000000000000000000000000000002");
    let pool3 = address!("3000000000000000000000000000000000000003");

    let token_b = USDC_ADDR;
    let token_c = WBTC_ADDR;
    let trade_size = U256::from(1_000_000_000_000_000_000u128); // 1.0 ETH

    // --- 2-Hop Route ---
    let route_2hop = RouteCandidate {
        borrow_token: WETH_ADDR,
        hops: vec![
            DirectedEdge {
                pool_id: pool1,
                venue: Venue::UniswapV2,
                token_in: WETH_ADDR,
                token_out: token_b,
                reserve_in: 100_000_000_000_000_000_000,
                reserve_out: 400_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
            DirectedEdge {
                pool_id: pool2,
                venue: Venue::UniswapV2,
                token_in: token_b,
                token_out: WETH_ADDR,
                reserve_in: 400_000_000_000,
                reserve_out: 100_000_000_000_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
        ],
        sizing: TradeSizeOutput {
            optimal_input: trade_size,
            expected_intermediate: U256::from(3_000_000_000u64),
            expected_output: U256::from(1_050_000_000_000_000_000u128), // 1.05 ETH
            gross_profit: U256::from(50_000_000_000_000_000u128),
            flash_fee: U256::ZERO,
            net_profit: U256::from(50_000_000_000_000_000u128),
        },
        net_profit: U256::from(50_000_000_000_000_000u128),
    };

    let outcome_2hop = engine
        .simulate_arbitrage_route(&route_2hop, trade_size)
        .expect("2-hop simulation must succeed");

    assert!(
        outcome_2hop.gas_used >= 150_000,
        "2-hop gas used ({}) must be >= 150,000 due to cold storage touches",
        outcome_2hop.gas_used
    );
    assert!(
        outcome_2hop.gas_used <= 220_000,
        "2-hop gas used ({}) should be realistically bounded around 180,000",
        outcome_2hop.gas_used
    );

    // --- 3-Hop Route ---
    // Hop 1: WETH -> USDC
    // Hop 2: USDC -> WBTC
    // Hop 3: WBTC -> WETH
    let route_3hop = RouteCandidate {
        borrow_token: WETH_ADDR,
        hops: vec![
            DirectedEdge {
                pool_id: pool1,
                venue: Venue::UniswapV2,
                token_in: WETH_ADDR,
                token_out: token_b,
                reserve_in: 100_000_000_000_000_000_000,
                reserve_out: 400_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
            DirectedEdge {
                pool_id: pool2,
                venue: Venue::UniswapV2,
                token_in: token_b,
                token_out: token_c,
                reserve_in: 400_000_000_000,
                reserve_out: 10_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
            DirectedEdge {
                pool_id: pool3,
                venue: Venue::UniswapV2,
                token_in: token_c,
                token_out: WETH_ADDR,
                reserve_in: 10_000_000_000,
                reserve_out: 100_000_000_000_000_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
        ],
        sizing: TradeSizeOutput {
            optimal_input: trade_size,
            expected_intermediate: U256::from(50_000_000u64), // Hop 2 output (WBTC)
            expected_output: U256::from(1_060_000_000_000_000_000u128), // 1.06 ETH
            gross_profit: U256::from(60_000_000_000_000_000u128),
            flash_fee: U256::ZERO,
            net_profit: U256::from(60_000_000_000_000_000u128),
        },
        net_profit: U256::from(60_000_000_000_000_000u128),
    };

    let outcome_3hop = engine
        .simulate_arbitrage_route(&route_3hop, trade_size)
        .expect("3-hop simulation must succeed");

    println!("EMPIRICAL GAS: 2-hop = {}, 3-hop = {}", outcome_2hop.gas_used, outcome_3hop.gas_used);
    assert!(
        outcome_3hop.gas_used >= 220_000,
        "3-hop gas used ({}) must be >= 220,000 due to 3 cold pool touches & transfers",
        outcome_3hop.gas_used
    );
    assert!(
        outcome_3hop.gas_used > outcome_2hop.gas_used,
        "3-hop gas used ({}) must exceed 2-hop gas used ({})",
        outcome_3hop.gas_used,
        outcome_2hop.gas_used
    );
    let gas_delta = outcome_3hop.gas_used - outcome_2hop.gas_used;
    assert!(
        gas_delta >= 50_000,
        "Additional 3rd hop must add >= 50,000 gas (cold SLOAD/SSTORE + transfer), got delta {}",
        gas_delta
    );
}

// ==============================================================================
// 4. Decimal Normalization on USDC (6), WBTC (8), and WETH (18)
// ==============================================================================

#[test]
fn test_adversarial_decimal_normalization_matrix_and_bounds() {
    let engine = create_test_engine(Chain::ArbitrumOne, "decimal_norm");

    // Matrix of gas costs: microscopic to extreme
    let test_gas_values = vec![
        U256::from(1u64),                                    // 1 wei
        U256::from(1_000_000_000u64),                       // 1 Gwei
        U256::from(500_000_000_000_000u64),                 // 0.0005 ETH
        U256::from(1_000_000_000_000_000_000u128),          // 1.0 ETH
        U256::from(100_000_000_000_000_000_000u128),        // 100.0 ETH
        U256::from(10_000_000_000_000_000_000_000u128),     // 10,000 ETH
    ];

    for gas_wei in test_gas_values {
        // WETH (18 decimals): strictly 1:1 identity
        let weth_norm = engine.convert_gas_cost_to_token(gas_wei, WETH_ADDR);
        assert_eq!(weth_norm, gas_wei, "WETH conversion must be identity");

        // USDC (6 decimals): 1 ETH = $2,500 USDC
        // formula: gas_wei * 2500 / 10^12
        let usdc_norm = engine.convert_gas_cost_to_token(gas_wei, USDC_ADDR);
        let expected_usdc = gas_wei
            .saturating_mul(U256::from(2500))
            .checked_div(U256::from(1_000_000_000_000u64))
            .unwrap_or(U256::ZERO);
        assert_eq!(usdc_norm, expected_usdc);

        // WBTC (8 decimals): 1 BTC = 25 ETH -> 0.04 BTC/ETH = 4,000,000 satoshis/ETH
        // formula: gas_wei * 4_000_000 / 10^18
        let wbtc_norm = engine.convert_gas_cost_to_token(gas_wei, WBTC_ADDR);
        let expected_wbtc = gas_wei
            .saturating_mul(U256::from(4_000_000))
            .checked_div(U256::from(1_000_000_000_000_000_000u128))
            .unwrap_or(U256::ZERO);
        assert_eq!(wbtc_norm, expected_wbtc);
    }

    // Test with MarketGraph live reserve ratios
    let graph = Arc::new(MarketGraph::new());
    let pool_weth_usdc = address!("9000000000000000000000000000000000000001");

    // Add pool: 1,000 WETH : 3,000,000 USDC ($3,000 / ETH)
    // Note: WETH < USDC, so token0 = WETH, token1 = USDC
    let discovered_pool = flash_scanner::discovery::DiscoveredPool::new(
        pool_weth_usdc,
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        WETH_ADDR,
        USDC_ADDR,
        flash_core::types::FeePips(3000),
        flash_core::types::PoolState::ConstantProduct {
            reserve0: 1_000_000_000_000_000_000_000, // 1,000 WETH
            reserve1: 3_000_000_000_000,             // 3,000,000 USDC
        },
    );
    graph.add_pool(&discovered_pool);

    let engine_with_graph = engine.with_graph(graph);

    // 0.001 ETH gas cost = 1_000_000_000_000_000 wei
    // At $3,000 / ETH, 0.001 ETH = $3.00 USDC = 3_000_000 USDC units
    let gas_cost = U256::from(1_000_000_000_000_000u128);
    let dynamic_usdc = engine_with_graph.convert_gas_cost_to_token(gas_cost, USDC_ADDR);
    assert_eq!(
        dynamic_usdc,
        U256::from(3_000_000),
        "Dynamic graph-based conversion must yield $3.00 USDC"
    );

    // Zero / microscopic gas cost should never panic
    assert_eq!(engine_with_graph.convert_gas_cost_to_token(U256::ZERO, USDC_ADDR), U256::ZERO);
    assert_eq!(engine_with_graph.convert_gas_cost_to_token(U256::ZERO, WBTC_ADDR), U256::ZERO);
    assert_eq!(engine_with_graph.convert_gas_cost_to_token(U256::ZERO, WETH_ADDR), U256::ZERO);

    // Base chain WETH check
    let base_engine = create_test_engine(Chain::Base, "base_decimals");
    let base_weth = base::WETH;
    let base_norm = base_engine.convert_gas_cost_to_token(gas_cost, base_weth);
    assert_eq!(base_norm, gas_cost, "Base WETH must be treated as 18-decimal identity");
}

// ==============================================================================
// 5. GasPrices::total_gas_cost_wei Arithmetic Safety & Overflow Prevention
// ==============================================================================

#[test]
fn test_adversarial_gas_prices_arithmetic_overflow_safety() {
    // 1. Zero values
    let zero_prices = GasPrices {
        l2_base_fee_wei: 0,
        l2_priority_fee_wei: 0,
        l1_data_fee_wei: 0,
    };
    assert_eq!(zero_prices.total_gas_cost_wei(0), U256::ZERO);
    assert_eq!(zero_prices.total_gas_cost_wei(200_000), U256::ZERO);

    // 2. Realistic Arbitrum One pricing
    let arb = GasPrices::default_for_chain(Chain::ArbitrumOne);
    // 200,000 gas * (100M + 10M) + 400T = 22,000,000,000,000 + 400,000,000,000,000 = 422,000,000,000,000 wei (~0.000422 ETH)
    let arb_cost = arb.total_gas_cost_wei(200_000);
    assert_eq!(arb_cost, U256::from(422_000_000_000_000u128));

    // 3. Realistic Base pricing
    let base = GasPrices::default_for_chain(Chain::Base);
    // 200,000 gas * (50M + 50M) + 300T = 20,000,000,000,000 + 300,000,000,000,000 = 320,000,000,000,000 wei (~0.000320 ETH)
    let base_cost = base.total_gas_cost_wei(200_000);
    assert_eq!(base_cost, U256::from(320_000_000_000_000u128));

    // 4. Extreme High Values (Stress test arithmetic saturation)
    let extreme_prices = GasPrices {
        l2_base_fee_wei: u128::MAX / 2,
        l2_priority_fee_wei: u128::MAX / 2,
        l1_data_fee_wei: u128::MAX,
    };
    // Should never panic with overflow
    let extreme_cost = extreme_prices.total_gas_cost_wei(u64::MAX);
    assert!(extreme_cost > U256::ZERO);

    // 5. High gas spike (10,000 Gwei base fee, 1 ETH L1 data fee)
    let spike_prices = GasPrices {
        l2_base_fee_wei: 10_000_000_000_000, // 10,000 Gwei
        l2_priority_fee_wei: 1_000_000_000_000, // 1,000 Gwei
        l1_data_fee_wei: 1_000_000_000_000_000_000, // 1.0 ETH
    };
    // 1,000,000 gas * 11,000 Gwei + 1 ETH = 11 ETH + 1 ETH = 12 ETH
    let spike_cost = spike_prices.total_gas_cost_wei(1_000_000);
    assert_eq!(spike_cost, U256::from(12_000_000_000_000_000_000u128));
}
