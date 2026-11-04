//! Adversarial Stress Test Suite for Milestone 1 Math Solvers
//! Tasks:
//! 1. Extreme reserve imbalances (10^18 vs 10^6, 10^30 vs 10^6, 500 fuzz cases)
//! 2. Maximum uint128 reserves near 2^128 - 1 (radical splitting & zero U256 overflow)
//! 3. Zero and dust reserves (graceful None, zero panics, no division by zero)
//! 4. Comparison against numerical Golden Section and Ternary Search oracles across 2-pool and 3-hop routes
//! 5. Empirical Bug Reproduction Harness (CLMM boundary tick panic & dust virtual pool zero truncation)

use alloy_primitives::U256;
use flash_core::types::PoolReserves;
use flash_math::{
    quick_spread_screen, reduce_virtual_pool, reduce_virtual_pool_raw, solve_2pool_optimal,
    solve_3pool_triangular, virtual_reserves_from_clmm,
};
use std::panic::catch_unwind;

/// Deterministic linear congruential generator for reproducible fuzzing
struct LcgRng {
    state: u64,
}

impl LcgRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    fn gen_range_u128(&mut self, min: u128, max: u128) -> u128 {
        let span = max - min + 1;
        let r = ((self.next_u64() as u128) << 64) | (self.next_u64() as u128);
        min + (r % span)
    }
}

/// High-precision simulated 2-pool swap in f64
fn sim_2pool_profit(
    dx: f64,
    x_a: f64,
    y_a: f64,
    x_b: f64,
    y_b: f64,
    fee_a_pips: u32,
    fee_b_pips: u32,
    flash_fee_pips: u32,
) -> f64 {
    if dx <= 0.0 {
        return 0.0;
    }
    let g_a = 1.0 - (fee_a_pips as f64 / 1e6);
    let g_b = 1.0 - (fee_b_pips as f64 / 1e6);
    let phi = flash_fee_pips as f64 / 1e6;

    let dy = (y_a * g_a * dx) / (x_a + g_a * dx);
    let dx_out = (x_b * g_b * dy) / (y_b + g_b * dy);
    let repayment = (1.0 + phi) * dx;
    dx_out - repayment
}

/// Golden Section Search for optimal input dx
fn golden_section_search_2pool(
    x_a: f64,
    y_a: f64,
    x_b: f64,
    y_b: f64,
    fee_a_pips: u32,
    fee_b_pips: u32,
    flash_fee_pips: u32,
    upper_bound: f64,
) -> (f64, f64) {
    let mut a = 0.0f64;
    let mut b = upper_bound;
    let invphi = (5.0f64.sqrt() - 1.0) / 2.0;

    let mut c = b - invphi * (b - a);
    let mut d = a + invphi * (b - a);

    for _ in 0..120 {
        let f_c = sim_2pool_profit(c, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);
        let f_d = sim_2pool_profit(d, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);
        if f_c > f_d {
            b = d;
        } else {
            a = c;
        }
        c = b - invphi * (b - a);
        d = a + invphi * (b - a);
    }

    let opt_x = (a + b) / 2.0;
    let max_profit = sim_2pool_profit(opt_x, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);
    (opt_x, max_profit)
}

/// Ternary Search for optimal input dx
fn ternary_search_2pool(
    x_a: f64,
    y_a: f64,
    x_b: f64,
    y_b: f64,
    fee_a_pips: u32,
    fee_b_pips: u32,
    flash_fee_pips: u32,
    upper_bound: f64,
) -> (f64, f64) {
    let mut left = 0.0f64;
    let mut right = upper_bound;

    for _ in 0..120 {
        let m1 = left + (right - left) / 3.0;
        let m2 = right - (right - left) / 3.0;

        let p1 = sim_2pool_profit(m1, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);
        let p2 = sim_2pool_profit(m2, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);

        if p1 < p2 {
            left = m1;
        } else {
            right = m2;
        }
    }

    let opt_x = (left + right) / 2.0;
    let max_profit = sim_2pool_profit(opt_x, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);
    (opt_x, max_profit)
}

// ----------------------------------------------------------------------------
// Task 1: Extreme reserve imbalances
// ----------------------------------------------------------------------------

