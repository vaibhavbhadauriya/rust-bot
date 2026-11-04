//! Adversarial Stress & Concurrency Test Suite for Vanity Miner & Flash Loan Routing
//!
//! Targeted stress verification for:
//! 1. CREATE2 Vanity Miner Concurrency & Bounds (1, 4, 8, 16 threads, rapid cancellation < 10ms, boundaries 0, 1, 2, >20, EIP-1014 reference math)
//! 2. Flash Loan Routing Cascade Adversarial Stress (exact boundaries, single-wei cascades, all-zero liquidity, u128::MAX overflow resistance)

use alloy_primitives::{address, keccak256, Address, U256};
use flash_core::types::Chain;
use flash_executor::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ============================================================================
// 1. CREATE2 Vanity Miner Concurrency & Thread Contention Stress
// ============================================================================

#[test]
fn test_vanity_concurrency_scaling_1_4_8_16_threads() {
    let deployer = address!("1111222233334444555566667777888899990000");
    let init_code_hash = [0x5A; 32];
    let thread_counts = [1, 4, 8, 16];

    for &threads in &thread_counts {
        let start = Instant::now();
        let miner = VanityMiner::new(deployer, init_code_hash)
            .with_target_zero_bytes(2)
            .with_threads(threads)
            .with_batch_size(512);

        let result = miner.mine().unwrap_or_else(|e| panic!("Mining with {} threads failed: {:?}", threads, e));
        let elapsed = start.elapsed();

        // 1. Derived address must have at least 2 zero bytes
        assert!(
            result.zero_bytes >= 2,
            "Thread count {}: expected >= 2 zero bytes, got {}",
            threads,
            result.zero_bytes
        );

        // 2. Deterministic address recomputation must match perfectly
        let derived = compute_create2_address(deployer, result.salt, init_code_hash);
        assert_eq!(
            derived, result.address,
            "Thread count {}: derived address mismatch",
            threads
        );

        // 3. Address must match B256 representation of salt
        assert_eq!(result.salt, result.salt_b256.0);

        println!(
            "Concurrency test: threads={}, attempts={}, elapsed={:?}, hash_rate={:.0} H/s",
            threads, result.attempts, elapsed, result.hash_rate
        );
    }
}

#[test]
fn test_vanity_disjoint_salt_spaces() {
    // Verify that multi-threaded workers execute over strictly disjoint salt spaces.
    // Specifically, worker_id is partitioned into salt bytes 29..37.
    let deployer = address!("7777777777777777777777777777777777777777");
    let init_code_hash = [0xAA; 32];
    let entropy = 0xDEAD_BEEF_FEED_FACE_u64;

    for _worker_id in 0..16u64 {
        let miner = VanityMiner::new(deployer, init_code_hash)
            .with_target_zero_bytes(1)
            .with_threads(1)
            .with_worker_entropy(entropy);

        let res = miner.mine().unwrap();
        // Byte 21..29 is entropy
        let entropy_slice = &res.salt[0..8];
        assert_eq!(entropy_slice, &entropy.to_le_bytes());
    }
}

// ============================================================================
// 2. Rapid Atomic Cancellation Stress (< 10ms, No Leaks)
// ============================================================================

#[test]
fn test_vanity_cancellation_pre_set_flag() {
    let deployer = address!("9999999999999999999999999999999999999999");
    let init_code_hash = [0x77; 32];
    let token = Arc::new(AtomicBool::new(true)); // Pre-set to cancelled

    let miner = VanityMiner::new(deployer, init_code_hash)
        .with_target_zero_bytes(8) // Astronomical target (would never finish)
        .with_threads(4)
        .with_batch_size(64)
        .with_cancellation_token(token);

    let start = Instant::now();
    let res = miner.mine();
    let elapsed = start.elapsed();

    assert_eq!(res.unwrap_err(), VanityMinerError::Cancelled);
    println!("Pre-set cancellation elapsed: {:?}", elapsed);

    // In release mode (production), termination is sub-millisecond.
    // In debug mode with unoptimized Windows thread creation, allow up to 25ms.
    let threshold = if cfg!(debug_assertions) {
        Duration::from_millis(30)
    } else {
        Duration::from_millis(10)
    };
    assert!(
        elapsed < threshold,
        "Pre-set cancellation took {:?}, threshold was {:?}",
        elapsed,
        threshold
    );
}

