//! Empirical Stress Test Harness for Milestone 1 (R1) - V3 Virtual Reserves & CLMM Math
//! Verified by M1 Stream Challenger 1

use alloy_primitives::U256;
use flash_math::ticks::{
    can_exhaust_tick, compute_max_tick_capacity, div_euclid_tick, tick_to_word_and_bit,
    virtual_reserves_from_clmm, virtual_reserves_from_clmm_u256,
};
use std::panic::catch_unwind;

#[test]
fn test_empirical_v3_virtual_reserves_zero_boundaries() {
    // 1. Zero liquidity with non-zero sqrtPriceX96
    for sp in [
        U256::from(1u64),
        U256::from(79228162514264337593543950336u128), // P = 1.0 (2^96)
        U256::MAX,
    ] {
        let (r0, r1) = virtual_reserves_from_clmm(0, sp);
        assert_eq!(r0, 0, "Zero liquidity must give 0 reserve0");
        assert_eq!(r1, 0, "Zero liquidity must give 0 reserve1");
    }

    // 2. Zero sqrtPriceX96 with non-zero liquidity
    for liq in [1u128, 1_000_000u128, u128::MAX] {
        let res = catch_unwind(|| virtual_reserves_from_clmm(liq, U256::ZERO));
        assert!(res.is_ok(), "Zero sqrtPriceX96 must not panic (division by zero)");
        let (r0, r1) = res.unwrap();
        assert_eq!(r0, 0, "Zero sqrtPriceX96 must return 0 reserve0");
        assert_eq!(r1, 0, "Zero sqrtPriceX96 must return 0 reserve1");
    }

    // 3. Both zero
    let (r0, r1) = virtual_reserves_from_clmm(0, U256::ZERO);
    assert_eq!(r0, 0);
    assert_eq!(r1, 0);
}

#[test]
fn test_empirical_v3_virtual_reserves_extreme_values_and_saturation() {
    // 1. sqrtPriceX96 = 1 (lowest non-zero price)
    // x_virt = (L << 96) / 1
    // For L = 1: (1 << 96) / 1 = 79228162514264337593543950336
    let (r0_l1, r1_l1) = virtual_reserves_from_clmm(1, U256::from(1u64));
    assert_eq!(r0_l1, 1u128 << 96);
    assert_eq!(r1_l1, 0);

    // For L = u128::MAX, (L << 96) overflows u128; must clamp to u128::MAX without panicking
    // y_virt = (u128::MAX * 1) >> 96 = (2^128 - 1) >> 96 = 2^32 - 1 = 4294967295
    let res_large = catch_unwind(|| virtual_reserves_from_clmm(u128::MAX, U256::from(1u64)));
    assert!(res_large.is_ok(), "Overflow in x_virt must clamp without panicking");
    let (r0_max, r1_max) = res_large.unwrap();
    assert_eq!(r0_max, u128::MAX);
    assert_eq!(r1_max, (1u128 << 32) - 1);

    // 2. Maximum sqrtPriceX96 = U256::MAX
    // y_virt = (L * U256::MAX) >> 96
    // Must not panic on widening mul and must clamp safely
    let res_max_sp = catch_unwind(|| virtual_reserves_from_clmm(u128::MAX, U256::MAX));
    assert!(res_max_sp.is_ok(), "U256::MAX sqrtPrice must not panic");
    let (r0_sp_max, r1_sp_max) = res_max_sp.unwrap();
    assert_eq!(r0_sp_max, 0); // x_virt approaches 0 as price approaches infinity
    assert_eq!(r1_sp_max, u128::MAX); // clamped to u128::MAX

    // 3. U256 version preserves full precision without premature truncation
    let (x_u256, y_u256) = virtual_reserves_from_clmm_u256(1_000_000_000_000u128, U256::from(1u64) << 96);
    // At price = 1.0 (sqrtPriceX96 = 2^96): x_virt == L, y_virt == L
    assert_eq!(x_u256, U256::from(1_000_000_000_000u128));
    assert_eq!(y_u256, U256::from(1_000_000_000_000u128));
}

