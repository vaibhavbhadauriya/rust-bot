//! Feature 12 Boundaries: Vanity Salt Miner BVA Test Suite

use crate::common::*;
use alloy_primitives::Address;

#[test]
fn test_b12_01_target_zero_bytes_boundary() {
    let deployer = Address::from([0x12; 20]);
    let salt = [0u8; 32];
    let init_code_hash = [0x34; 32];

    let addr = compute_create2_address(deployer, salt, init_code_hash);
    let count = count_leading_zero_bytes(&addr);
    assert!(count <= 20);
}

#[test]
fn test_b12_02_deployer_zero_address() {
    let deployer = Address::ZERO;
    let salt = [1u8; 32];
    let init_code_hash = [2u8; 32];

    let addr = compute_create2_address(deployer, salt, init_code_hash);
    assert_ne!(addr, Address::ZERO);
}

#[test]
fn test_b12_03_salt_all_ones() {
    let deployer = Address::from([0xAA; 20]);
    let salt = [0xFF; 32];
    let init_code_hash = [0xEE; 32];

    let addr = compute_create2_address(deployer, salt, init_code_hash);
    assert_ne!(addr, Address::ZERO);
}

#[test]
fn test_b12_04_init_code_hash_zero() {
    let deployer = Address::from([0xBB; 20]);
    let salt = [0u8; 32];
    let init_code_hash = [0u8; 32];

    let addr = compute_create2_address(deployer, salt, init_code_hash);
    assert_ne!(addr, Address::ZERO);
}

#[test]
fn test_b12_05_max_zero_bytes_address() {
    let zero_addr = Address::ZERO;
    assert_eq!(count_leading_zero_bytes(&zero_addr), 20);
}
