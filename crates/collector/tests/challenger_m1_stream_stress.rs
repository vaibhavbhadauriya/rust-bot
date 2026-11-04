//! Empirical Stress Test Harness for Milestone 1 (R1) - Collector & Ingestion
//! Verified by M1 Stream Challenger 1

use alloy_primitives::{Address, B256, U256};
use flash_collector::arbitrum::{
    build_arbitrum_logs_subscribe_request, DexCalldataInspector,
    TOPIC_PANCAKE_V3_SWAP as ARB_PANCAKE_TOPIC,
    TOPIC_UNIV2_SYNC as ARB_V2_TOPIC, TOPIC_UNIV3_SWAP as ARB_V3_TOPIC,
};
use flash_collector::base::{
    build_alchemy_logs_subscribe_request, process_dex_log, process_json_rpc_log,
    TOPIC_PANCAKE_V3_SWAP, TOPIC_UNIV2_SYNC, TOPIC_UNIV3_SWAP,
};
use flash_collector::feed::{MarketFeedPublisher, MarketUpdate};
use flash_core::types::Chain;
use std::panic::{catch_unwind, AssertUnwindSafe};

// ============================================================================
// 1. Topic Matching Tests: V2 Sync, V3 Swap, PancakeSwap V3 / Algebra
// ============================================================================

#[test]
fn test_empirical_topic_constants_and_hashes() {
    let univ2_sync_expected = alloy_primitives::keccak256(b"Sync(uint112,uint112)");
    assert_eq!(TOPIC_UNIV2_SYNC, univ2_sync_expected);
    assert_eq!(ARB_V2_TOPIC, univ2_sync_expected);
    assert_eq!(
        format!("0x{:x}", TOPIC_UNIV2_SYNC),
        "0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"
    );

    let univ3_swap_expected = alloy_primitives::keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24)");
    assert_eq!(TOPIC_UNIV3_SWAP, univ3_swap_expected);
    assert_eq!(ARB_V3_TOPIC, univ3_swap_expected);
    assert_eq!(
        format!("0x{:x}", TOPIC_UNIV3_SWAP),
        "0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67"
    );

    assert_eq!(TOPIC_PANCAKE_V3_SWAP, ARB_PANCAKE_TOPIC);
    assert_eq!(
        format!("0x{:x}", TOPIC_PANCAKE_V3_SWAP),
        "0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83"
    );
}

#[test]
fn test_empirical_subscription_payloads() {
    let req_base = build_alchemy_logs_subscribe_request();
    let req_arb = build_arbitrum_logs_subscribe_request();
    assert_eq!(req_base, req_arb, "Arbitrum and Base subscription builders should match");

    let val: serde_json::Value = serde_json::from_str(&req_base).expect("Valid JSON-RPC payload");
    assert_eq!(val["method"], "eth_subscribe");
    assert_eq!(val["params"][0], "logs");

    let topics = val["params"][1]["topics"][0].as_array().expect("topics array");
    assert_eq!(topics.len(), 3);
    let topic_strings: Vec<&str> = topics.iter().filter_map(|t| t.as_str()).collect();

    assert!(topic_strings.contains(&"0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"));
    assert!(topic_strings.contains(&"0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67"));
    assert!(topic_strings.contains(&"0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83"));
}

