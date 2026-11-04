//! Tier 5 Adversarial Coverage Hardening: crates/core
//!
//! White-box stress testing of:
//! 1. FeePips boundary and extreme inputs (0, 1, 10^6, overflow, float conversions)
//! 2. Chain IDs and string conversions
//! 3. Venue classification (12 DEX venues, CLMM, CPMM, Flash Loan Priority)
//! 4. Token decimal calculations and extreme unit boundaries (0..38, 39+ overflow safety)
//! 5. Reserve validation (repr(C) alignment, unit checks)
//! 6. Canonical addresses and execution selectors

use alloy_primitives::{address, Address, U256};
use flash_core::constants::*;
use flash_core::types::*;

#[test]
fn test_adv_feepips_boundaries_and_gamma() {
    // 0% fee (0 pips)
    let fee0 = FeePips::from_pips(0);
    assert_eq!(fee0.as_pips(), 0);
    assert_eq!(fee0.gamma_pips(), 1_000_000);
    assert_eq!(fee0.as_f64(), 0.0);
    assert_eq!(fee0.gamma_f64(), 1.0);

    // 0.3% standard Uniswap V2 fee (3,000 pips)
    let fee_v2 = FeePips::from_bps(30);
    assert_eq!(fee_v2.as_pips(), 3_000);
    assert_eq!(fee_v2.gamma_pips(), 997_000);
    assert!((fee_v2.as_f64() - 0.003).abs() < 1e-9);
    assert!((fee_v2.gamma_f64() - 0.997).abs() < 1e-9);

    // 100% fee (1,000,000 pips)
    let fee_max = FeePips::from_pips(1_000_000);
    assert_eq!(fee_max.as_pips(), 1_000_000);
    assert_eq!(fee_max.gamma_pips(), 0);

    // Fee > 100% (saturating sub check)
    let fee_overflow = FeePips::from_pips(2_000_000);
    assert_eq!(fee_overflow.gamma_pips(), 0);

    // from_percentage conversions
    let fee_pct = FeePips::from_percentage(0.05); // 0.05% = 500 pips
    assert_eq!(fee_pct.as_pips(), 500);

    // Conversion trait implementations
    let as_u32: u32 = fee_v2.into();
    assert_eq!(as_u32, 3_000);
    let from_u32 = FeePips::from(5_000);
    assert_eq!(from_u32.as_pips(), 5_000);
}

#[test]
fn test_adv_chain_id_and_parsing() {
    assert_eq!(Chain::ArbitrumOne.id(), 42161);
    assert_eq!(Chain::Base.id(), 8453);
    assert_eq!(Chain::ArbitrumOne.name(), "Arbitrum One");
    assert_eq!(Chain::Base.name(), "Base");

    assert_eq!(Chain::from_id(42161), Some(Chain::ArbitrumOne));
    assert_eq!(Chain::from_id(8453), Some(Chain::Base));
    assert_eq!(Chain::from_id(1), None);
    assert_eq!(Chain::from_id(0), None);
    assert_eq!(Chain::from_id(u64::MAX), None);
}

#[test]
fn test_adv_venue_matrix_classification() {
    let clmm_venues = [
        Venue::UniswapV3,
        Venue::UniswapV4,
        Venue::CamelotV3,
        Venue::AerodromeSlipstream,
    ];
    for v in &clmm_venues {
        assert!(v.is_clmm(), "Venue {:?} must be CLMM", v);
        assert!(!v.is_cpmm(), "Venue {:?} must NOT be CPMM", v);
    }

    let cpmm_venues = [
        Venue::UniswapV2,
        Venue::SushiSwap,
        Venue::SushiSwapV2,
        Venue::CamelotV2,
        Venue::AerodromeV2,
    ];
    for v in &cpmm_venues {
        assert!(v.is_cpmm(), "Venue {:?} must be CPMM", v);
        assert!(!v.is_clmm(), "Venue {:?} must NOT be CLMM", v);
    }

    // Flash loan providers & priority
    assert_eq!(Venue::MorphoBlue.flash_loan_priority(), Some(1));
    assert_eq!(Venue::BalancerV3.flash_loan_priority(), Some(2));
    assert_eq!(Venue::UniswapV4.flash_loan_priority(), Some(3));
    assert_eq!(Venue::AaveV3.flash_loan_priority(), Some(4));
    assert_eq!(Venue::UniswapV2.flash_loan_priority(), None);
    assert_eq!(Venue::UniswapV3.flash_loan_priority(), None);
}