#[test]
fn test_stress_extreme_reserve_imbalance_18_vs_6_decimals() {
    // Token X: 18 decimals (e.g. WETH), Token Y: 6 decimals (e.g. USDC)
    // Pool A: 10,000 WETH (10^22) vs 30,000,000 USDC (3 * 10^13)
    // Pool B: 30,000,000 USDC (3 * 10^13) vs 10,200 WETH (1.02 * 10^22)
    let eth_scale = 1_000_000_000_000_000_000u128; // 10^18
    let usdc_scale = 1_000_000u128; // 10^6

    let pool_a = PoolReserves {
        reserve_in: 10_000 * eth_scale,
        reserve_out: 30_000_000 * usdc_scale,
        fee_pips: 3000,
    };
    let pool_b = PoolReserves {
        reserve_in: 30_000_000 * usdc_scale,
        reserve_out: 10_200 * eth_scale,
        fee_pips: 3000,
    };

    let opt = solve_2pool_optimal(&pool_a, &pool_b, 0)
        .expect("Should solve 18 vs 6 decimals imbalance");

    assert!(opt.optimal_input > U256::ZERO);
    assert!(opt.gross_profit > U256::ZERO);
    assert_eq!(opt.net_profit, opt.gross_profit);

    // Compare with Golden Section search
    let (gs_opt, gs_prof) = golden_section_search_2pool(
        pool_a.reserve_in as f64,
        pool_a.reserve_out as f64,
        pool_b.reserve_out as f64,
        pool_b.reserve_in as f64,
        pool_a.fee_pips,
        pool_b.fee_pips,
        0,
        (opt.optimal_input.to::<u128>() as f64) * 2.5,
    );

    let cf_opt = opt.optimal_input.to::<u128>() as f64;
    let rel_err = (cf_opt - gs_opt).abs() / cf_opt;
    assert!(
        rel_err < 0.0001,
        "Relative error between closed-form and golden section too high: {:.6}%",
        rel_err * 100.0
    );
    let cf_profit = opt.gross_profit.to::<u128>() as f64;
    let prof_rel_err = (cf_profit - gs_prof).abs() / cf_profit;
    assert!(
        prof_rel_err < 0.0001,
        "Profit mismatch too high: {:.6}%",
        prof_rel_err * 100.0
    );
}

#[test]
fn test_stress_extreme_reserve_imbalance_10_pow_30_vs_10_pow_6() {
    let pool_a = PoolReserves {
        reserve_in: 1_000_000_000_000_000_000_000_000_000_000u128, // 10^30
        reserve_out: 100_000_000_000u128,                            // 10^11
        fee_pips: 500,
    };
    let pool_b = PoolReserves {
        reserve_in: 95_000_000_000u128,                              // 5% cheaper
        reserve_out: 1_000_000_000_000_000_000_000_000_000_000u128,
        fee_pips: 500,
    };

    let opt = solve_2pool_optimal(&pool_a, &pool_b, 0)
        .expect("Should handle 10^30 vs 10^11 imbalance");

    assert!(opt.optimal_input > U256::ZERO);
    assert!(opt.gross_profit > U256::ZERO);
    assert!(opt.expected_intermediate > U256::ZERO);
    assert!(opt.expected_output > opt.optimal_input);
}

#[test]
fn test_stress_fuzz_500_extreme_imbalances() {
    let mut rng = LcgRng::new(99991);
    let fee_tiers = [100u32, 500u32, 3000u32];

    for case in 0..500 {
        let exp_x = rng.gen_range_u128(6, 32);
        let exp_y = rng.gen_range_u128(6, 32);

        let rx_base = rng.gen_range_u128(1_000, 10_000);
        let ry_base = rng.gen_range_u128(1_000, 10_000);

        let rx_a = rx_base.saturating_mul(10u128.pow(exp_x.min(30) as u32));
        let ry_a = ry_base.saturating_mul(10u128.pow(exp_y.min(30) as u32));

        if rx_a == 0 || ry_a == 0 {
            continue;
        }

        let rx_b = rx_a;
        let ry_b = (ry_a as f64 * (0.95 + (rng.next_u64() % 40) as f64 / 1000.0)).round() as u128;

        if ry_b == 0 {
            continue;
        }

        let f_a = fee_tiers[(rng.next_u64() % 3) as usize];
        let f_b = fee_tiers[(rng.next_u64() % 3) as usize];

        let pool_a = PoolReserves {
            reserve_in: rx_a,
            reserve_out: ry_a,
            fee_pips: f_a,
        };
        let pool_b = PoolReserves {
            reserve_in: ry_b,
            reserve_out: rx_b,
            fee_pips: f_b,
        };

        if let Some(opt) = solve_2pool_optimal(&pool_a, &pool_b, 0) {
            assert!(opt.optimal_input > U256::ZERO);
            assert!(opt.gross_profit > U256::ZERO, "Gross profit must be positive at case {}", case);
            assert!(opt.expected_output > opt.optimal_input);
        }
    }
}

