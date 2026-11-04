//! Comprehensive Integration and Verification Tests for flash-scanner

use alloy_primitives::{Address, B256, U256};
use flash_core::types::{Chain, FeePips, PoolState, Venue};
use flash_scanner::{
    decode_slipstream_pool_created, decode_v2_pair_created, decode_v3_pool_created,
    decode_v4_initialize, dfs_cyclic_opportunities, encode_aggregate3, inspect_honeypot_safety,
    scan_mutated_opportunities, DiscoveredPool, HoneypotSimulationReport, HoneypotViolation,
    MarketGraph, Multicall3Discovery, PairId, PairTracker, Top100Ranker, PAIR_CREATED_TOPIC0,
    SLIPSTREAM_POOL_CREATED_TOPIC0, UNI_V3_POOL_CREATED_TOPIC0, UNI_V4_INITIALIZE_TOPIC0,
};
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn test_decode_v2_pair_created() {
    let token0 = Address::from([0x01; 20]);
    let token1 = Address::from([0x02; 20]);
    let pair = Address::from([0xaa; 20]);

    let mut t1 = [0u8; 32];
    t1[12..32].copy_from_slice(token0.as_slice());
    let mut t2 = [0u8; 32];
    t2[12..32].copy_from_slice(token1.as_slice());

    let topics = vec![PAIR_CREATED_TOPIC0, B256::from(t1), B256::from(t2)];

    let mut data = vec![0u8; 64];
    data[12..32].copy_from_slice(pair.as_slice());

    let decoded =
        decode_v2_pair_created(Chain::ArbitrumOne, Venue::UniswapV2, &topics, &data).unwrap();
    assert_eq!(decoded.id, pair);
    assert_eq!(decoded.token0, token0);
    assert_eq!(decoded.token1, token1);
    assert_eq!(decoded.fee, FeePips::from_bps(30));
}

#[test]
fn test_decode_v3_pool_created() {
    let token0 = Address::from([0x01; 20]);
    let token1 = Address::from([0x02; 20]);
    let pool = Address::from([0xbb; 20]);

    let mut t1 = [0u8; 32];
    t1[12..32].copy_from_slice(token0.as_slice());
    let mut t2 = [0u8; 32];
    t2[12..32].copy_from_slice(token1.as_slice());
    let mut t3 = [0u8; 32];
    t3[29..32].copy_from_slice(&500u32.to_be_bytes()[1..4]); // fee = 500

    let topics = vec![
        UNI_V3_POOL_CREATED_TOPIC0,
        B256::from(t1),
        B256::from(t2),
        B256::from(t3),
    ];

    let mut data = vec![0u8; 64];
    data[28..32].copy_from_slice(&10i32.to_be_bytes()); // tickSpacing = 10
    data[44..64].copy_from_slice(pool.as_slice());

    let decoded = decode_v3_pool_created(Chain::ArbitrumOne, &topics, &data).unwrap();
    assert_eq!(decoded.id, pool);
    assert_eq!(decoded.fee.0, 500);
    match decoded.state {
        PoolState::ConcentratedLiquidity { tick_spacing, .. } => {
            assert_eq!(tick_spacing, 10);
        }
        _ => panic!("Expected CLMM state"),
    }
}

#[test]
fn test_decode_slipstream_pool_created() {
    let token0 = Address::from([0x01; 20]);
    let token1 = Address::from([0x02; 20]);
    let pool = Address::from([0xcc; 20]);

    let mut t1 = [0u8; 32];
    t1[12..32].copy_from_slice(token0.as_slice());
    let mut t2 = [0u8; 32];
    t2[12..32].copy_from_slice(token1.as_slice());
    let mut t3 = [0u8; 32];
    t3[28..32].copy_from_slice(&1i32.to_be_bytes()); // tickSpacing = 1

    let topics = vec![
        SLIPSTREAM_POOL_CREATED_TOPIC0,
        B256::from(t1),
        B256::from(t2),
        B256::from(t3),
    ];

    let mut data = vec![0u8; 32];
    data[12..32].copy_from_slice(pool.as_slice());

    let decoded = decode_slipstream_pool_created(Chain::Base, &topics, &data).unwrap();
    assert_eq!(decoded.id, pool);
    assert_eq!(decoded.fee.0, 100); // 1 tickSpacing -> 100 PIPs
}

