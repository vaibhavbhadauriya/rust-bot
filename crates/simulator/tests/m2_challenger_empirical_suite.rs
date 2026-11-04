//! Empirical Challenger Verification Test Suite for Milestone M2
//!
//! Authoritative Test Coverage:
//! 1. Flash loan callback execution under realistic returns (25.5 ETH return on 25 ETH borrow)
//!    in Morpho Blue (0x3a48e7e1) and Balancer V3 (0x98dd202a).
//! 2. Un-repaid or zero-profit flash loan callbacks revert as expected:
//!    - Zero profit (25.0 ETH return on 25.0 ETH borrow)
//!    - Negative profit / loss (24.5 ETH return on 25.0 ETH borrow)
//!    - Insolvency / inability to repay flash loan (repay transfer fails)
//!    - Positive profit less than gas cost (reverts with profit in revert data)
//!    - Unauthorized callback callers (Morpho and Balancer spoofing)
//! 3. 3-hop calldata encoding/decoding and revm Cancun execution:
//!    - Roundtrip bit-level encoding/decoding for 163-byte layout
//!    - Direct 3-hop triangular route revm execution
//!    - 3-hop route via Morpho Blue flash loan callback
//!    - 3-hop route via Balancer V3 flash loan callback
//!    - 3-hop zero-profit revert
//!    - 3-hop fast-revert guard on Hop 1

use alloy_primitives::{address, Address, Bytes, U256};
use flash_core::constants::arbitrum;
use flash_executor::bytecode::executor_runtime_bytecode;
use flash_executor::calldata::{
    decode_calldata, encode_124byte_calldata, encode_163byte_calldata, encode_balancer_v3_callback,
    encode_calldata, encode_morpho_callback, ExecutionParams,
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
    state::AccountInfo,
};

const EXECUTOR_ADDR: Address = address!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
const TRADE_25_ETH_WEI: u128 = 25_000_000_000_000_000_000; // 25 ETH

/// Sets up an in-memory DB for 2-hop simulation
fn setup_2hop_env(
    executor_address: Address,
    weth_address: Address,
    pool1_address: Address,
    pool2_address: Address,
    executor_initial_weth: u128,
    pool2_reserves_weth: u128,
) -> InMemoryDB {
    let mut db = InMemoryDB::default();

    // 1. Executor contract
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

    // 2. Realistic WETH contract
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
        StorageKey::from(exec_slot),
        U256::from(executor_initial_weth),
    );

    // 3. Pools 1 and 2
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

        // Slot 8 & 9 reserves for fast-revert guard & getReserves
        let _ = db.insert_account_storage(
            pool,
            StorageKey::from(8),
            U256::from(100_000_000_000_000u128),
        );
        let _ = db.insert_account_storage(
            pool,
            StorageKey::from(9),
            U256::from(100_000_000_000_000u128),
        );
    }

    // Pre-fund Pool 2 with WETH liquidity so it can pay back the executor
    let pool2_slot = realistic_weth_balance_slot(pool2_address);
    let _ = db.insert_account_storage(
        weth_address,
        StorageKey::from(pool2_slot),
        U256::from(pool2_reserves_weth),
    );

    db
}

/// Sets up an in-memory DB for 3-hop simulation
fn setup_3hop_env(
    executor_address: Address,
    weth_address: Address,
    pool1_address: Address,
    pool2_address: Address,
    pool3_address: Address,
    executor_initial_weth: u128,
    pool3_reserves_weth: u128,
) -> InMemoryDB {
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
        StorageKey::from(exec_slot),
        U256::from(executor_initial_weth),
    );

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

        let _ = db.insert_account_storage(
            pool,
            StorageKey::from(8),
            U256::from(100_000_000_000_000u128),
        );
        let _ = db.insert_account_storage(
            pool,
            StorageKey::from(9),
            U256::from(100_000_000_000_000u128),
        );
    }

    let pool3_slot = realistic_weth_balance_slot(pool3_address);
    let _ = db.insert_account_storage(
        weth_address,
        StorageKey::from(pool3_slot),
        U256::from(pool3_reserves_weth),
    );

    db
}

