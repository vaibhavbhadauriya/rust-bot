use crate::closed_form::{solve_2pool_optimal_raw, TradeSizeOutput, PIP_SCALE};
use alloy_primitives::{ruint::UintTryFrom, U256, U512};
use flash_core::types::PoolReserves;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualPoolReserves {
    pub reserve_x: U256,
    pub reserve_y: U256,
    pub fee_pips: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualPool {
    pub reserve_in: u128,
    pub reserve_out: u128,
    pub fee_pips: u32,
}

/// Reduces two sequential CPMM pools into a single virtual CPMM pool:
/// E_x = (x1 * x2 * F) / (x2 * F + gamma2 * y1)
/// E_y = (y1 * y2 * gamma2) / (x2 * F + gamma2 * y1)
/// Effective fee factor is gamma1.
pub fn reduce_virtual_pool_raw(
    x1: U256,
    y1: U256,
    fee1_pips: u32,
    x2: U256,
    y2: U256,
    fee2_pips: u32,
) -> Option<VirtualPoolReserves> {
    if x1.is_zero() || y1.is_zero() || x2.is_zero() || y2.is_zero() {
        return None;
    }
    if fee1_pips >= PIP_SCALE as u32 || fee2_pips >= PIP_SCALE as u32 {
        return None;
    }
    let f = U256::from(PIP_SCALE);
    let gamma2 = U256::from(PIP_SCALE - fee2_pips as u64);

    let x2_f: U512 = x2.widening_mul(f);
    let gamma2_y1: U512 = gamma2.widening_mul(y1);
    let den = x2_f + gamma2_y1;
    if den.is_zero() {
        return None;
    }

    let x1_x2: U512 = x1.widening_mul(x2);
    let num_x = x1_x2 * U512::from(f);
    let reserve_x = U256::uint_try_from(num_x / den).ok()?;

    let y1_y2: U512 = y1.widening_mul(y2);
    let num_y = y1_y2 * U512::from(gamma2);
    let reserve_y = U256::uint_try_from(num_y / den).ok()?;

    if reserve_x.is_zero() || reserve_y.is_zero() {
        return None;
    }

    Some(VirtualPoolReserves {
        reserve_x,
        reserve_y,
        fee_pips: fee1_pips,
    })
}

pub fn reduce_virtual_pool(pool_1: &PoolReserves, pool_2: &PoolReserves) -> Option<VirtualPool> {
    let res = reduce_virtual_pool_raw(
        U256::from(pool_1.reserve_in),
        U256::from(pool_1.reserve_out),
        pool_1.fee_pips,
        U256::from(pool_2.reserve_in),
        U256::from(pool_2.reserve_out),
        pool_2.fee_pips,
    )?;

    let r_in = u128::try_from(res.reserve_x).ok()?;
    let r_out = u128::try_from(res.reserve_y).ok()?;
    if r_in == 0 || r_out == 0 {
        return None;
    }

    Some(VirtualPool {
        reserve_in: r_in,
        reserve_out: r_out,
        fee_pips: res.fee_pips,
    })
}

/// Solves a 3-hop triangular route (A -> B -> C -> A) in closed form:
/// - Pool 1: Token A -> Token B
/// - Pool 2: Token B -> Token C
/// - Pool 3: Token C -> Token A (input reserve c3, output reserve a3)
pub fn solve_3pool_triangular_raw(
    x1: U256,
    y1: U256,
    fee1_pips: u32,
    x2: U256,
    y2: U256,
    fee2_pips: u32,
    c3: U256,
    a3: U256,
    fee3_pips: u32,
    flash_fee_pips: u32,
) -> Option<TradeSizeOutput> {
    let virt = reduce_virtual_pool_raw(x1, y1, fee1_pips, x2, y2, fee2_pips)?;
    solve_2pool_optimal_raw(
        virt.reserve_x,
        virt.reserve_y,
        virt.fee_pips,
        a3,
        c3,
        fee3_pips,
        flash_fee_pips,
    )
}

pub fn solve_3pool_triangular(
    pool_1: &PoolReserves,
    pool_2: &PoolReserves,
    pool_3: &PoolReserves,
    flash_fee_pips: u32,
) -> Option<TradeSizeOutput> {
    solve_3pool_triangular_raw(
        U256::from(pool_1.reserve_in),
        U256::from(pool_1.reserve_out),
        pool_1.fee_pips,
        U256::from(pool_2.reserve_in),
        U256::from(pool_2.reserve_out),
        pool_2.fee_pips,
        U256::from(pool_3.reserve_in),
        U256::from(pool_3.reserve_out),
        pool_3.fee_pips,
        flash_fee_pips,
    )
}
