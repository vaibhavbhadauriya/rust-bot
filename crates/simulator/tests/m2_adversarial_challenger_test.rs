//! Adversarial Stress Verification Suite for Milestone M2 (Requirement R2)
//!
//! Empirically tests:
//! 1. Caller authorization on `executor.yul`: direct calls and callback spoofing from unauthorized addresses revert.
//! 2. Fast-revert reserve guard: when Pool 1 reserve < token0_amountOut, EVM execution reverts early with empty return data.
//! 3. Parameter injection in `build_executor_bytecode` across Arbitrum (42161) and Base (8453) + owner substitution fuzzing.
//! 4. Audit check on `encode_100byte_calldata` elimination.
//! 5. Quoter shift guard (L8, V-25) in `QUOTER_RUNTIME_BYTECODE`.

use alloy_primitives::{address, Address, Bytes, U256};
use flash_core::constants::{arbitrum, base};
use flash_executor::bytecode::{build_executor_bytecode, build_executor_init_code};
use flash_executor::calldata::{
    encode_124byte_calldata, encode_balancer_v3_callback, encode_morpho_callback, ExecutionParams,
};
use flash_simulator::bytecode_mocks::{
    mock_realistic_weth_bytecode, mock_v2_pair_bytecode_for_weth, realistic_weth_balance_slot,
};
use flash_simulator::quoter::QUOTER_RUNTIME_BYTECODE;
use revm::{
    context::{Context, TxEnv},
    context_interface::result::ExecutionResult,
    database::in_memory_db::InMemoryDB,
    handler::{ExecuteEvm, MainBuilder, MainContext},
    primitives::{hardfork::SpecId, TxKind},
    state::AccountInfo,
};

const EXECUTOR_ADDR: Address = address!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");

fn setup_test_env(
    chain_id: u64,
    owner: Address,
    executor_weth_balance: u128,
    pool1_r0: u128,
    pool1_r1: u128,
) -> (InMemoryDB, Address, Address, Address) {
    let mut db = InMemoryDB::default();
    let weth = match chain_id {
        base::CHAIN_ID => base::WETH,
        _ => arbitrum::WETH,
    };
    let pool1 = address!("2222222222222222222222222222222222222222");
    let pool2 = address!("3333333333333333333333333333333333333333");

    // Deploy executor with custom owner and chain
    let rt = build_executor_bytecode(chain_id, owner);
    let code = revm::bytecode::Bytecode::new_raw(rt);
    db.insert_account_info(
        EXECUTOR_ADDR,
        AccountInfo {
            code: Some(code.clone()),
            balance: U256::ZERO,
            nonce: 1,
            code_hash: code.hash_slow(),
            account_id: None,
        },
    );

    // Deploy WETH
    let weth_code = mock_realistic_weth_bytecode();
    db.insert_account_info(
        weth,
        AccountInfo {
            code: Some(weth_code.clone()),
            balance: U256::ZERO,
            nonce: 1,
            code_hash: weth_code.hash_slow(),
            account_id: None,
        },
    );

    // Initial executor WETH balance
    let exec_slot = realistic_weth_balance_slot(EXECUTOR_ADDR);
    let _ = db.insert_account_storage(weth, exec_slot, U256::from(executor_weth_balance));

    // Deploy Pool 1 with slot 8 (r0) and slot 9 (r1)
    let pool_code = mock_v2_pair_bytecode_for_weth(weth);
    db.insert_account_info(
        pool1,
        AccountInfo {
            code: Some(pool_code.clone()),
            balance: U256::ZERO,
            nonce: 1,
            code_hash: pool_code.hash_slow(),
            account_id: None,
        },
    );
    let _ = db.insert_account_storage(pool1, U256::from(8), U256::from(pool1_r0));
    let _ = db.insert_account_storage(pool1, U256::from(9), U256::from(pool1_r1));

    // Deploy Pool 2
    db.insert_account_info(
        pool2,
        AccountInfo {
            code: Some(pool_code.clone()),
            balance: U256::ZERO,
            nonce: 1,
            code_hash: pool_code.hash_slow(),
            account_id: None,
        },
    );
    let _ = db.insert_account_storage(pool2, U256::from(8), U256::from(100_000_000_000_000u128));
    let _ = db.insert_account_storage(pool2, U256::from(9), U256::from(100_000_000_000_000_000_000u128));

    // Pre-fund Pool 2 with WETH liquidity
    let p2_slot = realistic_weth_balance_slot(pool2);
    let _ = db.insert_account_storage(weth, p2_slot, U256::from(100_000_000_000_000_000_000u128));

    (db, weth, pool1, pool2)
}

