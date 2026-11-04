//! Feature 13 Boundaries: Flash Loan Routing BVA Test Suite

use crate::common::*;

#[test]
fn test_b13_01_zero_loan_amount() {
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 100,
        balancer_liquidity: 100,
        univ4_liquidity: 100,
        aave_liquidity: 100,
    };
    let res = route_flash_loan(0, &snapshot);
    assert!(res.is_some());
    assert_eq!(res.unwrap().0, FlashLoanProvider::MorphoBlue);
}

#[test]
fn test_b13_02_exact_liquidity_boundary() {
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 5_000,
        balancer_liquidity: 10_000,
        univ4_liquidity: 10_000,
        aave_liquidity: 10_000,
    };
    // Request exactly 5,000 -> Morpho Blue can fill it
    let res = route_flash_loan(5_000, &snapshot);
    assert_eq!(res.unwrap().0, FlashLoanProvider::MorphoBlue);
}

#[test]
fn test_b13_03_one_unit_over_morpho_liquidity() {
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 5_000,
        balancer_liquidity: 10_000,
        univ4_liquidity: 10_000,
        aave_liquidity: 10_000,
    };
    // Request 5,001 -> cascades to Balancer V3!
    let res = route_flash_loan(5_001, &snapshot);
    assert_eq!(res.unwrap().0, FlashLoanProvider::BalancerV3);
}

#[test]
fn test_b13_04_all_providers_zero_liquidity() {
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 0,
        balancer_liquidity: 0,
        univ4_liquidity: 0,
        aave_liquidity: 0,
    };
    let res = route_flash_loan(1, &snapshot);
    assert!(res.is_none());
}

#[test]
fn test_b13_05_extreme_loan_amount_overflow() {
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 1_000_000,
        balancer_liquidity: 1_000_000,
        univ4_liquidity: 1_000_000,
        aave_liquidity: 1_000_000,
    };
    let res = route_flash_loan(u128::MAX, &snapshot);
    assert!(res.is_none());
}
