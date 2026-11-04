//! Challenger Concentrated Liquidity Tick & Boundary Test Suite
//! Tests ticks, floor division, max capacity, exhaustion, and virtual reserves under extreme values.

use alloy_primitives::U256;
use flash_math::ticks::{
    can_exhaust_tick, compute_max_tick_capacity, div_euclid_tick, tick_to_word_and_bit,
    virtual_reserves_from_clmm, virtual_reserves_from_clmm_u256,
};

/// Uniswap V3 MIN and MAX SQRT RATIO constants
const MIN_SQRT_RATIO: u128 = 4295128739; // sqrt(1.0001^-887272) * 2^96
const MAX_SQRT_RATIO_STR: &str = "1461446703485210103287273052203988822378723970342"; // sqrt(1.0001^887272) * 2^96

// =========================================================================
// Task 1.1: Boundary Ticks Across Spacings (1, 10, 60, 200)
// =========================================================================

#[test]
fn test_boundary_ticks_across_all_spacings() {
    let boundary_ticks: [i32; 11] = [
        -887272, -887271, -257, -256, -255, -1, 0, 1, 255, 256, 887272,
    ];
    let spacings: [i32; 4] = [1, 10, 60, 200];

    for &spacing in &spacings {
        for &tick in &boundary_ticks {
            let (word_pos, bit_pos) = tick_to_word_and_bit(tick, spacing);

            // Ground truth using Euclidean division
            let compressed = tick.div_euclid(spacing);
            let expected_word = compressed.div_euclid(256) as i16;
            let expected_bit = compressed.rem_euclid(256) as u8;

            assert_eq!(
                word_pos, expected_word,
                "Word position mismatch for tick {} with spacing {}: got {}, expected {}",
                tick, spacing, word_pos, expected_word
            );
            assert_eq!(
                bit_pos, expected_bit,
                "Bit position mismatch for tick {} with spacing {}: got {}, expected {}",
                tick, spacing, bit_pos, expected_bit
            );

            // Verify div_euclid_tick alias matches tick_to_word_and_bit
            assert_eq!(
                div_euclid_tick(tick, spacing),
                (word_pos, bit_pos),
                "div_euclid_tick alias mismatch for tick {} spacing {}",
                tick, spacing
            );
        }
    }
}

#[test]
fn test_exhaustive_tick_domain_spacing_1() {
    // Exhaustively verify all 1,774,545 ticks from MIN_TICK (-887272) to MAX_TICK (887272)
    for tick in -887272..=887272 {
        let (word, bit) = tick_to_word_and_bit(tick, 1);
        let expected_word = tick.div_euclid(256) as i16;
        let expected_bit = tick.rem_euclid(256) as u8;
        assert_eq!(word, expected_word);
        assert_eq!(bit, expected_bit);
    }
}

// =========================================================================
// Task 1.2: Negative Tick Floor Division Correctness (tick >> 8 vs div_euclid)
// =========================================================================

#[test]
fn test_negative_tick_floor_division_properties() {
    // Check that for any compressed value, arithmetic right shift `>> 8`
    // strictly equals `div_euclid(256)`.
    let test_compressed_values: [i32; 15] = [
        -3466, -3465, -1000, -257, -256, -255, -2, -1, 0, 1, 255, 256, 257, 1000, 3465,
    ];

    for &c in &test_compressed_values {
        let shift_word = (c >> 8) as i16;
        let euclid_word = c.div_euclid(256) as i16;
        assert_eq!(
            shift_word, euclid_word,
            "Shift >> 8 ({}) != div_euclid(256) ({}) for c={}",
            shift_word, euclid_word, c
        );

        let mask_bit = (c & 0xff) as u8;
        let euclid_bit = c.rem_euclid(256) as u8;
        assert_eq!(
            mask_bit, euclid_bit,
            "Mask & 0xff ({}) != rem_euclid(256) ({}) for c={}",
            mask_bit, euclid_bit, c
        );
    }

    // Contrast with truncating division `/` to prove it would be WRONG
    let bad_trunc_word_neg1 = (-1i32) / 256;
    let good_floor_word_neg1 = (-1i32).div_euclid(256);
    assert_eq!(bad_trunc_word_neg1, 0); // Truncating gives 0
    assert_eq!(good_floor_word_neg1, -1); // Euclidean floor gives -1
    assert_ne!(
        bad_trunc_word_neg1, good_floor_word_neg1,
        "Truncating division is mathematically flawed for negative ticks"
    );
}