// ============================================================================
// PART 1: Realistic Flash Loan Callback Execution (25.5 ETH return on 25 ETH)
// ============================================================================

#[test]
fn test_empirical_morpho_blue_realistic_25_5_eth_return() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let morpho_pool = arbitrum::MORPHO_BLUE;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    // Executor has 1.0 ETH resting balance + 25.0 ETH loan disbursed by Morpho prior to callback = 26.0 ETH
    let resting_balance = 1_000_000_000_000_000_000u128; // 1 ETH
    let initial_weth = resting_balance + TRADE_25_ETH_WEI; // 26 ETH
    let return_amount = 25_500_000_000_000_000_000u128; // 25.5 ETH

    let db = setup_2hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: return_amount,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata_124 = encode_124byte_calldata(&params);
    let morpho_payload = encode_morpho_callback(U256::from(TRADE_25_ETH_WEI), &calldata_124);

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
    let exec_res = evm.transact(tx).expect("Morpho Blue callback must complete");

    assert!(
        matches!(exec_res.result, ExecutionResult::Success { .. }),
        "Morpho callback failed: {:?}",
        exec_res.result
    );

    let weth_state = exec_res.state.get(&weth_address).unwrap();

    // Verify Pool 1 received exact 25 ETH
    let p1_slot = realistic_weth_balance_slot(pool1_address);
    let p1_bal = weth_state
        .storage
        .get(&p1_slot)
        .map(|s| s.present_value())
        .unwrap_or_default();
    assert_eq!(p1_bal, U256::from(TRADE_25_ETH_WEI), "Pool 1 must receive 25 ETH");

    // Verify Morpho caller received 25 ETH repayment
    let morpho_slot = realistic_weth_balance_slot(morpho_pool);
    let morpho_bal = weth_state
        .storage
        .get(&morpho_slot)
        .map(|s| s.present_value())
        .unwrap_or_default();
    assert_eq!(morpho_bal, U256::from(TRADE_25_ETH_WEI), "Morpho must be repaid 25 ETH");

    // Verify Executor net balance: resting (1.0 ETH) + profit (0.5 ETH) = 1.5 ETH
    let exec_slot = realistic_weth_balance_slot(executor_address);
    let exec_bal = weth_state
        .storage
        .get(&exec_slot)
        .map(|s| s.present_value())
        .unwrap_or_default();
    assert_eq!(
        exec_bal,
        U256::from(1_500_000_000_000_000_000u128),
        "Executor ending balance must be 1.5 ETH (1.0 initial + 0.5 net profit)"
    );
}

#[test]
fn test_empirical_balancer_v3_realistic_25_5_eth_return() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let balancer_vault = arbitrum::BALANCER_V3_VAULT;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    let resting_balance = 1_000_000_000_000_000_000u128; // 1 ETH
    let initial_weth = resting_balance + TRADE_25_ETH_WEI; // 26 ETH
    let return_amount = 25_500_000_000_000_000_000u128; // 25.5 ETH

    let db = setup_2hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: return_amount,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata_124 = encode_124byte_calldata(&params);
    let balancer_payload = encode_balancer_v3_callback(&calldata_124);

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
    let exec_res = evm.transact(tx).expect("Balancer V3 callback must complete");

    assert!(
        matches!(exec_res.result, ExecutionResult::Success { .. }),
        "Balancer callback failed: {:?}",
        exec_res.result
    );

    let weth_state = exec_res.state.get(&weth_address).unwrap();

    // Verify Balancer Vault received 25 ETH repayment
    let balancer_slot = realistic_weth_balance_slot(balancer_vault);
    let vault_bal = weth_state
        .storage
        .get(&balancer_slot)
        .map(|s| s.present_value())
        .unwrap_or_default();
    assert_eq!(vault_bal, U256::from(TRADE_25_ETH_WEI), "Balancer Vault must be repaid 25 ETH");

    // Verify Executor net balance: 1.0 ETH + 0.5 ETH = 1.5 ETH
    let exec_slot = realistic_weth_balance_slot(executor_address);
    let exec_bal = weth_state
        .storage
        .get(&exec_slot)
        .map(|s| s.present_value())
        .unwrap_or_default();
    assert_eq!(
        exec_bal,
        U256::from(1_500_000_000_000_000_000u128),
        "Executor ending balance must be 1.5 ETH (1.0 initial + 0.5 net profit)"
    );
}

