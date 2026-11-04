//! Empirical Adversarial Concurrency & Stress Tests for Milestone M_R1: M9
//!
//! Objectives:
//! 1. Stress-test HoneypotFilter under intense multi-threaded concurrency (50 parallel threads).
//! 2. Concurrently query `check_token_safety`, insert malicious tokens, insert fee-on-transfer tokens,
//!    insert rebasing tokens, mark tokens verified safe, and verify safe tokens.
//! 3. Test concurrent `check_pair_safety` and `check_pool` under simultaneous mutation.
//! 4. Test rapid state toggling on identical token keys across competing threads.
//! 5. Ensure zero panics, zero deadlocks, and strict thread safety via `DashSet`.

use alloy_primitives::Address;
use flash_core::constants;
use flash_core::types::{Chain, FeePips, Pool, PoolState, Venue};
use flash_scanner::{HoneypotFilter, HoneypotViolation};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

// =========================================================================
// 1. 50-Thread Concurrent Read/Write Matrix Stress Test
// =========================================================================

#[test]
fn test_challenger_m9_50_threads_concurrency_stress() {
    let filter = Arc::new(HoneypotFilter::new());
    let num_threads = 50;
    let ops_per_thread = 1_000;
    let barrier = Arc::new(Barrier::new(num_threads));
    let total_ops = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::with_capacity(num_threads);

    for thread_idx in 0..num_threads {
        let f = Arc::clone(&filter);
        let b = Arc::clone(&barrier);
        let counter = Arc::clone(&total_ops);

        handles.push(thread::spawn(move || {
            // Synchronize start so all 50 threads hit the DashSet simultaneously
            b.wait();

            let role = thread_idx % 5;
            for i in 0..ops_per_thread {
                let token_unique = Address::from([
                    (thread_idx as u8),
                    ((i >> 8) as u8),
                    (i as u8),
                    0xaa, 0xbb, 0xcc, 0xdd, 0xee,
                    0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0x00, 0x12, 0x34,
                ]);

                match role {
                    0 => {
                        // Role 0: Check canonical safe tokens - must ALWAYS be Ok
                        let safe_tokens = [
                            constants::arbitrum::WETH,
                            constants::arbitrum::USDC_NATIVE,
                            constants::arbitrum::USDC_BRIDGED,
                            constants::arbitrum::USDT,
                            constants::arbitrum::WBTC,
                            constants::arbitrum::ARB,
                            constants::base::WETH,
                            constants::base::USDC_NATIVE,
                            constants::base::USDBC_BRIDGED,
                            constants::base::CBBTC,
                            constants::base::AERO,
                        ];
                        let target = safe_tokens[(i as usize) % safe_tokens.len()];
                        assert_eq!(
                            f.check_token_safety(target),
                            Ok(()),
                            "Canonical token {:?} must be safe",
                            target
                        );
                    }
                    1 => {
                        // Role 1: Insert malicious tokens, then immediately verify
                        f.add_malicious_token(token_unique);
                        assert_eq!(
                            f.check_token_safety(token_unique),
                            Err(HoneypotViolation::KnownMalicious(token_unique))
                        );
                    }
                    2 => {
                        // Role 2: Insert fee-on-transfer tokens, then immediately verify
                        f.add_fee_on_transfer_token(token_unique);
                        assert_eq!(
                            f.check_token_safety(token_unique),
                            Err(HoneypotViolation::FeeOnTransfer { fee_percent_bps: 1000 })
                        );
                    }
                    3 => {
                        // Role 3: Insert rebasing tokens, then immediately verify
                        f.add_rebasing_token(token_unique);
                        assert_eq!(
                            f.check_token_safety(token_unique),
                            Err(HoneypotViolation::RebasingToken(token_unique))
                        );
                    }
                    4 => {
                        // Role 4: Blacklist, verify, then mark verified safe and verify removal from blacklist
                        f.add_malicious_token(token_unique);
                        assert_eq!(
                            f.check_token_safety(token_unique),
                            Err(HoneypotViolation::KnownMalicious(token_unique))
                        );
                        f.mark_verified_safe(token_unique);
                        assert_eq!(
                            f.check_token_safety(token_unique),
                            Ok(()),
                            "Token should be safe after mark_verified_safe"
                        );
                    }
                    _ => unreachable!(),
                }

                counter.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    let start = Instant::now();
    for h in handles {
        h.join().expect("Thread must not panic or deadlock");
    }
    let elapsed = start.elapsed();

    let completed = total_ops.load(Ordering::SeqCst);
    assert_eq!(completed, (num_threads * ops_per_thread) as u64);

    println!(
        "50-thread HoneypotFilter concurrency stress passed: {} operations in {:?} ({:.0} ops/sec)",
        completed,
        elapsed,
        (completed as f64) / elapsed.as_secs_f64()
    );
}

// =========================================================================
// 2. High-Contention Key Toggling Across 50 Threads (Zero Deadlock Guard)
// =========================================================================

#[test]
fn test_challenger_m9_shared_key_high_contention_toggling() {
    let filter = Arc::new(HoneypotFilter::new());
    let num_threads = 50;
    let shared_token = Address::from([0x77; 20]);
    let stop_flag = Arc::new(AtomicBool::new(false));
    let total_toggles = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(Barrier::new(num_threads));

    let mut handles = Vec::with_capacity(num_threads);

    for thread_idx in 0..num_threads {
        let f = Arc::clone(&filter);
        let s = Arc::clone(&stop_flag);
        let count = Arc::clone(&total_toggles);
        let b = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            b.wait();
            let mut iter = 0u64;
            while !s.load(Ordering::Relaxed) && iter < 1_500 {
                if (thread_idx + iter as usize) % 2 == 0 {
                    f.add_malicious_token(shared_token);
                } else {
                    f.mark_verified_safe(shared_token);
                }

                // Querying the token safety concurrently must never panic or deadlock
                let res = f.check_token_safety(shared_token);
                assert!(
                    res == Ok(()) || res == Err(HoneypotViolation::KnownMalicious(shared_token)),
                    "Result must either be Ok or KnownMalicious, got {:?}",
                    res
                );

                count.fetch_add(1, Ordering::Relaxed);
                iter += 1;
            }
        }));
    }

    for h in handles {
        h.join().expect("High contention toggle must not panic or deadlock");
    }

    let final_toggles = total_toggles.load(Ordering::SeqCst);
    assert!(final_toggles >= (num_threads as u64) * 1_000);
}

// =========================================================================
// 3. Concurrent Pair Safety and Pool Pre-Filtering Under Mutation
// =========================================================================

#[test]
fn test_challenger_m9_concurrent_pair_and_pool_validation() {
    let filter = Arc::new(HoneypotFilter::new());
    let num_threads = 50;
    let iterations = 500;
    let barrier = Arc::new(Barrier::new(num_threads));

    let mut handles = Vec::with_capacity(num_threads);

    for thread_idx in 0..num_threads {
        let f = Arc::clone(&filter);
        let b = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            b.wait();

            for i in 0..iterations {
                let token_a = Address::from([0x10u8.wrapping_add(thread_idx as u8); 20]);
                let token_b = Address::from([0x20u8.wrapping_add((thread_idx + i) as u8); 20]);

                if thread_idx % 3 == 0 {
                    // Mutate
                    if i % 2 == 0 {
                        f.add_malicious_token(token_a);
                    } else {
                        f.mark_verified_safe(token_a);
                    }
                } else if thread_idx % 3 == 1 {
                    // Pair safety check
                    let _ = f.check_pair_safety(token_a, token_b);
                    // Check self-loop rejection
                    assert_eq!(
                        f.check_pair_safety(token_a, token_a),
                        Err(HoneypotViolation::IdenticalTokens)
                    );
                } else {
                    // Pool verification check with CPMM pool
                    let pool = Pool {
                        id: Address::from([0x99; 20]),
                        venue: Venue::UniswapV2,
                        chain_id: Chain::ArbitrumOne.id(),
                        token0: token_a,
                        token1: token_b,
                        fee: FeePips::from_bps(30),
                        state: PoolState::ConstantProduct {
                            reserve0: 10_000_000_000_000_000_000u128, // 10 units at 18 dec
                            reserve1: 25_000_000_000u128,             // 25,000 units at 6 dec
                        },
                    };
                    let _ = f.check_pool(&pool, 18, 6);
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("Concurrent pool/pair check must not panic");
    }
}