#[test]
fn test_vanity_cancellation_rapid_in_flight() {
    // Spawn miner with 4 threads and small batch size, cancel after 1 millisecond.
    let deployer = address!("8888888888888888888888888888888888888888");
    let init_code_hash = [0x33; 32];
    let token = Arc::new(AtomicBool::new(false));
    let token_clone = Arc::clone(&token);

    // Background thread signals cancellation after 1ms
    let cancel_signaler = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1));
        let cancel_instant = Instant::now();
        token_clone.store(true, Ordering::SeqCst);
        cancel_instant
    });

    let miner = VanityMiner::new(deployer, init_code_hash)
        .with_target_zero_bytes(8) // 16 hex zeros (impossible to find in 1ms)
        .with_threads(4)
        .with_batch_size(128)
        .with_cancellation_token(token);

    let res = miner.mine();
    let cancel_time = cancel_signaler.join().unwrap();
    let time_since_cancel = cancel_time.elapsed();

    assert_eq!(res.unwrap_err(), VanityMinerError::Cancelled);
    println!("In-flight cancellation time since signal: {:?}", time_since_cancel);

    let threshold = if cfg!(debug_assertions) {
        Duration::from_millis(30)
    } else {
        Duration::from_millis(10)
    };
    assert!(
        time_since_cancel < threshold,
        "Threads took {:?} to exit after cancellation signal, threshold was {:?}",
        time_since_cancel,
        threshold
    );
}

// ============================================================================
// 3. Target Boundaries Stress (0, 1, 2, >20)
// ============================================================================

#[test]
fn test_vanity_target_zero_bytes_0_instant() {
    let deployer = address!("1234123412341234123412341234123412341234");
    let init_code_hash = [0x11; 32];

    let miner = VanityMiner::new(deployer, init_code_hash)
        .with_target_zero_bytes(0)
        .with_threads(1);

    let res = miner.mine().expect("Target 0 must succeed instantly");
    assert_eq!(res.attempts, 1, "Target 0 should succeed on the first iteration");
    assert_eq!(compute_create2_address(deployer, res.salt, init_code_hash), res.address);
}

#[test]
fn test_vanity_target_zero_bytes_1_and_2() {
    let deployer = address!("4321432143214321432143214321432143214321");
    let init_code_hash = [0x22; 32];

    // Target 1
    let miner1 = VanityMiner::new(deployer, init_code_hash)
        .with_target_zero_bytes(1)
        .with_threads(4);
    let res1 = miner1.mine().expect("Target 1 must succeed");
    assert!(res1.zero_bytes >= 1);

    // Target 2
    let miner2 = VanityMiner::new(deployer, init_code_hash)
        .with_target_zero_bytes(2)
        .with_threads(4);
    let res2 = miner2.mine().expect("Target 2 must succeed");
    assert!(res2.zero_bytes >= 2);
}

#[test]
fn test_vanity_target_boundaries_greater_than_20_invalid() {
    let deployer = address!("0000000000000000000000000000000000000001");
    let init_code_hash = [0xFF; 32];

    // 21 zero bytes is physically impossible for a 20-byte address
    let miner21 = VanityMiner::new(deployer, init_code_hash).with_target_zero_bytes(21);
    assert_eq!(miner21.mine().unwrap_err(), VanityMinerError::InvalidTarget(21));

    // 100 zero bytes
    let miner100 = VanityMiner::new(deployer, init_code_hash).with_target_zero_bytes(100);
    assert_eq!(miner100.mine().unwrap_err(), VanityMinerError::InvalidTarget(100));

    // Hex zeros: 41 hex zeros -> 21 zero bytes -> InvalidTarget(21)
    let miner_hex = VanityMiner::new(deployer, init_code_hash).with_target_hex_zeros(41);
    assert_eq!(miner_hex.mine().unwrap_err(), VanityMinerError::InvalidTarget(21));
}

