//! Challenger R4 Adversarial Empirical Verification Suite
//!
//! Validates:
//! 1. Concurrency stress on `MarketGraph::add_pool` and `update_reserves` under simultaneous reader and writer load.
//! 2. Verification of `top_100_pairs.json` and canonical pools:
//!    valid address lengths, verified DEX venues, and $R_0/10^{d_0} \ge 1$.
//! 3. Multicall3 batch failure handling (e.g. `allow_failure = true`).
//! 4. 4-tier resilient fallback execution under RPC and cache failures.

use alloy_primitives::{Address, U256};
use flash_core::constants::{arbitrum, base};
use flash_core::types::{Chain, FeePips, PoolState, Venue};
use flash_scanner::discovery::AGGREGATE3_SELECTOR;
use flash_scanner::{
    decode_aggregate3_results, decode_liquidity_return,
    decode_reserves_return, decode_slot0_return, dfs_cyclic_opportunities,
    encode_aggregate3, filter_unit_reserves, scan_mutated_opportunities,
    Call3, DiscoveredPool, MarketGraph, Multicall3Discovery, TopPairMetadata,
};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

fn make_cpmm_pool(
    id: Address,
    venue: Venue,
    chain: Chain,
    token0: Address,
    token1: Address,
    fee_pips: u32,
    reserve0: u128,
    reserve1: u128,
) -> DiscoveredPool {
    DiscoveredPool::new(
        id,
        venue,
        chain,
        token0,
        token1,
        FeePips::from_pips(fee_pips),
        PoolState::ConstantProduct { reserve0, reserve1 },
    )
}