// ============================================================================
// PART 2: Un-repaid, Zero-Profit, and Loss Flash Loan Callbacks Revert
// ============================================================================

#[test]
fn test_empirical_morpho_blue_zero_profit_reverts() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let morpho_pool = arbitrum::MORPHO_BLUE;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    let initial_weth = 1_000_000_000_000_000_000u128 + TRADE_25_ETH_WEI;
    // Return exactly 25.0 ETH: zero net profit after repaying 25.0 ETH!
    let return_amount = TRADE_25_ETH_WEI;

    let db = setup_2hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: return_amount,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata_124 = encode_124byte_calldata(&params);
    let morpho_payload = encode_morpho_callback(U256::from(TRADE_25_ETH_WEI), &calldata_124);

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
    let exec_res = evm.transact(tx).expect("Execution completed");

    match exec_res.result {
        ExecutionResult::Revert { output, .. } => {
            assert_eq!(output.len(), 32, "Revert data must be 32 bytes");
            let val = U256::from_be_slice(&output);
            assert_eq!(val, U256::ZERO, "Zero profit revert output must be 0");
        }
        other => panic!("Expected Revert on zero profit, got: {:?}", other),
    }
}

#[test]
fn test_empirical_balancer_v3_zero_profit_reverts() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let balancer_vault = arbitrum::BALANCER_V3_VAULT;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    let initial_weth = 1_000_000_000_000_000_000u128 + TRADE_25_ETH_WEI;
    let return_amount = TRADE_25_ETH_WEI; // Exactly 25 ETH returned

    let db = setup_2hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: return_amount,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata_124 = encode_124byte_calldata(&params);
    let balancer_payload = encode_balancer_v3_callback(&calldata_124);

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
    let exec_res = evm.transact(tx).expect("Execution completed");

    match exec_res.result {
        ExecutionResult::Revert { output, .. } => {
            assert_eq!(output.len(), 32);
            assert_eq!(U256::from_be_slice(&output), U256::ZERO);
        }
        other => panic!("Expected Revert on zero profit, got: {:?}", other),
    }
}

#[test]
fn test_empirical_morpho_blue_loss_reverts() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let morpho_pool = arbitrum::MORPHO_BLUE;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    let initial_weth = 5_000_000_000_000_000_000u128 + TRADE_25_ETH_WEI; // 30 ETH
    let return_amount = 24_500_000_000_000_000_000u128; // 24.5 ETH (0.5 ETH loss!)

    let db = setup_2hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: return_amount,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata_124 = encode_124byte_calldata(&params);
    let morpho_payload = encode_morpho_callback(U256::from(TRADE_25_ETH_WEI), &calldata_124);

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
    let exec_res = evm.transact(tx).expect("Execution completed");

    assert!(
        matches!(exec_res.result, ExecutionResult::Revert { .. }),
        "Trade with net loss must revert"
    );
}

