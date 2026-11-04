//! Feature 4 Boundaries: Concentrated Liquidity BVA Test Suite

use crate::common::*;

#[test]
fn test_b04_01_min_tick_boundary() {
    // MIN_TICK = -887272
    let (word, bit) = div_euclid_tick(-887272, 1);
    assert_eq!(word, (-887272i32 >> 8) as i16);
    let _ = bit;
}

#[test]
fn test_b04_02_max_tick_boundary() {
    // MAX_TICK = 887272
    let (word, bit) = div_euclid_tick(887272, 1);
    assert_eq!(word, (887272i32 >> 8) as i16);
    let _ = bit;
}

#[test]
fn test_b04_03_tick_spacings() {
    // Common Uniswap V3 / Slipstream spacings: 1, 10, 60, 200
    for &spacing in &[1, 10, 60, 200] {
        let (w_neg, _) = div_euclid_tick(-1, spacing);
        assert_eq!(w_neg, -1, "Negative tick with spacing {} must map to word -1", spacing);

        let (w_pos, _) = div_euclid_tick(0, spacing);
        assert_eq!(w_pos, 0);
    }
}

#[test]
fn test_b04_04_zero_liquidity_virtual_reserves() {
    let (x, y) = virtual_reserves_from_clmm(0.0, 2500.0);
    assert_eq!(x, 0.0);
    assert_eq!(y, 0.0);
}

#[test]
fn test_b04_05_extreme_price_virtual_reserves() {
    let (x, y) = virtual_reserves_from_clmm(1_000_000.0, 1e8);
    assert!(x.is_finite() && y.is_finite());
}
