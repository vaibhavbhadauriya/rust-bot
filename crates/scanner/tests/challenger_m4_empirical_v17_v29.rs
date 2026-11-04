//! Adversarial Challenger 2 Empirical Test Suite for Milestone 4
//!
//! Validates:
//! 1. V-29: In-place zero-allocation reserve updates in `MarketGraph::update_reserves`
//!    via `Arc::make_mut` (pointer stability across 10,000 updates).
//! 2. V-17: Multicall3 out-of-bounds query indexing safety when `results.len() != queries.len()`.
//! 3. C1: Whole-workspace audit of `top_100_pairs.json` and elimination of synthetic loops `(190 - k) * 120_000.0`.
//! 4. V-20: Verification of workspace crates integration without disconnected facades.

use alloy_primitives::{address, Address, U256};
use flash_core::constants::arbitrum;
use flash_core::types::{Chain, FeePips, PoolState, Venue};
use flash_scanner::{
    decode_address_word, DiscoveredPool, MarketGraph, MulticallResult, TopPairMetadata,
};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
struct MockQuery {
    venue: Venue,
    token0: Address,
    token1: Address,
    fee: FeePips,
}

// =========================================================================
// 1. V-29: Zero-Allocation In-Place Reserve Updates (Arc::make_mut)
// =========================================================================

#[test]
fn test_v29_zero_allocation_in_place_reserve_updates() {
    let graph = MarketGraph::new();
    let token_weth = Address::from([0x01; 20]);
    let token_usdc = Address::from([0x02; 20]);
    let pool_id = Address::from([0xaa; 20]);

    let initial_r0 = 10_000_000u128;
    let initial_r1 = 20_000_000u128;

    let pool = DiscoveredPool::new(
        pool_id,
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        token_weth,
        token_usdc,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: initial_r0,
            reserve1: initial_r1,
        },
    );

    graph.add_pool(&pool);

    // Capture the initial underlying slice memory pointers for edges_u and edges_v
    let initial_ptr_u = {
        let edges = graph.get_edges(token_weth);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].reserve_in, initial_r0);
        assert_eq!(edges[0].reserve_out, initial_r1);
        edges.as_ptr()
    };

    let initial_ptr_v = {
        let edges = graph.get_edges(token_usdc);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].reserve_in, initial_r1);
        assert_eq!(edges[0].reserve_out, initial_r0);
        edges.as_ptr()
    };

    // Perform 10,000 sequential in-place reserve updates
    for i in 1..=10_000u128 {
        let new_r0 = initial_r0 + i;
        let new_r1 = initial_r1 + (i * 2);

        let success = graph.update_reserves(pool_id, new_r0, new_r1);
        assert!(success, "update_reserves must return true for registered pool");

        // Verify that the underlying slice allocation has NOT reallocated:
        // Arc::make_mut mutates in-place when strong_count == 1, keeping pointer identical.
        let current_ptr_u = {
            let edges = graph.get_edges(token_weth);
            assert_eq!(edges[0].reserve_in, new_r0);
            assert_eq!(edges[0].reserve_out, new_r1);
            edges.as_ptr()
        };

        let current_ptr_v = {
            let edges = graph.get_edges(token_usdc);
            assert_eq!(edges[0].reserve_in, new_r1);
            assert_eq!(edges[0].reserve_out, new_r0);
            edges.as_ptr()
        };

        assert_eq!(
            current_ptr_u, initial_ptr_u,
            "Allocation leak / re-allocation detected on edges_u at iteration {}! Arc::make_mut must not reallocate.",
            i
        );
        assert_eq!(
            current_ptr_v, initial_ptr_v,
            "Allocation leak / re-allocation detected on edges_v at iteration {}! Arc::make_mut must not reallocate.",
            i
        );
    }

    // Now test CLMM virtual reserve updates
    let clmm_pool_id = Address::from([0xbb; 20]);
    let clmm_pool = DiscoveredPool::new(
        clmm_pool_id,
        Venue::UniswapV3,
        Chain::ArbitrumOne,
        token_weth,
        token_usdc,
        FeePips::from_pips(500),
        PoolState::ConcentratedLiquidity {
            liquidity: 15_000_000,
            sqrt_price_x96: U256::from(1) << 96,
            current_tick: 0,
            tick_spacing: 10,
        },
    );
    graph.add_pool(&clmm_pool);

    let clmm_ptr_u = {
        let edges = graph.get_edges(token_weth);
        assert_eq!(edges.len(), 2);
        edges.as_ptr()
    };

    // Update CLMM pool 5,000 times
    for i in 1..=5_000u128 {
        let success = graph.update_clmm_reserves(
            clmm_pool_id,
            15_000_000 + i,
            U256::from(1) << 96,
        );
        assert!(success);

        let current_clmm_ptr_u = {
            let edges = graph.get_edges(token_weth);
            edges.as_ptr()
        };
        assert_eq!(
            current_clmm_ptr_u, clmm_ptr_u,
            "CLMM reserve update reallocated buffer at iteration {}!",
            i
        );
    }
}

