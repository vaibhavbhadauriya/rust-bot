//! Empirical Adversarial Verification & Stress Test Suite for Milestone M_R4
//!
//! Evaluates:
//! - C4: Keccak-256 topic0 oracle test for `PoolCreated(address,address,uint24,int24,address)`
//! - H7: Multi-threaded concurrency race test for `MarketGraph::add_pool` (no duplicate edges, no deadlock)
//! - L6: Zero-heap-cloning & Arc pointer-identity test on `MarketGraph::get_edges`
//! - L5: Route equivalence and sort-correctness test on `scan_mutated_opportunities`
//! - C1: Multicall3 ABI encoding/decoding & top_100_pairs.json non-synthetic validation

use alloy_primitives::{b256, keccak256, Address, B256};
use flash_core::types::{Chain, FeePips, PoolState, Venue};
use flash_scanner::discovery::AGGREGATE3_SELECTOR;
use flash_scanner::{
    build_get_pair_call, build_get_pool_v3_call, build_get_reserves_call, build_liquidity_call,
    build_slot0_call, decode_aggregate3_results, decode_v3_pool_created,
    dfs_cyclic_opportunities, encode_aggregate3, filter_unit_reserves,
    scan_mutated_opportunities, Call3, DiscoveredPool, MarketGraph, Multicall3Discovery,
    TopPairMetadata, UNI_V3_POOL_CREATED_TOPIC0,
};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

// =========================================================================
// C4: Keccak-256 Topic0 Oracle Verification
// =========================================================================

#[test]
fn test_adversarial_c4_keccak_oracle_exactness() {
    let raw_signature = b"PoolCreated(address,address,uint24,int24,address)";
    let computed_hash = keccak256(raw_signature);

    let authoritative_hex =
        b256!("783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118");
    let legacy_corrupt_hex =
        b256!("783cca1c04124a23a8e62df2694056dd2e0035e227d2b40730d586ab66724ec4");

    // 1. Authoritative ground truth match
    assert_eq!(
        computed_hash, authoritative_hex,
        "Computed Keccak-256 must match authoritative hex"
    );
    assert_eq!(
        UNI_V3_POOL_CREATED_TOPIC0, authoritative_hex,
        "Constant UNI_V3_POOL_CREATED_TOPIC0 must match authoritative hex"
    );
    assert_ne!(
        UNI_V3_POOL_CREATED_TOPIC0, legacy_corrupt_hex,
        "Constant must NOT match legacy corrupt hash"
    );

    // 2. Decoder test with genuine vs corrupt topic0
    let token0 = Address::from([0x01; 20]);
    let token1 = Address::from([0x02; 20]);
    let pool_addr = Address::from([0xaa; 20]);

    let mut t1 = [0u8; 32];
    t1[12..32].copy_from_slice(token0.as_slice());
    let mut t2 = [0u8; 32];
    t2[12..32].copy_from_slice(token1.as_slice());
    let mut t3 = [0u8; 32];
    t3[29..32].copy_from_slice(&500u32.to_be_bytes()[1..4]);

    let mut data = vec![0u8; 64];
    data[28..32].copy_from_slice(&10i32.to_be_bytes()); // tickSpacing = 10
    data[44..64].copy_from_slice(pool_addr.as_slice());

    // Genuine topic0: must succeed
    let genuine_topics = vec![
        UNI_V3_POOL_CREATED_TOPIC0,
        B256::from(t1),
        B256::from(t2),
        B256::from(t3),
    ];
    let decoded_ok = decode_v3_pool_created(Chain::ArbitrumOne, &genuine_topics, &data);
    assert!(decoded_ok.is_ok(), "Genuine topic0 must decode successfully");
    let pool = decoded_ok.unwrap();
    assert_eq!(pool.id, pool_addr);
    assert_eq!(pool.fee.0, 500);

    // Corrupt legacy topic0: must fail with Mismatched UniV3 PoolCreated topic0
    let corrupt_topics = vec![
        legacy_corrupt_hex,
        B256::from(t1),
        B256::from(t2),
        B256::from(t3),
    ];
    let decoded_err = decode_v3_pool_created(Chain::ArbitrumOne, &corrupt_topics, &data);
    assert!(
        decoded_err.is_err(),
        "Corrupt topic0 must be rejected by decoder"
    );
}