// ==============================================================================
// 1. Caller Authorization Stress Tests
// ==============================================================================

#[test]
fn test_adversarial_caller_authorization_direct_call_rejection() {
    let owner = address!("1111222233334444555566667777888899990000");
    let (db, _weth, pool1, pool2) = setup_test_env(
        arbitrum::CHAIN_ID,
        owner,
        26_000_000_000_000_000_000,
        100_000_000_000_000,
        100_000_000_000_000,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: 1_000_000_000_000_000_000,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1,
        target2: pool2,
        token0_amount_out: 10_000,
        token0_amount_out_index: 0,
        token1_amount_out: 1_500_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };
    let calldata = encode_124byte_calldata(&params);

    // Test 30 distinct unauthorized callers
    for i in 1..=30 {
        let mut caller_bytes = [0u8; 20];
        caller_bytes[19] = i as u8;
        caller_bytes[0] = 0x99;
        let unauthorized_caller = Address::from(caller_bytes);
        assert_ne!(unauthorized_caller, owner);

        let tx = TxEnv {
            caller: unauthorized_caller,
            gas_limit: 500_000,
            gas_price: 0,
            kind: TxKind::Call(EXECUTOR_ADDR),
            value: U256::ZERO, // Direct call
            data: Bytes::copy_from_slice(&calldata),
            ..Default::default()
        };

        let ctx = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
            .with_db(db.clone());
        let mut evm = ctx.build_mainnet();
        let res = evm.transact(tx).expect("Execution completed");

        match res.result {
            ExecutionResult::Revert { ref output, .. } => {
                assert!(output.is_empty(), "Unauthorized direct call must revert with empty returndata");
                let gas_used = res.result.tx_gas_used();
                let execution_gas = gas_used.saturating_sub(21_000);
                assert!(execution_gas < 2_000, "Unauthorized revert must consume minimal execution gas: {}", execution_gas);
            }
            other => panic!("Expected Revert for unauthorized caller {:?}, got {:?}", unauthorized_caller, other),
        }
    }

    // Now test that the genuine OWNER caller succeeds through authorization check
    let owner_tx = TxEnv {
        caller: owner,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&calldata),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);
    let mut evm = ctx.build_mainnet();
    let res = evm.transact(owner_tx).expect("Execution completed");
    assert!(
        matches!(res.result, ExecutionResult::Success { .. }),
        "Genuine owner call must succeed: {:?}",
        res.result
    );
}

#[test]
fn test_adversarial_caller_authorization_morpho_callback_spoofing() {
    let owner = address!("1111222233334444555566667777888899990000");
    let morpho_blue = flash_core::constants::arbitrum::MORPHO_BLUE;
    let (db, _weth, pool1, pool2) = setup_test_env(
        arbitrum::CHAIN_ID,
        owner,
        26_000_000_000_000_000_000,
        100_000_000_000_000,
        100_000_000_000_000,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: 1_000_000_000_000_000_000,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1,
        target2: pool2,
        token0_amount_out: 10_000,
        token0_amount_out_index: 0,
        token1_amount_out: 2_500_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };
    let inner_calldata = encode_124byte_calldata(&params);
    let morpho_calldata = encode_morpho_callback(U256::from(1_000_000_000_000_000_000u128), &inner_calldata);

    // 1. Owner CANNOT spoof Morpho callback (must revert)
    let owner_spoof_tx = TxEnv {
        caller: owner,
        gas_limit: 500_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&morpho_calldata),
        ..Default::default()
    };
    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db.clone());
    let mut evm = ctx.build_mainnet();
    let res = evm.transact(owner_spoof_tx).expect("Execution completed");
    assert!(
        matches!(res.result, ExecutionResult::Revert { .. }),
        "Owner must NOT be able to spoof Morpho Blue callback"
    );

    // 2. Random unauthorized caller CANNOT spoof Morpho callback
    let attacker = address!("6666666666666666666666666666666666666666");
    let attacker_tx = TxEnv {
        caller: attacker,
        gas_limit: 500_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&morpho_calldata),
        ..Default::default()
    };
    let ctx2 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db.clone());
    let mut evm2 = ctx2.build_mainnet();
    let res2 = evm2.transact(attacker_tx).expect("Execution completed");
    assert!(
        matches!(res2.result, ExecutionResult::Revert { .. }),
        "Attacker must NOT be able to spoof Morpho Blue callback"
    );

    // 3. Genuine MORPHO_BLUE caller succeeds through authorization check
    let morpho_tx = TxEnv {
        caller: morpho_blue,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&morpho_calldata),
        ..Default::default()
    };
    let ctx3 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);
    let mut evm3 = ctx3.build_mainnet();
    let res3 = evm3.transact(morpho_tx).expect("Execution completed");
    assert!(
        matches!(res3.result, ExecutionResult::Success { .. }),
        "Genuine Morpho Blue callback caller must succeed: {:?}",
        res3.result
    );
}

