//! Feature 13: Zero-Fee Flash Loan Routing & Callbacks Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 48-52 and explorer_survey_3/handoff.md.

use crate::common::*;

#[test]
fn test_f13_01_morpho_blue_priority_1() {
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 10_000_000,
        balancer_liquidity: 10_000_000,
        univ4_liquidity: 10_000_000,
        aave_liquidity: 10_000_000,
    };

    let (provider, fee) = route_flash_loan(5_000_000, &snapshot).expect("Route should exist");
    assert_eq!(provider, FlashLoanProvider::MorphoBlue);
    assert_eq!(fee, 0, "Morpho Blue must have 0% fee");
    assert_eq!(provider.callback_selector(), "0x3a48e7e1");
}

#[test]
fn test_f13_02_balancer_v3_priority_2() {
    // Morpho lacks liquidity -> falls back to Balancer V3
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 1_000_000,
        balancer_liquidity: 10_000_000,
        univ4_liquidity: 10_000_000,
        aave_liquidity: 10_000_000,
    };

    let (provider, fee) = route_flash_loan(5_000_000, &snapshot).expect("Route should exist");
    assert_eq!(provider, FlashLoanProvider::BalancerV3);
    assert_eq!(fee, 0, "Balancer V3 must have 0% fee");
    assert_eq!(provider.callback_selector(), "0x98dd202a");
}

#[test]
fn test_f13_03_univ4_priority_3() {
    // Morpho and Balancer lack liquidity -> falls back to Uniswap V4
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 1_000_000,
        balancer_liquidity: 1_000_000,
        univ4_liquidity: 10_000_000,
        aave_liquidity: 10_000_000,
    };

    let (provider, fee) = route_flash_loan(5_000_000, &snapshot).expect("Route should exist");
    assert_eq!(provider, FlashLoanProvider::UniswapV4);
    assert_eq!(fee, 0, "Uniswap V4 must have 0% fee");
}

#[test]
fn test_f13_04_aave_v3_last_resort() {
    // All zero-fee venues lack liquidity -> falls back to Aave V3
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 1_000_000,
        balancer_liquidity: 1_000_000,
        univ4_liquidity: 1_000_000,
        aave_liquidity: 10_000_000,
    };

    let (provider, fee) = route_flash_loan(5_000_000, &snapshot).expect("Route should exist");
    assert_eq!(provider, FlashLoanProvider::AaveV3);
    assert_eq!(fee, 500, "Aave V3 charges 0.05% (500 PIPs)");
    assert_eq!(provider.callback_selector(), "0x9293e4e7");
}

#[test]
fn test_f13_05_insufficient_liquidity_across_all_venues() {
    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: 1_000,
        balancer_liquidity: 1_000,
        univ4_liquidity: 1_000,
        aave_liquidity: 1_000,
    };

    let res = route_flash_loan(5_000_000, &snapshot);
    assert!(res.is_none(), "Must return None when all venues lack sufficient liquidity");
}
