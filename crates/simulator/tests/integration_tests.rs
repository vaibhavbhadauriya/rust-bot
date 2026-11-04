//! Integration tests for flash-simulator crate.
//! Validates embedded revm execution, two-tier bytecode caching, UniV3Quoter parity, and Honeypot defense.
//! Authoritative source: ORIGINAL_REQUEST.md lines 43-46, 89; PROJECT.md § Milestone 4.

use alloy_primitives::{address, Bytes, U256};
use flash_core::types::{Chain, Venue};
use flash_math::TradeSizeOutput;
use flash_scanner::dfs::RouteCandidate;
use flash_scanner::graph::DirectedEdge;
use flash_simulator::{
    cache::BytecodeCache,
    engine::SimulationEngine,
    honeypot::{
        inspect_honeypot_safety, is_canonical_safe_token, verify_token_integrity,
        HoneypotDetector, HoneypotSimulationReport,
    },
    inspector::HoneypotViolation,
    quoter::{
        execute_evm_quote, inject_pool_state, inject_quoter_bytecode,
        verify_quoter_parity, QuoteExactInputSingleParams, SimulatedPoolState,
        DEFAULT_QUOTER_ADDRESS,
    },
    types::{SimError, StateOverride},
};
use revm::database::in_memory_db::InMemoryDB;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("{}_{}", prefix, nanos))
}

