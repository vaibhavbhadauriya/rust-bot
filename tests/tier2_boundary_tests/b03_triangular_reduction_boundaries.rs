//! Feature 3 Boundaries: 3-Hop Virtual Pool Reduction BVA Test Suite

use crate::common::*;

#[test]
fn test_b03_01_zero_intermediate_reserve_fails() {
    // x2 = 0 and y1 = 0 yields zero denominator -> None
    let res = reduce_virtual_pool_f64(1000.0, 0.0, 0.997, 0.0, 1000.0, 0.997);
    assert!(res.is_none());
}

#[test]
fn test_b03_02_massive_intermediate_reserve() {
    let res = reduce_virtual_pool_f64(1_000_000.0, 400.0, 0.997, 1e18, 1e18, 0.997);
    assert!(res.is_some());
    let (ex, ey, _) = res.unwrap();
    assert!(ex.is_finite() && ey.is_finite());
}

#[test]
fn test_b03_03_zero_effective_fees() {
    // 0% fee (gamma = 1.0)
    let res = reduce_virtual_pool_f64(10_000.0, 10_000.0, 1.0, 10_000.0, 10_000.0, 1.0);
    assert!(res.is_some());
    let (ex, ey, g) = res.unwrap();
    assert_eq!(g, 1.0);
    assert_eq!(ex, 5_000.0);
    assert_eq!(ey, 5_000.0);
}

#[test]
fn test_b03_04_microscopic_input_reduction_accuracy() {
    let (ex, ey, g) = reduce_virtual_pool_f64(1e6, 400.0, 0.997, 400.0, 1e6, 0.997).unwrap();
    let dx = 1e-6; // 1 micro-token
    let dy_direct = (400.0 * 0.997 * dx) / (1e6 + 0.997 * dx);
    let dz_direct = (1e6 * 0.997 * dy_direct) / (400.0 + 0.997 * dy_direct);
    let dz_virt = (ey * g * dx) / (ex + g * dx);

    assert!((dz_direct - dz_virt).abs() < 1e-15);
}

#[test]
fn test_b03_05_100_percent_fee_in_hop2() {
    // If Hop 2 has 100% fee (gamma2 = 0.0), Ey becomes 0
    let res = reduce_virtual_pool_f64(10_000.0, 10_000.0, 0.997, 10_000.0, 10_000.0, 0.0);
    assert!(res.is_some());
    let (_, ey, _) = res.unwrap();
    assert_eq!(ey, 0.0);
}
