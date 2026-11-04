//! Adversarial Concurrency & Deadlock Stress Test for MarketGraph (H7)
//!
//! Stress-tests crates/scanner/src/graph.rs under high-concurrency lock contention:
//! - 16 concurrent threads (8 readers, 8 writers)
//! - Readers: continuous `get_edges` on existing and non-existing tokens
//! - Writers: continuous `add_pool` (which exercises `get_or_create_vertex` and `adj.write`)
//!   and `update_reserves` (in-place `Arc::make_mut` under `adj.write`)
//! - Strict 15-second watchdog timer to prove zero deadlocks under heavy lock contention.

use alloy_primitives::Address;
use flash_core::types::{Chain, FeePips, PoolState, Venue};
use flash_scanner::{DiscoveredPool, MarketGraph};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn test_h7_concurrency_deadlock_stress_16_threads() {
    let graph = Arc::new(MarketGraph::new());
    let token_base = Address::from([0x01; 20]);
    let token_quote = Address::from([0x02; 20]);

    // Pre-populate initial pools
    for i in 0..10u8 {
        let pool = DiscoveredPool::new(
            Address::from([0x10 + i; 20]),
            Venue::UniswapV2,
            Chain::ArbitrumOne,
            token_base,
            Address::from([0x20 + i; 20]),
            FeePips::from_bps(30),
            PoolState::ConstantProduct {
                reserve0: 1_000_000,
                reserve1: 2_000_000,
            },
        );
        graph.add_pool(&pool);
    }

    let num_readers = 8;
    let num_writers = 8;
    let total_threads = num_readers + num_writers; // 16 threads
    let barrier = Arc::new(Barrier::new(total_threads));
    let stop_signal = Arc::new(AtomicBool::new(false));
    let reads_completed = Arc::new(AtomicUsize::new(0));
    let writes_completed = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(total_threads);

    // 8 Reader threads
    for r_idx in 0..num_readers {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let stop = Arc::clone(&stop_signal);
        let reads = Arc::clone(&reads_completed);

        handles.push(thread::spawn(move || {
            b.wait();
            let mut iters = 0;
            while !stop.load(Ordering::Relaxed) && iters < 25_000 {
                // Read existing tokens and dynamic tokens
                let tok = if iters % 2 == 0 {
                    token_base
                } else {
                    Address::from([0x20 + ((r_idx + iters) % 20) as u8; 20])
                };
                let edges = g.get_edges(tok);
                let _ = edges.len();
                reads.fetch_add(1, Ordering::Relaxed);
                iters += 1;
            }
        }));
    }

    // 8 Writer threads
    for w_idx in 0..num_writers {
        let g = Arc::clone(&graph);
        let b = Arc::clone(&barrier);
        let stop = Arc::clone(&stop_signal);
        let writes = Arc::clone(&writes_completed);

        handles.push(thread::spawn(move || {
            b.wait();
            let mut iters = 0;
            while !stop.load(Ordering::Relaxed) && iters < 2_500 {
                if iters % 2 == 0 {
                    // add_pool with new and overlapping tokens
                    let p_id = Address::from([
                        0xA0 + (w_idx as u8),
                        (iters & 0xFF) as u8,
                        ((iters >> 8) & 0xFF) as u8,
                        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
                        0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11,
                    ]);
                    let t0 = if w_idx % 2 == 0 { token_base } else { token_quote };
                    let t1 = Address::from([0x30 + ((w_idx + iters) % 30) as u8; 20]);
                    let pool = DiscoveredPool::new(
                        p_id,
                        Venue::UniswapV3,
                        Chain::ArbitrumOne,
                        t0,
                        t1,
                        FeePips::from_bps(5),
                        PoolState::ConstantProduct {
                            reserve0: 500_000 + (iters as u128),
                            reserve1: 1_500_000,
                        },
                    );
                    g.add_pool(&pool);
                } else {
                    // update_reserves on pre-existing pool
                    let p_id = Address::from([0x10 + ((iters + w_idx) % 10) as u8; 20]);
                    g.update_reserves(p_id, 1_000_000 + (iters as u128), 2_000_000 + (iters as u128));
                }
                writes.fetch_add(1, Ordering::Relaxed);
                iters += 1;
            }
        }));
    }

    // Monitor with strict 15s watchdog timer to empirically detect deadlock
    let start_time = Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for h in handles {
            let _ = h.join();
        }
        let _ = tx.send(());
    });

    let completed_cleanly = rx.recv_timeout(Duration::from_secs(15)).is_ok();
    stop_signal.store(true, Ordering::Relaxed);
    let duration = start_time.elapsed();

    let total_reads = reads_completed.load(Ordering::SeqCst);
    let total_writes = writes_completed.load(Ordering::SeqCst);

    assert!(
        completed_cleanly,
        "DEADLOCK DETECTED: 16 concurrent reader/writer threads failed to complete within 15s watchdog! Reads completed: {}, Writes completed: {}",
        total_reads, total_writes
    );

    println!(
        "H7 Concurrency Stress Passed: 16 threads (8 readers, 8 writers) completed {} reads and {} writes in {:?} without deadlock.",
        total_reads, total_writes, duration
    );

    // Verify graph invariants hold post-stress
    assert!(graph.pool_count() >= 10);
    assert!(graph.vertex_count() >= 12);
    let base_edges = graph.get_edges(token_base);
    assert!(!base_edges.is_empty(), "Base token must retain valid outgoing edges");
}
