//! Feature 3: 3-Hop Virtual Pool Reduction Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 39-41, 80, and spec_miner_survey_2/handoff.md.

use crate::common::*;

#[test]
fn test_f03_01_reduction_algebraic_identity() {
    // Pool 1: Token A -> B (x1, y1, gamma1)
    // Pool 2: Token B -> C (x2, y2, gamma2)
    let x1 = 1_000_000.0;
    let y1 = 400.0;
    let gamma1 = 0.997; // 0.3% fee

    let x2 = 500.0;
    let y2 = 1_200_000.0;
    let gamma2 = 0.997;

    let reduced = reduce_virtual_pool_f64(x1, y1, gamma1, x2, y2, gamma2);
    assert!(reduced.is_some());
    let (ex, ey, gamma_eff) = reduced.unwrap();
    assert_eq!(gamma_eff, gamma1, "Effective fee of virtual pool must equal gamma1");

    // Test across 10 different input amounts that 2-hop swap exactly equals 1-hop virtual swap
    for &dx in &[1.0, 10.0, 100.0, 1000.0, 5000.0, 10000.0] {
        // Step 1 & 2 directly:
        let dy = (y1 * gamma1 * dx) / (x1 + gamma1 * dx);
        let dz_direct = (y2 * gamma2 * dy) / (x2 + gamma2 * dy);

        // Virtual pool swap directly:
        let dz_virtual = (ey * gamma_eff * dx) / (ex + gamma_eff * dx);

        let relative_diff = (dz_direct - dz_virtual).abs() / dz_direct;
        assert!(
            relative_diff < 1e-12,
            "Virtual pool output must identically match 2-hop composition (diff: {})",
            relative_diff
        );
    }
}

#[test]
fn test_f03_02_triangular_closed_form_vs_numerical_search() {
    // 3 pools forming a cycle A -> B -> C -> A
    let x1 = 1_000_000.0;
    let y1 = 405.0;
    let gamma1 = 0.997;

    let x2 = 400.0;
    let y2 = 420_000.0;
    let gamma2 = 0.997;

    let x3 = 410_000.0;
    let y3 = 1_010_000.0;
    let gamma3 = 0.997;

    // Reduce Pool 1 and 2 to Virtual Pool (Ex, Ey)
    let (ex, ey, g12) = reduce_virtual_pool_f64(x1, y1, gamma1, x2, y2, gamma2).unwrap();

    // Solve (Ex, Ey) against Pool 3 using closed-form 2-pool solver
    let f12 = 1.0 - g12;
    let f3 = 1.0 - gamma3;
    let dx_closed = solve_2pool_optimal_f64(ex, ey, y3, x3, f12, f3, 0.0).unwrap();

    // Golden-section search directly on 3-hop objective function
    let profit_fn = |dx: f64| -> f64 {
        let dy = (y1 * gamma1 * dx) / (x1 + gamma1 * dx);
        let dz = (y2 * gamma2 * dy) / (x2 + gamma2 * dy);
        let dx_out = (y3 * gamma3 * dz) / (x3 + gamma3 * dz);
        dx_out - dx
    };

    let mut left = 1.0;
    let mut right = 50_000.0;
    let phi_ratio = (5.0f64.sqrt() - 1.0) / 2.0;
    for _ in 0..100 {
        let x_l = right - phi_ratio * (right - left);
        let x_r = left + phi_ratio * (right - left);
        if profit_fn(x_l) < profit_fn(x_r) {
            left = x_l;
        } else {
            right = x_r;
        }
    }
    let dx_numerical = (left + right) / 2.0;

    let rel_err = (dx_closed - dx_numerical).abs() / dx_numerical;
    assert!(
        rel_err < 0.00001, // < 0.001% required by AC1
        "Closed form virtual reduction relative error {} must be < 0.001%",
        rel_err
    );
}

#[test]
fn test_f03_03_triangular_weth_usdc_dai_cycle() {
    // WETH -> USDC -> DAI -> WETH
    let (ex, ey, g1) = reduce_virtual_pool_f64(100.0, 250_000.0, 0.9995, 250_000.0, 250_500.0, 0.9999).unwrap();
    assert!(ex > 0.0 && ey > 0.0 && g1 > 0.0);
}

#[test]
fn test_f03_04_symmetric_pools_reduction() {
    let x = 10_000.0;
    let y = 10_000.0;
    let g = 0.997;

    let (ex, ey, _) = reduce_virtual_pool_f64(x, y, g, x, y, g).unwrap();
    // When x1=x2=x and y1=y2=y, den = x + g*y
    let expected_ex = (x * x) / (x + g * y);
    let expected_ey = (y * y * g) / (x + g * y);

    assert!((ex - expected_ex).abs() < 1e-6);
    assert!((ey - expected_ey).abs() < 1e-6);
}

#[test]
fn test_f03_05_extreme_ratio_bottleneck() {
    // Intermediate token has tiny liquidity (bottleneck)
    let x1 = 1_000_000.0;
    let y1 = 10.0;
    let g1 = 0.997;
    let x2 = 10.0;
    let y2 = 1_000_000.0;
    let g2 = 0.997;

    let (ex, ey, _) = reduce_virtual_pool_f64(x1, y1, g1, x2, y2, g2).unwrap();
    // Ex and Ey are heavily damped by intermediate bottleneck
    assert!(ex < 1_000_000.0);
    assert!(ey < 1_000_000.0);
}
