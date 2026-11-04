use alloy_primitives::{ruint::UintTryFrom, U256, U512};

/// Mandatory negative-infinity floor division for Uniswap V3 tick bitmaps:
/// Maps tick to word_pos (i16) and bit_pos (u8).
/// Correctly maps negative tick -1 with spacing 1 to word -1, bit 255.
#[inline(always)]
pub fn tick_to_word_and_bit(tick: i32, tick_spacing: i32) -> (i16, u8) {
    if tick_spacing <= 0 {
        return (0, 0);
    }
    let compressed = tick.div_euclid(tick_spacing);
    let word_pos = (compressed >> 8) as i16;
    let bit_pos = (compressed & 0xff) as u8;
    (word_pos, bit_pos)
}

/// Convenience alias matching PROJECT.md interface contract:
#[inline(always)]
pub fn div_euclid_tick(tick: i32, spacing: i32) -> (i16, u8) {
    tick_to_word_and_bit(tick, spacing)
}

/// Derives constant-product virtual reserves (x_virt, y_virt) from CLMM liquidity and sqrtPriceX96 as U256:
/// x_virt = (L << 96) / sqrtPriceX96
/// y_virt = (L * sqrtPriceX96) >> 96
pub fn virtual_reserves_from_clmm_u256(liquidity: u128, sqrt_price_x96: U256) -> (U256, U256) {
    if sqrt_price_x96.is_zero() || liquidity == 0 {
        return (U256::ZERO, U256::ZERO);
    }
    let l = U256::from(liquidity);
    let x_virt = (l << 96) / sqrt_price_x96;

    let prod_y: U512 = l.widening_mul(sqrt_price_x96);
    let y_virt_wide = prod_y >> 96;
    let y_virt = U256::uint_try_from(y_virt_wide).unwrap_or(U256::MAX);

    (x_virt, y_virt)
}

/// Derives constant-product virtual reserves (x_virt, y_virt) returning (u128, u128):
#[inline]
pub fn virtual_reserves_from_clmm(liquidity: u128, sqrt_price_x96: U256) -> (u128, u128) {
    let (x, y) = virtual_reserves_from_clmm_u256(liquidity, sqrt_price_x96);
    (
        u128::try_from(x).unwrap_or(u128::MAX),
        u128::try_from(y).unwrap_or(u128::MAX),
    )
}

/// Computes the maximum amount of input token a tick interval can absorb before crossing boundary:
/// For zero_for_one = true: Δx_max = (L * (sqrtP_curr - sqrtP_next) * 2^96) / (sqrtP_curr * sqrtP_next)
/// For zero_for_one = false: Δy_max = (L * (sqrtP_next - sqrtP_curr)) >> 96
pub fn compute_max_tick_capacity(
    liquidity: u128,
    sqrt_price_current_x96: U256,
    sqrt_price_target_x96: U256,
    zero_for_one: bool,
) -> U256 {
    if liquidity == 0 {
        return U256::ZERO;
    }
    let l = U256::from(liquidity);

    if zero_for_one {
        if sqrt_price_current_x96 <= sqrt_price_target_x96 {
            return U256::ZERO;
        }
        let delta_sqrt_p = sqrt_price_current_x96 - sqrt_price_target_x96;
        let l_delta: U512 = l.widening_mul(delta_sqrt_p);
        let num: U512 = l_delta << 96;
        let den: U512 = sqrt_price_current_x96.widening_mul(sqrt_price_target_x96);
        if den.is_zero() {
            return U256::ZERO;
        }
        let quotient = num / den;
        U256::uint_try_from(quotient).unwrap_or(U256::MAX)
    } else {
        if sqrt_price_target_x96 <= sqrt_price_current_x96 {
            return U256::ZERO;
        }
        let delta_sqrt_p = sqrt_price_target_x96 - sqrt_price_current_x96;
        let prod: U512 = l.widening_mul(delta_sqrt_p);
        let res = prod >> 96;
        U256::uint_try_from(res).unwrap_or(U256::MAX)
    }
}

/// Non-crossover check: returns true if the input amount is sufficient to exhaust the current tick range.
#[inline]
pub fn can_exhaust_tick(
    amount_in: U256,
    liquidity: u128,
    sqrt_price_current_x96: U256,
    sqrt_price_target_x96: U256,
    zero_for_one: bool,
) -> bool {
    if (zero_for_one && sqrt_price_current_x96 < sqrt_price_target_x96)
        || (!zero_for_one && sqrt_price_target_x96 < sqrt_price_current_x96)
    {
        return false;
    }
    let max_cap = compute_max_tick_capacity(liquidity, sqrt_price_current_x96, sqrt_price_target_x96, zero_for_one);
    amount_in >= max_cap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_negative_tick_floor_division() {
        let cases = [
            (-887272, 1, -3466i16, 24u8),
            (-257, 1, -2, 255),
            (-256, 1, -1, 0),
            (-1, 1, -1, 255),
            (0, 1, 0, 0),
            (255, 1, 0, 255),
            (256, 1, 1, 0),
            (887272, 1, 3465, 232),
        ];

        for (tick, spacing, exp_word, exp_bit) in cases {
            let (word_pos, bit_pos) = tick_to_word_and_bit(tick, spacing);
            assert_eq!(word_pos, exp_word, "wordPos mismatch for tick {}", tick);
            assert_eq!(bit_pos, exp_bit, "bitPos mismatch for tick {}", tick);
        }
    }
}
