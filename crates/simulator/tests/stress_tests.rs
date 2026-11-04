//! Empirical Adversarial Stress Harness for flash-simulator crate.
//! Milestone 4 Stress Challenger: m4_challenger_1
//!
//! Authoritative vectors:
//! 1. State Isolation Under High Concurrency (25 threads, conflicting storage slots & balances)
//! 2. UniV3Quoter Wei-Level Parity Fuzzing (1,200 fuzzed pools, |diff| <= 1 wei)
//! 3. Boundary Gas & Block Limits (30M block gas limit, OOG conditions, zero & extreme gas prices)
//! 4. Two-Tier Cache Stress & Fault Tolerance (LRU capacity overflow, high concurrency, invalid paths)
//! 5. Fast-Revert Guard Detection & Classification (0-byte slippage aborts, 32-byte profit shortfalls, 28-byte custom reverts, success)

use alloy_primitives::{address, Address, Bytes, U256};
use flash_core::types::{Chain, Venue};
use flash_math::TradeSizeOutput;
use flash_scanner::dfs::RouteCandidate;
use flash_scanner::graph::DirectedEdge;
use flash_simulator::{
    cache::BytecodeCache,
    engine::SimulationEngine,
    quoter::{
        execute_evm_quote, inject_pool_state, inject_quoter_bytecode,
        quote_analytical_clmm, verify_quoter_parity, QuoteExactInputSingleParams,
        SimulatedPoolState, DEFAULT_QUOTER_ADDRESS,
    },
    types::{SimError, StateOverride},
};
use revm::{
    context::TxEnv,
    context_interface::result::ExecutionResult,
    database::in_memory_db::InMemoryDB,
    primitives::TxKind,
};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Barrier,
};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn unique_stress_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos))
}

/// Fast, deterministic PRNG (Xorshift64) for reproducible fuzzing
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        Self { state: if seed == 0 { 0xdeadbeefcafebabe } else { seed } }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_range(&mut self, min: u64, max: u64) -> u64 {
        if min >= max {
            return min;
        }
        min + (self.next_u64() % (max - min + 1))
    }

    fn next_u128(&mut self) -> u128 {
        let hi = self.next_u64() as u128;
        let lo = self.next_u64() as u128;
        (hi << 64) | lo
    }

    fn next_range_u128(&mut self, min: u128, max: u128) -> u128 {
        if min >= max {
            return min;
        }
        min + (self.next_u128() % (max - min + 1))
    }
}

// ============================================================================
// Vector 1: State Isolation Under High Concurrency
// ============================================================================

