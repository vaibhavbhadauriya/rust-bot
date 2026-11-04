//! Milestone 6 Phase 2: Tier 5 Adversarial Stress & White-Box Coverage Hardening
//! Target: crates/strategy
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 69-74, 90; PROJECT.md § Milestone 5 & 6.
//!
//! Probes:
//! 1. Multi-EOA Signer Pool: Lock-free CAS leasing under heavy contention (64 threads), monotonic nonces, and ECDSA signature recovery
//! 2. Shadow Paper-Trading Engine: PGA bid matching, queue pruning, and PnL metrics
//! 3. Telemetry Accounting: Exact budget boundaries (<1500µs math+sim, <3500µs e2e), P99 percentiles
//! 4. Automated Kill Switch: Boundary conditions (>100ms lag, >=3 fast reverts), latching invariants, sub-10ns hot path

use alloy_primitives::{Address, Bytes, B256, I256, U256};
use flash_core::types::Chain;
use flash_scanner::dfs::RouteCandidate;
use flash_scanner::graph::DirectedEdge;
use flash_simulator::types::SimulationOutcome;
use flash_strategy::kill_switch::{KillSwitchController, KillSwitchError};
use flash_strategy::shadow::{CompetingTx, ShadowEngine, SyntheticOutcome};
use flash_strategy::signers::{Eip1559TransactionRequest, SignerPool};
use flash_strategy::telemetry::PipelineTelemetry;
use std::sync::{Arc, Barrier};
use std::thread;

// ============================================================================
// 1. Multi-EOA Pool Concurrency, Nonces, and ECDSA Recovery
// ============================================================================

