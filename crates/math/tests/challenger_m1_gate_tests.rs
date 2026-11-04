//! Empirical Challenger Gate Tests for Milestone 1 Math Solvers
//! Adversarial verification of:
//! 1. Defect 2 remediation (dust reserves returning None instead of Some(0, 0))
//! 2. 1,000 closed-form vs Golden Section & Ternary Search (< 0.001% relative error)
//! 3. 1,000 3-hop virtual reduction vs direct numerical simulation (< 0.001% relative error)
//! 4. Extreme reserve imbalances (10^30 vs 10^6, u128::MAX vs 1) and zero-panic guarantee
//! 5. CLMM tick edge cases and saturating conversions

use alloy_primitives::U256;
use flash_core::types::PoolReserves;
use flash_math::{
    can_exhaust_tick, div_euclid_tick, reduce_virtual_pool, reduce_virtual_pool_raw,
    solve_2pool_optimal, solve_3pool_triangular, tick_to_word_and_bit, virtual_reserves_from_clmm,
};
use std::panic::catch_unwind;

/// High-quality XorShift64 PRNG for independent fuzz testing
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0xdeadbeefcafebabe } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn gen_range_u128(&mut self, min: u128, max: u128) -> u128 {
        if min >= max {
            return min;
        }
        let span = max - min + 1;
        let r = ((self.next_u64() as u128) << 64) | (self.next_u64() as u128);
        min + (r % span)
    }
}

// ----------------------------------------------------------------------------
// 1. Defect 2 Probe: Dust reserves and zero-truncation
// ----------------------------------------------------------------------------

#[test]
fn test_challenger_defect_2_dust_reserves_return_none() {
    // 1-wei symmetric dust reserves
    let p_dust1 = PoolReserves {
        reserve_in: 1,
        reserve_out: 1,
        fee_pips: 3000,
    };
    let p_dust2 = PoolReserves {
        reserve_in: 1,
        reserve_out: 1,
        fee_pips: 3000,
    };

    let virt = reduce_virtual_pool(&p_dust1, &p_dust2);
    assert!(
        virt.is_none(),
        "CRITICAL: reduce_virtual_pool must return None on 1-wei dust reserves (was Some(0, 0))"
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
        "CRITICAL: reduce_virtual_pool_raw must return None on 1-wei dust reserves"
    );

    // Test a wide matrix of dust values: (1..50) x (1..50) where integer division truncates to zero
    for r1 in 1..=20u128 {
        for r2 in 1..=20u128 {
            let p1 = PoolReserves {
                reserve_in: r1,
                reserve_out: r2,
                fee_pips: 3000,
            };
            let p2 = PoolReserves {
                reserve_in: r2,
                reserve_out: r1,
                fee_pips: 3000,
            };

            if let Some(v) = reduce_virtual_pool(&p1, &p2) {
                // If it returns Some, reserves MUST BE strictly > 0
                assert!(
                    v.reserve_in > 0,
                    "Virtual reserve_in must never be 0 when Some is returned! Got {:?}",
                    v
                );
                assert!(
                    v.reserve_out > 0,
                    "Virtual reserve_out must never be 0 when Some is returned! Got {:?}",
                    v
                );
            }
        }
    }

    // Triangular arbitrage must propagate None when reduction yields None
    let p_third = PoolReserves {
        reserve_in: 1_000_000_000,
        reserve_out: 1_000_000_000,
        fee_pips: 3000,
    };
    let tri_res = solve_3pool_triangular(&p_dust1, &p_dust2, &p_third, 0);
    assert!(
        tri_res.is_none(),
        "solve_3pool_triangular must propagate None when virtual reduction yields None"
    );
}

// ----------------------------------------------------------------------------
// 2. 1,000 Closed-Form vs Golden Section & Ternary Search Oracles
// ----------------------------------------------------------------------------

fn sim_2pool_profit_oracle(
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
    let gamma_a = 1.0 - (fee_a_pips as f64 / 1_000_000.0);
    let gamma_b = 1.0 - (fee_b_pips as f64 / 1_000_000.0);
    let phi = flash_fee_pips as f64 / 1_000_000.0;

    let dy = (y_a * gamma_a * dx) / (x_a + gamma_a * dx);
    let dx_out = (x_b * gamma_b * dy) / (y_b + gamma_b * dy);
    let repayment = (1.0 + phi) * dx;
    dx_out - repayment
}

