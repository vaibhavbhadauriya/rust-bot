//! Adversarial Empirical Challenger Test Suite for Milestone M2
//!
//! Validates:
//! 1. L8 Quoter bytecode inspection: confirms QUOTER_RUNTIME_BYTECODE len == 354 and contains [0x60, 0xa0, 0x1c] (shr(160, liquidity)).
//! 2. L8 EVM runtime behavior: rejects quotes when liquidity has high bits (> 160 bits) via EVM revert, and executes valid quotes.
//! 3. H4 25 ETH trade execution in revm Cancun mode without u64 truncation (preserves 25 * 10^18 wei > u64::MAX).
//! 4. V-04 3-hop triangular route calldata encoding/decoding bit-level roundtrip (163 bytes).
//! 5. V-04 3-hop triangular route end-to-end execution in revm Cancun mode through Pure Yul executor (Pool 1 -> Pool 2 -> Pool 3 -> Executor).
//! 6. V-04 3-hop triangular route execution via Morpho Blue and Balancer V3 / Uni V4 callbacks.
//! 7. Truncation rejection matrix: ExecutionParams / decode_calldata strictly rejects calldata lengths 0..123, and Yul EVM reverts on truncated payloads.

use alloy_primitives::{address, Address, Bytes, U256};
use flash_core::constants::arbitrum;
use flash_executor::bytecode::executor_runtime_bytecode;
use flash_executor::calldata::{
    decode_124byte_calldata, decode_calldata, encode_124byte_calldata, encode_163byte_calldata,
    encode_balancer_v3_callback, encode_calldata, encode_morpho_callback, CalldataError,
    ExecutionParams,
};
use flash_simulator::bytecode_mocks::{
    mock_realistic_weth_bytecode, mock_v2_pair_bytecode_for_weth, realistic_weth_balance_slot,
    MOCK_CALLER_ADDRESS,
};
use flash_simulator::quoter::{
    execute_evm_quote, inject_pool_state, inject_quoter_bytecode, QuoteExactInputSingleParams,
    SimulatedPoolState, DEFAULT_QUOTER_ADDRESS, QUOTER_RUNTIME_BYTECODE,
};
use revm::{
    context::{Context, TxEnv},
    context_interface::result::ExecutionResult,
    database::in_memory_db::InMemoryDB,
    handler::{ExecuteEvm, MainBuilder, MainContext},
    primitives::hardfork::SpecId,
    primitives::TxKind,
    state::{AccountInfo, EvmStorageSlot},
};
use std::collections::HashMap;

const EXECUTOR_ADDR: Address = address!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
const TRADE_25_ETH_WEI: u128 = 25_000_000_000_000_000_000;

fn setup_3hop_env(
    executor_address: Address,
    weth_address: Address,
    pool1_address: Address,
    pool2_address: Address,
    pool3_address: Address,
    executor_initial_weth: u128,
) -> InMemoryDB {
    let mut db = InMemoryDB::default();

    // 1. Deploy Pure Yul Executor
    let executor_code = executor_runtime_bytecode();
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

    // 2. Deploy Realistic WETH
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

    // Fund executor with initial WETH
    let exec_slot = realistic_weth_balance_slot(executor_address);
    let _ = db.insert_account_storage(
        weth_address,
        exec_slot,
        U256::from(executor_initial_weth),
    );

    // 3. Deploy Pools 1, 2, and 3
    let pool_code = mock_v2_pair_bytecode_for_weth(weth_address);
    for pool in [pool1_address, pool2_address, pool3_address] {
        db.insert_account_info(
            pool,
            AccountInfo {
                code: Some(pool_code.clone()),
                balance: U256::ZERO,
                nonce: 1,
                code_hash: pool_code.hash_slow(),
                account_id: None,
            },
        );
        // Set slot 8 & 9 reserves for fast-revert guard and getReserves
        let _ = db.insert_account_storage(
            pool,
            U256::from(8),
            U256::from(500_000_000_000_000u128),
        );
        let _ = db.insert_account_storage(
            pool,
            U256::from(9),
            U256::from(500_000_000_000_000u128),
        );
    }

    // Pre-fund Pool 3 with 100,000 ETH so it can return final swap output to executor
    let pool3_slot = realistic_weth_balance_slot(pool3_address);
    let _ = db.insert_account_storage(
        weth_address,
        pool3_slot,
        U256::from(100_000_000_000_000_000_000_000u128),
    );

    db
}