// ----------------------------------------------------------------------------
// Task 2: Maximum uint128 reserves near 2^128 - 1 (Zero U256 Overflow)
// ----------------------------------------------------------------------------

#[test]
fn test_stress_max_u128_reserves_symmetric() {
    let max = u128::MAX; // 2^128 - 1
    let pool_a = PoolReserves {
        reserve_in: max,
        reserve_out: max,
        fee_pips: 3000,
    };
    let pool_b = PoolReserves {
        reserve_in: max,
        reserve_out: max,
        fee_pips: 3000,
    };

    let res = solve_2pool_optimal(&pool_a, &pool_b, 0);
    assert!(res.is_none(), "Symmetric max u128 pools must return None");
}

#[test]
fn test_stress_max_u128_reserves_profitable_spread() {
    let max = u128::MAX;
    let pool_a = PoolReserves {
        reserve_in: max,
        reserve_out: max,
        fee_pips: 500, // 0.05%
    };
    let pool_b = PoolReserves {
        reserve_in: (max / 100) * 98, // 2% cheaper
        reserve_out: max,
        fee_pips: 500, // 0.05%
    };

    let opt = solve_2pool_optimal(&pool_a, &pool_b, 0)
        .expect("Should solve optimal sizing near u128::MAX without overflow");

    assert!(opt.optimal_input > U256::ZERO);
    assert!(opt.gross_profit > U256::ZERO);
    assert!(opt.expected_output > opt.optimal_input);
    assert!(opt.optimal_input < U256::from(max));
}

#[test]
fn test_stress_max_u128_reserves_various_boundaries() {
    let max = u128::MAX;
    let deltas: [u128; 5] = [0, 1, 10, 1000, 1_000_000_000_000];

    for &d1 in &deltas {
        for &d2 in &deltas {
            let p_a = PoolReserves {
                reserve_in: max - d1,
                reserve_out: max - d2,
                fee_pips: 3000,
            };
            let p_b = PoolReserves {
                reserve_in: (max - d1) / 2, // 50% dislocation
                reserve_out: max - d2,
                fee_pips: 3000,
            };

            let res = solve_2pool_optimal(&p_a, &p_b, 0);
            assert!(res.is_some(), "Must succeed for max - {} / max - {}", d1, d2);
            let out = res.unwrap();
            assert!(out.optimal_input > U256::ZERO);
            assert!(out.gross_profit > U256::ZERO);
        }
    }
}

#[test]
fn test_stress_max_u128_virtual_reduction() {
    let max = u128::MAX;
    let p1 = PoolReserves {
        reserve_in: max,
        reserve_out: max,
        fee_pips: 3000,
    };
    let p2 = PoolReserves {
        reserve_in: max,
        reserve_out: max,
        fee_pips: 3000,
    };

    let vp = reduce_virtual_pool(&p1, &p2).expect("Virtual reduction should succeed at u128::MAX");
    assert!(vp.reserve_in > 0);
    assert!(vp.reserve_out > 0);
    assert!(vp.reserve_in < max);
    assert!(vp.reserve_out < max);
}

// ----------------------------------------------------------------------------
// Task 3: Zero and dust reserves (Graceful None, no panics, no div-by-zero)
// ----------------------------------------------------------------------------

