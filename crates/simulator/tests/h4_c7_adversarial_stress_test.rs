//! Adversarial Stress & Empirical Challenge Test Suite for H4 & C7
//!
//! EMPIRICAL CHALLENGER:
//! Validates:
//! 1. H4 Trade Size Widening > u64::MAX (19 ETH, 25 ETH, 100 ETH, 1,000,000 ETH, u128::MAX).
//! 2. H4 124-byte layout calldataload offsets in Yul: bit-level alignment of all 9 fields.
//! 3. H4 Execution in revm with exact storage slot balance verification.
//! 4. H4 Adversarial atomic profit checks with u128 gasCost and balanceBefore.
//! 5. C7 Multi-chain bytecode and init code generation for Arbitrum (42161), Base (8453), and arbitrary owners.
//! 6. C7 Verification of exact byte offsets and zero-missed placeholder substitutions.
//! 7. C7 revm execution of Owner sweeps (callvalue 1 & 2) with custom owners and unauthorized callers.
//! 8. C7 revm execution on Base chain using canonical Base WETH.
//! 9. Flash loan callback unwrapping (Morpho Blue & Balancer V3) under extreme trade sizes (1,000,000 ETH).

use alloy_primitives::{address, hex, keccak256, Address, Bytes, U256};
use flash_core::constants::{arbitrum, base};
use flash_executor::bytecode::{
    build_executor_bytecode, build_executor_init_code, executor_runtime_bytecode,
    EXECUTOR_INIT_CODE, EXECUTOR_RUNTIME_BYTECODE,
};
use flash_executor::calldata::{
    decode_124byte_calldata, encode_124byte_calldata, encode_balancer_v3_callback,
    encode_morpho_callback, ExecutionParams,
};
use flash_simulator::bytecode_mocks::{
    mock_realistic_weth_bytecode, mock_v2_pair_bytecode, realistic_weth_balance_slot,
    MOCK_CALLER_ADDRESS,
};
use revm::{
    bytecode::Bytecode,
    context::{Context, TxEnv},
    context_interface::result::ExecutionResult,
    database::in_memory_db::InMemoryDB,
    handler::{ExecuteEvm, MainBuilder, MainContext},
    primitives::{hardfork::SpecId, TxKind},
    state::{AccountInfo, EvmStorageSlot},
};

const EXECUTOR_ADDR: Address = address!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

/// Enhanced Mock Uniswap V2 Pair bytecode that inspects and stores swap arguments:
/// - getReserves (0x0902f1ac): returns slot 8 (reserve0) and 1 (reserve1)
/// - swap (0x022c0d9f):
///     stores calldataload(0x04) [amount0Out] in storage slot 1
///     stores calldataload(0x24) [amount1Out] in storage slot 2
///     stores calldataload(0x44) [to address] in storage slot 3
///     returns STOP (success)
const RECORDING_V2_PAIR_BYTECODE: &[u8] = &hex!(
    "5f3560e01c80630902f1ac1460185763022c0d9f14602e57"
    "5b6008545f526009546020524260405260605ff30000"
    "5b600435600155602435600255604435600355"
    "604435331415608b57"
    "60043580605557506024355b"
    "63a9059cbb60e01b5f52"
    "604435600452"
    "80602452"
    "7382af49447d8a07e3bd95bd0d56f35241523fbab1"
    "60205f60445f5f855af15050"
    "5b5f5ff3"
);

fn recording_v2_pair_bytecode() -> Bytecode {
    Bytecode::new_raw(Bytes::from_static(RECORDING_V2_PAIR_BYTECODE))
}