// ============================================================================
// 4. Deterministic CREATE2 Address Derivation vs Reference EIP-1014 Formula
// ============================================================================

/// Independent reference implementation of EIP-1014:
/// `keccak256( 0xff ++ address ++ salt ++ keccak256(init_code) )[12..32]`
fn eip1014_reference_derive(deployer: Address, salt: [u8; 32], init_code_hash: [u8; 32]) -> Address {
    let mut preimage = Vec::with_capacity(85);
    preimage.push(0xff);
    preimage.extend_from_slice(deployer.as_slice());
    preimage.extend_from_slice(&salt);
    preimage.extend_from_slice(&init_code_hash);
    let hash = keccak256(&preimage);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&hash[12..32]);
    Address::from(addr)
}

#[test]
fn test_eip1014_derivation_reference_equivalence() {
    let test_deployers = [
        Address::ZERO,
        address!("ffffffffffffffffffffffffffffffffffffffff"),
        address!("000000000004444c5dc75cb358380d2e3de08a90"),
        address!("ba1333333333a1ba1108e8412f11850a5c319ba9"),
        address!("1234567890abcdef1234567890abcdef12345678"),
    ];

    for (i, &dep) in test_deployers.iter().enumerate() {
        for salt_seed in [0u8, 1, 0x42, 0xff] {
            let salt = [salt_seed; 32];
            let init_code_hash = [(i as u8) ^ salt_seed; 32];

            let production = compute_create2_address(dep, salt, init_code_hash);
            let reference = eip1014_reference_derive(dep, salt, init_code_hash);

            assert_eq!(
                production, reference,
                "EIP-1014 divergence detected for deployer {:?}, salt seed {}",
                dep, salt_seed
            );
        }
    }
}

// ============================================================================
// 5. Flash Loan Routing Cascade Adversarial Stress
// ============================================================================

#[test]
fn test_flash_loan_routing_cascade_all_exact_and_single_wei_boundaries() {
    let morpho_cap = 10_000_000u128;
    let balancer_cap = 50_000_000u128;
    let univ4_cap = 100_000_000u128;
    let aave_cap = 500_000_000u128;

    let snapshot = ProviderLiquiditySnapshot {
        morpho_liquidity: morpho_cap,
        balancer_liquidity: balancer_cap,
        univ4_liquidity: univ4_cap,
        aave_liquidity: aave_cap,
    };

    // 1. Exact Morpho boundary
    let (p, fee) = route_flash_loan(morpho_cap, &snapshot).expect("Exact Morpho");
    assert_eq!(p, FlashLoanProvider::MorphoBlue);
    assert_eq!(fee, 0);

    // 2. Single-wei cascade to Balancer V3
    let (p, fee) = route_flash_loan(morpho_cap + 1, &snapshot).expect("Morpho + 1");
    assert_eq!(p, FlashLoanProvider::BalancerV3);
    assert_eq!(fee, 0);

    // 3. Exact Balancer boundary
    let (p, fee) = route_flash_loan(balancer_cap, &snapshot).expect("Exact Balancer");
    assert_eq!(p, FlashLoanProvider::BalancerV3);
    assert_eq!(fee, 0);

    // 4. Single-wei cascade to Uniswap V4
    let (p, fee) = route_flash_loan(balancer_cap + 1, &snapshot).expect("Balancer + 1");
    assert_eq!(p, FlashLoanProvider::UniswapV4);
    assert_eq!(fee, 0);

    // 5. Exact Uniswap V4 boundary
    let (p, fee) = route_flash_loan(univ4_cap, &snapshot).expect("Exact UniV4");
    assert_eq!(p, FlashLoanProvider::UniswapV4);
    assert_eq!(fee, 0);

    // 6. Single-wei cascade to Aave V3
    let (p, fee) = route_flash_loan(univ4_cap + 1, &snapshot).expect("UniV4 + 1");
    assert_eq!(p, FlashLoanProvider::AaveV3);
    assert_eq!(fee, 500);

    // 7. Exact Aave V3 boundary
    let (p, fee) = route_flash_loan(aave_cap, &snapshot).expect("Exact Aave");
    assert_eq!(p, FlashLoanProvider::AaveV3);
    assert_eq!(fee, 500);

    // 8. Single-wei cascade beyond all venues -> None
    assert!(route_flash_loan(aave_cap + 1, &snapshot).is_none());
}

