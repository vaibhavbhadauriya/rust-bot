//! Honeypot & Opcode Trap Stress Challenger Test Suite
//! Empirically stress-tests HoneypotTxInspector and honeypot detection logic against
//! adversarial honeypot attack contracts and EVM boundary conditions.

use alloy_primitives::{Address, Bytes, FixedBytes, U256};
use flash_simulator::{
    honeypot::{inspect_honeypot_safety, HoneypotSimulationReport},
    inspector::{
        decode_revert_reason, HoneypotTxInspector, HoneypotViolation,
    },
};
use revm::{
    context::{Context, TxEnv},
    database::in_memory_db::InMemoryDB,
    handler::{MainBuilder, MainContext},
    inspector::InspectEvm,
    primitives::{hardfork::SpecId, TxKind},
    state::AccountInfo,
};

// ============================================================================
// VECTOR 1: Exact Boundary Delivery Fractions
// ============================================================================

#[test]
fn test_vector1_01_exactly_10_percent_delivery_salmonella() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    // Case A: Emitted = 100, Delivered = 10 (exactly 10%)
    // 5 * 10 = 50 <= 100 -> Salmonella must trigger
    let report = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(10),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert_eq!(
        inspect_honeypot_safety(&report),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: U256::from(100),
            delivered: U256::from(10),
        })
    );

    // Also test HoneypotTxInspector::evaluate_safety
    let mut inspector = HoneypotTxInspector::new(token, recipient);
    inspector.emitted_amount = U256::from(100);
    inspector.storage_delta = U256::from(10);
    assert_eq!(
        inspector.evaluate_safety(),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: U256::from(100),
            delivered: U256::from(10),
        })
    );

    // Case B: Large scale: Emitted = 1,000,000, Delivered = 100,000
    let report_large = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(1_000_000),
        storage_delta: U256::from(100_000),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert_eq!(
        inspect_honeypot_safety(&report_large),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: U256::from(1_000_000),
            delivered: U256::from(100_000),
        })
    );
}

#[test]
fn test_vector1_02_exactly_20_percent_delivery_salmonella() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    // Case A: Emitted = 100, Delivered = 20 (exactly 20%)
    // 5 * 20 = 100 <= 100 -> Salmonella must trigger
    let report = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(20),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert_eq!(
        inspect_honeypot_safety(&report),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: U256::from(100),
            delivered: U256::from(20),
        })
    );

    let mut inspector = HoneypotTxInspector::new(token, recipient);
    inspector.emitted_amount = U256::from(100);
    inspector.storage_delta = U256::from(20);
    assert_eq!(
        inspector.evaluate_safety(),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: U256::from(100),
            delivered: U256::from(20),
        })
    );

    // Case B: Emitted = 5, Delivered = 1
    // 5 * 1 = 5 <= 5 -> Salmonella must trigger
    let report_small = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(5),
        storage_delta: U256::from(1),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert_eq!(
        inspect_honeypot_safety(&report_small),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: U256::from(5),
            delivered: U256::from(1),
        })
    );
}

#[test]
fn test_vector1_03_exactly_20_0001_percent_delivery_listeria() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    // Delivered = 200,001, Expected = 1,000,000
    // 5 * 200,001 = 1,000,005 > 1,000,000 -> Salmonella does NOT trigger
    // 10 * 200,001 = 2,000,010 < 9 * 1,000,000 = 9,000,000 -> Listeria DOES trigger!
    let report = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(1_000_000),
        storage_delta: U256::from(200_001),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    match inspect_honeypot_safety(&report) {
        Err(HoneypotViolation::ExcessiveFeeOnTransfer { fee_percent }) => {
            let expected_fee = (1.0 - (200_001.0 / 1_000_000.0)) * 100.0;
            assert!((fee_percent - expected_fee).abs() < 1e-4);
        }
        other => panic!("Expected ExcessiveFeeOnTransfer, got {:?}", other),
    }

    let mut inspector = HoneypotTxInspector::new(token, recipient);
    inspector.emitted_amount = U256::from(1_000_000);
    inspector.storage_delta = U256::from(200_001);
    match inspector.evaluate_safety() {
        Err(HoneypotViolation::ExcessiveFeeOnTransfer { fee_percent }) => {
            let expected_fee = (1.0 - (200_001.0 / 1_000_000.0)) * 100.0;
            assert!((fee_percent - expected_fee).abs() < 1e-4);
        }
        other => panic!("Expected ExcessiveFeeOnTransfer, got {:?}", other),
    }
}

