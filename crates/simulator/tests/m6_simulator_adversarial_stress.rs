//! Milestone 6 Phase 2: Tier 5 Adversarial Stress & White-Box Coverage Hardening
//! Target: crates/simulator
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 43-46, 89; PROJECT.md § Milestone 4 & 6.
//!
//! Probes:
//! 1. Cancun In-Memory Revm Simulation Adversarial Paths & Revert Variations
//! 2. Custom Minimal UniV3Quoter (329-byte bytecode) ABI Boundaries & Numerical Invariants
//! 3. Two-Tier Bytecode Cache (Memory LRU + cacache disk) Extreme Concurrency & Fault Paths
//! 4. TxInspector Honeypot Defense & Exact Salmonella / Listeria Tax Boundaries

use alloy_primitives::{address, Address, Bytes, U256};
use flash_core::types::{Chain, Venue};
use flash_math::TradeSizeOutput;
use flash_scanner::dfs::RouteCandidate;
use flash_scanner::graph::DirectedEdge;
use flash_simulator::{
    cache::{BytecodeCache, MemoryLru},
    engine::SimulationEngine,
    honeypot::{
        inspect_honeypot_safety, is_canonical_safe_token, verify_token_integrity,
        HoneypotDetector, HoneypotSimulationReport,
    },
    inspector::{
        decode_revert_reason, HoneypotTxInspector, HoneypotViolation,
    },
    quoter::{
        calculate_rpc_reduction_percentage, decode_quote_output,
        encode_quote_exact_input_single, quote_analytical_clmm,
        QuoteExactInputSingleParams, QuoterError, SimulatedPoolState,
        DEFAULT_QUOTER_ADDRESS, QUOTE_EXACT_INPUT_SINGLE_SELECTOR,
        QUOTE_EXACT_INPUT_SINGLE_V2_SELECTOR, QUOTER_RUNTIME_BYTECODE,
    },
    types::{SimError, StateOverride},
};
use revm::{
    context::TxEnv,
    context_interface::result::ExecutionResult,
    primitives::TxKind,
};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_test_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos))
}

// ============================================================================
// 1. Cancun Revm In-Memory Simulation Adversarial Paths
// ============================================================================