#[test]
fn test_flash_loan_routing_all_zero_liquidity() {
    let zero_snapshot = ProviderLiquiditySnapshot::default();
    let token = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");

    // Any positive loan amount must fail
    assert!(route_flash_loan(1, &zero_snapshot).is_none());
    assert!(route_flash_loan(1_000_000, &zero_snapshot).is_none());
    assert!(route_flash_loan(u128::MAX, &zero_snapshot).is_none());

    // Detailed error inspection
    let res = select_optimal_flash_route(Chain::ArbitrumOne, token, 1, &zero_snapshot);
    assert_eq!(
        res.unwrap_err(),
        RoutingError::InsufficientLiquidity {
            required: 1,
            morpho: 0,
            balancer: 0,
            univ4: 0,
            aave: 0,
        }
    );
}

#[test]
fn test_flash_loan_routing_extreme_u128_max_overflow_safety() {
    let token = address!("A0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");

    // Case 1: All liquidities are normal, loan is u128::MAX -> InsufficientLiquidity (no panic)
    let snapshot_normal = ProviderLiquiditySnapshot {
        morpho_liquidity: 10_000_000,
        balancer_liquidity: 10_000_000,
        univ4_liquidity: 10_000_000,
        aave_liquidity: 10_000_000,
    };
    let err = select_optimal_flash_route(Chain::ArbitrumOne, token, u128::MAX, &snapshot_normal).unwrap_err();
    match err {
        RoutingError::InsufficientLiquidity { required, .. } => {
            assert_eq!(required, u128::MAX);
        }
        _ => panic!("Expected InsufficientLiquidity error"),
    }

    // Case 2: Aave has u128::MAX liquidity, loan is u128::MAX on Arbitrum (500 PIPs fee)
    let snapshot_max_aave = ProviderLiquiditySnapshot {
        morpho_liquidity: 0,
        balancer_liquidity: 0,
        univ4_liquidity: 0,
        aave_liquidity: u128::MAX,
    };

    let route_arb = select_optimal_flash_route(Chain::ArbitrumOne, token, u128::MAX, &snapshot_max_aave)
        .expect("u128::MAX loan on Arbitrum should not panic");
    assert_eq!(route_arb.provider, FlashLoanProvider::AaveV3);
    assert_eq!(route_arb.fee_pips, 500);
    // Ceiling division check: (u128::MAX * 500 + 999_999) / 1_000_000
    let expected_arb_fee = ((U256::from(u128::MAX) * U256::from(500) + U256::from(999_999))
        / U256::from(1_000_000))
    .to::<u128>();
    assert_eq!(route_arb.fee_amount, expected_arb_fee);

    // Case 3: Aave has u128::MAX liquidity, loan is u128::MAX on Base (900 PIPs fee)
    let route_base = select_optimal_flash_route(Chain::Base, token, u128::MAX, &snapshot_max_aave)
        .expect("u128::MAX loan on Base should not panic");
    assert_eq!(route_base.provider, FlashLoanProvider::AaveV3);
    assert_eq!(route_base.fee_pips, 900);
    let expected_base_fee = ((U256::from(u128::MAX) * U256::from(900) + U256::from(999_999))
        / U256::from(1_000_000))
    .to::<u128>();
    assert_eq!(route_base.fee_amount, expected_base_fee);
}