#[test]
fn test_empirical_unrepaid_flash_loan_reverts_due_to_insolvency() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let morpho_pool = arbitrum::MORPHO_BLUE;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    // Executor has 0 resting balance before loan, so only gets the 25 ETH loan
    let initial_weth = TRADE_25_ETH_WEI;
    // Pool returns only 24 ETH, so executor only holds 24 ETH when trying to repay 25 ETH!
    let return_amount = 24_000_000_000_000_000_000u128;

    let db = setup_2hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: return_amount,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata_124 = encode_124byte_calldata(&params);
    let morpho_payload = encode_morpho_callback(U256::from(TRADE_25_ETH_WEI), &calldata_124);

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
    let exec_res = evm.transact(tx).expect("Execution completed");

    // Inability to repay the flash loan must cause EVM revert
    assert!(
        matches!(exec_res.result, ExecutionResult::Revert { .. }),
        "Insolvent flash loan repayment must revert"
    );
}

#[test]
fn test_empirical_positive_profit_below_gas_cost_reverts_with_profit_data() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let morpho_pool = arbitrum::MORPHO_BLUE;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");

    let initial_weth = 1_000_000_000_000_000_000u128 + TRADE_25_ETH_WEI;
    let net_profit_wei = 100_000u128; // 100,000 wei net profit
    let return_amount = TRADE_25_ETH_WEI + net_profit_wei;
    let gas_cost = 200_000u128; // gas_cost (200k) > net_profit (100k)

    let db = setup_2hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 62_500_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: return_amount,
        token1_amount_out_index: 1,
        ..Default::default()
    };

    let calldata_124 = encode_124byte_calldata(&params);
    let morpho_payload = encode_morpho_callback(U256::from(TRADE_25_ETH_WEI), &calldata_124);

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
    let exec_res = evm.transact(tx).expect("Execution completed");

    match exec_res.result {
        ExecutionResult::Revert { output, .. } => {
            assert_eq!(output.len(), 32);
            let profit_in_revert = U256::from_be_slice(&output);
            assert_eq!(
                profit_in_revert,
                U256::from(net_profit_wei),
                "Revert data must contain the exact wethProfit when lt gasCost"
            );
        }
        other => panic!("Expected Revert when profit < gasCost, got: {:?}", other),
    }
}

#[test]
fn test_empirical_callback_spoofing_unauthorized_callers_revert() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");
    let attacker = address!("6666666666666666666666666666666666666666");

    let db = setup_2hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        TRADE_25_ETH_WEI,
        100_000_000_000_000_000_000_000u128,
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

    // 1. Spoofed Morpho callback from attacker
    let morpho_payload = encode_morpho_callback(U256::from(TRADE_25_ETH_WEI), &calldata_124);
    let tx1 = TxEnv {
        caller: attacker,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::ZERO,
        data: Bytes::from(morpho_payload),
        ..Default::default()
    };

    let ctx1 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db.clone());
    let mut evm1 = ctx1.build_mainnet();
    let res1 = evm1.transact(tx1).expect("Execution completed");
    assert!(
        matches!(res1.result, ExecutionResult::Revert { .. }),
        "Attacker spoofing Morpho Blue callback must revert"
    );

    // 2. Spoofed Balancer callback from attacker
    let balancer_payload = encode_balancer_v3_callback(&calldata_124);
    let tx2 = TxEnv {
        caller: attacker,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(executor_address),
        value: U256::ZERO,
        data: Bytes::from(balancer_payload),
        ..Default::default()
    };

    let ctx2 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);
    let mut evm2 = ctx2.build_mainnet();
    let res2 = evm2.transact(tx2).expect("Execution completed");
    assert!(
        matches!(res2.result, ExecutionResult::Revert { .. }),
        "Attacker spoofing Balancer V3 callback must revert"
    );
}

// ============================================================================
// PART 3: 3-Hop Calldata Encoding/Decoding & revm Execution
// ============================================================================