// =========================================================================
// H7: Multi-threaded Concurrency Race Test for MarketGraph::add_pool
// =========================================================================

#[test]
fn test_adversarial_h7_add_pool_massive_concurrency_race() {
    let graph = Arc::new(MarketGraph::new());
    let token_a = Address::from([0x11; 20]);
    let token_b = Address::from([0x22; 20]);
    let pool_id = Address::from([0x99; 20]);

    let pool = DiscoveredPool::new(
        pool_id,
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        token_a,
        token_b,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 10_000_000,
            reserve1: 20_000_000,
        },
    );

    let num_threads = 100;
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::with_capacity(num_threads);

    for _ in 0..num_threads {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let p = pool.clone();
        handles.push(thread::spawn(move || {
            b.wait();
            g.add_pool(&p);
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Invariant: Exact idempotency under 100 concurrent writers
    assert_eq!(
        graph.pool_count(),
        1,
        "pool_count must be exactly 1 despite 100 concurrent add_pool calls"
    );

    let edges_a = graph.get_edges(token_a);
    let edges_b = graph.get_edges(token_b);

    assert_eq!(
        edges_a.len(),
        1,
        "token_a must have exactly 1 outgoing edge (no duplicate edges in adj)"
    );
    assert_eq!(
        edges_b.len(),
        1,
        "token_b must have exactly 1 outgoing edge (no duplicate edges in adj)"
    );
    assert_eq!(edges_a[0].pool_id, pool_id);
    assert_eq!(edges_b[0].pool_id, pool_id);
}

#[test]
fn test_adversarial_h7_concurrent_mixed_operations_no_deadlock() {
    let graph = Arc::new(MarketGraph::new());
    let num_threads = 50;
    let barrier = Arc::new(Barrier::new(num_threads));
    let stop_signal = Arc::new(AtomicBool::new(false));
    let ops_completed = Arc::new(AtomicUsize::new(0));

    let tokens: Vec<Address> = (0..10).map(|i| Address::from([i as u8 + 1; 20])).collect();

    // Pre-populate 5 pools
    for i in 0..5 {
        let p = DiscoveredPool::new(
            Address::from([0x50 + i as u8; 20]),
            Venue::UniswapV2,
            Chain::ArbitrumOne,
            tokens[i],
            tokens[i + 1],
            FeePips::from_bps(30),
            PoolState::ConstantProduct {
                reserve0: 1_000_000,
                reserve1: 1_000_000,
            },
        );
        graph.add_pool(&p);
    }

    let mut handles = Vec::with_capacity(num_threads);

    for t_id in 0..num_threads {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let stop = Arc::clone(&stop_signal);
        let ops = Arc::clone(&ops_completed);
        let tok = tokens.clone();

        handles.push(thread::spawn(move || {
            b.wait();
            let mut iter = 0;
            while !stop.load(Ordering::Relaxed) && iter < 500 {
                iter += 1;
                match t_id % 4 {
                    0 => {
                        // Thread group 0: add_pool with overlapping tokens
                        let pool_idx = ((t_id * 10 + iter) % 20) as u8;
                        let t0 = tok[iter % tok.len()];
                        let t1 = tok[(iter + 1) % tok.len()];
                        let p = DiscoveredPool::new(
                            Address::from([0x80 + pool_idx; 20]),
                            Venue::UniswapV3,
                            Chain::ArbitrumOne,
                            t0,
                            t1,
                            FeePips::from_pips(500),
                            PoolState::ConstantProduct {
                                reserve0: 1_000_000 + (iter as u128) * 100,
                                reserve1: 2_000_000,
                            },
                        );
                        g.add_pool(&p);
                    }
                    1 => {
                        // Thread group 1: update_reserves on pre-existing pools
                        let pool_idx = (iter % 5) as u8;
                        let pool_id = Address::from([0x50 + pool_idx; 20]);
                        g.update_reserves(
                            pool_id,
                            1_000_000 + (iter as u128),
                            1_000_000 + (iter as u128),
                        );
                    }
                    2 => {
                        // Thread group 2: get_edges read contention
                        let t = tok[iter % tok.len()];
                        let edges = g.get_edges(t);
                        let _ = edges.len();
                    }
                    3 => {
                        // Thread group 3: drain mutated vertices and search opportunities
                        let _ = scan_mutated_opportunities(&g, 2, 0);
                    }
                    _ => unreachable!(),
                }
                ops.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for h in handles {
            let _ = h.join();
        }
        let _ = done_tx.send(());
    });

    let finished_in_time = done_rx.recv_timeout(Duration::from_secs(5)).is_ok();
    assert!(
        finished_in_time,
        "CRITICAL DEADLOCK DETECTED: MarketGraph mixed concurrent operations deadlocked! ABBA lock inversion between self.adj and self.token_to_idx."
    );
}

#[test]
fn test_adversarial_h7_targeted_abba_deadlock() {
    let graph = Arc::new(MarketGraph::new());
    let target_token = Address::from([0x77; 20]);

    // Pre-register target token
    let dummy_pool = DiscoveredPool::new(
        Address::from([0x01; 20]),
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        target_token,
        Address::from([0x02; 20]),
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 1_000_000,
            reserve1: 1_000_000,
        },
    );
    graph.add_pool(&dummy_pool);

    let num_readers = 8;
    let num_writers = 8;
    let barrier = Arc::new(Barrier::new(num_readers + num_writers));
    let stop = Arc::new(AtomicBool::new(false));

    let mut handles = Vec::new();

    // Reader threads: repeatedly call get_edges(target_token)
    // In current implementation: get_edges takes token_to_idx.get() (shard read lock) -> adj.read()
    for _ in 0..num_readers {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let s = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            b.wait();
            let mut iters = 0;
            while !s.load(Ordering::Relaxed) && iters < 20_000 {
                let _ = g.get_edges(target_token);
                iters += 1;
            }
        }));
    }

    // Writer threads: repeatedly call add_pool with new tokens
    // In current implementation: add_pool -> get_or_create_vertex takes adj.write() -> token_to_idx.insert() (shard write lock)
    for w_idx in 0..num_writers {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let s = Arc::clone(&stop);
        handles.push(thread::spawn(move || {
            b.wait();
            let mut iters = 0;
            while !s.load(Ordering::Relaxed) && iters < 2_000 {
                let pool_addr = Address::from([
                    0xaa, (w_idx as u8), ((iters >> 8) as u8), (iters as u8),
                    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
                    0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
                ]);
                let other_tok = Address::from([
                    0xbb, (w_idx as u8), ((iters >> 8) as u8), (iters as u8),
                    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
                    0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
                ]);
                let p = DiscoveredPool::new(
                    pool_addr,
                    Venue::UniswapV3,
                    Chain::ArbitrumOne,
                    target_token,
                    other_tok,
                    FeePips::from_pips(500),
                    PoolState::ConstantProduct {
                        reserve0: 1_000_000,
                        reserve1: 1_000_000,
                    },
                );
                g.add_pool(&p);
                iters += 1;
            }
        }));
    }

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for h in handles {
            let _ = h.join();
        }
        let _ = done_tx.send(());
    });

    let completed = done_rx.recv_timeout(Duration::from_secs(20)).is_ok();
    stop.store(true, Ordering::Relaxed);

    assert!(
        completed,
        "CRITICAL DEADLOCK CONFIRMED: get_edges (holding token_to_idx shard read lock, waiting for adj.read) deadlocked with get_or_create_vertex (holding adj.write, waiting for token_to_idx shard write lock)!"
    );
}

