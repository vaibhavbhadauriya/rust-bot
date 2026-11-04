//! Feature 16: Multi-EOA Signer Pool Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 70, 84, and explorer_survey_3/handoff.md.

use crate::common::*;

#[test]
fn test_f16_01_concurrent_lease_without_collision() {
    let pool = MockEoaPool::new(10);

    let lease1 = pool.acquire_signer().expect("Should lease signer 1");
    let lease2 = pool.acquire_signer().expect("Should lease signer 2");

    assert_ne!(lease1.0, lease2.0, "Signer indices must be distinct");
    assert_ne!(lease1.1, lease2.1, "Signer addresses must be distinct");
}

#[test]
fn test_f16_02_signer_release_and_reuse() {
    let pool = MockEoaPool::new(2);

    let lease1 = pool.acquire_signer().unwrap();
    let _lease2 = pool.acquire_signer().unwrap();
    assert!(pool.acquire_signer().is_none(), "Pool should be exhausted");

    // Release signer 1
    pool.release_signer(lease1.0);

    // Now acquire should succeed
    let lease3 = pool.acquire_signer().expect("Should re-acquire released signer");
    assert_eq!(lease3.0, lease1.0);
}

#[test]
fn test_f16_03_independent_nonce_sequencing() {
    let pool = MockEoaPool::new(3);

    let (idx0, _, nonce0_first) = pool.acquire_signer().unwrap();
    assert_eq!(nonce0_first, 0);
    pool.release_signer(idx0);

    let (_, _, nonce0_second) = pool.acquire_signer().unwrap();
    assert_eq!(nonce0_second, 1, "Nonce for same EOA must increment monotonically");
}

#[test]
fn test_f16_04_arbitrum_nonce_cache_bypass() {
    // 5 concurrent trades acquire 5 distinct EOAs
    let pool = MockEoaPool::new(10);
    let mut signers = vec![];

    for _ in 0..5 {
        signers.push(pool.acquire_signer().unwrap());
    }

    // Verify all 5 senders are distinct
    for i in 0..5 {
        for j in i + 1..5 {
            assert_ne!(signers[i].1, signers[j].1, "Senders must bypass single-account cache");
        }
    }
}

#[test]
fn test_f16_05_pool_exhaustion() {
    let pool = MockEoaPool::new(3);
    let _s1 = pool.acquire_signer().unwrap();
    let _s2 = pool.acquire_signer().unwrap();
    let _s3 = pool.acquire_signer().unwrap();

    // 4th acquire must return None safely
    assert!(pool.acquire_signer().is_none());
}
