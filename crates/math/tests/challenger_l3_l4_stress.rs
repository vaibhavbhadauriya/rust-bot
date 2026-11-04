//! Empirical Adversarial Stress Tests for Milestone M_R1: L3 & L4
//!
//! Objectives:
//! 1. Verify integer square root correctness and monotonicity across extreme boundaries:
//!    (0, 1, 2, 3, 4, 2^64-1, 2^128-1, 2^256-1, U512::MAX).
//! 2. Verify invariant: sqrt(n)^2 <= n < (sqrt(n)+1)^2 across all boundaries and random fuzzing.
//! 3. Verify sqrt monotonicity: sqrt(n) <= sqrt(n+1) for all tested sequences.
//! 4. Verify PIP_SCALE import and scale consistency across flash_core, closed_form, and virtual_pool.

use alloy_primitives::{U256, U512};
use flash_core::constants::PIP_SCALE as CORE_PIP_SCALE;
use flash_math::{
    closed_form::PIP_SCALE as MATH_CLOSED_FORM_PIP_SCALE, mul_div_u256, reduce_virtual_pool_raw,
    solve_2pool_optimal_raw, sqrt_u256, sqrt_u512, PIP_SCALE as MATH_PIP_SCALE,
};

/// High-performance XorShift64 PRNG for independent adversarial fuzzing
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x5eed_c0de_cafe_babe } else { seed },
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

    fn next_u256(&mut self) -> U256 {
        U256::from_limbs([
            self.next_u64(),
            self.next_u64(),
            self.next_u64(),
            self.next_u64(),
        ])
    }

    fn next_u512(&mut self) -> U512 {
        U512::from_limbs([
            self.next_u64(),
            self.next_u64(),
            self.next_u64(),
            self.next_u64(),
            self.next_u64(),
            self.next_u64(),
            self.next_u64(),
            self.next_u64(),
        ])
    }
}

// =========================================================================
// 1. Exact Boundary Tests for sqrt_u256 and sqrt_u512
// =========================================================================

#[test]
fn test_sqrt_u256_exact_boundaries() {
    // 0, 1, 2, 3, 4
    assert_eq!(sqrt_u256(U256::ZERO), U256::ZERO);
    assert_eq!(sqrt_u256(U256::from(1)), U256::from(1));
    assert_eq!(sqrt_u256(U256::from(2)), U256::from(1));
    assert_eq!(sqrt_u256(U256::from(3)), U256::from(1));
    assert_eq!(sqrt_u256(U256::from(4)), U256::from(2));

    // Perfect squares up to 100
    for i in 0..=100u64 {
        let sq = U256::from(i * i);
        assert_eq!(sqrt_u256(sq), U256::from(i), "Failed for perfect square {}", i);
        if i > 0 {
            assert_eq!(
                sqrt_u256(sq - U256::from(1)),
                U256::from(i - 1),
                "Failed for sq-1 of {}",
                i
            );
        }
        let next_sq = U256::from((i + 1) * (i + 1));
        if sq + U256::from(1) < next_sq {
            assert_eq!(
                sqrt_u256(sq + U256::from(1)),
                U256::from(i),
                "Failed for sq+1 of {}",
                i
            );
        }
    }

    // Extreme boundaries: 2^64-1, 2^128-1, 2^256-1
    let boundaries_u256 = [
        U256::from(u64::MAX),
        U256::from(u128::MAX),
        (U256::from(1) << 192) - U256::from(1),
        U256::MAX, // 2^256 - 1
    ];

    for &b in &boundaries_u256 {
        for offset in [-3i64, -2, -1, 0, 1, 2, 3] {
            let n = if offset < 0 {
                b.saturating_sub(U256::from((-offset) as u64))
            } else {
                b.saturating_add(U256::from(offset as u64))
            };

            let s = sqrt_u256(n);
            let s_wide = U512::from(s);
            let n_wide = U512::from(n);

            // Invariant: s^2 <= n
            assert!(
                s_wide * s_wide <= n_wide,
                "Invariant s^2 <= n violated for n={}: s={}",
                n,
                s
            );

            // Invariant: (s+1)^2 > n
            let s_plus_1 = s_wide + U512::from(1);
            assert!(
                s_plus_1 * s_plus_1 > n_wide,
                "Invariant (s+1)^2 > n violated for n={}: s={}",
                n,
                s
            );
        }
    }

    // Specific check for U256::MAX = 2^256 - 1
    // sqrt(2^256 - 1) must equal 2^128 - 1 = u128::MAX
    let max_sqrt = sqrt_u256(U256::MAX);
    assert_eq!(
        max_sqrt,
        U256::from(u128::MAX),
        "sqrt(2^256 - 1) must be exactly 2^128 - 1"
    );
}