fn golden_section_oracle(
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

    for _ in 0..128 {
        let f_c = sim_2pool_profit_oracle(c, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);
        let f_d = sim_2pool_profit_oracle(d, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);
        if f_c > f_d {
            b = d;
        } else {
            a = c;
        }
        c = b - invphi * (b - a);
        d = a + invphi * (b - a);
    }

    let opt_x = (a + b) / 2.0;
    let max_profit = sim_2pool_profit_oracle(opt_x, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);
    (opt_x, max_profit)
}

fn ternary_search_oracle(
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

    for _ in 0..128 {
        let m1 = left + (right - left) / 3.0;
        let m2 = right - (right - left) / 3.0;

        let p1 = sim_2pool_profit_oracle(m1, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);
        let p2 = sim_2pool_profit_oracle(m2, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);

        if p1 < p2 {
            left = m1;
        } else {
            right = m2;
        }
    }

    let opt_x = (left + right) / 2.0;
    let max_profit = sim_2pool_profit_oracle(opt_x, x_a, y_a, x_b, y_b, fee_a_pips, fee_b_pips, flash_fee_pips);
    (opt_x, max_profit)
}

#[test]
fn test_challenger_1000_closed_form_vs_numerical_oracles() {
    let mut rng = XorShift64::new(0x1337c0de);
    let fee_tiers = [100u32, 500u32, 1000u32, 3000u32, 10000u32];
    let flash_fee_tiers = [0u32, 500u32, 900u32];

    let mut count = 0;
    let mut max_input_err_pct = 0.0f64;

    while count < 1000 {
        let scale = 1_000_000u128; // 6 decimals
        let xa = rng.gen_range_u128(1_000, 50_000_000) * scale;
        let ya = rng.gen_range_u128(10, 500_000) * scale;

        let dislocation = 1.002 + (rng.next_u64() % 800) as f64 / 10000.0;
        let xb = ((xa as f64) * dislocation).round() as u128;
        let yb = ya;

        let fa = fee_tiers[(rng.next_u64() % fee_tiers.len() as u64) as usize];
        let fb = fee_tiers[(rng.next_u64() % fee_tiers.len() as u64) as usize];
        let phi = flash_fee_tiers[(rng.next_u64() % flash_fee_tiers.len() as u64) as usize];

        let p_a = PoolReserves {
            reserve_in: xa,
            reserve_out: ya,
            fee_pips: fa,
        };
        let p_b = PoolReserves {
            reserve_in: yb,
            reserve_out: xb,
            fee_pips: fb,
        };

        let opt = match solve_2pool_optimal(&p_a, &p_b, phi) {
            Some(o) => o,
            None => continue,
        };

        let cf_dx = opt.optimal_input.to::<u128>() as f64;
        let cf_profit = opt.net_profit.to::<u128>() as f64;

        if cf_dx <= 10.0 || cf_profit <= 1.0 {
            continue;
        }

        let upper_bound = cf_dx * 2.5;
        let (gs_dx, _gs_prof) = golden_section_oracle(
            xa as f64,
            ya as f64,
            xb as f64,
            yb as f64,
            fa,
            fb,
            phi,
            upper_bound,
        );

        let (ts_dx, _ts_prof) = ternary_search_oracle(
            xa as f64,
            ya as f64,
            xb as f64,
            yb as f64,
            fa,
            fb,
            phi,
            upper_bound,
        );

        // Cross-check oracles
        let oracle_diff_pct = ((gs_dx - ts_dx).abs() / gs_dx) * 100.0;
        assert!(oracle_diff_pct < 0.0005, "Oracles diverged at iteration {}", count);

        // Check closed form vs golden section
        let err_pct = ((cf_dx - gs_dx).abs() / cf_dx) * 100.0;
        if err_pct > max_input_err_pct {
            max_input_err_pct = err_pct;
        }

        assert!(
            err_pct < 0.001,
            "Closed-form relative error {:.6}% exceeded 0.001% limit at case {}",
            err_pct,
            count
        );

        count += 1;
    }

    println!(
        "Challenger 1,000 cases closed form vs Oracles PASSED. Max dx* relative error: {:.7}%",
        max_input_err_pct
    );
    assert!(max_input_err_pct < 0.001);
}