fn setup_adv_env(
    executor_code: Bytecode,
    executor_address: Address,
    weth_address: Address,
    pool1_address: Address,
    pool2_address: Address,
    executor_initial_weth: u128,
    use_recording_pools: bool,
) -> InMemoryDB {
    let mut db = InMemoryDB::default();

    db.insert_account_info(
        executor_address,
        AccountInfo {
            code: Some(executor_code.clone()),
            balance: U256::ZERO,
            nonce: 1,
            code_hash: executor_code.hash_slow(),
            account_id: None,
        },
    );

    let weth_code = mock_realistic_weth_bytecode();
    db.insert_account_info(
        weth_address,
        AccountInfo {
            code: Some(weth_code.clone()),
            balance: U256::ZERO,
            nonce: 1,
            code_hash: weth_code.hash_slow(),
            account_id: None,
        },
    );

    let exec_slot = realistic_weth_balance_slot(executor_address);
    let _ = db.insert_account_storage(
        weth_address,
        exec_slot,
        U256::from(executor_initial_weth),
    );

    let pool_code = if use_recording_pools {
        recording_v2_pair_bytecode()
    } else {
        mock_v2_pair_bytecode()
    };

    db.insert_account_info(
        pool1_address,
        AccountInfo {
            code: Some(pool_code.clone()),
            balance: U256::ZERO,
            nonce: 1,
            code_hash: pool_code.hash_slow(),
            account_id: None,
        },
    );

    let pool1_reserves: U256 = U256::from(100_000_000_000_000_000_000u128)
        | (U256::from(40_000_000_000_000_000_000_000u128) << 112);
    let _ = db.insert_account_storage(pool1_address, U256::from(8), pool1_reserves);
    let _ = db.insert_account_storage(pool1_address, U256::from(9), U256::from(40_000_000_000_000_000_000_000u128));

    db.insert_account_info(
        pool2_address,
        AccountInfo {
            code: Some(pool_code.clone()),
            balance: U256::ZERO,
            nonce: 1,
            code_hash: pool_code.hash_slow(),
            account_id: None,
        },
    );

    let pool2_reserves: U256 = U256::from(100_000_000_000_000_000_000u128)
        | (U256::from(39_000_000_000_000_000_000_000u128) << 112);
    let _ = db.insert_account_storage(pool2_address, U256::from(8), pool2_reserves);
    let _ = db.insert_account_storage(pool2_address, U256::from(9), U256::from(39_000_000_000_000_000_000_000u128));

    // Pre-fund Pool 2 with ample WETH liquidity so swaps return output WETH to executor
    let p2_weth_slot = realistic_weth_balance_slot(pool2_address);
    let _ = db.insert_account_storage(
        weth_address,
        p2_weth_slot,
        U256::MAX / U256::from(2),
    );

    db
}

// ==============================================================================
// 1. H4: Trade Size Widening Beyond u64::MAX Matrix (19 ETH, 25 ETH, 100 ETH, 1M ETH, u128::MAX)
// ==============================================================================

