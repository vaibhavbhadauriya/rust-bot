//! Feature 15: revm-trace Honeypot Defense Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 45, 89, and explorer_survey_3/handoff.md.

use crate::common::*;
use alloy_primitives::{Address, U256};

#[test]
fn test_f15_01_salmonella_partial_delivery_detected() {
    // Malicious Salmonella token: emits 100 ETH Transfer event, but only delivers 10 ETH to recipient!
    let report = HoneypotSimulationReport {
        token: Address::from([0x66; 20]),
        emitted_amount: U256::from(100_000_000_000_000_000_000u128),
        storage_delta: U256::from(10_000_000_000_000_000_000u128), // Only 10%
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };

    let inspection = inspect_honeypot_safety(&report);
    assert_eq!(
        inspection,
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: report.emitted_amount,
            delivered: report.storage_delta,
        }),
        "Salmonella token transferring only 10% balance must be detected and rejected"
    );
}

#[test]
fn test_f15_02_honest_token_accepted() {
    let report = HoneypotSimulationReport {
        token: Address::from([0x77; 20]),
        emitted_amount: U256::from(50_000_000_000_000_000_000u128),
        storage_delta: U256::from(50_000_000_000_000_000_000u128), // 100% delivered
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };

    assert!(inspect_honeypot_safety(&report).is_ok(), "Honest token must be accepted");
}

#[test]
fn test_f15_03_origin_opcode_trap_detected() {
    let report = HoneypotSimulationReport {
        token: Address::from([0x88; 20]),
        emitted_amount: U256::from(10_000_000_000_000_000_000u128),
        storage_delta: U256::from(10_000_000_000_000_000_000u128),
        used_origin_opcode: true, // ORIGIN branch detected!
        used_gasprice_opcode: false,
    };

    assert_eq!(
        inspect_honeypot_safety(&report),
        Err(HoneypotViolation::OriginSandwichTrap),
        "Contracts branching on ORIGIN must be flagged as MEV honeypots"
    );
}

#[test]
fn test_f15_04_gasprice_opcode_trap_detected() {
    let report = HoneypotSimulationReport {
        token: Address::from([0x99; 20]),
        emitted_amount: U256::from(10_000_000_000_000_000_000u128),
        storage_delta: U256::from(10_000_000_000_000_000_000u128),
        used_origin_opcode: false,
        used_gasprice_opcode: true, // GASPRICE branch detected!
    };

    assert_eq!(
        inspect_honeypot_safety(&report),
        Err(HoneypotViolation::GasPriceSandwichTrap),
        "Contracts branching on GASPRICE must be flagged as sandwich traps"
    );
}

#[test]
fn test_f15_05_excessive_fee_on_transfer() {
    // 15% transfer tax (exceeds 10% limit)
    let emitted = 100_000_000_000_000_000_000u128;
    let delivered = 85_000_000_000_000_000_000u128; // 85% delivered (15% tax)
    let report = HoneypotSimulationReport {
        token: Address::from([0x55; 20]),
        emitted_amount: U256::from(emitted),
        storage_delta: U256::from(delivered),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };

    match inspect_honeypot_safety(&report) {
        Err(HoneypotViolation::ExcessiveFeeOnTransfer { fee_percent }) => {
            assert!((fee_percent - 15.0).abs() < 0.1);
        }
        _ => panic!("Expected ExcessiveFeeOnTransfer error"),
    }
}