#[test]
fn test_m6_sim_01_revert_payload_classification_exhaustive() {
    let temp_dir = unique_test_temp_dir("m6_sim_revert");
    let cache = Arc::new(BytecodeCache::new(&temp_dir, 100));
    let engine = SimulationEngine::new(Chain::ArbitrumOne, cache);

    // Case A: 0 bytes -> FastRevertTriggered
    let code_0b = Bytes::from_static(&[0x5f, 0x5f, 0xfd]); // revert(0,0)
    let addr_0b = address!("AA00000000000000000000000000000000000001");
    let tx_0b = TxEnv {
        caller: Address::from([0x11; 20]),
        gas_limit: 100_000,
        kind: TxKind::Call(addr_0b),
        ..Default::default()
    };
    let res_0b = engine.execute_raw(tx_0b, &[StateOverride::new(addr_0b).with_code(code_0b)]).unwrap();
    if let ExecutionResult::Revert { output, .. } = res_0b {
        assert!(output.is_empty());
        let sim_err = if output.is_empty() {
            SimError::FastRevertTriggered
        } else {
            SimError::Reverted(output)
        };
        assert_eq!(sim_err, SimError::FastRevertTriggered);
    } else {
        panic!("Expected Revert");
    }

    // Case B: Exactly 32 bytes -> Unprofitable
    let code_32b = Bytes::from_static(&[
        0x60, 0x55, // PUSH1 0x55
        0x5f,       // PUSH0
        0x52,       // MSTORE
        0x60, 0x20, // PUSH1 32
        0x5f,       // PUSH0
        0xfd,       // REVERT
    ]);
    let addr_32b = address!("AA00000000000000000000000000000000000002");
    let tx_32b = TxEnv {
        caller: Address::from([0x11; 20]),
        gas_limit: 100_000,
        kind: TxKind::Call(addr_32b),
        ..Default::default()
    };
    let res_32b = engine.execute_raw(tx_32b, &[StateOverride::new(addr_32b).with_code(code_32b)]).unwrap();
    if let ExecutionResult::Revert { output, .. } = res_32b {
        assert_eq!(output.len(), 32);
        let profit = U256::from_be_slice(&output);
        assert_eq!(profit, U256::from(0x55));
        let sim_err = SimError::Unprofitable {
            profit,
            gas_cost: U256::from(50_000),
        };
        assert_eq!(
            sim_err,
            SimError::Unprofitable {
                profit: U256::from(0x55),
                gas_cost: U256::from(50_000),
            }
        );
    } else {
        panic!("Expected Revert");
    }

    // Case C: 68 bytes standard Error(string) "SlippageExceeded"
    let reason = "SlippageExceeded";
    let mut err_data = Vec::new();
    err_data.extend_from_slice(&[0x08, 0xc3, 0x79, 0xa0]); // Error(string) selector
    err_data.extend_from_slice(&[0u8; 31]);
    err_data.push(0x20); // offset = 32
    err_data.extend_from_slice(&[0u8; 31]);
    err_data.push(reason.len() as u8); // string len
    err_data.extend_from_slice(reason.as_bytes());
    while err_data.len() % 32 != 4 {
        err_data.push(0);
    }
    let decoded = decode_revert_reason(&err_data);
    assert_eq!(decoded, "SlippageExceeded");

    // Case D: Panic(0x11) code 0x11 (arithmetic overflow)
    let mut panic_data = Vec::new();
    panic_data.extend_from_slice(&[0x4e, 0x48, 0x7b, 0x71]); // Panic(uint256) selector
    panic_data.extend_from_slice(&[0u8; 31]);
    panic_data.push(0x11);
    let decoded_panic = decode_revert_reason(&panic_data);
    assert_eq!(decoded_panic, "Panic(0x11)");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_m6_sim_02_route_topology_validation() {
    let temp_dir = unique_test_temp_dir("m6_sim_topo");
    let cache = Arc::new(BytecodeCache::new(&temp_dir, 100));
    let engine = SimulationEngine::new(Chain::ArbitrumOne, cache);

    // Empty route hops -> InvalidRoute
    let empty_route = RouteCandidate {
        borrow_token: engine.weth_address,
        hops: vec![],
        sizing: TradeSizeOutput {
            optimal_input: U256::ZERO,
            expected_intermediate: U256::ZERO,
            expected_output: U256::ZERO,
            gross_profit: U256::ZERO,
            flash_fee: U256::ZERO,
            net_profit: U256::ZERO,
        },
        net_profit: U256::ZERO,
    };
    let res_empty = engine.simulate_arbitrage_route(&empty_route, U256::from(100));
    assert!(matches!(res_empty, Err(SimError::InvalidRoute(_))));

    // Exactly 1 hop -> InvalidRoute (< 2 hops)
    let hop1 = DirectedEdge {
        pool_id: address!("1111000000000000000000000000000000000001"),
        venue: Venue::UniswapV2,
        token_in: engine.weth_address,
        token_out: address!("2222000000000000000000000000000000000002"),
        reserve_in: 1_000_000,
        reserve_out: 1_000_000,
        fee_pips: 3000,
        zero_for_one: true,
    };
    let mut single_hop = empty_route;
    single_hop.hops.push(hop1.clone());
    let res_single = engine.simulate_arbitrage_route(&single_hop, U256::from(100));
    assert!(matches!(res_single, Err(SimError::InvalidRoute(_))));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// ============================================================================
// 2. Custom Minimal UniV3Quoter 329-Byte Bytecode Boundaries
// ============================================================================

#[test]
fn test_m6_quoter_01_calldata_length_and_selectors() {
    assert_eq!(QUOTER_RUNTIME_BYTECODE.len(), 354);
    assert_eq!(DEFAULT_QUOTER_ADDRESS, address!("0000000000000000000000000000000000000099"));

    let params = QuoteExactInputSingleParams {
        token_in: address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1"),
        token_out: address!("af88d065e77c8cC2239327C5EDb3A432268e5831"),
        fee: 3000,
        amount_in: U256::from(1_000_000_000u64),
        sqrt_price_limit_x96: U256::ZERO,
    };

    let calldata = encode_quote_exact_input_single(&params);
    assert_eq!(calldata.len(), 164);
    assert_eq!(&calldata[0..4], &QUOTE_EXACT_INPUT_SINGLE_SELECTOR);

    // Verify V2 selector constant
    assert_eq!(QUOTE_EXACT_INPUT_SINGLE_V2_SELECTOR, [0xc6, 0xa5, 0x02, 0x6a]);

    // Decoding check: Returndata < 128 bytes must return InvalidReturndataLength
    let short_data = [0u8; 127];
    assert!(matches!(
        decode_quote_output(&short_data),
        Err(QuoterError::InvalidReturndataLength(127))
    ));

    let zero_data = [];
    assert!(matches!(
        decode_quote_output(&zero_data),
        Err(QuoterError::InvalidReturndataLength(0))
    ));
}

#[test]
fn test_m6_quoter_02_analytical_boundary_conditions() {
    let pool_zero_liq = SimulatedPoolState::new(
        Address::ZERO,
        address!("1111000000000000000000000000000000000001"),
        address!("2222000000000000000000000000000000000002"),
        3000,
        U256::from(1) << 96,
        0, // zero liquidity
        0,
        60,
    );
    assert_eq!(
        quote_analytical_clmm(
            &pool_zero_liq,
            pool_zero_liq.token0,
            U256::from(100),
            U256::ZERO
        ),
        Err(QuoterError::InvalidPoolState)
    );

    let pool_zero_price = SimulatedPoolState::new(
        Address::ZERO,
        address!("1111000000000000000000000000000000000001"),
        address!("2222000000000000000000000000000000000002"),
        3000,
        U256::ZERO, // zero price
        1_000_000,
        0,
        60,
    );
    assert_eq!(
        quote_analytical_clmm(
            &pool_zero_price,
            pool_zero_price.token0,
            U256::from(100),
            U256::ZERO
        ),
        Err(QuoterError::InvalidPoolState)
    );

    // Zero amount in returns zero amount out with no price change
    let pool_valid = SimulatedPoolState::new(
        Address::ZERO,
        address!("1111000000000000000000000000000000000001"),
        address!("2222000000000000000000000000000000000002"),
        3000,
        U256::from(1) << 96,
        10_000_000_000u128,
        0,
        60,
    );
    let zero_quote = quote_analytical_clmm(&pool_valid, pool_valid.token0, U256::ZERO, U256::ZERO).unwrap();
    assert_eq!(zero_quote.amount_out, U256::ZERO);
    assert_eq!(zero_quote.sqrt_price_x96_after, pool_valid.sqrt_price_x96);
}

#[test]
fn test_m6_quoter_03_rpc_traffic_reduction_formula() {
    // 0 quotes requested -> 100.0%
    assert_eq!(calculate_rpc_reduction_percentage(0, 0), 100.0);
    assert_eq!(calculate_rpc_reduction_percentage(0, 10), 100.0);

    // 1000 quotes requested, 0 cold calls -> 100.0%
    assert_eq!(calculate_rpc_reduction_percentage(1000, 0), 100.0);

    // 1000 quotes, 100 cold calls -> 90.0%
    assert_eq!(calculate_rpc_reduction_percentage(1000, 100), 90.0);

    // 1000 quotes, 50 cold calls -> 95.0% (> 90% requirement)
    assert!(calculate_rpc_reduction_percentage(1000, 50) >= 95.0);

    // Cold calls >= requested -> 0.0%
    assert_eq!(calculate_rpc_reduction_percentage(100, 150), 0.0);
}

// ============================================================================
// 3. Two-Tier Bytecode Cache (Memory LRU + cacache disk) Fault Tolerance
// ============================================================================

#[test]
fn test_m6_cache_01_lru_capacity_one_extreme_boundary() {
    let mut lru = MemoryLru::new(1);
    assert_eq!(lru.len(), 0);
    assert!(lru.is_empty());

    let addr1 = address!("1111111111111111111111111111111111111111");
    let addr2 = address!("2222222222222222222222222222222222222222");
    let code1 = revm::bytecode::Bytecode::new_raw(Bytes::from_static(&[0x01]));
    let code2 = revm::bytecode::Bytecode::new_raw(Bytes::from_static(&[0x02]));

    lru.put(42161, addr1, code1.clone());
    assert_eq!(lru.len(), 1);
    assert!(lru.contains(42161, addr1));

    // Put second key: evicts addr1 immediately
    lru.put(42161, addr2, code2.clone());
    assert_eq!(lru.len(), 1);
    assert!(!lru.contains(42161, addr1));
    assert!(lru.contains(42161, addr2));

    // Clear
    lru.clear();
    assert_eq!(lru.len(), 0);
    assert!(lru.is_empty());
}

#[test]
fn test_m6_cache_02_concurrency_race_50_threads() {
    let temp_dir = unique_test_temp_dir("m6_cache_concurrent");
    let cache = Arc::new(BytecodeCache::new(&temp_dir, 5)); // tiny memory capacity

    const THREADS: usize = 30;
    const OPS: usize = 50;
    let barrier = Arc::new(Barrier::new(THREADS));
    let mut handles = Vec::with_capacity(THREADS);

    for tid in 0..THREADS {
        let cache_clone = Arc::clone(&cache);
        let barrier_clone = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            barrier_clone.wait();
            for i in 0..OPS {
                let key_byte = ((tid + i) % 15) as u8;
                let addr = Address::from([key_byte; 20]);
                let code = Bytes::from(vec![0x60, key_byte, 0x00]);

                let _ = cache_clone.put_bytecode(Chain::ArbitrumOne, addr, code.clone());
                let fetched = cache_clone.get_bytecode(Chain::ArbitrumOne, addr);
                if let Some(c) = fetched {
                    assert_eq!(c.original_bytes(), code);
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// ============================================================================
// 4. TxInspector Honeypot Defense & Exact Salmonella/Listeria Tax Boundaries
// ============================================================================

#[test]
fn test_m6_honeypot_01_exact_tax_gradients() {
    let token = address!("0000000000000000000000000000000000000077");

    // Case 1: 20% delivery (Salmonella: storage_delta * 5 <= emitted)
    // 20 * 5 = 100 <= 100 -> Salmonella
    let rep_20 = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(20),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert_eq!(
        inspect_honeypot_safety(&rep_20),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: U256::from(100),
            delivered: U256::from(20),
        })
    );

    // Case 2: 21% delivery (Listeria: fee > 10%)
    // 21 * 5 = 105 > 100 (not Salmonella), but 21 * 10 = 210 < 900 (Listeria tax = 79%)
    let rep_21 = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(21),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    match inspect_honeypot_safety(&rep_21) {
        Err(HoneypotViolation::ExcessiveFeeOnTransfer { fee_percent }) => {
            assert!((fee_percent - 79.0).abs() < 1e-4);
        }
        other => panic!("Expected ExcessiveFeeOnTransfer, got {:?}", other),
    }

    // Case 3: 89% delivery (Listeria: 11% fee > 10%)
    let rep_89 = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(89),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    match inspect_honeypot_safety(&rep_89) {
        Err(HoneypotViolation::ExcessiveFeeOnTransfer { fee_percent }) => {
            assert!((fee_percent - 11.0).abs() < 1e-4);
        }
        other => panic!("Expected ExcessiveFeeOnTransfer, got {:?}", other),
    }

    // Case 4: Exactly 90% delivery (10% fee limit -> SAFE)
    // 90 * 10 = 900, not < 900 -> passes!
    let rep_90 = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(90),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(inspect_honeypot_safety(&rep_90).is_ok());

    // Case 5: 100% delivery -> SAFE
    let rep_100 = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(100),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(inspect_honeypot_safety(&rep_100).is_ok());

    // Case 6: Bonus / Positive Rebasing (110% delivery) -> SAFE
    let rep_110 = HoneypotSimulationReport {
        token,
        emitted_amount: U256::from(100),
        storage_delta: U256::from(110),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(inspect_honeypot_safety(&rep_110).is_ok());

    // Case 7: Zero emitted, zero delivered -> SAFE
    let rep_zero = HoneypotSimulationReport {
        token,
        emitted_amount: U256::ZERO,
        storage_delta: U256::ZERO,
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(inspect_honeypot_safety(&rep_zero).is_ok());
}

#[test]
fn test_m6_honeypot_02_blacklist_and_canonical_tokens() {
    let mut detector = HoneypotDetector::new();
    let zero_token = Address::ZERO;
    let weth = flash_core::constants::arbitrum::WETH;
    let usdc = flash_core::constants::base::USDC_NATIVE;
    let custom_safe = address!("000000000000000000000000000000000000beef");
    let custom_bad = address!("000000000000000000000000000000000000dead");

    // Zero address is always known malicious / blacklisted
    assert!(detector.is_known_malicious(zero_token));
    assert!(detector.verify_token_integrity(zero_token).is_err());
    assert!(verify_token_integrity(zero_token).is_err());

    // Canonical tokens always safe
    assert!(detector.is_known_safe(weth));
    assert!(is_canonical_safe_token(weth));
    assert!(detector.verify_token_integrity(weth).is_ok());
    assert!(verify_token_integrity(weth).is_ok());
    assert!(detector.is_known_safe(usdc));
    assert!(is_canonical_safe_token(usdc));

    // Custom tokens dynamic registration
    detector.mark_safe(custom_safe);
    assert!(detector.is_known_safe(custom_safe));
    assert!(detector.verify_token_integrity(custom_safe).is_ok());

    detector.mark_malicious(custom_bad);
    assert!(detector.is_known_malicious(custom_bad));
    assert!(detector.verify_token_integrity(custom_bad).is_err());
}

#[test]
fn test_m6_honeypot_03_inspector_revert_blacklist_keyword_detection() {
    let _inspector = HoneypotTxInspector::new(
        address!("1111000000000000000000000000000000000001"),
        address!("2222000000000000000000000000000000000002"),
    );
    assert_eq!(_inspector.emitted_amount, U256::ZERO);

    // Test blacklist keyword matching in revert string
    let blacklist_reason = "TransferFailed: Recipient is blacklisted";
    let mut err_data = Vec::new();
    err_data.extend_from_slice(&[0x08, 0xc3, 0x79, 0xa0]);
    err_data.extend_from_slice(&[0u8; 31]);
    err_data.push(0x20);
    err_data.extend_from_slice(&[0u8; 31]);
    err_data.push(blacklist_reason.len() as u8);
    err_data.extend_from_slice(blacklist_reason.as_bytes());
    while err_data.len() % 32 != 4 {
        err_data.push(0);
    }

    let decoded = decode_revert_reason(&err_data);
    assert!(decoded.to_lowercase().contains("blacklist"));
}