#[test]
fn test_adv_token_decimals_and_unit_reserves() {
    // Standard ERC20 decimals
    let token_usdc = Token::new(Address::ZERO, "USDC", "USD Coin", 6, 42161);
    assert_eq!(token_usdc.unit(), 1_000_000);
    assert!(token_usdc.is_unit_reserve(1_000_000));
    assert!(token_usdc.is_unit_reserve(1_000_001));
    assert!(!token_usdc.is_unit_reserve(999_999));

    let token_weth = Token::new(Address::ZERO, "WETH", "Wrapped Ether", 18, 42161);
    assert_eq!(token_weth.unit(), 1_000_000_000_000_000_000);

    let token_d0 = Token::new(Address::ZERO, "D0", "Zero Decimals", 0, 8453);
    assert_eq!(token_d0.unit(), 1);
    assert!(token_d0.is_unit_reserve(1));
    assert!(!token_d0.is_unit_reserve(0));

    // Boundary decimals 38 (max power fitting into u128: 10^38 <= u128::MAX = 3.4e38)
    let token_d38 = Token::new(Address::ZERO, "D38", "38 Decimals", 38, 8453);
    assert!(token_d38.unit() > 0);

    // format_units & parse_units
    let parsed_1eth = token_weth.parse_units(1.0);
    assert_eq!(parsed_1eth, U256::from(1_000_000_000_000_000_000u64));
    assert_eq!(token_weth.format_units(parsed_1eth), 1.0);

    let parsed_half_usdc = token_usdc.parse_units(0.5);
    assert_eq!(parsed_half_usdc, U256::from(500_000u64));
    assert_eq!(token_usdc.format_units(parsed_half_usdc), 0.5);

    // Negative parse_units returns ZERO
    assert_eq!(token_usdc.parse_units(-1.0), U256::ZERO);
}

#[test]
fn test_adv_reserve_repr_c_and_unit_filter() {
    use std::mem;
    // Verify memory alignment and layout
    assert_eq!(mem::size_of::<Reserve>(), 48); // 16B + 16B + 4B + 12B padding = 48 bytes
    assert_eq!(mem::align_of::<Reserve>(), 16);

    let r_valid = Reserve::new(1_000_000, 1_000_000_000_000_000_000);
    assert!(r_valid.is_valid_unit(6, 18));
    assert!(!r_valid.is_valid_unit(7, 18)); // Reserve 0 too small

    // Decimal >= 39 checked_pow fallback without panic
    assert!(!r_valid.is_valid_unit(39, 18));
    assert!(!r_valid.is_valid_unit(6, 255));
}

#[test]
fn test_adv_canonical_constants_and_selectors() {
    assert_eq!(GET_RESERVES_SELECTOR, [0x09, 0x02, 0xf1, 0xac]);
    assert_eq!(GET_PAIR_SELECTOR, [0xe6, 0xa4, 0x39, 0x05]);
    assert_eq!(BALANCE_OF_SELECTOR, [0x70, 0xa0, 0x82, 0x31]);
    assert_eq!(TRANSFER_SELECTOR, [0xa9, 0x05, 0x9c, 0xbb]);

    assert_eq!(CALLDATA_PACKED_LEN, 100);
    assert_eq!(FAST_REVERT_GUARD_LEN, 30);
    assert_eq!(VANITY_TARGET_ZEROS, 16);

    // Canonical WETH Arbitrum
    assert_eq!(
        arbitrum::WETH,
        address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1")
    );
    // Canonical WETH Base
    assert_eq!(
        base::WETH,
        address!("4200000000000000000000000000000000000006")
    );
}
