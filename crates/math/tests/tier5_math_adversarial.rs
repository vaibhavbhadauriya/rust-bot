//! Tier 5 Adversarial Coverage Hardening: crates/math
//!
//! White-box stress testing of:
//! 1. sqrt_u512 Newton-Raphson convergence, termination on U512::MAX, exactness invariants
//! 2. mul_div_u256 and mul_div_u256_round_up boundary conditions and ceiling correctness
//! 3. quick_spread_screen zero-division avoidance and 256-bit multiplication invariants
//! 4. solve_2pool_optimal_raw radical splitting under extreme reserves (10^30) and edge cases
//! 5. 3-Hop virtual pool reduction and triangular closed-form solver
//! 6. CLMM tick division floor math and virtual reserve conversion

use alloy_primitives::{U256, U512};
use flash_core::types::PoolReserves;
use flash_math::{
    can_exhaust_tick, compute_max_tick_capacity, div_euclid_tick, mul_div_u256,
    mul_div_u256_round_up, quick_spread_screen, reduce_virtual_pool_raw,
    solve_2pool_optimal_raw, solve_3pool_triangular, sqrt_u512,
    virtual_reserves_from_clmm,
};

#[test]
fn test_adv_sqrt_u512_invariants_and_max_boundary() {
    // 1. Zero and small integers
    assert_eq!(sqrt_u512(U512::ZERO), U256::ZERO);
    assert_eq!(sqrt_u512(U512::from(1u64)), U256::from(1u64));
    assert_eq!(sqrt_u512(U512::from(2u64)), U256::from(1u64));
    assert_eq!(sqrt_u512(U512::from(3u64)), U256::from(1u64));
    assert_eq!(sqrt_u512(U512::from(4u64)), U256::from(2u64));
    assert_eq!(sqrt_u512(U512::from(8u64)), U256::from(2u64));
    assert_eq!(sqrt_u512(U512::from(9u64)), U256::from(3u64));

    // 2. Exact squares across powers of 2: for k in 0..128, sqrt( (2^k)^2 ) = 2^k
    for k in 0..128 {
        let root = U256::from(1u64) << k;
        let root_u512 = U512::from(root);
        let square = root_u512 * root_u512;
        assert_eq!(
            sqrt_u512(square),
            root,
            "Failed exact square recovery for root 2^{}",
            k
        );

        // square - 1 should yield root - 1 (for root > 1)
        if root > U256::from(1u64) {
            let below = square - U512::from(1u64);
            assert_eq!(
                sqrt_u512(below),
                root - U256::from(1u64),
                "Failed floor sqrt for 2^{}^2 - 1",
                k
            );
        }
    }

    // 3. Maximum U512 boundary: U512::MAX = 2^512 - 1
    // Its square root is exactly 2^256 - 1 = U256::MAX
    let max_u512 = U512::MAX;
    let sqrt_max = sqrt_u512(max_u512);
    assert_eq!(
        sqrt_max,
        U256::MAX,
        "sqrt_u512(U512::MAX) must equal U256::MAX"
    );

    // Verify mathematical invariant: (sqrt_max)^2 <= U512::MAX
    let s_wide = U512::from(sqrt_max);
    assert!(s_wide * s_wide <= max_u512);
}

#[test]
fn test_adv_mul_div_u256_precision_and_overflow_guards() {
    // 1. Division by zero returns None
    assert_eq!(mul_div_u256(U256::from(100), U256::from(100), U256::ZERO), None);
    assert_eq!(mul_div_u256_round_up(U256::from(100), U256::from(100), U256::ZERO), None);

    // 2. Rounding properties
    // 10 * 10 / 3 = 100 / 3 = 33 floor, 34 ceil
    let a = U256::from(10);
    let b = U256::from(10);
    let den = U256::from(3);
    assert_eq!(mul_div_u256(a, b, den), Some(U256::from(33)));
    assert_eq!(mul_div_u256_round_up(a, b, den), Some(U256::from(34)));

    // Exact division: 100 / 4 = 25 for both
    let den_exact = U256::from(4);
    assert_eq!(mul_div_u256(a, b, den_exact), Some(U256::from(25)));
    assert_eq!(mul_div_u256_round_up(a, b, den_exact), Some(U256::from(25)));

    // 3. Overflow: result > U256::MAX returns None
    let max_u256 = U256::MAX;
    assert_eq!(mul_div_u256(max_u256, U256::from(2), U256::from(1)), None);
    assert_eq!(mul_div_u256_round_up(max_u256, U256::from(2), U256::from(1)), None);

    // 4. Large intermediate that fits in U256 output
    // (U256::MAX * 1) / 1 = U256::MAX
    assert_eq!(mul_div_u256(max_u256, U256::from(1), U256::from(1)), Some(max_u256));
    assert_eq!(mul_div_u256_round_up(max_u256, U256::from(1), U256::from(1)), Some(max_u256));
}

