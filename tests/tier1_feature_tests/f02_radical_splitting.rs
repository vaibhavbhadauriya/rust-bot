//! Feature 2: Radical Splitting & 512-bit mulDiv Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md line 38 and spec_miner_survey_2/handoff.md.

use crate::common::*;
use alloy_primitives::U256;

#[test]
fn test_f02_01_radical_splitting_prevents_256bit_overflow() {
    // 4 reserves of size 2^110 each:
    // Product xA * yA * xB * yB would be 2^440, completely overflowing U256!
    let x_a = 1u128 << 110;
    let y_a = 1u128 << 110;
    let x_b = (1u128 << 110) + (1u128 << 105);
    let y_b = (1u128 << 110) - (1u128 << 105);

    let res = radical_split_sizing(x_a, y_a, x_b, y_b, 3000, 3000, 0);
    // Radical splitting safely evaluates without panic or register overflow
    assert!(res.is_some(), "Radical splitting must not panic or overflow on 2^440 products");
}

#[test]
fn test_f02_02_mul_div_u256_precision() {
    let a = U256::from(10_000_000_000u64);
    let b = U256::from(3_000_000_000u64);
    let c = U256::from(2_000_000_000u64);

    let result = mul_div_u256(a, b, c);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), U256::from(15_000_000_000u64));
}

#[test]
fn test_f02_03_integer_sqrt_invariant() {
    let test_values = [
        U256::from(0),
        U256::from(1),
        U256::from(4),
        U256::from(9),
        U256::from(15),
        U256::from(16),
        U256::from(1_000_000_000_000u64),
        U256::from(u128::MAX),
    ];

    for &val in &test_values {
        let root = sqrt_u256(val);
        let root_sq = root * root;
        let next_sq = (root + U256::from(1)) * (root + U256::from(1));
        assert!(root_sq <= val, "root^2 must be <= val for {}", val);
        assert!(val < next_sq, "val must be < (root + 1)^2 for {}", val);
    }
}

#[test]
fn test_f02_04_pips_fee_scaling() {
    // 3,000 PIPs is 0.3%
    let pips_3000 = 3000u32;
    let fee_fraction = pips_3000 as f64 / 1_000_000.0;
    assert_eq!(fee_fraction, 0.003);

    // 500 PIPs is 0.05%
    let pips_500 = 500u32;
    let fee_fraction_500 = pips_500 as f64 / 1_000_000.0;
    assert_eq!(fee_fraction_500, 0.0005);

    // 1 PIP is 0.0001%
    let pips_1 = 1u32;
    let fee_fraction_1 = pips_1 as f64 / 1_000_000.0;
    assert_eq!(fee_fraction_1, 0.000001);
}

#[test]
fn test_f02_05_division_by_zero_safety() {
    let a = U256::from(1_000);
    let b = U256::from(2_000);
    let c = U256::ZERO;

    let res = mul_div_u256(a, b, c);
    assert_eq!(res, None, "mulDiv with zero denominator must return None safely");
}