#[test]
fn test_cache_memory_lru_and_disk_two_tier() {
    let temp_dir = unique_temp_dir("test_cacache_tier");
    let cache = BytecodeCache::new(&temp_dir, 2);

    let chain = Chain::ArbitrumOne;
    let addr1 = address!("1111111111111111111111111111111111111111");
    let addr2 = address!("2222222222222222222222222222222222222222");
    let addr3 = address!("3333333333333333333333333333333333333333");

    let code1 = Bytes::from_static(&[0x60, 0x01, 0x60, 0x02, 0x01]);
    let code2 = Bytes::from_static(&[0x60, 0x03, 0x60, 0x04, 0x02]);
    let code3 = Bytes::from_static(&[0x60, 0x05, 0x60, 0x06, 0x03]);

    // Put addr1 & addr2 into cache
    cache.put_bytecode(chain, addr1, code1.clone()).unwrap();
    cache.put_bytecode(chain, addr2, code2.clone()).unwrap();

    assert!(cache.contains_memory(chain, addr1));
    assert!(cache.contains_memory(chain, addr2));

    // Put addr3: causes LRU eviction of addr1 from hot memory (capacity = 2)
    cache.put_bytecode(chain, addr3, code3.clone()).unwrap();
    assert!(!cache.contains_memory(chain, addr1)); // evicted from memory
    assert!(cache.contains_memory(chain, addr2));
    assert!(cache.contains_memory(chain, addr3));

    // Read addr1: read-through from persistent cacache disk back into memory!
    let fetched1 = cache.get_bytecode(chain, addr1).expect("read from disk");
    assert_eq!(fetched1.original_bytes(), code1);
    assert!(cache.contains_memory(chain, addr1), "promoted to hot memory");

    // Clean up
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_cache_key_formatting_canonical() {
    let key_arb = BytecodeCache::format_key(42161, address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1"));
    assert_eq!(key_arb, "42161:0x82aF49447D8a07e3bd95BD0d56f35241523fBab1");

    let key_base = BytecodeCache::format_key(8453, address!("4200000000000000000000000000000000000006"));
    assert_eq!(key_base, "8453:0x4200000000000000000000000000000000000006");
}

#[test]
fn test_univ3_quoter_evm_execution_and_numerical_parity() {
    let mut db = InMemoryDB::default();
    let quoter_addr = DEFAULT_QUOTER_ADDRESS;
    inject_quoter_bytecode(&mut db, quoter_addr);

    let token0 = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1"); // WETH
    let token1 = address!("af88d065e77c8cC2239327C5EDb3A432268e5831"); // USDC
    let fee_pips = 500u32; // 0.05%

    // sqrtPriceX96 for ~2495 USDC per ETH
    let sqrt_price_x96 = U256::from(3957444141979261350849182367744u128);
    let liquidity = 15_000_000_000_000_000_000u128; // 15 ETH liquidity scale

    let pool = SimulatedPoolState::new(
        address!("3333333333333333333333333333333333333333"),
        token0,
        token1,
        fee_pips,
        sqrt_price_x96,
        liquidity,
        0,
        60,
    );

    inject_pool_state(&mut db, quoter_addr, &pool).unwrap();

    let params = QuoteExactInputSingleParams {
        token_in: token0,
        token_out: token1,
        fee: fee_pips,
        amount_in: U256::from(1_000_000_000_000_000_000u128), // 1 ETH in
        sqrt_price_limit_x96: U256::ZERO,
    };

    // Parity verification between EVM bytecode and Rust analytical math
    verify_quoter_parity(&mut db, quoter_addr, &pool, &params).expect("parity check passes <= 1 wei");

    let quote_res = execute_evm_quote(&mut db, quoter_addr, &params).unwrap();
    assert!(quote_res.amount_out > U256::ZERO);
    assert!(quote_res.sqrt_price_x96_after > U256::ZERO);
    assert_eq!(quote_res.initialized_ticks_crossed, 0);
    assert_eq!(quote_res.gas_estimate, U256::from(105000));
}

#[test]
fn test_honeypot_inspector_catches_salmonella_and_traps() {
    // 1. Salmonella partial delivery check (emits 100, delivers 10)
    let report_salmonella = HoneypotSimulationReport {
        token: address!("6666666666666666666666666666666666666666"),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(10),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert_eq!(
        inspect_honeypot_safety(&report_salmonella),
        Err(HoneypotViolation::SalmonellaPartialDelivery {
            emitted: U256::from(100),
            delivered: U256::from(10)
        })
    );

    // 2. Honest token passes
    let report_honest = HoneypotSimulationReport {
        token: address!("7777777777777777777777777777777777777777"),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(100),
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    assert!(inspect_honeypot_safety(&report_honest).is_ok());

    // 3. ORIGIN opcode sandwich trap
    let report_origin = HoneypotSimulationReport {
        token: address!("8888888888888888888888888888888888888888"),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(100),
        used_origin_opcode: true,
        used_gasprice_opcode: false,
    };
    assert_eq!(
        inspect_honeypot_safety(&report_origin),
        Err(HoneypotViolation::OriginSandwichTrap)
    );

    // 4. GASPRICE opcode sandwich trap
    let report_gasprice = HoneypotSimulationReport {
        token: address!("9999999999999999999999999999999999999999"),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(100),
        used_origin_opcode: false,
        used_gasprice_opcode: true,
    };
    assert_eq!(
        inspect_honeypot_safety(&report_gasprice),
        Err(HoneypotViolation::GasPriceSandwichTrap)
    );

    // 5. Listeria excessive fee (> 10%)
    let report_fee = HoneypotSimulationReport {
        token: address!("5555555555555555555555555555555555555555"),
        emitted_amount: U256::from(100),
        storage_delta: U256::from(85), // 15% tax
        used_origin_opcode: false,
        used_gasprice_opcode: false,
    };
    match inspect_honeypot_safety(&report_fee) {
        Err(HoneypotViolation::ExcessiveFeeOnTransfer { fee_percent }) => {
            assert!((fee_percent - 15.0).abs() < 1e-4);
        }
        other => panic!("Expected ExcessiveFeeOnTransfer, got {:?}", other),
    }
}

#[test]
fn test_canonical_tokens_fast_pre_whitelist() {
    assert!(is_canonical_safe_token(flash_core::constants::arbitrum::WETH));
    assert!(is_canonical_safe_token(flash_core::constants::arbitrum::USDC_NATIVE));
    assert!(is_canonical_safe_token(flash_core::constants::arbitrum::ARB));
    assert!(is_canonical_safe_token(flash_core::constants::base::WETH));
    assert!(is_canonical_safe_token(flash_core::constants::base::USDC_NATIVE));
    assert!(is_canonical_safe_token(flash_core::constants::base::AERO));

    let unknown_token = address!("1234567890123456789012345678901234567890");
    assert!(!is_canonical_safe_token(unknown_token));

    let detector = HoneypotDetector::new();
    assert!(detector.verify_token_integrity(flash_core::constants::arbitrum::WETH).is_ok());
    assert!(verify_token_integrity(flash_core::constants::arbitrum::USDC_NATIVE).is_ok());
}

#[test]
fn test_simulation_engine_state_isolation() {
    let temp_dir = unique_temp_dir("test_cacache_iso");
    let cache = Arc::new(BytecodeCache::new(&temp_dir, 100));
    let engine = SimulationEngine::new(Chain::ArbitrumOne, cache);

    // Verify initial base_db has executor and weth
    let mut db_clone = engine.base_db.clone();
    let account = db_clone.load_account(engine.executor_address).unwrap();
    assert!(account.info.code.is_some());

    // Apply custom state override to isolated DB
    let override_addr = address!("4444444444444444444444444444444444444444");
    let ovr = StateOverride::new(override_addr)
        .with_balance(U256::from(500))
        .with_storage_slot(U256::from(1), U256::from(999));

    engine.apply_state_overrides(&mut db_clone, &[ovr]);

    // Verify overridden state in db_clone
    let ovr_account = db_clone.load_account(override_addr).unwrap();
    assert_eq!(ovr_account.info.balance, U256::from(500));

    // Verify pristine isolation in base_db (NOT modified!)
    let mut pristine_clone = engine.base_db.clone();
    let pristine_account = pristine_clone.load_account(override_addr).unwrap();
    assert_eq!(pristine_account.info.balance, U256::ZERO);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_simulation_engine_invalid_route_rejection() {
    let temp_dir = unique_temp_dir("test_cacache_invalid");
    let cache = Arc::new(BytecodeCache::new(&temp_dir, 100));
    let engine = SimulationEngine::new(Chain::ArbitrumOne, cache);

    // Route with < 2 hops should be rejected
    let single_hop_route = RouteCandidate {
        borrow_token: flash_core::constants::arbitrum::WETH,
        hops: vec![DirectedEdge {
            pool_id: address!("1111111111111111111111111111111111111111"),
            venue: Venue::UniswapV2,
            token_in: flash_core::constants::arbitrum::WETH,
            token_out: address!("2222222222222222222222222222222222222222"),
            reserve_in: 1_000_000,
            reserve_out: 2_000_000,
            fee_pips: 3000,
            zero_for_one: true,
        }],
        sizing: TradeSizeOutput {
            optimal_input: U256::from(100),
            expected_intermediate: U256::from(200),
            expected_output: U256::from(150),
            gross_profit: U256::from(50),
            flash_fee: U256::ZERO,
            net_profit: U256::from(50),
        },
        net_profit: U256::from(50),
    };

    let result = engine.simulate_arbitrage_route(&single_hop_route, U256::from(100));
    assert_eq!(
        result,
        Err(SimError::InvalidRoute("Route must have >= 2 hops".into()))
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}
