//! End-to-End revm Integration Test for 25 ETH Arbitrage Trade
//!
//! Validates:
//! 1. 25 ETH trade (25 * 10^18 wei = 25_000_000_000_000_000_000 u128) executes through Pure Yul executor.
//! 2. Execution succeeds in revm Cancun mode with zero EVM reverts.
//! 3. Absence of `.to::<u64>()` truncation: 25 ETH exceeds u64::MAX (18.44 ETH); the exact 25 ETH amount
//!    is transferred into Pool 1 without losing high bits.
//! 4. Atomic profit enforcement: net profit exceeds gas cost.
//! 5. Chain parameterization on Arbitrum One (42161) and Base (8453).
//! 6. Flash loan callback unwrapping (Morpho Blue & Balancer V3 / Uniswap V4).

use alloy_primitives::{address, Address, Bytes, U256};
use flash_core::constants::{arbitrum, base};
use flash_executor::bytecode::{build_executor_bytecode, executor_runtime_bytecode};
use flash_executor::calldata::{
    encode_124byte_calldata, encode_balancer_v3_callback, encode_morpho_callback, ExecutionParams,
};
use flash_simulator::bytecode_mocks::{
    mock_realistic_weth_bytecode, mock_v2_pair_bytecode_for_weth, realistic_weth_balance_slot,
    MOCK_CALLER_ADDRESS,
};
use revm::{
    context::{Context, TxEnv},
    context_interface::result::ExecutionResult,
    database::in_memory_db::InMemoryDB,
    handler::{ExecuteEvm, MainBuilder, MainContext},
    primitives::{hardfork::SpecId, StorageKey, TxKind},
    state::{AccountInfo, EvmStorageSlot},
};

/// 25 ETH in wei: 25 * 10^18 = 25_000_000_000_000_000_000
const TRADE_25_ETH_WEI: u128 = 25_000_000_000_000_000_000;

/// Canonical non-precompile executor address for revm simulation (avoids precompile 0x01 collision)
const EXECUTOR_ADDR: Address = address!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

/// Sets up an in-memory database with the Pure Yul executor, realistic WETH, and 2 mock V2 pools.
fn setup_simulation_env(
    _chain_id: u64,
    executor_code: revm::bytecode::Bytecode,
    executor_address: Address,
    weth_address: Address,
    pool1_address: Address,
    pool2_address: Address,
    executor_initial_weth: u128,
) -> InMemoryDB {
    let mut db = InMemoryDB::default();

    // 1. Deploy Pure Yul Executor
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

    // 2. Deploy Realistic Mock WETH with real balance tracking
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

    // Fund executor with initial WETH balance
    let exec_slot = realistic_weth_balance_slot(executor_address);
    let _ = db.insert_account_storage(
        weth_address,
        StorageKey::from(exec_slot),
        U256::from(executor_initial_weth),
    );

    // 3. Deploy Pool 1 (Mock V2 Pair)
    let pool_code = mock_v2_pair_bytecode_for_weth(weth_address);
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

    // Pack non-zero reserves into slot 8 for fast-revert guard: reserve0 = 100M USDC, reserve1 = 40,000 ETH
    let pool1_reserves: U256 = U256::from(100_000_000_000_000u128)
        | (U256::from(40_000_000_000_000_000_000_000u128) << 112);
    let _ = db.insert_account_storage(
        pool1_address,
        StorageKey::from(8),
        pool1_reserves,
    );

    // 4. Deploy Pool 2 (Mock V2 Pair)
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

    let pool2_reserves: U256 = U256::from(100_000_000_000_000u128)
        | (U256::from(39_000_000_000_000_000_000_000u128) << 112);
    let _ = db.insert_account_storage(
        pool2_address,
        StorageKey::from(8),
        pool2_reserves,
    );

    // Pre-fund Pool 2 with WETH liquidity so it can return swap output to the executor
    let pool2_slot = realistic_weth_balance_slot(pool2_address);
    let _ = db.insert_account_storage(
        weth_address,
        StorageKey::from(pool2_slot),
        U256::from(100_000_000_000_000_000_000_000u128), // 100,000 ETH
    );

    db
}

