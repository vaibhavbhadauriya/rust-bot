//! Mathematical Oracle and Specification Reference Models
//! Authoritative source: ORIGINAL_REQUEST.md lines 20-46, 78-81, and spec_miner_survey_2/handoff.md.

use alloy_primitives::U256;

/// Exact integer square root for U256 using Newton-Raphson iteration.
pub fn sqrt_u256(n: U256) -> U256 {
    flash_math::sqrt_u256(n)
}

/// Integer square root for u128.
pub fn isqrt_u128(n: u128) -> u128 {
    flash_math::sqrt_u256(U256::from(n)).to::<u128>()
}

/// 512-bit safe mulDiv: (a * b) / c with floor division.
pub fn mul_div_u256(a: U256, b: U256, c: U256) -> Option<U256> {
    flash_math::mul_div_u256(a, b, c)
}

pub fn u256_to_f64(v: U256) -> f64 {
    let limbs = v.as_limbs();
    (limbs[0] as f64)
        + (limbs[1] as f64) * 18446744073709551616.0
        + (limbs[2] as f64) * 340282366920938463463374607431768211456.0
        + (limbs[3] as f64) * 6277101735386680763835789423207666416102355444464034512896.0
}

pub fn f64_to_u256(mut f: f64) -> U256 {
    if f <= 0.0 || f.is_nan() {
        return U256::ZERO;
    }
    let mut limbs = [0u64; 4];
    for i in (0..4).rev() {
        let base = (2.0f64).powi((i * 64) as i32);
        if f >= base {
            let limb = (f / base) as u64;
            limbs[i] = limb;
            f -= (limb as f64) * base;
        }
    }
    U256::from_limbs(limbs)
}

/// Feature 1: Closed-form 2-pool optimal input amount (dx*)
/// Equation:
/// dx* = (sqrt( (xA * yA * xB * yB * gammaA * gammaB) / (1 + phi) ) - xA * yB) / (gammaA * (yB + gammaB * yA))
pub fn solve_2pool_optimal_f64(
    x_a: f64,
    y_a: f64,
    x_b: f64,
    y_b: f64,
    f_a: f64, // e.g. 0.003 for 0.3%
    f_b: f64, // e.g. 0.003 for 0.3%
    phi: f64, // e.g. 0.0005 for 0.05%
) -> Option<f64> {
    let gamma_a = 1.0 - f_a;
    let gamma_b = 1.0 - f_b;

    let numerator_inner = (x_a * y_a * x_b * y_b * gamma_a * gamma_b) / (1.0 + phi);
    if numerator_inner <= 0.0 {
        return None;
    }
    let sqrt_term = numerator_inner.sqrt();
    let numerator = sqrt_term - (x_a * y_b);
    let denominator = gamma_a * (y_b + gamma_b * y_a);

    if denominator <= 0.0 || numerator <= 0.0 {
        return None;
    }
    Some(numerator / denominator)
}

/// Feature 1: Calculate output amount and gross/net profit for 2-pool arbitrage.
pub fn calculate_2pool_profit(
    dx: f64,
    x_a: f64,
    y_a: f64,
    x_b: f64,
    y_b: f64,
    f_a: f64,
    f_b: f64,
    phi: f64,
) -> (f64, f64, f64) {
    let gamma_a = 1.0 - f_a;
    let gamma_b = 1.0 - f_b;

    let dy = (y_a * gamma_a * dx) / (x_a + gamma_a * dx);
    let dx_out = (x_b * gamma_b * dy) / (y_b + gamma_b * dy);

    let gross_profit = dx_out - dx;
    let _flash_fee = phi * dx;
    let net_profit = dx_out - (1.0 + phi) * dx;

    (dx_out, gross_profit, net_profit)
}

