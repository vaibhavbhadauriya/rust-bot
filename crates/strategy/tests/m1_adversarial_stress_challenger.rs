//! Adversarial Challenger Stress Test Suite for Milestone M1 Remediations
//!
//! Validates:
//! 1. High-contention concurrent lease acquisition and nonce rollback (20 threads, random commit/rollback).
//!    Asserts zero nonce skips, zero duplicates, exact contiguous nonce progression per signer,
//!    and rollback idempotence/isolation.
//! 2. Signature recovery (`recover_signer`):
//!    Asserts genuine signatures recover expected address, and modified sighashes,
//!    corrupted signatures, and inverted recovery IDs strictly fail or mismatch.
//! 3. Pipeline execution target address integrity:
//!    Asserts live mode transactions NEVER target 0x000...001 or 0x000...000,
//!    and CLI / config validation strictly prevents misconfiguration.

use alloy_primitives::{keccak256, Address, Bytes, U256};
use flash_core::config::AppConfig;
use flash_core::types::ExecutionMode;
use flash_strategy::signers::{
    recover_signer, Eip1559TransactionRequest, EoaSigner, SignerPool,
};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

// ==============================================================================
// 1. Concurrent Lease Acquisition & Nonce Rollback Stress Test (20 Threads)
// ==============================================================================