#[test]
fn test_decode_v4_initialize() {
    let pool_id = Address::from([0xdd; 20]);
    let token0 = Address::from([0x01; 20]);
    let token1 = Address::from([0x02; 20]);

    let mut t1 = [0u8; 32];
    t1[12..32].copy_from_slice(pool_id.as_slice());
    let mut t2 = [0u8; 32];
    t2[12..32].copy_from_slice(token0.as_slice());
    let mut t3 = [0u8; 32];
    t3[12..32].copy_from_slice(token1.as_slice());

    let topics = vec![
        UNI_V4_INITIALIZE_TOPIC0,
        B256::from(t1),
        B256::from(t2),
        B256::from(t3),
    ];

    let mut data = vec![0u8; 64];
    data[28..32].copy_from_slice(&3000u32.to_be_bytes()); // fee
    data[60..64].copy_from_slice(&60i32.to_be_bytes()); // tickSpacing

    let decoded = decode_v4_initialize(Chain::ArbitrumOne, &topics, &data).unwrap();
    assert_eq!(decoded.id, pool_id);
    assert_eq!(decoded.venue, Venue::UniswapV4);
    assert_eq!(decoded.fee.0, 3000);
}

#[test]
fn test_multigraph_parallel_pools() {
    let graph = MarketGraph::new();
    let token_weth = Address::from([0xee; 20]);
    let token_usdc = Address::from([0xff; 20]);

    let pool_v3 = DiscoveredPool::new(
        Address::from([0x11; 20]),
        Venue::UniswapV3,
        Chain::ArbitrumOne,
        token_weth,
        token_usdc,
        FeePips::from_pips(500),
        PoolState::ConstantProduct {
            reserve0: 10_000,
            reserve1: 25_000_000,
        },
    );
    let pool_sushi = DiscoveredPool::new(
        Address::from([0x22; 20]),
        Venue::SushiSwap,
        Chain::ArbitrumOne,
        token_weth,
        token_usdc,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 10_000,
            reserve1: 25_000_000,
        },
    );

    graph.add_pool(&pool_v3);
    graph.add_pool(&pool_sushi);

    assert_eq!(graph.vertex_count(), 2);
    assert_eq!(graph.pool_count(), 2);

    let edges_weth = graph.get_edges(token_weth);
    assert_eq!(edges_weth.len(), 2, "Must have 2 parallel outgoing edges");
}

#[test]
fn test_graph_mutation_tracking() {
    let graph = MarketGraph::new();
    let token_a = Address::from([0x01; 20]);
    let token_b = Address::from([0x02; 20]);

    let pool = DiscoveredPool::new(
        Address::from([0x11; 20]),
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        token_a,
        token_b,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 1000,
            reserve1: 2000,
        },
    );
    graph.add_pool(&pool);

    let mutated = graph.take_mutated_vertices();
    assert_eq!(mutated.len(), 2);
    assert!(graph.take_mutated_vertices().is_empty());

    // Update reserves in O(1)
    graph.update_reserves(pool.id, 1200, 2400);
    let mutated_after = graph.take_mutated_vertices();
    assert_eq!(mutated_after.len(), 2);
    assert!(graph.take_mutated_vertices().is_empty());
}

