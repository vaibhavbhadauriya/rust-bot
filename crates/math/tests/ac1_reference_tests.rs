//! Acceptance Criteria 1 Reference Tests
//! Authoritative source: ORIGINAL_REQUEST.md lines 77-80, PROJECT.md, and m1_explorer_3/handoff.md.

use flash_core::types::PoolReserves;
use flash_math::{
    quick_spread_screen, reduce_virtual_pool, solve_2pool_optimal, solve_3pool_triangular,
};

#[test]
fn test_ac1_v2_constant_product_reference() {
    // Pool A: 1,000,000 USDC, 405 ETH, f = 0.3% (3,000 PIPs)
    // Pool B: 1,000,000 USDC, 395 ETH, f = 0.3% (3,000 PIPs)
    let d_usdc = 1_000_000u128;
    let d_eth = 1_000_000_000_000_000_000u128;

    let pool_a = PoolReserves {
        reserve_in: 1_000_000 * d_usdc,
        reserve_out: 405 * d_eth,
        fee_pips: 3000,
    };
    let pool_b = PoolReserves {
        reserve_in: 395 * d_eth,
        reserve_out: 1_000_000 * d_usdc,
        fee_pips: 3000,
    };

    // Quick spread screening: ETH cheaper on Pool A than Pool B: yA * xB < yB * xA
    assert!(quick_spread_screen(395, 1_000_000, 405, 1_000_000));

    // 1. Solve with phi = 0% (Morpho Blue / Balancer V3)
    let res_zero_fee = solve_2pool_optimal(&pool_a, &pool_b, 0)
        .expect("Optimal sizing should find profitable arbitrage");

    let dx_usdc = res_zero_fee.optimal_input.to::<u128>() as f64 / 1e6;
    let gross_usdc = res_zero_fee.gross_profit.to::<u128>() as f64 / 1e6;

    println!(
        "V2 Constant-Product Reference: dx* = {:.4} USDC, gross profit = {:.4} USDC",
        dx_usdc, gross_usdc
    );

    // Assertions from Acceptance Criteria 1:
    // dx* = 4,732.42 USDC (+- 0.01)
    // gross profit = 45.15 USDC (+- 0.01)
    assert!(
        (dx_usdc - 4732.42).abs() <= 0.01,
        "dx* expected 4732.42 USDC, got {:.4}",
        dx_usdc
    );
    assert!(
        (gross_usdc - 45.15).abs() <= 0.01,
        "Gross profit expected 45.15 USDC, got {:.4}",
        gross_usdc
    );

    // 2. Flash loan fee phi = 0.05% (500 PIPs, Aave V3 fallback)
    // Flash fee on dx* = 4732.42 * 0.0005 = 2.3662 USDC
    // Net profit = 45.15 - 2.3662 = 42.7875 -> 42.79 USDC
    let net_with_fee = gross_usdc - (dx_usdc * 0.0005);
    assert!(
        (net_with_fee - 42.79).abs() <= 0.01,
        "Net profit after 0.05% fee expected 42.79 USDC, got {:.4}",
        net_with_fee
    );
}

#[test]
fn test_ac1_v3_slipstream_reference() {
    // UniV3: f = 0.05% (500 PIPs), P = 2495.0, L = 15,000,000
    // Slipstream: f = 0.01% (100 PIPs), P = 2505.0, L = 12,000,000
    let la = 15_000_000.0f64;
    let pa = 2495.0f64;
    let lb = 12_000_000.0f64;
    let pb = 2505.0f64;

    // Virtual reserves:
    // Pool A (UniV3): buy ETH with USDC -> input USDC (token 0), output ETH (token 1)
    let xa_usdc = (la * pa.sqrt()) * 1e6;
    let ya_eth = (la / pa.sqrt()) * 1e18;

    // Pool B (Slipstream): sell ETH for USDC -> input ETH (token 1), output USDC (token 0)
    let xb_usdc = (lb * pb.sqrt()) * 1e6;
    let yb_eth = (lb / pb.sqrt()) * 1e18;

    let pool_a = PoolReserves {
        reserve_in: xa_usdc as u128,
        reserve_out: ya_eth as u128,
        fee_pips: 500,
    };
    let pool_b = PoolReserves {
        reserve_in: yb_eth as u128,
        reserve_out: xb_usdc as u128,
        fee_pips: 100,
    };

    let res = solve_2pool_optimal(&pool_a, &pool_b, 0)
        .expect("Optimal sizing should succeed for V3/Slipstream");

    let dx_usdc = res.optimal_input.to::<u128>() as f64 / 1e6;
    let gross_usdc = res.gross_profit.to::<u128>() as f64 / 1e6;

    println!(
        "V3 Slipstream Reference: dx* = {:.4} USDC, gross profit = {:.4} USDC",
        dx_usdc, gross_usdc
    );

    // Assertions from Acceptance Criteria 1:
    // dx* = 566,245.53 USDC (+- 0.05)
    // gross profit = 963.40 USDC (+- 0.05)
    assert!(
        (dx_usdc - 566245.53).abs() <= 0.05,
        "dx* expected 566245.53 USDC, got {:.4}",
        dx_usdc
    );
    assert!(
        (gross_usdc - 963.40).abs() <= 0.05,
        "Gross profit expected 963.40 USDC, got {:.4}",
        gross_usdc
    );
}

