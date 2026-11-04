//! Feature 5 Boundaries: Hard Pre-Filters BVA Test Suite

use crate::common::*;

#[test]
fn test_b05_01_exactly_one_wei_below_unit() {
    // 6-decimal: 10^6 - 1 = 999,999
    assert!(!filter_unit_reserves(999_999, 6, 1_000_000, 6));
}

#[test]
fn test_b05_02_exactly_unit_boundary() {
    assert!(filter_unit_reserves(1_000_000, 6, 1_000_000, 6));
}

#[test]
fn test_b05_03_zero_decimal_token() {
    // d = 0: unit is 10^0 = 1
    assert!(!filter_unit_reserves(0, 0, 10, 0));
    assert!(filter_unit_reserves(1, 0, 1, 0));
}

#[test]
fn test_b05_04_max_reserves_boundary() {
    let max = u128::MAX;
    assert!(filter_unit_reserves(max, 18, max, 18));
}

#[test]
fn test_b05_05_asymmetric_decimals() {
    // 6-decimal (USDC) and 18-decimal (WETH)
    assert!(filter_unit_reserves(1_000_000, 6, 1_000_000_000_000_000_000, 18));
    assert!(!filter_unit_reserves(999_999, 6, 1_000_000_000_000_000_000, 18));
}