#[test]
fn test_vector1_04_exactly_89_999_percent_delivery_listeria() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    // Delivered = 89,999, Expected = 100,000
    // 10 * 89,999 = 899,990 < 9 * 100,000 = 900,000 -> Listeria triggers!
    let report = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100_000),
        storage_delta: U256::from(89_999),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    match inspect_honeypot_safety(&report) {
        Err(HoneypotViolation::ExcessiveFeeOnTransfer { fee_percent }) => {
            assert!((fee_percent - 10.001).abs() < 1e-3);
        }
        other => panic!("Expected ExcessiveFeeOnTransfer, got {:?}", other),
    }

    let mut inspector = HoneypotTxInspector::new(token, recipient);
    inspector.emitted_amount = U256::from(100_000);
    inspector.storage_delta = U256::from(89_999);
    assert!(matches!(
        inspector.evaluate_safety(),
        Err(HoneypotViolation::ExcessiveFeeOnTransfer { .. })
    ));
}

#[test]
fn test_vector1_05_exactly_90_percent_delivery_safe() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    // Delivered = 90, Expected = 100 (exactly 90%)
    // 10 * 90 = 900, which is NOT < 900. Listeria does NOT trigger.
    let report = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(90),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(inspect_honeypot_safety(&report).is_ok());

    let mut inspector = HoneypotTxInspector::new(token, recipient);
    inspector.emitted_amount = U256::from(100);
    inspector.storage_delta = U256::from(90);
    assert!(inspector.evaluate_safety().is_ok());
}

#[test]
fn test_vector1_06_exactly_100_percent_delivery_safe() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    let report = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(1_000_000_000_000_000_000u128),
        storage_delta: U256::from(1_000_000_000_000_000_000u128),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(inspect_honeypot_safety(&report).is_ok());

    let mut inspector = HoneypotTxInspector::new(token, recipient);
    inspector.emitted_amount = U256::from(1_000_000_000_000_000_000u128);
    inspector.storage_delta = U256::from(1_000_000_000_000_000_000u128);
    assert!(inspector.evaluate_safety().is_ok());
}

// ============================================================================
// VECTOR 2: Zero & Extreme Quantities and Overflow Safety
// ============================================================================

#[test]
fn test_vector2_01_zero_delivered_and_zero_expected() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    let report = HoneypotSimulationReport {
        token,
        emitted_amount: U256::ZERO,
        storage_delta: U256::ZERO,
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    // No division by zero, cleanly accepted
    assert!(inspect_honeypot_safety(&report).is_ok());

    let mut inspector = HoneypotTxInspector::new(token, recipient);
    inspector.emitted_amount = U256::ZERO;
    inspector.storage_delta = U256::ZERO;
    assert!(inspector.evaluate_safety().is_ok());
}

#[test]
fn test_vector2_02_zero_delivered_and_max_expected() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    let report = HoneypotSimulationReport {
        token,
        emitted_amount: U256::MAX,
        storage_delta: U256::ZERO,
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    // 0 delivered out of U256::MAX must trigger Salmonella
    assert_eq!(
        inspect_honeypot_safety(&report),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: U256::MAX,
            delivered: U256::ZERO,
        })
    );

    let mut inspector = HoneypotTxInspector::new(token, recipient);
    inspector.emitted_amount = U256::MAX;
    inspector.storage_delta = U256::ZERO;
    assert_eq!(
        inspector.evaluate_safety(),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: U256::MAX,
            delivered: U256::ZERO,
        })
    );
}

