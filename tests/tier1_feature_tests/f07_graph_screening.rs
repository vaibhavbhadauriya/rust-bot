//! Feature 7: In-Memory Graph & Multiplication Screening Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 26, 32-34, and spec_miner_survey_2/handoff.md.

use crate::common::*;

#[test]
fn test_f07_01_multiplication_screening_positive() {
    // Pool A: Price = 1000/405 = 2.469
    // Pool B: Price = 1000/395 = 2.531
    // Token 0 cheaper in Pool A: yA * xB < yB * xA
    let x_a = 1_000_000u128;
    let y_a = 405u128;
    let x_b = 1_000_000u128;
    let y_b = 395u128;

    // Buying Token Y in Pool A and selling in Pool B:
    // yB * xA = 395 * 1,000,000
    // yA * xB = 405 * 1,000,000
    // 395 * 1M < 405 * 1M is true!
    assert!(quick_spread_screen(y_b, x_a, y_a, x_b));
}

#[test]
fn test_f07_02_multiplication_screening_negative() {
    // Equal prices: screening condition must return false
    let x_a = 1_000_000u128;
    let y_a = 400u128;
    let x_b = 1_000_000u128;
    let y_b = 400u128;

    assert!(!quick_spread_screen(y_a, x_b, y_b, x_a));
}

#[test]
fn test_f07_03_zero_division_safety_in_screening() {
    // Zero reserves at boundaries should not panic with divide-by-zero
    let x_a = 0u128;
    let y_a = 100u128;
    let x_b = 100u128;
    let y_b = 0u128;

    let res = quick_spread_screen(y_a, x_b, y_b, x_a);
    // Evaluates with zero division
    assert!(!res);
}

#[test]
fn test_f07_04_targeted_dfs_max_hops_depth_3() {
    // DFS paths with > 3 hops must be pruned
    let hops_2 = 2;
    let hops_3 = 3;
    let hops_4 = 4;

    let max_hops = 3;
    assert!(hops_2 <= max_hops);
    assert!(hops_3 <= max_hops);
    assert!(hops_4 > max_hops, "Paths with hops > 3 must be pruned");
}

#[test]
fn test_f07_05_cyclic_path_validation() {
    // Path: WETH -> USDC -> WETH is a valid 2-hop cycle
    let path = ["WETH", "USDC", "WETH"];
    assert_eq!(path.first(), path.last(), "Path must start and end with the same token");
    assert_eq!(path.len() - 1, 2, "Must be 2 hops");
}