// =========================================================================
// L6: Zero-Heap-Cloning & Arc Pointer Identity for MarketGraph::get_edges
// =========================================================================

#[test]
fn test_adversarial_l6_zero_heap_cloning_microbenchmark() {
    let graph = MarketGraph::new();
    let token_a = Address::from([0x01; 20]);
    let token_b = Address::from([0x02; 20]);
    let pool_id = Address::from([0xaa; 20]);

    let pool = DiscoveredPool::new(
        pool_id,
        Venue::UniswapV3,
        Chain::ArbitrumOne,
        token_a,
        token_b,
        FeePips::from_pips(500),
        PoolState::ConstantProduct {
            reserve0: 10_000_000,
            reserve1: 20_000_000,
        },
    );
    graph.add_pool(&pool);

    // 1. Empirical proof of zero heap buffer allocation: Arc pointer identity
    let edges1 = graph.get_edges(token_a);
    let edges2 = graph.get_edges(token_a);

    // Arc::ptr_eq tests whether both Arcs point to the SAME heap allocation
    assert!(
        Arc::ptr_eq(&edges1, &edges2),
        "get_edges must return an Arc clone pointing to the EXACT same memory allocation"
    );
    assert_eq!(
        edges1.as_ptr(),
        edges2.as_ptr(),
        "Underlying slice memory addresses must be identical (zero buffer clone)"
    );
    assert!(
        Arc::strong_count(&edges1) >= 3,
        "Strong reference count must reflect sharing in graph + edges1 + edges2"
    );

    // 2. Microbenchmark: 100,000 get_edges lookups must complete in sub-millisecond to low-millisecond range
    let iterations = 100_000;
    let start = Instant::now();
    let mut sum_reserves = 0u128;
    for _ in 0..iterations {
        let e = graph.get_edges(token_a);
        sum_reserves = sum_reserves.wrapping_add(e[0].reserve_in);
    }
    let elapsed = start.elapsed();
    let ns_per_op = elapsed.as_nanos() as f64 / iterations as f64;

    assert_eq!(sum_reserves, 10_000_000u128.wrapping_mul(iterations as u128));
    let threshold_ns = if cfg!(debug_assertions) { 2500.0 } else { 200.0 };
    assert!(
        ns_per_op < threshold_ns,
        "get_edges should take < {:.0} ns per lookup with zero heap allocation, took {:.2} ns",
        threshold_ns,
        ns_per_op
    );
}