#[test]
fn test_vector2_03_max_delivered_and_max_expected() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    let report = HoneypotSimulationReport {
        token,
        emitted_amount: U256::MAX,
        storage_delta: U256::MAX,
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    // Exact delivery at maximum scale: safe, no overflow
    assert!(inspect_honeypot_safety(&report).is_ok());

    let mut inspector = HoneypotTxInspector::new(token, recipient);
    inspector.emitted_amount = U256::MAX;
    inspector.storage_delta = U256::MAX;
    assert!(inspector.evaluate_safety().is_ok());
}

// ============================================================================
// VECTOR 3: Opcode Traps Stress in Native EVM Execution
// ============================================================================

#[test]
fn test_vector3_01_evm_opcode_trap_origin() {
    let mut db = InMemoryDB::default();
    let token_address = Address::from([0xa1; 20]);
    let recipient = Address::from([0xb2; 20]);
    let caller = Address::from([0xc3; 20]);

    // Bytecode that executes ORIGIN (0x32), pops it (0x50), and returns
    let bytecode = Bytes::from_static(&[0x32, 0x50, 0x00]);

    let account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: FixedBytes::ZERO,
        code: Some(revm::bytecode::Bytecode::new_raw(bytecode)),
        account_id: None,
    };
    db.insert_account_info(token_address, account);

    let tx = TxEnv {
        caller,
        kind: TxKind::Call(token_address),
        gas_limit: 100_000,
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg: &mut revm::context::CfgEnv| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN)
        })
        .with_db(db);

    let inspector = HoneypotTxInspector::new(token_address, recipient);
    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let res = evm.inspect_tx(tx).unwrap();

    assert!(res.result.is_success());
    assert!(evm.inspector.used_origin_opcode, "ORIGIN opcode must be detected");
    assert_eq!(
        evm.inspector.evaluate_safety(),
        Err(HoneypotViolation::OriginSandwichTrap)
    );
}

#[test]
fn test_vector3_02_evm_opcode_trap_gasprice() {
    let mut db = InMemoryDB::default();
    let token_address = Address::from([0xa1; 20]);
    let recipient = Address::from([0xb2; 20]);
    let caller = Address::from([0xc3; 20]);

    // Bytecode: 3a (GASPRICE), 50 (POP), 00 (STOP)
    let bytecode = Bytes::from_static(&[0x3a, 0x50, 0x00]);

    let account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: FixedBytes::ZERO,
        code: Some(revm::bytecode::Bytecode::new_raw(bytecode)),
        account_id: None,
    };
    db.insert_account_info(token_address, account);

    let tx = TxEnv {
        caller,
        kind: TxKind::Call(token_address),
        gas_limit: 100_000,
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg: &mut revm::context::CfgEnv| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN)
        })
        .with_db(db);

    let inspector = HoneypotTxInspector::new(token_address, recipient);
    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let res = evm.inspect_tx(tx).unwrap();

    assert!(res.result.is_success());
    assert!(evm.inspector.used_gasprice_opcode, "GASPRICE opcode must be detected");
    assert_eq!(
        evm.inspector.evaluate_safety(),
        Err(HoneypotViolation::GasPriceSandwichTrap)
    );
}

#[test]
fn test_vector3_03_evm_opcode_trap_coinbase() {
    let mut db = InMemoryDB::default();
    let token_address = Address::from([0xa1; 20]);
    let recipient = Address::from([0xb2; 20]);
    let caller = Address::from([0xc3; 20]);

    // Bytecode: 41 (COINBASE), 50 (POP), 00 (STOP)
    let bytecode = Bytes::from_static(&[0x41, 0x50, 0x00]);

    let account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: FixedBytes::ZERO,
        code: Some(revm::bytecode::Bytecode::new_raw(bytecode)),
        account_id: None,
    };
    db.insert_account_info(token_address, account);

    let tx = TxEnv {
        caller,
        kind: TxKind::Call(token_address),
        gas_limit: 100_000,
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg: &mut revm::context::CfgEnv| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN)
        })
        .with_db(db);

    let inspector = HoneypotTxInspector::new(token_address, recipient);
    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let res = evm.inspect_tx(tx).unwrap();

    assert!(res.result.is_success());
    assert!(evm.inspector.used_coinbase_opcode, "COINBASE opcode must be detected");
    assert_eq!(
        evm.inspector.evaluate_safety(),
        Err(HoneypotViolation::CoinbaseSandwichTrap)
    );
}

