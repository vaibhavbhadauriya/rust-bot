//! Tier 5 Adversarial Coverage Hardening: crates/collector
//!
//! White-box stress testing of:
//! 1. SequencerDeduplicator atomic CAS monotonicity under heavy multi-threaded contention
//! 2. Arbitrum batch unpacking recursion depth limits (>16 prevents stack overflow)
//! 3. Arbitrum transaction type decoding across Legacy, Eip2930, Eip1559, and Eip7702
//! 4. Base Flashblock cumulative gas budget calculations, clamping, and overflow protection

use flash_collector::{
    unpack_arbitrum_batch, ArbitrumTxType, BatchPayload, FlashblockGasTracker, MockTx,
    SequencerDeduplicator,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

#[test]
fn test_adv_collector_dedup_multithreaded_contention() {
    let dedup = Arc::new(SequencerDeduplicator::new(0));
    let accepted_count = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();

    // 16 threads attempting to insert identical and overlapping sequence numbers
    for thread_id in 0..16 {
        let d = Arc::clone(&dedup);
        let acc = Arc::clone(&accepted_count);
        handles.push(thread::spawn(move || {
            for seq in 1..=500 {
                // Thread introduces scrambled sequence patterns
                let s = if thread_id % 2 == 0 {
                    seq
                } else {
                    501 - seq
                };
                if d.check_and_update(s) {
                    acc.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Since sequences were up to 500, the maximum sequence must be 500
    assert_eq!(dedup.current_sequence(), 500);
    // Any sequence <= 500 must now be rejected
    assert!(!dedup.check_and_update(500));
    assert!(!dedup.check_and_update(499));
    assert!(!dedup.check_and_update(0));
    // Sequence 501 must be accepted
    assert!(dedup.check_and_update(501));
    assert_eq!(dedup.current_sequence(), 501);

    // u64::MAX boundary
    assert!(dedup.check_and_update(u64::MAX));
    assert_eq!(dedup.current_sequence(), u64::MAX);
    // Nothing can be greater than u64::MAX
    assert!(!dedup.check_and_update(u64::MAX));
    assert!(!dedup.check_and_update(u64::MAX - 1));
}

#[test]
fn test_adv_collector_arbitrum_batch_recursion_depth_limit() {
    let dummy_tx = MockTx {
        tx_type: ArbitrumTxType::Eip1559,
        pool_target: [0x11; 20],
        calldata_len: 100,
    };

    // 1. Shallow batch (depth 3)
    let shallow = BatchPayload::NestedBatch(vec![
        BatchPayload::Direct(dummy_tx.clone()),
        BatchPayload::NestedBatch(vec![
            BatchPayload::Direct(dummy_tx.clone()),
            BatchPayload::NestedBatch(vec![BatchPayload::Direct(dummy_tx.clone())]),
        ]),
    ]);
    let unpacked_shallow = unpack_arbitrum_batch(&shallow);
    assert_eq!(unpacked_shallow.len(), 3);

    // 2. Extremely deep batch: 25 levels deep (exceeds MAX_BATCH_DEPTH = 16)
    // Must prune beyond depth 16 to prevent stack overflow attack
    let mut deep = BatchPayload::Direct(dummy_tx.clone());
    for _ in 0..25 {
        deep = BatchPayload::NestedBatch(vec![deep]);
    }
    let unpacked_deep = unpack_arbitrum_batch(&deep);
    // Depth is 25 > 16, so the deepest tx must be pruned (out is empty or truncated)
    assert!(
        unpacked_deep.is_empty(),
        "Deep batch exceeding depth 16 must be pruned"
    );

    // 3. Exactly at limit: 16 levels deep
    let mut exact_16 = BatchPayload::Direct(dummy_tx.clone());
    for _ in 0..16 {
        exact_16 = BatchPayload::NestedBatch(vec![exact_16]);
    }
    let unpacked_16 = unpack_arbitrum_batch(&exact_16);
    assert_eq!(unpacked_16.len(), 1, "Batch at depth 16 must be unpacked");
}

#[test]
fn test_adv_collector_arbitrum_tx_type_decoding() {
    assert_eq!(ArbitrumTxType::from_byte(0), ArbitrumTxType::Legacy);
    assert_eq!(ArbitrumTxType::from_byte(1), ArbitrumTxType::Eip2930);
    assert_eq!(ArbitrumTxType::from_byte(2), ArbitrumTxType::Eip1559);
    assert_eq!(ArbitrumTxType::from_byte(4), ArbitrumTxType::Eip7702);
    // Fallback for unknown type bytes
    assert_eq!(ArbitrumTxType::from_byte(3), ArbitrumTxType::Legacy);
    assert_eq!(ArbitrumTxType::from_byte(255), ArbitrumTxType::Legacy);
}

#[test]
fn test_adv_collector_flashblock_gas_budgeting_and_overflow() {
    let limit = 30_000_000u64;
    let mut tracker = FlashblockGasTracker::new(limit);

    // 1. Sub-blocks 0 through 9: budget scales linearly from 3M to 30M
    for j in 0..10 {
        let expected = ((j as u64 + 1) * limit) / 10;
        assert_eq!(tracker.sub_block_gas_budget(j), expected);
    }

    // 2. Clamping for sub-block index >= 10
    assert_eq!(tracker.sub_block_gas_budget(10), limit);
    assert_eq!(tracker.sub_block_gas_budget(50), limit);
    assert_eq!(tracker.sub_block_gas_budget(255), limit);

    // 3. Incremental budgeting
    // Sub-block 0 has 3M limit.
    assert!(tracker.try_budget_tx(0, 1_500_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 1_500_000);

    assert!(tracker.try_budget_tx(0, 1_500_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 3_000_000);

    // Next tx exceeds sub-block 0 budget (3M)
    assert!(tracker.try_budget_tx(0, 1).is_err());
    assert_eq!(tracker.cumulative_gas_spent, 3_000_000);

    // Advancing to sub-block 1 allows up to 6M total
    assert!(tracker.try_budget_tx(1, 2_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 5_000_000);

    // 4. Arithmetic overflow with u64::MAX
    assert!(tracker.try_budget_tx(9, u64::MAX).is_err());
    assert_eq!(tracker.cumulative_gas_spent, 5_000_000, "Overflow must not modify gas spent");

    // 5. Block reset
    tracker.reset_block();
    assert_eq!(tracker.cumulative_gas_spent, 0);
    assert_eq!(tracker.current_sub_block_index, 0);
}
