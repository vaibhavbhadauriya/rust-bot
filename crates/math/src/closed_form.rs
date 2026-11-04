use crate::mul_div::{mul_div_u256, sqrt_u512};
use alloy_primitives::{ruint::UintTryFrom, U256, U512};
use flash_core::types::PoolReserves;

pub use flash_core::constants::PIP_SCALE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradeSizeOutput {
    pub optimal_input: U256,
    pub expected_intermediate: U256,
    pub expected_output: U256,
    pub gross_profit: U256,
    pub net_profit: U256,
    pub flash_fee: U256,
}

/// 256-bit multiplication spread screening: returns true iff y_A * x_B < y_B * x_A.
/// Zero integer division, microsecond-fast.
#[inline(always)]
pub fn quick_spread_screen(y_a: u128, x_b: u128, y_b: u128, x_a: u128) -> bool {
    U256::from(y_a) * U256::from(x_b) < U256::from(y_b) * U256::from(x_a)
}

/// Solves 2-pool optimal sizing:
/// Δx* = [ sqrt( (x_A * y_A * x_B * y_B * γ_A * γ_B) / (1 + φ) ) - x_A * y_B ] / [ γ_A * (y_B + γ_B * y_A) ]
///
/// Parameters:
/// - `x_a`: Pool A input reserve (Token X)
/// - `y_a`: Pool A output reserve (Token Y)
/// - `fee_a_pips`: Pool A swap fee in PIPs (10^6 scale)
/// - `x_b`: Pool B output reserve (Token X)
/// - `y_b`: Pool B input reserve (Token Y)
/// - `fee_b_pips`: Pool B swap fee in PIPs (10^6 scale)
/// - `flash_fee_pips`: Flash loan fee in PIPs (0 for Morpho/Balancer, 500 for Aave V3)
pub fn solve_2pool_optimal_raw(
    x_a: U256,
    y_a: U256,
    fee_a_pips: u32,
    x_b: U256,
    y_b: U256,
    fee_b_pips: u32,
    flash_fee_pips: u32,
) -> Option<TradeSizeOutput> {
    if x_a.is_zero() || y_a.is_zero() || x_b.is_zero() || y_b.is_zero() {
        return None;
    }
    if fee_a_pips >= PIP_SCALE as u32 || fee_b_pips >= PIP_SCALE as u32 {
        return None;
    }

    let f = U256::from(PIP_SCALE);
    let gamma_a = U256::from(PIP_SCALE - fee_a_pips as u64);
    let gamma_b = U256::from(PIP_SCALE - fee_b_pips as u64);

    // 1. Mandatory Radical Splitting:
    // sqrtPoolA = sqrt(x_A * y_A), sqrtPoolB = sqrt(x_B * y_B)
    let prod_a: U512 = x_a.widening_mul(y_a);
    let sqrt_pool_a = sqrt_u512(prod_a);

    let prod_b: U512 = x_b.widening_mul(y_b);
    let sqrt_pool_b = sqrt_u512(prod_b);

    let p_wide: U512 = sqrt_pool_a.widening_mul(sqrt_pool_b);

    // 2. Q64 Fixed-Point Fee Scaling:
    // fee_factor_q64 = floor(sqrt( (gamma_a * gamma_b * 2^128) / (F * (F + phi)) ))
    let fee_num = gamma_a * gamma_b; // fits in U256
    let fee_num_scaled: U512 = U512::from(fee_num) << 128;
    let fee_den = f * (f + U256::from(flash_fee_pips));
    let fee_ratio_q128 = fee_num_scaled / U512::from(fee_den);
    let fee_factor_q64: U256 = sqrt_u512(fee_ratio_q128);

    // 3. term1 = (P * fee_factor_q64) >> 64
    let term1_wide: U512 = (p_wide * U512::from(fee_factor_q64)) >> 64;
    let term1 = U256::uint_try_from(term1_wide).ok()?;

    // 4. Base product: x_A * y_B
    let base_prod_wide: U512 = x_a.widening_mul(y_b);
    let base_prod = U256::uint_try_from(base_prod_wide).ok()?;

    if term1 <= base_prod {
        return None;
    }
    let numerator = term1 - base_prod;

    // 5. Denominator: γ_A * (y_B + γ_B * y_A)
    // scaled by F inside integer math: den = (γ_A * (y_B * F + γ_B * y_A)) / F
    let y_b_scaled: U512 = y_b.widening_mul(f);
    let gamma_b_y_a: U512 = gamma_b.widening_mul(y_a);
    let den_inner = y_b_scaled + gamma_b_y_a; // U512
    let den_total = (den_inner * U512::from(gamma_a)) / U512::from(f); // U512

    // 6. Optimal trade size: dx* = (numerator * F) / den_total
    let num_scaled: U512 = U512::from(numerator) * U512::from(f);
    let dx_wide = num_scaled / den_total;
    let optimal_input = U256::uint_try_from(dx_wide).ok()?;

    if optimal_input.is_zero() {
        return None;
    }

    // 7. Compute outputs and profits:
    // dy = (y_A * gamma_A * dx) / (x_A * F + gamma_A * dx)
    let y_a_gamma_a: U512 = y_a.widening_mul(gamma_a);
    let num_dy = y_a_gamma_a * U512::from(optimal_input);
    let x_a_f: U512 = x_a.widening_mul(f);
    let gamma_a_dx: U512 = gamma_a.widening_mul(optimal_input);
    let den_dy = x_a_f + gamma_a_dx;
    let expected_intermediate = U256::uint_try_from(num_dy / den_dy).ok()?;

    // dx_out = (x_B * gamma_B * dy) / (y_B * F + gamma_B * dy)
    let x_b_gamma_b: U512 = x_b.widening_mul(gamma_b);
    let num_dx_out = x_b_gamma_b * U512::from(expected_intermediate);
    let y_b_f: U512 = y_b.widening_mul(f);
    let gamma_b_dy: U512 = gamma_b.widening_mul(expected_intermediate);
    let den_dx_out = y_b_f + gamma_b_dy;
    let expected_output = U256::uint_try_from(num_dx_out / den_dx_out).ok()?;

    if expected_output <= optimal_input {
        return None;
    }

    let gross_profit = expected_output - optimal_input;
    let flash_fee = mul_div_u256(optimal_input, U256::from(flash_fee_pips), f).unwrap_or(U256::ZERO);
    let repayment = optimal_input + flash_fee;

    if expected_output <= repayment {
        return None;
    }
    let net_profit = expected_output - repayment;

    Some(TradeSizeOutput {
        optimal_input,
        expected_intermediate,
        expected_output,
        gross_profit,
        net_profit,
        flash_fee,
    })
}

/// Solves 2-pool optimal sizing using PoolReserves references:
pub fn solve_2pool_optimal(
    pool_a: &PoolReserves,
    pool_b: &PoolReserves,
    flash_fee_pips: u32,
) -> Option<TradeSizeOutput> {
    solve_2pool_optimal_raw(
        U256::from(pool_a.reserve_in),
        U256::from(pool_a.reserve_out),
        pool_a.fee_pips,
        U256::from(pool_b.reserve_out),
        U256::from(pool_b.reserve_in),
        pool_b.fee_pips,
        flash_fee_pips,
    )
}
