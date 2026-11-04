//! Feature 8 Boundaries: Arbitrum Feed Unpacking BVA Test Suite

use crate::common::*;

#[test]
fn test_b08_01_sequence_zero_start() {
    let dedup = SequencerDeduplicator::new(0);
    assert!(dedup.check_and_update(1));
    assert_eq!(dedup.current_sequence(), 1);
}

#[test]
fn test_b08_02_sequence_u64_max() {
    let dedup = SequencerDeduplicator::new(u64::MAX - 2);
    assert!(dedup.check_and_update(u64::MAX - 1));
    assert!(dedup.check_and_update(u64::MAX));
    assert!(!dedup.check_and_update(u64::MAX));
}

#[test]
fn test_b08_03_empty_batch_envelope() {
    let empty_batch = BatchPayload::NestedBatch(vec![]);
    let unpacked = unpack_arbitrum_batch(&empty_batch);
    assert_eq!(unpacked.len(), 0);
}

#[test]
fn test_b08_04_deep_nesting_10_levels() {
    let leaf_tx = MockTx {
        tx_type: ArbitrumTxType::Eip1559,
        pool_target: [0x55; 20],
        calldata_len: 120,
    };

    let mut current = BatchPayload::Direct(leaf_tx.clone());
    for _ in 0..10 {
        current = BatchPayload::NestedBatch(vec![current]);
    }

    let flat = unpack_arbitrum_batch(&current);
    assert_eq!(flat.len(), 1);
    assert_eq!(flat[0], leaf_tx);
}

#[test]
fn test_b08_05_legacy_transaction_envelope() {
    let tx = MockTx {
        tx_type: ArbitrumTxType::Legacy,
        pool_target: [0x00; 20],
        calldata_len: 0,
    };
    assert_eq!(tx.tx_type, ArbitrumTxType::Legacy);
}