#[test]
fn test_empirical_topic_dispatch_routing() {
    let publisher = MarketFeedPublisher::new(32);
    let mut rx = publisher.subscribe();
    let pool = Address::repeat_byte(0x11);

    // 1. V2 Sync on Arbitrum One
    let mut v2_data = [0u8; 64];
    v2_data[0..32].copy_from_slice(&U256::from(5000u64).to_be_bytes::<32>());
    v2_data[32..64].copy_from_slice(&U256::from(10000u64).to_be_bytes::<32>());
    assert!(process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &v2_data, &publisher));
    let ev1 = rx.try_recv().unwrap();
    match ev1 {
        MarketUpdate::ReserveUpdate { chain, pool_id, reserve0, reserve1, .. } => {
            assert_eq!(chain, Chain::ArbitrumOne);
            assert_eq!(pool_id, pool);
            assert_eq!(reserve0, 5000);
            assert_eq!(reserve1, 10000);
        }
        _ => panic!("Expected ReserveUpdate"),
    }

    // 2. V3 Swap on Base
    let mut v3_data = [0u8; 160];
    v3_data[64..96].copy_from_slice(&U256::from(20000u64).to_be_bytes::<32>());
    v3_data[96..128].copy_from_slice(&U256::from(30000u64).to_be_bytes::<32>());
    v3_data[156..160].copy_from_slice(&(-50i32).to_be_bytes());
    assert!(process_dex_log(Chain::Base, pool, &[TOPIC_UNIV3_SWAP], &v3_data, &publisher));
    let ev2 = rx.try_recv().unwrap();
    match ev2 {
        MarketUpdate::TickUpdate { chain, pool_id, sqrt_price_x96, current_tick, liquidity, .. } => {
            assert_eq!(chain, Chain::Base);
            assert_eq!(pool_id, pool);
            assert_eq!(sqrt_price_x96, U256::from(20000u64));
            assert_eq!(current_tick, -50);
            assert_eq!(liquidity, 30000);
        }
        _ => panic!("Expected TickUpdate"),
    }

    // 3. PancakeSwap V3 / Algebra Swap on Arbitrum One
    assert!(process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_PANCAKE_V3_SWAP], &v3_data, &publisher));
    let ev3 = rx.try_recv().unwrap();
    match ev3 {
        MarketUpdate::TickUpdate { chain, current_tick, .. } => {
            assert_eq!(chain, Chain::ArbitrumOne);
            assert_eq!(current_tick, -50);
        }
        _ => panic!("Expected TickUpdate for PancakeSwap V3 on Arbitrum"),
    }

    // 4. Multiple topics in slice: topic0 is prioritized
    assert!(process_dex_log(
        Chain::ArbitrumOne,
        pool,
        &[TOPIC_UNIV2_SYNC, TOPIC_UNIV3_SWAP],
        &v2_data,
        &publisher
    ));
    assert!(matches!(rx.try_recv().unwrap(), MarketUpdate::ReserveUpdate { .. }));

    // 5. Unrecognized topic must be safely ignored
    let unknown_topic = B256::repeat_byte(0x77);
    assert!(!process_dex_log(Chain::ArbitrumOne, pool, &[unknown_topic], &v3_data, &publisher));
    assert!(rx.try_recv().is_err());

    // 6. Empty topics slice must return false
    assert!(!process_dex_log(Chain::ArbitrumOne, pool, &[], &v3_data, &publisher));
    assert!(rx.try_recv().is_err());
}

// ============================================================================
// 2. V2 Reserve Decoding Edge Cases
// ============================================================================