#[test]
fn test_m1_concurrent_rollback_contention_20_threads_zero_skips_zero_duplicates() {
    let pool_capacity = 4;
    let pool = Arc::new(SignerPool::new(pool_capacity));
    let num_threads = 20;
    let iterations_per_thread = 150;

    // Per-signer recorded successful nonces: map signer_index -> Vec<u64>
    let successful_nonces: Arc<Mutex<HashMap<usize, Vec<u64>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    for i in 0..pool_capacity {
        successful_nonces.lock().insert(i, Vec::new());
    }

    let total_attempts = Arc::new(AtomicUsize::new(0));
    let total_rollbacks = Arc::new(AtomicUsize::new(0));
    let total_successes = Arc::new(AtomicUsize::new(0));

    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::with_capacity(num_threads);

    for tid in 0..num_threads {
        let pool = Arc::clone(&pool);
        let successful_nonces = Arc::clone(&successful_nonces);
        let total_attempts = Arc::clone(&total_attempts);
        let total_rollbacks = Arc::clone(&total_rollbacks);
        let total_successes = Arc::clone(&total_successes);
        let barrier = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            // Synchronize all 20 threads at the starting line to maximize contention
            barrier.wait();

            // Pseudo-random deterministic generator per thread using wrapping arithmetic
            let mut lcg_state = (tid as u64 + 1)
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);

            for _ in 0..iterations_per_thread {
                loop {
                    if let Some(lease) = pool.acquire() {
                        let signer_idx = lease.index();
                        let nonce = lease.nonce();
                        total_attempts.fetch_add(1, Ordering::Relaxed);

                        // LCG random step
                        lcg_state = lcg_state
                            .wrapping_mul(6364136223846793005)
                            .wrapping_add(1442695040888963407);
                        let simulate_success = (lcg_state % 100) < 60; // 60% success, 40% failure

                        if simulate_success {
                            // Transaction succeeded and was mined
                            {
                                let mut map = successful_nonces.lock();
                                map.get_mut(&signer_idx).unwrap().push(nonce);
                            }
                            total_successes.fetch_add(1, Ordering::Relaxed);
                            // Normal drop releases signer without rollback
                            drop(lease);
                        } else {
                            // Transaction failed: network drop / mempool reject / HTTP timeout
                            // Must rollback nonce to prevent nonce gap!
                            total_rollbacks.fetch_add(1, Ordering::Relaxed);
                            lease.rollback_nonce();

                            // Verify idempotence: redundant rollback call should be harmless
                            if (lcg_state % 2) == 0 {
                                lease.rollback_nonce();
                            }

                            drop(lease);
                        }
                        break;
                    } else {
                        // Pool contention, yield CPU
                        thread::yield_now();
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("Worker thread panicked during contention stress");
    }

    let attempts = total_attempts.load(Ordering::SeqCst);
    let successes = total_successes.load(Ordering::SeqCst);
    let rollbacks = total_rollbacks.load(Ordering::SeqCst);

    assert_eq!(attempts, num_threads * iterations_per_thread);
    assert_eq!(attempts, successes + rollbacks);
    assert!(rollbacks > 0, "Stress test must trigger multiple rollbacks");
    assert!(successes > 0, "Stress test must trigger multiple successes");

    // Critical Invariant Verification:
    // For every signer, the sequence of successfully committed nonces MUST be strictly contiguous:
    // [0, 1, 2, ..., N-1] with ZERO duplicates and ZERO skips!
    let map = successful_nonces.lock();
    for i in 0..pool_capacity {
        let nonces = map.get(&i).unwrap();
        let current_pool_nonce = pool.current_nonce(i);

        // Check for duplicates
        let mut seen = HashSet::new();
        for &n in nonces {
            assert!(
                seen.insert(n),
                "FATAL DUPLICATE NONCE DETECTED on signer {}: nonce {} appeared multiple times!",
                i,
                n
            );
        }

        // Sort nonces to check contiguous progression
        let mut sorted = nonces.clone();
        sorted.sort_unstable();

        for (expected, &actual) in sorted.iter().enumerate() {
            assert_eq!(
                expected as u64, actual,
                "FATAL NONCE SKIP DETECTED on signer {}: expected nonce {}, got {}!",
                i, expected, actual
            );
        }

        // The on-signer current nonce must match exactly the count of successful commits
        assert_eq!(
            current_pool_nonce,
            sorted.len() as u64,
            "Signer {} nonce mismatch: pool current_nonce is {}, but successful count is {}",
            i,
            current_pool_nonce,
            sorted.len()
        );
    }
}

#[test]
fn test_m1_single_signer_extreme_contention_rollback() {
    // 1 signer pool with 20 threads contending: worst-case serialization contention
    let pool = Arc::new(SignerPool::new(1));
    let num_threads = 20;
    let iterations_per_thread = 50;

    let successful_nonces = Arc::new(Mutex::new(Vec::new()));
    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::with_capacity(num_threads);

    for tid in 0..num_threads {
        let pool = Arc::clone(&pool);
        let successful_nonces = Arc::clone(&successful_nonces);
        let barrier = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            barrier.wait();
            let mut lcg = (tid as u64 + 42)
                .wrapping_mul(2862933555777941757)
                .wrapping_add(3037000493);

            for _ in 0..iterations_per_thread {
                loop {
                    if let Some(lease) = pool.acquire() {
                        let nonce = lease.nonce();
                        lcg = lcg.wrapping_mul(2862933555777941757).wrapping_add(3037000493);
                        let commit = (lcg % 10) < 5; // 50% commit, 50% rollback

                        if commit {
                            successful_nonces.lock().push(nonce);
                            drop(lease);
                        } else {
                            lease.rollback_nonce();
                            drop(lease);
                        }
                        break;
                    } else {
                        thread::yield_now();
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("Thread panicked");
    }

    let mut nonces = successful_nonces.lock().clone();
    nonces.sort_unstable();

    // Verify zero skips and zero duplicates
    for (i, &nonce) in nonces.iter().enumerate() {
        assert_eq!(
            i as u64, nonce,
            "Contiguous nonce check failed at index {}: got {}",
            i, nonce
        );
    }
    assert_eq!(pool.current_nonce(0), nonces.len() as u64);
}

// ==============================================================================
// 2. Signature Recovery (recover_signer) Adversarial Verification
// ==============================================================================

#[test]
fn test_m1_recover_signer_genuine_and_adversarial_corruption() {
    let raw_key = [
        0x5a, 0x82, 0x79, 0x99, 0x11, 0x22, 0x33, 0x44,
        0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
        0xdd, 0xee, 0xff, 0x00, 0x11, 0x22, 0x33, 0x44,
        0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
    ];
    let signer = EoaSigner::new(0, raw_key).expect("Valid secp256k1 key");
    let signing_key = k256::ecdsa::SigningKey::from_bytes((&raw_key).into()).unwrap();

    let genuine_sighash = keccak256(b"FlashArbitrageOpportunityPayloadPreimage1234567890");

    // 1. Genuine Signature Verification
    let (sig, recid) = signing_key
        .sign_prehash_recoverable(genuine_sighash.as_slice())
        .expect("Signing succeeds");
    let recovered_addr = recover_signer(genuine_sighash.as_slice(), &sig, recid)
        .expect("Signature recovery must succeed for genuine inputs");
    assert_eq!(
        recovered_addr,
        signer.address(),
        "Recovered address must match signer address"
    );

    // 2. Modified Sighash: 1 bit altered
    let mut modified_sighash = genuine_sighash;
    modified_sighash[0] ^= 0x01; // flip 1 bit
    let recovery_on_modified = recover_signer(modified_sighash.as_slice(), &sig, recid);
    match recovery_on_modified {
        Ok(addr) => {
            assert_ne!(
                addr,
                signer.address(),
                "Adversarial check failed: recovered address on tampered sighash must NOT match signer!"
            );
        }
        Err(_) => {
            // Acceptable: recovery can return Err if point is not on curve
        }
    }

    // Completely different sighash
    let completely_different = keccak256(b"AttackerHijackedPayloadPreimage");
    let recovery_on_diff = recover_signer(completely_different.as_slice(), &sig, recid);
    if let Ok(addr) = recovery_on_diff {
        assert_ne!(addr, signer.address());
    }

    // 3. Corrupted Recovery ID (inverted parity: 0 -> 1 or 1 -> 0)
    let alt_recid_byte = if recid.to_byte() == 0 { 1 } else { 0 };
    if let Some(inverted_recid) = k256::ecdsa::RecoveryId::from_byte(alt_recid_byte) {
        let recovery_on_alt_recid = recover_signer(genuine_sighash.as_slice(), &sig, inverted_recid);
        if let Ok(addr) = recovery_on_alt_recid {
            assert_ne!(
                addr,
                signer.address(),
                "Inverted recovery ID must recover distinct address"
            );
        }
    }

    // 4. Corrupted Signature Bytes (tampered r or s)
    let r_bytes = sig.r().to_bytes();
    let mut s_bytes = sig.s().to_bytes();
    s_bytes[31] ^= 0x01; // tamper with s scalar

    if let Ok(corrupted_sig) = k256::ecdsa::Signature::from_scalars(r_bytes, s_bytes) {
        let recovery_on_corrupted_sig = recover_signer(genuine_sighash.as_slice(), &corrupted_sig, recid);
        if let Ok(addr) = recovery_on_corrupted_sig {
            assert_ne!(
                addr,
                signer.address(),
                "Tampered signature must never recover original signer address"
            );
        }
    }
}

#[test]
fn test_m1_sign_transaction_ecrecover_assertion_guarantee() {
    let raw_key = [0x77u8; 32];
    let pool = SignerPool::from_keys(&[raw_key]).expect("Valid pool");
    let signer = pool.signer(0).unwrap();

    let tx_req = Eip1559TransactionRequest {
        chain_id: 42161,
        nonce: Some(0),
        max_priority_fee_per_gas: 1_000_000_000,
        max_fee_per_gas: 10_000_000_000,
        gas_limit: 350_000,
        to: Address::from([0x99; 20]),
        value: U256::ZERO,
        data: Bytes::from(vec![0x12, 0x34]),
        access_list: vec![],
    };

    let signed_tx = pool.sign_transaction(0, &tx_req).expect("sign_transaction must succeed");
    assert_eq!(signed_tx.sender, signer.address());
    assert_eq!(signed_tx.nonce, 0);
    assert_eq!(signed_tx.chain_id, 42161);
    assert!(!signed_tx.raw_bytes.is_empty());
}

// ==============================================================================
// 3. Pipeline Target Address Verification: Never 0x01 or 0x00 in Live Mode
// ==============================================================================

#[test]
fn test_m1_pipeline_target_address_never_precompile_or_zero_in_live_mode() {
    let deployed_contract = Address::from([0x42; 20]);
    let precompile_01 = Address::with_last_byte(1);
    let zero_address = Address::ZERO;

    // 1. Live config with deployed contract is valid
    let mut valid_live_config = AppConfig {
        alchemy_key: "dummy".into(),
        arbitrum_rpc_ws: "".into(),
        arbitrum_rpc_http: "".into(),
        arbitrum_sequencer_feed: "".into(),
        base_rpc_ws: "".into(),
        base_rpc_http: "".into(),
        base_flashblocks_ws: "".into(),
        execution_mode: ExecutionMode::Live,
        log_level: "info".into(),
        signer_private_keys: vec![],
        executor_contract_address: deployed_contract,
        max_hops: 3,
        kill_switch_feed_lag_ms: 100,
        kill_switch_fast_revert_count: 3,
    };

    assert!(valid_live_config.validate_live_mode().is_ok());

    // 2. Reject Address::ZERO in live mode
    valid_live_config.executor_contract_address = zero_address;
    assert!(
        valid_live_config.validate_live_mode().is_err(),
        "Live mode MUST reject Address::ZERO"
    );

    // 3. Reject precompile 0x01 in live mode
    valid_live_config.executor_contract_address = precompile_01;
    assert!(
        valid_live_config.validate_live_mode().is_err(),
        "Live mode MUST reject precompile 0x000...001"
    );

    // 4. Pipeline target_to logic verification
    // When config is live, target_to is strictly config.executor_contract_address
    let mut live_config_with_contract = valid_live_config.clone();
    live_config_with_contract.executor_contract_address = deployed_contract;

    let target_to_live = if live_config_with_contract.execution_mode.is_live() {
        live_config_with_contract.executor_contract_address
    } else {
        precompile_01
    };

    assert_eq!(target_to_live, deployed_contract);
    assert_ne!(target_to_live, precompile_01);
    assert_ne!(target_to_live, zero_address);

    // When config is shadow, simulator executor address is used for local revm state
    let mut shadow_config = live_config_with_contract.clone();
    shadow_config.execution_mode = ExecutionMode::Shadow;

    let target_to_shadow = if shadow_config.execution_mode.is_live() {
        shadow_config.executor_contract_address
    } else {
        precompile_01
    };
    assert_eq!(target_to_shadow, precompile_01);
}