#[test]
fn test_empirical_3hop_calldata_bit_level_roundtrip() {
    let params = ExecutionParams {
        weth_amount_to_first_market: 25_000_000_000_000_000_000,
        balance_before: 1_234_567_890_123_456_789,
        gas_cost: 987_654,
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

    let encoded_163 = encode_163byte_calldata(&params);
    assert_eq!(encoded_163.len(), 163);

    let encoded_dyn = encode_calldata(&params);
    assert_eq!(encoded_dyn.len(), 163);
    assert_eq!(encoded_dyn, encoded_163.to_vec());

    let decoded = decode_calldata(&encoded_163).expect("Decoding must succeed");
    assert_eq!(decoded, params, "3-hop roundtrip decoded must equal input params");
}

#[test]
fn test_empirical_3hop_execution_in_revm() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");
    let pool3_address = address!("4444444444444444444444444444444444444444");

    let initial_weth = 26_000_000_000_000_000_000u128; // 26 ETH
    let return_amount = 25_800_000_000_000_000_000u128; // 25.8 ETH final output from Pool 3

    let db = setup_3hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        pool3_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
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
        token2_amount_out: return_amount,
        token2_amount_out_index: 1,
        hop_count: 3,
    };

    let calldata_163 = encode_163byte_calldata(&params);
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
    let exec_res = evm.transact(tx).expect("3-hop EVM execution completed");

    assert!(
        matches!(exec_res.result, ExecutionResult::Success { .. }),
        "3-hop execution failed: {:?}",
        exec_res.result
    );

    let weth_state = exec_res.state.get(&weth_address).unwrap();

    // Pool 1 received exact 25 ETH
    let p1_slot = realistic_weth_balance_slot(pool1_address);
    let p1_bal = weth_state
        .storage
        .get(&p1_slot)
        .map(|s| s.present_value())
        .unwrap_or_default();
    assert_eq!(p1_bal, U256::from(TRADE_25_ETH_WEI));

    // Executor ending balance: 26 - 25 + 25.8 = 26.8 ETH
    let exec_slot = realistic_weth_balance_slot(executor_address);
    let exec_bal = weth_state
        .storage
        .get(&exec_slot)
        .map(|s| s.present_value())
        .unwrap_or_default();
    assert_eq!(exec_bal, U256::from(26_800_000_000_000_000_000u128));
}

#[test]
fn test_empirical_3hop_morpho_blue_flash_loan_realistic_returns() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");
    let pool3_address = address!("4444444444444444444444444444444444444444");
    let morpho_pool = arbitrum::MORPHO_BLUE;

    let initial_weth = 1_000_000_000_000_000_000u128 + TRADE_25_ETH_WEI; // 1 ETH resting + 25 ETH loan = 26 ETH
    let return_amount = 25_500_000_000_000_000_000u128; // 25.5 ETH return from Pool 3

    let db = setup_3hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        pool3_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
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
        token2_amount_out: return_amount,
        token2_amount_out_index: 1,
        hop_count: 3,
    };

    let calldata_163 = encode_163byte_calldata(&params);
    let morpho_payload = encode_morpho_callback(U256::from(TRADE_25_ETH_WEI), &calldata_163);

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
    let exec_res = evm.transact(tx).expect("Morpho 3-hop callback completed");

    assert!(
        matches!(exec_res.result, ExecutionResult::Success { .. }),
        "Morpho 3-hop callback failed: {:?}",
        exec_res.result
    );

    let weth_state = exec_res.state.get(&weth_address).unwrap();

    // Verify Morpho received 25 ETH repayment
    let morpho_slot = realistic_weth_balance_slot(morpho_pool);
    let morpho_bal = weth_state
        .storage
        .get(&morpho_slot)
        .map(|s| s.present_value())
        .unwrap_or_default();
    assert_eq!(morpho_bal, U256::from(TRADE_25_ETH_WEI));

    // Verify Executor net balance: 1.0 + 0.5 = 1.5 ETH
    let exec_slot = realistic_weth_balance_slot(executor_address);
    let exec_bal = weth_state
        .storage
        .get(&exec_slot)
        .map(|s| s.present_value())
        .unwrap_or_default();
    assert_eq!(exec_bal, U256::from(1_500_000_000_000_000_000u128));
}

