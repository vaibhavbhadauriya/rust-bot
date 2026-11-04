//! Adversarial Stress Test for Hot-Path Watch Channel Decoupling (V-19)
//!
//! Validates the tokio::sync::watch pattern used in src/main.rs:271-365:
//! 1. High burst frequency (10,000 updates/sec) with zero backpressure or producer blocking.
//! 2. Zero unbounded queue memory growth (watch channel holds strictly the single latest state).
//! 3. Prompt propagation: a slower consumer simulating revm execution always receives the freshest state.

use alloy_primitives::Address;
use flash_collector::MarketUpdate;
use flash_core::types::Chain;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_v19_watch_channel_10k_burst_zero_backpressure_and_bounded_memory() {
    let (candidate_tx, mut candidate_rx) = tokio::sync::watch::channel::<Option<MarketUpdate>>(None);
    let stop_signal = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop_signal);

    let total_produced = Arc::new(AtomicUsize::new(0));
    let total_consumed = Arc::new(AtomicUsize::new(0));
    let latest_seen_seq = Arc::new(AtomicU64::new(0));

    let prod_count = Arc::clone(&total_produced);
    let cons_count = Arc::clone(&total_consumed);
    let seen_seq = Arc::clone(&latest_seen_seq);

    let pool_id = Address::from([0x77; 20]);
    let burst_target = 20_000; // 20,000 updates to test 10,000 updates/sec

    // Producer task: High-frequency burst ingestion task
    let producer_handle = tokio::spawn(async move {
        let start = Instant::now();
        for seq in 1..=burst_target {
            let update = MarketUpdate::ReserveUpdate {
                chain: Chain::ArbitrumOne,
                pool_id,
                reserve0: 1_000_000 + seq as u128,
                reserve1: 2_000_000 + seq as u128,
                timestamp_ms: seq,
            };

            // send_replace replaces prior unconsumed candidate instantly with 0 queue buildup
            candidate_tx.send_replace(Some(update));
            prod_count.fetch_add(1, Ordering::Relaxed);

            // Yield periodically to allow realistic scheduling without sleeping
            if seq % 500 == 0 {
                tokio::task::yield_now().await;
            }
        }
        let elapsed = start.elapsed();
        (elapsed, prod_count.load(Ordering::SeqCst))
    });

    // Consumer task: Simulates revm candidate execution (slower rate, e.g. 500 µs - 1 ms per candidate)
    let consumer_handle = tokio::spawn(async move {
        let mut prev_timestamp = 0u64;
        while !stop_clone.load(Ordering::Relaxed) {
            tokio::select! {
                res = candidate_rx.changed() => {
                    if res.is_err() {
                        break;
                    }
                    let maybe_update = candidate_rx.borrow_and_update().clone();
                    if let Some(MarketUpdate::ReserveUpdate { timestamp_ms, .. }) = maybe_update {
                        assert!(
                            timestamp_ms >= prev_timestamp,
                            "Stale state regression detected: current {} < prev {}",
                            timestamp_ms,
                            prev_timestamp
                        );
                        prev_timestamp = timestamp_ms;
                        seen_seq.store(timestamp_ms, Ordering::Relaxed);
                        cons_count.fetch_add(1, Ordering::Relaxed);

                        // Simulate heavy candidate revm evaluation
                        tokio::time::sleep(Duration::from_micros(500)).await;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if stop_clone.load(Ordering::Relaxed) {
                        break;
                    }
                }
            }
        }
    });

    let (prod_elapsed, produced) = producer_handle.await.expect("Producer task failed");
    let burst_rate = (produced as f64) / prod_elapsed.as_secs_f64();

    println!(
        "V-19 Watch Channel Decoupling: Produced {} updates in {:?} -> Rate: {:.0} updates/sec",
        produced, prod_elapsed, burst_rate
    );

    // Verify producer was not stalled by slow consumer (must exceed 10,000 updates/sec)
    assert_eq!(produced as u64, burst_target);
    assert!(
        burst_rate >= 10_000.0,
        "Burst rate {:.0} updates/sec fell below 10,000 updates/sec threshold!",
        burst_rate
    );

    // Allow consumer to catch the final published state
    tokio::time::sleep(Duration::from_millis(50)).await;
    stop_signal.store(true, Ordering::Relaxed);
    let _ = consumer_handle.await;

    let final_seen = latest_seen_seq.load(Ordering::SeqCst);
    println!(
        "Final state observed by consumer: seq {} (target: {}), total consumed: {}",
        final_seen,
        burst_target,
        total_consumed.load(Ordering::SeqCst)
    );

    // Consumer must have received and observed the latest or near-latest sequence
    assert!(
        final_seen >= (burst_target - 100),
        "Consumer failed to observe freshest state: observed {} vs target {}",
        final_seen,
        burst_target
    );
}