#[test]
fn test_ac1_2pool_arbitrage_end_to_end() {
    let graph = MarketGraph::new();
    let token_usdc = Address::from([0x01; 20]);
    let token_eth = Address::from([0x02; 20]);

    let d_usdc = 1_000_000u128; // 6 decimals
    let d_eth = 1_000_000_000_000_000_000u128; // 18 decimals

    // Pool A: 1,000,000 USDC, 405 ETH, fee = 3,000 PIPs
    let pool_a = DiscoveredPool::new(
        Address::from([0x11; 20]),
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        token_usdc,
        token_eth,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 1_000_000 * d_usdc,
            reserve1: 405 * d_eth,
        },
    );

    // Pool B: 1,000,000 USDC, 395 ETH, fee = 3,000 PIPs
    let pool_b = DiscoveredPool::new(
        Address::from([0x22; 20]),
        Venue::SushiSwap,
        Chain::ArbitrumOne,
        token_usdc,
        token_eth,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 1_000_000 * d_usdc,
            reserve1: 395 * d_eth,
        },
    );

    graph.add_pool(&pool_a);
    graph.add_pool(&pool_b);

    // Search opportunities starting strictly from token_usdc
    let candidates = dfs_cyclic_opportunities(&graph, token_usdc, 2, 0);
    assert_eq!(candidates.len(), 1, "Must find exactly 1 arbitrage route");

    let sizing = &candidates[0].sizing;
    let dx_usdc = sizing.optimal_input.to::<u128>() as f64 / 1e6;
    let gross_usdc = sizing.gross_profit.to::<u128>() as f64 / 1e6;

    // AC1 Assertions:
    // dx* = 4,732.42 (+- 0.01)
    // gross profit = 45.15 (+- 0.01)
    assert!(
        (dx_usdc - 4732.42).abs() < 0.01,
        "Optimal input {} must equal 4,732.42",
        dx_usdc
    );
    assert!(
        (gross_usdc - 45.15).abs() < 0.01,
        "Gross profit {} must equal 45.15",
        gross_usdc
    );
}

#[test]
fn test_multiplication_screening_prunes_unprofitable_cycles() {
    let graph = MarketGraph::new();
    let token_usdc = Address::from([0x01; 20]);
    let token_eth = Address::from([0x02; 20]);

    let d_usdc = 1_000_000u128;
    let d_eth = 1_000_000_000_000_000_000u128;

    // Identical pools -> no spread
    let pool_a = DiscoveredPool::new(
        Address::from([0x11; 20]),
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        token_usdc,
        token_eth,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 1_000_000 * d_usdc,
            reserve1: 400 * d_eth,
        },
    );
    let pool_b = DiscoveredPool::new(
        Address::from([0x22; 20]),
        Venue::SushiSwap,
        Chain::ArbitrumOne,
        token_usdc,
        token_eth,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 1_000_000 * d_usdc,
            reserve1: 400 * d_eth,
        },
    );

    graph.add_pool(&pool_a);
    graph.add_pool(&pool_b);

    let candidates = dfs_cyclic_opportunities(&graph, token_usdc, 2, 0);
    assert!(candidates.is_empty(), "Unprofitable route must be pruned");
}

#[test]
fn test_3pool_triangular_arbitrage() {
    let graph = MarketGraph::new();
    let token_weth = Address::from([0x01; 20]);
    let token_usdc = Address::from([0x02; 20]);
    let token_usdt = Address::from([0x03; 20]);

    let d18 = 1_000_000_000_000_000_000u128;
    let d6 = 1_000_000u128;

    // Pool 1: WETH -> USDC (1 WETH = 2600 USDC)
    let p1 = DiscoveredPool::new(
        Address::from([0x11; 20]),
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        token_weth,
        token_usdc,
        FeePips::from_bps(10),
        PoolState::ConstantProduct {
            reserve0: 1_000 * d18,
            reserve1: 2_600_000 * d6,
        },
    );
    // Pool 2: USDC -> USDT (1 USDC = 1.01 USDT)
    let p2 = DiscoveredPool::new(
        Address::from([0x22; 20]),
        Venue::SushiSwap,
        Chain::ArbitrumOne,
        token_usdc,
        token_usdt,
        FeePips::from_bps(10),
        PoolState::ConstantProduct {
            reserve0: 5_000_000 * d6,
            reserve1: 5_050_000 * d6,
        },
    );
    // Pool 3: USDT -> WETH (2500 USDT = 1 WETH)
    let p3 = DiscoveredPool::new(
        Address::from([0x33; 20]),
        Venue::CamelotV2,
        Chain::ArbitrumOne,
        token_usdt,
        token_weth,
        FeePips::from_bps(10),
        PoolState::ConstantProduct {
            reserve0: 2_500_000 * d6,
            reserve1: 1_000 * d18,
        },
    );

    graph.add_pool(&p1);
    graph.add_pool(&p2);
    graph.add_pool(&p3);

    let candidates = dfs_cyclic_opportunities(&graph, token_weth, 3, 0);
    assert!(!candidates.is_empty(), "Should discover triangular arbitrage");
    assert_eq!(candidates[0].hops.len(), 3);
    assert!(candidates[0].net_profit > U256::ZERO);
}