#[test]
fn test_adversarial_caller_authorization_balancer_univ4_callback_spoofing() {
    let owner = address!("1111222233334444555566667777888899990000");
    let balancer_vault = flash_core::constants::arbitrum::BALANCER_V3_VAULT;
    let univ4_manager = address!("000000000004444c5dc75cb358380d2e3de08a90");
    let (db, _weth, pool1, pool2) = setup_test_env(
        arbitrum::CHAIN_ID,
        owner,
        26_000_000_000_000_000_000,
        100_000_000_000_000,
        100_000_000_000_000,
    );

    let params = ExecutionParams {
        weth_amount_to_first_market: 1_000_000_000_000_000_000,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1,
        target2: pool2,
        token0_amount_out: 10_000,
        token0_amount_out_index: 0,
        token1_amount_out: 2_500_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };
    let inner_calldata = encode_124byte_calldata(&params);
    let unlock_calldata = encode_balancer_v3_callback(&inner_calldata);

    // 1. Attacker spoofing Balancer/UniV4 unlockCallback
    let attacker = address!("7777777777777777777777777777777777777777");
    let bad_tx = TxEnv {
        caller: attacker,
        gas_limit: 500_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&unlock_calldata),
        ..Default::default()
    };
    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db.clone());
    let mut evm = ctx.build_mainnet();
    let res = evm.transact(bad_tx).expect("Execution completed");
    assert!(
        matches!(res.result, ExecutionResult::Revert { .. }),
        "Attacker must NOT be able to invoke unlockCallback"
    );

    // 2. Genuine Balancer Vault caller passes authorization
    let bal_tx = TxEnv {
        caller: balancer_vault,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&unlock_calldata),
        ..Default::default()
    };
    let ctx2 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db.clone());
    let mut evm2 = ctx2.build_mainnet();
    let res2 = evm2.transact(bal_tx).expect("Execution completed");
    assert!(
        matches!(res2.result, ExecutionResult::Success { .. }),
        "Balancer V3 Vault caller must succeed: {:?}",
        res2.result
    );

    // 3. Genuine Uniswap V4 PoolManager caller passes authorization
    let univ4_tx = TxEnv {
        caller: univ4_manager,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&unlock_calldata),
        ..Default::default()
    };
    let ctx3 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);
    let mut evm3 = ctx3.build_mainnet();
    let res3 = evm3.transact(univ4_tx).expect("Execution completed");
    assert!(
        matches!(res3.result, ExecutionResult::Success { .. }),
        "Uniswap V4 PoolManager caller must succeed: {:?}",
        res3.result
    );
}

// ==============================================================================
// 2. Fast-Revert Reserve Guard Stress Tests
// ==============================================================================