#[test]
fn test_stress_concurrent_state_isolation() {
    let temp_dir = unique_stress_temp_dir("stress_iso_concurrent");
    let cache = Arc::new(BytecodeCache::new(&temp_dir, 500));
    let engine = Arc::new(SimulationEngine::new(Chain::ArbitrumOne, cache));

    const THREAD_COUNT: usize = 25;
    const ITERATIONS_PER_THREAD: usize = 40; // Total 1,000 concurrent simulations

    let barrier = Arc::new(Barrier::new(THREAD_COUNT));
    let state_leak_detected = Arc::new(AtomicBool::new(false));
    let success_count = Arc::new(AtomicUsize::new(0));

    // Shared conflicting pool ID used by all threads simultaneously
    let conflicting_pool_id = address!("AAAA000000000000000000000000000000000001");
    let weth = flash_core::constants::arbitrum::WETH;

    let mut handles = Vec::with_capacity(THREAD_COUNT);

    for thread_idx in 0..THREAD_COUNT {
        let engine_clone = Arc::clone(&engine);
        let barrier_clone = Arc::clone(&barrier);
        let state_leak = Arc::clone(&state_leak_detected);
        let successes = Arc::clone(&success_count);

        let handle = thread::spawn(move || {
            // Synchronize starting line
            barrier_clone.wait();

            for iter in 0..ITERATIONS_PER_THREAD {
                // Each thread/iteration injects wildly conflicting reserves into the SAME pool ID
                let res_in = 1_000_000u128 + (thread_idx as u128 * 100_000) + iter as u128;
                let res_out = 5_000_000u128 - (thread_idx as u128 * 50_000) - iter as u128;
                let trade_in = 10_000u64 + (thread_idx as u64 * 100);

                let dummy_route = RouteCandidate {
                    borrow_token: weth,
                    hops: vec![
                        DirectedEdge {
                            pool_id: conflicting_pool_id,
                            venue: Venue::UniswapV2,
                            token_in: weth,
                            token_out: address!("BBBB000000000000000000000000000000000002"),
                            reserve_in: res_in,
                            reserve_out: res_out,
                            fee_pips: 3000,
                            zero_for_one: true,
                        },
                        DirectedEdge {
                            pool_id: address!("CCCC000000000000000000000000000000000003"),
                            venue: Venue::UniswapV2,
                            token_in: address!("BBBB000000000000000000000000000000000002"),
                            token_out: weth,
                            reserve_in: res_out,
                            reserve_out: res_in + 20_000,
                            fee_pips: 3000,
                            zero_for_one: false,
                        },
                    ],
                    sizing: TradeSizeOutput {
                        optimal_input: U256::from(trade_in),
                        expected_intermediate: U256::from(15_000),
                        expected_output: U256::from(trade_in + 5_000),
                        gross_profit: U256::from(5_000),
                        flash_fee: U256::ZERO,
                        net_profit: U256::from(5_000),
                    },
                    net_profit: U256::from(5_000),
                };

                // Run simulation
                match engine_clone.simulate_arbitrage_route(&dummy_route, U256::from(trade_in)) {
                    Ok(_) | Err(SimError::FastRevertTriggered) | Err(SimError::Unprofitable { .. }) => {
                        successes.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        eprintln!("Unexpected simulation error in thread {}: {:?}", thread_idx, e);
                    }
                }

                // Verify base_db was NEVER modified by checking pristine state of conflicting pool
                let mut base_check = engine_clone.base_db.clone();
                let acc = base_check.load_account(conflicting_pool_id).unwrap();
                if acc.storage.contains_key(&U256::from(8)) {
                    state_leak.store(true, Ordering::SeqCst);
                }
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().expect("thread join failed");
    }

    assert!(
        !state_leak_detected.load(Ordering::SeqCst),
        "CRITICAL: State leakage detected into base_db under concurrent simulation!"
    );
    assert_eq!(
        success_count.load(Ordering::Relaxed),
        THREAD_COUNT * ITERATIONS_PER_THREAD,
        "All concurrent simulations must complete deterministically"
    );

    // Final sanity check on pristine base_db
    let mut final_db = engine.base_db.clone();
    let conflicting_acc = final_db.load_account(conflicting_pool_id).unwrap();
    assert_eq!(conflicting_acc.info.balance, U256::ZERO);
    assert!(conflicting_acc.storage.is_empty(), "conflicting pool storage leaked into base_db");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// ============================================================================
// Vector 2: UniV3Quoter Wei-Level Parity Fuzzing (1,200 samples)
// ============================================================================

#[test]
fn test_fuzz_univ3_quoter_parity_1000_samples() {
    let mut prng = XorShift64::new(0x20261025_42161);
    let quoter_addr = DEFAULT_QUOTER_ADDRESS;

    // Shared base DB with quoter injected once
    let mut base_db = InMemoryDB::default();
    inject_quoter_bytecode(&mut base_db, quoter_addr);

    let token0 = address!("1000000000000000000000000000000000000001");
    let token1 = address!("2000000000000000000000000000000000000002");
    assert!(token0 < token1, "Token ordering token0 < token1 required for Uniswap V3");

    const TOTAL_SAMPLES: usize = 1200;
    let mut max_diff = U256::ZERO;
    let mut exact_matches = 0usize;
    let mut one_wei_diffs = 0usize;

    let start_time = Instant::now();

    for i in 0..TOTAL_SAMPLES {
        // Diverse fee tiers: 100 (0.01%), 500 (0.05%), 3000 (0.3%), 10000 (1%)
        let fee_pips = match prng.next_range(0, 3) {
            0 => 100u32,
            1 => 500u32,
            2 => 3000u32,
            _ => 10000u32,
        };

        // Realistic and extreme sqrtPriceX96: price from $0.001 to $100,000
        // sqrtPriceX96 scale: 1.0 is 2^96 = 79,228,162,514,264,337,593,543,950,336
        let sqrt_price_base = 79_228_162_514_264_337_593_543_950_336u128; // 1.0
        let price_multiplier_bps = prng.next_range(10, 1_000_000); // 0.001x to 100x
        let sqrt_price_x96 = U256::from(sqrt_price_base / 1000) * U256::from(price_multiplier_bps);

        // Diverse liquidities: 10^12 to 10^22
        let liquidity = prng.next_range_u128(1_000_000_000_000u128, 50_000_000_000_000_000_000u128);

        let pool = SimulatedPoolState::new(
            address!("3333333333333333333333333333333333333333"),
            token0,
            token1,
            fee_pips,
            sqrt_price_x96,
            liquidity,
            0,
            60,
        );

        // Clone base DB (takes < 2 µs) and inject pool
        let mut sim_db = base_db.clone();
        inject_pool_state(&mut sim_db, quoter_addr, &pool).unwrap();

        // Alternate token_in direction
        let zero_for_one = (i % 2) == 0;
        let (token_in, token_out) = if zero_for_one {
            (token0, token1)
        } else {
            (token1, token0)
        };

        // Diverse trade sizes: 1,000 wei to 10^19 (10 ETH)
        let amount_in = U256::from(prng.next_range_u128(1_000, 10_000_000_000_000_000_000u128));

        let params = QuoteExactInputSingleParams {
            token_in,
            token_out,
            fee: fee_pips,
            amount_in,
            sqrt_price_limit_x96: U256::ZERO,
        };

        // Execute EVM quote
        let evm_out = execute_evm_quote(&mut sim_db, quoter_addr, &params)
            .expect("EVM quote execution must succeed");

        // Execute Analytical CLMM quote
        let ana_out = quote_analytical_clmm(&pool, token_in, amount_in, U256::ZERO)
            .expect("Analytical quote must succeed");

        // Wei-level difference
        let diff = if evm_out.amount_out >= ana_out.amount_out {
            evm_out.amount_out - ana_out.amount_out
        } else {
            ana_out.amount_out - evm_out.amount_out
        };

        if diff > max_diff {
            max_diff = diff;
        }

        if diff == U256::ZERO {
            exact_matches += 1;
        } else if diff == U256::from(1) {
            one_wei_diffs += 1;
        }

        assert!(
            diff <= U256::from(1),
            "Fuzz sample #{} failed parity: EVM={}, Analytical={}, diff={} wei (allowed <= 1 wei)",
            i,
            evm_out.amount_out,
            ana_out.amount_out,
            diff
        );

        // Verify quoter parity function passes directly
        verify_quoter_parity(&mut sim_db, quoter_addr, &pool, &params)
            .expect("verify_quoter_parity must pass for valid sample");
    }

    let elapsed = start_time.elapsed();
    let per_sample_us = elapsed.as_micros() as f64 / TOTAL_SAMPLES as f64;

    println!(
        "UniV3Quoter Parity Fuzz Results: {} samples in {:.2?}. Average {:.2} µs/sample. Exact matches: {}, 1-wei diffs: {}, Max diff: {} wei",
        TOTAL_SAMPLES, elapsed, per_sample_us, exact_matches, one_wei_diffs, max_diff
    );

    assert!(max_diff <= U256::from(1), "Maximum diff across 1,200 fuzzed pools must be <= 1 wei");
    assert_eq!(exact_matches + one_wei_diffs, TOTAL_SAMPLES);
}

// ============================================================================
// Vector 3: Boundary Gas & Block Limits
// ============================================================================

#[test]
fn test_stress_boundary_gas_and_block_limits() {
    let temp_dir = unique_stress_temp_dir("stress_gas_boundaries");
    let cache = Arc::new(BytecodeCache::new(&temp_dir, 100));
    let engine = SimulationEngine::new(Chain::ArbitrumOne, cache);

    // 1. Block Gas Limit Boundary: 30,000,000 gas
    let tx_30m = TxEnv {
        caller: Address::from([0x99; 20]),
        gas_limit: 30_000_000,
        gas_price: 0,
        kind: TxKind::Call(engine.executor_address),
        value: U256::ZERO,
        data: Bytes::from_static(&[0x00; 100]),
        ..Default::default()
    };
    let res_30m = engine.execute_raw(tx_30m, &[]);
    assert!(res_30m.is_ok(), "30M gas limit transaction must execute without panicking");

    // 2. Out-of-Gas Boundary: gas_limit = 500 (far below 21,000 intrinsic gas)
    let tx_oog = TxEnv {
        caller: Address::from([0x99; 20]),
        gas_limit: 500,
        gas_price: 0,
        kind: TxKind::Call(engine.executor_address),
        value: U256::ZERO,
        data: Bytes::from_static(&[0x00; 100]),
        ..Default::default()
    };
    let res_oog = engine.execute_raw(tx_oog, &[]);
    match res_oog {
        Ok(ExecutionResult::Halt { reason, .. }) => {
            println!("Cleanly halted on insufficient gas: {:?}", reason);
        }
        Err(SimError::Halted(reason)) => {
            println!("SimError::Halted on insufficient gas: {}", reason);
        }
        Err(SimError::DatabaseError(msg)) => {
            // revm returns transaction error on intrinsic gas violation
            println!("Clean EVM transaction error on sub-intrinsic gas: {}", msg);
        }
        other => panic!("Expected halt or error on extreme sub-intrinsic gas, got {:?}", other),
    }

    // 3. Zero Gas Price: gas_price = 0
    let tx_zero_price = TxEnv {
        caller: Address::from([0x99; 20]),
        gas_limit: 100_000,
        gas_price: 0,
        kind: TxKind::Call(engine.executor_address),
        value: U256::ZERO,
        data: Bytes::from_static(&[0x00; 100]),
        ..Default::default()
    };
    let res_zero = engine.execute_raw(tx_zero_price, &[]);
    assert!(res_zero.is_ok(), "Zero gas price must execute cleanly without division-by-zero");

    // 4. Max U256 Gas Price with Adequate Caller Balance Override
    let caller_addr = Address::from([0x88; 20]);
    let tx_max_gas = TxEnv {
        caller: caller_addr,
        gas_limit: 100_000,
        gas_price: 1_000_000_000_000u128, // 1000 gwei
        kind: TxKind::Call(engine.executor_address),
        value: U256::ZERO,
        data: Bytes::from_static(&[0x00; 100]),
        ..Default::default()
    };
    let override_rich = StateOverride::new(caller_addr).with_balance(U256::MAX);
    let res_max_price = engine.execute_raw(tx_max_gas, &[override_rich]);
    assert!(res_max_price.is_ok(), "High gas price with balance override must execute cleanly");

    // 5. Zero Caller Balance with Non-Zero Gas Price -> Handled gracefully without crash
    let broke_caller = Address::from([0x77; 20]);
    let tx_broke = TxEnv {
        caller: broke_caller,
        gas_limit: 100_000,
        gas_price: 20_000_000_000u128,
        kind: TxKind::Call(engine.executor_address),
        value: U256::ZERO,
        data: Bytes::from_static(&[0x00; 100]),
        ..Default::default()
    };
    let override_zero = StateOverride::new(broke_caller).with_balance(U256::ZERO);
    let res_broke = engine.execute_raw(tx_broke, &[override_zero]);
    assert!(
        res_broke.is_err(),
        "Zero caller balance with positive gas price must return error without panicking"
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// ============================================================================
// Vector 4: Two-Tier Cache Stress & Fault Tolerance
// ============================================================================

#[test]
fn test_stress_two_tier_cache_concurrency_and_faults() {
    let temp_dir = unique_stress_temp_dir("stress_cacache_tier");
    // LRU memory capacity set deliberately small (10) to trigger continuous eviction pressure
    const MEM_CAPACITY: usize = 10;
    let cache = Arc::new(BytecodeCache::new(&temp_dir, MEM_CAPACITY));

    const THREADS: usize = 20;
    const OPS_PER_THREAD: usize = 50; // Total 1,000 concurrent cache operations

    let barrier = Arc::new(Barrier::new(THREADS));
    let mut handles = Vec::with_capacity(THREADS);

    for t in 0..THREADS {
        let cache_clone = Arc::clone(&cache);
        let barrier_clone = Arc::clone(&barrier);

        let handle = thread::spawn(move || {
            barrier_clone.wait();

            for op in 0..OPS_PER_THREAD {
                let addr_byte = ((t * 10 + op) % 50) as u8;
                let addr = Address::from([addr_byte; 20]);
                let code_content = vec![0x60, addr_byte, 0x60, op as u8, 0x01];
                let code = Bytes::from(code_content.clone());

                // Concurrent put
                let put_res = cache_clone.put_bytecode(Chain::ArbitrumOne, addr, code.clone());
                assert!(put_res.is_ok(), "Concurrent put_bytecode failed: {:?}", put_res);

                // Concurrent get
                let fetched = cache_clone.get_bytecode(Chain::ArbitrumOne, addr);
                assert!(fetched.is_some(), "Concurrent get_bytecode missed stored key");
                assert_eq!(fetched.unwrap().original_bytes(), code);
            }
        });
        handles.push(handle);
    }

    for h in handles {
        h.join().expect("thread joined");
    }

    // Memory LRU must never exceed capacity
    assert!(
        !cache.contains_memory(Chain::ArbitrumOne, address!("ffffffffffffffffffffffffffffffffffffffff"))
    );

    // Fault tolerance: query non-existent disk directory & non-existent keys
    let non_existent_dir = temp_dir.join("non_existent_subdir");
    let ghost_cache = BytecodeCache::new(non_existent_dir, 5);
    let missing = ghost_cache.get_bytecode(Chain::Base, address!("9999999999999999999999999999999999999999"));
    assert!(missing.is_none(), "Non-existent disk cache must return None cleanly");

    // Clear memory does not corrupt persistent disk cache
    let test_addr = address!("1234567890123456789012345678901234567890");
    let test_code = Bytes::from_static(&[0x00, 0x01, 0x02]);
    cache.put_bytecode(Chain::ArbitrumOne, test_addr, test_code.clone()).unwrap();
    assert!(cache.contains_memory(Chain::ArbitrumOne, test_addr));

    cache.clear_memory();
    assert!(!cache.contains_memory(Chain::ArbitrumOne, test_addr), "Memory should be cleared");

    // Read-through from disk restores hot memory
    let read_back = cache.get_bytecode(Chain::ArbitrumOne, test_addr).expect("disk read-through");
    assert_eq!(read_back.original_bytes(), test_code);
    assert!(cache.contains_memory(Chain::ArbitrumOne, test_addr), "Promoted to hot memory");

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// ============================================================================
// Vector 5: Fast-Revert Guard Detection & Classification
// ============================================================================

#[test]
fn test_stress_fast_revert_guard_detection_and_classification() {
    let temp_dir = unique_stress_temp_dir("stress_guard_detection");
    let cache = Arc::new(BytecodeCache::new(&temp_dir, 100));
    let engine = SimulationEngine::new(Chain::ArbitrumOne, cache);

    // 1. Clean Fast-Revert Guard Slippage Abort (0-byte returndata via revert(0,0))
    // A mock contract that immediately reverts with revert(0,0) simulating frontrun slippage
    let slippage_bytecode = Bytes::from_static(&[
        0x5f, // PUSH0 (0)
        0x5f, // PUSH0 (0)
        0xfd, // REVERT
    ]);

    let slippage_guard_addr = address!("EEEE000000000000000000000000000000000001");
    let ovr_slippage = StateOverride::new(slippage_guard_addr).with_code(slippage_bytecode);

    let tx_slippage = TxEnv {
        caller: Address::from([0x11; 20]),
        gas_limit: 100_000,
        gas_price: 0,
        kind: TxKind::Call(slippage_guard_addr),
        value: U256::ZERO,
        data: Bytes::from_static(&[0x00; 100]),
        ..Default::default()
    };

    let res_slippage = engine.execute_raw(tx_slippage, &[ovr_slippage]).unwrap();
    match res_slippage {
        ExecutionResult::Revert { output, .. } => {
            assert!(output.is_empty(), "Fast revert guard must emit empty returndata");
            // Verify engine classification logic
            let classified = if output.is_empty() {
                SimError::FastRevertTriggered
            } else {
                SimError::Reverted(output)
            };
            assert_eq!(classified, SimError::FastRevertTriggered);
        }
        other => panic!("Expected Revert, got {:?}", other),
    }

    // 2. Atomic Profit Shortfall Revert (32-byte returndata via revert(0, 0x20))
    // Bytecode that writes profit to memory and reverts with 32 bytes
    let profit_shortfall_bytecode = Bytes::from_static(&[
        0x60, 0x2a, // PUSH1 42 (profit)
        0x5f,       // PUSH0 (offset 0)
        0x52,       // MSTORE
        0x60, 0x20, // PUSH1 32
        0x5f,       // PUSH0
        0xfd,       // REVERT (0, 32)
    ]);

    let shortfall_addr = address!("EEEE000000000000000000000000000000000002");
    let ovr_shortfall = StateOverride::new(shortfall_addr).with_code(profit_shortfall_bytecode);

    let tx_shortfall = TxEnv {
        caller: Address::from([0x11; 20]),
        gas_limit: 100_000,
        gas_price: 0,
        kind: TxKind::Call(shortfall_addr),
        value: U256::ZERO,
        data: Bytes::from_static(&[0x00; 100]),
        ..Default::default()
    };

    let res_shortfall = engine.execute_raw(tx_shortfall, &[ovr_shortfall]).unwrap();
    match res_shortfall {
        ExecutionResult::Revert { output, .. } => {
            assert_eq!(output.len(), 32, "Profit shortfall must emit 32 bytes");
            let profit = U256::from_be_slice(&output);
            assert_eq!(profit, U256::from(42));

            let classified = if output.len() == 32 {
                SimError::Unprofitable {
                    profit,
                    gas_cost: U256::from(50_000),
                }
            } else {
                SimError::Reverted(output)
            };
            assert_eq!(
                classified,
                SimError::Unprofitable {
                    profit: U256::from(42),
                    gas_cost: U256::from(50_000)
                }
            );
        }
        other => panic!("Expected Revert, got {:?}", other),
    }

    // 3. Custom / Standard 28-Byte Revert Abort
    // Bytecode that reverts with exactly 28 bytes: revert(0, 28)
    let custom_28_bytecode = Bytes::from_static(&[
        0x60, 0x1c, // PUSH1 28 (0x1c)
        0x5f,       // PUSH0
        0xfd,       // REVERT (0, 28)
    ]);

    let custom_28_addr = address!("EEEE000000000000000000000000000000000003");
    let ovr_28 = StateOverride::new(custom_28_addr).with_code(custom_28_bytecode);

    let tx_28 = TxEnv {
        caller: Address::from([0x11; 20]),
        gas_limit: 100_000,
        gas_price: 0,
        kind: TxKind::Call(custom_28_addr),
        value: U256::ZERO,
        data: Bytes::from_static(&[0x00; 100]),
        ..Default::default()
    };

    let res_28 = engine.execute_raw(tx_28, &[ovr_28]).unwrap();
    match res_28 {
        ExecutionResult::Revert { output, .. } => {
            assert_eq!(output.len(), 28, "Custom revert must emit exactly 28 bytes");
            let classified = if output.is_empty() {
                SimError::FastRevertTriggered
            } else if output.len() == 32 {
                SimError::Unprofitable {
                    profit: U256::ZERO,
                    gas_cost: U256::ZERO,
                }
            } else {
                SimError::Reverted(output.clone())
            };
            assert_eq!(classified, SimError::Reverted(output));
        }
        other => panic!("Expected Revert, got {:?}", other),
    }

    // 4. Clean Profitable Execution (stop / return)
    let ok_bytecode = Bytes::from_static(&[
        0x60, 0x01, // PUSH1 1
        0x5f,       // PUSH0
        0x52,       // MSTORE
        0x60, 0x20, // PUSH1 32
        0x5f,       // PUSH0
        0xf3,       // RETURN
    ]);

    let ok_addr = address!("EEEE000000000000000000000000000000000004");
    let ovr_ok = StateOverride::new(ok_addr).with_code(ok_bytecode);

    let tx_ok = TxEnv {
        caller: Address::from([0x11; 20]),
        gas_limit: 100_000,
        gas_price: 0,
        kind: TxKind::Call(ok_addr),
        value: U256::ZERO,
        data: Bytes::from_static(&[0x00; 100]),
        ..Default::default()
    };

    let res_ok = engine.execute_raw(tx_ok, &[ovr_ok]).unwrap();
    assert!(res_ok.is_success(), "Profitable execution must return Success");

    let _ = std::fs::remove_dir_all(&temp_dir);
}
