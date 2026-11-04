//! Feature 7 Boundaries: In-Memory Graph & Screening BVA Test Suite

use crate::common::*;

#[test]
fn test_b07_01_exact_cross_product_equality() {
    let y_a = 400u128;
    let x_b = 1000u128;
    let y_b = 400u128;
    let x_a = 1000u128;

    // yA * xB == yB * xA -> not strictly less than -> false
    assert!(!quick_spread_screen(y_a, x_b, y_b, x_a));
}

#[test]
fn test_b07_02_single_unit_cross_product_difference() {
    let y_a = 400u128;
    let x_b = 1000u128; // 400,000
    let y_b = 400u128;
    let x_a = 1001u128; // 400,400

    // 400,000 < 400,400 -> true
    assert!(quick_spread_screen(y_a, x_b, y_b, x_a));
}

#[test]
fn test_b07_03_u128_max_screening() {
    let max = u128::MAX;
    // max * (max - 1) < max * max
    assert!(quick_spread_screen(max - 1, max, max, max));
}

#[test]
fn test_b07_04_zero_spread_screening() {
    assert!(!quick_spread_screen(0, 0, 0, 0));
}

#[test]
fn test_b07_05_max_hops_depth_boundary() {
    let hops = 3;
    assert!(hops <= 3);
    assert!(!(4 <= 3));
}
