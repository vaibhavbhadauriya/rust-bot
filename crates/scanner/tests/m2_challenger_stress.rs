//! Empirical Challenger Stress & Concurrency Test for Milestone 2 (Discovery & Chunking)
//!
//! Authoritative source: Task prompt & PROJECT.md
//! 1. Stress tests `MarketGraph::get_pool_tokens` and dynamic pool insertion across concurrent threads
//!    to guarantee zero lock contention, zero deadlocks, and strict token-pair retrieval correctness.
//! 2. Verifies that `canonical_mainnet_pools` provides >= 50 valid pools per chain (Arbitrum One and Base)
//!    that load cleanly into `MarketGraph`.
//! 3. Verifies `MULTICALL_CHUNK_SIZE` chunking semantics and boundary preservation.

use alloy_primitives::{Address, U256};
use flash_core::types::{Chain, FeePips, PoolState, Venue};
use flash_scanner::discovery::{DiscoveredPool, Multicall3Discovery, MULTICALL_CHUNK_SIZE};
use flash_scanner::graph::MarketGraph;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

// =========================================================================
// TEST 1: Canonical Mainnet Pools Catalog Verification (>= 50 Pools Per Chain)
// =========================================================================
#[test]
fn test_canonical_mainnet_pools_counts_and_clean_loading() {
    let chains = [Chain::ArbitrumOne, Chain::Base];

    for chain in chains {
        let pools = Multicall3Discovery::canonical_mainnet_pools(chain);
        println!(
            "Verifying canonical pools for {}: count = {}",
            chain.name(),
            pools.len()
        );

        // Verification requirement: >= 50 valid pools per chain
        assert!(
            pools.len() >= 50,
            "{} must provide at least 50 canonical pools, but got {}",
            chain.name(),
            pools.len()
        );

        let graph = MarketGraph::new();
        let mut seen_ids = HashSet::new();

        for (idx, pool) in pools.iter().enumerate() {
            // Pool identity integrity
            assert_ne!(
                pool.id,
                Address::ZERO,
                "Chain {} pool #{} has null pool ID",
                chain.name(),
                idx
            );
            assert_ne!(
                pool.token0,
                Address::ZERO,
                "Chain {} pool #{} has null token0",
                chain.name(),
                idx
            );
            assert_ne!(
                pool.token1,
                Address::ZERO,
                "Chain {} pool #{} has null token1",
                chain.name(),
                idx
            );
            assert_ne!(
                pool.token0,
                pool.token1,
                "Chain {} pool #{} has identical token0 and token1",
                chain.name(),
                idx
            );

            // Canonical token ordering
            assert!(
                pool.token0 < pool.token1,
                "Chain {} pool #{} violates canonical token0 < token1 ordering",
                chain.name(),
                idx
            );

            // Valid non-zero fee
            assert!(
                pool.fee.0 > 0,
                "Chain {} pool #{} has zero fee",
                chain.name(),
                idx
            );

            // Reserve validity
            match pool.state {
                PoolState::ConstantProduct { reserve0, reserve1 } => {
                    assert!(
                        reserve0 > 0,
                        "Chain {} pool #{} has zero reserve0",
                        chain.name(),
                        idx
                    );
                    assert!(
                        reserve1 > 0,
                        "Chain {} pool #{} has zero reserve1",
                        chain.name(),
                        idx
                    );
                }
                PoolState::ConcentratedLiquidity {
                    liquidity,
                    sqrt_price_x96,
                    ..
                } => {
                    assert!(
                        liquidity > 0,
                        "Chain {} pool #{} has zero liquidity",
                        chain.name(),
                        idx
                    );
                    assert!(
                        sqrt_price_x96 > U256::ZERO,
                        "Chain {} pool #{} has zero sqrt_price_x96",
                        chain.name(),
                        idx
                    );
                }
            }

            // Clean loading into MarketGraph
            graph.add_pool(pool);
            seen_ids.insert(pool.id);

            // Verify get_pool_tokens immediately after insertion
            assert_eq!(
                graph.get_pool_tokens(pool.id),
                Some((pool.token0, pool.token1)),
                "Failed to retrieve pool tokens for newly added pool {:?}",
                pool.id
            );
            assert!(
                graph.has_pool(&pool.id),
                "graph.has_pool should be true for added pool {:?}",
                pool.id
            );
        }

        // Verify total loaded pools matches unique count
        assert_eq!(
            graph.pool_count(),
            seen_ids.len(),
            "Graph pool count {} does not match seen unique pool count {}",
            graph.pool_count(),
            seen_ids.len()
        );

        // Verify token pair resolution and bidirectional edge lookup for all pools
        for pool in &pools {
            let tokens = graph.get_pool_tokens(pool.id);
            assert_eq!(
                tokens,
                Some((pool.token0, pool.token1)),
                "get_pool_tokens mismatch for pool {:?}",
                pool.id
            );

            let edges_t0 = graph.get_edges(pool.token0);
            assert!(
                edges_t0.iter().any(|e| e.pool_id == pool.id
                    && e.token_in == pool.token0
                    && e.token_out == pool.token1),
                "Missing forward edge from token0 to token1 for pool {:?}",
                pool.id
            );

            let edges_t1 = graph.get_edges(pool.token1);
            assert!(
                edges_t1.iter().any(|e| e.pool_id == pool.id
                    && e.token_in == pool.token1
                    && e.token_out == pool.token0),
                "Missing reverse edge from token1 to token0 for pool {:?}",
                pool.id
            );
        }
    }
}