#[test]
fn test_sqrt_u512_exact_boundaries() {
    // 0, 1, 2, 3, 4
    assert_eq!(sqrt_u512(U512::ZERO), U256::ZERO);
    assert_eq!(sqrt_u512(U512::from(1)), U256::from(1));
    assert_eq!(sqrt_u512(U512::from(2)), U256::from(1));
    assert_eq!(sqrt_u512(U512::from(3)), U256::from(1));
    assert_eq!(sqrt_u512(U512::from(4)), U256::from(2));

    // U512::MAX = 2^512 - 1
    // sqrt(2^512 - 1) must equal 2^256 - 1 = U256::MAX
    let max_u512_sqrt = sqrt_u512(U512::MAX);
    assert_eq!(
        max_u512_sqrt,
        U256::MAX,
        "sqrt(2^512 - 1) must be exactly U256::MAX (2^256 - 1)"
    );

    // Verify perfect squares in U512: k^2 where k is U256
    let test_roots = [
        U256::ZERO,
        U256::from(1),
        U256::from(2),
        U256::from(42),
        U256::from(u32::MAX),
        U256::from(u64::MAX),
        U256::from(u128::MAX),
        (U256::from(1) << 192) - U256::from(1),
        U256::MAX - U256::from(1),
        U256::MAX,
    ];

    for &k in &test_roots {
        let n: U512 = k.widening_mul(k);
        let s = sqrt_u512(n);
        assert_eq!(s, k, "sqrt_u512(k^2) must equal k for k={}", k);

        if k > U256::ZERO {
            let s_minus = sqrt_u512(n - U512::from(1));
            assert_eq!(
                s_minus,
                k - U256::from(1),
                "sqrt_u512(k^2 - 1) must equal k - 1 for k={}",
                k
            );
        }

        if k > U256::ZERO && k < U256::MAX {
            let s_plus = sqrt_u512(n + U512::from(1));
            assert_eq!(
                s_plus, k,
                "sqrt_u512(k^2 + 1) must equal k for k={}",
                k
            );
        }
    }

    // Boundary neighborhoods in U512
    let boundaries_u512 = [
        U512::from(u64::MAX),
        U512::from(u128::MAX),
        U512::from(U256::MAX),
        (U512::from(1) << 384) - U512::from(1),
        U512::MAX - U512::from(10),
        U512::MAX,
    ];

    for &b in &boundaries_u512 {
        for offset in [-3i64, -2, -1, 0, 1, 2, 3] {
            let n = if offset < 0 {
                b.saturating_sub(U512::from((-offset) as u64))
            } else {
                b.saturating_add(U512::from(offset as u64))
            };

            let s = sqrt_u512(n);
            let s_wide: U512 = s.widening_mul(s);

            // Invariant: s^2 <= n
            assert!(
                s_wide <= n,
                "Invariant s^2 <= n violated for U512 n={}: s={}",
                n,
                s
            );

            // Invariant: (s+1)^2 > n (unless s == U256::MAX, where (s+1) is 2^256 and (2^256)^2 = 2^512 > U512::MAX)
            if s < U256::MAX {
                let s_next = s + U256::from(1);
                let s_next_wide: U512 = s_next.widening_mul(s_next);
                assert!(
                    s_next_wide > n,
                    "Invariant (s+1)^2 > n violated for U512 n={}: s={}",
                    n,
                    s
                );
            }
        }
    }
}

// =========================================================================
// 2. Monotonicity Sweeps and Invariant Verification
// =========================================================================

#[test]
fn test_sqrt_u256_monotonicity_dense_sweep() {
    // Dense sweep from 0 to 50,000
    let mut prev_s = sqrt_u256(U256::ZERO);
    assert_eq!(prev_s, U256::ZERO);

    for i in 1..=50_000u64 {
        let n = U256::from(i);
        let s = sqrt_u256(n);

        // Monotonicity: sqrt(n) >= sqrt(n-1)
        assert!(
            s >= prev_s,
            "Monotonicity failed at n={}: s={}, prev_s={}",
            n,
            s,
            prev_s
        );

        // Invariant: s^2 <= n < (s+1)^2
        let s_u64 = s.to::<u64>();
        assert!(s_u64 * s_u64 <= i);
        assert!((s_u64 + 1) * (s_u64 + 1) > i);

        prev_s = s;
    }
}