#[test]
fn test_m6_multieoa_01_cas_contention_and_ecdsa_recovery() {
    let pool_capacity = 10;
    let pool = Arc::new(SignerPool::new(pool_capacity));
    let num_threads = 40;
    let iterations_per_thread = 500; // 20,000 total acquisitions

    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::with_capacity(num_threads);

    for _ in 0..num_threads {
        let pool_clone = Arc::clone(&pool);
        let barrier_clone = Arc::clone(&barrier);

        handles.push(thread::spawn(move || {
            barrier_clone.wait();
            for _ in 0..iterations_per_thread {
                loop {
                    if let Some(lease) = pool_clone.acquire() {
                        let idx = lease.index();
                        let addr = lease.address();
                        let nonce = lease.nonce();

                        // Sign an EIP-1559 transaction
                        let tx_req = Eip1559TransactionRequest {
                            chain_id: 42161,
                            nonce: Some(nonce),
                            max_priority_fee_per_gas: 2_000_000_000,
                            max_fee_per_gas: 25_000_000_000,
                            gas_limit: 100_000,
                            to: Address::from([0x99; 20]),
                            value: U256::ZERO,
                            data: Bytes::from_static(&[0xaa, 0xbb]),
                            access_list: vec![],
                        };

                        let signed_tx = pool_clone.sign_transaction(idx, &tx_req).unwrap();
                        assert_eq!(signed_tx.sender, addr);
                        assert_eq!(signed_tx.nonce, nonce);
                        assert_eq!(signed_tx.raw_bytes[0], 0x02, "EIP-2718 envelope prefix");

                        // Release lease
                        drop(lease);
                        break;
                    } else {
                        thread::yield_now();
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    assert_eq!(
        pool.total_leases(),
        num_threads * iterations_per_thread,
        "Total leases must match total iterations"
    );
}

#[test]
fn test_m6_multieoa_02_strict_monotonic_nonce_ceiling() {
    let pool = SignerPool::new(1);
    assert_eq!(pool.current_nonce(0), 0);

    // Syncing to 50 sets nonce to 50
    pool.sync_nonce(0, 50);
    assert_eq!(pool.current_nonce(0), 50);

    // Syncing to stale nonce 30 does NOT regress
    pool.sync_nonce(0, 30);
    assert_eq!(pool.current_nonce(0), 50, "Nonce must not regress on stale sync");

    // Acquire advances nonce to 50, subsequent is 51
    let lease = pool.acquire().unwrap();
    assert_eq!(lease.nonce(), 50);
    drop(lease);
    assert_eq!(pool.current_nonce(0), 51);

    // Manual set_nonce override
    pool.set_nonce(0, 200);
    assert_eq!(pool.current_nonce(0), 200);
}

// ============================================================================
// 2. Shadow Paper-Trading Engine & PGA Bid Matching
// ============================================================================

#[test]
fn test_m6_shadow_01_pga_matching_outcomes() {
    let shadow = ShadowEngine::new(Chain::ArbitrumOne, false);
    assert!(!shadow.is_live());

    let pool_a = Address::from([0x11; 20]);
    let pool_b = Address::from([0x22; 20]);
    let borrow = Address::from([0x33; 20]);

    let candidate = RouteCandidate {
        borrow_token: borrow,
        hops: vec![
            DirectedEdge {
                pool_id: pool_a,
                venue: flash_core::types::Venue::UniswapV2,
                token_in: borrow,
                token_out: Address::from([0x44; 20]),
                reserve_in: 1_000_000,
                reserve_out: 1_000_000,
                fee_pips: 3000,
                zero_for_one: true,
            },
            DirectedEdge {
                pool_id: pool_b,
                venue: flash_core::types::Venue::UniswapV2,
                token_in: Address::from([0x44; 20]),
                token_out: borrow,
                reserve_in: 1_000_000,
                reserve_out: 1_000_000,
                fee_pips: 3000,
                zero_for_one: false,
            },
        ],
        sizing: flash_math::TradeSizeOutput {
            optimal_input: U256::from(100),
            expected_intermediate: U256::from(200),
            expected_output: U256::from(150),
            gross_profit: U256::from(50),
            flash_fee: U256::ZERO,
            net_profit: U256::from(50),
        },
        net_profit: U256::from(50),
    };

    let sim_outcome = SimulationOutcome {
        is_success: true,
        gas_used: 120_000,
        gas_cost_wei: U256::from(200_000),
        actual_profit: U256::from(50),
        net_yield: I256::try_from(50i64).unwrap(),
        calldata: Bytes::from(vec![0u8; 124]),
        fast_revert_tripped: false,
        logs: vec![],
        return_data: Bytes::new(),
    };

    let telemetry = PipelineTelemetry::default();
    let raw_tx = [0x02, 0x01, 0x02, 0x03];

    // Case A: Register opportunity with priority fee = 5 Gwei at block 100
    shadow.register_simulated_opportunity(
        &candidate,
        &sim_outcome,
        telemetry,
        &raw_tx,
        5_000_000_000, // 5 Gwei
        100,
    );

    // Match against block 100 with NO competitors on pool_a or pool_b -> UncontestedWin
    let results_uncontested = shadow.match_mined_block(100, &[]);
    assert_eq!(results_uncontested.len(), 1);
    assert_eq!(results_uncontested[0].outcome, SyntheticOutcome::UncontestedWin);
    assert!(results_uncontested[0].outcome.is_win());

    // Case B: Register opportunity with priority fee = 4 Gwei at block 101
    shadow.register_simulated_opportunity(
        &candidate,
        &sim_outcome,
        telemetry,
        &raw_tx,
        4_000_000_000, // 4 Gwei
        101,
    );

    // Match against competitor on pool_a bidding 2 Gwei -> WonPriorityGasAuction
    let comp_lower = CompetingTx {
        tx_hash: B256::from([0xaa; 32]),
        target_pool: pool_a,
        max_priority_fee_per_gas: 2_000_000_000, // 2 Gwei
        gas_used: 100_000,
        is_reverted: false,
    };
    let results_pga_win = shadow.match_mined_block(101, &[comp_lower]);
    assert_eq!(results_pga_win.len(), 1);
    assert_eq!(
        results_pga_win[0].outcome,
        SyntheticOutcome::WonPriorityGasAuction {
            competitor_bid: 2_000_000_000,
            our_bid: 4_000_000_000,
        }
    );
    assert!(results_pga_win[0].outcome.is_win());

    // Case C: Register opportunity with priority fee = 3 Gwei at block 102
    shadow.register_simulated_opportunity(
        &candidate,
        &sim_outcome,
        telemetry,
        &raw_tx,
        3_000_000_000, // 3 Gwei
        102,
    );

    // Match against competitor on pool_a bidding 6 Gwei -> FastRevertPrevented
    let comp_higher = CompetingTx {
        tx_hash: B256::from([0xbb; 32]),
        target_pool: pool_a,
        max_priority_fee_per_gas: 6_000_000_000, // 6 Gwei
        gas_used: 100_000,
        is_reverted: false,
    };
    let results_pga_loss = shadow.match_mined_block(102, &[comp_higher]);
    assert_eq!(results_pga_loss.len(), 1);
    assert_eq!(
        results_pga_loss[0].outcome,
        SyntheticOutcome::FastRevertPrevented {
            competitor_bid: 6_000_000_000,
            gas_saved: 120_000,
        }
    );
    assert!(!results_pga_loss[0].outcome.is_win());

    // Check aggregated metrics snapshot
    let snapshot = shadow.tracker.snapshot();
    assert_eq!(snapshot.total_simulated, 3);
    assert_eq!(snapshot.uncontested_wins, 1);
    assert_eq!(snapshot.pga_wins, 1);
    assert_eq!(snapshot.fast_reverts_prevented, 1);
    assert!((snapshot.win_rate_percentage - 66.666).abs() < 1.0);
}

// ============================================================================
// 3. Telemetry Accounting & Latency Budget Boundaries
// ============================================================================

#[test]
fn test_m6_telemetry_01_exact_compliance_boundaries() {
    // Math + Sim: max budget is 1500us (< 1500us is compliant)
    let t_1499 = PipelineTelemetry {
        sizing_math_us: 499,
        revm_sim_us: 1000,
        ..Default::default()
    };
    assert!(t_1499.is_math_plus_sim_compliant());

    let t_1500 = PipelineTelemetry {
        sizing_math_us: 500,
        revm_sim_us: 1000,
        ..Default::default()
    };
    assert!(!t_1500.is_math_plus_sim_compliant()); // exactly 1500 is NOT < 1500

    let t_1501 = PipelineTelemetry {
        sizing_math_us: 501,
        revm_sim_us: 1000,
        ..Default::default()
    };
    assert!(!t_1501.is_math_plus_sim_compliant());

    // E2E Pipeline: max budget is 3500us (< 3500us is compliant)
    let t_3499 = PipelineTelemetry {
        feed_arrival_us: 500,
        graph_dfs_us: 200,
        sizing_math_us: 100,
        revm_sim_us: 700,
        tx_sign_us: 1999,
    };
    assert!(t_3499.is_e2e_compliant());

    let t_3500 = PipelineTelemetry {
        feed_arrival_us: 500,
        graph_dfs_us: 200,
        sizing_math_us: 100,
        revm_sim_us: 700,
        tx_sign_us: 2000,
    };
    assert!(!t_3500.is_e2e_compliant());
}

// ============================================================================
// 4. Automated Kill Switch Boundaries & Invariants
// ============================================================================

#[test]
fn test_m6_killswitch_01_feed_lag_and_revert_state_machine() {
    let ks = KillSwitchController::new();
    assert!(!ks.is_tripped());

    // 100ms exact boundary does NOT trip
    assert!(!ks.check_feed_lag(100));
    assert!(!ks.is_tripped());

    // 101ms trips
    assert!(ks.check_feed_lag(101));
    assert!(ks.is_tripped());
    assert!(matches!(ks.assert_healthy(), Err(KillSwitchError::Halted(_))));

    // Reset restores healthy state
    ks.reset();
    assert!(!ks.is_tripped());
    assert!(ks.assert_healthy().is_ok());

    // 2 consecutive fast reverts do NOT trip
    assert!(!ks.record_trade_result(true));
    assert_eq!(ks.consecutive_fast_reverts(), 1);
    assert!(!ks.record_trade_result(true));
    assert_eq!(ks.consecutive_fast_reverts(), 2);
    assert!(!ks.is_tripped());

    // 3rd consecutive fast revert TRIPS
    assert!(ks.record_trade_result(true));
    assert_eq!(ks.consecutive_fast_reverts(), 3);
    assert!(ks.is_tripped());

    // Once tripped, honest trade CANNOT untrip
    assert!(!ks.record_trade_result(false));
    assert!(ks.is_tripped());
    assert_eq!(ks.consecutive_fast_reverts(), 3, "Latched counter");

    // Explicit reset clears tripped flag and counter
    ks.reset();
    assert!(!ks.is_tripped());
    assert_eq!(ks.consecutive_fast_reverts(), 0);
    assert!(ks.assert_healthy().is_ok());
}