// ==============================================================================
// 1. Deliverable 1: L8 Quoter Bytecode Inspection & EVM Execution
// ==============================================================================

#[test]
fn test_challenger_l8_quoter_bytecode_inspection_and_evm_execution() {
    let shr_160 = [0x60, 0xa0, 0x1c];
    let has_shr_160 = QUOTER_RUNTIME_BYTECODE.windows(3).any(|w| w == shr_160);

    // 1. Assert bytecode length and presence of shift guard opcode sequence
    assert_eq!(
        QUOTER_RUNTIME_BYTECODE.len(),
        354,
        "Recompiled QUOTER_RUNTIME_BYTECODE must be exactly 354 bytes"
    );
    assert!(
        has_shr_160,
        "QUOTER_RUNTIME_BYTECODE must contain [0x60, 0xa0, 0x1c] (PUSH1 0xa0; SHR)"
    );

    // 2. In-memory revm execution: test quote with valid inputs
    let mut db = InMemoryDB::default();
    inject_quoter_bytecode(&mut db, DEFAULT_QUOTER_ADDRESS);

    let token_in = arbitrum::WETH;
    let token_out = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");
    let pool_state = SimulatedPoolState {
        pool_id: address!("0000000000000000000000000000000000000001"),
        token0: token_in,
        token1: token_out,
        fee_pips: 500,
        sqrt_price_x96: U256::from(39614081257132168796771975168u128),
        liquidity: 15_000_000_000_000_000_000u128,
        tick: 200000,
        tick_spacing: 10,
        tick_bitmap: HashMap::new(),
    };
    inject_pool_state(&mut db, DEFAULT_QUOTER_ADDRESS, &pool_state).unwrap();

    let valid_params = QuoteExactInputSingleParams {
        token_in,
        token_out,
        fee: 500,
        amount_in: U256::from(1_000_000_000_000_000_000u128), // 1 ETH
        sqrt_price_limit_x96: U256::ZERO,
    };
    let quote_res = execute_evm_quote(&mut db, DEFAULT_QUOTER_ADDRESS, &valid_params);
    assert!(quote_res.is_ok(), "Valid quote must succeed in revm");
    let quote_out = quote_res.unwrap();
    assert!(quote_out.amount_out > U256::ZERO, "amount_out must be positive");

    // 3. Test that high bits in liquidity (> 160 bits) trigger revert
    let slot1_high = U256::from(1) << 165;
    db.insert_account_storage(DEFAULT_QUOTER_ADDRESS, U256::from(1), slot1_high).unwrap();
    let quote_overflow = execute_evm_quote(&mut db, DEFAULT_QUOTER_ADDRESS, &valid_params);
    assert!(
        quote_overflow.is_err(),
        "Quote must revert when liquidity storage has > 160 bits"
    );
}

// ==============================================================================
// 2. Deliverable 2: 25 ETH Trade Execution in revm Without u64 Truncation
// ==============================================================================