#[test]
fn test_vector3_04_evm_multiple_opcode_traps() {
    let mut db = InMemoryDB::default();
    let token_address = Address::from([0xa1; 20]);
    let recipient = Address::from([0xb2; 20]);
    let caller = Address::from([0xc3; 20]);

    // Bytecode: 32 (ORIGIN), 3a (GASPRICE), 41 (COINBASE), 50, 50, 50, 00
    let bytecode = Bytes::from_static(&[0x32, 0x3a, 0x41, 0x50, 0x50, 0x50, 0x00]);

    let account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: FixedBytes::ZERO,
        code: Some(revm::bytecode::Bytecode::new_raw(bytecode)),
        account_id: None,
    };
    db.insert_account_info(token_address, account);

    let tx = TxEnv {
        caller,
        kind: TxKind::Call(token_address),
        gas_limit: 100_000,
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg: &mut revm::context::CfgEnv| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN)
        })
        .with_db(db);

    let inspector = HoneypotTxInspector::new(token_address, recipient);
    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let res = evm.inspect_tx(tx).unwrap();

    assert!(res.result.is_success());
    assert!(evm.inspector.used_origin_opcode, "ORIGIN must be recorded");
    assert!(evm.inspector.used_gasprice_opcode, "GASPRICE must be recorded");
    assert!(evm.inspector.used_coinbase_opcode, "COINBASE must be recorded");
    // Priority check flags the first violation
    assert!(evm.inspector.evaluate_safety().is_err());
}

#[test]
fn test_vector3_05_evm_clean_operations_no_false_positives() {
    let mut db = InMemoryDB::default();
    let token_address = Address::from([0xa1; 20]);
    let recipient = Address::from([0xb2; 20]);
    let caller = Address::from([0xc3; 20]);

    // Standard bytecode: PUSH1 42, PUSH1 0, MSTORE, PUSH1 32, PUSH1 0, RETURN
    let bytecode = Bytes::from_static(&[0x60, 0x2a, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3]);

    let account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: FixedBytes::ZERO,
        code: Some(revm::bytecode::Bytecode::new_raw(bytecode)),
        account_id: None,
    };
    db.insert_account_info(token_address, account);

    let tx = TxEnv {
        caller,
        kind: TxKind::Call(token_address),
        gas_limit: 100_000,
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg: &mut revm::context::CfgEnv| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN)
        })
        .with_db(db);

    let inspector = HoneypotTxInspector::new(token_address, recipient);
    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let res = evm.inspect_tx(tx).unwrap();

    assert!(res.result.is_success());
    assert!(!evm.inspector.used_origin_opcode);
    assert!(!evm.inspector.used_gasprice_opcode);
    assert!(!evm.inspector.used_coinbase_opcode);
    assert!(!evm.inspector.reverted);
    assert!(evm.inspector.evaluate_safety().is_ok());
}

// ============================================================================
// VECTOR 4: Revert Reason & Blacklist Strings
// ============================================================================

fn encode_abi_error_string(reason: &str) -> Vec<u8> {
    // 0x08c379a0 selector
    let mut bytes = vec![0x08, 0xc3, 0x79, 0xa0];
    // offset to string (0x20 = 32)
    bytes.extend_from_slice(&[0u8; 31]);
    bytes.push(0x20);
    // string length
    let len = reason.len();
    let mut len_bytes = [0u8; 32];
    len_bytes[24..32].copy_from_slice(&(len as u64).to_be_bytes());
    bytes.extend_from_slice(&len_bytes);
    // string bytes
    bytes.extend_from_slice(reason.as_bytes());
    // pad to 32 bytes
    let pad = (32 - (reason.len() % 32)) % 32;
    bytes.extend(std::iter::repeat(0).take(pad));
    bytes
}