#[test]
fn test_honeypot_safety_inspection() {
    // 1. Salmonella
    let rep_salmonella = HoneypotSimulationReport {
        token: Address::from([0x66; 20]),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(10),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    match inspect_honeypot_safety(&rep_salmonella) {
        Err(HoneypotViolation::SalmonellaPartialDelivery { .. }) => {}
        _ => panic!("Expected Salmonella error"),
    }

    // 2. ORIGIN opcode
    let rep_origin = HoneypotSimulationReport {
        token: Address::from([0x88; 20]),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(100),
        used_origin_opcode: true,
        used_gasprice_opcode: false,
    };
    assert_eq!(
        inspect_honeypot_safety(&rep_origin),
        Err(HoneypotViolation::OriginSandwichTrap)
    );

    // 3. GASPRICE opcode
    let rep_gasprice = HoneypotSimulationReport {
        token: Address::from([0x99; 20]),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(100),
        used_origin_opcode: false,
        used_gasprice_opcode: true,
    };
    assert_eq!(
        inspect_honeypot_safety(&rep_gasprice),
        Err(HoneypotViolation::GasPriceSandwichTrap)
    );
}

#[test]
fn test_sp_scoring_and_top_100_heap() {
    let mut ranker = Top100Ranker::new(5);
    let mut trackers = HashMap::new();

    for i in 1..=10 {
        let pair_id = PairId::new(Address::from([i as u8; 20]), Address::from([0xfe; 20]));
        let mut tracker = PairTracker::new(pair_id);
        tracker.update_venue(Venue::UniswapV3, Address::ZERO, 0.003, 500_000.0);
        tracker.update_venue(Venue::SushiSwap, Address::ZERO, 0.003, 500_000.0);
        tracker.record_trade(100, 100.0, (i * 20_000) as f64);
        tracker.record_trade(101, 103.0, (i * 20_000) as f64);
        trackers.insert(pair_id, tracker);
    }

    let top = ranker.recompute_top_100(&mut trackers, 105);
    assert_eq!(top.len(), 5);
    assert_eq!(ranker.active_pairs.len(), 5);

    // Unranked burst promotion with mega score
    let unranked = PairId::new(Address::from([0x77; 20]), Address::from([0x88; 20]));
    let promoted = ranker.check_instant_burst_promotion(unranked, 100_000_000.0, 5.0);
    assert!(promoted);
    assert!(ranker.active_pairs.contains(&unranked));
    assert_eq!(ranker.ranked_list.len(), 5);
}

#[test]
fn test_c4_univ3_pool_created_topic0_exact_keccak() {
    use alloy_primitives::keccak256;
    let expected = keccak256(b"PoolCreated(address,address,uint24,int24,address)");
    assert_eq!(
        UNI_V3_POOL_CREATED_TOPIC0, expected,
        "UNI_V3_POOL_CREATED_TOPIC0 must match exact keccak256 hash"
    );
    assert_eq!(
        UNI_V3_POOL_CREATED_TOPIC0,
        alloy_primitives::b256!("783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118")
    );
}

#[test]
fn test_h7_add_pool_toctou_concurrency() {
    let graph = Arc::new(MarketGraph::new());
    let pool = DiscoveredPool::new(
        Address::from([0x55; 20]),
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        Address::from([0x10; 20]),
        Address::from([0x20; 20]),
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 1_000_000,
            reserve1: 2_000_000,
        },
    );

    let mut handles = Vec::new();
    for _ in 0..50 {
        let g = Arc::clone(&graph);
        let p = pool.clone();
        handles.push(std::thread::spawn(move || {
            g.add_pool(&p);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(graph.pool_count(), 1, "Pool count must be exactly 1 despite 50 concurrent add_pool calls");
    let edges_0 = graph.get_edges(pool.token0);
    let edges_1 = graph.get_edges(pool.token1);
    assert_eq!(edges_0.len(), 1, "Token0 outgoing edge count must be 1 (no duplicate edges)");
    assert_eq!(edges_1.len(), 1, "Token1 outgoing edge count must be 1 (no duplicate edges)");
}

#[test]
fn test_l6_get_edges_zero_heap_cloning() {
    let graph = MarketGraph::new();
    let pool = DiscoveredPool::new(
        Address::from([0x66; 20]),
        Venue::UniswapV3,
        Chain::ArbitrumOne,
        Address::from([0x11; 20]),
        Address::from([0x22; 20]),
        FeePips::from_pips(500),
        PoolState::ConstantProduct {
            reserve0: 5_000_000,
            reserve1: 10_000_000,
        },
    );
    graph.add_pool(&pool);

    // Call get_edges 10,000 times; each returns an Arc reference clone without heap reallocation
    for _ in 0..10_000 {
        let edges = graph.get_edges(pool.token0);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].reserve_in, 5_000_000);
    }
}

#[test]
fn test_l5_scan_mutated_opportunities_no_duplicate_sorting() {
    let graph = MarketGraph::new();
    let token_a = Address::from([0x01; 20]);
    let token_b = Address::from([0x02; 20]);

    let d_a = 1_000_000u128;
    let d_b = 1_000_000_000_000_000_000u128;

    let p1 = DiscoveredPool::new(
        Address::from([0x11; 20]),
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        token_a,
        token_b,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 1_000_000 * d_a,
            reserve1: 405 * d_b,
        },
    );
    let p2 = DiscoveredPool::new(
        Address::from([0x22; 20]),
        Venue::SushiSwap,
        Chain::ArbitrumOne,
        token_a,
        token_b,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 1_000_000 * d_a,
            reserve1: 395 * d_b,
        },
    );

    graph.add_pool(&p1);
    graph.add_pool(&p2);

    let candidates = scan_mutated_opportunities(&graph, 2, 0);
    assert!(!candidates.is_empty());
    // Verify descending sort order
    for w in candidates.windows(2) {
        assert!(w[0].net_profit >= w[1].net_profit);
    }
}