#[test]
fn test_revm_25eth_trade_execution_without_u64_truncation() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    // Initial funding: 26 ETH (25 ETH to be transferred to Pool 1 + 1 ETH remaining)
    let executor_initial_weth = 26_000_000_000_000_000_000u128; // 26 ETH
    let db = setup_simulation_env(
        arbitrum::CHAIN_ID,
        executor_runtime_bytecode(),
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        executor_initial_weth,
    );

    // Construct 124-byte execution parameters
    // weth_amount_to_first_market = 25 ETH (strictly > u64::MAX)
    // balance_before = 0.5 ETH (500_000_000_000_000_000 wei)
    // Remaining balance after 25 ETH transfer = 1.0 ETH (1_000_000_000_000_000_000 wei)
    // Profit = balanceAfter - balanceBefore = 1.0 ETH - 0.5 ETH = 0.5 ETH > gas_cost (50_000 wei)
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

    let calldata_124 = encode_124byte_calldata(&params);
    assert_eq!(calldata_124.len(), 124);

    // Build TxEnv for revm Cancun execution
    let tx = TxEnv {
        caller: MOCK_CALLER_ADDRESS,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::ZERO, // 0 wei triggers arbitrage path
        data: Bytes::copy_from_slice(&calldata_124),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);

    let mut evm = ctx.build_mainnet();
    let exec_res = evm.transact(tx).expect("revm Cancun execution should complete");

    // 1. Assert execution succeeds
    assert!(
        matches!(exec_res.result, ExecutionResult::Success { .. }),
        "Execution failed: {:?}",
        exec_res.result
    );

    let weth_account = exec_res
        .state
        .get(&weth_address)
        .expect("WETH account must be in modified state");

    // 2. Assert Pool 1 received exactly 25 ETH without u64 truncation
    let pool1_slot = realistic_weth_balance_slot(pool1_address);
    let pool1_weth = weth_account
        .storage
        .get(&pool1_slot)
        .map(|slot: &EvmStorageSlot| slot.present_value())
        .unwrap_or_default();

    let expected_25_eth = U256::from(TRADE_25_ETH_WEI);
    assert_eq!(
        pool1_weth, expected_25_eth,
        "Pool 1 must hold exactly 25 ETH in WETH"
    );

    // 3. Mathematical proof of absence of u64 truncation
    let u64_max_as_u256 = U256::from(u64::MAX);
    assert!(
        pool1_weth > u64_max_as_u256,
        "25 ETH ({}) must strictly exceed u64::MAX ({})",
        pool1_weth,
        u64_max_as_u256
    );

    let truncated_u64_val = (TRADE_25_ETH_WEI as u64) as u128;
    assert_ne!(
        pool1_weth,
        U256::from(truncated_u64_val),
        "Pool 1 balance must NOT match truncated u64 value ({})",
        truncated_u64_val
    );
    assert_eq!(
        truncated_u64_val, 6_553_255_926_290_448_384,
        "Counterfactual verification: u64 truncation maps 25 ETH to ~6.55 ETH"
    );

    // 4. Verify executor remaining WETH balance and profit
    let exec_slot = realistic_weth_balance_slot(executor_address);
    let exec_weth = weth_account
        .storage
        .get(&exec_slot)
        .map(|slot: &EvmStorageSlot| slot.present_value())
        .unwrap_or_default();

    assert_eq!(
        exec_weth,
        U256::from(26_500_000_000_000_000_000u128),
        "Executor balance must equal initial (26 ETH) - transferred (25 ETH) + returned (25.5 ETH) = 26.5 ETH"
    );
    assert!(
        exec_weth > U256::from(executor_initial_weth),
        "balanceAfter must exceed balanceBefore"
    );
    let profit = exec_weth - U256::from(executor_initial_weth);
    assert!(
        profit >= U256::from(params.gas_cost),
        "Profit must satisfy atomic profit check (profit {} >= gas_cost {})",
        profit,
        params.gas_cost
    );
}