fn encode_abi_panic(code: u64) -> Vec<u8> {
    // 0x4e487b71 selector
    let mut bytes = vec![0x4e, 0x48, 0x7b, 0x71];
    let mut code_bytes = [0u8; 32];
    code_bytes[24..32].copy_from_slice(&code.to_be_bytes());
    bytes.extend_from_slice(&code_bytes);
    bytes
}

#[test]
fn test_vector4_01_standard_error_string_decoding() {
    // Test "BOT_NOT_ALLOWED"
    let enc1 = encode_abi_error_string("BOT_NOT_ALLOWED");
    assert_eq!(decode_revert_reason(&enc1), "BOT_NOT_ALLOWED");

    // Test "BLACKLISTED"
    let enc2 = encode_abi_error_string("BLACKLISTED");
    assert_eq!(decode_revert_reason(&enc2), "BLACKLISTED");

    // Test "Slippage"
    let enc3 = encode_abi_error_string("Slippage");
    assert_eq!(decode_revert_reason(&enc3), "Slippage");
}

#[test]
fn test_vector4_02_panic_code_decoding() {
    // Panic 0x01 (assert failed)
    let p1 = encode_abi_panic(0x01);
    assert_eq!(decode_revert_reason(&p1), "Panic(0x1)");

    // Panic 0x11 (overflow/underflow)
    let p2 = encode_abi_panic(0x11);
    assert_eq!(decode_revert_reason(&p2), "Panic(0x11)");

    // Panic 0x12 (division by zero)
    let p3 = encode_abi_panic(0x12);
    assert_eq!(decode_revert_reason(&p3), "Panic(0x12)");

    // Panic 0x32 (array out of bounds)
    let p4 = encode_abi_panic(0x32);
    assert_eq!(decode_revert_reason(&p4), "Panic(0x32)");
}

#[test]
fn test_vector4_03_empty_and_raw_revert_payloads() {
    // Empty revert
    assert_eq!(decode_revert_reason(&[]), "Empty revert");

    // Raw hex error
    let raw = vec![0xde, 0xad, 0xbe, 0xef];
    assert_eq!(decode_revert_reason(&raw), "Custom error (0xdeadbeef)");

    // Direct plain ascii string without selector
    let plain = b"direct error message";
    assert_eq!(decode_revert_reason(plain), "direct error message");
}

#[test]
fn test_vector4_04_blacklist_violation_in_revert() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    // Revert with "BLACKLISTED: address blocked"
    let enc = encode_abi_error_string("BLACKLISTED: address blocked");

    let mut db = InMemoryDB::default();
    // Bytecode that reverts with the encoded reason
    // Store in memory, then REVERT(0, size)
    let mut code = vec![];
    for (i, chunk) in enc.chunks(32).enumerate() {
        let mut padded = [0u8; 32];
        padded[..chunk.len()].copy_from_slice(chunk);
        // PUSH32 <chunk>
        code.push(0x7f);
        code.extend_from_slice(&padded);
        // PUSH1 <offset>
        code.push(0x60);
        code.push((i * 32) as u8);
        // MSTORE
        code.push(0x52);
    }
    // PUSH1 <size>
    code.push(0x60);
    code.push(enc.len() as u8);
    // PUSH1 0
    code.push(0x60);
    code.push(0x00);
    // REVERT
    code.push(0xfd);

    let account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: FixedBytes::ZERO,
        code: Some(revm::bytecode::Bytecode::new_raw(Bytes::from(code))),
        account_id: None,
    };
    db.insert_account_info(token, account);

    let tx = TxEnv {
        caller: recipient,
        kind: TxKind::Call(token),
        gas_limit: 100_000,
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg: &mut revm::context::CfgEnv| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN)
        })
        .with_db(db);

    let inspector = HoneypotTxInspector::new(token, recipient);
    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let _ = evm.inspect_tx(tx).unwrap();

    assert!(evm.inspector.reverted);
    assert_eq!(
        evm.inspector.violation,
        Some(HoneypotViolation::BlacklistedRecipient)
    );
}