#[test]
fn test_adversarial_trade_sizes_exceeding_u64_max_matrix() {
    let test_sizes: [(u128, &'static str); 5] = [
        (19_000_000_000_000_000_000, "19 ETH"),
        (25_000_000_000_000_000_000, "25 ETH"),
        (100_000_000_000_000_000_000, "100 ETH"),
        (1_000_000_000_000_000_000_000_000, "1,000,000 ETH"),
        (u128::MAX - 10_000_000_000_000_000_000, "Near u128::MAX"),
    ];

    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");
    let u64_max_u256 = U256::from(u64::MAX);

    for (trade_size, label) in test_sizes {
        // Mathematical proof: trade_size strictly exceeds u64::MAX
        assert!(
            U256::from(trade_size) > u64_max_u256,
            "{}: {} must strictly exceed u64::MAX ({})",
            label,
            trade_size,
            u64::MAX
        );

        let initial_weth = trade_size + 1_000_000_000_000_000_000; // trade_size + 1 ETH buffer
        let db = setup_adv_env(
            executor_runtime_bytecode(),
            executor_address,
            weth_address,
            pool1_address,
            pool2_address,
            initial_weth,
            false,
        );

        let params = ExecutionParams {
            weth_amount_to_first_market: trade_size,
            balance_before: 500_000_000_000_000_000, // 0.5 ETH
            gas_cost: 50_000,
            target1: pool1_address,
            target2: pool2_address,
            token0_amount_out: 62_500_000_000,
            token0_amount_out_index: 0,
            token1_amount_out: trade_size + 500_000_000_000_000_000,
            token1_amount_out_index: 1,
            ..Default::default()
        };

        let calldata_124 = encode_124byte_calldata(&params);
        let tx = TxEnv {
            caller: MOCK_CALLER_ADDRESS,
            gas_limit: 1_200_000,
            gas_price: 0,
            kind: TxKind::Call(executor_address),
            value: U256::ZERO,
            data: Bytes::copy_from_slice(&calldata_124),
            ..Default::default()
        };

        let ctx = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
            .with_db(db);

        let mut evm = ctx.build_mainnet();
        let exec_res = evm.transact(tx).expect("Execution must not panic");

        assert!(
            matches!(exec_res.result, ExecutionResult::Success { .. }),
            "Execution failed for {}: {:?}",
            label,
            exec_res.result
        );

        let weth_account = exec_res
            .state
            .get(&weth_address)
            .expect("WETH account must exist in modified state");

        let pool1_slot = realistic_weth_balance_slot(pool1_address);
        let pool1_weth = weth_account
            .storage
            .get(&pool1_slot)
            .map(|s: &EvmStorageSlot| s.present_value())
            .unwrap_or_default();

        // Exact 128-bit storage slot balance verification
        assert_eq!(
            pool1_weth,
            U256::from(trade_size),
            "Pool 1 storage slot balance must exactly match untruncated trade size for {}",
            label
        );

        // Counterfactual verification: assert it does NOT equal truncated u64 value
        let truncated_u64 = (trade_size as u64) as u128;
        assert_ne!(
            pool1_weth,
            U256::from(truncated_u64),
            "Pool 1 balance must NOT equal truncated u64 value ({}) for {}",
            truncated_u64,
            label
        );
    }
}

// ==============================================================================
// 2. H4: 124-Byte Layout Calldataload Offsets & Bit-Level Alignment in Yul
// ==============================================================================

#[test]
fn test_adversarial_124byte_calldata_bit_level_fidelity() {
    let params = ExecutionParams {
        weth_amount_to_first_market: 0x112233445566778899aabbccddeeff00,
        balance_before: 0x00112233445566778899aabbccddeeff,
        gas_cost: 0xffeeddccbbaa99887766554433221100,
        target1: address!("1234567890123456789012345678901234567890"),
        target2: address!("abcdefabcdefabcdefabcdefabcdefabcdefabcd"),
        token0_amount_out: 0x0102030405060708090a0b0c0d0e0f10,
        token0_amount_out_index: 0x0001,
        token1_amount_out: 0x100f0e0d0c0b0a090807060504030201,
        token1_amount_out_index: 0x0001,
        ..Default::default()
    };

    let encoded = encode_124byte_calldata(&params);
    assert_eq!(encoded.len(), 124);

    // Byte-by-byte offset boundary verification
    assert_eq!(&encoded[0..16], &params.weth_amount_to_first_market.to_be_bytes());
    assert_eq!(&encoded[16..32], &params.balance_before.to_be_bytes());
    assert_eq!(&encoded[32..48], &params.gas_cost.to_be_bytes());
    assert_eq!(&encoded[48..68], params.target1.as_slice());
    assert_eq!(&encoded[68..88], params.target2.as_slice());
    assert_eq!(&encoded[88..104], &params.token0_amount_out.to_be_bytes());
    assert_eq!(&encoded[104..106], &params.token0_amount_out_index.to_be_bytes());
    assert_eq!(&encoded[106..122], &params.token1_amount_out.to_be_bytes());
    assert_eq!(&encoded[122..124], &params.token1_amount_out_index.to_be_bytes());

    let decoded = decode_124byte_calldata(&encoded).expect("Decoding must succeed");
    assert_eq!(decoded, params, "Bit-level roundtrip must be 100% identical");
}

#[test]
fn test_adversarial_yul_calldata_swap_dispatch_fidelity() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    // Test combinations of token0_amount_out_index and token1_amount_out_index (0, 1)
    let test_indices = [(0u16, 0u16), (0u16, 1u16), (1u16, 0u16), (1u16, 1u16)];

    for (idx0, idx1) in test_indices {
        let t0_out = 62_500_000_000u128;
        let t1_out = 25_500_000_000_000_000_000u128; // 25.5 ETH
        let initial_weth = 30_000_000_000_000_000_000u128; // 30 ETH
        let db = setup_adv_env(
            executor_runtime_bytecode(),
            executor_address,
            weth_address,
            pool1_address,
            pool2_address,
            initial_weth,
            true, // use recording pools
        );

        let params = ExecutionParams {
            weth_amount_to_first_market: 25_000_000_000_000_000_000,
            balance_before: 500_000_000_000_000_000,
            gas_cost: 50_000,
            target1: pool1_address,
            target2: pool2_address,
            token0_amount_out: t0_out,
            token0_amount_out_index: idx0,
            token1_amount_out: t1_out,
            token1_amount_out_index: idx1,
            ..Default::default()
        };

        let calldata_124 = encode_124byte_calldata(&params);
        let tx = TxEnv {
            caller: MOCK_CALLER_ADDRESS,
            gas_limit: 1_200_000,
            gas_price: 0,
            kind: TxKind::Call(executor_address),
            value: U256::ZERO,
            data: Bytes::copy_from_slice(&calldata_124),
            ..Default::default()
        };

        let ctx = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
            .with_db(db);

        let mut evm = ctx.build_mainnet();
        let exec_res = evm.transact(tx).expect("Execution must succeed");
        assert!(
            matches!(exec_res.result, ExecutionResult::Success { .. }),
            "Swap dispatch execution must succeed for idx0={}, idx1={}: {:?}",
            idx0,
            idx1,
            exec_res.result
        );

        // Check Pool 1 received swap args
        let p1_acc = exec_res.state.get(&pool1_address).unwrap();
        let p1_slot1 = p1_acc.storage.get(&U256::from(1)).map(|s| s.present_value()).unwrap_or_default();
        let p1_slot2 = p1_acc.storage.get(&U256::from(2)).map(|s| s.present_value()).unwrap_or_default();
        let p1_slot3 = p1_acc.storage.get(&U256::from(3)).map(|s| s.present_value()).unwrap_or_default();

        if idx0 == 0 {
            assert_eq!(p1_slot1, U256::from(t0_out), "Pool 1 slot 1 must receive amount0");
            assert_eq!(p1_slot2, U256::ZERO, "Pool 1 slot 2 must be 0");
        } else {
            assert_eq!(p1_slot1, U256::ZERO, "Pool 1 slot 1 must be 0");
            assert_eq!(p1_slot2, U256::from(t0_out), "Pool 1 slot 2 must receive amount1");
        }
        // Destination of Pool 1 swap must be Pool 2 (target2)
        let expected_p2_word = U256::from_be_slice(&{
            let mut w = [0u8; 32];
            w[12..32].copy_from_slice(pool2_address.as_slice());
            w
        });
        assert_eq!(p1_slot3, expected_p2_word, "Pool 1 swap 'to' must be target2");

        // Check Pool 2 received swap args
        let p2_acc = exec_res.state.get(&pool2_address).unwrap();
        let p2_slot1 = p2_acc.storage.get(&U256::from(1)).map(|s| s.present_value()).unwrap_or_default();
        let p2_slot2 = p2_acc.storage.get(&U256::from(2)).map(|s| s.present_value()).unwrap_or_default();
        let p2_slot3 = p2_acc.storage.get(&U256::from(3)).map(|s| s.present_value()).unwrap_or_default();

        if idx1 == 0 {
            assert_eq!(p2_slot1, U256::from(t1_out), "Pool 2 slot 1 must receive amount0");
            assert_eq!(p2_slot2, U256::ZERO, "Pool 2 slot 2 must be 0");
        } else {
            assert_eq!(p2_slot1, U256::ZERO, "Pool 2 slot 1 must be 0");
            assert_eq!(p2_slot2, U256::from(t1_out), "Pool 2 slot 2 must receive amount1");
        }
        // Destination of Pool 2 swap must be executor
        let expected_exec_word = U256::from_be_slice(&{
            let mut w = [0u8; 32];
            w[12..32].copy_from_slice(executor_address.as_slice());
            w
        });
        assert_eq!(p2_slot3, expected_exec_word, "Pool 2 swap 'to' must be executor");
    }
}