// =========================================================================
// L5: Route Equivalence and Sort-Correctness for scan_mutated_opportunities
// =========================================================================

#[test]
fn test_adversarial_l5_scan_mutated_opportunities_route_equivalence() {
    let graph = MarketGraph::new();

    let token_weth = Address::from([0x01; 20]);
    let token_usdc = Address::from([0x02; 20]);
    let token_usdt = Address::from([0x03; 20]);

    let d_eth = 1_000_000_000_000_000_000u128;
    let d_usd = 1_000_000u128;

    // Create 3 interconnected pools forming both 2-hop and 3-hop opportunities
    // Pool 1 (UniV3 WETH/USDC): 1000 WETH : 2,600,000 USDC (Price: 2600)
    let p1 = DiscoveredPool::new(
        Address::from([0x11; 20]),
        Venue::UniswapV3,
        Chain::ArbitrumOne,
        token_weth,
        token_usdc,
        FeePips::from_pips(500),
        PoolState::ConstantProduct {
            reserve0: 1_000 * d_eth,
            reserve1: 2_600_000 * d_usd,
        },
    );

    // Pool 2 (Sushi WETH/USDC): 1000 WETH : 2,500,000 USDC (Price: 2500) -> 2-hop arb
    let p2 = DiscoveredPool::new(
        Address::from([0x22; 20]),
        Venue::SushiSwap,
        Chain::ArbitrumOne,
        token_weth,
        token_usdc,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 1_000 * d_eth,
            reserve1: 2_500_000 * d_usd,
        },
    );

    // Pool 3 (Camelot USDC/USDT): 2,500,000 USDC : 2,550,000 USDT
    let p3 = DiscoveredPool::new(
        Address::from([0x33; 20]),
        Venue::CamelotV2,
        Chain::ArbitrumOne,
        token_usdc,
        token_usdt,
        FeePips::from_bps(10),
        PoolState::ConstantProduct {
            reserve0: 2_500_000 * d_usd,
            reserve1: 2_550_000 * d_usd,
        },
    );

    // Pool 4 (UniV3 USDT/WETH): 2,550,000 USDT : 990 WETH -> 3-hop arb
    let p4 = DiscoveredPool::new(
        Address::from([0x44; 20]),
        Venue::UniswapV3,
        Chain::ArbitrumOne,
        token_usdt,
        token_weth,
        FeePips::from_pips(500),
        PoolState::ConstantProduct {
            reserve0: 2_550_000 * d_usd,
            reserve1: 990 * d_eth,
        },
    );

    graph.add_pool(&p1);
    graph.add_pool(&p2);
    graph.add_pool(&p3);
    graph.add_pool(&p4);

    // 1. Clear initial pool addition mutations, then manually mark specific vertices as mutated
    let _ = graph.take_mutated_vertices();
    graph.mark_mutated(token_weth);
    graph.mark_mutated(token_usdc);

    // Compare scan_mutated_opportunities against ground-truth naive multi-sort approach
    let mutated_tokens = vec![token_weth, token_usdc];

    // Reference naive collection:
    let mut naive_routes = Vec::new();
    for &t in &mutated_tokens {
        let mut routes = dfs_cyclic_opportunities(&graph, t, 3, 0);
        naive_routes.append(&mut routes);
    }
    naive_routes.sort_by(|a, b| b.net_profit.cmp(&a.net_profit));

    // Fast unified scan:
    // Re-mark since reference drain didn't happen (it used dfs_cyclic_opportunities directly)
    graph.mark_mutated(token_weth);
    graph.mark_mutated(token_usdc);
    let optimized_routes = scan_mutated_opportunities(&graph, 3, 0);

    // Route equivalence verification
    assert_eq!(
        optimized_routes.len(),
        naive_routes.len(),
        "Route count must match between naive and optimized"
    );

    for (opt, naive) in optimized_routes.iter().zip(naive_routes.iter()) {
        assert_eq!(opt.net_profit, naive.net_profit);
        assert_eq!(opt.borrow_token, naive.borrow_token);
        assert_eq!(opt.hops.len(), naive.hops.len());
    }

    // Sort order verification: strictly descending
    for window in optimized_routes.windows(2) {
        assert!(
            window[0].net_profit >= window[1].net_profit,
            "Optimized routes must be strictly sorted by net_profit descending"
        );
    }

    // Invalidation condition: empty graph or no mutated vertices returns empty vector without panic
    let empty_routes = scan_mutated_opportunities(&graph, 3, 0);
    assert!(
        empty_routes.is_empty(),
        "Subsequent call after drain must return empty vector"
    );
}

