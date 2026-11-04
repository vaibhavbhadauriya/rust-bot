//! Feature 1: V2 Constant-Product Sizing (dx*) Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 35-38, 78, and spec_miner_survey_2/handoff.md.

use crate::common::*;

#[test]
fn test_f01_01_ac1_reference_vector_morpho_zero_fee() {
    // Authoritative AC1 Vector:
    // Pool A: 1,000,000 USDC, 405 ETH, fA = 0.3%
    // Pool B: 1,000,000 USDC, 395 ETH, fB = 0.3%
    // Zero-fee flash loan (phi = 0%)
    let x_a = 1_000_000.0;
    let y_a = 405.0;
    let x_b = 1_000_000.0;
    let y_b = 395.0;
    let f_a = 0.003;
    let f_b = 0.003;
    let phi = 0.0;

    let dx_opt = solve_2pool_optimal_f64(x_a, y_a, x_b, y_b, f_a, f_b, phi);
    assert!(dx_opt.is_some(), "Optimal size should exist for AC1 reference vector");
    let dx = dx_opt.unwrap();

    // Verify dx* = 4,732.42 USDC (+- 0.01)
    let rounded_dx = (dx * 100.0).round() / 100.0;
    assert_eq!(rounded_dx, 4732.42, "dx* must match 4732.42 USDC");

    let (dx_out, gross_profit, net_profit) = calculate_2pool_profit(dx, x_a, y_a, x_b, y_b, f_a, f_b, phi);
    let rounded_gross = (gross_profit * 100.0).round() / 100.0;
    assert_eq!(rounded_gross, 45.15, "Gross profit must match 45.15 USDC verbatim");
    assert_eq!(rounded_gross, (net_profit * 100.0).round() / 100.0, "Gross == Net when phi = 0");
    assert!(dx_out > dx);
}

#[test]
fn test_f01_02_ac1_reference_vector_aave_fee() {
    // Authoritative AC1 Vector with Aave V3 fee (phi = 0.05% = 0.0005)
    let x_a = 1_000_000.0;
    let y_a = 405.0;
    let x_b = 1_000_000.0;
    let y_b = 395.0;
    let f_a = 0.003;
    let f_b = 0.003;
    let phi = 0.0005;

    let dx_opt = solve_2pool_optimal_f64(x_a, y_a, x_b, y_b, f_a, f_b, 0.0);
    assert!(dx_opt.is_some());
    let dx = dx_opt.unwrap();

    let (_, gross_profit, _) = calculate_2pool_profit(dx, x_a, y_a, x_b, y_b, f_a, f_b, 0.0);
    let flash_fee = phi * dx;
    let net_profit = gross_profit - flash_fee;
    let rounded_net = (net_profit * 100.0).round() / 100.0;
    assert_eq!(rounded_net, 42.79, "Net profit after 0.05% fee must match 42.79 USDC verbatim");
    assert!(gross_profit > net_profit);
}

#[test]
fn test_f01_03_asymmetric_fees() {
    // Test asymmetric fee tiers (e.g. Uniswap 0.05% vs Sushi 0.30%)
    let x_a = 2_000_000.0;
    let y_a = 800.0;
    let x_b = 2_000_000.0;
    let y_b = 780.0;
    let f_a = 0.0005; // 0.05%
    let f_b = 0.0030; // 0.30%
    let phi = 0.0;

    let dx_opt = solve_2pool_optimal_f64(x_a, y_a, x_b, y_b, f_a, f_b, phi);
    assert!(dx_opt.is_some());
    let dx = dx_opt.unwrap();
    assert!(dx > 0.0, "Optimal input size must be positive");

    let (_, gross_profit, _) = calculate_2pool_profit(dx, x_a, y_a, x_b, y_b, f_a, f_b, phi);
    assert!(gross_profit > 0.0, "Trade must be profitable");
}

#[test]
fn test_f01_04_zero_dislocation_rejection() {
    // Identical pools with identical prices must yield None (no arbitrage)
    let x_a = 1_000_000.0;
    let y_a = 400.0;
    let x_b = 1_000_000.0;
    let y_b = 400.0;
    let f_a = 0.003;
    let f_b = 0.003;
    let phi = 0.0;

    let dx_opt = solve_2pool_optimal_f64(x_a, y_a, x_b, y_b, f_a, f_b, phi);
    assert!(dx_opt.is_none(), "Zero spread must yield None");
}

#[test]
fn test_f01_05_deep_liquidity_scaling() {
    // Scaled up by 100x liquidity
    let x_a = 100_000_000.0;
    let y_a = 40_500.0;
    let x_b = 100_000_000.0;
    let y_b = 39_500.0;
    let f_a = 0.003;
    let f_b = 0.003;
    let phi = 0.0;

    let dx_opt = solve_2pool_optimal_f64(x_a, y_a, x_b, y_b, f_a, f_b, phi);
    assert!(dx_opt.is_some());
    let dx = dx_opt.unwrap();
    let rounded_dx = (dx * 100.0).round() / 100.0;

    // Sizing scales linearly with pool liquidity: 4,732.42 * 100 = 473,241.62
    assert!((rounded_dx - 473241.62).abs() < 1.0, "Sizing must scale proportionally with liquidity");
}
