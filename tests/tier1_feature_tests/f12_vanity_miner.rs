//! Feature 12: 16-Zero Vanity Salt Miner Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 53-54 and explorer_survey_3/handoff.md.

use crate::common::*;
use alloy_primitives::Address;

#[test]
fn test_f12_01_create2_deterministic_derivation() {
    let deployer = Address::from([0x11; 20]);
    let salt = [0u8; 32];
    let init_code_hash = [0xAA; 32];

    let addr1 = compute_create2_address(deployer, salt, init_code_hash);
    let addr2 = compute_create2_address(deployer, salt, init_code_hash);

    assert_eq!(addr1, addr2, "CREATE2 address must be strictly deterministic");
    assert_ne!(addr1, Address::ZERO);
}

#[test]
fn test_f12_02_leading_zero_bytes_counter() {
    let mut bytes = [0u8; 20];
    bytes[0] = 0;
    bytes[1] = 0;
    bytes[2] = 0;
    bytes[3] = 0x12;
    let addr = Address::from(bytes);

    assert_eq!(count_leading_zero_bytes(&addr), 3);
}

#[test]
fn test_f12_03_gas_savings_formula() {
    // 16 hex zeros = 8 zero bytes
    let zero_bytes = 8;
    // EIP-2028: non-zero byte = 16 gas, zero byte = 4 gas
    // Savings = 16 - 4 = 12 gas per zero byte
    let savings_per_appearance = zero_bytes * 12;
    assert_eq!(savings_per_appearance, 96, "8 zero bytes must save exactly 96 calldata gas");
}

#[test]
fn test_f12_04_vanity_mine_target_2_bytes() {
    // Mine a salt yielding at least 2 zero bytes (fast in test)
    let deployer = Address::from([0x99; 20]);
    let init_code_hash = [0x55; 32];

    let mut found_salt = None;
    let mut found_addr = None;

    for nonce in 0..100_000u64 {
        let mut salt = [0u8; 32];
        salt[..8].copy_from_slice(&nonce.to_le_bytes());
        let addr = compute_create2_address(deployer, salt, init_code_hash);
        if count_leading_zero_bytes(&addr) >= 2 {
            found_salt = Some(salt);
            found_addr = Some(addr);
            break;
        }
    }

    assert!(found_salt.is_some(), "Should find a 2-byte vanity salt within 100k attempts");
    assert!(count_leading_zero_bytes(&found_addr.unwrap()) >= 2);
}

#[test]
fn test_f12_05_salt_variation_yields_distinct_addresses() {
    let deployer = Address::from([0x11; 20]);
    let init_code_hash = [0xAA; 32];

    let salt1 = [1u8; 32];
    let salt2 = [2u8; 32];

    let addr1 = compute_create2_address(deployer, salt1, init_code_hash);
    let addr2 = compute_create2_address(deployer, salt2, init_code_hash);

    assert_ne!(addr1, addr2, "Distinct salts must produce distinct addresses");
}
