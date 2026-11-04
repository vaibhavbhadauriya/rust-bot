//! Challenger Empirical Verification Test for C3, H6, and L8
//! Written by m_r2_challenger_2 to independently verify and stress-test claims.

use alloy_primitives::{keccak256, Address};
use flash_core::constants::{arbitrum, base};
use flash_executor::{
    bytecode::{build_executor_bytecode, build_executor_init_code},
    routing::{
        route_flash_loan, route_flash_loan_for_chain, FlashLoanProvider, ProviderLiquiditySnapshot,
    },
    EXECUTOR_INIT_CODE, EXECUTOR_RUNTIME_BYTECODE,
};

#[test]
fn test_c3_keccak_hash_exactness() {
    // Exact Solidity signature: swap(uint256,uint256,address,bytes)
    let sig = b"swap(uint256,uint256,address,bytes)";
    let hash = keccak256(sig);
    let selector = &hash[0..4];

    // Assert selector is exactly 0x022c0d9f
    assert_eq!(
        selector,
        &[0x02, 0x2c, 0x0d, 0x9f],
        "Keccak256 of swap(uint256,uint256,address,bytes) must be 0x022c0d9f"
    );

    // Assert it does NOT match the typo 0x022c089f
    assert_ne!(
        selector,
        &[0x02, 0x2c, 0x08, 0x9f],
        "Keccak256 must NOT match buggy selector 0x022c089f"
    );
}

#[test]
fn test_c3_executor_bytecode_no_0x022c089f() {
    let buggy = [0x02, 0x2c, 0x08, 0x9f];
    let correct = [0x02, 0x2c, 0x0d, 0x9f];

    // Check runtime bytecode
    assert!(
        !EXECUTOR_RUNTIME_BYTECODE
            .windows(4)
            .any(|w| w == buggy),
        "EXECUTOR_RUNTIME_BYTECODE must not contain buggy selector 0x022c089f"
    );
    assert!(
        EXECUTOR_RUNTIME_BYTECODE
            .windows(4)
            .any(|w| w == correct),
        "EXECUTOR_RUNTIME_BYTECODE must contain correct selector 0x022c0d9f"
    );

    // Check init code
    assert!(
        !EXECUTOR_INIT_CODE.windows(4).any(|w| w == buggy),
        "EXECUTOR_INIT_CODE must not contain buggy selector 0x022c089f"
    );
    assert!(
        EXECUTOR_INIT_CODE.windows(4).any(|w| w == correct),
        "EXECUTOR_INIT_CODE must contain correct selector 0x022c0d9f"
    );

    // Check parameterized runtime bytecode for Arbitrum and Base
    let owner = Address::repeat_byte(0x42);
    let arb_bc = build_executor_bytecode(arbitrum::CHAIN_ID, owner);
    assert!(
        !arb_bc.windows(4).any(|w| w == buggy),
        "Arbitrum bytecode must not contain buggy selector 0x022c089f"
    );
    assert!(
        arb_bc.windows(4).any(|w| w == correct),
        "Arbitrum bytecode must contain correct selector 0x022c0d9f"
    );

    let base_bc = build_executor_bytecode(base::CHAIN_ID, owner);
    assert!(
        !base_bc.windows(4).any(|w| w == buggy),
        "Base bytecode must not contain buggy selector 0x022c089f"
    );
    assert!(
        base_bc.windows(4).any(|w| w == correct),
        "Base bytecode must contain correct selector 0x022c0d9f"
    );

    let arb_init = build_executor_init_code(arbitrum::CHAIN_ID, owner);
    assert!(!arb_init.windows(4).any(|w| w == buggy));
    assert!(arb_init.windows(4).any(|w| w == correct));

    let base_init = build_executor_init_code(base::CHAIN_ID, owner);
    assert!(!base_init.windows(4).any(|w| w == buggy));
    assert!(base_init.windows(4).any(|w| w == correct));
}

#[test]
fn test_h6_fee_pips_matrix_comprehensive() {
    // 1. Arbitrum One (42161)
    assert_eq!(FlashLoanProvider::MorphoBlue.fee_pips(arbitrum::CHAIN_ID), 0);
    assert_eq!(FlashLoanProvider::BalancerV3.fee_pips(arbitrum::CHAIN_ID), 0);
    assert_eq!(FlashLoanProvider::UniswapV4.fee_pips(arbitrum::CHAIN_ID), 0);
    assert_eq!(FlashLoanProvider::AaveV3.fee_pips(arbitrum::CHAIN_ID), 500); // 0.05%

    // 2. Base (8453)
    assert_eq!(FlashLoanProvider::MorphoBlue.fee_pips(base::CHAIN_ID), 0);
    assert_eq!(FlashLoanProvider::BalancerV3.fee_pips(base::CHAIN_ID), 0);
    assert_eq!(FlashLoanProvider::UniswapV4.fee_pips(base::CHAIN_ID), 0);
    assert_eq!(FlashLoanProvider::AaveV3.fee_pips(base::CHAIN_ID), 900); // 0.09%

    // 3. Fallback / other chain IDs (e.g. 1, 10, 0, u64::MAX)
    for unknown_chain in [1u64, 10, 0, u64::MAX, 999_999] {
        assert_eq!(FlashLoanProvider::MorphoBlue.fee_pips(unknown_chain), 0);
        assert_eq!(FlashLoanProvider::BalancerV3.fee_pips(unknown_chain), 0);
        assert_eq!(FlashLoanProvider::UniswapV4.fee_pips(unknown_chain), 0);
        assert_eq!(
            FlashLoanProvider::AaveV3.fee_pips(unknown_chain),
            500,
            "Unknown chains should default to 500 PIPs"
        );
    }
}

