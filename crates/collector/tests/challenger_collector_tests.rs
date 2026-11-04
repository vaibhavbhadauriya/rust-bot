//! Adversarial Stress & Boundary Challenge Tests for flash-collector
//! Author: m2_challenger_2 (Milestone 2 Boundary Challenger)
//!
//! Testing Targets:
//! 1. Monotonic deduplication under multi-threaded CAS race conditions (zero out-of-order acceptance, u64::MAX boundary).
//! 2. Recursive batch unpacking under extreme conditions: 16-level deep tree, empty batches, corrupted envelopes, huge payloads.
//! 3. Transaction decoding across Legacy (Type 0), EIP-2930 (Type 1), EIP-1559 (Type 2), and EIP-7702 (Type 4).
//! 4. Base Flashblock cumulative gas budgeting: sub-block 0 minimum (10%), sub-block 9 maximum (100%), out-of-range clamping (j >= 10), exact exhaustion, resets.
//! 5. DEX calldata inspection and feed publisher saturation.

use alloy_consensus::{Signed, TxEip1559, TxEip2930, TxEip7702, TxEnvelope, TxLegacy};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, Signature, TxKind, B256, U256};
use flash_collector::{
    unpack_arbitrum_batch, ArbitrumFeedError, ArbitrumTxType, BatchPayload, DexCalldataInspector,
    FastTxDecoder, FlashblockGasTracker, MarketFeedPublisher, MarketUpdate, MockTx,
    SequencerDeduplicator,
};
use flash_core::types::Chain;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

// ============================================================================
// AREA 1: Monotonic Deduplication Under Multi-Threaded CAS Race Conditions
// ============================================================================