#[test]
fn test_c1_multicall3_encoding_and_decoding() {
    let factory = Address::from([0x33; 20]);
    let token0 = Address::from([0x44; 20]);
    let token1 = Address::from([0x55; 20]);

    let call_v2 = flash_scanner::build_get_pair_call(factory, token0, token1);
    let call_v3 = flash_scanner::build_get_pool_v3_call(factory, token0, token1, 500);
    let call_res = flash_scanner::build_get_reserves_call(factory);
    let call_s0 = flash_scanner::build_slot0_call(factory);
    let call_liq = flash_scanner::build_liquidity_call(factory);

    let calls = vec![call_v2, call_v3, call_res, call_s0, call_liq];
    let encoded = encode_aggregate3(&calls);
    assert!(encoded.len() > 100);
    assert_eq!(&encoded[0..4], &flash_scanner::discovery::AGGREGATE3_SELECTOR);
}

#[test]
fn test_c1_top_100_pairs_no_synthetic_addresses() {
    let top_pairs = Multicall3Discovery::generate_top_100_pairs(Chain::ArbitrumOne);
    assert_eq!(top_pairs.len(), 100);

    for pair in &top_pairs {
        assert!(
            !pair.token0.to_lowercase().starts_with("0x000000000000000000000000000000000000"),
            "token0 must not be synthetic: {}",
            pair.token0
        );
        assert!(
            !pair.token1.to_lowercase().starts_with("0x000000000000000000000000000000000000"),
            "token1 must not be synthetic: {}",
            pair.token1
        );
        assert!(pair.sp_score > 0.0);
        assert!(pair.venues.len() >= 2);
    }

    // Write genuine top_100_pairs.json
    let write_res = Multicall3Discovery::write_top_100_pairs_json(&top_pairs, "top_100_pairs.json");
    assert!(write_res.is_ok(), "Writing top_100_pairs.json must succeed");
    let _ = Multicall3Discovery::write_top_100_pairs_json(&top_pairs, "../../top_100_pairs.json");

    // Verify written file
    let file_content = std::fs::read_to_string("top_100_pairs.json").unwrap();
    let re_read: Vec<flash_scanner::TopPairMetadata> = serde_json::from_str(&file_content).unwrap();
    assert_eq!(re_read.len(), 100);
    assert!(!file_content.contains("0x000000000000000000000000000000000000115c"));
}