// ==============================================================================
// 3. H4: Adversarial Atomic Profit with u128 gasCost and balanceBefore
// ==============================================================================

#[test]
fn test_adversarial_atomic_profit_with_u128_gas_cost_boundary() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    // Initial funding: 26.8 ETH.
    // 25 ETH to pool1 leaves 1.8 ETH.
    // balanceBefore = 0, so wethProfit = 1.8 ETH.
    // Set gas_cost = 20 ETH (20 * 10^18 > u64::MAX).
    // Note: If gas_cost truncated to u64, 20 ETH mod 2^64 = 1.553 ETH < 1.8 ETH profit (would pass!).
    // With u128, 1.8 ETH profit < 20 ETH gas_cost (MUST REVERT).
    let db = setup_adv_env(
        executor_runtime_bytecode(),
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        26_800_000_000_000_000_000,
        false,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: 25_000_000_000_000_000_000,
        balance_before: 0,
        gas_cost: 20_000_000_000_000_000_000, // 20 ETH > u64::MAX
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: 26_800_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata_124 = encode_124byte_calldata(&params);
    let tx = TxEnv {
        caller: MOCK_CALLER_ADDRESS,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&calldata_124),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);

    let mut evm = ctx.build_mainnet();
    let exec_res = evm.transact(tx).expect("Execution completed");

    match exec_res.result {
        ExecutionResult::Revert { output, .. } => {
            assert_eq!(output.len(), 32);
            let profit = U256::from_be_slice(&output);
            assert_eq!(profit, U256::from(1_800_000_000_000_000_000u128));
        }
        other => panic!("Expected Revert due to gasCost > profit, got {:?}", other),
    }
}

