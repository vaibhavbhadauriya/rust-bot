//! Integration Test Suite for flash-executor crate
//! Tests all Milestone 3 components: Pure Yul calldata, fast-revert guard, vanity miner, flash loan routing, and embedded bytecode.

use alloy_primitives::{address, Address, B256, U256};
use flash_core::types::Chain;
use flash_executor::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn test_calldata_100byte_roundtrip_fidelity() {
    let params = ExecutionParams {
        weth_amount_to_first_market: 4_732_420_000,
        balance_before: 10_000_000_000,
        gas_cost: 250_000,
        target1: address!("1111111111111111111111111111111111111111"),
        target2: address!("2222222222222222222222222222222222222222"),
        token0_amount_out: 1_901_905_000_000_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: 4_777_570_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let encoded = encode_124byte_calldata(&params);
    assert_eq!(encoded.len(), 124);

    let decoded = decode_124byte_calldata(&encoded).expect("Decoding must succeed");
    assert_eq!(decoded, params);
}

#[test]
fn test_calldata_boundaries_and_revert() {
    // Under 124 bytes -> Error
    let short = [0u8; 123];
    assert!(decode_124byte_calldata(&short).is_err());

    // Exactly 124 zero bytes
    let zeros = [0u8; 124];
    let decoded_zeros = decode_124byte_calldata(&zeros).expect("All zeros should decode");
    assert_eq!(decoded_zeros.weth_amount_to_first_market, 0);
    assert_eq!(decoded_zeros.target1, Address::ZERO);

    // Exactly 124 0xFF bytes
    let ones = [0xFFu8; 124];
    let decoded_ones = decode_124byte_calldata(&ones).expect("All ones should decode");
    assert_eq!(decoded_ones.weth_amount_to_first_market, u128::MAX);
    assert_eq!(decoded_ones.gas_cost, u128::MAX);
    assert_eq!(decoded_ones.token0_amount_out, u128::MAX);
    assert_eq!(decoded_ones.token0_amount_out_index, u16::MAX);

    // Oversized calldata (130 bytes) decodes first 124 bytes
    let mut oversized = Vec::from(zeros);
    oversized.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
    assert_eq!(oversized.len(), 130);
    let decoded_over = decode_124byte_calldata(&oversized).expect("Oversized calldata should succeed");
    assert_eq!(decoded_over, decoded_zeros);
}

#[test]
fn test_callvalue_dispatch_routing() {
    assert_eq!(dispatch_action_by_callvalue(0).unwrap(), CallvalueAction::ArbitragePath);
    assert_eq!(dispatch_action_by_callvalue(1).unwrap(), CallvalueAction::OwnerErc20Sweep);
    assert_eq!(dispatch_action_by_callvalue(2).unwrap(), CallvalueAction::OwnerEthSweep);
    assert!(dispatch_action_by_callvalue(3).is_err());
    assert!(dispatch_action_by_callvalue(100).is_err());
}

#[test]
fn test_fast_revert_guard_behavior() {
    // Unchanged reserves -> Passed
    assert_eq!(simulate_fast_revert_guard(1000, 2000, 1000, 2000), GuardOutcome::Passed);

    // Competitor shifted reserves by even 1 wei -> FastRevert
    let outcome = simulate_fast_revert_guard(1000, 2000, 1001, 2000);
    match outcome {
        GuardOutcome::FastRevert { gas_used, gas_saved_percent } => {
            assert!(gas_used < 3000, "Fast-revert must consume < 3000 gas, got {}", gas_used);
            assert!(
                gas_saved_percent > 80.0,
                "Gas savings must exceed 80%, got {:.2}%",
                gas_saved_percent
            );
        }
        GuardOutcome::Passed => panic!("Expected FastRevert outcome"),
    }
}

#[test]
fn test_atomic_profit_enforcement() {
    // Profit exceeds gas cost -> Ok
    assert!(check_atomic_profit(100_000, 50_000).is_ok());

    // Profit exactly equals gas cost (breakeven) -> Ok
    assert!(check_atomic_profit(50_000, 50_000).is_ok());

    // Profit is 1 wei below gas cost -> Error
    assert!(check_atomic_profit(49_999, 50_000).is_err());

    // Zero profit with positive gas cost -> Error
    assert!(check_atomic_profit(0, 50_000).is_err());
}

#[test]
fn test_vanity_address_math_and_savings() {
    // 8 zero bytes (16 hex zeros) * 12 gas/byte = 96 gas savings
    assert_eq!(calculate_calldata_gas_savings(8), 96);
    assert_eq!(calculate_calldata_gas_savings(0), 0);
    assert_eq!(calculate_calldata_gas_savings(1), 12);
    assert_eq!(calculate_calldata_gas_savings(20), 240);

    // Verify address with 8 leading zero bytes
    let mut vanity_bytes = [0xAAu8; 20];
    vanity_bytes[0..8].fill(0);
    let vanity_addr = Address::from(vanity_bytes);
    assert_eq!(count_leading_zero_bytes(&vanity_addr), 8);
    assert_eq!(count_leading_hex_zeros(&vanity_addr), 16);
}

#[test]
fn test_vanity_miner_multithreaded_search() {
    let deployer = address!("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
    let init_code = b"PURE_YUL_EXECUTOR_INIT_CODE_PAYLOAD";

    let (salt, addr) = mine_vanity_salt(deployer, init_code, 2);
    assert_ne!(salt, B256::ZERO);
    assert!(count_leading_zero_bytes(&addr) >= 2);
    assert_eq!(compute_create2_address_from_code(deployer, salt.0, init_code), addr);
}

#[test]
fn test_vanity_miner_cancellation_graceful() {
    let deployer = address!("BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB");
    let init_code_hash = [0x55; 32];
    let token = Arc::new(AtomicBool::new(false));

    // Abort mining immediately
    token.store(true, Ordering::SeqCst);

    let miner = VanityMiner::new(deployer, init_code_hash)
        .with_target_zero_bytes(8)
        .with_threads(2)
        .with_batch_size(256)
        .with_cancellation_token(token);

    let res = miner.mine();
    assert_eq!(res.unwrap_err(), VanityMinerError::Cancelled);
}

#[test]
fn test_flash_loan_priority_ladder_and_callbacks() {
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 10_000_000,
        balancer_liquidity: 10_000_000,
        univ4_liquidity: 10_000_000,
        aave_liquidity: 10_000_000,
    };

    // Priority 1: Morpho Blue
    let (p1, f1) = route_flash_loan(5_000_000, &snapshot).unwrap();
    assert_eq!(p1, FlashLoanProvider::MorphoBlue);
    assert_eq!(f1, 0);
    assert_eq!(p1.callback_selector(), "0x3a48e7e1");

    // Priority 2: Balancer V3
    let snapshot_no_morpho = ProviderLiquiditySnapshot {
        morpho_liquidity: 1_000_000,
        balancer_liquidity: 10_000_000,
        univ4_liquidity: 10_000_000,
        aave_liquidity: 10_000_000,
    };
    let (p2, f2) = route_flash_loan(5_000_000, &snapshot_no_morpho).unwrap();
    assert_eq!(p2, FlashLoanProvider::BalancerV3);
    assert_eq!(f2, 0);
    assert_eq!(p2.callback_selector(), "0x98dd202a");

    // Priority 3: Uniswap V4
    let snapshot_no_bal = ProviderLiquiditySnapshot {
        morpho_liquidity: 1_000_000,
        balancer_liquidity: 1_000_000,
        univ4_liquidity: 10_000_000,
        aave_liquidity: 10_000_000,
    };
    let (p3, f3) = route_flash_loan(5_000_000, &snapshot_no_bal).unwrap();
    assert_eq!(p3, FlashLoanProvider::UniswapV4);
    assert_eq!(f3, 0);

    // Priority 4: Aave V3 fallback
    let snapshot_only_aave = ProviderLiquiditySnapshot {
        morpho_liquidity: 1_000_000,
        balancer_liquidity: 1_000_000,
        univ4_liquidity: 1_000_000,
        aave_liquidity: 10_000_000,
    };
    let (p4, f4) = route_flash_loan(5_000_000, &snapshot_only_aave).unwrap();
    assert_eq!(p4, FlashLoanProvider::AaveV3);
    assert_eq!(f4, 500); // 0.05%
    assert_eq!(p4.callback_selector(), "0x9293e4e7");

    // Chain-specific Aave fee
    assert_eq!(p4.protocol_fee_pips_for_chain(Chain::ArbitrumOne), 500);
    assert_eq!(p4.protocol_fee_pips_for_chain(Chain::Base), 900);

    // Out of liquidity
    let snapshot_dry = ProviderLiquiditySnapshot::default();
    assert!(route_flash_loan(5_000_000, &snapshot_dry).is_none());
}

#[test]
fn test_protocol_callback_encoders_and_decoders() {
    let params = ExecutionParams {
        weth_amount_to_first_market: 1_000,
        balance_before: 2_000,
        gas_cost: 50,
        target1: address!("1111111111111111111111111111111111111111"),
        target2: address!("2222222222222222222222222222222222222222"),
        token0_amount_out: 3_000,
        token0_amount_out_index: 0,
        token1_amount_out: 4_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };
    let encoded = encode_124byte_calldata(&params);

    // 1. Morpho Blue
    let morpho_cb = encode_morpho_callback(U256::from(10_000), &encoded);
    let (assets, m_params) = decode_morpho_callback(&morpho_cb).unwrap();
    assert_eq!(assets, U256::from(10_000));
    assert_eq!(m_params, params);

    // 2. Balancer V3 / Uniswap V4
    let bal_cb = encode_balancer_v3_callback(&encoded);
    let b_params = decode_unlock_callback(&bal_cb).unwrap();
    assert_eq!(b_params, params);

    // 3. Aave V3
    let aave_cb = encode_aave_v3_callback(
        address!("3333333333333333333333333333333333333333"),
        U256::from(50_000),
        U256::from(25),
        address!("4444444444444444444444444444444444444444"),
        &encoded,
    );
    let (asset, amount, premium, initiator, a_params) = decode_aave_v3_callback(&aave_cb).unwrap();
    assert_eq!(asset, address!("3333333333333333333333333333333333333333"));
    assert_eq!(amount, U256::from(50_000));
    assert_eq!(premium, U256::from(25));
    assert_eq!(initiator, address!("4444444444444444444444444444444444444444"));
    assert_eq!(a_params, params);
}

#[test]
fn test_embedded_bytecode_constants() {
    assert_eq!(EXECUTOR_INIT_CODE.len(), 1126);
    assert_eq!(EXECUTOR_RUNTIME_BYTECODE.len(), 1114);

    let bc = executor_runtime_bytecode();
    assert!(!bc.is_empty());
    assert_eq!(executor_init_code(), EXECUTOR_INIT_CODE);
    assert_eq!(executor_runtime_bytes(), EXECUTOR_RUNTIME_BYTECODE);
}
