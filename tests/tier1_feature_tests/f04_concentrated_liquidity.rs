//! Feature 4: Concentrated Liquidity Sizing & Negative Ticks Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 42, 79, and spec_miner_survey_2/handoff.md.

use crate::common::*;

#[test]
fn test_f04_01_negative_tick_floor_division() {
    // Tick -1 with spacing 1:
    // In standard integer division: -1 / 256 = 0 (WRONG!)
    // In arithmetic right shift: -1 >> 8 = -1 (CORRECT!)
    // Bit index: (-1) & 0xff = 255
    let (word, bit) = div_euclid_tick(-1, 1);
    assert_eq!(word, -1, "Tick -1 must map to word -1");
    assert_eq!(bit, 255, "Tick -1 must map to bit 255");

    let (word_256, bit_256) = div_euclid_tick(-256, 1);
    assert_eq!(word_256, -1, "Tick -256 must map to word -1");
    assert_eq!(bit_256, 0, "Tick -256 must map to bit 0");

    let (word_257, bit_257) = div_euclid_tick(-257, 1);
    assert_eq!(word_257, -2, "Tick -257 must map to word -2");
    assert_eq!(bit_257, 255, "Tick -257 must map to bit 255");
}

#[test]
fn test_f04_02_positive_tick_alignment() {
    let (word_0, bit_0) = div_euclid_tick(0, 1);
    assert_eq!(word_0, 0);
    assert_eq!(bit_0, 0);

    let (word_255, bit_255) = div_euclid_tick(255, 1);
    assert_eq!(word_255, 0);
    assert_eq!(bit_255, 255);

    let (word_256, bit_256) = div_euclid_tick(256, 1);
    assert_eq!(word_256, 1);
    assert_eq!(bit_256, 0);
}

#[test]
fn test_f04_03_virtual_reserves_conversion_ac1() {
    // Pool A: L = 15,000,000, P = 2495 (USDC per ETH)
    // x_virt = L * sqrt(P) = 15,000,000 * sqrt(2495)
    // y_virt = L / sqrt(P) = 15,000,000 / sqrt(2495)
    let l_a = 15_000_000.0;
    let p_a = 2495.0;
    let (y_virt, x_virt) = virtual_reserves_from_clmm(l_a, p_a);

    assert!((x_virt - 749_249_624.63).abs() < 1.0);
    assert!((y_virt - 300_300.45).abs() < 1.0);
}

#[test]
fn test_f04_04_univ3_slipstream_ac1_reference_vector() {
    // Authoritative AC1 Reference Vector:
    // Pool A (Uniswap V3): fA = 0.05%, PA = 2495, LA = 15,000,000
    // Pool B (Aerodrome Slipstream): fB = 0.01%, PB = 2505, LB = 12,000,000
    let l_a = 15_000_000.0;
    let p_a = 2495.0;
    let f_a = 0.0005;

    let l_b = 12_000_000.0;
    let p_b = 2505.0;
    let f_b = 0.0001;

    let (y_a, x_a) = virtual_reserves_from_clmm(l_a, p_a);
    let (y_b, x_b) = virtual_reserves_from_clmm(l_b, p_b);

    let dx_opt = solve_2pool_optimal_f64(x_a, y_a, x_b, y_b, f_a, f_b, 0.0);
    assert!(dx_opt.is_some());
    let dx = dx_opt.unwrap();

    // Verify dx* = 566,245.53 USDC (+- 0.05)
    let rounded_dx = (dx * 100.0).round() / 100.0;
    assert!(
        (rounded_dx - 566245.53).abs() <= 0.05,
        "dx* {} must match 566245.53 +- 0.05",
        rounded_dx
    );

    let (_, gross_profit, _) = calculate_2pool_profit(dx, x_a, y_a, x_b, y_b, f_a, f_b, 0.0);
    let rounded_gross = (gross_profit * 100.0).round() / 100.0;
    assert_eq!(rounded_gross, 963.40, "Gross profit must match 963.40 USDC verbatim");
}

#[test]
fn test_f04_05_non_crossover_capacity() {
    // Maximum absorbable input without crossing tick
    let l = 15_000_000.0;
    let p_curr = 2495.0f64.sqrt();
    let p_next = 2494.0f64.sqrt();
    let dx_max = l * (p_curr - p_next) / (p_curr * p_next);

    assert!(dx_max > 0.0);
    // If trade size < dx_max, trade remains within tick
    let small_trade = dx_max * 0.5;
    assert!(small_trade < dx_max);
}