#[test]
fn test_adv_quick_spread_screen_boundaries() {
    // Spread screen condition: y_A * x_B < y_B * x_A
    // Equivalent to (x_B / y_B) < (x_A / y_A), meaning price difference exists.

    // 1. Equal ratios -> false (no profit potential)
    assert!(!quick_spread_screen(100, 100, 100, 100));
    assert!(!quick_spread_screen(1_000_000, 2_000_000, 1_000_000, 2_000_000));

    // 2. Unfavorable ratio -> false
    assert!(!quick_spread_screen(200, 100, 100, 100)); // 200 * 100 = 20000, 100 * 100 = 10000

    // 3. Favorable ratio -> true
    assert!(quick_spread_screen(100, 100, 200, 100)); // 100 * 100 = 10000 < 200 * 100 = 20000

    // 4. u128::MAX boundary without 256-bit overflow
    let large_a = u128::MAX / 2;
    let large_b = u128::MAX / 3;
    // (large_a * large_b) fits in U256 (up to 2^256)
    assert!(!quick_spread_screen(large_a, large_b, large_a, large_b));
    assert!(quick_spread_screen(large_a - 1, large_b, large_a, large_b));
}

#[test]
fn test_adv_solve_2pool_extreme_and_boundary_cases() {
    // 1. Zero reserve inputs must return None gracefully without panic
    assert_eq!(solve_2pool_optimal_raw(U256::ZERO, U256::from(100), 3000, U256::from(100), U256::from(100), 3000, 0), None);
    assert_eq!(solve_2pool_optimal_raw(U256::from(100), U256::ZERO, 3000, U256::from(100), U256::from(100), 3000, 0), None);
    assert_eq!(solve_2pool_optimal_raw(U256::from(100), U256::from(100), 3000, U256::ZERO, U256::from(100), 3000, 0), None);
    assert_eq!(solve_2pool_optimal_raw(U256::from(100), U256::from(100), 3000, U256::from(100), U256::ZERO, 3000, 0), None);

    // 2. 100% or greater fees must return None
    assert_eq!(solve_2pool_optimal_raw(U256::from(1000), U256::from(1000), 1_000_000, U256::from(1000), U256::from(1000), 3000, 0), None);
    assert_eq!(solve_2pool_optimal_raw(U256::from(1000), U256::from(1000), 3000, U256::from(1000), U256::from(1000), 1_000_000, 0), None);

    // 3. Inverted / Negative spread (Pool A price higher than Pool B, no arb) returns None
    let res = solve_2pool_optimal_raw(
        U256::from(1_000_000_000_000u64), // 1M USDC
        U256::from(395_000_000_000_000_000_000u128), // 395 ETH
        3000,
        U256::from(1_000_000_000_000u64), // 1M USDC
        U256::from(405_000_000_000_000_000_000u128), // 405 ETH
        3000,
        0,
    );
    assert_eq!(res, None, "Unfavorable route must return None");

    // 4. Extreme 10^30 reserves (radical splitting prevents 256-bit overflow)
    // 10^30 ~ 2^100, prod_a ~ 10^60 ~ 2^200, within U512
    let extreme_reserve = U256::from(10u128).pow(U256::from(30));
    let extreme_ratio = solve_2pool_optimal_raw(
        extreme_reserve,
        extreme_reserve * U256::from(2),
        3000,
        extreme_reserve * U256::from(3),
        extreme_reserve,
        3000,
        0,
    );
    assert!(extreme_ratio.is_some(), "Radical splitting must handle 10^30 reserves without overflow");
    let out = extreme_ratio.unwrap();
    assert!(out.optimal_input > U256::ZERO);
    assert!(out.net_profit > U256::ZERO);
}