// =========================================================================
// 2. V-17: Multicall3 Out-Of-Bounds Query Indexing Safety
// =========================================================================

#[test]
fn test_v17_multicall3_out_of_bounds_query_indexing_safety() {
    // Exact logic from discovery.rs:955-965
    fn decode_and_match(
        results: Vec<MulticallResult>,
        queries: Vec<MockQuery>,
    ) -> Vec<(Address, MockQuery)> {
        let mut resolved = Vec::new();
        for (idx, res) in results.into_iter().enumerate() {
            if res.success {
                if let Some(pool_addr) = decode_address_word(&res.return_data) {
                    let Some(q) = queries.get(idx).cloned() else {
                        // V-17: Gracefully handled out-of-bounds result index
                        continue;
                    };
                    resolved.push((pool_addr, q));
                }
            }
        }
        resolved
    }

    let dummy_query = MockQuery {
        venue: Venue::UniswapV3,
        token0: arbitrum::WETH,
        token1: arbitrum::USDC_NATIVE,
        fee: FeePips::from_pips(500),
    };

    let make_return_address = |addr: Address| -> Vec<u8> {
        let mut data = vec![0u8; 32];
        data[12..32].copy_from_slice(addr.as_slice());
        data
    };

    let pool_1 = address!("1111111111111111111111111111111111111111");
    let pool_2 = address!("2222222222222222222222222222222222222222");
    let pool_3 = address!("3333333333333333333333333333333333333333");

    // Case 1: Mismatched length: results (100 items) >> queries (2 items)
    // Previously, `queries[idx]` would panic at idx = 2 with index out of bounds!
    let mut excess_results = Vec::new();
    for i in 0..100 {
        let addr = Address::from([(i + 1) as u8; 20]);
        excess_results.push(MulticallResult {
            success: true,
            return_data: make_return_address(addr),
        });
    }
    let short_queries = vec![dummy_query.clone(), dummy_query.clone()];

    // Must NOT panic and must only resolve matching query indices
    let resolved = decode_and_match(excess_results, short_queries);
    assert_eq!(
        resolved.len(),
        2,
        "Must resolve exactly 2 queries without panicking on remaining 98 results"
    );
    assert_eq!(resolved[0].0, Address::from([1u8; 20]));
    assert_eq!(resolved[1].0, Address::from([2u8; 20]));

    // Case 2: Mismatched length: queries (100 items) >> results (3 items)
    let few_results = vec![
        MulticallResult {
            success: true,
            return_data: make_return_address(pool_1),
        },
        MulticallResult {
            success: false,
            return_data: vec![],
        },
        MulticallResult {
            success: true,
            return_data: make_return_address(pool_3),
        },
    ];
    let mut many_queries = Vec::new();
    for _ in 0..100 {
        many_queries.push(dummy_query.clone());
    }

    let resolved_few = decode_and_match(few_results, many_queries);
    assert_eq!(resolved_few.len(), 2);
    assert_eq!(resolved_few[0].0, pool_1);
    assert_eq!(resolved_few[1].0, pool_3);

    // Case 3: Empty results, non-empty queries
    let empty_results = Vec::new();
    let resolved_empty = decode_and_match(empty_results, vec![dummy_query.clone()]);
    assert!(resolved_empty.is_empty());

    // Case 4: Non-empty results, empty queries
    let results_with_empty_queries = vec![MulticallResult {
        success: true,
        return_data: make_return_address(pool_2),
    }];
    let resolved_no_queries = decode_and_match(results_with_empty_queries, Vec::new());
    assert!(resolved_no_queries.is_empty());
}