#[test]
fn test_revm_25eth_trade_cancun_arbitrum_and_base_parameterized() {
    let test_chains = [
        (arbitrum::CHAIN_ID, arbitrum::WETH, "Arbitrum One"),
        (base::CHAIN_ID, base::WETH, "Base"),
    ];

    let owner = address!("4444444444444444444444444444444444444444");
    let executor_address = EXECUTOR_ADDR;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    for (chain_id, weth_address, chain_name) in test_chains {
        // Build parameterized bytecode for this chain
        let parameterized_bytecode_bytes = build_executor_bytecode(chain_id, owner);
        let executor_bytecode = revm::bytecode::Bytecode::new_raw(parameterized_bytecode_bytes);

        let db = setup_simulation_env(
            chain_id,
            executor_bytecode,
            executor_address,
            weth_address,
            pool1_address,
            pool2_address,
            26_000_000_000_000_000_000,
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

        let calldata_124 = encode_124byte_calldata(&params);

        let tx = TxEnv {
            caller: owner,
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
        let exec_res = evm.transact(tx).expect("Execution should complete");

        assert!(
            matches!(exec_res.result, ExecutionResult::Success { .. }),
            "Execution failed on {}: {:?}",
            chain_name,
            exec_res.result
        );

        let weth_account = exec_res
            .state
            .get(&weth_address)
            .expect("WETH account must be in state");
        let pool1_slot = realistic_weth_balance_slot(pool1_address);
        let pool1_weth = weth_account
            .storage
            .get(&pool1_slot)
            .map(|s: &EvmStorageSlot| s.present_value())
            .unwrap_or_default();

        assert_eq!(
            pool1_weth,
            U256::from(TRADE_25_ETH_WEI),
            "Pool 1 on {} must receive untruncated 25 ETH",
            chain_name
        );
    }
}

#[test]
fn test_revm_25eth_trade_morpho_flash_loan_callback() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let morpho_pool = flash_core::constants::arbitrum::MORPHO_BLUE;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    // In Morpho flash loan callback:
    // Executor has 26 ETH initial resting balance
    let initial_balance = 26_000_000_000_000_000_000u128; // 26 ETH
    let db = setup_simulation_env(
        arbitrum::CHAIN_ID,
        executor_runtime_bytecode(),
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        initial_balance,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: TRADE_25_ETH_WEI + 500_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata_124 = encode_124byte_calldata(&params);
    let callback_payload = encode_morpho_callback(U256::from(TRADE_25_ETH_WEI), &calldata_124);

    // Call from Morpho pool to executor onMorphoFlashLoan
    let tx = TxEnv {
        caller: morpho_pool,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::ZERO,
        data: Bytes::from(callback_payload),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);

    let mut evm = ctx.build_mainnet();
    let exec_res = evm.transact(tx).expect("Execution should complete");

    assert!(
        matches!(exec_res.result, ExecutionResult::Success { .. }),
        "Morpho callback failed: {:?}",
        exec_res.result
    );

    let weth_account = exec_res
        .state
        .get(&weth_address)
        .expect("WETH account must be in state");

    // Assert Pool 1 received untruncated 25 ETH
    let pool1_slot = realistic_weth_balance_slot(pool1_address);
    let pool1_weth = weth_account
        .storage
        .get(&pool1_slot)
        .map(|s: &EvmStorageSlot| s.present_value())
        .unwrap_or_default();
    assert_eq!(pool1_weth, U256::from(TRADE_25_ETH_WEI));

    // Assert Morpho caller received 25 ETH repayment
    let morpho_slot = realistic_weth_balance_slot(morpho_pool);
    let morpho_weth = weth_account
        .storage
        .get(&morpho_slot)
        .map(|s: &EvmStorageSlot| s.present_value())
        .unwrap_or_default();
    assert_eq!(morpho_weth, U256::from(TRADE_25_ETH_WEI));

    // Remaining executor balance: 26 - 25 (to pool1) + 25.5 (from pool2) - 25 (to morpho) = 1.5 ETH
    let exec_slot = realistic_weth_balance_slot(executor_address);
    let exec_weth = weth_account
        .storage
        .get(&exec_slot)
        .map(|s: &EvmStorageSlot| s.present_value())
        .unwrap_or_default();
    assert_eq!(exec_weth, U256::from(1_500_000_000_000_000_000u128));
}

#[test]
fn test_revm_25eth_trade_balancer_univ4_unlock_callback() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let vault = address!("bA1333333333a1BA1108E8412f11850A5C319bA9");
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    let initial_balance = 26_000_000_000_000_000_000u128; // 26 ETH
    let db = setup_simulation_env(
        arbitrum::CHAIN_ID,
        executor_runtime_bytecode(),
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        initial_balance,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: TRADE_25_ETH_WEI + 500_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata_124 = encode_124byte_calldata(&params);
    let callback_payload = encode_balancer_v3_callback(&calldata_124);

    let tx = TxEnv {
        caller: vault,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::ZERO,
        data: Bytes::from(callback_payload),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);

    let mut evm = ctx.build_mainnet();
    let exec_res = evm.transact(tx).expect("Execution should complete");

    assert!(
        matches!(exec_res.result, ExecutionResult::Success { .. }),
        "Balancer callback failed: {:?}",
        exec_res.result
    );

    let weth_account = exec_res
        .state
        .get(&weth_address)
        .expect("WETH account must be in state");
    let pool1_slot = realistic_weth_balance_slot(pool1_address);
    let pool1_weth = weth_account
        .storage
        .get(&pool1_slot)
        .map(|s: &EvmStorageSlot| s.present_value())
        .unwrap_or_default();
    assert_eq!(pool1_weth, U256::from(TRADE_25_ETH_WEI));

    // Assert Balancer vault received 25 ETH repayment
    let vault_slot = realistic_weth_balance_slot(vault);
    let vault_weth = weth_account
        .storage
        .get(&vault_slot)
        .map(|s: &EvmStorageSlot| s.present_value())
        .unwrap_or_default();
    assert_eq!(vault_weth, U256::from(TRADE_25_ETH_WEI));
}

#[test]
fn test_revm_25eth_trade_atomic_profit_rejection_boundary() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    // Executor has 26 ETH initially; after 25 ETH transfer, 1 ETH remains.
    // Set balance_before = 0.5 ETH, so profit is 0.5 ETH (500_000_000_000_000_000 wei).
    // Set gas_cost = 2.0 ETH (2_000_000_000_000_000_000 wei).
    // Since profit (0.5 ETH) < gas_cost (2.0 ETH), executor must atomically revert!
    let db = setup_simulation_env(
        arbitrum::CHAIN_ID,
        executor_runtime_bytecode(),
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        26_000_000_000_000_000_000,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 2_000_000_000_000_000_000, // 2 ETH gas cost exceeds 0.5 ETH profit
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: 25_500_000_000_000_000_000,
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
    let exec_res = evm.transact(tx).expect("Execution should complete");

    // Must revert with 32-byte error data containing the profit
    match exec_res.result {
        ExecutionResult::Revert { output, .. } => {
            assert_eq!(output.len(), 32, "Revert data must be 32 bytes");
            let reported_profit = U256::from_be_slice(&output);
            assert_eq!(
                reported_profit,
                U256::from(500_000_000_000_000_000u128),
                "Revert data must contain the exact wethProfit"
            );
        }
        other => panic!("Expected ExecutionResult::Revert, got {:?}", other),
    }
}

#[test]
fn test_revm_25eth_u64_truncation_mathematical_proof() {
    let trade_size_u128 = TRADE_25_ETH_WEI;
    let u64_max = u64::MAX as u128;

    // 1. Prove 25 ETH > u64::MAX
    assert!(trade_size_u128 > u64_max);

    // 2. Exact overflow delta
    let delta = trade_size_u128 - u64_max;
    assert_eq!(delta, 6_553_255_926_290_448_385);

    // 3. Modulo 2^64 truncation
    let truncated = (trade_size_u128 as u64) as u128;
    assert_eq!(truncated, 6_553_255_926_290_448_384);

    // 4. Lost value under u64 truncation is 18.4467 ETH
    let lost_value = trade_size_u128 - truncated;
    assert_eq!(lost_value, 18_446_744_073_709_551_616);
    assert_eq!(lost_value, 1u128 << 64);
}