#[test]
fn test_stress_zero_reserves_all_combinations() {
    let valid = 1_000_000u128;
    let zero = 0u128;

    let test_cases = [
        (zero, valid, valid, valid),
        (valid, zero, valid, valid),
        (valid, valid, zero, valid),
        (valid, valid, valid, zero),
        (zero, zero, zero, zero),
    ];

    for (xa, ya, yb, xb) in test_cases {
        let p_a = PoolReserves {
            reserve_in: xa,
            reserve_out: ya,
            fee_pips: 3000,
        };
        let p_b = PoolReserves {
            reserve_in: yb,
            reserve_out: xb,
            fee_pips: 3000,
        };

        let res = solve_2pool_optimal(&p_a, &p_b, 0);
        assert!(res.is_none(), "Zero reserve combination must return None");

        let p1 = PoolReserves {
            reserve_in: xa,
            reserve_out: ya,
            fee_pips: 3000,
        };
        let p2 = PoolReserves {
            reserve_in: yb,
            reserve_out: xb,
            fee_pips: 3000,
        };
        let virt = reduce_virtual_pool(&p1, &p2);
        if xa == 0 || ya == 0 || yb == 0 || xb == 0 {
            assert!(virt.is_none(), "Zero reserve in virtual pool must return None");
        }
    }
}

#[test]
fn test_stress_dust_reserves_solve_2pool() {
    let dust_values = [1u128, 2u128, 5u128, 10u128, 100u128];

    for &r1 in &dust_values {
        for &r2 in &dust_values {
            let p_a = PoolReserves {
                reserve_in: r1,
                reserve_out: r2,
                fee_pips: 3000,
            };
            let p_b = PoolReserves {
                reserve_in: r2,
                reserve_out: r1 * 2,
                fee_pips: 3000,
            };

            let res = solve_2pool_optimal(&p_a, &p_b, 0);
            if let Some(out) = res {
                assert!(out.optimal_input > U256::ZERO);
                assert!(out.gross_profit > U256::ZERO);
            }
        }
    }
}

#[test]
fn test_stress_fee_boundary_conditions() {
    let p_a = PoolReserves {
        reserve_in: 1_000_000,
        reserve_out: 405,
        fee_pips: 1_000_000, // 100% fee
    };
    let p_b = PoolReserves {
        reserve_in: 395,
        reserve_out: 1_000_000,
        fee_pips: 3000,
    };
    assert!(solve_2pool_optimal(&p_a, &p_b, 0).is_none(), "100% fee must return None");

    let p_a_over = PoolReserves {
        reserve_in: 1_000_000,
        reserve_out: 405,
        fee_pips: 1_000_001, // > 100% fee
    };
    assert!(solve_2pool_optimal(&p_a_over, &p_b, 0).is_none(), "> 100% fee must return None");
}

// ----------------------------------------------------------------------------
// Task 4: Comparison against Golden Section & Ternary Search Oracles
// ----------------------------------------------------------------------------

#[test]
fn test_stress_1000_cases_closed_form_vs_golden_section_and_ternary_search() {
    let mut rng = LcgRng::new(123456789);
    let fee_tiers = [100u32, 500u32, 1000u32, 3000u32];

    let mut count_profitable = 0;
    let mut max_input_rel_err_pct = 0.0f64;
    let mut max_sim_profit_err_pct = 0.0f64;

    while count_profitable < 1000 {
        let scale = 1_000_000u128; // standard 6-decimal units
        let xa = rng.gen_range_u128(10_000, 10_000_000) * scale;
        let ya = rng.gen_range_u128(100, 100_000) * scale;

        let dislocation = 1.005 + (rng.next_u64() % 750) as f64 / 10000.0;
        let xb = ((xa as f64) * dislocation).round() as u128;
        let yb = ya;

        let fa = fee_tiers[(rng.next_u64() % 4) as usize];
        let fb = fee_tiers[(rng.next_u64() % 4) as usize];
        let phi = if rng.next_u64() % 2 == 0 { 0u32 } else { 500u32 };

        let pool_a = PoolReserves {
            reserve_in: xa,
            reserve_out: ya,
            fee_pips: fa,
        };
        let pool_b = PoolReserves {
            reserve_in: yb,
            reserve_out: xb,
            fee_pips: fb,
        };

        let opt = match solve_2pool_optimal(&pool_a, &pool_b, phi) {
            Some(o) => o,
            None => continue,
        };

        let cf_dx = opt.optimal_input.to::<u128>() as f64;
        let cf_net_profit = opt.net_profit.to::<u128>() as f64;

        if cf_dx <= 10.0 || cf_net_profit <= 1.0 {
            continue;
        }

        let upper_bound = cf_dx * 2.5;
        let (gs_dx, gs_profit) = golden_section_search_2pool(
            xa as f64,
            ya as f64,
            xb as f64,
            yb as f64,
            fa,
            fb,
            phi,
            upper_bound,
        );

        let (ts_dx, _ts_profit) = ternary_search_2pool(
            xa as f64,
            ya as f64,
            xb as f64,
            yb as f64,
            fa,
            fb,
            phi,
            upper_bound,
        );

        // Oracles must agree with each other
        let oracle_diff_pct = ((gs_dx - ts_dx).abs() / gs_dx) * 100.0;
        assert!(oracle_diff_pct < 0.001, "GS and TS oracle disagreement at case {}", count_profitable);

        // Optimal input dx* comparison in percentage
        let input_rel_err_pct = ((cf_dx - gs_dx).abs() / cf_dx) * 100.0;
        if input_rel_err_pct > max_input_rel_err_pct {
            max_input_rel_err_pct = input_rel_err_pct;
        }

        // Must meet Acceptance Criteria 1 limit (< 0.001%)
        assert!(
            input_rel_err_pct < 0.001,
            "Input relative error {:.7}% exceeded 0.001% threshold at case {}",
            input_rel_err_pct,
            count_profitable
        );

        // Continuous profit comparison
        let sim_cf_profit = sim_2pool_profit(cf_dx, xa as f64, ya as f64, xb as f64, yb as f64, fa, fb, phi);
        let sim_profit_err_pct = ((sim_cf_profit - gs_profit).abs() / sim_cf_profit) * 100.0;
        if sim_profit_err_pct > max_sim_profit_err_pct {
            max_sim_profit_err_pct = sim_profit_err_pct;
        }
        assert!(
            sim_profit_err_pct < 0.0001,
            "Simulated profit error {:.7}% exceeded threshold at case {}",
            sim_profit_err_pct,
            count_profitable
        );

        count_profitable += 1;
    }

    println!(
        "Golden Section & Ternary Search 1,000 Fuzz Cases PASSED. Max dx* err: {:.7}%, max profit err: {:.8}%",
        max_input_rel_err_pct,
        max_sim_profit_err_pct
    );
}