#[test]
fn test_empirical_v2_reserve_decoding_boundaries() {
    let publisher = MarketFeedPublisher::new(32);
    let mut rx = publisher.subscribe();
    let pool = Address::repeat_byte(0x22);

    // Case 2a: Zero reserves (reserve0 = 0, reserve1 = 0)
    let zero_data = [0u8; 64];
    assert!(process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &zero_data, &publisher));
    match rx.try_recv().unwrap() {
        MarketUpdate::ReserveUpdate { reserve0, reserve1, .. } => {
            assert_eq!(reserve0, 0);
            assert_eq!(reserve1, 0);
        }
        _ => panic!("Expected ReserveUpdate for zero reserves"),
    }

    // Case 2b: Maximum uint112 values: (1 << 112) - 1 = 5192296858534827628530496329220095
    let max_u112: u128 = (1u128 << 112) - 1;
    let mut max_u112_data = [0u8; 64];
    max_u112_data[0..32].copy_from_slice(&U256::from(max_u112).to_be_bytes::<32>());
    max_u112_data[32..64].copy_from_slice(&U256::from(max_u112).to_be_bytes::<32>());
    assert!(process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &max_u112_data, &publisher));
    match rx.try_recv().unwrap() {
        MarketUpdate::ReserveUpdate { reserve0, reserve1, .. } => {
            assert_eq!(reserve0, max_u112);
            assert_eq!(reserve1, max_u112);
        }
        _ => panic!("Expected ReserveUpdate for max uint112"),
    }

    // Case 2c: Asymmetric boundaries: reserve0 = 1, reserve1 = max_u112
    let mut asym_data = [0u8; 64];
    asym_data[0..32].copy_from_slice(&U256::from(1u64).to_be_bytes::<32>());
    asym_data[32..64].copy_from_slice(&U256::from(max_u112).to_be_bytes::<32>());
    assert!(process_dex_log(Chain::Base, pool, &[TOPIC_UNIV2_SYNC], &asym_data, &publisher));
    match rx.try_recv().unwrap() {
        MarketUpdate::ReserveUpdate { reserve0, reserve1, .. } => {
            assert_eq!(reserve0, 1);
            assert_eq!(reserve1, max_u112);
        }
        _ => panic!("Expected ReserveUpdate for asymmetric reserves"),
    }

    // Case 2d: Maximum u128 values: u128::MAX
    let mut max_u128_data = [0u8; 64];
    max_u128_data[0..32].copy_from_slice(&U256::from(u128::MAX).to_be_bytes::<32>());
    max_u128_data[32..64].copy_from_slice(&U256::from(u128::MAX).to_be_bytes::<32>());
    assert!(process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &max_u128_data, &publisher));
    match rx.try_recv().unwrap() {
        MarketUpdate::ReserveUpdate { reserve0, reserve1, .. } => {
            assert_eq!(reserve0, u128::MAX);
            assert_eq!(reserve1, u128::MAX);
        }
        _ => panic!("Expected ReserveUpdate for max u128"),
    }

    // Case 2d-2: Adversarial reserve exceeding u128::MAX (e.g. U256::MAX)
    // Note: uint112 max is 5.19e33, whereas u128::MAX is 3.4e38.
    // Checked u128::try_from handles conversion safely without panicking, returning false.
    let over_u128_data = [0xffu8; 64];
    let res_over = catch_unwind(AssertUnwindSafe(|| {
        process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &over_u128_data, &publisher)
    }));
    assert!(res_over.is_ok(), "Payload > u128::MAX must not panic");
    assert!(!res_over.unwrap(), "Payload > u128::MAX must return false");

    // Case 2e: Odd byte lengths and truncations (must NOT panic and must return false if < 64 bytes)
    for bad_len in [0, 1, 7, 15, 31, 32, 45, 63] {
        let truncated = vec![0xaa; bad_len];
        let res = catch_unwind(AssertUnwindSafe(|| {
            process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &truncated, &publisher)
        }));
        assert!(res.is_ok(), "Under-length data of len {} must not panic", bad_len);
        assert!(!res.unwrap(), "Under-length data of len {} must return false", bad_len);
    }

    // Case 2f: Over-length data (e.g. 65, 96, 128, 512 bytes) -> must process first 64 bytes safely
    for extra_len in [65, 96, 128, 512] {
        let mut overlength = vec![0u8; extra_len];
        overlength[0..32].copy_from_slice(&U256::from(42u64).to_be_bytes::<32>());
        overlength[32..64].copy_from_slice(&U256::from(84u64).to_be_bytes::<32>());
        assert!(process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &overlength, &publisher));
        match rx.try_recv().unwrap() {
            MarketUpdate::ReserveUpdate { reserve0, reserve1, .. } => {
                assert_eq!(reserve0, 42);
                assert_eq!(reserve1, 84);
            }
            _ => panic!("Expected ReserveUpdate for overlength"),
        }
    }
}

#[test]
fn test_empirical_v2_json_rpc_hex_odd_lengths_and_corruptions() {
    let publisher = MarketFeedPublisher::new(16);
    let pool = Address::repeat_byte(0x33);

    // 1. Odd hex character length in JSON (e.g. "0x123")
    let odd_hex_json = serde_json::json!({
        "address": format!("0x{:x}", pool),
        "topics": [format!("0x{:x}", TOPIC_UNIV2_SYNC)],
        "data": "0x123"
    });
    assert!(!process_json_rpc_log(Chain::ArbitrumOne, &odd_hex_json, &publisher));

    // 2. Corrupt non-hex data (e.g. "0xZZZZ")
    let non_hex_json = serde_json::json!({
        "address": format!("0x{:x}", pool),
        "topics": [format!("0x{:x}", TOPIC_UNIV2_SYNC)],
        "data": "0xZZZZ"
    });
    assert!(!process_json_rpc_log(Chain::ArbitrumOne, &non_hex_json, &publisher));

    // 3. Invalid address format
    let bad_addr_json = serde_json::json!({
        "address": "0xinvalidaddress",
        "topics": [format!("0x{:x}", TOPIC_UNIV2_SYNC)],
        "data": "0x00"
    });
    assert!(!process_json_rpc_log(Chain::ArbitrumOne, &bad_addr_json, &publisher));

    // 4. Missing fields
    let missing_data = serde_json::json!({
        "address": format!("0x{:x}", pool),
        "topics": [format!("0x{:x}", TOPIC_UNIV2_SYNC)]
    });
    assert!(!process_json_rpc_log(Chain::ArbitrumOne, &missing_data, &publisher));
}

// ============================================================================
// 3. Calldata Inspector: All Selectors & Truncated Calldata
// ============================================================================