/// Feature 2: Radical Splitting to prevent 512-bit overflow.
/// Computes sqrtPoolA = sqrt(xA * yA) and sqrtPoolB = sqrt(xB * yB) in u128
pub fn radical_split_sizing(
    x_a: u128,
    y_a: u128,
    x_b: u128,
    y_b: u128,
    f_a_pips: u32, // 10^6 scale
    f_b_pips: u32,
    phi_pips: u32,
) -> Option<U256> {
    let pips_scale = 1_000_000u64;
    let gamma_a_pips = (pips_scale - f_a_pips as u64) as u128;
    let gamma_b_pips = (pips_scale - f_b_pips as u64) as u128;

    // Separate radical calculation avoiding 512-bit product
    let sqrt_pool_a = ((x_a as f64) * (y_a as f64)).sqrt() as u128;
    let sqrt_pool_b = ((x_b as f64) * (y_b as f64)).sqrt() as u128;
    let sqrt_fees = ((gamma_a_pips as f64) * (gamma_b_pips as f64)).sqrt() as u128;

    let p_prod = U256::from(sqrt_pool_a) * U256::from(sqrt_pool_b);
    let phi_factor = (1.0 + (phi_pips as f64 / pips_scale as f64)).sqrt();

    let term1_f = (u256_to_f64(p_prod) * (sqrt_fees as f64)) / ((pips_scale as f64) * phi_factor);
    let term1 = f64_to_u256(term1_f);

    let base_prod = U256::from(x_a) * U256::from(y_b);
    if term1 <= base_prod {
        return None;
    }
    let num = term1 - base_prod;

    let den_f = (gamma_a_pips as f64 / pips_scale as f64)
        * (y_b as f64 + (gamma_b_pips as f64 / pips_scale as f64) * y_a as f64);
    let den = f64_to_u256(den_f);
    if den.is_zero() {
        return None;
    }
    Some(num / den)
}

/// Feature 3: 3-Hop Virtual Pool Reduction (Ex, Ey)
/// Ex = (x1 * x2) / (x2 + gamma2 * y1)
/// Ey = (y1 * y2 * gamma2) / (x2 + gamma2 * y1)
/// Effective fee gamma12 = gamma1
pub fn reduce_virtual_pool_f64(
    x1: f64,
    y1: f64,
    gamma1: f64,
    x2: f64,
    y2: f64,
    gamma2: f64,
) -> Option<(f64, f64, f64)> {
    let den = x2 + gamma2 * y1;
    if den <= 0.0 {
        return None;
    }
    let ex = (x1 * x2) / den;
    let ey = (y1 * y2 * gamma2) / den;
    Some((ex, ey, gamma1))
}

/// Feature 4: Concentrated Liquidity Negative-Infinity Floor Division
pub fn div_euclid_tick(tick: i32, spacing: i32) -> (i16, u8) {
    flash_math::div_euclid_tick(tick, spacing)
}

/// Feature 4: Concentrated Liquidity Virtual Reserves Conversion (Float Oracle)
pub fn virtual_reserves_from_clmm(liquidity: f64, price: f64) -> (f64, f64) {
    let sqrt_p = price.sqrt();
    let x_virt = liquidity / sqrt_p;
    let y_virt = liquidity * sqrt_p;
    (x_virt, y_virt)
}

/// Feature 4: Concentrated Liquidity Virtual Reserves Conversion (Fixed Point)
pub fn virtual_reserves_from_clmm_u256(liquidity: u128, sqrt_price_x96: U256) -> (u128, u128) {
    flash_math::virtual_reserves_from_clmm(liquidity, sqrt_price_x96)
}

/// Feature 5: Hard Pre-Filters
/// Discard pool if R0 / 10^d0 < 1 or R1 / 10^d1 < 1
pub fn filter_unit_reserves(r0: u128, d0: u8, r1: u128, d1: u8) -> bool {
    let unit0 = 10u128.pow(d0 as u32);
    let unit1 = 10u128.pow(d1 as u32);
    r0 >= unit0 && r1 >= unit1
}

/// Feature 6: Dynamic Top-100 Pair Scoring Formula
/// Sp = (sigma / avg_fee) * ln(1 + volume) * min(L_k) * I(venues >= 2)
pub fn compute_sp_score(
    sigma: f64,
    avg_fee: f64,
    volume: f64,
    min_liquidity: f64,
    venues: usize,
) -> f64 {
    if venues < 2 {
        return 0.0;
    }
    // Prevent division by zero with epsilon floor
    let fee_denom = if avg_fee <= 1e-6 { 1e-6 } else { avg_fee };
    let vol_damped = (1.0 + volume).ln();
    (sigma / fee_denom) * vol_damped * min_liquidity
}

/// Feature 7: Zero-Division 256-Bit Multiplication Spread Check
pub fn quick_spread_screen(y_a: u128, x_b: u128, y_b: u128, x_a: u128) -> bool {
    flash_math::quick_spread_screen(y_a, x_b, y_b, x_a)
}
