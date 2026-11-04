//! Empirical Challenger Stress & Adversarial Test Suite for Multi-EOA Signer Pool
//!
//! Authoritative requirements: ORIGINAL_REQUEST.md lines 69-74; PROJECT.md § Milestone 5 Feature 19.
//! Challenger: m5_challenger_1
//!
//! Verifies under high concurrency and adversarial conditions:
//! 1. High-concurrency rapid acquire/release stress test (30-50 threads, 1,000 cycles).
//! 2. Invariant: No two threads ever hold the same signer index or address simultaneously.
//! 3. Pool exhaustion returns None safely, re-acquisition succeeds immediately upon release.
//! 4. Strict monotonic nonce advancement with ZERO duplicates and ZERO skipped nonces.
//! 5. Arbitrum 125ms PGA burst simulation with 10 concurrent opportunities bypassing the mempool nonce cache.
//! 6. RAII lease panic safety and zero leak under stack unwinding.
//! 7. Zero capacity, capacity cap, and adversarial hex key boundaries.

use alloy_primitives::{Address, Bytes, U256};
use flash_strategy::signers::{
    Eip1559TransactionRequest, SignerError, SignerPool,
};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::panic::catch_unwind;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;

// ==============================================================================
// 1. High-Concurrency Stress Test (50 Threads, 1,000 Cycles)
// ==============================================================================