// =========================================================================
// C1: Multicall3 ABI Encoding/Decoding & top_100_pairs.json Verification
// =========================================================================

#[test]
fn test_adversarial_c1_multicall3_encoding_and_fuzz_decoding() {
    let factory = Address::from([0x22; 20]);
    let token0 = Address::from([0x33; 20]);
    let token1 = Address::from([0x44; 20]);

    // 1. Encoding various Call3 types
    let calls = vec![
        build_get_pair_call(factory, token0, token1),
        build_get_pool_v3_call(factory, token0, token1, 3000),
        build_get_reserves_call(factory),
        build_slot0_call(factory),
        build_liquidity_call(factory),
        Call3 {
            target: factory,
            allow_failure: true,
            call_data: vec![], // Empty call data
        },
        Call3 {
            target: factory,
            allow_failure: false,
            call_data: vec![0xde, 0xad, 0xbe, 0xef, 0x01], // Odd length call data (5 bytes)
        },
    ];

    let encoded = encode_aggregate3(&calls);
    assert_eq!(&encoded[0..4], &AGGREGATE3_SELECTOR);
    assert!(encoded.len() > 100);

    // 2. Synthesize valid aggregate3 return bytes and test decoding
    // Return structure:
    // (uint256 array_offset = 0x20)
    // (uint256 array_len = 3)
    // (uint256 tuple0_offset, uint256 tuple1_offset, uint256 tuple2_offset)
    // tuple 0: (bool success=true, bytes returnData = 32 bytes address)
    // tuple 1: (bool success=false, bytes returnData = empty)
    // tuple 2: (bool success=true, bytes returnData = 64 bytes reserves)
    let mut ret_bytes = Vec::new();

    // Word 0: Offset to Result[] (0x20 = 32)
    let mut w0 = [0u8; 32];
    w0[31] = 0x20;
    ret_bytes.extend_from_slice(&w0);

    // Word 1: Length of Result[] (3)
    let mut w1 = [0u8; 32];
    w1[31] = 3;
    ret_bytes.extend_from_slice(&w1);

    // Word 2, 3, 4: Offsets to tuple 0, tuple 1, tuple 2 from elements_start
    let tuple_head_size = 3 * 32;
    let offset_t0 = tuple_head_size;
    let tuple0_len = 32 + 32 + 32 + 32; // success + cd_offset + cd_len + 32 bytes data = 128
    let offset_t1 = offset_t0 + tuple0_len;
    let tuple1_len = 32 + 32 + 32; // success + cd_offset + cd_len = 96
    let offset_t2 = offset_t1 + tuple1_len;

    for &off in &[offset_t0, offset_t1, offset_t2] {
        let mut w = [0u8; 32];
        w[24..32].copy_from_slice(&(off as u64).to_be_bytes());
        ret_bytes.extend_from_slice(&w);
    }

    // Tuple 0 data: success=true, returnData = 32 bytes
    let mut t0 = [0u8; 32];
    t0[31] = 1; // success = true
    ret_bytes.extend_from_slice(&t0);
    let mut off_bytes = [0u8; 32];
    off_bytes[31] = 0x40; // returnData offset inside tuple = 64
    ret_bytes.extend_from_slice(&off_bytes);
    let mut len_bytes = [0u8; 32];
    len_bytes[31] = 32; // length = 32
    ret_bytes.extend_from_slice(&len_bytes);
    let dummy_addr = Address::from([0x77; 20]);
    let mut addr_word = [0u8; 32];
    addr_word[12..32].copy_from_slice(dummy_addr.as_slice());
    ret_bytes.extend_from_slice(&addr_word);

    // Tuple 1 data: success=false, returnData = 0 bytes
    let mut t1 = [0u8; 32];
    t1[31] = 0; // success = false
    ret_bytes.extend_from_slice(&t1);
    ret_bytes.extend_from_slice(&off_bytes);
    let len_zero = [0u8; 32];
    ret_bytes.extend_from_slice(&len_zero);

    // Tuple 2 data: success=true, returnData = 64 bytes
    ret_bytes.extend_from_slice(&t0);
    ret_bytes.extend_from_slice(&off_bytes);
    let mut len_64 = [0u8; 32];
    len_64[31] = 64;
    ret_bytes.extend_from_slice(&len_64);
    let mut reserves_data = [0u8; 64];
    reserves_data[15] = 10;
    reserves_data[47] = 20;
    ret_bytes.extend_from_slice(&reserves_data);

    let decoded_res = decode_aggregate3_results(&ret_bytes).expect("Decoding must succeed");
    assert_eq!(decoded_res.len(), 3);
    assert!(decoded_res[0].success);
    assert_eq!(decoded_res[0].return_data.len(), 32);
    assert_eq!(&decoded_res[0].return_data[12..32], dummy_addr.as_slice());

    assert!(!decoded_res[1].success);
    assert!(decoded_res[1].return_data.is_empty());

    assert!(decoded_res[2].success);
    assert_eq!(decoded_res[2].return_data.len(), 64);
    assert_eq!(&decoded_res[2].return_data[..], &reserves_data[..]);

    // 3. Error handling on truncated data
    assert!(decode_aggregate3_results(&ret_bytes[..40]).is_err());
    assert!(decode_aggregate3_results(&[]).is_err());
}

