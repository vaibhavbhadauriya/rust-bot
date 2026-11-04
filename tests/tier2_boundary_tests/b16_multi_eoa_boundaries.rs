//! Feature 16 Boundaries: Multi-EOA Signer Pool BVA Test Suite

use crate::common::*;

#[test]
fn test_b16_01_single_signer_pool() {
    let pool = MockEoaPool::new(1);
    let s = pool.acquire_signer();
    assert!(s.is_some());
    // Next acquire fails
    assert!(pool.acquire_signer().is_none());
}

#[test]
fn test_b16_02_20_signers_pool() {
    let pool = MockEoaPool::new(20);
    let mut signers = vec![];
    for _ in 0..20 {
        signers.push(pool.acquire_signer().unwrap());
    }
    assert_eq!(signers.len(), 20);
    assert!(pool.acquire_signer().is_none());
}

#[test]
fn test_b16_03_out_of_bounds_release_safety() {
    let pool = MockEoaPool::new(2);
    // Releasing invalid index 99 must not panic
    pool.release_signer(99);
}

#[test]
fn test_b16_04_rapid_cycle_leasing() {
    let pool = MockEoaPool::new(2);
    for _ in 0..1000 {
        let (idx, _, _) = pool.acquire_signer().unwrap();
        pool.release_signer(idx);
    }
}

#[test]
fn test_b16_05_nonce_monotonicity_high_count() {
    let pool = MockEoaPool::new(1);
    for _ in 0..50 {
        let (idx, _, _) = pool.acquire_signer().unwrap();
        pool.release_signer(idx);
    }
    assert_eq!(pool.current_nonce(0), 50);
}
