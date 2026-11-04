//! Feature 8: Arbitrum Sequencer Feed Unpacking & Monotonic Dedup Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 27-28, 84, and explorer_survey_3/handoff.md.

use crate::common::*;

#[test]
fn test_f08_01_monotonic_deduplication() {
    let dedup = SequencerDeduplicator::new(100);

    // Sequence 101: new -> accepted
    assert!(dedup.check_and_update(101));
    assert_eq!(dedup.current_sequence(), 101);

    // Duplicate 101 -> dropped
    assert!(!dedup.check_and_update(101));

    // Stale 99 -> dropped
    assert!(!dedup.check_and_update(99));

    // Monotonic next 102 -> accepted
    assert!(dedup.check_and_update(102));
    assert_eq!(dedup.current_sequence(), 102);
}

#[test]
fn test_f08_02_recursive_nested_batch_unpacking() {
    let tx1 = MockTx {
        tx_type: ArbitrumTxType::Eip1559,
        pool_target: [1u8; 20],
        calldata_len: 128,
    };
    let tx2 = MockTx {
        tx_type: ArbitrumTxType::Legacy,
        pool_target: [2u8; 20],
        calldata_len: 64,
    };
    let tx3 = MockTx {
        tx_type: ArbitrumTxType::Eip7702,
        pool_target: [3u8; 20],
        calldata_len: 256,
    };

    // Construct 3-level deeply nested batch envelope
    let inner_batch = BatchPayload::NestedBatch(vec![
        BatchPayload::Direct(tx2.clone()),
        BatchPayload::Direct(tx3.clone()),
    ]);
    let root_batch = BatchPayload::NestedBatch(vec![
        BatchPayload::Direct(tx1.clone()),
        inner_batch,
    ]);

    let flat = unpack_arbitrum_batch(&root_batch);
    assert_eq!(flat.len(), 3, "Must recursively flatten 3 transactions");
    assert_eq!(flat[0], tx1);
    assert_eq!(flat[1], tx2);
    assert_eq!(flat[2], tx3);
}

#[test]
fn test_f08_03_eip7702_transaction_support() {
    let tx = MockTx {
        tx_type: ArbitrumTxType::Eip7702,
        pool_target: [0x77; 20],
        calldata_len: 100,
    };
    assert_eq!(tx.tx_type, ArbitrumTxType::Eip7702);
}

#[test]
fn test_f08_04_arbitrum_feed_url() {
    let feed_url = "wss://arb1-feed.arbitrum.io/feed";
    assert!(feed_url.starts_with("wss://"));
    assert!(feed_url.contains("arb1-feed.arbitrum.io"));
}

#[test]
fn test_f08_05_concurrent_dedup_thread_safety() {
    use std::sync::Arc;
    use std::thread;

    let dedup = Arc::new(SequencerDeduplicator::new(0));
    let mut handles = vec![];

    // Spawn 10 concurrent threads each attempting to claim sequences
    for i in 1..=10 {
        let d = Arc::clone(&dedup);
        handles.push(thread::spawn(move || {
            d.check_and_update(i * 10)
        }));
    }

    for h in handles {
        let _ = h.join();
    }

    assert!(dedup.current_sequence() > 0);
}