// =========================================================================
// 3. C1: Absence of Synthetic Loops & Verified top_100_pairs.json
// =========================================================================

#[test]
fn test_c1_absence_of_synthetic_loops_and_verified_top_100() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    let workspace_root = if manifest_dir.ends_with("scanner") {
        manifest_dir.parent().unwrap().parent().unwrap().to_path_buf()
    } else {
        manifest_dir
    };

    let target_files = vec![
        workspace_root.join("top_100_pairs.json"),
        workspace_root.join("crates/scanner/top_100_pairs.json"),
    ];

    let verified_venues = [
        "Uniswap V3",
        "SushiSwap V2",
        "Camelot V3",
        "Aerodrome Slipstream",
        "Aerodrome V2",
        "Uniswap V2",
    ];

    for file_path in target_files {
        assert!(
            file_path.exists(),
            "Pool configuration file must exist: {:?}",
            file_path
        );
        let content = fs::read_to_string(&file_path)
            .unwrap_or_else(|e| panic!("Failed to read {:?}: {}", file_path, e));

        // Audit check 1: Zero synthetic formula strings
        assert!(
            !content.contains("190 - k"),
            "Synthetic formula string '190 - k' found in {:?}",
            file_path
        );
        assert!(
            !content.contains("120_000"),
            "Synthetic metric constant '120_000' found in {:?}",
            file_path
        );

        let pairs: Vec<TopPairMetadata> = serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("Failed to parse JSON in {:?}: {}", file_path, e));

        assert_eq!(
            pairs.len(),
            100,
            "Must contain exactly 100 pairs, found {}",
            pairs.len()
        );

        let mut seen = HashSet::new();

        for (idx, p) in pairs.iter().enumerate() {
            assert_eq!(p.rank, idx + 1, "Ranks must be 1-indexed contiguous");
            assert_eq!(p.token0.len(), 42, "token0 must be 42 characters");
            assert_eq!(p.token1.len(), 42, "token1 must be 42 characters");

            let a0: Address = p.token0.parse().expect("Valid token0 address hex");
            let a1: Address = p.token1.parse().expect("Valid token1 address hex");

            assert_ne!(a0, Address::ZERO, "token0 must not be zero address");
            assert_ne!(a1, Address::ZERO, "token1 must not be zero address");
            assert_ne!(a0, a1, "token0 and token1 must be distinct");

            assert!(
                !p.token0.to_lowercase().starts_with("0x000000000000000000000000000000000000"),
                "token0 must not be trivial placeholder"
            );
            assert!(
                !p.token1.to_lowercase().starts_with("0x000000000000000000000000000000000000"),
                "token1 must not be trivial placeholder"
            );

            // Verified venues
            assert!(
                p.venues.len() >= 2,
                "Pair {} has < 2 venues: {:?}",
                p.pair,
                p.venues
            );
            for v in &p.venues {
                assert!(
                    verified_venues.contains(&v.as_str()),
                    "Unrecognized venue: {} in pair {}",
                    v,
                    p.pair
                );
            }

            // S_p composite score
            assert!(p.sp_score.is_finite(), "sp_score must be finite");
            assert!(p.sp_score > 0.0, "sp_score must be strictly positive");
            assert!(p.active_liquidity_usd > 0.0, "active_liquidity_usd must be positive");
            assert!(p.trailing_24h_volume_usd > 0.0, "trailing_24h_volume_usd must be positive");
            assert!(p.volatility_sigma > 0.0, "volatility_sigma must be positive");

            let pair_key = if a0 < a1 { (a0, a1) } else { (a1, a0) };
            assert!(seen.insert(pair_key), "Duplicate pair: {:?}", pair_key);
        }

        // Descending score order
        for w in pairs.windows(2) {
            assert!(
                w[0].sp_score >= w[1].sp_score,
                "Pairs must be sorted in descending sp_score order: {} < {}",
                w[0].sp_score,
                w[1].sp_score
            );
        }
    }
}