// =========================================================================
// TEST 2: High-Concurrency Dynamic Pool Insertion & Lock-Free get_pool_tokens
// =========================================================================
#[test]
fn test_concurrent_pool_insertion_and_get_pool_tokens_stress() {
    let graph = Arc::new(MarketGraph::new());

    // Generate a fixed token space of 20 tokens
    let tokens: Vec<Address> = (0..20u8)
        .map(|i| {
            let mut bytes = [0u8; 20];
            bytes[0] = 0xAA;
            bytes[19] = i;
            Address::from(bytes)
        })
        .collect();

    // Generate 120 unique synthetic pools across these tokens
    let mut pool_catalogs = Vec::new();
    let mut pool_counter = 1u16;
    for i in 0..tokens.len() {
        for j in (i + 1)..tokens.len() {
            if pool_catalogs.len() >= 120 {
                break;
            }
            let mut p_bytes = [0u8; 20];
            p_bytes[0] = 0xEE;
            p_bytes[18..20].copy_from_slice(&pool_counter.to_be_bytes());
            pool_counter += 1;

            let pool = DiscoveredPool::new(
                Address::from(p_bytes),
                Venue::UniswapV3,
                Chain::ArbitrumOne,
                tokens[i],
                tokens[j],
                FeePips::from_pips(500),
                PoolState::ConstantProduct {
                    reserve0: 1_000_000_000,
                    reserve1: 2_000_000_000,
                },
            );
            pool_catalogs.push(pool);
        }
    }
    let pool_catalogs = Arc::new(pool_catalogs);

    // 24 threads total: 12 dynamic writers + 12 lock-free readers
    let num_writers = 12;
    let num_readers = 12;
    let total_threads = num_writers + num_readers;
    let barrier = Arc::new(Barrier::new(total_threads + 1));
    let stop_signal = Arc::new(AtomicBool::new(false));

    let writes_done = Arc::new(AtomicUsize::new(0));
    let reads_done = Arc::new(AtomicUsize::new(0));
    let token_resolutions = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();

    // Spawn 12 Writer Threads: dynamically inserting pools and updating reserves
    for w_id in 0..num_writers {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let pools = Arc::clone(&pool_catalogs);
        let stop = Arc::clone(&stop_signal);
        let writes = Arc::clone(&writes_done);

        handles.push(thread::spawn(move || {
            b.wait();
            let mut iter = 0;
            while !stop.load(Ordering::Relaxed) && iter < 1_000 {
                let pool_idx = (w_id * 17 + iter) % pools.len();
                let pool = &pools[pool_idx];

                // Dynamic insertion (must be idempotent & thread-safe)
                g.add_pool(pool);

                // Reserve mutations under lock
                let new_r0 = 1_000_000u128 + (iter as u128 * 10);
                let new_r1 = 2_000_000u128 + (iter as u128 * 20);
                g.update_reserves(pool.id, new_r0, new_r1);

                // CLMM update
                g.update_clmm_reserves(
                    pool.id,
                    50_000_000,
                    U256::from(79228162514264337593543950336u128),
                );

                writes.fetch_add(1, Ordering::Relaxed);
                iter += 1;
            }
        }));
    }

    // Spawn 12 Reader Threads: high-frequency lock-free get_pool_tokens and get_edges
    for r_id in 0..num_readers {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let pools = Arc::clone(&pool_catalogs);
        let stop = Arc::clone(&stop_signal);
        let reads = Arc::clone(&reads_done);
        let resolutions = Arc::clone(&token_resolutions);

        handles.push(thread::spawn(move || {
            b.wait();
            let mut iter = 0;
            while !stop.load(Ordering::Relaxed) && iter < 2_000 {
                let pool_idx = (r_id * 13 + iter) % pools.len();
                let pool = &pools[pool_idx];

                // Lock-free get_pool_tokens check
                if let Some((t0, t1)) = g.get_pool_tokens(pool.id) {
                    assert_eq!(
                        (t0, t1),
                        (pool.token0, pool.token1),
                        "Thread {} observed corrupted token pair for pool {:?}",
                        r_id,
                        pool.id
                    );
                    assert!(
                        g.has_pool(&pool.id),
                        "has_pool returned false despite get_pool_tokens returning Some"
                    );
                    resolutions.fetch_add(1, Ordering::Relaxed);
                }

                // Query non-existent pool must return None
                let fake_pool = Address::from([0xFF; 20]);
                assert_eq!(g.get_pool_tokens(fake_pool), None);
                assert!(!g.has_pool(&fake_pool));

                // Lock-free edges read
                let edges = g.get_edges(pool.token0);
                for edge in edges.iter() {
                    assert_eq!(edge.token_in, pool.token0);
                    assert_ne!(edge.token_out, pool.token0);
                }

                reads.fetch_add(1, Ordering::Relaxed);
                iter += 1;
            }
        }));
    }

    // Synchronize thread launch
    barrier.wait();

    // Strict watchdog: ensure test terminates within 10 seconds without deadlocks
    let start_time = Instant::now();
    let timeout = Duration::from_secs(10);
    while start_time.elapsed() < timeout {
        if writes_done.load(Ordering::Relaxed) >= (num_writers * 1_000)
            && reads_done.load(Ordering::Relaxed) >= (num_readers * 2_000)
        {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    stop_signal.store(true, Ordering::SeqCst);

    assert!(
        start_time.elapsed() < timeout,
        "DEADLOCK DETECTED! Stress test failed to complete within {:?}",
        timeout
    );

    // Join all handles
    for handle in handles {
        handle.join().expect("Worker thread panicked during stress run");
    }

    println!(
        "Stress test passed successfully: {} writes, {} reads, {} token resolutions in {:.2?}",
        writes_done.load(Ordering::SeqCst),
        reads_done.load(Ordering::SeqCst),
        token_resolutions.load(Ordering::SeqCst),
        start_time.elapsed()
    );

    // Post-concurrency verification: Every pool inserted into graph must have exact token resolution
    for pool in pool_catalogs.iter() {
        assert!(
            graph.has_pool(&pool.id),
            "Pool {:?} was not recorded in graph after concurrent writes",
            pool.id
        );
        let resolved = graph.get_pool_tokens(pool.id);
        assert_eq!(
            resolved,
            Some((pool.token0, pool.token1)),
            "Token pair mismatch after concurrent stress"
        );
    }
}

// =========================================================================
// TEST 3: Multicall3 Chunk Size and Slicing Correctness
// =========================================================================
#[test]
fn test_multicall3_chunking_exactness() {
    assert_eq!(
        MULTICALL_CHUNK_SIZE, 150,
        "MULTICALL_CHUNK_SIZE must be exactly 150 per specification"
    );

    let test_sizes = [0, 1, 50, 149, 150, 151, 299, 300, 301, 750, 1000];

    for &total_calls in &test_sizes {
        let dummy_items: Vec<usize> = (0..total_calls).collect();
        let chunks: Vec<&[usize]> = dummy_items.chunks(MULTICALL_CHUNK_SIZE).collect();

        if total_calls == 0 {
            assert!(chunks.is_empty());
        } else {
            let expected_chunks = (total_calls + MULTICALL_CHUNK_SIZE - 1) / MULTICALL_CHUNK_SIZE;
            assert_eq!(
                chunks.len(),
                expected_chunks,
                "Incorrect chunk count for total_calls = {}",
                total_calls
            );

            // Verify each chunk size <= 150
            for (c_idx, chunk) in chunks.iter().enumerate() {
                assert!(
                    chunk.len() <= MULTICALL_CHUNK_SIZE,
                    "Chunk #{} exceeds MULTICALL_CHUNK_SIZE: len = {}",
                    c_idx,
                    chunk.len()
                );
                assert!(!chunk.is_empty(), "Chunk #{} is empty", c_idx);
            }

            // Verify total flattened elements preserve original order and length
            let flattened: Vec<usize> = chunks.iter().flat_map(|c| c.iter().copied()).collect();
            assert_eq!(
                flattened, dummy_items,
                "Flattened chunks do not match original sequence for total_calls = {}",
                total_calls
            );
        }
    }
}

// =========================================================================
// TEST 4: Token Pair Sorting Invariance
// =========================================================================
#[test]
fn test_pool_token_sorting_invariance() {
    let graph = MarketGraph::new();

    let token_low = Address::from([0x10; 20]);
    let token_high = Address::from([0x20; 20]);

    // Pool 1 created with (token_low, token_high)
    let pool_ordered = DiscoveredPool::new(
        Address::from([0x01; 20]),
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        token_low,
        token_high,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 100,
            reserve1: 200,
        },
    );

    // Pool 2 created inverted with (token_high, token_low)
    let pool_inverted = DiscoveredPool::new(
        Address::from([0x02; 20]),
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        token_high,
        token_low,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 100,
            reserve1: 200,
        },
    );

    // Both must maintain canonical token0 < token1
    assert_eq!(pool_ordered.token0, token_low);
    assert_eq!(pool_ordered.token1, token_high);
    assert_eq!(pool_inverted.token0, token_low);
    assert_eq!(pool_inverted.token1, token_high);

    graph.add_pool(&pool_ordered);
    graph.add_pool(&pool_inverted);

    assert_eq!(
        graph.get_pool_tokens(pool_ordered.id),
        Some((token_low, token_high))
    );
    assert_eq!(
        graph.get_pool_tokens(pool_inverted.id),
        Some((token_low, token_high))
    );
}