#[test]
fn test_adversarial_c1_top_100_pairs_json_integrity() {
    let paths = ["top_100_pairs.json", "../../top_100_pairs.json"];

    for path in &paths {
        if let Ok(content) = std::fs::read_to_string(path) {
            let pairs: Vec<TopPairMetadata> =
                serde_json::from_str(&content).expect("Must parse as valid JSON array");

            assert_eq!(pairs.len(), 100, "{}: Must contain exactly 100 pairs", path);

            let mut prev_score = f64::MAX;
            for (idx, pair) in pairs.iter().enumerate() {
                // Rank continuity
                assert_eq!(pair.rank, idx + 1, "Rank must be contiguous 1..100");

                // Valid address syntax
                let addr0 = Address::from_str(&pair.token0).expect("token0 must be valid address");
                let addr1 = Address::from_str(&pair.token1).expect("token1 must be valid address");
                assert_ne!(addr0, Address::ZERO, "token0 cannot be Address::ZERO");
                assert_ne!(addr1, Address::ZERO, "token1 cannot be Address::ZERO");
                assert_ne!(addr0, addr1, "token0 and token1 must be distinct");

                // Check for fake addresses / placeholders
                let t0_lower = pair.token0.to_lowercase();
                let t1_lower = pair.token1.to_lowercase();
                assert!(
                    !t0_lower.starts_with("0x000000000000000000000000000000000000"),
                    "token0 must not be synthetic placeholder: {}",
                    t0_lower
                );
                assert!(
                    !t1_lower.starts_with("0x000000000000000000000000000000000000"),
                    "token1 must not be synthetic placeholder: {}",
                    t1_lower
                );
                assert!(
                    !t0_lower.contains("115c") && !t0_lower.contains("22b8"),
                    "token0 contains forbidden mock signature: {}",
                    t0_lower
                );
                assert!(
                    !t1_lower.contains("115c") && !t1_lower.contains("22b8"),
                    "token1 contains forbidden mock signature: {}",
                    t1_lower
                );

                // Venues constraint: minimum 2 venues
                assert!(
                    pair.venues.len() >= 2,
                    "Pair must have >= 2 venues for arbitrage"
                );

                // Financial metrics: positive
                assert!(pair.sp_score > 0.0);
                assert!(pair.active_liquidity_usd > 0.0);
                assert!(pair.trailing_24h_volume_usd > 0.0);
                assert!(pair.volatility_sigma > 0.0);

                // Monotonic ranking by S_p composite score
                assert!(
                    pair.sp_score <= prev_score + 1e-6,
                    "Pairs must be sorted descending by sp_score (index {}, {} > {})",
                    idx,
                    pair.sp_score,
                    prev_score
                );
                prev_score = pair.sp_score;
            }
        }
    }

    // 4. Canonical pools decimal-adjusted unit reserves (R / 10^d >= 1)
    for chain in &[Chain::ArbitrumOne, Chain::Base] {
        let canonical_pools = Multicall3Discovery::canonical_mainnet_pools(*chain);
        assert!(
            !canonical_pools.is_empty(),
            "Canonical pool list for {:?} must not be empty",
            chain
        );

        for pool in canonical_pools {
            match pool.state {
                PoolState::ConstantProduct { reserve0, reserve1 } => {
                    // Check decimal filter
                    // Even assuming 18 decimals, reserve must be >= 1e18
                    assert!(
                        reserve0 >= 1_000_000,
                        "reserve0 must exceed unit threshold: {}",
                        reserve0
                    );
                    assert!(
                        reserve1 >= 1_000_000,
                        "reserve1 must exceed unit threshold: {}",
                        reserve1
                    );

                    // Test with filter_unit_reserves directly
                    let ok = filter_unit_reserves(reserve0, 6, reserve1, 18)
                        || filter_unit_reserves(reserve0, 18, reserve1, 6)
                        || filter_unit_reserves(reserve0, 8, reserve1, 6)
                        || filter_unit_reserves(reserve0, 18, reserve1, 18);
                    assert!(
                        ok,
                        "Pool {:?} must pass unit reserve filter: ({}, {})",
                        pool.id, reserve0, reserve1
                    );
                }
                _ => panic!("Expected constant product representation for canonical pools"),
            }
        }
    }
}