#[test]
fn test_stress_3hop_virtual_reduction_vs_numerical_oracle_1000_cases() {
    let mut rng = LcgRng::new(55555);
    let fee_tiers = [100u32, 500u32, 1000u32, 3000u32];

    let mut cases = 0;
    let mut max_rel_err = 0.0f64;

    while cases < 1000 {
        let scale = 1_000_000u128;
        let r1 = rng.gen_range_u128(100_000, 5_000_000) * scale;
        let r2 = rng.gen_range_u128(50, 5_000) * scale;
        let r3 = (r2 as f64 * (0.96 + (rng.next_u64() % 80) as f64 / 1000.0)).round() as u128;
        let r4 = rng.gen_range_u128(100_000, 5_000_000) * scale;
        let r5 = (r4 as f64 * (0.96 + (rng.next_u64() % 80) as f64 / 1000.0)).round() as u128;
        let r6 = (r1 as f64 * (1.02 + (rng.next_u64() % 200) as f64 / 10000.0)).round() as u128;

        let f1 = fee_tiers[(rng.next_u64() % 4) as usize];
        let f2 = fee_tiers[(rng.next_u64() % 4) as usize];
        let f3 = fee_tiers[(rng.next_u64() % 4) as usize];

        let p1 = PoolReserves {
            reserve_in: r1,
            reserve_out: r2,
            fee_pips: f1,
        };
        let p2 = PoolReserves {
            reserve_in: r3,
            reserve_out: r4,
            fee_pips: f2,
        };
        let p3 = PoolReserves {
            reserve_in: r5,
            reserve_out: r6,
            fee_pips: f3,
        };

        let opt = match solve_3pool_triangular(&p1, &p2, &p3, 0) {
            Some(o) => o,
            None => continue,
        };

        let cf_dx = opt.optimal_input.to::<u128>() as f64;
        if cf_dx <= 1.0 {
            continue;
        }

        let sim_3hop = |dx: f64| -> f64 {
            let g1 = 1.0 - (f1 as f64 / 1e6);
            let g2 = 1.0 - (f2 as f64 / 1e6);
            let g3 = 1.0 - (f3 as f64 / 1e6);
            let dy = (r2 as f64 * g1 * dx) / (r1 as f64 + g1 * dx);
            let dz = (r4 as f64 * g2 * dy) / (r3 as f64 + g2 * dy);
            let dx_out = (r6 as f64 * g3 * dz) / (r5 as f64 + g3 * dz);
            dx_out - dx
        };

        let mut a = 0.0f64;
        let mut b = cf_dx * 2.5;
        let invphi = (5.0f64.sqrt() - 1.0) / 2.0;
        let mut c = b - invphi * (b - a);
        let mut d = a + invphi * (b - a);

        for _ in 0..120 {
            if sim_3hop(c) > sim_3hop(d) {
                b = d;
            } else {
                a = c;
            }
            c = b - invphi * (b - a);
            d = a + invphi * (b - a);
        }
        let gs_opt = (a + b) / 2.0;

        let rel_err = ((cf_dx - gs_opt).abs() / cf_dx) * 100.0;
        if rel_err > max_rel_err {
            max_rel_err = rel_err;
        }

        assert!(
            rel_err < 0.001,
            "3-hop relative error {:.6}% exceeded 0.001% limit at case {}",
            rel_err,
            cases
        );

        cases += 1;
    }

    println!(
        "3-Hop Virtual Reduction 1,000 Fuzz Cases PASSED. Max rel error: {:.7}%",
        max_rel_err
    );
}