// =========================================================================
// Task 1.3: compute_max_tick_capacity and can_exhaust_tick
// =========================================================================

#[test]
fn test_capacity_and_exhaustion_zero_liquidity() {
    let p_curr = U256::from(5000) << 96;
    let p_target = U256::from(4900) << 96;

    // Zero liquidity
    let cap_z41 = compute_max_tick_capacity(0, p_curr, p_target, true);
    assert_eq!(cap_z41, U256::ZERO, "Capacity with zero liquidity must be 0");

    let cap_one4z = compute_max_tick_capacity(0, p_target, p_curr, false);
    assert_eq!(cap_one4z, U256::ZERO, "Capacity with zero liquidity must be 0");

    // Exhaustion checks
    // If liquidity is 0, any trade >= 0 can exhaust the tick
    assert!(can_exhaust_tick(U256::ZERO, 0, p_curr, p_target, true));
    assert!(can_exhaust_tick(U256::from(100), 0, p_curr, p_target, true));
}

#[test]
fn test_capacity_and_exhaustion_equal_prices() {
    let p_curr = U256::from(5000) << 96;
    let l = 15_000_000_000_000_000_000u128; // 15 ETH

    // Equal prices: zero_for_one = true
    let cap_z41_eq = compute_max_tick_capacity(l, p_curr, p_curr, true);
    assert_eq!(cap_z41_eq, U256::ZERO, "Equal prices capacity must be 0");

    // Equal prices: zero_for_one = false
    let cap_one4z_eq = compute_max_tick_capacity(l, p_curr, p_curr, false);
    assert_eq!(cap_one4z_eq, U256::ZERO, "Equal prices capacity must be 0");

    // Exhaustion checks: since capacity is 0, any trade >= 0 exhausts
    assert!(can_exhaust_tick(U256::ZERO, l, p_curr, p_curr, true), "amount 0 must exhaust equal prices (z41)");
    assert!(can_exhaust_tick(U256::from(1), l, p_curr, p_curr, true), "amount 1 must exhaust equal prices (z41)");
    assert!(can_exhaust_tick(U256::from(100), l, p_curr, p_curr, true), "amount 100 must exhaust equal prices (z41)");
    assert!(can_exhaust_tick(U256::MAX, l, p_curr, p_curr, true), "amount MAX must exhaust equal prices (z41)");

    assert!(can_exhaust_tick(U256::ZERO, l, p_curr, p_curr, false), "amount 0 must exhaust equal prices (one4z)");
    assert!(can_exhaust_tick(U256::from(1), l, p_curr, p_curr, false), "amount 1 must exhaust equal prices (one4z)");
    assert!(can_exhaust_tick(U256::from(100), l, p_curr, p_curr, false), "amount 100 must exhaust equal prices (one4z)");
    assert!(can_exhaust_tick(U256::MAX, l, p_curr, p_curr, false), "amount MAX must exhaust equal prices (one4z)");
}

#[test]
fn test_capacity_and_exhaustion_inverted_prices() {
    let p_low = U256::from(4000) << 96;
    let p_high = U256::from(5000) << 96;
    let l = 15_000_000_000_000_000_000u128;

    // zero_for_one = true moves price DOWN. Inverted condition: p_curr < p_target
    // (p_curr = p_low, p_target = p_high)
    let cap_inverted_z41 = compute_max_tick_capacity(l, p_low, p_high, true);
    assert_eq!(
        cap_inverted_z41,
        U256::ZERO,
        "Inverted prices for zero_for_one must return ZERO capacity"
    );

    assert!(!can_exhaust_tick(U256::ZERO, l, p_low, p_high, true), "can_exhaust_tick must return false for inverted prices (z41, amount 0)");
    assert!(!can_exhaust_tick(U256::from(1), l, p_low, p_high, true), "can_exhaust_tick must return false for inverted prices (z41, amount 1)");
    assert!(!can_exhaust_tick(U256::from(100), l, p_low, p_high, true), "can_exhaust_tick must return false for inverted prices (z41, amount 100)");
    assert!(!can_exhaust_tick(U256::MAX, l, p_low, p_high, true), "can_exhaust_tick must return false for inverted prices (z41, amount MAX)");

    // zero_for_one = false moves price UP. Inverted condition: p_target < p_curr
    // (p_curr = p_high, p_target = p_low)
    let cap_inverted_one4z = compute_max_tick_capacity(l, p_high, p_low, false);
    assert_eq!(
        cap_inverted_one4z,
        U256::ZERO,
        "Inverted prices for one_for_zero must return ZERO capacity"
    );
    assert!(!can_exhaust_tick(U256::ZERO, l, p_high, p_low, false), "can_exhaust_tick must return false for inverted prices (one4z, amount 0)");
    assert!(!can_exhaust_tick(U256::from(1), l, p_high, p_low, false), "can_exhaust_tick must return false for inverted prices (one4z, amount 1)");
    assert!(!can_exhaust_tick(U256::from(100), l, p_high, p_low, false), "can_exhaust_tick must return false for inverted prices (one4z, amount 100)");
    assert!(!can_exhaust_tick(U256::MAX, l, p_high, p_low, false), "can_exhaust_tick must return false for inverted prices (one4z, amount MAX)");
}