// ============================================================================
// ADDITIONAL ADVERSARIAL STRESS VECTORS
// ============================================================================

#[test]
fn test_vector2_04_extreme_u256_greater_than_u128() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    // Emitted and delivered exceed u128::MAX (2^128 - 1)
    let large_e = U256::from(1) << 150;
    let large_d = (U256::from(1) << 150) / U256::from(10); // exactly 10% delivery

    let report = HoneypotSimulationReport {
        token,
        emitted_amount: large_e,
        storage_delta: large_d,
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };

    // 5 * large_d <= large_e -> must trigger Salmonella
    assert_eq!(
        inspect_honeypot_safety(&report),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: large_e,
            delivered: large_d,
        })
    );

    let mut inspector = HoneypotTxInspector::new(token, recipient);
    inspector.emitted_amount = large_e;
    inspector.storage_delta = large_d;
    assert_eq!(
        inspector.evaluate_safety(),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: large_e,
            delivered: large_d,
        })
    );
}

#[test]
fn test_vector2_05_revert_malformed_and_boundary_lengths() {
    // 1-byte payload
    assert_eq!(decode_revert_reason(&[0x08]), "Custom error (0x08)");

    // 2-byte payload
    assert_eq!(decode_revert_reason(&[0x08, 0xc3]), "Custom error (0x08c3)");

    // 3-byte payload
    assert_eq!(decode_revert_reason(&[0x08, 0xc3, 0x79]), "Custom error (0x08c379)");

    // 4-byte selector without data
    assert_eq!(decode_revert_reason(&[0x08, 0xc3, 0x79, 0xa0]), "Custom error (0x08c379a0)");

    // 35 bytes (just short of 36 bytes for Panic selector)
    let mut short_panic = vec![0x4e, 0x48, 0x7b, 0x71];
    short_panic.extend_from_slice(&[0u8; 31]);
    assert!(decode_revert_reason(&short_panic).starts_with("Custom error"));

    // 67 bytes (just short of 68 bytes for Error selector)
    let mut short_err = vec![0x08, 0xc3, 0x79, 0xa0];
    short_err.extend_from_slice(&[0u8; 63]);
    assert!(decode_revert_reason(&short_err).starts_with("Custom error"));

    // Error selector with huge claimed length (0xffffffff) but truncated data
    let mut malformed_len = vec![0x08, 0xc3, 0x79, 0xa0];
    malformed_len.extend_from_slice(&[0u8; 31]);
    malformed_len.push(0x20); // offset = 32
    malformed_len.extend_from_slice(&[0u8; 28]);
    malformed_len.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]); // claimed length = 4GB
    malformed_len.extend_from_slice(b"short string");
    // Must gracefully fall back without OOM or panic
    let decoded = decode_revert_reason(&malformed_len);
    assert!(decoded.starts_with("Custom error"));
}