// =========================================================================
// TEST 1: Concurrency stress on MarketGraph::add_pool and update_reserves
// =========================================================================
#[test]
fn test_empirical_concurrency_stress_market_graph() {
    let graph = Arc::new(MarketGraph::new());
    let num_tokens = 16;
    let tokens: Vec<Address> = (0..num_tokens)
        .map(|i| {
            let mut b = [0u8; 20];
            b[19] = (i + 1) as u8;
            Address::from(b)
        })
        .collect();

    // Pre-create a set of pool definitions
    let mut pool_defs = Vec::new();
    let mut pool_id_counter = 100u32;
    for i in 0..num_tokens {
        for j in (i + 1)..num_tokens {
            let mut b = [0u8; 20];
            b[16..20].copy_from_slice(&pool_id_counter.to_be_bytes());
            pool_id_counter += 1;
            let pool_id = Address::from(b);

            pool_defs.push(make_cpmm_pool(
                pool_id,
                if pool_id_counter % 2 == 0 {
                    Venue::UniswapV2
                } else {
                    Venue::SushiSwap
                },
                Chain::ArbitrumOne,
                tokens[i],
                tokens[j],
                3000,
                10_000_000,
                20_000_000,
            ));
        }
    }
    let total_pools = pool_defs.len();

    let num_add_threads = 10;
    let num_update_threads = 10;
    let num_read_threads = 10;
    let num_dfs_threads = 10;
    let total_threads = num_add_threads + num_update_threads + num_read_threads + num_dfs_threads;

    let barrier = Arc::new(Barrier::new(total_threads));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let add_count = Arc::new(AtomicUsize::new(0));
    let update_count = Arc::new(AtomicUsize::new(0));
    let read_count = Arc::new(AtomicUsize::new(0));
    let dfs_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(total_threads);

    // 1. Pool Addition Threads (Writers)
    for t_idx in 0..num_add_threads {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let pools = pool_defs.clone();
        let added = Arc::clone(&add_count);
        let stop = Arc::clone(&stop_flag);

        handles.push(thread::spawn(move || {
            b.wait();
            let mut iter = 0;
            while !stop.load(Ordering::Relaxed) && iter < 500 {
                let pool = &pools[(t_idx + iter) % pools.len()];
                g.add_pool(pool);
                added.fetch_add(1, Ordering::Relaxed);
                iter += 1;
            }
        }));
    }

    // 2. Reserve Update Threads (Writers)
    for t_idx in 0..num_update_threads {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let pools = pool_defs.clone();
        let updated = Arc::clone(&update_count);
        let stop = Arc::clone(&stop_flag);

        handles.push(thread::spawn(move || {
            b.wait();
            let mut iter = 0;
            while !stop.load(Ordering::Relaxed) && iter < 500 {
                let pool = &pools[(t_idx * 7 + iter) % pools.len()];
                let r0 = 10_000_000u128 + (iter as u128 * 100);
                let r1 = 20_000_000u128 - (iter as u128 * 50);
                g.update_reserves(pool.id, r0, r1);
                updated.fetch_add(1, Ordering::Relaxed);
                iter += 1;
            }
        }));
    }

    // 3. Edge Reader Threads (Readers)
    for t_idx in 0..num_read_threads {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let toks = tokens.clone();
        let read = Arc::clone(&read_count);
        let stop = Arc::clone(&stop_flag);

        handles.push(thread::spawn(move || {
            b.wait();
            let mut iter = 0;
            while !stop.load(Ordering::Relaxed) && iter < 500 {
                let token = toks[(t_idx + iter) % toks.len()];
                let edges = g.get_edges(token);
                // Invariant: each edge must be internally consistent
                for edge in edges.iter() {
                    assert_eq!(edge.token_in, token);
                    assert_ne!(edge.token_in, edge.token_out);
                    assert!(edge.reserve_in > 0);
                    assert!(edge.reserve_out > 0);
                }
                read.fetch_add(1, Ordering::Relaxed);
                iter += 1;
            }
        }));
    }

    // 4. DFS Cycle Searcher Threads (Readers)
    for t_idx in 0..num_dfs_threads {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let toks = tokens.clone();
        let dfs_c = Arc::clone(&dfs_count);
        let stop = Arc::clone(&stop_flag);

        handles.push(thread::spawn(move || {
            b.wait();
            let mut iter = 0;
            while !stop.load(Ordering::Relaxed) && iter < 300 {
                let token = toks[(t_idx + iter) % toks.len()];
                let opps = dfs_cyclic_opportunities(&g, token, 2, 0);
                for opp in opps {
                    assert_eq!(opp.borrow_token, token);
                    assert_eq!(opp.hops.len(), 2);
                    assert_ne!(opp.hops[0].pool_id, opp.hops[1].pool_id);
                    assert!(opp.net_profit > U256::ZERO);
                }
                dfs_c.fetch_add(1, Ordering::Relaxed);
                iter += 1;
            }
        }));
    }

    // Run stress load
    thread::sleep(Duration::from_millis(300));
    stop_flag.store(true, Ordering::Relaxed);

    for h in handles {
        h.join().expect("Worker thread panicked under concurrency stress");
    }

    // Post-stress validation: ensure all pools were added and graph is consistent
    for pool in &pool_defs {
        graph.add_pool(pool);
    }

    assert_eq!(
        graph.pool_count(),
        total_pools,
        "Graph pool count must exactly match total unique pools"
    );
    assert_eq!(
        graph.vertex_count(),
        num_tokens,
        "Graph vertex count must exactly match total unique tokens"
    );

    // Drain mutations
    let mutated = graph.take_mutated_vertices();
    assert!(!mutated.is_empty(), "Mutated vertices must contain touched tokens");
    let mutated_after = graph.take_mutated_vertices();
    assert!(mutated_after.is_empty(), "take_mutated_vertices must clear atomically");

    // Scan mutated opportunities must run safely and produce descending order
    let all_opps = scan_mutated_opportunities(&graph, 2, 0);
    for w in all_opps.windows(2) {
        assert!(
            w[0].net_profit >= w[1].net_profit,
            "Opportunities must be strictly sorted by net profit descending"
        );
    }
}