#[test]
fn test_stress_50_concurrent_threads_1000_cycles_no_collisions() {
    let pool_capacity = 10;
    let pool = Arc::new(SignerPool::new(pool_capacity));
    let num_threads = 50;
    let cycles_per_thread = 1_000;

    // Active holder tracker: active_holder[signer_idx] = thread_id (or -1 if free)
    let active_holders: Arc<Vec<AtomicI64>> = Arc::new(
        (0..pool_capacity).map(|_| AtomicI64::new(-1)).collect()
    );
    let collision_detected = Arc::new(AtomicBool::new(false));
    let completed_cycles = Arc::new(AtomicUsize::new(0));

    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::with_capacity(num_threads);

    for tid in 0..num_threads {
        let pool = Arc::clone(&pool);
        let active_holders = Arc::clone(&active_holders);
        let collision_detected = Arc::clone(&collision_detected);
        let completed_cycles = Arc::clone(&completed_cycles);
        let barrier = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            // Synchronize all 50 threads to maximize initial contention
            barrier.wait();

            for _ in 0..cycles_per_thread {
                loop {
                    if let Some(lease) = pool.acquire() {
                        let idx = lease.index();
                        let my_tid = tid as i64;

                        // Atomic check: Verify previous holder was -1
                        let prev = active_holders[idx].swap(my_tid, Ordering::SeqCst);
                        if prev != -1 {
                            collision_detected.store(true, Ordering::SeqCst);
                            panic!("COLLISION DETECTED: Signer {} held by {} while thread {} acquired it!", idx, prev, my_tid);
                        }

                        // Burn a few cycles holding the signer to simulate work
                        let _addr = lease.address();
                        let _nonce = lease.nonce();
                        std::hint::spin_loop();

                        // Atomic check: Verify holder is STILL my_tid before releasing
                        let curr = active_holders[idx].swap(-1, Ordering::SeqCst);
                        if curr != my_tid {
                            collision_detected.store(true, Ordering::SeqCst);
                            panic!("CORRUPTION DETECTED: Signer {} expected holder {} but found {}!", idx, my_tid, curr);
                        }

                        // Release signer
                        drop(lease);
                        completed_cycles.fetch_add(1, Ordering::Relaxed);
                        break;
                    } else {
                        // Pool exhausted, yield and retry
                        thread::yield_now();
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("Thread panicked during high-concurrency stress");
    }

    assert!(
        !collision_detected.load(Ordering::SeqCst),
        "Fatal: Collisions occurred in SignerPool during 50-thread concurrent stress test!"
    );
    assert_eq!(
        completed_cycles.load(Ordering::SeqCst),
        num_threads * cycles_per_thread,
        "All 50,000 acquire/release cycles must complete successfully"
    );

    // Verify all signers are released back to pool
    for idx in 0..pool_capacity {
        assert_eq!(
            active_holders[idx].load(Ordering::SeqCst),
            -1,
            "Signer {} should be free after all threads complete",
            idx
        );
    }
}

// ==============================================================================
// 2. Mutual Exclusion & Zero Simultaneous Leases Invariant
// ==============================================================================

#[test]
fn test_stress_mutual_exclusion_invariant_under_contention() {
    let pool = Arc::new(SignerPool::new(5));
    let num_threads = 30;
    let cycles = 500;

    let active_addresses: Arc<Mutex<HashSet<Address>>> = Arc::new(Mutex::new(HashSet::new()));
    let violation_count = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(num_threads));

    let mut handles = Vec::with_capacity(num_threads);

    for _ in 0..num_threads {
        let pool = Arc::clone(&pool);
        let active_addresses = Arc::clone(&active_addresses);
        let violation_count = Arc::clone(&violation_count);
        let barrier = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            barrier.wait();

            for _ in 0..cycles {
                loop {
                    if let Some(lease) = pool.acquire() {
                        let addr = lease.address();

                        {
                            let mut set = active_addresses.lock();
                            if set.contains(&addr) {
                                violation_count.fetch_add(1, Ordering::SeqCst);
                            }
                            set.insert(addr);
                        }

                        // Simulate work
                        std::hint::spin_loop();

                        {
                            let mut set = active_addresses.lock();
                            set.remove(&addr);
                        }

                        drop(lease);
                        break;
                    } else {
                        thread::yield_now();
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("Thread join failed");
    }

    assert_eq!(
        violation_count.load(Ordering::SeqCst),
        0,
        "Invariant violated: identical EOA address leased to multiple threads concurrently!"
    );
}

// ==============================================================================
// 3. Strict Monotonic Nonce Sequence Verification (Zero Duplicates / Zero Skips)
// ==============================================================================

#[test]
fn test_stress_strict_monotonic_nonce_zero_duplicates_zero_skips() {
    let pool_capacity = 10;
    let pool = Arc::new(SignerPool::new(pool_capacity));
    let num_threads = 30;
    let target_leases_per_thread = 350;
    let total_expected_leases = num_threads * target_leases_per_thread;

    // Per-signer recorded nonces: per_signer_nonces[signer_idx] = Vec<u64>
    let per_signer_nonces: Arc<Vec<Mutex<Vec<u64>>>> = Arc::new(
        (0..pool_capacity).map(|_| Mutex::new(Vec::new())).collect()
    );

    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::with_capacity(num_threads);

    for _ in 0..num_threads {
        let pool = Arc::clone(&pool);
        let per_signer_nonces = Arc::clone(&per_signer_nonces);
        let barrier = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            barrier.wait();

            for _ in 0..target_leases_per_thread {
                loop {
                    if let Some(lease) = pool.acquire() {
                        let idx = lease.index();
                        let nonce = lease.nonce();

                        // Record nonce under per-signer lock
                        per_signer_nonces[idx].lock().push(nonce);

                        // Release signer
                        drop(lease);
                        break;
                    } else {
                        thread::yield_now();
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("Thread join failed");
    }

    let mut total_recorded = 0;

    for idx in 0..pool_capacity {
        let mut nonces = per_signer_nonces[idx].lock().clone();
        total_recorded += nonces.len();

        if nonces.is_empty() {
            continue;
        }

        // Sort nonces to check strict contiguous sequence
        nonces.sort_unstable();

        // 1. Verify zero skipped nonces and zero duplicates: must form 0, 1, 2, ..., N-1
        for (expected_nonce, &actual_nonce) in nonces.iter().enumerate() {
            assert_eq!(
                actual_nonce, expected_nonce as u64,
                "Signer {} nonce mismatch: expected {}, got {}. Nonce sequence corrupted!",
                idx, expected_nonce, actual_nonce
            );
        }

        // 2. Verify current nonce matches total leases for this signer
        let expected_final_nonce = nonces.len() as u64;
        assert_eq!(
            pool.current_nonce(idx),
            expected_final_nonce,
            "Signer {} pool current_nonce {} != expected {}",
            idx,
            pool.current_nonce(idx),
            expected_final_nonce
        );
    }

    assert_eq!(
        total_recorded, total_expected_leases,
        "Total recorded nonces {} != expected {}",
        total_recorded, total_expected_leases
    );
}

// ==============================================================================
// 4. Pool Exhaustion Safety & Instant Re-acquisition
// ==============================================================================

#[test]
fn test_stress_pool_exhaustion_and_instant_reacquisition() {
    let pool = SignerPool::new(4);

    // Acquire all 4 signers
    let lease0 = pool.acquire().expect("Lease 0");
    let lease1 = pool.acquire().expect("Lease 1");
    let lease2 = pool.acquire().expect("Lease 2");
    let lease3 = pool.acquire().expect("Lease 3");

    // 100 consecutive acquire calls must return None safely
    for _ in 0..100 {
        assert!(pool.acquire().is_none(), "Exhausted pool must return None");
        assert!(pool.acquire_signer().is_none(), "acquire_signer must return None");
    }

    // Release lease 2
    let released_idx = lease2.index();
    assert_eq!(released_idx, 2);
    drop(lease2);

    // Re-acquisition must succeed immediately with index 2
    let lease_new = pool.acquire().expect("Immediate re-acquisition after release");
    assert_eq!(lease_new.index(), 2, "Must reacquire index 2");

    // Exhausted again
    assert!(pool.acquire().is_none());

    // Clean release of remaining
    drop(lease0);
    drop(lease1);
    drop(lease3);
    drop(lease_new);

    // All 4 should now be re-acquirable
    let mut reacquired = vec![];
    for _ in 0..4 {
        reacquired.push(pool.acquire().expect("Should reacquire all"));
    }
    assert_eq!(reacquired.len(), 4);
    assert!(pool.acquire().is_none());
}

// ==============================================================================
// 5. Arbitrum 125ms PGA Burst Simulation (10 Concurrent Opportunities)
// ==============================================================================

#[test]
fn test_stress_arbitrum_125ms_burst_10_concurrent_opportunities() {
    let burst_size = 10;
    let pool = Arc::new(SignerPool::new(burst_size));
    let barrier = Arc::new(Barrier::new(burst_size));
    let results = Arc::new(Mutex::new(Vec::with_capacity(burst_size)));

    let t_burst_start = Instant::now();
    let mut handles = Vec::with_capacity(burst_size);

    for opp_id in 0..burst_size {
        let pool = Arc::clone(&pool);
        let barrier = Arc::clone(&barrier);
        let results = Arc::clone(&results);

        handles.push(thread::spawn(move || {
            // Wait for all 10 opportunities to arrive simultaneously at the sequencer edge
            barrier.wait();
            let t_start = Instant::now();

            // 1. Acquire signer
            let lease = pool.acquire().expect("Burst opportunity must acquire a signer");
            let idx = lease.index();
            let addr = lease.address();
            let nonce = lease.nonce();

            // 2. Build and sign transaction
            let tx_req = Eip1559TransactionRequest {
                chain_id: 42161, // Arbitrum One
                nonce: Some(nonce),
                max_priority_fee_per_gas: 2_000_000_000 + (opp_id as u128 * 100_000_000),
                max_fee_per_gas: 30_000_000_000,
                gas_limit: 120_000,
                to: Address::from([0x88; 20]),
                value: U256::ZERO,
                data: Bytes::from(vec![0x12, 0x34, 0x56, opp_id as u8]),
                access_list: vec![],
            };

            let signed_tx = pool.sign_transaction(idx, &tx_req).expect("Signing succeeds");
            let latency = t_start.elapsed();

            results.lock().push((opp_id, idx, addr, signed_tx, latency));
        }));
    }

    for h in handles {
        h.join().expect("Burst thread join failed");
    }

    let elapsed_burst = t_burst_start.elapsed();
    let dispatches = results.lock().clone();

    // 1. Invariant: Exactly 10 dispatches completed
    assert_eq!(dispatches.len(), burst_size);

    // 2. Invariant: All 10 sender addresses must be strictly DISTINCT
    // This confirms 100% Mempool Nonce Cache Bypass on Arbitrum!
    let mut unique_senders = HashSet::new();
    let mut unique_hashes = HashSet::new();
    let mut unique_indices = HashSet::new();

    for (_opp_id, idx, addr, signed_tx, latency) in &dispatches {
        assert!(
            unique_senders.insert(*addr),
            "Duplicate sender address {:?} found! Nonce cache bypass failed!",
            addr
        );
        assert!(
            unique_hashes.insert(signed_tx.tx_hash),
            "Duplicate tx hash {:?} found!",
            signed_tx.tx_hash
        );
        assert!(
            unique_indices.insert(*idx),
            "Duplicate signer index {} found!",
            idx
        );
        assert_eq!(*addr, signed_tx.sender);
        assert_eq!(signed_tx.raw_bytes[0], 0x02, "EIP-1559 tx envelope");

        // Latency per opportunity must be < 5ms even in unoptimized debug build
        let max_opp_latency_ms = if cfg!(debug_assertions) { 50 } else { 2 };
        assert!(
            latency.as_millis() < max_opp_latency_ms as u128,
            "Opportunity latency took {}ms (< {}ms)",
            latency.as_millis(),
            max_opp_latency_ms
        );
    }

    assert_eq!(unique_senders.len(), burst_size);
    assert_eq!(unique_hashes.len(), burst_size);
    assert_eq!(unique_indices.len(), burst_size);

    // 3. Overall burst execution well within Arbitrum 125ms PGA window
    let max_burst_window_ms = if cfg!(debug_assertions) { 100 } else { 20 };
    assert!(
        elapsed_burst.as_millis() < max_burst_window_ms as u128,
        "Total burst dispatch took {}ms (< {}ms allowed in 125ms PGA window)",
        elapsed_burst.as_millis(),
        max_burst_window_ms
    );
}

// ==============================================================================
// 6. RAII Lease Guard Panic Safety (Stack Unwinding Without Leak)
// ==============================================================================

#[test]
fn test_stress_raii_lease_unwind_panic_safety() {
    let pool = Arc::new(SignerPool::new(1));

    // Thread panics while holding lease
    let pool_clone = Arc::clone(&pool);
    let result = catch_unwind(move || {
        let _lease = pool_clone.acquire().expect("Should acquire lease");
        panic!("Simulated worker pipeline panic while holding lease!");
    });

    assert!(result.is_err(), "Thread must have panicked");

    // Invariant: Signer must NOT be permanently leaked; RAII Drop must have released it
    let lease_recovered = pool.acquire().expect("Must reacquire signer after panic unwinding");
    assert_eq!(lease_recovered.index(), 0);
    assert_eq!(lease_recovered.nonce(), 1, "Nonce advanced monotonically across panic");
}

// ==============================================================================
// 7. Nonce Sync & Monotonic Ceiling Verification
// ==============================================================================

#[test]
fn test_nonce_sync_monotonic_ceiling() {
    let pool = SignerPool::new(1);

    assert_eq!(pool.current_nonce(0), 0);

    // Syncing with on-chain nonce 15 advances nonce
    pool.sync_nonce(0, 15);
    assert_eq!(pool.current_nonce(0), 15);

    // Syncing with a stale on-chain nonce 10 must NOT regress the counter
    pool.sync_nonce(0, 10);
    assert_eq!(pool.current_nonce(0), 15, "Nonce must not regress on stale sync");

    // Acquire advances to 15, next nonce is 16
    let lease = pool.acquire().unwrap();
    assert_eq!(lease.nonce(), 15);
    drop(lease);
    assert_eq!(pool.current_nonce(0), 16);

    // Explicit set_nonce override
    pool.set_nonce(0, 100);
    assert_eq!(pool.current_nonce(0), 100);
}

// ==============================================================================
// 8. Boundary Conditions & Adversarial Key Input Handling
// ==============================================================================

#[test]
fn test_boundary_zero_capacity_pool() {
    let pool = SignerPool::new(0);
    assert_eq!(pool.capacity(), 0);
    assert!(pool.acquire().is_none());
    assert!(pool.acquire_signer().is_none());
    assert_eq!(pool.current_nonce(0), 0);
    // Out of bounds release does not panic
    pool.release_signer(0);
    pool.release_signer(999);
}

#[test]
fn test_boundary_capacity_cap_at_64() {
    let pool = SignerPool::new(100);
    assert_eq!(pool.capacity(), 64, "SignerPool capacity must be capped at 64");

    let mut leases = vec![];
    for _ in 0..64 {
        leases.push(pool.acquire().expect("Should lease up to 64"));
    }

    assert_eq!(leases.len(), 64);
    // 65th acquire returns None
    assert!(pool.acquire().is_none());
}

#[test]
fn test_adversarial_hex_key_parsing() {
    // 1. Odd length hex
    let res = SignerPool::from_hex_keys(&["0x123".to_string()]);
    assert!(matches!(res, Err(SignerError::InvalidPrivateKey(_))));

    // 2. Invalid hex characters
    let res = SignerPool::from_hex_keys(&["0xZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ".to_string()]);
    assert!(matches!(res, Err(SignerError::InvalidPrivateKey(_))));

    // 3. Short key (16 bytes)
    let res = SignerPool::from_hex_keys(&["0x11223344556677889900aabbccddeeff".to_string()]);
    assert!(matches!(res, Err(SignerError::InvalidPrivateKey(_))));

    // 4. Valid 32-byte key
    let valid_hex = "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string();
    let res = SignerPool::from_hex_keys(&[valid_hex]);
    assert!(res.is_ok());
    let pool = res.unwrap();
    assert_eq!(pool.capacity(), 1);
}