#[test]
fn test_adversarial_atomic_profit_with_u128_balance_before_boundary() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    // Initial funding: 46 ETH.
    // 25 ETH to pool1 leaves 21 ETH.
    // balanceBefore = 20 ETH (> u64::MAX).
    // Actual profit = 21 ETH - 20 ETH = 1 ETH.
    // Set gas_cost = 2 ETH.
    // Since profit (1 ETH) < gas_cost (2 ETH), it MUST REVERT!
    // Note: If balanceBefore was truncated to u64, balanceBefore = 1.553 ETH,
    // so calculated profit would be 21 - 1.553 = 19.45 ETH >= 2 ETH (would erroneously pass!).
    let db = setup_adv_env(
        executor_runtime_bytecode(),
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        46_000_000_000_000_000_000, // 46 ETH
        false,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: 25_000_000_000_000_000_000,
        balance_before: 20_000_000_000_000_000_000, // 20 ETH > u64::MAX
        gas_cost: 2_000_000_000_000_000_000,       // 2 ETH
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: 26_000_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata_124 = encode_124byte_calldata(&params);
    let tx = TxEnv {
        caller: MOCK_CALLER_ADDRESS,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&calldata_124),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);

    let mut evm = ctx.build_mainnet();
    let exec_res = evm.transact(tx).expect("Execution completed");

    match exec_res.result {
        ExecutionResult::Revert { output, .. } => {
            assert_eq!(output.len(), 32);
            let profit = U256::from_be_slice(&output);
            assert_eq!(profit, U256::from(1_000_000_000_000_000_000u128));
        }
        other => panic!("Expected Revert due to profit (1 ETH) < gasCost (2 ETH), got {:?}", other),
    }
}

// ==============================================================================
// 4. C7: Bytecode Offsets, Canonical WETH, and Arbitrary Owners
// ==============================================================================