// ----------------------------------------------------------------------------
// 3. 1,000 3-Hop Virtual Reduction vs Numerical Oracle
// ----------------------------------------------------------------------------

#[test]
fn test_challenger_1000_3hop_virtual_reduction_vs_oracle() {
    let mut rng = XorShift64::new(0xabcdef0123456789);
    let fee_tiers = [100u32, 500u32, 1000u32, 3000u32];

    let mut cases = 0;
    let mut max_err_pct = 0.0f64;

    while cases < 1000 {
        let scale = 1_000_000u128;
        let r1 = rng.gen_range_u128(100_000, 5_000_000) * scale;
        let r2 = rng.gen_range_u128(50, 5_000) * scale;
        let r3 = (r2 as f64 * (0.97 + (rng.next_u64() % 60) as f64 / 1000.0)).round() as u128;
        let r4 = rng.gen_range_u128(100_000, 5_000_000) * scale;
        let r5 = (r4 as f64 * (0.97 + (rng.next_u64() % 60) as f64 / 1000.0)).round() as u128;
        let r6 = (r1 as f64 * (1.01 + (rng.next_u64() % 150) as f64 / 10000.0)).round() as u128;

        let f1 = fee_tiers[(rng.next_u64() % 4) as usize];
        let f2 = fee_tiers[(rng.next_u64() % 4) as usize];
        let f3 = fee_tiers[(rng.next_u64() % 4) as usize];

        let p1 = PoolReserves { reserve_in: r1, reserve_out: r2, fee_pips: f1 };
        let p2 = PoolReserves { reserve_in: r3, reserve_out: r4, fee_pips: f2 };
        let p3 = PoolReserves { reserve_in: r5, reserve_out: r6, fee_pips: f3 };

        let opt = match solve_3pool_triangular(&p1, &p2, &p3, 0) {
            Some(o) => o,
            None => continue,
        };

        let cf_dx = opt.optimal_input.to::<u128>() as f64;
        if cf_dx <= 10.0 {
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

        for _ in 0..128 {
            if sim_3hop(c) > sim_3hop(d) {
                b = d;
            } else {
                a = c;
            }
            c = b - invphi * (b - a);
            d = a + invphi * (b - a);
        }
        let gs_opt = (a + b) / 2.0;

        let err_pct = ((cf_dx - gs_opt).abs() / cf_dx) * 100.0;
        if err_pct > max_err_pct {
            max_err_pct = err_pct;
        }

        assert!(
            err_pct < 0.001,
            "3-hop relative error {:.6}% exceeded 0.001% limit at case {}",
            err_pct,
            cases
        );

        cases += 1;
    }

    println!(
        "Challenger 1,000 cases 3-hop virtual reduction PASSED. Max relative error: {:.7}%",
        max_err_pct
    );
    assert!(max_err_pct < 0.001);
}

// ----------------------------------------------------------------------------
// 4. Extreme Reserve Imbalances and Max u128 Zero Panic Stress
// ----------------------------------------------------------------------------

#[test]
fn test_challenger_extreme_reserve_imbalances_and_max_u128() {
    let max = u128::MAX;

    // 10^30 vs 10^6
    let p_huge = PoolReserves {
        reserve_in: 1_000_000_000_000_000_000_000_000_000_000u128, // 10^30
        reserve_out: 1_000_000u128,                                  // 10^6
        fee_pips: 3000,
    };
    let p_huge_b = PoolReserves {
        reserve_in: 950_000u128,                                     // 5% cheaper
        reserve_out: 1_000_000_000_000_000_000_000_000_000_000u128,
        fee_pips: 3000,
    };

    let panic_res = catch_unwind(|| {
        solve_2pool_optimal(&p_huge, &p_huge_b, 0)
    });
    assert!(panic_res.is_ok(), "10^30 vs 10^6 must not panic");
    let sol = panic_res.unwrap().expect("Should find optimal sizing for 10^30 vs 10^6");
    assert!(sol.optimal_input > U256::ZERO);
    assert!(sol.gross_profit > U256::ZERO);

    // Reversed 10^6 vs 10^30
    let panic_rev = catch_unwind(|| {
        solve_2pool_optimal(&p_huge_b, &p_huge, 0)
    });
    assert!(panic_rev.is_ok(), "Reversed 10^6 vs 10^30 must not panic");

    // Max u128 near 2^128 - 1
    let p_max_a = PoolReserves {
        reserve_in: max,
        reserve_out: max,
        fee_pips: 500,
    };
    let p_max_b = PoolReserves {
        reserve_in: (max / 100) * 98,
        reserve_out: max,
        fee_pips: 500,
    };

    let panic_max = catch_unwind(|| {
        solve_2pool_optimal(&p_max_a, &p_max_b, 0)
    });
    assert!(panic_max.is_ok(), "Max u128 sizing must not panic");
    let sol_max = panic_max.unwrap().expect("Should solve near u128::MAX");
    assert!(sol_max.optimal_input > U256::ZERO);
    assert!(sol_max.gross_profit > U256::ZERO);

    // Max u128 virtual pool reduction
    let panic_virt_max = catch_unwind(|| {
        reduce_virtual_pool(&p_max_a, &p_max_b)
    });
    assert!(panic_virt_max.is_ok(), "Virtual reduction at u128::MAX must not panic");
    let v_max = panic_virt_max.unwrap().expect("Virtual reduction should succeed at u128::MAX");
    assert!(v_max.reserve_in > 0);
    assert!(v_max.reserve_out > 0);

    // Max u128 reserves with 1-wei reserve
    let p_extreme_asym = PoolReserves {
        reserve_in: max,
        reserve_out: 1,
        fee_pips: 3000,
    };
    let p_extreme_asym_b = PoolReserves {
        reserve_in: 1,
        reserve_out: max,
        fee_pips: 3000,
    };
    let panic_asym = catch_unwind(|| {
        solve_2pool_optimal(&p_extreme_asym, &p_extreme_asym_b, 0)
    });
    assert!(panic_asym.is_ok(), "u128::MAX vs 1-wei must not panic");
}

// ----------------------------------------------------------------------------
// 5. CLMM Tick & Invariant Stress
// ----------------------------------------------------------------------------

#[test]
fn test_challenger_clmm_edge_invariants() {
    let max_sqrt_ratio = U256::from_str_radix("1461446703485210103287273052203988822378723970342", 10).unwrap();
    let min_sqrt_ratio = U256::from(4295128739u64);

    // Saturating virtual reserves at extreme ticks
    let (x_min, y_min) = virtual_reserves_from_clmm(10_000_000_000_000_000_000_000u128, min_sqrt_ratio);
    assert!(y_min > 0);
    assert_eq!(x_min, u128::MAX, "x_min should saturate to u128::MAX");

    let (x_max, y_max) = virtual_reserves_from_clmm(10_000_000_000_000_000_000_000u128, max_sqrt_ratio);
    assert!(x_max > 0);
    assert_eq!(y_max, u128::MAX, "y_max should saturate to u128::MAX");

    // Tick spacing <= 0 graceful handling
    assert_eq!(tick_to_word_and_bit(100, 0), (0, 0));
    assert_eq!(tick_to_word_and_bit(-500, -10), (0, 0));
    assert_eq!(div_euclid_tick(100, 0), (0, 0));
    assert_eq!(div_euclid_tick(-500, -10), (0, 0));

    // can_exhaust_tick inverted direction guards
    let p_low = U256::from(1000);
    let p_high = U256::from(2000);
    let l = 10_000_000u128;

    // zero_for_one expects p_curr >= p_target; if p_curr < p_target, must return false
    assert!(!can_exhaust_tick(U256::from(1_000_000), l, p_low, p_high, true));
    // one_for_zero expects p_curr <= p_target; if p_curr > p_target, must return false
    assert!(!can_exhaust_tick(U256::from(1_000_000), l, p_high, p_low, false));

    // Equal prices: capacity is 0, so amount_in >= 0 is true
    assert!(can_exhaust_tick(U256::ZERO, l, p_low, p_low, true));
    assert!(can_exhaust_tick(U256::ZERO, l, p_high, p_high, false));
}