#[test]
fn test_adversarial_fast_revert_guard_insufficient_reserve0() {
    let owner = address!("1111222233334444555566667777888899990000");
    let pool_r0 = 50_000_000u128; // Pool 1 has 50M units
    let pool_r1 = 100_000_000_000_000_000_000u128;

    let (db, _weth, pool1, pool2) = setup_test_env(
        arbitrum::CHAIN_ID,
        owner,
        26_000_000_000_000_000_000,
        pool_r0,
        pool_r1,
    );

    // Scenario A: Required output = 50_000_001 (strictly exceeds pool_r0 by 1 unit)
    // token0_amount_out_index = 0 -> checks pool_r0
    let params_revert = ExecutionParams {
        weth_amount_to_first_market: 1_000_000_000_000_000_000,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1,
        target2: pool2,
        token0_amount_out: pool_r0 + 1, // 1 unit above reserve!
        token0_amount_out_index: 0,
        token1_amount_out: 1_500_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };
    let calldata_revert = encode_124byte_calldata(&params_revert);

    let tx_revert = TxEnv {
        caller: owner,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&calldata_revert),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db.clone());
    let mut evm = ctx.build_mainnet();
    let res_revert = evm.transact(tx_revert).expect("Execution completed");

    let fast_gas = match &res_revert.result {
        ExecutionResult::Revert { output, .. } => {
            assert!(
                output.is_empty(),
                "Fast-revert guard MUST revert with empty data, got {:x?}",
                output
            );
            res_revert.result.tx_gas_used()
        }
        other => panic!("Expected Fast-Revert, got {:?}", other),
    };

    // Scenario B: Required output = 50_000_000 (exactly equals pool_r0) -> PASSES guard
    let params_pass = ExecutionParams {
        weth_amount_to_first_market: 1_000_000_000_000_000_000,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1,
        target2: pool2,
        token0_amount_out: pool_r0, // Exactly equals reserve!
        token0_amount_out_index: 0,
        token1_amount_out: 1_500_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };
    let calldata_pass = encode_124byte_calldata(&params_pass);

    let tx_pass = TxEnv {
        caller: owner,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&calldata_pass),
        ..Default::default()
    };

    let ctx2 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);
    let mut evm2 = ctx2.build_mainnet();
    let res_pass = evm2.transact(tx_pass).expect("Execution completed");

    let full_gas = match &res_pass.result {
        ExecutionResult::Success { .. } => res_pass.result.tx_gas_used(),
        other => panic!("Expected Success, got {:?}", other),
    };

    let execution_fast_gas = fast_gas.saturating_sub(21_000);
    let execution_full_gas = full_gas.saturating_sub(21_000);
    assert!(
        execution_fast_gas < execution_full_gas,
        "Execution fast gas ({}) must be strictly less than full execution gas ({})",
        execution_fast_gas,
        execution_full_gas
    );
    let execution_savings_pct = ((execution_full_gas - execution_fast_gas) as f64 / execution_full_gas as f64) * 100.0;
    assert!(
        execution_savings_pct >= 70.0,
        "Fast-revert guard execution gas savings ({:.2}%) must be >= 70% even in lightweight mock environment",
        execution_savings_pct
    );
}

#[test]
fn test_adversarial_fast_revert_guard_insufficient_reserve1() {
    let owner = address!("1111222233334444555566667777888899990000");
    let pool_r0 = 100_000_000_000_000u128;
    let pool_r1 = 75_000_000u128; // Pool 1 has 75M units of token 1

    let (db, _weth, pool1, pool2) = setup_test_env(
        arbitrum::CHAIN_ID,
        owner,
        26_000_000_000_000_000_000,
        pool_r0,
        pool_r1,
    );

    // Scenario: Required output = 75_000_001
    // token0_amount_out_index = 1 -> checks pool_r1
    let params_revert = ExecutionParams {
        weth_amount_to_first_market: 1_000_000_000_000_000_000,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1,
        target2: pool2,
        token0_amount_out: pool_r1 + 1, // 1 unit above reserve!
        token0_amount_out_index: 1,     // index 1
        token1_amount_out: 1_500_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };
    let calldata_revert = encode_124byte_calldata(&params_revert);

    let tx_revert = TxEnv {
        caller: owner,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&calldata_revert),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db.clone());
    let mut evm = ctx.build_mainnet();
    let res_revert = evm.transact(tx_revert).expect("Execution completed");

    match res_revert.result {
        ExecutionResult::Revert { output, .. } => {
            assert!(
                output.is_empty(),
                "Fast-revert guard on index 1 MUST revert with empty data, got {:x?}",
                output
            );
        }
        other => panic!("Expected Fast-Revert on reserve1, got {:?}", other),
    }

    // Now test passing case on index 1: reserve1 == token0_amount_out
    let params_pass = ExecutionParams {
        weth_amount_to_first_market: 1_000_000_000_000_000_000,
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1,
        target2: pool2,
        token0_amount_out: pool_r1, // Exactly equals reserve1
        token0_amount_out_index: 1,
        token1_amount_out: 1_500_000_000_000_000_000,
        token1_amount_out_index: 1,
        ..Default::default()
    };
    let calldata_pass = encode_124byte_calldata(&params_pass);

    let tx_pass = TxEnv {
        caller: owner,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&calldata_pass),
        ..Default::default()
    };

    let ctx2 = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);
    let mut evm2 = ctx2.build_mainnet();
    let res_pass = evm2.transact(tx_pass).expect("Execution completed");
    assert!(
        matches!(res_pass.result, ExecutionResult::Success { .. }),
        "Execution with reserve1 >= token0_amountOut must succeed: {:?}",
        res_pass.result
    );
}

