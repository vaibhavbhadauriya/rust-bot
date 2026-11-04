//! Challenger Empirical Test for L8: UniV3Quoter shift guard & overflow behaviors.
//! Written by m_r2_challenger_2.

use alloy_primitives::{address, U256};
use flash_simulator::quoter::{
    execute_evm_quote, inject_quoter_bytecode, QuoteExactInputSingleParams, SimulatedPoolState,
    DEFAULT_QUOTER_ADDRESS, QUOTER_RUNTIME_BYTECODE,
};
use revm::database::in_memory_db::InMemoryDB;
use std::collections::HashMap;

#[test]
fn test_l8_quoter_bytecode_inspection() {
    // Check whether the compiled quoter bytecode contains the shift guard:
    // In UniV3Quoter.yul:
    //   if gt(shr(160, liquidity), 0) { revert(0, 0) }
    // In EVM opcodes:
    //   PUSH1 0xa0 (0x60, 0xa0) -> SHR (0x1c) -> GT (0x11) / ISZERO (0x15) -> REVERT (0xfd)
    let shr_160 = [0x60, 0xa0, 0x1c];
    let has_shr_160 = QUOTER_RUNTIME_BYTECODE
        .windows(3)
        .any(|w| w == shr_160);

    println!(
        "QUOTER_RUNTIME_BYTECODE (len={}) has shr(160, ...): {}",
        QUOTER_RUNTIME_BYTECODE.len(),
        has_shr_160
    );

    assert_eq!(QUOTER_RUNTIME_BYTECODE.len(), 354);
    assert!(has_shr_160, "Compiled QUOTER_RUNTIME_BYTECODE must contain shr(160, ...) guard!");
}

#[test]
fn test_l8_evm_execution_with_high_bits_in_price_and_limit() {
    let mut db = InMemoryDB::default();
    inject_quoter_bytecode(&mut db, DEFAULT_QUOTER_ADDRESS);

    let token_in = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
    let token_out = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");

    // Standard valid 160-bit sqrtPriceX96 (e.g. ~2500 USDC per ETH)
    let valid_sqrt_price = U256::from(39614081257132168796771975168u128); // Q64.96 for ~2500
    let valid_liquidity = 15_000_000_000_000_000_000u128;

    // Normal quote works
    let pool_normal = SimulatedPoolState {
        pool_id: address!("0000000000000000000000000000000000000001"),
        token0: token_in,
        token1: token_out,
        fee_pips: 500,
        sqrt_price_x96: valid_sqrt_price,
        liquidity: valid_liquidity,
        tick: 200000,
        tick_spacing: 10,
        tick_bitmap: HashMap::new(),
    };

    flash_simulator::quoter::inject_pool_state(&mut db, DEFAULT_QUOTER_ADDRESS, &pool_normal)
        .unwrap();

    let params_normal = QuoteExactInputSingleParams {
        token_in,
        token_out,
        fee: 500,
        amount_in: U256::from(1_000_000_000_000_000_000u128),
        sqrt_price_limit_x96: U256::ZERO,
    };

    let res = execute_evm_quote(&mut db, DEFAULT_QUOTER_ADDRESS, &params_normal);
    assert!(res.is_ok(), "Normal quote must succeed");

    // Test with sqrtPriceLimitX96 having high bits (> 160 bits)
    let high_bit_limit = (U256::from(1) << 165) | valid_sqrt_price;
    let params_high_limit = QuoteExactInputSingleParams {
        token_in,
        token_out,
        fee: 500,
        amount_in: U256::from(1_000_000_000_000_000_000u128),
        sqrt_price_limit_x96: high_bit_limit,
    };

    let res_limit = execute_evm_quote(&mut db, DEFAULT_QUOTER_ADDRESS, &params_high_limit);
    println!("Quote with >160 bit sqrt_price_limit_x96 result: {:?}", res_limit);
    assert!(res_limit.is_ok());

    // Test with liquidity having high bits (> 160 bits)
    // Note that SimulatedPoolState.liquidity is u128, which is at most 128 bits.
    // But direct EVM storage slot 1 could hold a U256 > 160 bits:
    let slot1_high = U256::from(1) << 170;
    db.insert_account_storage(DEFAULT_QUOTER_ADDRESS, U256::from(1), slot1_high).unwrap();
    let res_high_liq = execute_evm_quote(&mut db, DEFAULT_QUOTER_ADDRESS, &params_normal);
    println!("Quote with >160 bit liquidity in slot 1 result: {:?}", res_high_liq);
}
