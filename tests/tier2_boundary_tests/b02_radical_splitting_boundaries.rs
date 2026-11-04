//! Feature 2 Boundaries: Radical Splitting & 512-bit mulDiv BVA Test Suite

use crate::common::*;
use alloy_primitives::U256;

#[test]
fn test_b02_01_u128_max_reserves() {
    let max = u128::MAX;
    let res = radical_split_sizing(max, max, max, max, 3000, 3000, 0);
    // At maximum reserves and symmetric fees, price difference is zero -> returns None
    assert!(res.is_none());
}

#[test]
fn test_b02_02_zero_sqrt() {
    assert_eq!(sqrt_u256(U256::ZERO), U256::ZERO);
    assert_eq!(isqrt_u128(0), 0);
}

#[test]
fn test_b02_03_mul_div_numerator_equals_denominator() {
    let a = U256::from(999_999);
    let b = U256::from(1);
    let c = U256::from(999_999);
    assert_eq!(mul_div_u256(a, b, c), Some(U256::from(1)));
}

#[test]
fn test_b02_04_one_pip_fee_extreme() {
    // 1 PIP = 0.0001%
    let res = radical_split_sizing(1_000_000, 405, 1_000_000, 395, 1, 1, 0);
    assert!(res.is_some());
}

#[test]
fn test_b02_05_u256_max_mul_div_safety() {
    let max = U256::MAX;
    let one = U256::from(1);
    assert_eq!(mul_div_u256(max, one, max), Some(U256::from(1)));
}