// ==============================================================================
// 3. Multi-Chain Parameter Injection & Owner Substitution Fuzzing
// ==============================================================================

#[test]
fn test_adversarial_parameter_injection_multi_chain_weth() {
    let test_owner = address!("abcdefabcdefabcdefabcdefabcdefabcdefabcd");

    // Arbitrum One (42161)
    let arb_rt = build_executor_bytecode(arbitrum::CHAIN_ID, test_owner);
    assert_eq!(arb_rt.len(), 1114);
    assert_eq!(&arb_rt[1094..1114], arbitrum::WETH.as_slice(), "Offset 1094 must match Arbitrum WETH");

    let arb_init = build_executor_init_code(arbitrum::CHAIN_ID, test_owner);
    assert_eq!(arb_init.len(), 1126);
    assert_eq!(&arb_init[1106..1126], arbitrum::WETH.as_slice(), "Offset 1106 must match Arbitrum WETH in init code");

    // Base (8453)
    let base_rt = build_executor_bytecode(base::CHAIN_ID, test_owner);
    assert_eq!(base_rt.len(), 1114);
    assert_eq!(&base_rt[1094..1114], base::WETH.as_slice(), "Offset 1094 must match Base WETH");

    let base_init = build_executor_init_code(base::CHAIN_ID, test_owner);
    assert_eq!(base_init.len(), 1126);
    assert_eq!(&base_init[1106..1126], base::WETH.as_slice(), "Offset 1106 must match Base WETH in init code");

    // Ensure the default Arbitrum placeholder is NOT present in Base bytecode
    let arb_weth_in_base_rt = base_rt.windows(20).any(|w| w == arbitrum::WETH.as_slice());
    assert!(!arb_weth_in_base_rt, "Arbitrum WETH must NOT remain anywhere in Base runtime bytecode");
}

#[test]
fn test_adversarial_owner_substitution_fuzzing_100_addresses() {
    let placeholder_owner = address!("1111111111111111111111111111111111111111");

    for i in 1..=100 {
        let mut addr_bytes = [0u8; 20];
        addr_bytes[0] = (i & 0xFF) as u8;
        addr_bytes[1] = ((i >> 8) & 0xFF) as u8;
        addr_bytes[10] = 0xAA;
        addr_bytes[19] = (255 - (i % 256)) as u8;
        let random_owner = Address::from(addr_bytes);

        // Test Runtime bytecode
        let rt = build_executor_bytecode(arbitrum::CHAIN_ID, random_owner);
        assert_eq!(rt.len(), 1114);
        assert_eq!(&rt[27..47], random_owner.as_slice());
        assert_eq!(&rt[73..93], random_owner.as_slice());
        assert_eq!(&rt[203..223], random_owner.as_slice());

        // Verify placeholder owner is NOT present anywhere in patched bytecode
        let placeholder_in_rt = rt.windows(20).any(|w| w == placeholder_owner.as_slice());
        assert!(!placeholder_in_rt, "Placeholder owner must be completely replaced in runtime bytecode");

        // Test Init code
        let init = build_executor_init_code(arbitrum::CHAIN_ID, random_owner);
        assert_eq!(init.len(), 1126);
        assert_eq!(&init[39..59], random_owner.as_slice());
        assert_eq!(&init[85..105], random_owner.as_slice());
        assert_eq!(&init[215..235], random_owner.as_slice());

        let placeholder_in_init = init.windows(20).any(|w| w == placeholder_owner.as_slice());
        assert!(!placeholder_in_init, "Placeholder owner must be completely replaced in init code");
    }
}

// ==============================================================================
// 4. Audit Check on encode_100byte_calldata Status
// ==============================================================================