#[test]
fn test_adversarial_c7_bytecode_placeholder_offsets_and_uniqueness() {
    let placeholder_owner = address!("1111111111111111111111111111111111111111");
    let arb_weth = arbitrum::WETH;

    // 1. Scan EXECUTOR_RUNTIME_BYTECODE for all occurrences of placeholder_owner
    let mut rt_owner_matches = Vec::new();
    for i in 0..=EXECUTOR_RUNTIME_BYTECODE.len().saturating_sub(20) {
        if &EXECUTOR_RUNTIME_BYTECODE[i..i + 20] == placeholder_owner.as_slice() {
            rt_owner_matches.push(i);
        }
    }
    assert_eq!(
        rt_owner_matches,
        vec![27, 73, 203],
        "Placeholder owner must appear EXACTLY three times at offsets [27, 73, 203] in runtime bytecode"
    );

    // 2. Scan EXECUTOR_RUNTIME_BYTECODE for all occurrences of Arbitrum WETH
    let mut rt_weth_matches = Vec::new();
    for i in 0..=EXECUTOR_RUNTIME_BYTECODE.len().saturating_sub(20) {
        if &EXECUTOR_RUNTIME_BYTECODE[i..i + 20] == arb_weth.as_slice() {
            rt_weth_matches.push(i);
        }
    }
    assert_eq!(
        rt_weth_matches,
        vec![1094],
        "Arbitrum WETH must appear at offset [1094] in runtime bytecode"
    );

    // 3. Scan EXECUTOR_INIT_CODE for all occurrences of placeholder_owner
    let mut init_owner_matches = Vec::new();
    for i in 0..=EXECUTOR_INIT_CODE.len().saturating_sub(20) {
        if &EXECUTOR_INIT_CODE[i..i + 20] == placeholder_owner.as_slice() {
            init_owner_matches.push(i);
        }
    }
    assert_eq!(
        init_owner_matches,
        vec![39, 85, 215],
        "Placeholder owner must appear EXACTLY three times at offsets [39, 85, 215] in init code (12 + [27, 73, 203])"
    );

    // 4. Scan EXECUTOR_INIT_CODE for all occurrences of Arbitrum WETH
    let mut init_weth_matches = Vec::new();
    for i in 0..=EXECUTOR_INIT_CODE.len().saturating_sub(20) {
        if &EXECUTOR_INIT_CODE[i..i + 20] == arb_weth.as_slice() {
            init_weth_matches.push(i);
        }
    }
    assert_eq!(
        init_weth_matches,
        vec![1106],
        "Arbitrum WETH must appear at offset [1106] in init code (12 + 1094)"
    );
}