#[test]
fn test_capacity_standard_direction_values() {
    // Normal swap zero_for_one: p_curr = 2500, p_target = 2400
    // sqrt(2500) = 50, sqrt(2400) = 48.98979486
    // sqrtPriceX96 = 50 * 2^96
    let p_curr = U256::from(50) << 96;
    let p_target = U256::from(40) << 96;
    let l = 1_000_000u128;

    let cap_z41 = compute_max_tick_capacity(l, p_curr, p_target, true);
    // delta_sqrt_p = (50 - 40) * 2^96 = 10 * 2^96
    // num = L * 10 * 2^96 * 2^96 = L * 10 * 2^192
    // den = (50 * 2^96) * (40 * 2^96) = 2000 * 2^192
    // num / den = (1_000_000 * 10) / 2000 = 5_000
    assert_eq!(cap_z41, U256::from(5000));

    // Exhaustion boundary tests
    assert!(!can_exhaust_tick(U256::from(4999), l, p_curr, p_target, true));
    assert!(can_exhaust_tick(U256::from(5000), l, p_curr, p_target, true));
    assert!(can_exhaust_tick(U256::from(5001), l, p_curr, p_target, true));

    // Normal swap one_for_zero: p_curr = 40, p_target = 50
    let cap_one4z = compute_max_tick_capacity(l, p_target, p_curr, false);
    // delta_sqrt_p = 10 * 2^96
    // prod = L * 10 * 2^96 >> 96 = L * 10 = 10_000_000
    assert_eq!(cap_one4z, U256::from(10_000_000));

    assert!(!can_exhaust_tick(U256::from(9_999_999), l, p_target, p_curr, false));
    assert!(can_exhaust_tick(U256::from(10_000_000), l, p_target, p_curr, false));
    assert!(can_exhaust_tick(U256::from(10_000_001), l, p_target, p_curr, false));
}

// =========================================================================
// Task 1.4: Virtual Reserve Conversion Under Extreme sqrtPriceX96 Values
// =========================================================================