#[test]
fn test_empirical_3hop_balancer_v3_flash_loan_realistic_returns() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");
    let pool3_address = address!("4444444444444444444444444444444444444444");
    let balancer_vault = arbitrum::BALANCER_V3_VAULT;

    let initial_weth = 1_000_000_000_000_000_000u128 + TRADE_25_ETH_WEI;
    let return_amount = 25_500_000_000_000_000_000u128; // 25.5 ETH

    let db = setup_3hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        pool3_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
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
        token2_amount_out: return_amount,
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
    let exec_res = evm.transact(tx).expect("Balancer 3-hop callback completed");

    assert!(
        matches!(exec_res.result, ExecutionResult::Success { .. }),
        "Balancer 3-hop callback failed: {:?}",
        exec_res.result
    );

    let weth_state = exec_res.state.get(&weth_address).unwrap();

    let vault_slot = realistic_weth_balance_slot(balancer_vault);
    let vault_bal = weth_state
        .storage
        .get(&vault_slot)
        .map(|s| s.present_value())
        .unwrap_or_default();
    assert_eq!(vault_bal, U256::from(TRADE_25_ETH_WEI));

    let exec_slot = realistic_weth_balance_slot(executor_address);
    let exec_bal = weth_state
        .storage
        .get(&exec_slot)
        .map(|s| s.present_value())
        .unwrap_or_default();
    assert_eq!(exec_bal, U256::from(1_500_000_000_000_000_000u128));
}

#[test]
fn test_empirical_3hop_zero_profit_reverts() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");
    let pool3_address = address!("4444444444444444444444444444444444444444");

    let initial_weth = 25_000_000_000_000_000_000u128; // 25 ETH
    let return_amount = 25_000_000_000_000_000_000u128; // 25 ETH (zero profit: final balance == balanceBefore)

    let db = setup_3hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        pool3_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
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
        token2_amount_out: return_amount,
        token2_amount_out_index: 1,
        hop_count: 3,
    };

    let calldata_163 = encode_163byte_calldata(&params);
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
    let exec_res = evm.transact(tx).expect("Execution completed");

    assert!(
        matches!(exec_res.result, ExecutionResult::Revert { .. }),
        "3-hop zero profit trade must revert"
    );
}

#[test]
fn test_empirical_3hop_fast_revert_guard_hop1() {
    let executor_address = EXECUTOR_ADDR;
    let weth_address = arbitrum::WETH;
    let pool1_address = address!("2222222222222222222222222222222222222222");
    let pool2_address = address!("3333333333333333333333333333333333333333");
    let pool3_address = address!("4444444444444444444444444444444444444444");

    let initial_weth = 26_000_000_000_000_000_000u128;
    let mut db = setup_3hop_env(
        executor_address,
        weth_address,
        pool1_address,
        pool2_address,
        pool3_address,
        initial_weth,
        100_000_000_000_000_000_000_000u128,
    );

    // Set Pool 1 reserves in slot 8 to 1,000, which is strictly less than token0_amount_out (50_000_000_000)
    let _ = db.insert_account_storage(
        pool1_address,
        StorageKey::from(8),
        U256::from(1_000u128),
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: TRADE_25_ETH_WEI,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1_address,
        target2: pool2_address,
        token0_amount_out: 50_000_000_000, // Exceeds reserve (1,000)
        token0_amount_out_index: 0,
        token1_amount_out: 48_000_000_000,
        token1_amount_out_index: 1,
        target3: pool3_address,
        token2_amount_out: 25_800_000_000_000_000_000,
        token2_amount_out_index: 1,
        hop_count: 3,
    };

    let calldata_163 = encode_163byte_calldata(&params);
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
    let exec_res = evm.transact(tx).expect("Execution completed");

    // Fast revert guard must catch insufficient reserves and revert immediately
    assert!(
        matches!(exec_res.result, ExecutionResult::Revert { .. }),
        "Fast-revert guard must revert when reserve is less than requested amountOut"
    );
}