#[test]
fn test_challenger_25eth_trade_execution_without_u64_truncation() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    let executor_initial_weth = 26_000_000_000_000_000_000u128; // 26 ETH
    let mut db = InMemoryDB::default();

    let executor_code = executor_runtime_bytecode();
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

    let pool_code = mock_v2_pair_bytecode_for_weth(weth_address);
    for pool in [pool1_address, pool2_address] {
        db.insert_account_info(
            pool,
            AccountInfo {
                code: Some(pool_code.clone()),
                balance: U256::ZERO,
                nonce: 1,
                code_hash: pool_code.hash_slow(),
                account_id: None,
            },
        );
        let _ = db.insert_account_storage(
            pool,
            U256::from(8),
            U256::from(500_000_000_000_000u128),
        );
        let _ = db.insert_account_storage(
            pool,
            U256::from(9),
            U256::from(500_000_000_000_000u128),
        );
    }

    let pool2_slot = realistic_weth_balance_slot(pool2_address);
    let _ = db.insert_account_storage(
        weth_address,
        pool2_slot,
        U256::from(100_000_000_000_000_000_000_000u128),
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: 25_500_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata = encode_124byte_calldata(&params);
    let tx = TxEnv {
        caller: MOCK_CALLER_ADDRESS,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&calldata),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);

    let mut evm = ctx.build_mainnet();
    let exec_res = evm.transact(tx).expect("revm execution should succeed");

    assert!(
        matches!(exec_res.result, ExecutionResult::Success { .. }),
        "Execution failed: {:?}",
        exec_res.result
    );

    let weth_acc = exec_res.state.get(&weth_address).unwrap();
    let pool1_slot = realistic_weth_balance_slot(pool1_address);
    let pool1_balance = weth_acc
        .storage
        .get(&pool1_slot)
        .map(|s: &EvmStorageSlot| s.present_value())
        .unwrap_or_default();

    // Verify untruncated 25 ETH transfer
    assert_eq!(
        pool1_balance,
        U256::from(TRADE_25_ETH_WEI),
        "Pool 1 must hold exact untruncated 25 ETH"
    );

    // Verify it exceeds u64::MAX and does NOT equal truncated u64 value
    assert!(pool1_balance > U256::from(u64::MAX));
    let truncated_u64 = (TRADE_25_ETH_WEI as u64) as u128;
    assert_ne!(pool1_balance, U256::from(truncated_u64));
}

// ==============================================================================
// 3. Deliverable 3: 3-Hop Triangular Route Calldata Roundtrip & revm Execution
// ==============================================================================

#[test]
fn test_challenger_3hop_calldata_encoding_and_decoding_roundtrip() {
    let params_3hop = ExecutionParams {
        weth_amount_to_first_market: 25_000_000_000_000_000_000,
        balance_before: 1_000_000_000_000_000_000,
        gas_cost: 150_000,
        target1: address!("1111111111111111111111111111111111111111"),
        target2: address!("2222222222222222222222222222222222222222"),
        token0_amount_out: 50_000_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: 48_000_000_000,
        token1_amount_out_index: 1,
        target3: address!("3333333333333333333333333333333333333333"),
        token2_amount_out: 25_800_000_000_000_000_000,
        token2_amount_out_index: 1,
        hop_count: 3,
    };

    let encoded_163 = encode_163byte_calldata(&params_3hop);
    assert_eq!(encoded_163.len(), 163);

    // Dynamic encoder must also output 163 bytes when hop_count >= 3
    let encoded_dyn = encode_calldata(&params_3hop);
    assert_eq!(encoded_dyn.len(), 163);
    assert_eq!(encoded_dyn, encoded_163.to_vec());

    // Verify 3-hop field offsets
    assert_eq!(&encoded_163[0..16], &params_3hop.weth_amount_to_first_market.to_be_bytes());
    assert_eq!(&encoded_163[16..32], &params_3hop.balance_before.to_be_bytes());
    assert_eq!(&encoded_163[32..48], &params_3hop.gas_cost.to_be_bytes());
    assert_eq!(&encoded_163[48..68], params_3hop.target1.as_slice());
    assert_eq!(&encoded_163[68..88], params_3hop.target2.as_slice());
    assert_eq!(&encoded_163[88..104], &params_3hop.token0_amount_out.to_be_bytes());
    assert_eq!(&encoded_163[104..106], &params_3hop.token0_amount_out_index.to_be_bytes());
    assert_eq!(&encoded_163[106..122], &params_3hop.token1_amount_out.to_be_bytes());
    assert_eq!(&encoded_163[122..124], &params_3hop.token1_amount_out_index.to_be_bytes());
    assert_eq!(&encoded_163[124..144], params_3hop.target3.as_slice());
    assert_eq!(&encoded_163[144..160], &params_3hop.token2_amount_out.to_be_bytes());
    assert_eq!(&encoded_163[160..162], &params_3hop.token2_amount_out_index.to_be_bytes());
    assert_eq!(encoded_163[162], 3);

    // Decode and verify exact identity
    let decoded = decode_calldata(&encoded_163).expect("Decoding 163B calldata must succeed");
    assert_eq!(decoded, params_3hop);
}