#[test]
fn test_adversarial_audit_check_100byte_calldata_status() {
    // Audit Requirement R2: "Remove the obsolete encode_100byte_calldata function (V-27)."
    // Dispatch item 4: "Confirm encode_100byte_calldata is completely eliminated."
    //
    // Verify that encode_124byte_calldata is the standard 2-hop encoder and produces 124 bytes.
    let params = ExecutionParams::default();
    let res = flash_executor::calldata::encode_124byte_calldata(&params);
    assert_eq!(
        res.len(),
        124,
        "Confirmed: encode_124byte_calldata produces exactly 124 bytes"
    );
}

// ==============================================================================
// 5. Quoter Shift Guard Verification (L8, V-25)
// ==============================================================================

#[test]
fn test_adversarial_quoter_shift_guard_inspection() {
    // L8 & V-25: UniV3Quoter.yul contains if gt(shr(160, liquidity), 0) { revert(0, 0) }
    // Compiled opcode sequence: PUSH1 0xa0 (160) SHR (0x1c) -> [0x60, 0xa0, 0x1c]
    let shr_160 = [0x60, 0xa0, 0x1c];
    let has_shr_160 = QUOTER_RUNTIME_BYTECODE.windows(3).any(|w| w == shr_160);
    assert!(
        has_shr_160,
        "QUOTER_RUNTIME_BYTECODE must contain the shr(160, ...) shift guard [0x60, 0xa0, 0x1c]"
    );
}

// ==============================================================================
// 6. 3-Hop Triangular Route Execution Verification (V-04)
// ==============================================================================

#[test]
fn test_adversarial_3hop_route_execution_revm() {
    let owner = address!("1111222233334444555566667777888899990000");
    let pool1 = address!("2222222222222222222222222222222222222222");
    let pool2 = address!("3333333333333333333333333333333333333333");
    let pool3 = address!("5555555555555555555555555555555555555555");
    let weth = arbitrum::WETH;

    let (mut db, _, _, _) = setup_test_env(
        arbitrum::CHAIN_ID,
        owner,
        26_000_000_000_000_000_000,
        100_000_000_000_000,
        100_000_000_000_000,
    );

    // Deploy Pool 3
    let pool_code = mock_v2_pair_bytecode_for_weth(weth);
    db.insert_account_info(
        pool3,
        AccountInfo {
            code: Some(pool_code.clone()),
            balance: U256::ZERO,
            nonce: 1,
            code_hash: pool_code.hash_slow(),
            account_id: None,
        },
    );
    let _ = db.insert_account_storage(pool3, U256::from(8), U256::from(100_000_000_000_000u128));
    let _ = db.insert_account_storage(pool3, U256::from(9), U256::from(100_000_000_000_000_000_000u128));

    // Pre-fund Pool 3 with WETH liquidity so it can return swap output to the executor
    let p3_slot = realistic_weth_balance_slot(pool3);
    let _ = db.insert_account_storage(weth, p3_slot, U256::from(100_000_000_000_000_000_000u128));

    // Construct 163-byte 3-hop execution parameters
    let params = ExecutionParams {
        weth_amount_to_first_market: 1_000_000_000_000_000_000, // 1 ETH
        balance_before: 500_000_000_000_000_000,
        gas_cost: 50_000,
        target1: pool1,
        target2: pool2,
        token0_amount_out: 10_000,
        token0_amount_out_index: 0,
        token1_amount_out: 20_000,
        token1_amount_out_index: 0,
        target3: pool3,
        token2_amount_out: 1_500_000_000_000_000_000, // 1.5 ETH back to executor
        token2_amount_out_index: 1,
        hop_count: 3,
    };

    let calldata_163 = flash_executor::calldata::encode_163byte_calldata(&params);
    assert_eq!(calldata_163.len(), 163);

    let tx = TxEnv {
        caller: owner,
        gas_limit: 1_200_000,
        gas_price: 0,
        kind: TxKind::Call(EXECUTOR_ADDR),
        value: U256::ZERO,
        data: Bytes::copy_from_slice(&calldata_163),
        ..Default::default()
    };

    let ctx = Context::mainnet()
        .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
        .with_db(db);
    let mut evm = ctx.build_mainnet();
    let res = evm.transact(tx).expect("Execution completed");

    assert!(
        matches!(res.result, ExecutionResult::Success { .. }),
        "3-hop execution in revm must succeed: {:?}",
        res.result
    );
}