// =========================================================================
// TEST 2: Verification of top_100_pairs.json and Canonical Pools
// =========================================================================
#[test]
fn test_empirical_top_100_pairs_verification() {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()),
    );
    let workspace_root = if manifest_dir.ends_with("scanner") {
        manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| manifest_dir.clone())
    } else {
        manifest_dir.clone()
    };
    let file1 = workspace_root.join("top_100_pairs.json").to_string_lossy().to_string();
    let file2 = workspace_root.join("crates").join("scanner").join("top_100_pairs.json").to_string_lossy().to_string();
    let files_to_test = [file1, file2];
    let verified_venues: HashSet<&str> = [
        "Uniswap V3",
        "SushiSwap V2",
        "Camelot V3",
        "Balancer V3",
        "Aerodrome Slipstream",
        "Aerodrome V2",
    ]
    .into_iter()
    .collect();

    for file_path in &files_to_test {
        let content = std::fs::read_to_string(file_path)
            .unwrap_or_else(|e| panic!("Failed to read {:?}: {}", file_path, e));
        let pairs: Vec<TopPairMetadata> = serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("Failed to parse JSON in {:?}: {}", file_path, e));

        assert_eq!(
            pairs.len(),
            100,
            "{:?} must contain exactly 100 pairs, found {}",
            file_path,
            pairs.len()
        );

        let mut seen_pairs = HashSet::new();
        for (idx, pair) in pairs.iter().enumerate() {
            // Rank verification
            assert_eq!(pair.rank, idx + 1, "Ranks must be 1-indexed contiguous");

            // Address format and length (42 chars: '0x' + 40 hex digits)
            assert_eq!(
                pair.token0.len(),
                42,
                "token0 address length invalid: {}",
                pair.token0
            );
            assert_eq!(
                pair.token1.len(),
                42,
                "token1 address length invalid: {}",
                pair.token1
            );
            assert!(
                pair.token0.starts_with("0x"),
                "token0 must start with 0x: {}",
                pair.token0
            );
            assert!(
                pair.token1.starts_with("0x"),
                "token1 must start with 0x: {}",
                pair.token1
            );

            // Valid hex parsing into Address
            let addr0: Address = pair
                .token0
                .parse()
                .unwrap_or_else(|e| panic!("token0 invalid hex in {:?}: {}", file_path, e));
            let addr1: Address = pair
                .token1
                .parse()
                .unwrap_or_else(|e| panic!("token1 invalid hex in {:?}: {}", file_path, e));

            // Must NOT be Address::ZERO or synthetic placeholder
            assert_ne!(addr0, Address::ZERO, "token0 must not be Address::ZERO");
            assert_ne!(addr1, Address::ZERO, "token1 must not be Address::ZERO");
            assert_ne!(addr0, addr1, "token0 and token1 must not be identical");
            assert!(
                !pair.token0.to_lowercase().starts_with("0x000000000000000000000000000000000000"),
                "Must not be synthetic placeholder: {}",
                pair.token0
            );
            assert!(
                !pair.token1.to_lowercase().starts_with("0x000000000000000000000000000000000000"),
                "Must not be synthetic placeholder: {}",
                pair.token1
            );

            // Pair uniqueness
            let pair_key = if addr0 < addr1 {
                (addr0, addr1)
            } else {
                (addr1, addr0)
            };
            assert!(
                seen_pairs.insert(pair_key),
                "Duplicate pair detected in {}: {:?}",
                file_path,
                pair_key
            );

            // Verified venues check: venues >= 2 and all belong to verified DEX list
            assert!(
                pair.venues.len() >= 2,
                "Pair must span >= 2 venues, found {} in {}",
                pair.venues.len(),
                file_path
            );
            for venue_str in &pair.venues {
                assert!(
                    verified_venues.contains(venue_str.as_str()),
                    "Venue '{}' is not a verified DEX venue in {}",
                    venue_str,
                    file_path
                );
            }

            // S_p composite score checks
            assert!(pair.sp_score.is_finite(), "S_p must be finite");
            assert!(pair.sp_score > 0.0, "S_p must be positive");
            assert!(pair.active_liquidity_usd > 0.0);
            assert!(pair.trailing_24h_volume_usd > 0.0);
            assert!(pair.volatility_sigma > 0.0);
        }

        // Descending score order
        for w in pairs.windows(2) {
            assert!(
                w[0].sp_score >= w[1].sp_score,
                "Pairs must be sorted strictly descending by S_p in {}",
                file_path
            );
        }
    }

    // Verify Canonical Mainnet Pools satisfy R_0/10^{d_0} >= 1 and R_1/10^{d_1} >= 1
    let pools_arb = Multicall3Discovery::canonical_mainnet_pools(Chain::ArbitrumOne);
    assert!(!pools_arb.is_empty(), "Arbitrum canonical pool list must not be empty");
    for pool in &pools_arb {
        assert_ne!(pool.id, Address::ZERO, "Pool ID must not be zero");
        assert_ne!(pool.token0, Address::ZERO);
        assert_ne!(pool.token1, Address::ZERO);

        let d0 = get_decimals(pool.token0);
        let d1 = get_decimals(pool.token1);

        match pool.state {
            PoolState::ConstantProduct { reserve0, reserve1 } => {
                assert!(
                    filter_unit_reserves(reserve0, d0, reserve1, d1),
                    "Pool {:?} failed unit reserve requirement R0/10^d0 >= 1, R1/10^d1 >= 1. r0={}, d0={}, r1={}, d1={}",
                    pool.id, reserve0, d0, reserve1, d1
                );
            }
            PoolState::ConcentratedLiquidity { .. } => {
                panic!("Canonical pool must provide valid reserves");
            }
        }
    }

    let pools_base = Multicall3Discovery::canonical_mainnet_pools(Chain::Base);
    assert!(!pools_base.is_empty(), "Base canonical pool list must not be empty");
    for pool in &pools_base {
        assert_ne!(pool.id, Address::ZERO);
        assert_ne!(pool.token0, Address::ZERO);
        assert_ne!(pool.token1, Address::ZERO);

        let d0 = get_decimals(pool.token0);
        let d1 = get_decimals(pool.token1);

        match pool.state {
            PoolState::ConstantProduct { reserve0, reserve1 } => {
                assert!(
                    filter_unit_reserves(reserve0, d0, reserve1, d1),
                    "Base Pool {:?} failed unit reserve requirement. r0={}, d0={}, r1={}, d1={}",
                    pool.id, reserve0, d0, reserve1, d1
                );
            }
            PoolState::ConcentratedLiquidity { .. } => {
                panic!("Canonical pool must provide valid reserves");
            }
        }
    }
}