#[test]
fn test_virtual_reserves_u256_extremes() {
    let l = 10_000_000_000_000_000_000u128; // 10 ETH
    let max_sqrt_ratio = U256::from_str_radix(MAX_SQRT_RATIO_STR, 10).unwrap();
    let min_sqrt_ratio = U256::from(MIN_SQRT_RATIO);

    // 1. Zero sqrt_price or zero liquidity
    assert_eq!(virtual_reserves_from_clmm_u256(0, min_sqrt_ratio), (U256::ZERO, U256::ZERO));
    assert_eq!(virtual_reserves_from_clmm_u256(l, U256::ZERO), (U256::ZERO, U256::ZERO));

    // 2. MIN_SQRT_RATIO
    let (x_min, y_min) = virtual_reserves_from_clmm_u256(l, min_sqrt_ratio);
    assert!(!x_min.is_zero());
    // NOTE: For L = 10 ETH (10^19 wei), (L * MIN_SQRT_RATIO) >> 96 underflows to ZERO because
    // 10^19 * 4295128739 = 4.295e28 < 2^96 (7.9228e28).
    // To avoid underflow, L must be >= 1.84467e19 wei (18.45 ETH).
    assert_eq!(y_min, U256::ZERO, "Underflow: (10 ETH * MIN_SQRT_RATIO) >> 96 underflows to 0");

    let l_sufficient = 20_000_000_000_000_000_000u128; // 20 ETH
    let (x_min_suff, y_min_suff) = virtual_reserves_from_clmm_u256(l_sufficient, min_sqrt_ratio);
    assert!(!x_min_suff.is_zero());
    assert!(!y_min_suff.is_zero(), "Sufficient L produces non-zero y_virt");

    // 3. MAX_SQRT_RATIO
    let (x_max, y_max) = virtual_reserves_from_clmm_u256(l, max_sqrt_ratio);
    // NOTE: For L = 10 ETH (10^19 wei), (L << 96) / MAX_SQRT_RATIO underflows to ZERO because
    // 10^19 * 2^96 = 7.9228e47 < 1.4614e48.
    // To avoid underflow, L must be >= 2^64 (18.45 ETH).
    assert_eq!(x_max, U256::ZERO, "Underflow: (10 ETH << 96) / MAX_SQRT_RATIO underflows to 0");
    assert!(!y_max.is_zero());

    let (x_max_suff, y_max_suff) = virtual_reserves_from_clmm_u256(l_sufficient, max_sqrt_ratio);
    assert!(!x_max_suff.is_zero(), "Sufficient L produces non-zero x_virt at MAX_SQRT_RATIO");
    assert!(!y_max_suff.is_zero());

    // 4. Extreme values: sqrtPriceX96 = 1
    let (x_p1, y_p1) = virtual_reserves_from_clmm_u256(l, U256::from(1));
    assert_eq!(x_p1, U256::from(l) << 96);
    assert_eq!(y_p1, U256::ZERO); // (L * 1) >> 96 = 0 since L < 2^96

    // 5. Extreme values: sqrtPriceX96 = U256::MAX
    let (x_pmax, y_pmax) = virtual_reserves_from_clmm_u256(l, U256::MAX);
    assert_eq!(x_pmax, U256::ZERO); // (L << 96) / U256::MAX = 0
    let prod_512: alloy_primitives::U512 = U256::from(l).widening_mul(U256::MAX);
    let expected_y_pmax = prod_512 >> 96;
    use alloy_primitives::ruint::UintTryFrom;
    assert_eq!(y_pmax, U256::uint_try_from(expected_y_pmax).unwrap());
}

