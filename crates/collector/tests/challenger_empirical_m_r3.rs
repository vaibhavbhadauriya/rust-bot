//! Adversarial Empirical Challenge Suite for Milestone M_R3
//! Tests C2, C3, C5:
//! 1. Base Flashblock cumulative gas budget calculations and saturation safety
//! 2. Alchemy logs topic parsing for Uniswap V2 Sync (0x1c411e9a...) and Uniswap V3 Swap (0xc42079f9...)
//! 3. Monotonic deduplicator concurrent sequence numbers and reordering behavior
//! 4. Keccak-256 selector and topic0 mathematical ground truth invariants

use alloy_consensus::{Signed, TxEip1559, TxEnvelope};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{keccak256, Address, Bytes, Signature, TxKind, B256, U256};
use base64::Engine;
use flash_collector::{
    ArbitrumFeedHandler, BaseFlashblockHandler, DexCalldataInspector,
    FlashblockGasTracker, MarketFeedPublisher, MarketUpdate, SequencerDeduplicator,
    TOPIC_UNIV2_SYNC, TOPIC_UNIV3_SWAP,
};
use flash_core::types::Chain;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

// ============================================================================
// PART 1: Keccak-256 Mathematical Ground Truth Invariants
// ============================================================================

#[test]
fn test_keccak_mathematical_ground_truth_invariants() {
    // 1. Uniswap V2 Sync Topic0
    let univ2_sync_sig = b"Sync(uint112,uint112)";
    let computed_sync = keccak256(univ2_sync_sig);
    assert_eq!(
        computed_sync, TOPIC_UNIV2_SYNC,
        "TOPIC_UNIV2_SYNC must match keccak256('Sync(uint112,uint112)')"
    );
    assert_eq!(
        format!("0x{:x}", computed_sync),
        "0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"
    );

    // 2. Uniswap V3 Swap Topic0
    let univ3_swap_sig = b"Swap(address,address,int256,int256,uint160,uint128,int24)";
    let computed_swap = keccak256(univ3_swap_sig);
    assert_eq!(
        computed_swap, TOPIC_UNIV3_SWAP,
        "TOPIC_UNIV3_SWAP must match keccak256('Swap(address,address,int256,int256,uint160,uint128,int24)')"
    );
    assert_eq!(
        format!("0x{:x}", computed_swap),
        "0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67"
    );

    // 3. Uniswap V2 swap() Function Selector (C3 Remediation)
    let univ2_swap_fn = b"swap(uint256,uint256,address,bytes)";
    let computed_fn_selector = &keccak256(univ2_swap_fn)[0..4];
    assert_eq!(
        computed_fn_selector,
        &DexCalldataInspector::UNIV2_SWAP,
        "UNIV2_SWAP must be 0x022c0d9f"
    );
    assert_eq!(DexCalldataInspector::UNIV2_SWAP, [0x02, 0x2c, 0x0d, 0x9f]);

    // Negative invariant: Old bugged selector 0x022c089f must NOT match
    let old_bugged_selector = [0x02, 0x2c, 0x08, 0x9f];
    assert_ne!(DexCalldataInspector::UNIV2_SWAP, old_bugged_selector);
    assert!(!DexCalldataInspector::is_dex_swap(&old_bugged_selector));
}

// ============================================================================
// PART 2: Base Flashblock Gas Budgeting & Saturation Safety
// ============================================================================

