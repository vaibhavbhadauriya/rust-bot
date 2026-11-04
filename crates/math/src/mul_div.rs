use alloy_primitives::{ruint::UintTryFrom, U256, U512};

/// Multiplies two U256 values and divides by a third using full 512-bit intermediate precision.
/// Computes floor( (a * b) / denominator ).
#[inline]
pub fn mul_div_u256(a: U256, b: U256, denominator: U256) -> Option<U256> {
    if denominator.is_zero() {
        return None;
    }
    let prod: U512 = a.widening_mul(b);
    let den: U512 = U512::from(denominator);
    let quotient: U512 = prod / den;
    U256::uint_try_from(quotient).ok()
}

/// Multiplies two U256 values and divides by a third, rounding up to the nearest integer.
/// Computes ceil( (a * b) / denominator ).
#[inline]
pub fn mul_div_u256_round_up(a: U256, b: U256, denominator: U256) -> Option<U256> {
    if denominator.is_zero() {
        return None;
    }
    let prod: U512 = a.widening_mul(b);
    let den: U512 = U512::from(denominator);
    let quotient: U512 = prod / den;
    let remainder: U512 = prod % den;

    let final_quotient = if remainder.is_zero() {
        quotient
    } else {
        quotient + U512::from(1u64)
    };
    U256::uint_try_from(final_quotient).ok()
}

/// Fast Newton-Raphson integer square root for U256: floor(sqrt(n)).
#[inline(always)]
pub fn sqrt_u256(n: U256) -> U256 {
    if n.is_zero() {
        return U256::ZERO;
    }
    let bits = n.bit_len();
    let mut x = U256::from(1u64) << ((bits + 1) / 2);
    loop {
        let y = (x + n / x) >> 1;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// Fast Newton-Raphson integer square root for U512 returning a U256: floor(sqrt(n)).
#[inline(always)]
pub fn sqrt_u512(n: U512) -> U256 {
    if n.is_zero() {
        return U256::ZERO;
    }
    let bits = n.bit_len();
    let mut x = U512::from(1u64) << ((bits + 1) / 2);
    loop {
        let y = (x + n / x) >> 1;
        if y >= x {
            return U256::uint_try_from(x).unwrap_or(U256::MAX);
        }
        x = y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mul_div_basic() {
        let a = U256::from(100);
        let b = U256::from(200);
        let c = U256::from(50);
        assert_eq!(mul_div_u256(a, b, c), Some(U256::from(400)));
    }

    #[test]
    fn test_sqrt_basic() {
        assert_eq!(sqrt_u256(U256::from(144)), U256::from(12));
        assert_eq!(sqrt_u256(U256::from(150)), U256::from(12));
        assert_eq!(sqrt_u256(U256::ZERO), U256::ZERO);

        let u512_val = U512::from(144);
        assert_eq!(sqrt_u512(u512_val), U256::from(12));
    }
}
