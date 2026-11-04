//! Feature 15 Boundaries: Honeypot Defense BVA Test Suite

use crate::common::*;
use alloy_primitives::{Address, U256};

#[test]
fn test_b15_01_exactly_10_percent_delivered_boundary() {
    let report = HoneypotSimulationReport {
        token: Address::from([0x11; 20]),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(10), // exactly 10%
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    // 10% delivered is not < 0.90 (wait: 10/100 is 0.10, which is < 0.90) -> flagged as Salmonella!
    assert!(inspect_honeypot_safety(&report).is_err());
}

#[test]
fn test_b15_02_90_percent_delivered_boundary() {
    let report = HoneypotSimulationReport {
        token: Address::from([0x22; 20]),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(90), // 10% tax
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    // 10% tax is within acceptable limit
    assert!(inspect_honeypot_safety(&report).is_ok());
}

#[test]
fn test_b15_03_89_percent_delivered_tax_violation() {
    let report = HoneypotSimulationReport {
        token: Address::from([0x33; 20]),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(89), // 11% tax (> 10%)
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    match inspect_honeypot_safety(&report) {
        Err(HoneypotViolation::ExcessiveFeeOnTransfer { fee_percent }) => {
            assert!((fee_percent - 11.0).abs() < 1e-6);
        }
        _ => panic!("Expected ExcessiveFeeOnTransfer"),
    }
}

#[test]
fn test_b15_04_both_origin_and_gasprice_traps() {
    let report = HoneypotSimulationReport {
        token: Address::from([0x44; 20]),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(100),
        used_origin_opcode: true,
        used_gasprice_opcode: true,
    };
    // Prioritizes flagging the first violation
    assert!(inspect_honeypot_safety(&report).is_err());
}

#[test]
fn test_b15_05_zero_emitted_amount_safety() {
    let report = HoneypotSimulationReport {
        token: Address::from([0x55; 20]),
        emitted_amount: U256::ZERO,
        storage_delta: U256::ZERO,
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(inspect_honeypot_safety(&report).is_ok());
}