#[test]
fn test_ac1_3hop_triangular_fuzz_1000() {
    let mut seed = 424242u64;
    let mut rng = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        seed
    };

    let fee_tiers = [100u32, 500u32, 1000u32, 3000u32];
    let mut max_relative_error = 0.0f64;
    let mut cases_tested = 0;

    while cases_tested < 1000 {
        let scale = 1_000_000u128; // Standard 6-decimal base units (e.g. USDC/USDT)
        let r1 = ((100_000 + (rng() % 9_900_000)) as u128) * scale;
        let r2 = ((100 + (rng() % 9_900)) as u128) * scale;
        let r3 = ((r2 as f64) * (0.95 + ((rng() % 100) as f64 / 1000.0))).round() as u128;
        let r4 = ((100_000 + (rng() % 9_900_000)) as u128) * scale;
        let r5 = ((r4 as f64) * (0.95 + ((rng() % 100) as f64 / 1000.0))).round() as u128;
        let r6 = ((r1 as f64) * (1.015 + ((rng() % 250) as f64 / 10000.0))).round() as u128; // Profitable spread

        let f1 = fee_tiers[(rng() % 4) as usize];
        let f2 = fee_tiers[(rng() % 4) as usize];
        let f3 = fee_tiers[(rng() % 4) as usize];

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

        let r1 = p1.reserve_in as f64;
        let r2 = p1.reserve_out as f64;
        let r3 = p2.reserve_in as f64;
        let r4 = p2.reserve_out as f64;
        let r5 = p3.reserve_in as f64;
        let r6 = p3.reserve_out as f64;

        let virt_pool = match reduce_virtual_pool(&p1, &p2) {
            Some(vp) => PoolReserves {
                reserve_in: vp.reserve_in,
                reserve_out: vp.reserve_out,
                fee_pips: vp.fee_pips,
            },
            None => continue,
        };

        let closed_form = match solve_2pool_optimal(&virt_pool, &p3, 0) {
            Some(cf) => cf.optimal_input.to::<u128>() as f64,
            None => continue,
        };

        if closed_form <= 1.0 {
            continue;
        }

        // Also test solve_3pool_triangular helper directly
        let triangular_res = solve_3pool_triangular(&p1, &p2, &p3, 0);
        assert!(triangular_res.is_some(), "3-pool triangular solver should find solution");

        // Brute-force golden section search
        let sim_3hop = |dx: f64| -> f64 {
            let g1 = 1.0 - (f1 as f64 / 1e6);
            let g2 = 1.0 - (f2 as f64 / 1e6);
            let g3 = 1.0 - (f3 as f64 / 1e6);
            let dy = (r2 * g1 * dx) / (r1 + g1 * dx);
            let dz = (r4 * g2 * dy) / (r3 + g2 * dy);
            let dx_out = (r6 * g3 * dz) / (r5 + g3 * dz);
            dx_out - dx
        };

        let mut a = 0.0f64;
        let mut b = closed_form * 2.5;
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
        let search_opt = (a + b) / 2.0;

        let rel_error = ((closed_form - search_opt).abs() / closed_form) * 100.0;
        if rel_error > max_relative_error {
            max_relative_error = rel_error;
        }

        // Tolerance check: must be within 0.001%
        assert!(
            rel_error < 0.001,
            "Relative error {:.6}% exceeded 0.001% limit at case {}",
            rel_error,
            cases_tested
        );

        cases_tested += 1;
    }

    println!(
        "AC1 3-Hop 1,000 Fuzz Cases PASSED. Max relative error: {:.7}% across {} cases",
        max_relative_error, cases_tested
    );
}