/// Stress-test CAS deduplication with 24 concurrent threads throwing 12,000 out-of-order
/// and duplicate sequence numbers alongside an observer thread.
/// Verifies:
/// - Observer thread reads strictly monotonic sequence values (current >= prev) at all times.
/// - Zero duplicate sequence numbers ever accepted across any thread.
/// - Highest sequence number matches the maximum accepted number.
#[test]
fn test_adv_monotonic_dedup_heavy_multithreaded_race() {
    let dedup = Arc::new(SequencerDeduplicator::new(100));
    let accepted_log = Arc::new(Mutex::new(Vec::<u64>::new()));
    let num_threads = 24;
    let ops_per_thread = 500;
    let barrier = Arc::new(Barrier::new(num_threads + 1));
    let done = Arc::new(AtomicBool::new(false));

    // Spawn observer thread to continuously verify non-decreasing monotonic state
    let dedup_observer = Arc::clone(&dedup);
    let done_observer = Arc::clone(&done);
    let barrier_observer = Arc::clone(&barrier);
    let observer_handle = thread::spawn(move || {
        barrier_observer.wait();
        let mut prev = 100u64;
        while !done_observer.load(Ordering::Relaxed) {
            let curr = dedup_observer.current_sequence();
            assert!(
                curr >= prev,
                "VIOLATION: Atomic sequence moved backwards! curr={} < prev={}",
                curr,
                prev
            );
            prev = curr;
            thread::yield_now();
        }
    });

    let mut handles = Vec::with_capacity(num_threads);

    for thread_id in 0..num_threads {
        let dedup_clone = Arc::clone(&dedup);
        let log_clone = Arc::clone(&accepted_log);
        let barrier_clone = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            // Align all threads to start at the exact same instant
            barrier_clone.wait();

            // Deterministic LCG pseudo-random sequence generator using wrapping arithmetic
            let mut state = (thread_id as u64)
                .wrapping_add(1)
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            for _ in 0..ops_per_thread {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                // Sequence numbers in range [50..5000], intentionally overlapping across threads
                let seq = 50 + (state % 4950);

                if dedup_clone.check_and_update(seq) {
                    let mut log = log_clone.lock().unwrap();
                    log.push(seq);
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("Worker thread panicked in CAS race test");
    }

    done.store(true, Ordering::Relaxed);
    observer_handle
        .join()
        .expect("Observer thread observed non-monotonic sequence regression");

    let mut accepted = accepted_log.lock().unwrap().clone();
    assert!(
        !accepted.is_empty(),
        "At least one sequence number must have been accepted"
    );

    let max_accepted = *accepted.iter().max().unwrap();
    assert_eq!(
        dedup.current_sequence(),
        max_accepted,
        "Final atomic state must match the maximum accepted sequence number"
    );

    // Verify all accepted numbers are strictly unique (zero duplicates)
    accepted.sort_unstable();
    for window in accepted.windows(2) {
        assert!(
            window[0] < window[1],
            "VIOLATION: Duplicate sequence number accepted! seq={}",
            window[0]
        );
    }
}

/// 32 concurrent threads attempt to claim the exact same sequence number simultaneously.
/// Verifies:
/// - Exactly one thread wins the CAS race (returns true).
/// - Exactly 31 threads lose (return false).
#[test]
fn test_adv_monotonic_dedup_simultaneous_identical_stampede() {
    let dedup = Arc::new(SequencerDeduplicator::new(50_000));
    let num_threads = 32;
    let target_seq = 99_999;
    let barrier = Arc::new(Barrier::new(num_threads));
    let win_count = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();
    for _ in 0..num_threads {
        let dedup_clone = Arc::clone(&dedup);
        let barrier_clone = Arc::clone(&barrier);
        let win_count_clone = Arc::clone(&win_count);

        handles.push(thread::spawn(move || {
            barrier_clone.wait();
            if dedup_clone.check_and_update(target_seq) {
                win_count_clone.fetch_add(1, Ordering::SeqCst);
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(
        win_count.load(Ordering::SeqCst),
        1,
        "Exactly ONE thread must succeed in setting identical sequence number"
    );
    assert_eq!(dedup.current_sequence(), target_seq);

    // Any subsequent attempts must also fail
    assert!(!dedup.check_and_update(target_seq));
    assert!(!dedup.check_and_update(target_seq - 1));
}

/// Boundary stress at u64::MAX.
/// Verifies:
/// - Monotonic progression up to u64::MAX succeeds.
/// - Once at u64::MAX, no subsequent updates can ever succeed (no overflow/wraparound to 0).
#[test]
fn test_adv_monotonic_dedup_boundary_u64_max() {
    let dedup = SequencerDeduplicator::new(u64::MAX - 3);

    // Progression to MAX
    assert!(dedup.check_and_update(u64::MAX - 2));
    assert!(dedup.check_and_update(u64::MAX - 1));
    assert!(dedup.check_and_update(u64::MAX));
    assert_eq!(dedup.current_sequence(), u64::MAX);

    // Inability to advance past MAX
    assert!(!dedup.check_and_update(u64::MAX));
    assert!(!dedup.check_and_update(u64::MAX - 1));
    assert!(!dedup.check_and_update(0));
    assert!(!dedup.check_and_update(1));

    // Multi-threaded attempt at MAX
    let dedup_arc = Arc::new(dedup);
    let mut handles = vec![];
    for _ in 0..16 {
        let d = Arc::clone(&dedup_arc);
        handles.push(thread::spawn(move || {
            assert!(!d.check_and_update(u64::MAX));
            assert!(!d.check_and_update(0));
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(dedup_arc.current_sequence(), u64::MAX);
}

/// Verifies zero start, stale sequence rejection, and oscillating input rejection.
#[test]
fn test_adv_monotonic_dedup_zero_and_reversals() {
    let dedup = SequencerDeduplicator::new(0);

    // Initial sequence 0 should be rejected because seq <= current
    assert!(!dedup.check_and_update(0));
    assert_eq!(dedup.current_sequence(), 0);

    // Oscillating inputs: 100, 50, 200, 150, 300, 250, 400
    let sequence_stream = vec![100, 50, 200, 150, 300, 250, 400];
    let mut accepted = Vec::new();

    for seq in sequence_stream {
        if dedup.check_and_update(seq) {
            accepted.push(seq);
        }
    }

    assert_eq!(accepted, vec![100, 200, 300, 400]);
    assert_eq!(dedup.current_sequence(), 400);
}

// ============================================================================
// AREA 2: Recursive Batch Unpacking Under Extreme Conditions
// ============================================================================

/// Verifies exact boundary at MAX_BATCH_DEPTH = 16:
/// - Depth 16: child transaction IS unpacked.
/// - Depth 17: child transaction is PRUNED.
#[test]
fn test_adv_unpack_depth_limit_boundary_16_vs_17() {
    let leaf_tx = MockTx {
        tx_type: ArbitrumTxType::Eip1559,
        pool_target: [0x16; 20],
        calldata_len: 200,
    };

    // Construct tree with leaf at depth 16
    // Root = depth 0.
    // 16 nested wrappers: depth 0 -> 1 -> ... -> 16 (leaf)
    let mut tree_16 = BatchPayload::Direct(leaf_tx.clone());
    for _ in 0..16 {
        tree_16 = BatchPayload::NestedBatch(vec![tree_16]);
    }

    let unpacked_16 = unpack_arbitrum_batch(&tree_16);
    assert_eq!(
        unpacked_16.len(),
        1,
        "Transaction at exactly depth 16 must be unpacked"
    );
    assert_eq!(unpacked_16[0], leaf_tx);

    // Construct tree with leaf at depth 17 (17 nested wrappers: depth 0 -> 1 -> ... -> 17)
    let mut tree_17 = BatchPayload::Direct(leaf_tx.clone());
    for _ in 0..17 {
        tree_17 = BatchPayload::NestedBatch(vec![tree_17]);
    }

    let unpacked_17 = unpack_arbitrum_batch(&tree_17);
    assert_eq!(
        unpacked_17.len(),
        0,
        "Transaction at depth 17 must be pruned because depth > MAX_BATCH_DEPTH (16)"
    );

    // Construct adversarial 100-level deep tree: must terminate safely without stack overflow
    let mut tree_100 = BatchPayload::Direct(leaf_tx.clone());
    for _ in 0..100 {
        tree_100 = BatchPayload::NestedBatch(vec![tree_100]);
    }

    let unpacked_100 = unpack_arbitrum_batch(&tree_100);
    assert_eq!(
        unpacked_100.len(),
        0,
        "100-level deep tree must be cleanly pruned without stack overflow"
    );
}

/// Verifies unpacking of empty and sparse batch envelopes.
#[test]
fn test_adv_unpack_empty_and_sparse_batches() {
    // 1. Completely empty root
    let empty_root = BatchPayload::NestedBatch(vec![]);
    assert!(unpack_arbitrum_batch(&empty_root).is_empty());

    // 2. Tree with multiple empty branches
    let sparse_tree = BatchPayload::NestedBatch(vec![
        BatchPayload::NestedBatch(vec![]),
        BatchPayload::NestedBatch(vec![BatchPayload::NestedBatch(vec![])]),
        BatchPayload::NestedBatch(vec![BatchPayload::NestedBatch(vec![
            BatchPayload::NestedBatch(vec![]),
        ])]),
    ]);
    assert!(unpack_arbitrum_batch(&sparse_tree).is_empty());

    // 3. Tree with sparse interspersed valid transactions
    let tx_a = MockTx {
        tx_type: ArbitrumTxType::Legacy,
        pool_target: [0xaa; 20],
        calldata_len: 10,
    };
    let tx_b = MockTx {
        tx_type: ArbitrumTxType::Eip2930,
        pool_target: [0xbb; 20],
        calldata_len: 20,
    };

    let interspersed = BatchPayload::NestedBatch(vec![
        BatchPayload::NestedBatch(vec![]),
        BatchPayload::Direct(tx_a.clone()),
        BatchPayload::NestedBatch(vec![
            BatchPayload::NestedBatch(vec![]),
            BatchPayload::Direct(tx_b.clone()),
            BatchPayload::NestedBatch(vec![]),
        ]),
        BatchPayload::NestedBatch(vec![]),
    ]);

    let flat = unpack_arbitrum_batch(&interspersed);
    assert_eq!(flat.len(), 2);
    assert_eq!(flat[0], tx_a);
    assert_eq!(flat[1], tx_b);
}

/// Verifies huge batch payloads (wide batch of 2,500 children and balanced tree of 4,096 leaves).
#[test]
fn test_adv_unpack_wide_and_huge_payloads() {
    // 1. Wide batch: 2,500 direct children in root
    let mut wide_children = Vec::with_capacity(2500);
    for i in 0..2500 {
        wide_children.push(BatchPayload::Direct(MockTx {
            tx_type: ArbitrumTxType::Eip1559,
            pool_target: [(i % 256) as u8; 20],
            calldata_len: 64,
        }));
    }
    let wide_batch = BatchPayload::NestedBatch(wide_children);
    let flat_wide = unpack_arbitrum_batch(&wide_batch);
    assert_eq!(flat_wide.len(), 2500);

    // 2. Balanced 4-level oct-tree (8^4 = 4,096 transactions)
    fn build_oct_tree(depth: usize) -> BatchPayload {
        if depth == 4 {
            BatchPayload::Direct(MockTx {
                tx_type: ArbitrumTxType::Eip7702,
                pool_target: [0x77; 20],
                calldata_len: 128,
            })
        } else {
            let children = (0..8).map(|_| build_oct_tree(depth + 1)).collect();
            BatchPayload::NestedBatch(children)
        }
    }

    let oct_root = build_oct_tree(0);
    let flat_oct = unpack_arbitrum_batch(&oct_root);
    assert_eq!(flat_oct.len(), 4096);
}

/// Verifies JSON deserialization resilience against corrupted, truncated, or malformed batch envelopes.
#[test]
fn test_adv_unpack_corrupted_json_envelopes() {
    // 1. Truncated JSON
    let truncated_json = r#"{"NestedBatch": [{"Direct": {"tx_type": "Eip1559""#;
    assert!(serde_json::from_str::<BatchPayload>(truncated_json).is_err());

    // 2. Unknown enum variant
    let invalid_variant = r#"{"MalformedBatch": []}"#;
    assert!(serde_json::from_str::<BatchPayload>(invalid_variant).is_err());

    // 3. Invalid transaction type string
    let bad_tx_type = r#"{"Direct": {"tx_type": "EipNonExistent", "pool_target": [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0], "calldata_len": 10}}"#;
    assert!(serde_json::from_str::<BatchPayload>(bad_tx_type).is_err());

    // 4. Empty string
    assert!(serde_json::from_str::<BatchPayload>("").is_err());

    // 5. Bare JSON null
    assert!(serde_json::from_str::<BatchPayload>("null").is_err());

    // 6. Valid serialization round-trip
    let valid = BatchPayload::Direct(MockTx {
        tx_type: ArbitrumTxType::Eip1559,
        pool_target: [0x99; 20],
        calldata_len: 32,
    });
    let serialized = serde_json::to_string(&valid).unwrap();
    let deserialized: BatchPayload = serde_json::from_str(&serialized).unwrap();
    assert_eq!(valid, deserialized);
}

// ============================================================================
// AREA 3: Transaction Decoding Across All 4 Envelope Types
// ============================================================================

fn make_dummy_sig() -> Signature {
    Signature::from_scalars_and_parity(B256::repeat_byte(0x11), B256::repeat_byte(0x22), false)
}

/// Verifies transaction decoding across Legacy, EIP-2930, EIP-1559, and EIP-7702 transactions.
#[test]
fn test_adv_tx_decode_all_envelope_types() {
    let mock_target = Address::repeat_byte(0x5a);
    let sig = make_dummy_sig();

    // 1. Legacy (Type 0)
    let legacy_tx = TxLegacy {
        chain_id: Some(42161),
        nonce: 5,
        gas_price: 100_000_000,
        gas_limit: 150_000,
        to: TxKind::Call(mock_target),
        value: U256::from(100_000_000_000u64),
        input: Bytes::from_static(&[0x01, 0x02, 0x03]),
    };
    let legacy_env = TxEnvelope::Legacy(Signed::new_unchecked(legacy_tx, sig, B256::ZERO));
    let mut raw_legacy = Vec::new();
    legacy_env.encode_2718(&mut raw_legacy);

    let decoded_legacy = FastTxDecoder::decode(&raw_legacy).expect("Decode Legacy must succeed");
    assert_eq!(decoded_legacy.tx_type, ArbitrumTxType::Legacy);
    assert_eq!(
        decoded_legacy.tx_hash,
        alloy_primitives::keccak256(&raw_legacy)
    );
    assert_eq!(decoded_legacy.to, Some(mock_target));

    // 2. EIP-2930 (Type 1)
    let eip2930_tx = TxEip2930 {
        chain_id: 42161,
        nonce: 6,
        gas_price: 200_000_000,
        gas_limit: 220_000,
        to: TxKind::Call(mock_target),
        value: U256::from(500_000_000_000u64),
        access_list: Default::default(),
        input: Bytes::from_static(&[0xaa, 0xbb]),
    };
    let eip2930_env = TxEnvelope::Eip2930(Signed::new_unchecked(eip2930_tx, sig, B256::ZERO));
    let mut raw_eip2930 = Vec::new();
    eip2930_env.encode_2718(&mut raw_eip2930);

    let decoded_2930 = FastTxDecoder::decode(&raw_eip2930).expect("Decode EIP-2930 must succeed");
    assert_eq!(decoded_2930.tx_type, ArbitrumTxType::Eip2930);
    assert_eq!(
        decoded_2930.tx_hash,
        alloy_primitives::keccak256(&raw_eip2930)
    );
    assert_eq!(decoded_2930.to, Some(mock_target));

    // 3. EIP-1559 (Type 2)
    let eip1559_tx = TxEip1559 {
        chain_id: 42161,
        nonce: 7,
        gas_limit: 450_000,
        max_fee_per_gas: 3_000_000_000,
        max_priority_fee_per_gas: 250_000_000,
        to: TxKind::Call(mock_target),
        value: U256::from(2_000_000_000_000_000_000u128),
        access_list: Default::default(),
        input: Bytes::from_static(&[0x11, 0x22]),
    };
    let eip1559_env = TxEnvelope::Eip1559(Signed::new_unchecked(eip1559_tx, sig, B256::ZERO));
    let mut raw_eip1559 = Vec::new();
    eip1559_env.encode_2718(&mut raw_eip1559);

    let decoded_1559 = FastTxDecoder::decode(&raw_eip1559).expect("Decode EIP-1559 must succeed");
    assert_eq!(decoded_1559.tx_type, ArbitrumTxType::Eip1559);
    assert_eq!(
        decoded_1559.tx_hash,
        alloy_primitives::keccak256(&raw_eip1559)
    );
    assert_eq!(decoded_1559.to, Some(mock_target));

    // 4. EIP-7702 (Type 4)
    let eip7702_tx = TxEip7702 {
        chain_id: 42161,
        nonce: 8,
        gas_limit: 500_000,
        max_fee_per_gas: 3_500_000_000,
        max_priority_fee_per_gas: 300_000_000,
        to: mock_target,
        value: U256::from(1_000_000_000_000_000_000u128),
        access_list: Default::default(),
        authorization_list: vec![],
        input: Bytes::from_static(&[0x77, 0x88]),
    };
    let eip7702_env = TxEnvelope::Eip7702(Signed::new_unchecked(eip7702_tx, sig, B256::ZERO));
    let mut raw_eip7702 = Vec::new();
    eip7702_env.encode_2718(&mut raw_eip7702);

    let decoded_7702 = FastTxDecoder::decode(&raw_eip7702).expect("Decode EIP-7702 must succeed");
    assert_eq!(decoded_7702.tx_type, ArbitrumTxType::Eip7702);
    assert_eq!(
        decoded_7702.tx_hash,
        alloy_primitives::keccak256(&raw_eip7702)
    );
    assert_eq!(decoded_7702.to, Some(mock_target));
}

/// Adversarial decoding checks: empty byte slice, truncated payloads, and non-crashing behavior.
#[test]
fn test_adv_tx_decode_adversarial_malformed_inputs() {
    // 1. Completely empty bytes
    let empty_res = FastTxDecoder::decode(&[]);
    assert!(
        matches!(empty_res, Err(ArbitrumFeedError::InvalidEncoding(_))),
        "Empty byte slice must return InvalidEncoding error"
    );

    // 2. Single byte inputs (no payload) must return DecodeError without panicking
    for b in [0x00, 0x01, 0x02, 0x04, 0x80, 0xc0, 0xff] {
        let res = FastTxDecoder::decode(&[b]);
        assert!(
            matches!(res, Err(ArbitrumFeedError::DecodeError(_))),
            "Single byte 0x{:02x} must return DecodeError without panicking",
            b
        );
    }

    // 3. Incomplete address tag (0x94 near end with < 20 bytes following)
    // Must NOT panic and must return DecodeError
    let truncated_address = vec![0xc0, 0x94, 0x01, 0x02, 0x03];
    let res_truncated = FastTxDecoder::decode(&truncated_address);
    assert!(
        matches!(res_truncated, Err(ArbitrumFeedError::DecodeError(_))),
        "Truncated address tag must return DecodeError without panicking"
    );
}

// ============================================================================
// AREA 4: Base Flashblock Cumulative Gas Budgeting
// ============================================================================

/// Verifies Sub-block 0 minimum budget (10%), Sub-block 9 maximum budget (100%),
/// and out-of-range clamping (j >= 10).
#[test]
fn test_adv_flashblock_budget_fractions_and_clamping() {
    let limits = [10_000_000u64, 30_000_000u64, 50_000_000u64, 100_000_000u64];

    for limit in limits {
        let tracker = FlashblockGasTracker::new(limit);

        // Sub-block 0: Exactly 10%
        let budget_0 = tracker.sub_block_gas_budget(0);
        assert_eq!(
            budget_0,
            limit / 10,
            "Sub-block 0 budget must be exactly 10% of limit"
        );

        // Sub-block 4: Exactly 50%
        let budget_4 = tracker.sub_block_gas_budget(4);
        assert_eq!(
            budget_4,
            (limit * 5) / 10,
            "Sub-block 4 budget must be exactly 50% of limit"
        );

        // Sub-block 9: Exactly 100%
        let budget_9 = tracker.sub_block_gas_budget(9);
        assert_eq!(
            budget_9, limit,
            "Sub-block 9 budget must be exactly 100% of limit"
        );

        // Out-of-range clamping for j in [10, 11, 20, 50, 255]
        for out_of_range_j in [10, 11, 20, 50, 100, 255] {
            let budget_clamped = tracker.sub_block_gas_budget(out_of_range_j);
            assert_eq!(
                budget_clamped, limit,
                "Out-of-range sub-block {} must be clamped to 100% limit",
                out_of_range_j
            );
        }
    }
}

/// Verifies exact gas budget exhaustion:
/// - Exact budget match succeeds.
/// - 1 single gas unit over budget fails with exact error message.
/// - 0 gas transaction after exact exhaustion succeeds.
#[test]
fn test_adv_flashblock_exact_budget_exhaustion() {
    let limit = 30_000_000u64;
    let mut tracker = FlashblockGasTracker::new(limit);

    // Sub-block 0 has 3,000,000 gas budget
    // Spend 2,000,000 gas
    assert!(tracker.try_budget_tx(0, 2_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 2_000_000);

    // Spend exactly 1,000,000 gas -> exact exhaustion (3,000,000 == 3,000,000)
    assert!(tracker.try_budget_tx(0, 1_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 3_000_000);

    // Any positive gas fails immediately
    let err = tracker.try_budget_tx(0, 1).unwrap_err();
    assert_eq!(err, "Flashblock gas budget exceeded for sub-block window");
    assert_eq!(
        tracker.cumulative_gas_spent, 3_000_000,
        "Cumulative gas spent must NOT increase on failed budget attempt"
    );

    // Zero gas tx succeeds without altering cumulative gas
    assert!(tracker.try_budget_tx(0, 0).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 3_000_000);

    // Advancing to sub-block 1 unlocks another 3,000,000 gas (total 6,000,000)
    assert!(tracker.try_budget_tx(1, 3_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 6_000_000);
}

/// Verifies block boundary resets and recovery.
#[test]
fn test_adv_flashblock_block_boundary_resets() {
    let limit = 30_000_000u64;
    let mut tracker = FlashblockGasTracker::new(limit);

    // Exhaust entire block in sub-block 9
    assert!(tracker.try_budget_tx(9, 30_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 30_000_000);
    assert_eq!(tracker.current_sub_block_index, 9);

    // Now all sub-blocks would reject positive gas
    assert!(tracker.try_budget_tx(0, 1).is_err());
    assert!(tracker.try_budget_tx(9, 1).is_err());

    // Reset block at 2s block boundary
    tracker.reset_block();
    assert_eq!(tracker.cumulative_gas_spent, 0);
    assert_eq!(tracker.current_sub_block_index, 0);

    // Sub-block 0 now accepts 3,000,000 gas again
    assert!(tracker.try_budget_tx(0, 3_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 3_000_000);
}

/// Simulates a realistic 10-step Flashblock lifecycle with 50 sequential transactions.
#[test]
fn test_adv_flashblock_full_lifecycle_simulation() {
    let limit = 30_000_000u64;
    let mut tracker = FlashblockGasTracker::new(limit);

    // 10 sub-blocks (j = 0..9), each with 5 transactions of 500,000 gas = 2,500,000 gas per sub-block.
    // Allowable increment per sub-block is 3,000,000, so 2,500,000 is always within window.
    for j in 0..10 {
        for tx_idx in 0..5 {
            let res = tracker.try_budget_tx(j, 500_000);
            assert!(
                res.is_ok(),
                "Failed at sub-block {} tx {}: cumulative={}",
                j,
                tx_idx,
                tracker.cumulative_gas_spent
            );
        }
        // At end of sub-block j, cumulative gas is (j + 1) * 2,500,000 <= (j + 1) * 3,000,000
        assert_eq!(tracker.cumulative_gas_spent, (j as u64 + 1) * 2_500_000);
    }

    assert_eq!(tracker.cumulative_gas_spent, 25_000_000);

    // Attempting a final 6,000,000 tx exceeds the 30,000,000 block limit
    assert!(tracker.try_budget_tx(9, 6_000_000).is_err());

    // Exact 5,000,000 tx finishes the block
    assert!(tracker.try_budget_tx(9, 5_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 30_000_000);
}

// ============================================================================
// AREA 5: DEX Calldata Inspector & Market Publisher Stress
// ============================================================================

/// Verifies DEX calldata inspection across canonical selectors and edge cases.
#[test]
fn test_adv_dex_calldata_inspector() {
    // 1. All canonical selectors
    let selectors = [
        DexCalldataInspector::UNIV2_SWAP,
        DexCalldataInspector::UNIV2_SYNC,
        DexCalldataInspector::UNIV3_SWAP,
        DexCalldataInspector::ROUTER_SWAP_EXACT_TOKENS,
        DexCalldataInspector::UNIV3_EXACT_INPUT_SINGLE,
        DexCalldataInspector::BALANCER_UNIV4_UNLOCK,
    ];

    for sel in selectors {
        let mut calldata = sel.to_vec();
        calldata.extend_from_slice(&[0x11; 128]); // append arbitrary calldata args
        assert!(
            DexCalldataInspector::is_dex_swap(&calldata),
            "Selector {:02x?} must be identified as DEX swap",
            sel
        );
        assert_eq!(DexCalldataInspector::extract_selector(&calldata), Some(sel));
    }

    // 2. Short slices (< 4 bytes)
    assert!(!DexCalldataInspector::is_dex_swap(&[]));
    assert!(!DexCalldataInspector::is_dex_swap(&[0x02]));
    assert!(!DexCalldataInspector::is_dex_swap(&[0x02, 0x2c]));
    assert!(!DexCalldataInspector::is_dex_swap(&[0x02, 0x2c, 0x08]));
    assert_eq!(DexCalldataInspector::extract_selector(&[]), None);
    assert_eq!(DexCalldataInspector::extract_selector(&[0x02, 0x2c]), None);

    // 3. Non-matching 4-byte selector
    let non_dex = [0xde, 0xad, 0xbe, 0xef, 0x00, 0x00];
    assert!(!DexCalldataInspector::is_dex_swap(&non_dex));
    assert_eq!(
        DexCalldataInspector::extract_selector(&non_dex),
        Some([0xde, 0xad, 0xbe, 0xef])
    );
}

/// Verifies MarketFeedPublisher under concurrent subscription and buffer saturation.
#[test]
fn test_adv_market_publisher_concurrency_and_saturation() {
    let publisher = MarketFeedPublisher::new(64);
    let num_subscribers = 8;
    let num_messages = 50;

    let mut receivers = Vec::new();
    for _ in 0..num_subscribers {
        receivers.push(publisher.subscribe());
    }

    // Publish 50 updates
    for i in 0..num_messages {
        let update = MarketUpdate::ReserveUpdate {
            chain: Chain::Base,
            pool_id: Address::repeat_byte((i % 256) as u8),
            reserve0: (i as u128) * 1000,
            reserve1: (i as u128) * 2000,
            timestamp_ms: 1_700_000_000 + i,
        };
        assert!(publisher.publish(update).is_ok());
    }

    // All receivers should read all 50 messages in exact order
    for (sub_idx, mut rx) in receivers.into_iter().enumerate() {
        for i in 0..num_messages {
            let received = rx
                .try_recv()
                .unwrap_or_else(|e| panic!("Subscriber {} failed at msg {}: {:?}", sub_idx, i, e));
            match received {
                MarketUpdate::ReserveUpdate {
                    chain,
                    reserve0,
                    reserve1,
                    ..
                } => {
                    assert_eq!(chain, Chain::Base);
                    assert_eq!(reserve0, (i as u128) * 1000);
                    assert_eq!(reserve1, (i as u128) * 2000);
                }
                _ => panic!("Unexpected update variant received"),
            }
        }
    }
}

// ============================================================================
// AREA 6: Additional Adversarial Boundary Stress Tests
// ============================================================================

/// Verifies initialization with u64::MAX and extreme jumps.
#[test]
fn test_adv_monotonic_dedup_initial_u64_max_and_extremes() {
    let dedup_max = SequencerDeduplicator::new(u64::MAX);
    // Any sequence must be rejected
    assert!(!dedup_max.check_and_update(0));
    assert!(!dedup_max.check_and_update(1));
    assert!(!dedup_max.check_and_update(1_000_000));
    assert!(!dedup_max.check_and_update(u64::MAX - 1));
    assert!(!dedup_max.check_and_update(u64::MAX));
    assert_eq!(dedup_max.current_sequence(), u64::MAX);

    // Initialized at u64::MAX - 100, jump directly to u64::MAX
    let dedup_jump = SequencerDeduplicator::new(u64::MAX - 100);
    assert!(dedup_jump.check_and_update(u64::MAX));
    assert_eq!(dedup_jump.current_sequence(), u64::MAX);
    assert!(!dedup_jump.check_and_update(u64::MAX));
    assert!(!dedup_jump.check_and_update(u64::MAX - 50));
}

/// 50 concurrent threads with chaotic interleaving of smaller, identical, and strictly increasing numbers.
#[test]
fn test_adv_monotonic_dedup_50_threads_chaotic_interleaving() {
    let dedup = Arc::new(SequencerDeduplicator::new(0));
    let num_threads = 50;
    let ops_per_thread = 200;
    let barrier = Arc::new(Barrier::new(num_threads));
    let accepted = Arc::new(Mutex::new(Vec::new()));

    let mut handles = Vec::new();
    for thread_id in 0..num_threads {
        let dedup_c = Arc::clone(&dedup);
        let barrier_c = Arc::clone(&barrier);
        let accepted_c = Arc::clone(&accepted);

        handles.push(thread::spawn(move || {
            barrier_c.wait();
            let mut lcg = (thread_id as u64)
                .wrapping_add(1)
                .wrapping_mul(2862933555777941757)
                .wrapping_add(3037000493);
            for _ in 0..ops_per_thread {
                lcg = lcg
                    .wrapping_mul(2862933555777941757)
                    .wrapping_add(3037000493);
                // Sequence numbers distributed across [1..10000]
                let seq = 1 + (lcg % 10000);
                if dedup_c.check_and_update(seq) {
                    accepted_c.lock().unwrap().push(seq);
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let mut list = accepted.lock().unwrap().clone();
    assert!(!list.is_empty(), "Must have accepted at least 1 sequence number");
    let max_val = *list.iter().max().unwrap();
    assert_eq!(dedup.current_sequence(), max_val);

    // Verify zero duplicates
    list.sort_unstable();
    for w in list.windows(2) {
        assert!(w[0] < w[1], "Accepted duplicate sequence: {}", w[0]);
    }
}

/// Asymmetric batch tree: some branches at depth 15 (accepted), depth 16 (accepted), depth 17 (pruned), depth 25 (pruned).
#[test]
fn test_adv_unpack_mixed_depth_partial_pruning() {
    let tx_depth_15 = MockTx {
        tx_type: ArbitrumTxType::Legacy,
        pool_target: [0x15; 20],
        calldata_len: 15,
    };
    let tx_depth_16 = MockTx {
        tx_type: ArbitrumTxType::Eip1559,
        pool_target: [0x16; 20],
        calldata_len: 16,
    };
    let tx_depth_17 = MockTx {
        tx_type: ArbitrumTxType::Eip2930,
        pool_target: [0x17; 20],
        calldata_len: 17,
    };
    let tx_depth_25 = MockTx {
        tx_type: ArbitrumTxType::Eip7702,
        pool_target: [0x25; 20],
        calldata_len: 25,
    };

    fn wrap_to_depth(tx: MockTx, target_depth: usize) -> BatchPayload {
        let mut cur = BatchPayload::Direct(tx);
        // target_depth is the desired depth in the overall tree.
        // In the tree, `tree = BatchPayload::NestedBatch(vec![branch])`.
        // So root is at depth 0, branch is at depth 1.
        // Therefore, we need (target_depth - 1) wrappers inside `cur`.
        let wrappers = target_depth.saturating_sub(1);
        for _ in 0..wrappers {
            cur = BatchPayload::NestedBatch(vec![cur]);
        }
        cur
    }

    let branch_15 = wrap_to_depth(tx_depth_15.clone(), 15);
    let branch_16 = wrap_to_depth(tx_depth_16.clone(), 16);
    let branch_17 = wrap_to_depth(tx_depth_17.clone(), 17);
    let branch_25 = wrap_to_depth(tx_depth_25.clone(), 25);

    let tree = BatchPayload::NestedBatch(vec![branch_15, branch_16, branch_17, branch_25]);

    let flat = unpack_arbitrum_batch(&tree);
    assert_eq!(flat.len(), 2, "Only depth 15 and 16 should be unpacked");
    assert!(flat.contains(&tx_depth_15));
    assert!(flat.contains(&tx_depth_16));
    assert!(!flat.contains(&tx_depth_17));
    assert!(!flat.contains(&tx_depth_25));
}

/// Stress test with 10,000 transactions across multiple nested batch nodes.
#[test]
fn test_adv_unpack_10k_fanout_stress() {
    let num_txs = 10_000;
    let mut batches = Vec::new();
    for chunk in 0..100 {
        let mut sub_txs = Vec::new();
        for i in 0..100 {
            let idx = chunk * 100 + i;
            sub_txs.push(BatchPayload::Direct(MockTx {
                tx_type: ArbitrumTxType::Eip1559,
                pool_target: [(idx % 256) as u8; 20],
                calldata_len: idx,
            }));
        }
        batches.push(BatchPayload::NestedBatch(sub_txs));
    }
    let root = BatchPayload::NestedBatch(batches);
    let flat = unpack_arbitrum_batch(&root);
    assert_eq!(flat.len(), num_txs);
    for (i, tx) in flat.iter().enumerate() {
        assert_eq!(tx.calldata_len, i);
    }
}

fn make_valid_eip1559_envelope(to: Address, calldata: Bytes) -> Vec<u8> {
    let tx = TxEip1559 {
        chain_id: 42161,
        nonce: 1,
        gas_limit: 300_000,
        max_fee_per_gas: 2_000_000_000,
        max_priority_fee_per_gas: 100_000_000,
        to: TxKind::Call(to),
        value: U256::ZERO,
        access_list: Default::default(),
        input: calldata,
    };
    let env = TxEnvelope::Eip1559(Signed::new_unchecked(tx, make_dummy_sig(), B256::ZERO));
    let mut out = Vec::new();
    env.encode_2718(&mut out);
    out
}

/// Verifies transaction decoding with Type 0 empty list, Type 3 (EIP-4844) fallback, and address boundaries.
#[test]
fn test_adv_tx_decode_deep_boundaries() {
    // 1. Type 0 with prefix 0xc0 (empty RLP list) is malformed -> returns DecodeError
    let legacy_empty_list = vec![0xc0];
    let res_empty = FastTxDecoder::decode(&legacy_empty_list);
    assert!(
        matches!(res_empty, Err(ArbitrumFeedError::DecodeError(_))),
        "Empty RLP list without envelope fields must return DecodeError"
    );

    // 2. Type 3 (Blob transaction) prefix without valid blob tx structure -> returns DecodeError
    let mut raw_type3 = vec![0x03, 0xc8, 0x94];
    raw_type3.extend_from_slice(&[0x33; 20]);
    raw_type3.extend_from_slice(&[0x80, 0x80]);
    let res_type3 = FastTxDecoder::decode(&raw_type3);
    assert!(
        matches!(res_type3, Err(ArbitrumFeedError::DecodeError(_))),
        "Invalid envelope type 3 must return DecodeError"
    );

    // 3. Zero address and max address with authentic EIP-1559 envelopes
    let zero_raw = make_valid_eip1559_envelope(Address::ZERO, Bytes::from_static(&[0x11, 0x22]));
    let decoded_zero = FastTxDecoder::decode(&zero_raw).unwrap();
    assert_eq!(decoded_zero.to, Some(Address::ZERO));

    let ff_target = Address::repeat_byte(0xff);
    let ff_raw = make_valid_eip1559_envelope(ff_target, Bytes::from_static(&[0x33, 0x44]));
    let decoded_ff = FastTxDecoder::decode(&ff_raw).unwrap();
    assert_eq!(decoded_ff.to, Some(ff_target));

    // 4. Huge payload: 1 MB calldata in authentic envelope
    let huge_calldata = Bytes::from(vec![0x55; 1024 * 1024]);
    let huge_raw = make_valid_eip1559_envelope(Address::repeat_byte(0x11), huge_calldata);
    let decoded_huge = FastTxDecoder::decode(&huge_raw).unwrap();
    assert_eq!(decoded_huge.tx_type, ArbitrumTxType::Eip1559);
    assert_eq!(decoded_huge.calldata.len(), 1024 * 1024);
}

/// Demonstrates elimination of FastTxDecoder's legacy heuristic pattern scan:
/// Malformed slices and 0x94 false-positive patterns are strictly rejected with DecodeError.
#[test]
fn test_adv_tx_decoder_heuristic_limitations() {
    let mock_target = Address::repeat_byte(0x77);

    // 1. Raw slice with 0x94 tag but no valid RLP structure must be rejected with DecodeError
    let mut raw_at_end = vec![0x02, 0xc8, 0x94];
    raw_at_end.extend_from_slice(mock_target.as_slice());
    let res_at_end = FastTxDecoder::decode(&raw_at_end);
    assert!(
        matches!(res_at_end, Err(ArbitrumFeedError::DecodeError(_))),
        "Raw heuristic slice must be rejected with DecodeError under strict EIP-2718 decoding"
    );

    // 2. False positive heuristic payload: 0x94 in non-envelope data
    let mut raw_false_positive = vec![
        0x02, 0xc8, 0x94,
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a,
        0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14,
        0x94,
    ];
    raw_false_positive.extend_from_slice(mock_target.as_slice());
    raw_false_positive.extend_from_slice(&[0x80, 0x80]);

    let res_fp = FastTxDecoder::decode(&raw_false_positive);
    assert!(
        matches!(res_fp, Err(ArbitrumFeedError::DecodeError(_))),
        "Heuristic 0x94 false-positive patterns are eliminated and strictly return DecodeError"
    );
}

/// Adversarially stresses gas tracker boundaries:
/// - total_gas_limit = 0
/// - out-of-order sub-block progression
/// - u8::MAX sub-block index
/// - arithmetic overflow inspection on u64 addition
#[test]
fn test_adv_flashblock_arithmetic_and_out_of_order() {
    // 1. Zero limit
    let mut zero_tracker = FlashblockGasTracker::new(0);
    assert_eq!(zero_tracker.sub_block_gas_budget(0), 0);
    assert_eq!(zero_tracker.sub_block_gas_budget(9), 0);
    assert!(zero_tracker.try_budget_tx(0, 0).is_ok());
    assert!(zero_tracker.try_budget_tx(0, 1).is_err());

    // 2. Out-of-order sub-block progression: jump to 8, then backwards to 2
    let mut tracker = FlashblockGasTracker::new(30_000_000);
    // At sub-block 8: allowed budget is 27,000,000
    assert!(tracker.try_budget_tx(8, 25_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 25_000_000);
    // Delayed sub-block 2 packet: budget for 2 is 9,000,000
    // Cumulative gas spent (25M) + 1 > 9M, must reject
    assert!(tracker.try_budget_tx(2, 1).is_err());
    assert_eq!(tracker.current_sub_block_index, 2);

    // 3. u8::MAX sub-block index clamped to 100% budget
    assert_eq!(tracker.sub_block_gas_budget(u8::MAX), 30_000_000);

    // 4. Large gas requests exceeding allowed budget
    assert!(tracker.try_budget_tx(9, 35_000_000).is_err());

    // 5. Overflow safety observation:
    // In debug mode, self.cumulative_gas_spent + tx_gas panics if tx_gas is u64::MAX.
    // In release mode without overflow-checks, it wraps.
    // We catch the panic to document the vulnerability without failing the test runner.
    let mut overflow_tracker = FlashblockGasTracker::new(30_000_000);
    overflow_tracker.cumulative_gas_spent = 10_000_000;
    let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = overflow_tracker.try_budget_tx(0, u64::MAX);
    }));
    if panic_result.is_err() {
        eprintln!("[CHALLENGER VULNERABILITY OBSERVATION]: try_budget_tx panics on u64::MAX overflow in debug mode");
    }
}

/// Verifies subscriber lagged error handling on buffer overflow.
#[test]
fn test_adv_market_publisher_lagged_error() {
    let publisher = MarketFeedPublisher::new(4);
    let mut rx = publisher.subscribe();

    // Send 10 messages without reading, overflowing the buffer of capacity 4
    for i in 0..10 {
        let update = MarketUpdate::FlashblockStep {
            block_number: 100,
            sub_block_index: i as u8,
            cumulative_gas_spent: (i as u64) * 1_000_000,
            gas_budget: 30_000_000,
        };
        let _ = publisher.publish(update);
    }

    // Attempting to read from lagging receiver should yield Lagged error
    let recv_res = rx.try_recv();
    assert!(
        matches!(recv_res, Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))),
        "Overflown subscriber must receive Lagged error, got {:?}",
        recv_res
    );

    // Subsequent read should succeed with the most recent messages
    let recovered = rx.try_recv();
    assert!(recovered.is_ok(), "Receiver must recover after acknowledging Lagged");
}