// ----------------------------------------------------------------------------
// Task 5: Empirical Bug Reproduction Harnesses
// ----------------------------------------------------------------------------

/// Verified: `virtual_reserves_from_clmm` does NOT panic on canonical Uniswap V3 boundary ticks
/// and correctly saturates virtual reserves to u128::MAX on overflow.
#[test]
fn test_reproduce_bug_clmm_boundary_tick_panic() {
    let max_sqrt_ratio = U256::from_str_radix("1461446703485210103287273052203988822378723970342", 10).unwrap();
    let liquidity = 10_000_000_000_000_000_000_000u128; // 10^22 (realistic WETH liquidity)

    let panic_result = catch_unwind(|| {
        virtual_reserves_from_clmm(liquidity, max_sqrt_ratio)
    });

    assert!(
        panic_result.is_ok(),
        "virtual_reserves_from_clmm must not panic on canonical Uniswap V3 boundary tick"
    );
    let (x, y) = panic_result.unwrap();
    assert_eq!(y, u128::MAX, "y_virt should saturate to u128::MAX on overflow");
    assert!(x > 0, "x_virt should remain positive");
}

/// Verified: `reduce_virtual_pool` returns `None` on dust reserves (e.g. 1 wei) that truncate to zero,
/// preventing invalid zero-reserve VirtualPool structs from leaking to downstream callers.
#[test]
fn test_reproduce_bug_dust_reserves_virtual_pool_zero_truncation() {
    let p_a = PoolReserves {
        reserve_in: 1,
        reserve_out: 1,
        fee_pips: 3000,
    };
    let p_b = PoolReserves {
        reserve_in: 1,
        reserve_out: 1,
        fee_pips: 3000,
    };

    let virt = reduce_virtual_pool(&p_a, &p_b);
    assert!(
        virt.is_none(),
        "REMEDIATION CONFIRMED: reduce_virtual_pool returned None on 1-wei dust reserves"
    );

    let raw = reduce_virtual_pool_raw(
        U256::from(1),
        U256::from(1),
        3000,
        U256::from(1),
        U256::from(1),
        3000,
    );
    assert!(
        raw.is_none(),
        "REMEDIATION CONFIRMED: reduce_virtual_pool_raw returned None on 1-wei dust reserves"
    );

    // Verify triangular propagation of None
    let p_c = PoolReserves {
        reserve_in: 1_000_000,
        reserve_out: 1_000_000,
        fee_pips: 3000,
    };
    assert!(
        solve_3pool_triangular(&p_a, &p_b, &p_c, 0).is_none(),
        "REMEDIATION CONFIRMED: solve_3pool_triangular propagated None when virtual reduction yields None"
    );
}

#[test]
fn test_stress_quick_spread_screen_boundaries() {
    let max = u128::MAX;
    // Equal products: y_a * x_b == y_b * x_a -> false
    assert!(!quick_spread_screen(max, max, max, max));
    // Favorable dislocation: y_a * x_b < y_b * x_a -> true
    assert!(quick_spread_screen(max - 1, max, max, max));
    // Extreme asymmetry
    assert!(quick_spread_screen(1, max, 2, max));
    assert!(!quick_spread_screen(2, max, 1, max));
}