#[test]
fn test_vector3_06_evm_erc20_transfer_log_interception() {
    let mut db = InMemoryDB::default();
    let token_address = Address::from([0xa1; 20]);
    let recipient = Address::from([0xb2; 20]);
    let caller = Address::from([0xc3; 20]);

    // Construct EVM bytecode that emits ERC-20 Transfer log:
    // LOG3(mem_offset, mem_size, topic0, topic1, topic2)
    // mem[0..32] = 5000 (amount)
    // topic0 = TRANSFER_TOPIC
    // topic1 = caller
    // topic2 = recipient
    let amount = U256::from(5000);
    let mut code = vec![];

    // PUSH32 amount -> MSTORE(0)
    code.push(0x7f);
    code.extend_from_slice(&amount.to_be_bytes::<32>());
    code.push(0x60); // PUSH1
    code.push(0x00); // 0
    code.push(0x52); // MSTORE

    // Push topic2 (recipient as word)
    code.push(0x7f);
    code.extend_from_slice(recipient.into_word().as_slice());

    // Push topic1 (caller as word)
    code.push(0x7f);
    code.extend_from_slice(caller.into_word().as_slice());

    // Push topic0 (TRANSFER_TOPIC)
    code.push(0x7f);
    code.extend_from_slice(&flash_simulator::inspector::TRANSFER_TOPIC.0);

    // Push size = 32, offset = 0
    code.push(0x60);
    code.push(0x20);
    code.push(0x60);
    code.push(0x00);

    // LOG3
    code.push(0xa3);

    // STOP
    code.push(0x00);

    let account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: FixedBytes::ZERO,
        code: Some(revm::bytecode::Bytecode::new_raw(Bytes::from(code))),
        account_id: None,
    };
    db.insert_account_info(token_address, account);

    let tx = TxEnv {
        caller,
        kind: TxKind::Call(token_address),
        gas_limit: 100_000,
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg: &mut revm::context::CfgEnv| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN)
        })
        .with_db(db);

    let inspector = HoneypotTxInspector::new(token_address, recipient);
    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let res = evm.inspect_tx(tx).unwrap();

    assert!(res.result.is_success());
    assert_eq!(evm.inspector.emitted_amount, U256::from(5000), "Inspector must record emitted transfer amount");
}

#[test]
fn test_vector3_07_evm_erc20_transfer_log_different_recipient_ignored() {
    let mut db = InMemoryDB::default();
    let token_address = Address::from([0xa1; 20]);
    let recipient = Address::from([0xb2; 20]);
    let other_recipient = Address::from([0xe5; 20]);
    let caller = Address::from([0xc3; 20]);

    // Emits Transfer to other_recipient, NOT our recipient
    let amount = U256::from(5000);
    let mut code = vec![];

    code.push(0x7f);
    code.extend_from_slice(&amount.to_be_bytes::<32>());
    code.push(0x60);
    code.push(0x00);
    code.push(0x52); // MSTORE

    code.push(0x7f);
    code.extend_from_slice(other_recipient.into_word().as_slice()); // topic2 = other_recipient

    code.push(0x7f);
    code.extend_from_slice(caller.into_word().as_slice());

    code.push(0x7f);
    code.extend_from_slice(&flash_simulator::inspector::TRANSFER_TOPIC.0);

    code.push(0x60);
    code.push(0x20);
    code.push(0x60);
    code.push(0x00);
    code.push(0xa3); // LOG3
    code.push(0x00); // STOP

    let account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: FixedBytes::ZERO,
        code: Some(revm::bytecode::Bytecode::new_raw(Bytes::from(code))),
        account_id: None,
    };
    db.insert_account_info(token_address, account);

    let tx = TxEnv {
        caller,
        kind: TxKind::Call(token_address),
        gas_limit: 100_000,
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg: &mut revm::context::CfgEnv| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN)
        })
        .with_db(db);

    let inspector = HoneypotTxInspector::new(token_address, recipient);
    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let res = evm.inspect_tx(tx).unwrap();

    assert!(res.result.is_success());
    assert_eq!(evm.inspector.emitted_amount, U256::ZERO, "Transfer to other recipient must not be counted");
}