#[test]
fn test_flashblock_gas_budget_exact_saturation_boundaries() {
    let limit = 30_000_000u64;
    let mut tracker = FlashblockGasTracker::new(limit);

    // Sub-blocks 0 through 9: exact budgets are 3M, 6M, 9M, ..., 30M
    for j in 0..10 {
        let expected_budget = ((j as u64 + 1) * limit) / 10;
        assert_eq!(tracker.sub_block_gas_budget(j), expected_budget);
    }

    // Clamping for out-of-range indices
    for j in 10..=255 {
        assert_eq!(
            tracker.sub_block_gas_budget(j),
            limit,
            "Budget must clamp to 100% of limit for j >= 10"
        );
    }

    // Exact Saturation Safety:
    // Sub-block 0 budget: 3,000,000
    // Fill with exactly 2,999,999 gas -> OK
    assert!(tracker.try_budget_tx(0, 2_999_999).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 2_999_999);

    // Add 1 more gas -> total 3,000,000 == budget -> OK (exact saturation)
    assert!(tracker.try_budget_tx(0, 1).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 3_000_000);

    // Try adding 1 more gas in sub-block 0 -> must fail with Err
    let err = tracker.try_budget_tx(0, 1).unwrap_err();
    assert_eq!(err, "Flashblock gas budget exceeded for sub-block window");
    // Ensure state was NOT modified on error
    assert_eq!(tracker.cumulative_gas_spent, 3_000_000);

    // Transition to sub-block 1 (budget 6,000,000)
    // Spending 3,000,000 gas brings cumulative to 6,000,000 -> OK
    assert!(tracker.try_budget_tx(1, 3_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 6_000_000);

    // Sub-block 1 now saturated
    assert!(tracker.try_budget_tx(1, 1).is_err());
    assert_eq!(tracker.cumulative_gas_spent, 6_000_000);

    // Out-of-order sub-block arrival (retrograde):
    // If a delayed sub-block 0 message arrives, budget for 0 is 3,000,000.
    // Since cumulative is 6,000,000, even 0 gas or 1 gas must fail
    assert!(tracker.try_budget_tx(0, 1).is_err());
    assert_eq!(tracker.cumulative_gas_spent, 6_000_000);

    // Reset block at boundary
    tracker.reset_block();
    assert_eq!(tracker.cumulative_gas_spent, 0);
    assert_eq!(tracker.current_sub_block_index, 0);
}

#[test]
fn test_flashblock_gas_budget_arithmetic_overflow_stress() {
    let mut tracker = FlashblockGasTracker::new(30_000_000);
    tracker.cumulative_gas_spent = 20_000_000;

    // 1. tx_gas = u64::MAX -> must safely return Err via checked_add, no panic or wrap
    assert!(tracker.try_budget_tx(9, u64::MAX).is_err());
    assert_eq!(tracker.cumulative_gas_spent, 20_000_000);

    // 2. tx_gas = u64::MAX - 10_000_000 -> arithmetic overflow
    assert!(tracker.try_budget_tx(9, u64::MAX - 10_000_000).is_err());
    assert_eq!(tracker.cumulative_gas_spent, 20_000_000);

    // 3. Very large total_block_gas_limit
    let large_limit = u64::MAX / 10;
    let large_tracker = FlashblockGasTracker::new(large_limit);
    assert_eq!(large_tracker.sub_block_gas_budget(0), large_limit / 10);
    assert_eq!(large_tracker.sub_block_gas_budget(9), large_limit);
}

#[test]
fn test_flashblock_process_and_reset_lifecycle() {
    let handler = BaseFlashblockHandler::new("wss://mainnet.flashblocks.base.org/ws", 30_000_000);
    let publisher = MarketFeedPublisher::new(64);
    let mut rx = publisher.subscribe();

    // Send sub-blocks 0 through 8 with 2M gas each
    for j in 0..9 {
        let fb_json = format!(
            r#"{{"blockNumber":19000000,"index":{},"gasUsed":2000000,"gasLimit":30000000,"transactions":[]}}"#,
            j
        );
        let handled = handler.process_message_text(&fb_json, &publisher).unwrap();
        assert!(handled);

        let step = rx.try_recv().unwrap();
        match step {
            MarketUpdate::FlashblockStep {
                block_number,
                sub_block_index,
                cumulative_gas_spent,
                gas_budget,
            } => {
                assert_eq!(block_number, 19000000);
                assert_eq!(sub_block_index, j);
                assert_eq!(cumulative_gas_spent, (j as u64 + 1) * 2_000_000);
                assert_eq!(gas_budget, ((j as u64 + 1) * 30_000_000) / 10);
            }
            other => panic!("Unexpected event: {:?}", other),
        }
    }

    // Sub-block 9 (block boundary)
    let fb9_json = r#"{"blockNumber":19000000,"index":9,"gasUsed":2000000,"gasLimit":30000000,"transactions":[]}"#;
    let handled9 = handler.process_message_text(fb9_json, &publisher).unwrap();
    assert!(handled9);

    let step9 = rx.try_recv().unwrap();
    match step9 {
        MarketUpdate::FlashblockStep {
            block_number,
            sub_block_index,
            cumulative_gas_spent,
            gas_budget,
        } => {
            assert_eq!(block_number, 19000000);
            assert_eq!(sub_block_index, 9);
            assert_eq!(cumulative_gas_spent, 20_000_000);
            assert_eq!(gas_budget, 30_000_000);
        }
        other => panic!("Unexpected event: {:?}", other),
    }

    // Because sub_block_index >= 9 was reached, tracker is reset for next block
    let tracker_after = handler.tracker();
    assert_eq!(tracker_after.cumulative_gas_spent, 0);
    assert_eq!(tracker_after.current_sub_block_index, 0);
}

// ============================================================================
// PART 3: Alchemy Logs Topic Parsing for Uniswap V2 Sync and Uniswap V3 Swap
// ============================================================================

#[test]
fn test_alchemy_logs_v2_sync_parsing_boundary_and_malformed() {
    let handler = BaseFlashblockHandler::new("wss://mainnet.flashblocks.base.org/ws", 30_000_000);
    let publisher = MarketFeedPublisher::new(16);
    let mut rx = publisher.subscribe();
    let pool = Address::repeat_byte(0x42);

    // 1. Valid Sync Event (exactly 64 bytes)
    let r0 = 123_456_789_000_000_000u128;
    let r1 = 987_654_321_000_000_000u128;
    let mut data64 = [0u8; 64];
    data64[0..32].copy_from_slice(&U256::from(r0).to_be_bytes::<32>());
    data64[32..64].copy_from_slice(&U256::from(r1).to_be_bytes::<32>());
    let data64_hex = format!("0x{}", alloy_primitives::hex::encode(data64));

    let json_valid = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0x1","result":{{"address":"0x{:x}","topics":["0x{:x}"],"data":"{}"}}}}}}"#,
        pool, TOPIC_UNIV2_SYNC, data64_hex
    );
    assert!(handler.process_message_text(&json_valid, &publisher).unwrap());

    let received = rx.try_recv().unwrap();
    match received {
        MarketUpdate::ReserveUpdate {
            chain,
            pool_id,
            reserve0,
            reserve1,
            ..
        } => {
            assert_eq!(chain, Chain::Base);
            assert_eq!(pool_id, pool);
            assert_eq!(reserve0, r0);
            assert_eq!(reserve1, r1);
        }
        other => panic!("Unexpected event: {:?}", other),
    }

    // 2. Over-length data (e.g. 96 bytes) -> must process first 64 bytes safely
    let mut data96 = vec![0u8; 96];
    data96[0..64].copy_from_slice(&data64);
    data96[64..96].fill(0xff);
    let data96_hex = format!("0x{}", alloy_primitives::hex::encode(&data96));
    let json_overlength = format!(
        r#"{{"address":"0x{:x}","topics":["0x{:x}"],"data":"{}"}}"#,
        pool, TOPIC_UNIV2_SYNC, data96_hex
    );
    assert!(handler.process_message_text(&json_overlength, &publisher).unwrap());
    let received_over = rx.try_recv().unwrap();
    assert!(matches!(received_over, MarketUpdate::ReserveUpdate { reserve0, reserve1, .. } if reserve0 == r0 && reserve1 == r1));

    // 3. Under-length data (<64 bytes) -> must reject safely without panic
    for bad_len in [0, 16, 32, 63] {
        let bad_hex = format!("0x{}", alloy_primitives::hex::encode(vec![0u8; bad_len]));
        let json_bad = format!(
            r#"{{"address":"0x{:x}","topics":["0x{:x}"],"data":"{}"}}"#,
            pool, TOPIC_UNIV2_SYNC, bad_hex
        );
        let handled = handler.process_message_text(&json_bad, &publisher).unwrap();
        assert!(!handled, "Data length {} must be rejected", bad_len);
        assert!(rx.try_recv().is_err());
    }

    // 4. Non-matching topic0 -> must ignore
    let random_topic = B256::repeat_byte(0xee);
    let json_unrelated = format!(
        r#"{{"address":"0x{:x}","topics":["0x{:x}"],"data":"{}"}}"#,
        pool, random_topic, data64_hex
    );
    assert!(!handler.process_message_text(&json_unrelated, &publisher).unwrap());
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_alchemy_logs_v3_swap_tick_decoding_and_negative_sign_extension() {
    let handler = BaseFlashblockHandler::new("wss://mainnet.flashblocks.base.org/ws", 30_000_000);
    let publisher = MarketFeedPublisher::new(16);
    let mut rx = publisher.subscribe();
    let pool = Address::repeat_byte(0x88);

    // Test a suite of critical Uniswap V3 ticks:
    // - Negative ticks: -887272 (MIN_TICK), -123456, -1
    // - Zero tick: 0
    // - Positive ticks: 1, 123456, 887272 (MAX_TICK)
    let test_ticks: Vec<i32> = vec![-887272, -123456, -1, 0, 1, 123456, 887272];

    for tick in test_ticks {
        let sqrt_price_x96 = U256::from(192837465000000000u64);
        let liquidity = 777_888_999_000u128;

        let mut data160 = [0u8; 160];
        // amount0 at 0..32, amount1 at 32..64
        // sqrtPriceX96 at 64..96
        data160[64..96].copy_from_slice(&sqrt_price_x96.to_be_bytes::<32>());
        // liquidity at 96..128
        data160[96..128].copy_from_slice(&U256::from(liquidity).to_be_bytes::<32>());
        // tick is int24 sign-extended to 32 bytes at 128..160
        let sign_byte = if tick < 0 { 0xff } else { 0x00 };
        data160[128..156].fill(sign_byte);
        data160[156..160].copy_from_slice(&tick.to_be_bytes());

        let data_hex = format!("0x{}", alloy_primitives::hex::encode(data160));
        let json_swap = format!(
            r#"{{"address":"0x{:x}","topics":["0x{:x}"],"data":"{}"}}"#,
            pool, TOPIC_UNIV3_SWAP, data_hex
        );

        let handled = handler.process_message_text(&json_swap, &publisher).unwrap();
        assert!(handled, "Swap with tick {} must be handled", tick);

        let event = rx.try_recv().unwrap();
        match event {
            MarketUpdate::TickUpdate {
                chain,
                pool_id,
                sqrt_price_x96: sp,
                current_tick,
                liquidity: liq,
                ..
            } => {
                assert_eq!(chain, Chain::Base);
                assert_eq!(pool_id, pool);
                assert_eq!(sp, sqrt_price_x96);
                assert_eq!(current_tick, tick, "Decoded tick must match exact signed value");
                assert_eq!(liq, liquidity);
            }
            other => panic!("Unexpected event: {:?}", other),
        }
    }

    // Under-length data (<160 bytes)
    for bad_len in [0, 64, 128, 159] {
        let bad_hex = format!("0x{}", alloy_primitives::hex::encode(vec![0u8; bad_len]));
        let json_bad = format!(
            r#"{{"address":"0x{:x}","topics":["0x{:x}"],"data":"{}"}}"#,
            pool, TOPIC_UNIV3_SWAP, bad_hex
        );
        let handled = handler.process_message_text(&json_bad, &publisher).unwrap();
        assert!(!handled, "Swap data length {} must be rejected", bad_len);
        assert!(rx.try_recv().is_err());
    }
}

// ============================================================================
// PART 4: Monotonic Deduplicator Concurrent Sequence Numbers & Reordering
// ============================================================================

#[test]
fn test_dedup_stress_concurrency_and_reordering_safety() {
    let dedup = Arc::new(SequencerDeduplicator::new(10_000));
    let num_threads = 48;
    let ops_per_thread = 200;
    let barrier = Arc::new(Barrier::new(num_threads + 1));
    let done = Arc::new(AtomicBool::new(false));
    let accepted_seqs = Arc::new(Mutex::new(Vec::<u64>::new()));

    // Observer thread continuously asserts monotonicity
    let dedup_obs = Arc::clone(&dedup);
    let done_obs = Arc::clone(&done);
    let barrier_obs = Arc::clone(&barrier);
    let obs_handle = thread::spawn(move || {
        barrier_obs.wait();
        let mut prev = 10_000u64;
        while !done_obs.load(Ordering::Relaxed) {
            let curr = dedup_obs.current_sequence();
            assert!(
                curr >= prev,
                "MONOTONICITY REGRESSION: curr={} < prev={}",
                curr,
                prev
            );
            prev = curr;
            thread::yield_now();
        }
    });

    let mut handles = Vec::with_capacity(num_threads);
    for t_id in 0..num_threads {
        let d = Arc::clone(&dedup);
        let b = Arc::clone(&barrier);
        let acc = Arc::clone(&accepted_seqs);
        handles.push(thread::spawn(move || {
            b.wait();
            // Thread generates a mix of ordered, out-of-order, and duplicate sequences
            for i in 0..ops_per_thread {
                let seq = if t_id % 3 == 0 {
                    // Ascending bursts
                    10_001 + (i * 10) as u64
                } else if t_id % 3 == 1 {
                    // Descending reversals
                    20_000u64.saturating_sub((i * 5) as u64)
                } else {
                    // Duplicate collisions around 15_000
                    15_000 + ((t_id + i) % 50) as u64
                };

                if d.check_and_update(seq) {
                    let mut log = acc.lock().unwrap();
                    log.push(seq);
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }
    done.store(true, Ordering::Relaxed);
    obs_handle.join().unwrap();

    let mut accepted = accepted_seqs.lock().unwrap().clone();
    assert!(!accepted.is_empty());

    // Invariant 1: All accepted sequence numbers must be strictly unique
    let pre_sort_len = accepted.len();
    accepted.sort_unstable();
    accepted.dedup();
    assert_eq!(
        accepted.len(),
        pre_sort_len,
        "Every accepted sequence number must be completely unique"
    );

    // Invariant 2: Current sequence in atomic must match the absolute max accepted
    let max_acc = *accepted.last().unwrap();
    assert_eq!(dedup.current_sequence(), max_acc);

    // Invariant 3: Any sequence <= max_acc must now return false
    assert!(!dedup.check_and_update(max_acc));
    assert!(!dedup.check_and_update(max_acc - 1));
    assert!(!dedup.check_and_update(10_000));
}

#[test]
fn test_dedup_end_to_end_feed_message_reordering_and_deduplication() {
    let handler = ArbitrumFeedHandler::new("wss://arb1-feed.arbitrum.io/feed");
    let publisher = MarketFeedPublisher::new(16);
    let mut rx = publisher.subscribe();

    // Helper to build a valid signed EIP-1559 tx frame
    let build_frame = |seq: u64| -> String {
        let tx = TxEip1559 {
            chain_id: 42161,
            nonce: 1,
            gas_limit: 250_000,
            max_fee_per_gas: 2_000_000_000,
            max_priority_fee_per_gas: 100_000_000,
            to: TxKind::Call(Address::repeat_byte(0x33)),
            value: U256::ZERO,
            access_list: Default::default(),
            input: Bytes::from(vec![0x02, 0x2c, 0x0d, 0x9f, 0x11, 0x22]), // UNIV2_SWAP
        };
        let sig = Signature::from_scalars_and_parity(B256::repeat_byte(1), B256::repeat_byte(2), false);
        let signed = Signed::new_unchecked(tx, sig, B256::ZERO);
        let envelope = TxEnvelope::Eip1559(signed);
        let mut encoded = Vec::new();
        envelope.encode_2718(&mut encoded);

        let mut signed_msg = vec![0x03];
        signed_msg.extend_from_slice(&encoded);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&signed_msg);

        format!(
            r#"{{"version":1,"messages":[{{"sequenceNumber":{},"message":{{"message":{{"header":null,"l2msg":"{}"}}}}}}]}}"#,
            seq, b64
        )
    };

    // Sequence arrivals:
    // 1. Seq 100 -> Accepted
    assert_eq!(handler.process_feed_message(&build_frame(100), &publisher).unwrap(), 1);
    let mut event = rx.try_recv().unwrap();
    assert!(matches!(event, MarketUpdate::PendingTxMutation { sequence_number: 100, .. }));

    // 2. Seq 90 (reordered / retrograde) -> Dropped
    assert_eq!(handler.process_feed_message(&build_frame(90), &publisher).unwrap(), 0);
    assert!(rx.try_recv().is_err());

    // 3. Seq 100 (exact duplicate) -> Dropped
    assert_eq!(handler.process_feed_message(&build_frame(100), &publisher).unwrap(), 0);
    assert!(rx.try_recv().is_err());

    // 4. Seq 105 -> Accepted
    assert_eq!(handler.process_feed_message(&build_frame(105), &publisher).unwrap(), 1);
    event = rx.try_recv().unwrap();
    assert!(matches!(event, MarketUpdate::PendingTxMutation { sequence_number: 105, .. }));

    // 5. Seq 102 (stale relative to 105) -> Dropped
    assert_eq!(handler.process_feed_message(&build_frame(102), &publisher).unwrap(), 0);
    assert!(rx.try_recv().is_err());
}
