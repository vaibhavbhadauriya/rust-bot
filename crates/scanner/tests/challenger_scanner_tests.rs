//! Adversarial Stress and Verification Tests for flash-scanner
//!
//! Authoritative validation for:
//! 1. In-memory multigraph G(V, E, P) concurrent edge reserve updates under contention.
//! 2. Parallel pools between identical token pairs across multiple DEX protocols/fees.
//! 3. Targeted DFS cycle search distinct-pool and distinct-vertex invariants (P1 != P2, P1 != P2 != P3 != P1).
//! 4. Multiplication-only spread screen pruning 100% of unprofitable cycles.
//! 5. Concurrency safety under simultaneous mutation, query, and pool discovery.

use alloy_primitives::{Address, U256};
use flash_core::types::{Chain, FeePips, PoolState, Venue};
use flash_math::{quick_spread_screen, solve_2pool_optimal};
use flash_scanner::{
    dfs_cyclic_opportunities, quick_3pool_spread_screen, scan_mutated_opportunities,
    DiscoveredPool, MarketGraph,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

/// Helper to create a CPMM DiscoveredPool
fn create_test_pool(
    id: Address,
    venue: Venue,
    token0: Address,
    token1: Address,
    fee_pips: u32,
    reserve0: u128,
    reserve1: u128,
) -> DiscoveredPool {
    DiscoveredPool::new(
        id,
        venue,
        Chain::ArbitrumOne,
        token0,
        token1,
        FeePips::from_pips(fee_pips),
        PoolState::ConstantProduct { reserve0, reserve1 },
    )
}

// =========================================================================
// TEST 1: Multigraph Parallel Pools Between Identical Token Pairs
// =========================================================================
#[test]
fn test_adversarial_parallel_pools_isolation_and_selection() {
    let graph = MarketGraph::new();
    let token_weth = Address::from([0x01; 20]);
    let token_usdc = Address::from([0x02; 20]);

    // 5 parallel pools for the identical token pair (WETH/USDC)
    let p_uni_v3_500 = create_test_pool(
        Address::from([0x10; 20]),
        Venue::UniswapV3,
        token_weth,
        token_usdc,
        500,
        1_000_000_000_000_000_000,     // 1 WETH
        2_500_000_000,                 // 2500 USDC
    );
    let p_uni_v3_3000 = create_test_pool(
        Address::from([0x20; 20]),
        Venue::UniswapV3,
        token_weth,
        token_usdc,
        3_000,
        10_000_000_000_000_000_000,    // 10 WETH
        25_000_000_000,                // 25000 USDC
    );
    let p_sushiswap = create_test_pool(
        Address::from([0x30; 20]),
        Venue::SushiSwap,
        token_weth,
        token_usdc,
        3_000,
        5_000_000_000_000_000_000,     // 5 WETH
        12_400_000_000,                // 2480 USDC/WETH
    );
    let p_camelot = create_test_pool(
        Address::from([0x40; 20]),
        Venue::CamelotV2,
        token_weth,
        token_usdc,
        2_000,
        8_000_000_000_000_000_000,     // 8 WETH
        20_160_000_000,                // 2520 USDC/WETH
    );
    let p_slipstream = create_test_pool(
        Address::from([0x50; 20]),
        Venue::AerodromeSlipstream,
        token_weth,
        token_usdc,
        100,
        20_000_000_000_000_000_000,    // 20 WETH
        50_000_000_000,                // 2500 USDC/WETH
    );

    graph.add_pool(&p_uni_v3_500);
    graph.add_pool(&p_uni_v3_3000);
    graph.add_pool(&p_sushiswap);
    graph.add_pool(&p_camelot);
    graph.add_pool(&p_slipstream);

    // Multigraph invariant: Exactly 2 vertices, 5 pools
    assert_eq!(graph.vertex_count(), 2, "Must contain exactly 2 token vertices");
    assert_eq!(graph.pool_count(), 5, "Must contain exactly 5 registered pools");

    // Adjacency check: 5 outgoing edges in each direction
    let edges_weth = graph.get_edges(token_weth);
    let edges_usdc = graph.get_edges(token_usdc);
    assert_eq!(edges_weth.len(), 5, "WETH must have 5 parallel outgoing edges");
    assert_eq!(edges_usdc.len(), 5, "USDC must have 5 parallel outgoing edges");

    // Verify all 5 pool IDs are distinct in the edge list
    let pool_ids: Vec<Address> = edges_weth.iter().map(|e| e.pool_id).collect();
    assert!(pool_ids.contains(&p_uni_v3_500.id));
    assert!(pool_ids.contains(&p_uni_v3_3000.id));
    assert!(pool_ids.contains(&p_sushiswap.id));
    assert!(pool_ids.contains(&p_camelot.id));
    assert!(pool_ids.contains(&p_slipstream.id));

    // Update ONLY SushiSwap reserves
    let updated = graph.update_reserves(p_sushiswap.id, 6_000_000_000_000_000_000, 15_000_000_000);
    assert!(updated, "SushiSwap update must succeed");

    // Verify isolation: Only SushiSwap edge changed, others untouched
    let edges_after = graph.get_edges(token_weth);
    for edge in edges_after.iter() {
        if edge.pool_id == p_sushiswap.id {
            assert_eq!(edge.reserve_in, 6_000_000_000_000_000_000);
            assert_eq!(edge.reserve_out, 15_000_000_000);
        } else if edge.pool_id == p_uni_v3_500.id {
            assert_eq!(edge.reserve_in, 1_000_000_000_000_000_000);
            assert_eq!(edge.reserve_out, 2_500_000_000);
        } else if edge.pool_id == p_camelot.id {
            assert_eq!(edge.reserve_in, 8_000_000_000_000_000_000);
            assert_eq!(edge.reserve_out, 20_160_000_000);
        }
    }
}

// =========================================================================
// TEST 2: DFS Cycle Search Enforces P1 != P2 (2-Hop) and Rejects Self-Loops
// =========================================================================
#[test]
fn test_adversarial_dfs_2hop_distinct_pools_enforced() {
    let graph = MarketGraph::new();
    let token_a = Address::from([0x01; 20]);
    let token_b = Address::from([0x02; 20]);

    // Single pool between A and B
    let p1 = create_test_pool(
        Address::from([0x10; 20]),
        Venue::UniswapV2,
        token_a,
        token_b,
        3_000,
        1_000_000,
        2_000_000,
    );
    graph.add_pool(&p1);

    // With only 1 pool, 2-hop cycle A -> B -> A would have P1 == P2.
    // MUST BE COMPLETELY REJECTED.
    let routes = dfs_cyclic_opportunities(&graph, token_a, 2, 0);
    assert!(
        routes.is_empty(),
        "DFS must NOT return self-cycles using the same pool back and forth"
    );

    // Now add a second parallel pool with price discrepancy
    let p2 = create_test_pool(
        Address::from([0x20; 20]),
        Venue::SushiSwap,
        token_a,
        token_b,
        3_000,
        1_000_000,
        3_000_000, // higher exchange rate
    );
    graph.add_pool(&p2);

    let routes2 = dfs_cyclic_opportunities(&graph, token_a, 2, 0);
    // Any returned route MUST have P1 != P2
    for route in &routes2 {
        assert_eq!(route.hops.len(), 2);
        assert_ne!(
            route.hops[0].pool_id, route.hops[1].pool_id,
            "2-hop cycle must strictly use distinct pools P1 != P2"
        );
        assert_eq!(route.hops[0].token_in, token_a);
        assert_eq!(route.hops[0].token_out, token_b);
        assert_eq!(route.hops[1].token_in, token_b);
        assert_eq!(route.hops[1].token_out, token_a);
    }
}

// =========================================================================
// TEST 3: DFS Cycle Search Enforces P1 != P2 != P3 != P1 and A != B != C != A (3-Hop)
// =========================================================================
#[test]
fn test_adversarial_dfs_3hop_strict_distinctness() {
    let graph = MarketGraph::new();
    let token_a = Address::from([0x01; 20]);
    let token_b = Address::from([0x02; 20]);
    let token_c = Address::from([0x03; 20]);

    // Triangle: A -> B -> C -> A
    let p_ab = create_test_pool(
        Address::from([0x11; 20]),
        Venue::UniswapV2,
        token_a,
        token_b,
        100,
        1_000_000_000_000_000_000,
        2_500_000_000,
    );
    let p_bc = create_test_pool(
        Address::from([0x22; 20]),
        Venue::SushiSwap,
        token_b,
        token_c,
        100,
        2_500_000_000,
        2_550_000_000,
    );
    let p_ca = create_test_pool(
        Address::from([0x33; 20]),
        Venue::CamelotV2,
        token_c,
        token_a,
        100,
        2_400_000_000,
        1_000_000_000_000_000_000,
    );

    graph.add_pool(&p_ab);
    graph.add_pool(&p_bc);
    graph.add_pool(&p_ca);

    let candidates = dfs_cyclic_opportunities(&graph, token_a, 3, 0);
    assert!(!candidates.is_empty(), "Triangular route must be found");

    for candidate in &candidates {
        if candidate.hops.len() == 3 {
            let h1 = &candidate.hops[0];
            let h2 = &candidate.hops[1];
            let h3 = &candidate.hops[2];

            // Verify pool distinctness: P1 != P2 != P3 != P1
            assert_ne!(h1.pool_id, h2.pool_id, "P1 must not equal P2");
            assert_ne!(h2.pool_id, h3.pool_id, "P2 must not equal P3");
            assert_ne!(h3.pool_id, h1.pool_id, "P3 must not equal P1");

            // Verify vertex distinctness: A != B != C != A
            assert_eq!(h1.token_in, token_a);
            let b = h1.token_out;
            assert_ne!(b, token_a, "Vertex B must not equal A");

            assert_eq!(h2.token_in, b);
            let c = h2.token_out;
            assert_ne!(c, token_a, "Vertex C must not equal A");
            assert_ne!(c, b, "Vertex C must not equal B");

            assert_eq!(h3.token_in, c);
            assert_eq!(h3.token_out, token_a, "Final hop must return to token A");
        }
    }
}

// =========================================================================
// TEST 4: Multiplication-Only Spread Screen Prunes 100% of Unprofitable Cycles
// =========================================================================
#[test]
fn test_adversarial_multiplication_screen_100_percent_pruning() {
    // 1. Exact equality boundary: R_in * R_in == R_out * R_out (spread == 0)
    // quick_spread_screen(r_in2, r_in1, r_out1, r_out2) evaluates r_in2 * r_in1 < r_out1 * r_out2
    assert!(
        !quick_spread_screen(1000, 1000, 1000, 1000),
        "Equal product must be pruned"
    );
    assert!(
        !quick_spread_screen(500, 2000, 1000, 1000),
        "Equal product (500*2000 == 1000*1000) must be pruned"
    );

    // 2. Deficit boundary: in_prod = out_prod + 1
    assert!(
        !quick_spread_screen(1001, 1000, 1000, 1000),
        "Deficit by 1 must be pruned"
    );

    // 3. 3-pool exact equality and deficit
    assert!(
        !quick_3pool_spread_screen(100, 100, 200, 200, 300, 300),
        "Equal 3-pool product must be pruned"
    );
    assert!(
        !quick_3pool_spread_screen(100, 99, 200, 200, 300, 300),
        "Deficit 3-pool product must be pruned"
    );

    // 4. Exhaustive fuzz sweep over 10,000 non-profitable reserve combinations:
    // For every case where in_prod >= out_prod:
    // quick_spread_screen MUST return false 100.0% of the time.
    let mut pruned_count = 0;
    let total_tests = 10_000;

    for i in 1..=total_tests {
        let r_in1 = 1_000_000u128 + (i as u128 * 7919) % 500_000;
        let r_in2 = 2_000_000u128 + (i as u128 * 104729) % 1_000_000;
        // Make out_prod <= in_prod
        let r_out1 = r_in1 - (i as u128 % 100);
        let r_out2 = r_in2 - ((i as u128 * 31) % 100);

        // Verify mathematically that in_prod >= out_prod
        let in_prod = U256::from(r_in1) * U256::from(r_in2);
        let out_prod = U256::from(r_out1) * U256::from(r_out2);
        assert!(in_prod >= out_prod);

        // Test the screen (mapping: edge2.reserve_in, edge1.reserve_in, edge1.reserve_out, edge2.reserve_out)
        let passed = quick_spread_screen(r_in2, r_in1, r_out1, r_out2);
        if !passed {
            pruned_count += 1;
        }

        // Also verify that solve_2pool_optimal never returns profit for in_prod >= out_prod
        let p1 = flash_core::types::PoolReserves {
            reserve_in: r_in1,
            reserve_out: r_out1,
            fee_pips: 0,
        };
        let p2 = flash_core::types::PoolReserves {
            reserve_in: r_in2,
            reserve_out: r_out2,
            fee_pips: 0,
        };
        let sizing = solve_2pool_optimal(&p1, &p2, 0);
        assert!(
            sizing.is_none() || sizing.unwrap().net_profit.is_zero(),
            "Non-positive spread can never produce positive arbitrage profit"
        );
    }

    assert_eq!(
        pruned_count, total_tests,
        "Screen must prune 100% of unprofitable cycles (pruned {}/{})",
        pruned_count, total_tests
    );
}

// =========================================================================
// TEST 5: Multigraph Concurrent Edge Reserve Updates Under Contention
// =========================================================================
#[test]
fn test_adversarial_concurrent_edge_updates_under_contention() {
    let graph = Arc::new(MarketGraph::new());
    let token_base = Address::from([0x01; 20]);

    // Create a star-shaped graph with 20 tokens and 30 pools
    let mut pool_ids = Vec::new();
    for i in 2..=21 {
        let token_target = Address::from([i as u8; 20]);
        let pool = create_test_pool(
            Address::from([0xa0 + i as u8; 20]),
            Venue::UniswapV2,
            token_base,
            token_target,
            3000,
            1_000_000,
            2_000_000,
        );
        graph.add_pool(&pool);
        pool_ids.push(pool.id);

        // Add a parallel pool for even tokens
        if i % 2 == 0 {
            let pool_parallel = create_test_pool(
                Address::from([0xb0 + i as u8; 20]),
                Venue::SushiSwap,
                token_base,
                token_target,
                500,
                5_000_000,
                10_000_000,
            );
            graph.add_pool(&pool_parallel);
            pool_ids.push(pool_parallel.id);
        }
    }

    let pool_ids = Arc::new(pool_ids);
    let num_threads = 8;
    let updates_per_thread = 2_500;
    let total_updates = Arc::new(AtomicUsize::new(0));
    let stop_flag = Arc::new(AtomicBool::new(false));

    let start_time = Instant::now();
    let mut handles = Vec::new();

    // 8 writer threads bombarding update_reserves
    for thread_idx in 0..num_threads {
        let g = Arc::clone(&graph);
        let pools = Arc::clone(&pool_ids);
        let counter = Arc::clone(&total_updates);

        handles.push(thread::spawn(move || {
            for iter in 0..updates_per_thread {
                let pool_id = pools[(thread_idx * 17 + iter) % pools.len()];
                let new_r0 = 1_000_000 + (iter as u128) * 10;
                let new_r1 = 2_000_000 + (iter as u128) * 20;

                let ok = g.update_reserves(pool_id, new_r0, new_r1);
                assert!(ok, "Concurrent reserve update must succeed");
                counter.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    // 2 reader threads concurrently executing DFS queries
    let mut reader_handles = Vec::new();
    for _ in 0..2 {
        let g = Arc::clone(&graph);
        let stop = Arc::clone(&stop_flag);

        reader_handles.push(thread::spawn(move || {
            let mut queries = 0;
            while !stop.load(Ordering::Relaxed) {
                let routes = dfs_cyclic_opportunities(&g, token_base, 2, 0);
                for route in &routes {
                    assert_eq!(route.hops.len(), 2);
                    assert_ne!(route.hops[0].pool_id, route.hops[1].pool_id);
                }
                queries += 1;
                thread::yield_now();
            }
            queries
        }));
    }

    // Wait for all writer threads
    for h in handles {
        h.join().expect("Writer thread must not panic");
    }

    stop_flag.store(true, Ordering::Relaxed);

    // Wait for all reader threads
    for rh in reader_handles {
        let q = rh.join().expect("Reader thread must not panic");
        assert!(q > 0, "Reader threads must complete concurrent queries");
    }

    let elapsed = start_time.elapsed();
    let updates = total_updates.load(Ordering::SeqCst);
    assert_eq!(updates, num_threads * updates_per_thread);

    println!(
        "Concurrent stress test completed: {} updates across {} threads in {:?} ({:.0} updates/sec)",
        updates,
        num_threads,
        elapsed,
        (updates as f64) / elapsed.as_secs_f64()
    );

    // Mutated vertices must be populated
    let mutated = graph.take_mutated_vertices();
    assert!(!mutated.is_empty(), "Mutated vertices set must track touched tokens");
    assert!(graph.take_mutated_vertices().is_empty(), "take_mutated_vertices must drain");
}

// =========================================================================
// TEST 6: High-Throughput Scan Opportunities Mutation Drain
// =========================================================================
#[test]
fn test_adversarial_scan_mutated_opportunities_lifecycle() {
    let graph = MarketGraph::new();
    let token_x = Address::from([0xaa; 20]);
    let token_y = Address::from([0xbb; 20]);

    let p1 = create_test_pool(
        Address::from([0x01; 20]),
        Venue::UniswapV2,
        token_x,
        token_y,
        3000,
        1_000_000,
        2_000_000,
    );
    let p2 = create_test_pool(
        Address::from([0x02; 20]),
        Venue::SushiSwap,
        token_x,
        token_y,
        3000,
        1_000_000,
        2_500_000,
    );
    graph.add_pool(&p1);
    graph.add_pool(&p2);

    // First scan: should find opportunity between p1 and p2
    let opps = scan_mutated_opportunities(&graph, 2, 0);
    assert!(!opps.is_empty(), "Must find opportunity on freshly added pools");

    // Second immediate scan without any mutations: must be empty (nothing mutated!)
    let opps_empty = scan_mutated_opportunities(&graph, 2, 0);
    assert!(
        opps_empty.is_empty(),
        "Second scan must return empty because mutated set was drained"
    );

    // Mutate pool 1
    graph.update_reserves(p1.id, 1_000_000, 1_800_000);

    // Third scan: mutated set now has token_x and token_y again
    let opps_after_mutation = scan_mutated_opportunities(&graph, 2, 0);
    assert!(
        !opps_after_mutation.is_empty(),
        "Must find opportunity after reserve mutation"
    );
}

// =========================================================================
// TEST 7: Concurrent Add Pool and Update Reserves (Lock Hierarchy & Deadlock Check)
// =========================================================================
#[test]
fn test_adversarial_concurrent_add_pool_and_update_reserves() {
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let graph = Arc::new(MarketGraph::new());
        let token_base = Address::from([0x01; 20]);

        // Pre-populate with 10 pools
        let mut initial_pools = Vec::new();
        for i in 10..20 {
            let pool = create_test_pool(
                Address::from([i as u8; 20]),
                Venue::UniswapV2,
                token_base,
                Address::from([0xa0 + i as u8; 20]),
                3000,
                1_000_000,
                2_000_000,
            );
            graph.add_pool(&pool);
            initial_pools.push(pool.id);
        }

        let initial_pools = Arc::new(initial_pools);
        let done = Arc::new(AtomicBool::new(false));

        // 4 Writer threads updating existing pools
        let mut update_handles = Vec::new();
        for t in 0..4 {
            let g = Arc::clone(&graph);
            let pools = Arc::clone(&initial_pools);
            let d = Arc::clone(&done);
            update_handles.push(thread::spawn(move || {
                let mut count = 0;
                while !d.load(Ordering::Relaxed) && count < 2_000 {
                    let pool_id = pools[(t + count) % pools.len()];
                    g.update_reserves(pool_id, 1_000_000 + count as u128, 2_000_000 + count as u128);
                    count += 1;
                }
                count
            }));
        }

        // 2 Ingestion threads adding brand new pools (new tokens)
        let mut add_handles = Vec::new();
        for t in 0..2 {
            let g = Arc::clone(&graph);
            let d = Arc::clone(&done);
            add_handles.push(thread::spawn(move || {
                let mut count = 0;
                while !d.load(Ordering::Relaxed) && count < 500 {
                    let new_tok = Address::from([(0xc0 + (t * 50 + count) % 60) as u8; 20]);
                    let pool_id = Address::from([(0x50 + (t * 50 + count) % 60) as u8; 20]);
                    let new_pool = create_test_pool(
                        pool_id,
                        Venue::SushiSwap,
                        token_base,
                        new_tok,
                        3000,
                        1_000_000,
                        2_000_000,
                    );
                    g.add_pool(&new_pool);
                    count += 1;
                }
                count
            }));
        }

        for h in update_handles {
            let _ = h.join();
        }
        done.store(true, Ordering::Relaxed);
        for h in add_handles {
            let _ = h.join();
        }
        let _ = tx.send(());
    });

    if rx.recv_timeout(std::time::Duration::from_secs(2)).is_err() {
        panic!("DEADLOCK DETECTED: MarketGraph lock inversion between get_or_create_vertex (idx_to_token -> adj) and update_reserves (adj -> idx_to_token)");
    }
}

// =========================================================================
// TEST 8: CLMM Virtual Reserves Update Concurrency
// =========================================================================
#[test]
fn test_adversarial_clmm_virtual_reserves_update() {
    let graph = MarketGraph::new();
    let token_weth = Address::from([0x01; 20]);
    let token_usdc = Address::from([0x02; 20]);
    let pool_id = Address::from([0x77; 20]);

    // Initial CLMM pool
    let pool = DiscoveredPool::new(
        pool_id,
        Venue::UniswapV3,
        Chain::ArbitrumOne,
        token_weth,
        token_usdc,
        FeePips::from_pips(500),
        PoolState::ConcentratedLiquidity {
            liquidity: 10_000_000,
            sqrt_price_x96: U256::from(79228162514264337593543950336u128), // 1.0 in Q64.96
            current_tick: 0,
            tick_spacing: 10,
        },
    );
    graph.add_pool(&pool);

    // Update with new liquidity and sqrt_price
    let new_liquidity = 15_000_000u128;
    let new_sqrt_price = U256::from(79228162514264337593543950336u128) * U256::from(2); // price = 4.0
    let ok = graph.update_clmm_reserves(pool_id, new_liquidity, new_sqrt_price);
    assert!(ok, "CLMM update must succeed");

    let edges = graph.get_edges(token_weth);
    assert_eq!(edges.len(), 1);
    assert!(edges[0].reserve_in > 0);
    assert!(edges[0].reserve_out > 0);
}

// =========================================================================
// TEST 9: Extreme Reserve Ratios and Overflow Resistance
// =========================================================================
#[test]
fn test_adversarial_extreme_reserve_ratios() {
    // 1. Extreme large reserves: 2^127
    let r_huge = (1u128 << 127) - 1;
    let r_small = 1_000_000u128;

    // quick_spread_screen with huge reserves must NOT overflow
    let screen_result = quick_spread_screen(r_small, r_small, r_huge, r_huge);
    assert!(screen_result, "Small in, huge out must pass screen without overflow");

    let screen_rev = quick_spread_screen(r_huge, r_huge, r_small, r_small);
    assert!(!screen_rev, "Huge in, small out must fail screen");

    // 2. 3-pool with large reserves: product must fit in U512 without panic
    let r3_huge = 1u128 << 126;
    let screen_3p = quick_3pool_spread_screen(r_small, r3_huge, r_small, r3_huge, r_small, r3_huge);
    assert!(screen_3p, "Huge 3-pool screen must pass without overflow");

    let screen_3p_rev = quick_3pool_spread_screen(r3_huge, r_small, r3_huge, r_small, r3_huge, r_small);
    assert!(!screen_3p_rev, "Deficit 3-pool screen must fail");
}

// =========================================================================
// TEST 10: Flash Loan Fee Deduction Invariants (Morpho/Balancer 0 vs Aave V3 500)
// =========================================================================
#[test]
fn test_adversarial_flash_loan_fee_deduction() {
    let graph = MarketGraph::new();
    let token_usdc = Address::from([0x01; 20]);
    let token_eth = Address::from([0x02; 20]);

    let d_usdc = 1_000_000u128;
    let d_eth = 1_000_000_000_000_000_000u128;

    let pool_a = create_test_pool(
        Address::from([0x11; 20]),
        Venue::UniswapV2,
        token_usdc,
        token_eth,
        3000,
        1_000_000 * d_usdc,
        405 * d_eth,
    );
    let pool_b = create_test_pool(
        Address::from([0x22; 20]),
        Venue::SushiSwap,
        token_usdc,
        token_eth,
        3000,
        1_000_000 * d_usdc,
        395 * d_eth,
    );
    graph.add_pool(&pool_a);
    graph.add_pool(&pool_b);

    // 1. Zero flash fee (Morpho Blue / Balancer V3)
    let routes_zero_fee = dfs_cyclic_opportunities(&graph, token_usdc, 2, 0);
    assert!(!routes_zero_fee.is_empty());
    let profit_zero = routes_zero_fee[0].net_profit;
    assert_eq!(profit_zero, routes_zero_fee[0].sizing.gross_profit);
    assert_eq!(routes_zero_fee[0].sizing.flash_fee, U256::ZERO);

    // 2. 500 PIPs flash fee (Aave V3, 0.05%)
    let routes_aave_fee = dfs_cyclic_opportunities(&graph, token_usdc, 2, 500);
    assert!(!routes_aave_fee.is_empty());
    let profit_aave = routes_aave_fee[0].net_profit;
    let flash_fee = routes_aave_fee[0].sizing.flash_fee;
    assert!(flash_fee > U256::ZERO, "Flash fee must be deducted");
    assert!(profit_aave < profit_zero, "Net profit under Aave fee must be strictly lower");
    assert_eq!(
        routes_aave_fee[0].sizing.gross_profit - flash_fee,
        profit_aave,
        "Net profit must equal gross profit minus flash fee"
    );
}


// =========================================================================
// TEST 11: Concurrent Token Registration Idempotency and Vertex Count
// =========================================================================
#[test]
fn test_adversarial_concurrent_token_registration_no_duplicate_indices() {
    let graph = Arc::new(MarketGraph::new());
    let token_shared = Address::from([0xee; 20]);
    let num_threads = 10;
    let pools_per_thread = 50;

    let mut handles = Vec::new();
    for t in 0..num_threads {
        let g = Arc::clone(&graph);
        handles.push(thread::spawn(move || {
            for i in 0..pools_per_thread {
                let other_token = Address::from([(0x10 + t as u8); 20]);
                let mut pool_bytes = [0u8; 20];
                let val = (0x8000 + t * 50 + i) as u32;
                pool_bytes[16..20].copy_from_slice(&val.to_be_bytes());
                let pool = create_test_pool(
                    Address::from(pool_bytes),
                    Venue::UniswapV2,
                    token_shared,
                    other_token,
                    3000,
                    1_000_000,
                    2_000_000,
                );
                g.add_pool(&pool);
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Expected tokens: token_shared (1) + 10 distinct other_tokens = 11 tokens total
    let actual_vertices = graph.vertex_count();
    println!("Actual vertex count: {}, Expected: 11", actual_vertices);
    assert_eq!(
        actual_vertices,
        11,
        "Concurrent addition of pools with shared token must not create duplicate vertex indices in idx_to_token"
    );

    // All pools must have an edge from token_shared
    let shared_edges = graph.get_edges(token_shared);
    assert_eq!(
        shared_edges.len(),
        num_threads * pools_per_thread,
        "token_shared must contain all outgoing edges without losing any to index overwrites"
    );
}

// =========================================================================
// TEST 12: Dense Subgraph DFS Traversal Throughput and Memory Safety
// =========================================================================
#[test]
fn test_adversarial_dense_subgraph_dfs_throughput() {
    let graph = MarketGraph::new();
    let num_tokens = 15;
    let mut tokens = Vec::new();
    for i in 1..=num_tokens {
        tokens.push(Address::from([i as u8; 20]));
    }

    // Fully connect tokens with pairs
    let mut pool_counter = 1u8;
    for i in 0..num_tokens {
        for j in (i + 1)..num_tokens {
            let pool = create_test_pool(
                Address::from([pool_counter; 20]),
                Venue::UniswapV2,
                tokens[i],
                tokens[j],
                3000,
                1_000_000,
                1_050_000,
            );
            pool_counter = pool_counter.wrapping_add(1);
            graph.add_pool(&pool);
        }
    }

    let start = Instant::now();
    let opps = dfs_cyclic_opportunities(&graph, tokens[0], 3, 0);
    let duration = start.elapsed();

    println!(
        "Dense multigraph DFS evaluated {} cyclic opportunities in {:?}",
        opps.len(),
        duration
    );
    // Traversal must complete in under 50 milliseconds even on dense graph
    assert!(
        duration.as_millis() < 50,
        "DFS must be fast, took {:?} (exceeded 50ms)",
        duration
    );
}

// =========================================================================
// TEST 13: Zero and Degenerate Reserve Handling Without Panics
// =========================================================================
#[test]
fn test_adversarial_zero_reserve_resilience() {
    let graph = MarketGraph::new();
    let token_a = Address::from([0x01; 20]);
    let token_b = Address::from([0x02; 20]);
    let token_c = Address::from([0x03; 20]);

    let p1 = create_test_pool(
        Address::from([0x10; 20]),
        Venue::UniswapV2,
        token_a,
        token_b,
        3000,
        0, // Zero reserve!
        1_000_000,
    );
    let p2 = create_test_pool(
        Address::from([0x20; 20]),
        Venue::SushiSwap,
        token_b,
        token_c,
        3000,
        1_000_000,
        0, // Zero reserve!
    );
    let p3 = create_test_pool(
        Address::from([0x30; 20]),
        Venue::CamelotV2,
        token_c,
        token_a,
        3000,
        0,
        0,
    );

    graph.add_pool(&p1);
    graph.add_pool(&p2);
    graph.add_pool(&p3);

    // DFS with zero reserves must NOT panic
    let routes = dfs_cyclic_opportunities(&graph, token_a, 3, 0);
    assert!(
        routes.is_empty(),
        "Zero reserve pools must produce zero valid opportunities"
    );
}

// =========================================================================
// TEST 14: Idempotent Add Pool Duplicate Rejection
// =========================================================================
#[test]
fn test_adversarial_duplicate_pool_registration_idempotency() {
    let graph = MarketGraph::new();
    let token_a = Address::from([0x01; 20]);
    let token_b = Address::from([0x02; 20]);
    let pool_id = Address::from([0x55; 20]);

    let pool = create_test_pool(
        pool_id,
        Venue::UniswapV2,
        token_a,
        token_b,
        3000,
        1_000_000,
        2_000_000,
    );

    graph.add_pool(&pool);
    assert_eq!(graph.pool_count(), 1);
    assert_eq!(graph.get_edges(token_a).len(), 1);

    // Add exact same pool again
    graph.add_pool(&pool);
    assert_eq!(
        graph.pool_count(),
        1,
        "Duplicate pool registration must be ignored"
    );
    assert_eq!(
        graph.get_edges(token_a).len(),
        1,
        "Duplicate pool must not duplicate edges in adjacency list"
    );
}