#[test]
fn test_h6_route_flash_loan_for_chain_arbitrum_and_base() {
    let req = 10_000u128;

    // Case A: Morpho has full liquidity -> Morpho on both chains with 0 fee
    let snap_morpho = ProviderLiquiditySnapshot::new(20_000, 20_000, 20_000, 20_000);
    assert_eq!(
        route_flash_loan_for_chain(req, &snap_morpho, arbitrum::CHAIN_ID),
        Some((FlashLoanProvider::MorphoBlue, 0))
    );
    assert_eq!(
        route_flash_loan_for_chain(req, &snap_morpho, base::CHAIN_ID),
        Some((FlashLoanProvider::MorphoBlue, 0))
    );

    // Case B: Morpho insufficient, Balancer has liquidity -> Balancer (0 fee)
    let snap_balancer = ProviderLiquiditySnapshot::new(5_000, 20_000, 20_000, 20_000);
    assert_eq!(
        route_flash_loan_for_chain(req, &snap_balancer, arbitrum::CHAIN_ID),
        Some((FlashLoanProvider::BalancerV3, 0))
    );
    assert_eq!(
        route_flash_loan_for_chain(req, &snap_balancer, base::CHAIN_ID),
        Some((FlashLoanProvider::BalancerV3, 0))
    );

    // Case C: Morpho and Balancer insufficient, UniV4 has liquidity -> UniV4 (0 fee)
    let snap_univ4 = ProviderLiquiditySnapshot::new(5_000, 5_000, 20_000, 20_000);
    assert_eq!(
        route_flash_loan_for_chain(req, &snap_univ4, arbitrum::CHAIN_ID),
        Some((FlashLoanProvider::UniswapV4, 0))
    );
    assert_eq!(
        route_flash_loan_for_chain(req, &snap_univ4, base::CHAIN_ID),
        Some((FlashLoanProvider::UniswapV4, 0))
    );

    // Case D: Zero-fee venues insufficient, AaveV3 has liquidity -> AaveV3 with CHAIN-SPECIFIC fee!
    let snap_aave = ProviderLiquiditySnapshot::new(5_000, 5_000, 5_000, 20_000);
    assert_eq!(
        route_flash_loan_for_chain(req, &snap_aave, arbitrum::CHAIN_ID),
        Some((FlashLoanProvider::AaveV3, 500)),
        "Arbitrum AaveV3 must return 500 pips"
    );
    assert_eq!(
        route_flash_loan_for_chain(req, &snap_aave, base::CHAIN_ID),
        Some((FlashLoanProvider::AaveV3, 900)),
        "Base AaveV3 must return 900 pips"
    );

    // Case E: None have liquidity -> None
    let snap_none = ProviderLiquiditySnapshot::new(5_000, 5_000, 5_000, 5_000);
    assert_eq!(
        route_flash_loan_for_chain(req, &snap_none, arbitrum::CHAIN_ID),
        None
    );
    assert_eq!(
        route_flash_loan_for_chain(req, &snap_none, base::CHAIN_ID),
        None
    );

    // Case F: route_flash_loan (default Arbitrum wrapper) matches route_flash_loan_for_chain(arb)
    assert_eq!(
        route_flash_loan(req, &snap_aave),
        Some((FlashLoanProvider::AaveV3, 500))
    );
}

#[test]
fn test_h6_routing_extreme_boundaries() {
    // 1. Exact boundary matching
    let snap_exact = ProviderLiquiditySnapshot::new(100, 0, 0, 0);
    assert_eq!(
        route_flash_loan_for_chain(100, &snap_exact, arbitrum::CHAIN_ID),
        Some((FlashLoanProvider::MorphoBlue, 0))
    );
    assert_eq!(
        route_flash_loan_for_chain(101, &snap_exact, arbitrum::CHAIN_ID),
        None
    );

    // 2. Zero amount requested
    let snap_zero = ProviderLiquiditySnapshot::new(0, 0, 0, 0);
    assert_eq!(
        route_flash_loan_for_chain(0, &snap_zero, arbitrum::CHAIN_ID),
        Some((FlashLoanProvider::MorphoBlue, 0))
    );

    // 3. u128::MAX handling
    let snap_max = ProviderLiquiditySnapshot::new(0, 0, 0, u128::MAX);
    assert_eq!(
        route_flash_loan_for_chain(u128::MAX, &snap_max, base::CHAIN_ID),
        Some((FlashLoanProvider::AaveV3, 900))
    );
}