#[test]
fn test_adv_virtual_pool_and_triangular_solver() {
    // 1. Zero input to virtual pool reduction returns None
    assert_eq!(reduce_virtual_pool_raw(U256::ZERO, U256::from(100), 3000, U256::from(100), U256::from(100), 3000), None);
    assert_eq!(reduce_virtual_pool_raw(U256::from(100), U256::from(100), 3000, U256::ZERO, U256::from(100), 3000), None);

    // 2. Identity pools (no triangular arbitrage)
    let p1 = PoolReserves::new(1_000_000, 1_000_000, 3000);
    let p2 = PoolReserves::new(1_000_000, 1_000_000, 3000);
    let p3 = PoolReserves::new(1_000_000, 1_000_000, 3000);
    assert_eq!(solve_3pool_triangular(&p1, &p2, &p3, 0), None);

    // 3. Profitable triangular route: Token A -> B -> C -> A
    // Pool 1: 1 A = 2 B
    // Pool 2: 1 B = 2 C
    // Pool 3: 1 C = 1 A
    // Total cycle: 1 A -> 2 B -> 4 C -> 4 A (huge profit)
    let p1_rich = PoolReserves::new(10_000_000, 20_000_000, 3000);
    let p2_rich = PoolReserves::new(10_000_000, 20_000_000, 3000);
    let p3_rich = PoolReserves::new(40_000_000, 40_000_000, 3000);

    let sol = solve_3pool_triangular(&p1_rich, &p2_rich, &p3_rich, 0);
    assert!(sol.is_some());
    let trade = sol.unwrap();
    assert!(trade.optimal_input > U256::ZERO);
    assert!(trade.net_profit > U256::ZERO);
}

#[test]
fn test_adv_tick_math_and_clmm_boundaries() {
    // 1. Negative ticks floor division
    assert_eq!(div_euclid_tick(-1, 1), (-1, 255));
    assert_eq!(div_euclid_tick(-256, 1), (-1, 0));
    assert_eq!(div_euclid_tick(-257, 1), (-2, 255));
    assert_eq!(div_euclid_tick(0, 1), (0, 0));
    assert_eq!(div_euclid_tick(255, 1), (0, 255));
    assert_eq!(div_euclid_tick(256, 1), (1, 0));

    // Zero or negative spacing returns (0, 0)
    assert_eq!(div_euclid_tick(100, 0), (0, 0));
    assert_eq!(div_euclid_tick(100, -10), (0, 0));

    // 2. CLMM virtual reserves conversion
    let (x, y) = virtual_reserves_from_clmm(0, U256::from(100));
    assert_eq!(x, 0);
    assert_eq!(y, 0);

    let (x_zero_p, y_zero_p) = virtual_reserves_from_clmm(1000, U256::ZERO);
    assert_eq!(x_zero_p, 0);
    assert_eq!(y_zero_p, 0);

    // 3. compute_max_tick_capacity & can_exhaust_tick
    let l = 15_000_000u128;
    let p_curr = U256::from(1000) << 96;
    let p_next = U256::from(900) << 96;

    // zero_for_one: curr > next is valid
    let cap = compute_max_tick_capacity(l, p_curr, p_next, true);
    assert!(cap > U256::ZERO);
    assert!(can_exhaust_tick(cap, l, p_curr, p_next, true));
    assert!(!can_exhaust_tick(cap - U256::from(1), l, p_curr, p_next, true));

    // target >= current for zero_for_one returns 0 capacity
    assert_eq!(compute_max_tick_capacity(l, p_curr, p_curr, true), U256::ZERO);
    assert_eq!(compute_max_tick_capacity(l, p_next, p_curr, true), U256::ZERO);
}