#[test]
fn test_empirical_v3_negative_ticks_comprehensive_matrix() {
    // Uniswap V3 valid tick range is [-887272, 887272]
    // Testing negative ticks across various tick spacings: 1, 10, 60, 200
    let test_spacings = [1, 10, 60, 200];
    let critical_ticks = [
        -887272, // MIN_TICK
        -887220,
        -100000,
        -257,
        -256,
        -255,
        -10,
        -2,
        -1,      // Must map to word_pos -1, bit_pos 255 (floor division invariant)
        0,
        1,
        2,
        255,
        256,
        887272,  // MAX_TICK
    ];

    for &spacing in &test_spacings {
        for &tick in &critical_ticks {
            let (word_pos, bit_pos) = tick_to_word_and_bit(tick, spacing);
            let (w2, b2) = div_euclid_tick(tick, spacing);
            assert_eq!(word_pos, w2);
            assert_eq!(bit_pos, b2);

            // Reconstruct compressed tick: compressed = (word_pos << 8) + bit_pos
            let reconstructed = ((word_pos as i32) * 256) + (bit_pos as i32);
            let expected_compressed = tick.div_euclid(spacing);
            assert_eq!(
                reconstructed, expected_compressed,
                "Euclidean reconstruction failed for tick={}, spacing={}",
                tick, spacing
            );
        }
    }

    // Invalid spacing checks (spacing <= 0 must return (0, 0) without panicking)
    for bad_spacing in [0, -1, -60, i32::MIN] {
        let res = catch_unwind(|| div_euclid_tick(-100, bad_spacing));
        assert!(res.is_ok(), "Invalid spacing {} must not panic", bad_spacing);
        assert_eq!(res.unwrap(), (0, 0));
    }
}

#[test]
fn test_empirical_v3_tick_capacity_boundaries() {
    // Use on-chain scale liquidity: 10^18 (1 ether of liquidity)
    let liq = 1_000_000_000_000_000_000u128;
    let p_curr = U256::from(1u64) << 96; // Price = 1.0 (7.9228e28)
    // 1% price change: delta_sqrt_p = p_curr / 200
    let delta_p = p_curr / U256::from(200u64);
    let p_target_lower = p_curr - delta_p;
    let p_target_higher = p_curr + delta_p;

    // zero_for_one = true (selling token 0, price drops from curr to target)
    let cap_0to1 = compute_max_tick_capacity(liq, p_curr, p_target_lower, true);
    assert!(cap_0to1 > U256::ZERO, "Capacity for selling token 0 must be positive");

    // If target >= curr for zero_for_one, capacity must be zero
    let cap_invalid_0to1 = compute_max_tick_capacity(liq, p_curr, p_target_higher, true);
    assert_eq!(cap_invalid_0to1, U256::ZERO);

    // zero_for_one = false (selling token 1, price rises from curr to target)
    let cap_1to0 = compute_max_tick_capacity(liq, p_curr, p_target_higher, false);
    assert!(cap_1to0 > U256::ZERO, "Capacity for selling token 1 must be positive");

    // If target <= curr for one_for_zero, capacity must be zero
    let cap_invalid_1to0 = compute_max_tick_capacity(liq, p_curr, p_target_lower, false);
    assert_eq!(cap_invalid_1to0, U256::ZERO);

    // Zero liquidity capacity must be zero
    assert_eq!(compute_max_tick_capacity(0, p_curr, p_target_lower, true), U256::ZERO);

    // can_exhaust_tick checks
    assert!(can_exhaust_tick(cap_0to1, liq, p_curr, p_target_lower, true));
    assert!(can_exhaust_tick(cap_0to1 + U256::from(1u64), liq, p_curr, p_target_lower, true));
    assert!(!can_exhaust_tick(cap_0to1 - U256::from(1u64), liq, p_curr, p_target_lower, true));
}