#[test]
fn test_vector3_08_evm_sstore_storage_interception() {
    let mut db = InMemoryDB::default();
    let token_address = Address::from([0xa1; 20]);
    let recipient = Address::from([0xb2; 20]);
    let caller = Address::from([0xc3; 20]);

    // Bytecode that executes SSTORE(slot = 0x42, val = 0x9999):
    // PUSH2 0x9999, PUSH1 0x42, SSTORE, STOP
    let bytecode = Bytes::from_static(&[
        0x61, 0x99, 0x99, // PUSH2 0x9999 (val)
        0x60, 0x42,       // PUSH1 0x42 (slot)
        0x55,             // SSTORE
        0x00,             // STOP
    ]);

    let account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: FixedBytes::ZERO,
        code: Some(revm::bytecode::Bytecode::new_raw(bytecode)),
        account_id: None,
    };
    db.insert_account_info(token_address, account);

    let tx = TxEnv {
        caller,
        kind: TxKind::Call(token_address),
        gas_limit: 100_000,
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg: &mut revm::context::CfgEnv| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN)
        })
        .with_db(db);

    let inspector = HoneypotTxInspector::new(token_address, recipient);
    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let res = evm.inspect_tx(tx).unwrap();

    assert!(res.result.is_success());
    let slot = U256::from(0x42);
    assert!(evm.inspector.storage_writes.contains_key(&slot), "SSTORE to slot 0x42 must be captured");
    let mutation = &evm.inspector.storage_writes[&slot];
    assert_eq!(mutation.slot, slot);
    assert_eq!(mutation.new_value, U256::from(0x9999));
}

#[test]
fn test_vector4_05_revert_blocked_keyword() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    let enc = encode_abi_error_string("TRANSFER_BLOCKED_FOR_TARGET");

    let mut db = InMemoryDB::default();
    let mut code = vec![];
    for (i, chunk) in enc.chunks(32).enumerate() {
        let mut padded = [0u8; 32];
        padded[..chunk.len()].copy_from_slice(chunk);
        code.push(0x7f);
        code.extend_from_slice(&padded);
        code.push(0x60);
        code.push((i * 32) as u8);
        code.push(0x52);
    }
    code.push(0x60);
    code.push(enc.len() as u8);
    code.push(0x60);
    code.push(0x00);
    code.push(0xfd);

    let account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: FixedBytes::ZERO,
        code: Some(revm::bytecode::Bytecode::new_raw(Bytes::from(code))),
        account_id: None,
    };
    db.insert_account_info(token, account);

    let tx = TxEnv {
        caller: recipient,
        kind: TxKind::Call(token),
        gas_limit: 100_000,
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg: &mut revm::context::CfgEnv| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN)
        })
        .with_db(db);

    let inspector = HoneypotTxInspector::new(token, recipient);
    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let _ = evm.inspect_tx(tx).unwrap();

    assert!(evm.inspector.reverted);
    assert_eq!(
        evm.inspector.violation,
        Some(HoneypotViolation::BlacklistedRecipient)
    );
}

#[test]
fn test_vector4_06_revert_non_blacklist_reason() {
    let token = Address::from([0x11; 20]);
    let recipient = Address::from([0x22; 20]);

    let enc = encode_abi_error_string("TRANSFER_FAILED");

    let mut db = InMemoryDB::default();
    let mut code = vec![];
    for (i, chunk) in enc.chunks(32).enumerate() {
        let mut padded = [0u8; 32];
        padded[..chunk.len()].copy_from_slice(chunk);
        code.push(0x7f);
        code.extend_from_slice(&padded);
        code.push(0x60);
        code.push((i * 32) as u8);
        code.push(0x52);
    }
    code.push(0x60);
    code.push(enc.len() as u8);
    code.push(0x60);
    code.push(0x00);
    code.push(0xfd);

    let account = AccountInfo {
        balance: U256::ZERO,
        nonce: 0,
        code_hash: FixedBytes::ZERO,
        code: Some(revm::bytecode::Bytecode::new_raw(Bytes::from(code))),
        account_id: None,
    };
    db.insert_account_info(token, account);

    let tx = TxEnv {
        caller: recipient,
        kind: TxKind::Call(token),
        gas_limit: 100_000,
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg: &mut revm::context::CfgEnv| {
            cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN)
        })
        .with_db(db);

    let inspector = HoneypotTxInspector::new(token, recipient);
    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let _ = evm.inspect_tx(tx).unwrap();

    assert!(evm.inspector.reverted);
    assert_eq!(
        evm.inspector.violation,
        Some(HoneypotViolation::ExecutionReverted {
            reason: "TRANSFER_FAILED".to_string()
        })
    );
}

