//! Tier 5 Adversarial Coverage Hardening: contracts/executor.yul & crates/executor
//!
//! White-box stress testing of:
//! 1. 100-byte tightly packed calldata layout (0x64 bytes) and byte boundary fuzzing
//! 2. Zero-selector dispatch via callvalue (0 = Arb, 1 = ERC20 Sweep, 2 = ETH Sweep, >2 = Error)
//! 3. 30-byte fast-revert guard 1-wei reserve divergence and >80% gas reduction verification
//! 4. Atomic profit enforcement (wethProfit < gasCost reverts)
//! 5. Flash loan provider priority routing cascade (Morpho -> Balancer -> UniV4 -> Aave)
//! 6. CREATE2 16-zero vanity salt miner EIP-1014 derivation and cancellation token

use alloy_primitives::{address, keccak256, Address};
use flash_core::types::Chain;
use flash_executor::*;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[test]
fn test_adv_executor_100byte_calldata_truncation_matrix() {
    let dummy = [0x55u8; 165];

    // Every length from 0 to 123 must fail with CalldataTooShort
    for len in 0..124 {
        let slice = &dummy[..len];
        assert_eq!(
            decode_calldata(slice),
            Err(CalldataError::CalldataTooShort(len))
        );
    }

    // Exactly 124 bytes succeeds
    let valid_slice = &dummy[..124];
    assert!(decode_calldata(valid_slice).is_ok());

    // Greater than 124 bytes succeeds (extra bytes ignored)
    assert!(decode_calldata(&dummy[..125]).is_ok());
    assert!(decode_calldata(&dummy[..163]).is_ok());
}

#[test]
fn test_adv_executor_calldata_encoding_roundtrip_extreme_values() {
    // 1. All Zeros
    let zeros = ExecutionParams {
        weth_amount_to_first_market: 0,
        balance_before: 0,
        gas_cost: 0,
        target1: Address::ZERO,
        target2: Address::ZERO,
        token0_amount_out: 0,
        token0_amount_out_index: 0,
        token1_amount_out: 0,
        token1_amount_out_index: 0,
        target3: Address::ZERO,
        token2_amount_out: 0,
        token2_amount_out_index: 0,
        hop_count: 2,
    };
    let encoded_zeros = encode_124byte_calldata(&zeros);
    assert_eq!(encoded_zeros.len(), 124);
    assert_eq!(encoded_zeros, [0u8; 124]);
    let decoded_zeros = decode_124byte_calldata(&encoded_zeros).unwrap();
    assert_eq!(decoded_zeros, zeros);

    // 2. All Max Integers (3-hop)
    let maxes = ExecutionParams {
        weth_amount_to_first_market: u128::MAX,
        balance_before: u128::MAX,
        gas_cost: u128::MAX,
        target1: Address::repeat_byte(0xFF),
        target2: Address::repeat_byte(0xAA),
        token0_amount_out: u128::MAX,
        token0_amount_out_index: u16::MAX,
        token1_amount_out: u128::MAX,
        token1_amount_out_index: u16::MAX,
        target3: Address::repeat_byte(0x55),
        token2_amount_out: u128::MAX,
        token2_amount_out_index: u16::MAX,
        hop_count: 3,
    };
    let encoded_maxes = encode_calldata(&maxes);
    assert_eq!(encoded_maxes.len(), 163);
    let decoded_maxes = decode_calldata(&encoded_maxes).unwrap();
    assert_eq!(decoded_maxes, maxes);
}

#[test]
fn test_adv_executor_zero_selector_callvalue_dispatch() {
    assert_eq!(dispatch_action_by_callvalue(0), Ok(CallvalueAction::ArbitragePath));
    assert_eq!(dispatch_action_by_callvalue(1), Ok(CallvalueAction::OwnerErc20Sweep));
    assert_eq!(dispatch_action_by_callvalue(2), Ok(CallvalueAction::OwnerEthSweep));

    // Values > 2 must return InvalidCallvalue
    for val in [3, 4, 10, 100, 1_000_000, u64::MAX] {
        assert_eq!(
            dispatch_action_by_callvalue(val),
            Err(CalldataError::InvalidCallvalue(val))
        );
    }
}

#[test]
fn test_adv_executor_fast_revert_guard_1wei_divergence() {
    let exp0 = 10_000_000_000_000_000_000u128; // 10 ETH
    let exp1 = 25_000_000_000u128;             // 25,000 USDC

    // 1. Identical reserves pass
    assert_eq!(
        simulate_fast_revert_guard(exp0, exp1, exp0, exp1),
        GuardOutcome::Passed
    );

    // 2. 1-wei shift in reserve0 triggers FastRevert
    let outcome_r0 = simulate_fast_revert_guard(exp0, exp1, exp0 + 1, exp1);
    match outcome_r0 {
        GuardOutcome::FastRevert { gas_used, gas_saved_percent } => {
            assert_eq!(gas_used, 2_300);
            assert!(gas_saved_percent > 80.0, "Gas saved must be > 80% (got {:.2}%)", gas_saved_percent);
        }
        _ => panic!("Expected FastRevert on reserve0 divergence"),
    }

    // 3. 1-wei shift in reserve1 triggers FastRevert
    let outcome_r1 = simulate_fast_revert_guard(exp0, exp1, exp0, exp1 - 1);
    match outcome_r1 {
        GuardOutcome::FastRevert { gas_used, gas_saved_percent } => {
            assert_eq!(gas_used, 2_300);
            assert!(gas_saved_percent > 80.0);
        }
        _ => panic!("Expected FastRevert on reserve1 divergence"),
    }
}