#[test]
fn test_challenger_3hop_triangular_route_execution_in_revm() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");
    let pool3_address = address!("4444444444444444444444444444444444444444");

    let initial_executor_weth = 26_000_000_000_000_000_000u128; // 26 ETH
    let db = setup_3hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        pool3_address,
        initial_executor_weth,
    );

    // 3-hop triangular route:
    // Input: 25 ETH
    // Pool 1 -> Pool 2 (token0 intermediate)
    // Pool 2 -> Pool 3 (token1 intermediate)
    // Pool 3 -> Executor (25.8 ETH WETH final output)
    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 50_000_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: 48_000_000_000,
        token1_amount_out_index: 1,
        target3: pool3_address,
        token2_amount_out: 25_800_000_000_000_000_000, // 25.8 ETH
        token2_amount_out_index: 1,
        hop_count: 3,
    };

    let calldata_163 = encode_163byte_calldata(&params);
    assert_eq!(calldata_163.len(), 163);

    let tx = TxEnv {
        caller: MOCK_CALLER_ADDRESS,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&calldata_163),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);

    let mut evm = ctx.build_mainnet();
    let exec_res = evm.transact(tx).expect("3-hop EVM execution should complete");

    assert!(
        matches!(exec_res.result, ExecutionResult::Success { .. }),
        "3-hop execution failed: {:?}",
        exec_res.result
    );

    let weth_acc = exec_res.state.get(&weth_address).unwrap();

    // Pool 1 received exact 25 ETH
    let pool1_slot = realistic_weth_balance_slot(pool1_address);
    let p1_weth = weth_acc
        .storage
        .get(&pool1_slot)
        .map(|s: &EvmStorageSlot| s.present_value())
        .unwrap_or_default();
    assert_eq!(p1_weth, U256::from(TRADE_25_ETH_WEI));

    // Executor ended with 26 - 25 + 25.8 = 26.8 ETH
    let exec_slot = realistic_weth_balance_slot(executor_address);
    let exec_weth = weth_acc
        .storage
        .get(&exec_slot)
        .map(|s: &EvmStorageSlot| s.present_value())
        .unwrap_or_default();
    assert_eq!(exec_weth, U256::from(26_800_000_000_000_000_000u128));
}

#[test]
fn test_challenger_3hop_route_via_morpho_and_balancer_callbacks() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");
    let pool3_address = address!("4444444444444444444444444444444444444444");
    let morpho_pool = arbitrum::MORPHO_BLUE;
    let balancer_vault = address!("bA1333333333a1BA1108E8412f11850A5C319bA9");

    let initial_balance = 26_000_000_000_000_000_000u128;

    // 1. Morpho Blue Callback 3-Hop
    {
        let db = setup_3hop_env(
            executor_address,
            weth_address,
            pool1_address,
            pool2_address,
            pool3_address,
            initial_balance,
        );

        let params = ExecutionParams {
            weth_amount_to_first_market: TRADE_25_ETH_WEI,
            balance_before: 500_000_000_000_000_000,
            gas_cost: 50_000,
            target1: pool1_address,
            target2: pool2_address,
            token0_amount_out: 50_000_000_000,
            token0_amount_out_index: 0,
            token1_amount_out: 48_000_000_000,
            token1_amount_out_index: 1,
            target3: pool3_address,
            token2_amount_out: TRADE_25_ETH_WEI + 500_000_000_000_000_000, // 25.5 ETH
            token2_amount_out_index: 1,
            hop_count: 3,
        };

        let calldata_163 = encode_163byte_calldata(&params);
        let morpho_payload =
            encode_morpho_callback(U256::from(TRADE_25_ETH_WEI), &calldata_163);

        let tx = TxEnv {
            caller: morpho_pool,
            gas_limit: 1_200_000,
            gas_price: 0,
            kind: TxKind::Call(executor_address),
            value: U256::ZERO,
            data: Bytes::from(morpho_payload),
            ..Default::default()
        };

        let ctx = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
            .with_db(db);

        let mut evm = ctx.build_mainnet();
        let exec_res = evm.transact(tx).expect("Morpho 3-hop callback should succeed");
        assert!(matches!(exec_res.result, ExecutionResult::Success { .. }));
    }

    // 2. Balancer V3 Vault Callback 3-Hop
    {
        let db = setup_3hop_env(
            executor_address,
            weth_address,
            pool1_address,
            pool2_address,
            pool3_address,
            initial_balance,
        );

        let params = ExecutionParams {
            weth_amount_to_first_market: TRADE_25_ETH_WEI,
            balance_before: 500_000_000_000_000_000,
            gas_cost: 50_000,
            target1: pool1_address,
            target2: pool2_address,
            token0_amount_out: 50_000_000_000,
            token0_amount_out_index: 0,
            token1_amount_out: 48_000_000_000,
            token1_amount_out_index: 1,
            target3: pool3_address,
            token2_amount_out: TRADE_25_ETH_WEI + 500_000_000_000_000_000, // 25.5 ETH
            token2_amount_out_index: 1,
            hop_count: 3,
        };

        let calldata_163 = encode_163byte_calldata(&params);
        let balancer_payload = encode_balancer_v3_callback(&calldata_163);

        let tx = TxEnv {
            caller: balancer_vault,
            gas_limit: 1_200_000,
            gas_price: 0,
            kind: TxKind::Call(executor_address),
            value: U256::ZERO,
            data: Bytes::from(balancer_payload),
            ..Default::default()
        };

        let ctx = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
            .with_db(db);

        let mut evm = ctx.build_mainnet();
        let exec_res = evm.transact(tx).expect("Balancer 3-hop callback should succeed");
        assert!(matches!(exec_res.result, ExecutionResult::Success { .. }));
    }
}