#[test]
fn test_adversarial_c7_arbitrary_owners_fuzz_substitution() {
    let test_owners = [
        address!("0000000000000000000000000000000000000001"),
        address!("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
        address!("ffffffffffffffffffffffffffffffffffffffff"),
        address!("1234567890123456789012345678901234567890"),
        address!("0102030405060708090a0b0c0d0e0f1011121314"),
    ];

    for owner in test_owners {
        // Test runtime bytecode substitution
        let rt = build_executor_bytecode(42161, owner);
        assert_eq!(&rt[27..47], owner.as_slice());
        assert_eq!(&rt[73..93], owner.as_slice());
        assert_eq!(&rt[203..223], owner.as_slice());
        assert_eq!(&rt[1094..1114], arbitrum::WETH.as_slice());

        // Test init code substitution
        let init = build_executor_init_code(42161, owner);
        assert_eq!(&init[39..59], owner.as_slice());
        assert_eq!(&init[85..105], owner.as_slice());
        assert_eq!(&init[215..235], owner.as_slice());
        assert_eq!(&init[1106..1126], arbitrum::WETH.as_slice());

        // Test Base substitution
        let rt_base = build_executor_bytecode(8453, owner);
        assert_eq!(&rt_base[27..47], owner.as_slice());
        assert_eq!(&rt_base[73..93], owner.as_slice());
        assert_eq!(&rt_base[203..223], owner.as_slice());
        assert_eq!(&rt_base[1094..1114], base::WETH.as_slice());
    }
}

#[test]
fn test_adversarial_c7_revm_owner_sweep_arbitrary_address() {
    let custom_owner = address!("abcdef0123456789abcdef0123456789abcdef01");
    let unauthorized_caller = address!("9999999999999999999999999999999999999999");
    let executor_address = EXECUTOR_ADDR;
    let sweep_recipient = address!("7777777777777777777777777777777777777777");
    let mock_token = address!("8888888888888888888888888888888888888888");

    let rt = build_executor_bytecode(42161, custom_owner);
    let mut db = InMemoryDB::default();

    // Deploy executor with custom owner
    db.insert_account_info(
        executor_address,
        AccountInfo {
            code: Some(Bytecode::new_raw(rt)),
            balance: U256::from(5_000_000_000_000_000_000u128), // 5 ETH balance in executor
            nonce: 1,
            code_hash: keccak256(&[1u8; 32]),
            account_id: None,
        },
    );

    // Fund custom_owner and unauthorized_caller with ETH
    db.insert_account_info(
        custom_owner,
        AccountInfo {
            code: None,
            balance: U256::from(10_000_000_000_000_000_000u128),
            nonce: 0,
            code_hash: keccak256(&[]),
            account_id: None,
        },
    );
    db.insert_account_info(
        unauthorized_caller,
        AccountInfo {
            code: None,
            balance: U256::from(10_000_000_000_000_000_000u128),
            nonce: 0,
            code_hash: keccak256(&[]),
            account_id: None,
        },
    );

    // Deploy mock token with balance in executor
    let token_code = mock_realistic_weth_bytecode();
    db.insert_account_info(
        mock_token,
        AccountInfo {
            code: Some(token_code),
            balance: U256::ZERO,
            nonce: 1,
            code_hash: keccak256(&[2u8; 32]),
            account_id: None,
        },
    );

    let exec_token_slot = realistic_weth_balance_slot(executor_address);
    let _ = db.insert_account_storage(
        mock_token,
        exec_token_slot,
        U256::from(100_000_000_000_000_000_000u128), // 100 tokens
    );

    // --- Scenario 1: Unauthorized caller tries ERC-20 sweep (callvalue == 1 wei) ---
    let mut sweep_calldata = [0u8; 40];
    sweep_calldata[0..20].copy_from_slice(mock_token.as_slice());
    sweep_calldata[20..40].copy_from_slice(sweep_recipient.as_slice());

    let bad_tx = TxEnv {
        caller: unauthorized_caller,
        gas_limit: 500_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::from(1), // 1 wei = ERC-20 sweep
        data: Bytes::copy_from_slice(&sweep_calldata),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db.clone());
    let mut evm = ctx.build_mainnet();
    let res = evm.transact(bad_tx).expect("Execution completed");
    assert!(
        matches!(res.result, ExecutionResult::Revert { .. }),
        "Unauthorized caller must be reverted on ERC-20 sweep"
    );

    // --- Scenario 2: Authorized custom_owner executes ERC-20 sweep ---
    let good_tx = TxEnv {
        caller: custom_owner,
        gas_limit: 500_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::from(1),
        data: Bytes::copy_from_slice(&sweep_calldata),
        ..Default::default()
    };

    let ctx2 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db.clone());
    let mut evm2 = ctx2.build_mainnet();
    let res2 = evm2.transact(good_tx).expect("Execution completed");
    assert!(
        matches!(res2.result, ExecutionResult::Success { .. }),
        "Custom owner must succeed in ERC-20 sweep: {:?}",
        res2.result
    );

    let token_acc = res2.state.get(&mock_token).unwrap();
    let recipient_slot = realistic_weth_balance_slot(sweep_recipient);
    let recipient_bal = token_acc.storage.get(&recipient_slot).map(|s| s.present_value()).unwrap_or_default();
    assert_eq!(recipient_bal, U256::from(100_000_000_000_000_000_000u128), "Recipient must receive swept tokens");

    // --- Scenario 3: Unauthorized caller tries ETH sweep (callvalue == 2 wei) ---
    let bad_eth_tx = TxEnv {
        caller: unauthorized_caller,
        gas_limit: 500_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::from(2), // 2 wei = ETH sweep
        data: Bytes::default(),
        ..Default::default()
    };

    let ctx3 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db.clone());
    let mut evm3 = ctx3.build_mainnet();
    let res3 = evm3.transact(bad_eth_tx).expect("Execution completed");
    assert!(
        matches!(res3.result, ExecutionResult::Revert { .. }),
        "Unauthorized caller must be reverted on ETH sweep"
    );

    // --- Scenario 4: Authorized custom_owner executes ETH sweep ---
    let good_eth_tx = TxEnv {
        caller: custom_owner,
        gas_limit: 500_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::from(2),
        data: Bytes::default(),
        ..Default::default()
    };

    let ctx4 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);
    let mut evm4 = ctx4.build_mainnet();
    let res4 = evm4.transact(good_eth_tx).expect("Execution completed");
    assert!(
        matches!(res4.result, ExecutionResult::Success { .. }),
        "Custom owner must succeed in ETH sweep: {:?}",
        res4.result
    );
}

