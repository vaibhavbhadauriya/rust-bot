//! Adversarial Stress and Edge-Case Test Suite for Flash Arbitrage Executor
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 48-68, 87-88.
//! EMPIRICAL CHALLENGER: Validates calldata boundaries, 0..99 byte truncations,
//! oversized payloads, extreme values, 1-wei reserve divergence, gas savings, and atomic profit checks.

use alloy_primitives::{address, Address, U256};
use flash_executor::*;

// ==============================================================================
// 1. Calldata Boundaries & Fuzzing
// ==============================================================================

#[test]
fn test_adversarial_sub100_byte_truncation_matrix() {
    // Test EVERY length from 0 to 123 bytes across 3 different byte patterns
    let patterns: [&[u8]; 3] = [
        &[0x00; 124],
        &[0xFF; 124],
        &[0xAA; 124],
    ];

    for pattern in patterns {
        for len in 0..124 {
            let slice = &pattern[..len];
            let res = decode_calldata(slice);
            assert_eq!(
                res,
                Err(CalldataError::CalldataTooShort(len)),
                "Expected CalldataTooShort({}) for length {}, pattern {:x?}",
                len,
                len,
                pattern[0]
            );
        }
    }
}

#[test]
fn test_adversarial_oversized_calldata_integrity() {
    let params = ExecutionParams {
        weth_amount_to_first_market: 4_732_420_000,
        balance_before: 10_000_000_000,
        gas_cost: 250_000,
        target1: address!("1111111111111111111111111111111111111111"),
        target2: address!("2222222222222222222222222222222222222222"),
        token0_amount_out: 1_901_905_000_000_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: 4_777_570_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let encoded = encode_124byte_calldata(&params);
    assert_eq!(encoded.len(), 124);

    // Test specific oversized lengths: 125, 126, 128, 163, 200, 256, 512, 1024, 4096
    let test_sizes = [125, 126, 128, 163, 200, 256, 512, 1024, 4096];
    for &size in &test_sizes {
        // Pad with various hostile/garbage bytes
        for pad_byte in [0x00u8, 0xFFu8, 0x5Au8, 0xA5u8] {
            let mut oversized = Vec::with_capacity(size);
            oversized.extend_from_slice(&encoded);
            oversized.resize(size, pad_byte);

            let decoded = decode_124byte_calldata(&oversized)
                .unwrap_or_else(|e| panic!("Failed to decode oversized calldata of size {}: {:?}", size, e));
            assert_eq!(
                decoded, params,
                "Decoded params from size {} with pad 0x{:02x} must match original verbatim",
                size, pad_byte
            );
        }
    }
}

#[test]
fn test_adversarial_extreme_value_boundaries() {
    // 1. All Zeros
    let all_zeros = ExecutionParams {
        weth_amount_to_first_market: 0,
        balance_before: 0,
        gas_cost: 0,
        target1: Address::ZERO,
        target2: Address::ZERO,
        token0_amount_out: 0,
        token0_amount_out_index: 0,
        token1_amount_out: 0,
        token1_amount_out_index: 0,
        ..Default::default()
    };
    let encoded_zeros = encode_124byte_calldata(&all_zeros);
    assert_eq!(encoded_zeros, [0u8; 124]);
    let decoded_zeros = decode_124byte_calldata(&encoded_zeros).unwrap();
    assert_eq!(decoded_zeros, all_zeros);

    // 2. All Max Values (u128::MAX, u16::MAX, Address::repeat_byte(0xFF))
    let all_max = ExecutionParams {
        weth_amount_to_first_market: u128::MAX,
        balance_before: u128::MAX,
        gas_cost: u128::MAX,
        target1: Address::repeat_byte(0xFF),
        target2: Address::repeat_byte(0xFF),
        token0_amount_out: u128::MAX,
        token0_amount_out_index: u16::MAX,
        token1_amount_out: u128::MAX,
        token1_amount_out_index: u16::MAX,
        ..Default::default()
    };
    let encoded_max = encode_124byte_calldata(&all_max);
    assert_eq!(encoded_max, [0xFFu8; 124]);
    let decoded_max = decode_124byte_calldata(&encoded_max).unwrap();
    assert_eq!(decoded_max, all_max);

    // 3. Alternating Bit Boundaries
    let alternating = ExecutionParams {
        weth_amount_to_first_market: 0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA,
        balance_before: 0x55555555555555555555555555555555,
        gas_cost: 0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA,
        target1: Address::repeat_byte(0x55),
        target2: Address::repeat_byte(0xAA),
        token0_amount_out: 0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA,
        token0_amount_out_index: 0x5555,
        token1_amount_out: 0x55555555555555555555555555555555,
        token1_amount_out_index: 0xAAAA,
        ..Default::default()
    };
    let encoded_alt = encode_124byte_calldata(&alternating);
    let decoded_alt = decode_124byte_calldata(&encoded_alt).unwrap();
    assert_eq!(decoded_alt, alternating);
}

#[test]
fn test_adversarial_callvalue_dispatch_routing_coverage() {
    // 0 wei = Arbitrage swap path
    assert_eq!(dispatch_action_by_callvalue(0).unwrap(), CallvalueAction::ArbitragePath);
    // 1 wei = Owner ERC-20 sweep
    assert_eq!(dispatch_action_by_callvalue(1).unwrap(), CallvalueAction::OwnerErc20Sweep);
    // 2 wei = Owner ETH sweep
    assert_eq!(dispatch_action_by_callvalue(2).unwrap(), CallvalueAction::OwnerEthSweep);

    // Any other callvalue must immediately error
    for invalid_val in [3, 4, 5, 10, 100, 1_000, 1_000_000, u64::MAX] {
        assert_eq!(
            dispatch_action_by_callvalue(invalid_val),
            Err(CalldataError::InvalidCallvalue(invalid_val))
        );
    }
}

// ==============================================================================
// 2. Fast-Revert Guard Adversarial Stress
// ==============================================================================

#[test]
fn test_adversarial_fast_revert_guard_1wei_divergence() {
    let r0 = 1_000_000_000_000_000_000u128;
    let r1 = 405_000_000_000_000_000u128;

    // Matching reserves -> Passed
    assert_eq!(simulate_fast_revert_guard(r0, r1, r0, r1), GuardOutcome::Passed);

    // 1-wei shift on reserve0: +1 wei
    let rev_r0_plus = simulate_fast_revert_guard(r0, r1, r0 + 1, r1);
    match rev_r0_plus {
        GuardOutcome::FastRevert { gas_used, gas_saved_percent } => {
            assert!(gas_used < 3000, "Fast-revert gas must be < 3,000, got {}", gas_used);
            assert!(
                gas_saved_percent > 80.0,
                "Gas savings ({:.2}%) must exceed 80%",
                gas_saved_percent
            );
        }
        GuardOutcome::Passed => panic!("Expected FastRevert for r0 + 1"),
    }

    // 1-wei shift on reserve0: -1 wei
    let rev_r0_minus = simulate_fast_revert_guard(r0, r1, r0 - 1, r1);
    match rev_r0_minus {
        GuardOutcome::FastRevert { gas_used, gas_saved_percent } => {
            assert!(gas_used < 3000);
            assert!(gas_saved_percent > 80.0);
        }
        GuardOutcome::Passed => panic!("Expected FastRevert for r0 - 1"),
    }

    // 1-wei shift on reserve1: +1 wei
    let rev_r1_plus = simulate_fast_revert_guard(r0, r1, r0, r1 + 1);
    match rev_r1_plus {
        GuardOutcome::FastRevert { gas_used, gas_saved_percent } => {
            assert!(gas_used < 3000);
            assert!(gas_saved_percent > 80.0);
        }
        GuardOutcome::Passed => panic!("Expected FastRevert for r1 + 1"),
    }

    // 1-wei shift on reserve1: -1 wei
    let rev_r1_minus = simulate_fast_revert_guard(r0, r1, r0, r1 - 1);
    match rev_r1_minus {
        GuardOutcome::FastRevert { gas_used, gas_saved_percent } => {
            assert!(gas_used < 3000);
            assert!(gas_saved_percent > 80.0);
        }
        GuardOutcome::Passed => panic!("Expected FastRevert for r1 - 1"),
    }

    // Simultaneous 1-wei divergence on both reserves
    let rev_both = simulate_fast_revert_guard(r0, r1, r0 + 1, r1 - 1);
    match rev_both {
        GuardOutcome::FastRevert { gas_used, gas_saved_percent } => {
            assert!(gas_used < 3000);
            assert!(gas_saved_percent > 80.0);
        }
        GuardOutcome::Passed => panic!("Expected FastRevert for simultaneous 1-wei shift"),
    }
}

#[test]
fn test_adversarial_fast_revert_guard_extreme_divergence() {
    // Zero reserve edge case: both match at 0 -> Passed
    assert_eq!(simulate_fast_revert_guard(0, 0, 0, 0), GuardOutcome::Passed);

    // Expected 0, actual 1 -> FastRevert
    assert!(matches!(
        simulate_fast_revert_guard(0, 0, 1, 0),
        GuardOutcome::FastRevert { .. }
    ));

    // Expected u128::MAX, actual u128::MAX -> Passed
    assert_eq!(
        simulate_fast_revert_guard(u128::MAX, u128::MAX, u128::MAX, u128::MAX),
        GuardOutcome::Passed
    );

    // Massive divergence: expected 1, actual u128::MAX -> FastRevert
    assert!(matches!(
        simulate_fast_revert_guard(1, 1, u128::MAX, u128::MAX),
        GuardOutcome::FastRevert { .. }
    ));
}

// ==============================================================================
// 3. Atomic Profit Checks
// ==============================================================================

#[test]
fn test_adversarial_atomic_profit_exact_boundaries() {
    let test_gas_costs = [
        0u128,
        1u128,
        2u128,
        100u128,
        50_000u128,
        250_000u128,
        1_000_000u128,
        (u64::MAX - 1) as u128,
        u64::MAX as u128,
        u128::MAX - 1,
        u128::MAX,
    ];

    for &gas_cost in &test_gas_costs {
        // Breakeven condition: weth_profit == gas_cost -> MUST PASS
        assert!(
            check_atomic_profit(gas_cost, gas_cost).is_ok(),
            "Breakeven (profit == gas_cost == {}) must pass",
            gas_cost
        );

        // Positive profit: weth_profit == gas_cost + 1 -> MUST PASS (if no overflow)
        if gas_cost < u128::MAX {
            assert!(
                check_atomic_profit(gas_cost + 1, gas_cost).is_ok(),
                "Profit ({}) > gas_cost ({}) must pass",
                gas_cost + 1,
                gas_cost
            );
        }

        // Shortfall condition: weth_profit == gas_cost - 1 -> MUST FAIL
        if gas_cost > 0 {
            let shortfall = gas_cost - 1;
            assert_eq!(
                check_atomic_profit(shortfall, gas_cost),
                Err(CalldataError::AtomicProfitViolation {
                    profit: shortfall,
                    gas_cost,
                }),
                "Shortfall (profit == gas_cost - 1 == {}) must fail",
                shortfall
            );
        }
    }

    // Zero profit with positive gas cost -> MUST FAIL
    for &gas_cost in &[1u128, 50_000u128, u64::MAX as u128, u128::MAX] {
        assert_eq!(
            check_atomic_profit(0, gas_cost),
            Err(CalldataError::AtomicProfitViolation {
                profit: 0,
                gas_cost,
            })
        );
    }
}

// ==============================================================================
// 4. Protocol Callback Encoders / Decoders Adversarial Stress
// ==============================================================================

#[test]
fn test_adversarial_morpho_callback_layout() {
    let params = ExecutionParams {
        weth_amount_to_first_market: 12345,
        balance_before: 67890,
        gas_cost: 300,
        target1: address!("3333333333333333333333333333333333333333"),
        target2: address!("4444444444444444444444444444444444444444"),
        token0_amount_out: 1_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: 2_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };
    let payload = encode_124byte_calldata(&params);
    let assets = U256::from(999_999);
    let cb_data = encode_morpho_callback(assets, &payload);

    // Truncated callback tests: every length from 0 to 223 bytes must fail
    for len in 0..224 {
        assert!(
            decode_morpho_callback(&cb_data[..len]).is_err(),
            "Morpho callback of length {} must fail",
            len
        );
    }

    // Oversized callback: trailing data should decode successfully
    let mut oversized = cb_data.clone();
    oversized.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    let (d_assets, d_params) = decode_morpho_callback(&oversized).unwrap();
    assert_eq!(d_assets, assets);
    assert_eq!(d_params, params);
}

#[test]
fn test_adversarial_balancer_univ4_callback_layout() {
    let params = ExecutionParams {
        weth_amount_to_first_market: 54321,
        balance_before: 98765,
        gas_cost: 400,
        target1: address!("5555555555555555555555555555555555555555"),
        target2: address!("6666666666666666666666666666666666666666"),
        token0_amount_out: 3_000_000,
        token0_amount_out_index: 1,
        token1_amount_out: 4_000_000,
        token1_amount_out_index: 0,
        ..Default::default()
    };
    let payload = encode_124byte_calldata(&params);
    let cb_data = encode_balancer_v3_callback(&payload);

    // Truncated callback tests: every length from 0 to 191 bytes must fail
    for len in 0..192 {
        assert!(
            decode_unlock_callback(&cb_data[..len]).is_err(),
            "unlockCallback of length {} must fail",
            len
        );
    }

    // Oversized callback: trailing data succeeds
    let mut oversized = cb_data.clone();
    oversized.extend_from_slice(&[1, 2, 3, 4, 5]);
    let d_params = decode_unlock_callback(&oversized).unwrap();
    assert_eq!(d_params, params);
}

// ==============================================================================
// 5. Pure Yul Bytecode & Assembly Opcode Invariant Stress Testing
// ==============================================================================

#[test]
fn test_adversarial_yul_bytecode_invariants() {
    let runtime = executor_runtime_bytes();
    let init_code = executor_init_code();

    // 1. Exact size budgets
    assert_eq!(runtime.len(), 1114, "Runtime bytecode must be exactly 1114 bytes");
    assert_eq!(init_code.len(), 1126, "Init bytecode must be exactly 1126 bytes");

    // 2. Embedded WETH address (Arbitrum canonical WETH: 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1)
    let weth_bytes = alloy_primitives::hex!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
    assert!(
        runtime.windows(20).any(|window| window == weth_bytes),
        "Runtime bytecode must embed canonical WETH address"
    );

    // 3. Embedded OWNER address (0x1111111111111111111111111111111111111111)
    let owner_bytes = alloy_primitives::hex!("1111111111111111111111111111111111111111");
    assert!(
        runtime.windows(20).any(|window| window == owner_bytes),
        "Runtime bytecode must embed OWNER address"
    );

    // 4. Critical Function Selectors embedded as PUSH4 (0x63):
    // Morpho Blue callback: onMorphoFlashLoan(uint256,bytes) -> 0x3a48e7e1
    let morpho_selector = alloy_primitives::hex!("633a48e7e1");
    assert!(
        runtime.windows(5).any(|w| w == morpho_selector),
        "Runtime bytecode must match Morpho callback selector 0x3a48e7e1"
    );

    // Balancer V3 / Uniswap V4 callback: unlockCallback(bytes) -> 0x98dd202a
    let unlock_selector = alloy_primitives::hex!("6398dd202a");
    assert!(
        runtime.windows(5).any(|w| w == unlock_selector),
        "Runtime bytecode must match unlockCallback selector 0x98dd202a"
    );

    // Uniswap V2 swap(uint256,uint256,address,bytes) -> 0x022c0d9f
    let swap_selector = alloy_primitives::hex!("63022c0d9f");
    assert!(
        runtime.windows(5).any(|w| w == swap_selector),
        "Runtime bytecode must match Uniswap V2 swap selector 0x022c0d9f"
    );

    // ERC20 transfer(address,uint256) -> 0xa9059cbb
    let transfer_selector = alloy_primitives::hex!("63a9059cbb");
    assert!(
        runtime.windows(5).any(|w| w == transfer_selector),
        "Runtime bytecode must match ERC20 transfer selector 0xa9059cbb"
    );

    // ERC20 balanceOf(address) -> 0x70a08231
    let balance_selector = alloy_primitives::hex!("6370a08231");
    assert!(
        runtime.windows(5).any(|w| w == balance_selector),
        "Runtime bytecode must match ERC20 balanceOf selector 0x70a08231"
    );

    // 5. 124-byte calldata check (PUSH1 0x7c = 0x607c) followed by CALLDATASIZE (0x36) and LT (0x10)
    let csize_check = alloy_primitives::hex!("607c3610");
    assert!(
        runtime.windows(4).any(|w| w == csize_check),
        "Runtime bytecode must enforce calldatasize >= 0x7c (124 bytes)"
    );

    // 6. 32-byte revert on profit shortfall: PUSH1 0x20 (0x6020), PUSH0 (0x5f), REVERT (0xfd) -> 60205ffd
    let revert_32bytes = alloy_primitives::hex!("60205ffd");
    assert!(
        runtime.windows(4).any(|w| w == revert_32bytes),
        "Runtime bytecode must contain revert(0, 0x20) for profit shortfall"
    );

    // 7. Fast-revert with zero return data: PUSH0 (0x5f), DUP1 (0x80), REVERT (0xfd) -> 5f80fd
    let revert_0bytes = alloy_primitives::hex!("5f80fd");
    assert!(
        runtime.windows(3).any(|w| w == revert_0bytes),
        "Runtime bytecode must contain revert(0, 0) for zero-data fast revert"
    );
}