#[test]
fn test_virtual_reserves_u128_overflow_behavior() {
    // Investigate whether virtual_reserves_from_clmm panics on large reserves
    let min_sqrt_ratio = U256::from(MIN_SQRT_RATIO);
    let max_sqrt_ratio = U256::from_str_radix(MAX_SQRT_RATIO_STR, 10).unwrap();

    // Case A: Modest liquidity (e.g. 1 ETH = 1e18 wei)
    let modest_l = 1_000_000_000_000_000_000u128;
    let res_modest = std::panic::catch_unwind(|| {
        virtual_reserves_from_clmm(modest_l, min_sqrt_ratio)
    });
    assert!(res_modest.is_ok(), "Modest liquidity should not panic");

    // Defect 1 Verification: L = 18.5 ETH boundary condition
    // For L >= 18.5 ETH:
    // At MIN_SQRT_RATIO: x_virt = (L << 96) / MIN_SQRT_RATIO > u128::MAX, MUST saturate to u128::MAX
    // At MAX_SQRT_RATIO: y_virt = (L * MAX_SQRT_RATIO) >> 96 > u128::MAX, MUST saturate to u128::MAX
    let l_18_5_eth = 18_500_000_000_000_000_000u128; // 18.5 ETH
    let res_18_5_min = std::panic::catch_unwind(|| {
        virtual_reserves_from_clmm(l_18_5_eth, min_sqrt_ratio)
    });
    assert!(res_18_5_min.is_ok(), "L = 18.5 ETH at MIN_SQRT_RATIO must not panic");
    let (x_18_5_min, y_18_5_min) = res_18_5_min.unwrap();
    assert_eq!(x_18_5_min, u128::MAX, "x_virt must saturate to u128::MAX for L = 18.5 ETH at MIN_SQRT_RATIO");
    assert!(y_18_5_min >= 1, "y_virt must be non-zero for L = 18.5 ETH at MIN_SQRT_RATIO");

    let res_18_5_max = std::panic::catch_unwind(|| {
        virtual_reserves_from_clmm(l_18_5_eth, max_sqrt_ratio)
    });
    assert!(res_18_5_max.is_ok(), "L = 18.5 ETH at MAX_SQRT_RATIO must not panic");
    let (x_18_5_max, y_18_5_max) = res_18_5_max.unwrap();
    assert_eq!(y_18_5_max, u128::MAX, "y_virt must saturate to u128::MAX for L = 18.5 ETH at MAX_SQRT_RATIO");
    assert!(x_18_5_max >= 1, "x_virt must be non-zero for L = 18.5 ETH at MAX_SQRT_RATIO");

    // Case B: Large liquidity (e.g. 100 ETH = 1e20 wei) at MIN_SQRT_RATIO
    let large_l = 100_000_000_000_000_000_000u128;
    let res_large_min = std::panic::catch_unwind(|| {
        virtual_reserves_from_clmm(large_l, min_sqrt_ratio)
    });
    assert!(res_large_min.is_ok(), "large_l at MIN_SQRT_RATIO must not panic");
    let (x_large_min, _) = res_large_min.unwrap();
    assert_eq!(x_large_min, u128::MAX, "x_virt must saturate to u128::MAX at MIN_SQRT_RATIO for 100 ETH");

    // Case C: Large liquidity at MAX_SQRT_RATIO
    let res_large_max = std::panic::catch_unwind(|| {
        virtual_reserves_from_clmm(large_l, max_sqrt_ratio)
    });
    assert!(res_large_max.is_ok(), "large_l at MAX_SQRT_RATIO must not panic");
    let (_, y_large_max) = res_large_max.unwrap();
    assert_eq!(y_large_max, u128::MAX, "y_virt must saturate to u128::MAX at MAX_SQRT_RATIO for 100 ETH");

    // Case D: Extreme liquidity u128::MAX
    let (x_max_l_min, _) = virtual_reserves_from_clmm(u128::MAX, min_sqrt_ratio);
    assert_eq!(x_max_l_min, u128::MAX, "x_virt must saturate to u128::MAX for L = u128::MAX");
    let (_, y_max_l_max) = virtual_reserves_from_clmm(u128::MAX, max_sqrt_ratio);
    assert_eq!(y_max_l_max, u128::MAX, "y_virt must saturate to u128::MAX for L = u128::MAX");

    // Case E: Extreme sqrtPriceX96 = 1
    let res_p1 = std::panic::catch_unwind(|| {
        virtual_reserves_from_clmm(modest_l, U256::from(1))
    });
    assert!(res_p1.is_ok(), "sqrtPriceX96=1 must not panic");
    let (x_p1, _) = res_p1.unwrap();
    assert_eq!(x_p1, u128::MAX, "x_virt must saturate to u128::MAX when sqrtPriceX96 = 1");

    // Case F: Extreme sqrtPriceX96 = U256::MAX
    let res_pmax = std::panic::catch_unwind(|| {
        virtual_reserves_from_clmm(modest_l, U256::MAX)
    });
    assert!(res_pmax.is_ok(), "sqrtPriceX96=U256::MAX must not panic");
    let (_, y_pmax) = res_pmax.unwrap();
    assert_eq!(y_pmax, u128::MAX, "y_virt must saturate to u128::MAX when sqrtPriceX96 = U256::MAX");
}

#[test]
fn test_tick_spacing_non_positive_graceful() {
    let test_spacings = [0, -1, -10, -256, i32::MIN];
    let test_ticks = [0, 100, -100, 887272, -887272, i32::MAX, i32::MIN];

    for &spacing in &test_spacings {
        for &tick in &test_ticks {
            let res = std::panic::catch_unwind(|| tick_to_word_and_bit(tick, spacing));
            assert!(res.is_ok(), "tick_to_word_and_bit must not panic on tick {} spacing {}", tick, spacing);
            assert_eq!(res.unwrap(), (0, 0), "tick_to_word_and_bit must return (0, 0) on spacing <= 0");

            let res_alias = std::panic::catch_unwind(|| div_euclid_tick(tick, spacing));
            assert!(res_alias.is_ok(), "div_euclid_tick must not panic on tick {} spacing {}", tick, spacing);
            assert_eq!(res_alias.unwrap(), (0, 0), "div_euclid_tick must return (0, 0) on spacing <= 0");
        }
    }
}