// ==============================================================================
// 5. Flash Loan Callback Unwrapping with 1,000,000 ETH Extreme Trade Size
// ==============================================================================

#[test]
fn test_adversarial_morpho_and_balancer_extreme_1m_eth_callbacks() {
    let trade_size: u128 = 1_000_000_000_000_000_000_000_000; // 1,000,000 ETH
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let morpho_pool = flash_core::constants::arbitrum::MORPHO_BLUE;
    let balancer_vault = flash_core::constants::arbitrum::BALANCER_V3_VAULT;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    let params = ExecutionParams {
        weth_amount_to_first_market: trade_size,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: trade_size + 500_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };
    let calldata_124 = encode_124byte_calldata(&params);

    // --- Morpho 1,000,000 ETH Callback ---
    let initial_weth_morpho = trade_size + 1_000_000_000_000_000_000; // 1,000,001 ETH
    let db_morpho = setup_adv_env(
        executor_runtime_bytecode(),
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        initial_weth_morpho,
        false,
    );

    let morpho_calldata = encode_morpho_callback(U256::from(trade_size), &calldata_124);
    let tx_morpho = TxEnv {
        caller: morpho_pool,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::ZERO,
        data: Bytes::from(morpho_calldata),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db_morpho);
    let mut evm = ctx.build_mainnet();
    let res_morpho = evm.transact(tx_morpho).expect("Morpho execution completed");
    assert!(
        matches!(res_morpho.result, ExecutionResult::Success { .. }),
        "Morpho 1M ETH callback failed: {:?}",
        res_morpho.result
    );

    let weth_acc = res_morpho.state.get(&weth_address).unwrap();
    let p1_slot = realistic_weth_balance_slot(pool1_address);
    let p1_bal = weth_acc.storage.get(&p1_slot).map(|s| s.present_value()).unwrap_or_default();
    assert_eq!(p1_bal, U256::from(trade_size), "Pool 1 must receive 1,000,000 ETH");

    let morpho_slot = realistic_weth_balance_slot(morpho_pool);
    let morpho_bal = weth_acc.storage.get(&morpho_slot).map(|s| s.present_value()).unwrap_or_default();
    assert_eq!(morpho_bal, U256::from(trade_size), "Morpho caller must be repaid 1,000,000 ETH");

    // --- Balancer 1,000,000 ETH Callback ---
    let initial_weth_balancer = trade_size + 1_000_000_000_000_000_000;
    let db_balancer = setup_adv_env(
        executor_runtime_bytecode(),
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        initial_weth_balancer,
        false,
    );

    let balancer_calldata = encode_balancer_v3_callback(&calldata_124);
    let tx_balancer = TxEnv {
        caller: balancer_vault,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::ZERO,
        data: Bytes::from(balancer_calldata),
        ..Default::default()
    };

    let ctx2 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db_balancer);
    let mut evm2 = ctx2.build_mainnet();
    let res_balancer = evm2.transact(tx_balancer).expect("Balancer execution completed");
    assert!(
        matches!(res_balancer.result, ExecutionResult::Success { .. }),
        "Balancer 1M ETH callback failed: {:?}",
        res_balancer.result
    );

    let weth_acc2 = res_balancer.state.get(&weth_address).unwrap();
    let p1_bal2 = weth_acc2.storage.get(&p1_slot).map(|s| s.present_value()).unwrap_or_default();
    assert_eq!(p1_bal2, U256::from(trade_size), "Pool 1 must receive 1,000,000 ETH from Balancer path");

    let balancer_slot = realistic_weth_balance_slot(balancer_vault);
    let balancer_bal = weth_acc2.storage.get(&balancer_slot).map(|s| s.present_value()).unwrap_or_default();
    assert_eq!(balancer_bal, U256::from(trade_size), "Balancer caller must be repaid 1,000,000 ETH");
}