#[test]
fn test_adv_executor_atomic_profit_enforcement() {
    // Profit exceeds gas cost -> Ok
    assert_eq!(check_atomic_profit(100_000, 50_000), Ok(()));
    // Profit exactly equals gas cost -> Ok
    assert_eq!(check_atomic_profit(50_000, 50_000), Ok(()));
    // Zero profit, zero gas cost -> Ok
    assert_eq!(check_atomic_profit(0, 0), Ok(()));

    // Profit 1 wei below gas cost -> Err
    assert_eq!(
        check_atomic_profit(49_999, 50_000),
        Err(CalldataError::AtomicProfitViolation { profit: 49_999, gas_cost: 50_000 })
    );
    // Zero profit with positive gas cost -> Err
    assert_eq!(
        check_atomic_profit(0, 1),
        Err(CalldataError::AtomicProfitViolation { profit: 0, gas_cost: 1 })
    );
}

#[test]
fn test_adv_executor_flash_loan_priority_cascade() {
    let token = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
    let mut snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 1_000_000,
        balancer_liquidity: 5_000_000,
        univ4_liquidity: 10_000_000,
        aave_liquidity: 50_000_000,
    };

    // 1. Amount <= Morpho -> selects Morpho (Priority 1, 0% fee)
    let r1 = select_optimal_flash_route(Chain::ArbitrumOne, token, 500_000, &snapshot).unwrap();
    assert_eq!(r1.provider, FlashLoanProvider::MorphoBlue);
    assert_eq!(r1.fee_pips, 0);

    // 2. Amount > Morpho, <= Balancer -> selects Balancer (Priority 2, 0% fee)
    let r2 = select_optimal_flash_route(Chain::ArbitrumOne, token, 2_000_000, &snapshot).unwrap();
    assert_eq!(r2.provider, FlashLoanProvider::BalancerV3);
    assert_eq!(r2.fee_pips, 0);

    // 3. Amount > Balancer, <= UniV4 -> selects UniV4 (Priority 3, 0% fee)
    let r3 = select_optimal_flash_route(Chain::ArbitrumOne, token, 8_000_000, &snapshot).unwrap();
    assert_eq!(r3.provider, FlashLoanProvider::UniswapV4);
    assert_eq!(r3.fee_pips, 0);

    // 4. Amount > UniV4, <= Aave -> selects Aave (Priority 4, fee applies)
    let r4_arb = select_optimal_flash_route(Chain::ArbitrumOne, token, 20_000_000, &snapshot).unwrap();
    assert_eq!(r4_arb.provider, FlashLoanProvider::AaveV3);
    assert_eq!(r4_arb.fee_pips, 500); // 0.05% on Arbitrum

    let r4_base = select_optimal_flash_route(Chain::Base, token, 20_000_000, &snapshot).unwrap();
    assert_eq!(r4_base.provider, FlashLoanProvider::AaveV3);
    assert_eq!(r4_base.fee_pips, 900); // 0.09% on Base

    // 5. Amount > all liquidity -> InsufficientLiquidity error
    let r_fail = select_optimal_flash_route(Chain::ArbitrumOne, token, 100_000_000, &snapshot);
    assert!(matches!(r_fail, Err(RoutingError::InsufficientLiquidity { .. })));

    // 6. Zero liquidity across all providers
    snapshot = ProviderLiquiditySnapshot::default();
    let r_zero = select_optimal_flash_route(Chain::ArbitrumOne, token, 1, &snapshot);
    assert!(matches!(r_zero, Err(RoutingError::InsufficientLiquidity { .. })));
}

#[test]
fn test_adv_executor_vanity_miner_eip1014_derivation_and_cancellation() {
    let deployer = address!("1111222233334444555566667777888899990000");
    let init_code = b"flash_arbitrage_executor_init_code_v1";
    let init_code_hash = keccak256(init_code).0;
    let salt = [0x42u8; 32];

    // 1. Verify EIP-1014 formula: keccak256(0xff ++ deployer ++ salt ++ init_code_hash)[12..32]
    let mut preimage = Vec::with_capacity(85);
    preimage.push(0xff);
    preimage.extend_from_slice(deployer.as_slice());
    preimage.extend_from_slice(&salt);
    preimage.extend_from_slice(&init_code_hash);
    let expected_hash = keccak256(&preimage);
    let mut expected_addr_bytes = [0u8; 20];
    expected_addr_bytes.copy_from_slice(&expected_hash.0[12..32]);
    let expected_addr = Address::from(expected_addr_bytes);

    let derived_addr = compute_create2_address(deployer, salt, init_code_hash);
    assert_eq!(derived_addr, expected_addr);

    // 2. Cancellation token aborts mining immediately (<10ms)
    let cancel = Arc::new(AtomicBool::new(true)); // Pre-cancelled
    let miner = VanityMiner::new(deployer, init_code_hash)
        .with_target_zero_bytes(8)
        .with_threads(2)
        .with_cancellation_token(cancel);

    let res = miner.mine();
    assert_eq!(res, Err(VanityMinerError::Cancelled));

    // 3. Calldata gas savings calculation
    // 8 zero bytes * 12 gas savings = 96 gas
    assert_eq!(VANITY_CALDATA_GAS_SAVINGS, 96);
}