fn get_decimals(token: Address) -> u8 {
    if token == arbitrum::WETH || token == base::WETH || token == arbitrum::ARB || token == base::AERO {
        18
    } else if token == arbitrum::USDC_NATIVE
        || token == arbitrum::USDC_BRIDGED
        || token == arbitrum::USDT
        || token == base::USDC_NATIVE
        || token == base::USDBC_BRIDGED
    {
        6
    } else if token == arbitrum::WBTC || token == base::CBBTC {
        8
    } else {
        18
    }
}

// =========================================================================
// TEST 3: Multicall3 Batch Failure Handling (allow_failure = true)
// =========================================================================
#[test]
fn test_empirical_multicall3_batch_failure_handling() {
    let dummy_target1 = Address::from([0x11; 20]);
    let dummy_target2 = Address::from([0x22; 20]);
    let dummy_target3 = Address::from([0x33; 20]);

    // Build batch of 5 calls: 3 with allow_failure = true, 2 with allow_failure = false
    let calls = vec![
        Call3 {
            target: dummy_target1,
            allow_failure: true,
            call_data: vec![0x09, 0x02, 0xf1, 0xac], // getReserves
        },
        Call3 {
            target: dummy_target2,
            allow_failure: true,
            call_data: vec![0x38, 0x50, 0xc7, 0xbd], // slot0
        },
        Call3 {
            target: dummy_target3,
            allow_failure: false,
            call_data: vec![0x1a, 0x68, 0x42, 0x54], // liquidity
        },
        Call3 {
            target: dummy_target1,
            allow_failure: true,
            call_data: vec![0xde, 0xad, 0xbe, 0xef], // failing method
        },
        Call3 {
            target: dummy_target2,
            allow_failure: true,
            call_data: vec![], // empty calldata
        },
    ];

    let encoded_calldata = encode_aggregate3(&calls);
    assert_eq!(&encoded_calldata[0..4], &AGGREGATE3_SELECTOR);
    assert!(encoded_calldata.len() > 68);

    // =====================================================================
    // Synthesize Multicall3 return data according to ABI:
    // Result[] = [(bool, bytes), (bool, bytes), ...]
    // =====================================================================
    // Call 0: Success, returns (uint112 reserve0 = 1,000,000, uint112 reserve1 = 2,000,000)
    let mut ret0 = vec![0u8; 64];
    ret0[31] = 100; // r0
    ret0[63] = 200; // r1

    // Call 1: Success, returns (uint160 sqrtPriceX96 = 79228162514264337593543950336, int24 tick = 120)
    let mut ret1 = vec![0u8; 64];
    ret1[15] = 1; // dummy sqrt price
    ret1[60..64].copy_from_slice(&120i32.to_be_bytes());

    // Call 2: Success, returns uint128 liquidity = 50,000,000
    let mut ret2 = vec![0u8; 32];
    ret2[28..32].copy_from_slice(&50_000_000u32.to_be_bytes());

    // Call 3: Failure (success = false), returnData = Error("execution reverted")
    let ret3 = b"revert reason: pool inactive".to_vec();

    // Call 4: Failure (success = false), returnData = empty (0 bytes)
    let ret4 = Vec::new();

    let items = vec![
        (true, ret0.clone()),
        (true, ret1.clone()),
        (true, ret2.clone()),
        (false, ret3.clone()),
        (false, ret4.clone()),
    ];

    // Encode Result[] ABI
    let mut abi_return = Vec::new();
    // Offset to array (0x20)
    let mut arr_off = [0u8; 32];
    arr_off[31] = 0x20;
    abi_return.extend_from_slice(&arr_off);

    // Length of array (5)
    let mut arr_len = [0u8; 32];
    arr_len[31] = items.len() as u8;
    abi_return.extend_from_slice(&arr_len);

    let head_size = items.len() * 32;
    let mut cur_tail = head_size;
    let mut encoded_results = Vec::new();

    for (succ, ret_bytes) in &items {
        let mut tup_off = [0u8; 32];
        tup_off[24..32].copy_from_slice(&(cur_tail as u64).to_be_bytes());
        abi_return.extend_from_slice(&tup_off);

        let mut elem = Vec::new();
        // success word
        let mut succ_w = [0u8; 32];
        if *succ {
            succ_w[31] = 1;
        }
        elem.extend_from_slice(&succ_w);

        // bytes offset (0x40 = 64 bytes)
        let mut b_off = [0u8; 32];
        b_off[31] = 0x40;
        elem.extend_from_slice(&b_off);

        // bytes length
        let mut b_len = [0u8; 32];
        b_len[24..32].copy_from_slice(&(ret_bytes.len() as u64).to_be_bytes());
        elem.extend_from_slice(&b_len);

        // bytes content + 32-byte padding
        elem.extend_from_slice(ret_bytes);
        let rem = ret_bytes.len() % 32;
        if rem != 0 {
            elem.extend_from_slice(&vec![0u8; 32 - rem]);
        }

        cur_tail += elem.len();
        encoded_results.push(elem);
    }

    for elem in encoded_results {
        abi_return.extend_from_slice(&elem);
    }

    // Decode aggregate3 results
    let decoded = decode_aggregate3_results(&abi_return)
        .expect("Must decode aggregate3 return bytes with mixed success/failure");

    assert_eq!(decoded.len(), 5);

    // Call 0: Success with reserves
    assert!(decoded[0].success);
    let (r0, r1) = decode_reserves_return(&decoded[0].return_data).unwrap();
    assert_eq!(r0, 100);
    assert_eq!(r1, 200);

    // Call 1: Success with slot0
    assert!(decoded[1].success);
    let (p, tick) = decode_slot0_return(&decoded[1].return_data).unwrap();
    assert!(!p.is_zero());
    assert_eq!(tick, 120);

    // Call 2: Success with liquidity
    assert!(decoded[2].success);
    let liq = decode_liquidity_return(&decoded[2].return_data).unwrap();
    assert_eq!(liq, 50_000_000);

    // Call 3: Failure with revert message
    assert!(!decoded[3].success);
    assert_eq!(decoded[3].return_data, ret3);

    // Call 4: Failure with empty data
    assert!(!decoded[4].success);
    assert!(decoded[4].return_data.is_empty());

    // Error case 1: Truncated data
    let truncated = &abi_return[0..30];
    assert!(decode_aggregate3_results(truncated).is_err());

    // Error case 2: Corrupted offset
    let mut corrupted = abi_return.clone();
    corrupted[31] = 0xff; // offset beyond payload
    assert!(decode_aggregate3_results(&corrupted).is_err());
}

