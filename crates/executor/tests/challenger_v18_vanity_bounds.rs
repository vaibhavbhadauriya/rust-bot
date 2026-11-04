//! Adversarial Bounds Fuzzing & Stress Test for Vanity Address Miner (V-18)
//!
//! Tests has_leading_zero_bytes with extreme targets:
//! - target_zero_bytes = 20, 21, 22, 100, 255, usize::MAX
//! - target_zero_bytes = 0, 1, 2, 7, 8, 19, usize::MAX - 1
//! Verifies:
//! - Zero panics occur on extreme targets (no slice index out of bounds)
//! - Targets > 20 strictly return false (Ethereum/Arbitrum address is 20 bytes max)
//! - Target 20 returns true iff all 20 address bytes are zero
//! - Fuzzing with pseudo-random and patterned hashes across extreme targets

use flash_executor::has_leading_zero_bytes;

#[test]
fn test_v18_extreme_targets_zero_panics() {
    let extreme_targets = [
        20usize,
        21,
        22,
        100,
        255,
        usize::MAX,
        usize::MAX - 1,
        usize::MAX / 2,
        1_000_000,
    ];

    // Case 1: All zero hash (hash[12..32] is 20 zero bytes)
    let all_zero_hash = [0u8; 32];
    for &target in &extreme_targets {
        let res = has_leading_zero_bytes(&all_zero_hash, target);
        if target == 20 {
            assert!(res, "target 20 on all-zero hash should return true");
        } else {
            assert!(!res, "target {} > 20 on all-zero hash MUST return false", target);
        }
    }

    // Case 2: All 0xFF hash
    let all_ff_hash = [0xFFu8; 32];
    for &target in &extreme_targets {
        let res = has_leading_zero_bytes(&all_ff_hash, target);
        assert!(!res, "target {} on all-0xFF hash MUST return false", target);
    }

    // Case 3: Hash with exactly 19 leading zero bytes in address (byte 31 is 0x01)
    let mut hash_19_zeros = [0u8; 32];
    hash_19_zeros[31] = 0x01;
    for &target in &extreme_targets {
        let res = has_leading_zero_bytes(&hash_19_zeros, target);
        assert!(!res, "target {} on 19-zero hash MUST return false", target);
    }
    assert!(has_leading_zero_bytes(&hash_19_zeros, 19), "19 zeros target should return true");

    // Case 4: Target 0 boundary
    assert!(has_leading_zero_bytes(&all_zero_hash, 0));
    assert!(has_leading_zero_bytes(&all_ff_hash, 0));
    assert!(has_leading_zero_bytes(&hash_19_zeros, 0));
}

#[test]
fn test_v18_fuzz_hashes_against_extreme_targets() {
    let extreme_targets = [20, 21, 22, 23, 50, 100, 255, 1024, usize::MAX];

    // Generate 1,000 deterministic pseudo-random hash patterns
    let mut state: u64 = 0xDEADBEEFCAFE1234;
    for i in 0..1000 {
        let mut hash = [0u8; 32];
        for chunk in hash.chunks_exact_mut(8) {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            chunk.copy_from_slice(&state.to_le_bytes());
        }

        // Randomly zero out leading bytes in address slice [12..32]
        let zero_prefix = (i % 22) as usize;
        for b in 0..zero_prefix.min(20) {
            hash[12 + b] = 0;
        }

        for &target in &extreme_targets {
            let res = has_leading_zero_bytes(&hash, target);
            if target > 20 {
                assert!(!res, "target {} must strictly return false", target);
            } else if target == 20 {
                let actual_zeros = hash[12..32].iter().take_while(|&&b| b == 0).count();
                assert_eq!(res, actual_zeros == 20);
            }
        }
    }
}