#[test]
fn test_empirical_calldata_inspector_all_required_selectors() {
    // Requirement R1 selectors:
    // 0x3593564c: Universal Router execute
    // 0x04e45aaf: SwapRouter02 exactInputSingle
    // 0x414bf389: Uniswap V3 exactInputSingle (formerly bugged 0x414bf382)
    // 0xb858183f: SwapRouter exactInput
    // 0x38ed1739: swapExactTokensForTokens
    // 0x022c0d9f: Uniswap V2 swap
    // Also:
    // 0xfff6dcbe: Uniswap V2 sync
    // 0x128acb08: Uniswap V3 swap
    // 0x91108b66: Balancer / Univ4 unlock

    let target_selectors: Vec<([u8; 4], &str)> = vec![
        ([0x35, 0x93, 0x56, 0x4c], "Universal Router execute (0x3593564c)"),
        ([0x04, 0xe4, 0x5a, 0xaf], "SwapRouter02 exactInputSingle (0x04e45aaf)"),
        ([0x41, 0x4b, 0xf3, 0x89], "Uniswap V3 exactInputSingle (0x414bf389)"),
        ([0xb8, 0x58, 0x18, 0x3f], "SwapRouter exactInput (0xb858183f)"),
        ([0x38, 0xed, 0x17, 0x39], "swapExactTokensForTokens (0x38ed1739)"),
        ([0x02, 0x2c, 0x0d, 0x9f], "Uniswap V2 swap (0x022c0d9f)"),
        ([0xff, 0xf6, 0xdc, 0xbe], "Uniswap V2 sync (0xfff6dcbe)"),
        ([0x12, 0x8a, 0xcb, 0x08], "Uniswap V3 swap (0x128acb08)"),
        ([0x91, 0x10, 0x8b, 0x66], "Balancer / Univ4 unlock (0x91108b66)"),
    ];

    for (sel, name) in target_selectors {
        // 1. Selector alone (exactly 4 bytes)
        assert!(
            DexCalldataInspector::is_dex_swap(&sel),
            "DexCalldataInspector must recognize {}",
            name
        );
        assert_eq!(DexCalldataInspector::extract_selector(&sel), Some(sel));

        // 2. Selector with full 128-byte payload
        let mut full_payload = sel.to_vec();
        full_payload.extend_from_slice(&[0x11; 128]);
        assert!(
            DexCalldataInspector::is_dex_swap(&full_payload),
            "DexCalldataInspector must recognize {} with calldata arguments",
            name
        );
        assert_eq!(
            DexCalldataInspector::extract_selector(&full_payload),
            Some(sel)
        );
    }
}

#[test]
fn test_empirical_calldata_inspector_typo_fixes_and_adversarial_rejections() {
    // 1. The old 1-byte typo in exactInputSingle was 0x414bf382 (should be 0x414bf389)
    let old_bugged_selector = [0x41, 0x4b, 0xf3, 0x82];
    assert!(
        !DexCalldataInspector::is_dex_swap(&old_bugged_selector),
        "Old bugged selector 0x414bf382 must be rejected"
    );

    // 2. Old bugged V2 selector 0x022c089f
    let old_v2_bug = [0x02, 0x2c, 0x08, 0x9f];
    assert!(
        !DexCalldataInspector::is_dex_swap(&old_v2_bug),
        "Old bugged V2 selector 0x022c089f must be rejected"
    );

    // 3. Random selectors must be rejected
    let random_selectors = [
        [0x00, 0x00, 0x00, 0x00],
        [0xff, 0xff, 0xff, 0xff],
        [0xaa, 0xbb, 0xcc, 0xdd],
        [0x70, 0xa0, 0x82, 0x31], // balanceOf
        [0xa9, 0x05, 0x9c, 0xbb], // transfer
    ];
    for sel in random_selectors {
        assert!(!DexCalldataInspector::is_dex_swap(&sel));
    }
}

#[test]
fn test_empirical_calldata_inspector_truncated_calldata() {
    // Truncated calldata: 0, 1, 2, 3 bytes must return false and None without panic
    let truncated_cases: &[&[u8]] = &[
        &[],
        &[0x35],
        &[0x35, 0x93],
        &[0x35, 0x93, 0x56],
        &[0x04],
        &[0x04, 0xe4, 0x5a],
        &[0x41, 0x4b],
    ];

    for slice in truncated_cases {
        let res = catch_unwind(|| {
            (
                DexCalldataInspector::is_dex_swap(slice),
                DexCalldataInspector::extract_selector(slice),
            )
        });
        assert!(res.is_ok(), "Truncated calldata must not panic");
        let (is_swap, sel) = res.unwrap();
        assert!(!is_swap, "Truncated calldata of len {} must not be identified as swap", slice.len());
        assert_eq!(sel, None, "Truncated calldata of len {} must return None selector", slice.len());
    }
}