#[test]
fn test_sqrt_u256_monotonicity_dense_near_boundaries() {
    // Dense sweep around 2^64
    let base_64 = U256::from(1) << 64;
    let mut prev_s = sqrt_u256(base_64 - U256::from(5000));
    for offset in -4999..=5000i64 {
        let n = if offset < 0 {
            base_64 - U256::from((-offset) as u64)
        } else {
            base_64 + U256::from(offset as u64)
        };
        let s = sqrt_u256(n);
        assert!(
            s >= prev_s,
            "Monotonicity failed around 2^64 at offset {}: s={}, prev_s={}",
            offset,
            s,
            prev_s
        );
        let s_wide = U512::from(s);
        let n_wide = U512::from(n);
        assert!(s_wide * s_wide <= n_wide);
        assert!((s_wide + U512::from(1)) * (s_wide + U512::from(1)) > n_wide);
        prev_s = s;
    }

    // Dense sweep around 2^128
    let base_128 = U256::from(1) << 128;
    let mut prev_s = sqrt_u256(base_128 - U256::from(5000));
    for offset in -4999..=5000i64 {
        let n = if offset < 0 {
            base_128 - U256::from((-offset) as u64)
        } else {
            base_128 + U256::from(offset as u64)
        };
        let s = sqrt_u256(n);
        assert!(
            s >= prev_s,
            "Monotonicity failed around 2^128 at offset {}: s={}, prev_s={}",
            offset,
            s,
            prev_s
        );
        let s_wide = U512::from(s);
        let n_wide = U512::from(n);
        assert!(s_wide * s_wide <= n_wide);
        assert!((s_wide + U512::from(1)) * (s_wide + U512::from(1)) > n_wide);
        prev_s = s;
    }

    // Dense sweep near U256::MAX
    let max = U256::MAX;
    let mut prev_s = sqrt_u256(max - U256::from(10000));
    for offset in 1..=10000u64 {
        let n = max - U256::from(10000 - offset);
        let s = sqrt_u256(n);
        assert!(
            s >= prev_s,
            "Monotonicity failed near U256::MAX at n={}: s={}, prev_s={}",
            n,
            s,
            prev_s
        );
        let s_wide = U512::from(s);
        let n_wide = U512::from(n);
        assert!(s_wide * s_wide <= n_wide);
        assert!((s_wide + U512::from(1)) * (s_wide + U512::from(1)) > n_wide);
        prev_s = s;
    }
}

// =========================================================================
// 3. 50,000 Iteration Random Fuzzing for Invariants and Monotonicity
// =========================================================================

#[test]
fn test_fuzz_50000_sqrt_u256_invariants() {
    let mut rng = XorShift64::new(0xdead_beef_1234_5678);
    let iterations = 50_000;

    for _ in 0..iterations {
        let n = rng.next_u256();
        let s = sqrt_u256(n);

        let s_wide = U512::from(s);
        let n_wide = U512::from(n);

        // Invariant 1: s^2 <= n
        assert!(
            s_wide * s_wide <= n_wide,
            "Fuzz failure: s^2 <= n violated for n={}: s={}",
            n,
            s
        );

        // Invariant 2: (s+1)^2 > n
        let s_plus_1 = s_wide + U512::from(1);
        assert!(
            s_plus_1 * s_plus_1 > n_wide,
            "Fuzz failure: (s+1)^2 > n violated for n={}: s={}",
            n,
            s
        );

        // Monotonicity check against n+1 (if n < U256::MAX)
        if n < U256::MAX {
            let s_next = sqrt_u256(n + U256::from(1));
            assert!(
                s_next >= s,
                "Fuzz monotonicity failed: sqrt(n+1) < sqrt(n) for n={}",
                n
            );
        }
    }
}

#[test]
fn test_fuzz_50000_sqrt_u512_invariants() {
    let mut rng = XorShift64::new(0xcafe_d00d_8765_4321);
    let iterations = 50_000;

    for _ in 0..iterations {
        let n = rng.next_u512();
        let s = sqrt_u512(n);

        let s_wide: U512 = s.widening_mul(s);

        // Invariant 1: s^2 <= n
        assert!(
            s_wide <= n,
            "Fuzz failure: s^2 <= n violated for U512 n={}: s={}",
            n,
            s
        );

        // Invariant 2: (s+1)^2 > n
        if s < U256::MAX {
            let s_next = s + U256::from(1);
            let s_next_wide: U512 = s_next.widening_mul(s_next);
            assert!(
                s_next_wide > n,
                "Fuzz failure: (s+1)^2 > n violated for U512 n={}: s={}",
                n,
                s
            );
        }

        // Monotonicity check against n+1 (if n < U512::MAX)
        if n < U512::MAX {
            let s_next = sqrt_u512(n + U512::from(1));
            assert!(
                s_next >= s,
                "Fuzz monotonicity failed: sqrt_u512(n+1) < sqrt_u512(n) for U512 n={}",
                n
            );
        }
    }
}