// =========================================================================
// TEST 4: Resilient Fallback 4-Tier Strategy (Tiers A -> B -> C -> D)
// =========================================================================
#[tokio::test]
async fn test_empirical_resilient_fallback_tiering() {
    // Construct discovery with unreachable Tier A RPC
    let discovery = Multicall3Discovery::new(
        Chain::ArbitrumOne,
        "http://127.0.0.1:54321/rpc".to_string(),
    );

    // Tier C & D fallback test
    // 1. With genuine cache file (top_100_pairs.json) -> loads Tier C/D
    let pools_c = discovery
        .discover_with_resilient_fallback("top_100_pairs.json")
        .await;
    assert!(
        !pools_c.is_empty(),
        "Fallback must return pools via Tier C / Tier D"
    );
    for p in &pools_c {
        assert_ne!(p.id, Address::ZERO);
        assert_eq!(p.chain, Chain::ArbitrumOne);
    }

    // 2. With missing cache file -> cleanly falls back to Tier D canonical pools
    let pools_d = discovery
        .discover_with_resilient_fallback("non_existent_cache_file_404.json")
        .await;
    assert!(
        !pools_d.is_empty(),
        "Tier D fallback must return canonical pools when cache file missing"
    );
    assert_eq!(pools_d.len(), pools_c.len());
}