// ==============================================================================
// 4. Deliverable 4: ExecutionParams Truncated Calldata Rejection Matrix
// ==============================================================================

#[test]
fn test_challenger_calldata_truncation_rejection_matrix() {
    let buffer = [0x77u8; 200];

    // 1. Rust decode_calldata: lengths 0 through 123 must return CalldataTooShort
    for len in 0..124 {
        let truncated = &buffer[..len];
        let res = decode_calldata(truncated);
        assert_eq!(
            res,
            Err(CalldataError::CalldataTooShort(len)),
            "decode_calldata must fail on length {}",
            len
        );
        let res_124 = decode_124byte_calldata(truncated);
        assert_eq!(
            res_124,
            Err(CalldataError::CalldataTooShort(len)),
            "decode_124byte_calldata must fail on length {}",
            len
        );
    }

    // 2. Lengths 124..162: decode_calldata succeeds as 2-hop (hop_count remains 2)
    for len in 124..163 {
        let slice = &buffer[..len];
        let decoded = decode_calldata(slice).expect(">= 124 bytes should decode 2-hop");
        assert_eq!(decoded.hop_count, 2, "Length {} must default to hop_count 2", len);
        assert_eq!(decoded.target3, Address::ZERO);
    }

    // 3. Length 163 with byte 162 >= 3 decodes as 3-hop
    let mut three_hop_buf = [0x88u8; 163];
    three_hop_buf[162] = 3;
    let decoded_3hop = decode_calldata(&three_hop_buf).expect("163 bytes with hop=3 must decode");
    assert_eq!(decoded_3hop.hop_count, 3);
    assert_ne!(decoded_3hop.target3, Address::ZERO);

    // 4. EVM Pure Yul contract truncation rejection in revm:
    let executor_address = EXECUTOR_ADDR;
    let mut db = InMemoryDB::default();
    let executor_code = executor_runtime_bytecode();
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

    // Test EVM reverts for sub-124 byte calldata
    let test_truncated_lengths = [0, 1, 10, 32, 64, 100, 120, 123];
    for len in test_truncated_lengths {
        let truncated_data = &buffer[..len];
        let tx = TxEnv {
            caller: MOCK_CALLER_ADDRESS,
            gas_limit: 500_000,
            gas_price: 0,
            kind: TxKind::Call(executor_address),
            value: U256::ZERO,
            data: Bytes::copy_from_slice(truncated_data),
            ..Default::default()
        };

        let ctx = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
            .with_db(db.clone());

        let mut evm = ctx.build_mainnet();
        let exec_res = evm.transact(tx).expect("EVM execution must complete");
        assert!(
            matches!(exec_res.result, ExecutionResult::Revert { .. }),
            "Pure Yul executor must revert on truncated calldata length {}",
            len
        );
    }
}