// =========================================================================
// 4. PIP_SCALE Import and Scale Usage Verification
// =========================================================================

#[test]
fn test_pip_scale_import_and_usage_consistency() {
    // 1. Verify PIP_SCALE definition and re-export equality
    assert_eq!(CORE_PIP_SCALE, 1_000_000u64);
    assert_eq!(MATH_CLOSED_FORM_PIP_SCALE, 1_000_000u64);
    assert_eq!(MATH_PIP_SCALE, 1_000_000u64);
    assert_eq!(CORE_PIP_SCALE, MATH_CLOSED_FORM_PIP_SCALE);
    assert_eq!(CORE_PIP_SCALE, MATH_PIP_SCALE);

    // 2. Verify fee scaling in closed_form trade sizing
    // Fee bounds: fee >= PIP_SCALE must return None
    let res = solve_2pool_optimal_raw(
        U256::from(1_000_000_000_000u128),
        U256::from(1_000_000_000_000u128),
        MATH_PIP_SCALE as u32, // 100% fee = PIP_SCALE
        U256::from(1_000_000_000_000u128),
        U256::from(1_000_000_000_000u128),
        3000,
        0,
    );
    assert!(res.is_none(), "Fee >= PIP_SCALE must return None");

    // 3. Verify fee calculation proportionality using PIP_SCALE
    // Flash fee of 500 PIPs (0.05%) on 10,000 units input:
    // fee = (10_000 * 500) / 1_000_000 = 5 units
    let opt_input = U256::from(10_000);
    let flash_fee = mul_div_u256(opt_input, U256::from(500), U256::from(MATH_PIP_SCALE)).unwrap();
    assert_eq!(flash_fee, U256::from(5));

    // Flash fee of 900 PIPs (0.09%) on 1,000,000 units input:
    // fee = (1_000_000 * 900) / 1_000_000 = 900 units
    let opt_input_large = U256::from(1_000_000);
    let flash_fee_large = mul_div_u256(
        opt_input_large,
        U256::from(900),
        U256::from(MATH_PIP_SCALE),
    )
    .unwrap();
    assert_eq!(flash_fee_large, U256::from(900));

    // 4. Verify virtual pool reduction respects PIP_SCALE
    // Fee >= PIP_SCALE must return None
    let virt_invalid = reduce_virtual_pool_raw(
        U256::from(1_000_000_000u128),
        U256::from(1_000_000_000u128),
        MATH_PIP_SCALE as u32,
        U256::from(1_000_000_000u128),
        U256::from(1_000_000_000u128),
        500,
    );
    assert!(virt_invalid.is_none(), "Virtual pool reduction with fee1 >= PIP_SCALE must return None");

    let virt_invalid2 = reduce_virtual_pool_raw(
        U256::from(1_000_000_000u128),
        U256::from(1_000_000_000u128),
        500,
        U256::from(1_000_000_000u128),
        U256::from(1_000_000_000u128),
        MATH_PIP_SCALE as u32,
    );
    assert!(virt_invalid2.is_none(), "Virtual pool reduction with fee2 >= PIP_SCALE must return None");

    // Valid virtual pool reduction with zero fee (gamma2 = PIP_SCALE)
    let virt_valid = reduce_virtual_pool_raw(
        U256::from(100_000_000u128),
        U256::from(200_000_000u128),
        3000,
        U256::from(200_000_000u128),
        U256::from(400_000_000u128),
        0, // zero fee on pool 2
    );
    assert!(virt_valid.is_some());
    let v = virt_valid.unwrap();
    assert_eq!(v.fee_pips, 3000);
    // E_x = (x1 * x2 * F) / (x2 * F + gamma2 * y1)
    // with x2 = y1, gamma2 = F: den = x2 * F + F * x2 = 2 * x2 * F.
    // num_x = x1 * x2 * F.
    // E_x = (x1 * x2 * F) / (2 * x2 * F) = x1 / 2 = 50_000_000.
    assert_eq!(v.reserve_x, U256::from(50_000_000u128));
    // E_y = (y1 * y2 * F) / (2 * x2 * F) = y2 / 2 = 200_000_000.
    assert_eq!(v.reserve_y, U256::from(200_000_000u128));
}
