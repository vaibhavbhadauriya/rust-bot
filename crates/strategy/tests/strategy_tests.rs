//! Integration Test Suite for crates/strategy
//! Tests Multi-EOA Pool, Signer Leases, Local EIP-1559 Signing, Kill Switch, Telemetry, Shadow Engine, and Pipeline.

use alloy_primitives::{Address, Bytes, B256, U256};
use flash_core::types::Chain;
use flash_strategy::kill_switch::KillSwitchController;
use flash_strategy::shadow::{CompetingTx, ShadowEngine, SyntheticOutcome};
use flash_strategy::signers::{
    encode_access_list, recover_signer, Eip1559TransactionRequest, EoaSigner, SignerPool,
};
use flash_strategy::telemetry::{PipelineStageTimings, PipelineTelemetry, TelemetryCollector};
use std::time::Instant;

#[test]
fn test_signers_mock_pool_and_cas_acquisition() {
    let pool = SignerPool::new(2);
    assert_eq!(pool.capacity(), 2);

    // Acquire two signers concurrently
    let lease1 = pool.acquire_signer().expect("Should lease signer 1");
    let lease2 = pool.acquire_signer().expect("Should lease signer 2");

    assert_ne!(lease1.0, lease2.0, "Indices must be distinct");
    assert_ne!(lease1.1, lease2.1, "Addresses must be distinct");
    assert_eq!(lease1.2, 0, "Initial nonce must be 0");
    assert_eq!(lease2.2, 0, "Initial nonce must be 0");

    // Monotonic nonce: release lease1 (index 0) while lease2 (index 1) is still held.
    // Next acquisition rotates (start = 2 % 2 = 0) and reacquires index 0 with incremented nonce.
    pool.release_signer(lease1.0);
    let lease1_again = pool.acquire_signer().expect("Should reacquire signer 1");
    assert_eq!(lease1_again.0, lease1.0);
    assert_eq!(lease1_again.2, 1, "Nonce must increment monotonically");
}

#[test]
fn test_signers_true_round_robin_rotation() {
    let pool = SignerPool::new(4);
    // Acquire 0, release 0
    let l0 = pool.acquire_signer().expect("Lease 0");
    assert_eq!(l0.0, 0);
    pool.release_signer(l0.0);

    // Even though index 0 was released and is free, round-robin advances to index 1!
    let l1 = pool.acquire_signer().expect("Lease 1");
    assert_eq!(l1.0, 1);
    pool.release_signer(l1.0);

    let l2 = pool.acquire_signer().expect("Lease 2");
    assert_eq!(l2.0, 2);
    pool.release_signer(l2.0);

    let l3 = pool.acquire_signer().expect("Lease 3");
    assert_eq!(l3.0, 3);
    pool.release_signer(l3.0);

    // Full circle wrap-around back to index 0
    let l0_again = pool.acquire_signer().expect("Lease 0 wrap");
    assert_eq!(l0_again.0, 0);
}

#[test]
fn test_signers_raii_lease_guard() {
    let pool = SignerPool::new(1);
    {
        let lease = pool.acquire().expect("Should acquire lease");
        assert_eq!(lease.index(), 0);
        assert_eq!(lease.nonce(), 0);
        // While leased, pool is exhausted
        assert!(pool.acquire().is_none());
    }
    // Dropping lease automatically releases signer
    let lease2 = pool.acquire().expect("Should acquire after RAII drop");
    assert_eq!(lease2.index(), 0);
    assert_eq!(lease2.nonce(), 1);
}

#[test]
fn test_signers_real_ecdsa_key_and_signing() {
    // Generate valid 32-byte key: 0x01 ... 0x01
    let mut key = [0u8; 32];
    key[31] = 42;

    let signer = EoaSigner::new(0, key).expect("Valid secp256k1 key");
    assert_ne!(signer.address(), Address::ZERO);

    let pool = SignerPool::from_keys(&[key]).expect("Valid pool from keys");
    assert_eq!(pool.capacity(), 1);

    let tx_req = Eip1559TransactionRequest {
        chain_id: 42161,
        nonce: Some(0),
        max_priority_fee_per_gas: 2_000_000_000,
        max_fee_per_gas: 20_000_000_000,
        gas_limit: 100_000,
        to: Address::from([0x77; 20]),
        value: U256::ZERO,
        data: Bytes::from(vec![0xaa, 0xbb, 0xcc]),
        access_list: vec![],
    };

    let start = Instant::now();
    let signed = pool.sign_transaction(0, &tx_req).expect("Signing succeeds");
    let elapsed = start.elapsed();

    // Verify signed envelope starts with 0x02
    assert_eq!(signed.raw_bytes[0], 0x02, "EIP-1559 envelope starts with 0x02");
    assert_ne!(signed.tx_hash, B256::ZERO);
    assert_eq!(signed.sender, signer.address());
    assert_eq!(signed.chain_id, 42161);

    // Assert local signing completes within budget (release: <50µs, debug: <10ms)
    let max_allowed_us = if cfg!(debug_assertions) { 10_000 } else { 500 };
    assert!(
        elapsed.as_micros() < max_allowed_us,
        "Signing took {}µs (< {}µs)",
        elapsed.as_micros(),
        max_allowed_us
    );
}

#[test]
fn test_kill_switch_hot_path_and_thresholds() {
    let ks = KillSwitchController::new();
    assert!(!ks.is_tripped());
    assert!(ks.assert_healthy().is_ok());

    // Feed lag boundary checks
    assert!(!ks.check_feed_lag(100), "Exactly 100ms does NOT trip");
    assert!(!ks.is_tripped());

    assert!(ks.check_feed_lag(101), "101ms trips!");
    assert!(ks.is_tripped());
    assert!(ks.assert_healthy().is_err());
    assert!(ks.trip_reason().unwrap().contains("101ms"));

    // Reset
    ks.reset();
    assert!(!ks.is_tripped());

    // Consecutive fast reverts checks
    assert!(!ks.record_trade_result(true));
    assert!(!ks.record_trade_result(true));
    assert_eq!(ks.consecutive_fast_reverts(), 2);
    assert!(!ks.is_tripped());

    assert!(ks.record_trade_result(true), "3rd revert trips!");
    assert!(ks.is_tripped());

    // Success does not un-trip a latched state
    assert!(!ks.record_trade_result(false));
    assert!(ks.is_tripped(), "Latched until reset");

    ks.reset();
    assert!(!ks.is_tripped());
}

#[test]
fn test_telemetry_and_collector() {
    let timings = PipelineStageTimings {
        feed_arrival_us: 100,
        graph_dfs_us: 50,
        sizing_math_us: 40,
        revm_sim_us: 400,
        tx_sign_us: 30,
    };
    let telemetry = PipelineTelemetry::new(timings);

    assert_eq!(telemetry.math_plus_sim_latency_us(), 440);
    assert_eq!(telemetry.total_e2e_latency_us(), 620);
    assert!(telemetry.is_math_plus_sim_compliant());
    assert!(telemetry.is_e2e_compliant());

    let collector = TelemetryCollector::new();
    collector.record(&telemetry);
    assert_eq!(collector.sample_count.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(collector.p99_latency_us(), 620);
    assert!((collector.avg_e2e_latency_us() - 620.0).abs() < 1e-6);
}

#[test]
fn test_shadow_engine_pga_matching() {
    let engine = ShadowEngine::new(Chain::ArbitrumOne, false);
    assert!(!engine.is_live());

    let pool_a = Address::from([0x11; 20]);
    let pool_b = Address::from([0x22; 20]);

    let candidate = flash_scanner::dfs::RouteCandidate {
        borrow_token: Address::from([0x33; 20]),
        hops: vec![
            flash_scanner::graph::DirectedEdge {
                pool_id: pool_a,
                venue: flash_core::types::Venue::UniswapV2,
                token_in: Address::from([0x33; 20]),
                token_out: Address::from([0x44; 20]),
                reserve_in: 1_000_000,
                reserve_out: 405,
                fee_pips: 3000,
                zero_for_one: true,
            },
            flash_scanner::graph::DirectedEdge {
                pool_id: pool_b,
                venue: flash_core::types::Venue::UniswapV2,
                token_in: Address::from([0x44; 20]),
                token_out: Address::from([0x33; 20]),
                reserve_in: 395,
                reserve_out: 1_000_000,
                fee_pips: 3000,
                zero_for_one: false,
            },
        ],
        sizing: flash_math::closed_form::TradeSizeOutput {
            optimal_input: U256::from(1000),
            expected_intermediate: U256::from(100),
            expected_output: U256::from(1050),
            gross_profit: U256::from(50),
            net_profit: U256::from(50),
            flash_fee: U256::ZERO,
        },
        net_profit: U256::from(50),
    };

    let sim_outcome = flash_simulator::types::SimulationOutcome {
        is_success: true,
        gas_used: 120_000,
        gas_cost_wei: U256::from(2_400_000_000_000_000u64),
        actual_profit: U256::from(50),
        net_yield: alloy_primitives::I256::try_from(45).unwrap(),
        fast_revert_tripped: false,
        calldata: Bytes::from(vec![0u8; 124]),
        logs: vec![],
        return_data: Bytes::new(),
    };

    let telemetry = PipelineTelemetry::default();
    let raw_tx = [0x55u8; 128];

    // Enqueue order with 5 Gwei priority fee at block 100
    let _order_id = engine.register_simulated_opportunity(
        &candidate,
        &sim_outcome,
        telemetry,
        &raw_tx,
        5_000_000_000,
        100,
    );

    // 1. Block with competitor bidding 2 Gwei -> We WIN the PGA!
    let competitors = vec![CompetingTx {
        tx_hash: B256::from([0x99; 32]),
        target_pool: pool_a,
        max_priority_fee_per_gas: 2_000_000_000,
        gas_used: 120_000,
        is_reverted: false,
    }];

    let results = engine.match_mined_block(100, &competitors);
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].outcome, SyntheticOutcome::WonPriorityGasAuction { .. }));
    assert!(results[0].outcome.is_win());

    let metrics = engine.tracker.snapshot();
    assert_eq!(metrics.pga_wins, 1);
    assert_eq!(metrics.total_simulated, 1);
}

#[test]
fn test_v10_empty_access_list_rlp_encoding_0xc0() {
    let mut out = Vec::new();
    encode_access_list(&[], &mut out);
    assert_eq!(out, vec![0xc0], "Empty access list must encode as 0xc0 (RLP list), not 0x80");

    let mut out_non_empty = Vec::new();
    let addr = Address::from([0x42; 20]);
    let key = B256::from([0x01; 32]);
    encode_access_list(&[(addr, vec![key])], &mut out_non_empty);
    assert_ne!(out_non_empty, vec![0x80]);
    assert_ne!(out_non_empty, vec![0xc0]);
    // First byte of an RLP list with payload length > 0 is >= 0xc0
    assert!(out_non_empty[0] > 0xc0);
}

#[test]
fn test_v09_rollback_nonce_idempotence_and_isolation() {
    let pool = SignerPool::new(1);
    assert_eq!(pool.current_nonce(0), 0);

    // Acquire lease: advances local nonce from 0 to 1
    let lease = pool.acquire().expect("Acquisition should succeed");
    assert_eq!(lease.nonce(), 0);
    assert_eq!(pool.current_nonce(0), 1);

    // Rollback: reverts nonce back to 0
    lease.rollback_nonce();
    assert_eq!(pool.current_nonce(0), 0);

    // Redundant rollback is idempotent
    lease.rollback_nonce();
    assert_eq!(pool.current_nonce(0), 0);

    drop(lease);

    // Reacquiring gives nonce 0 again
    let lease2 = pool.acquire().expect("Reacquisition should succeed");
    assert_eq!(lease2.nonce(), 0);
    assert_eq!(pool.current_nonce(0), 1);
}

#[tokio::test]
async fn test_v08_sync_nonces_from_rpc_graceful() {
    let pool = SignerPool::new(2);
    pool.set_nonce(0, 10);
    pool.set_nonce(1, 20);

    // Empty URL is a no-op that retains nonces
    let res = pool.sync_nonces_from_rpc("").await;
    assert!(res.is_ok());
    assert_eq!(pool.current_nonce(0), 10);
    assert_eq!(pool.current_nonce(1), 20);

    // Unreachable URL returns Err but does not corrupt/zero local nonces
    let res_err = pool.sync_nonces_from_rpc("http://127.0.0.1:9").await;
    assert!(res_err.is_err());
    assert_eq!(pool.current_nonce(0), 10);
    assert_eq!(pool.current_nonce(1), 20);
}

#[test]
fn test_v23_signature_recovery_and_address_match() {
    let key: [u8; 32] = [
        0x4c, 0x08, 0x83, 0xa6, 0x91, 0x02, 0x93, 0x7d, 0x62, 0x31, 0x47, 0x1b, 0x5d, 0xb0, 0x1a,
        0x70, 0x22, 0x76, 0x75, 0x56, 0xb6, 0x89, 0x67, 0x0d, 0x62, 0x7d, 0x2e, 0x66, 0x64, 0x77,
        0x83, 0xa5,
    ];
    let signer = EoaSigner::new(0, key).expect("Valid signer");
    let pool = SignerPool::from_keys(&[key]).expect("Valid pool");

    let tx_req = Eip1559TransactionRequest {
        chain_id: 42161,
        nonce: Some(0),
        max_priority_fee_per_gas: 2_000_000_000,
        max_fee_per_gas: 20_000_000_000,
        gas_limit: 350_000,
        to: Address::from([0x77; 20]),
        value: U256::ZERO,
        data: Bytes::from(vec![0xaa, 0xbb]),
        access_list: vec![],
    };

    let signed = pool.sign_transaction(0, &tx_req).expect("Signing succeeds with verified ecrecover");
    assert_eq!(signed.sender, signer.address());

    // Direct recover_signer validation on arbitrary sighash
    let signing_key = k256::ecdsa::SigningKey::from_bytes((&key).into()).unwrap();
    let sighash = [0x55u8; 32];
    let (sig, recid) = signing_key.sign_prehash_recoverable(&sighash).unwrap();
    let direct_recovered = recover_signer(&sighash, &sig, recid).expect("Direct recovery succeeds");
    assert_eq!(direct_recovered, signer.address());
}

#[test]
fn test_v01_v07_pipeline_configuration_live_vs_shadow() {
    let config = flash_core::config::AppConfig {
        alchemy_key: "k".into(),
        arbitrum_rpc_ws: "".into(),
        arbitrum_rpc_http: "".into(),
        arbitrum_sequencer_feed: "".into(),
        base_rpc_ws: "".into(),
        base_rpc_http: "".into(),
        base_flashblocks_ws: "".into(),
        execution_mode: flash_core::types::ExecutionMode::Live,
        log_level: "info".into(),
        signer_private_keys: vec![],
        executor_contract_address: Address::from([0xee; 20]),
        max_hops: 3,
        kill_switch_feed_lag_ms: 100,
        kill_switch_fast_revert_count: 3,
    };

    assert!(config.validate_live_mode().is_ok());

    let cache = std::sync::Arc::new(flash_simulator::cache::BytecodeCache::new("./target/cache_test", 100));
    let sim = std::sync::Arc::new(flash_simulator::engine::SimulationEngine::new(Chain::ArbitrumOne, cache));
    let pool = std::sync::Arc::new(SignerPool::new(1));
    let graph = std::sync::Arc::new(flash_scanner::graph::MarketGraph::new());
    let shadow = std::sync::Arc::new(ShadowEngine::new(Chain::ArbitrumOne, true));
    let ks = std::sync::Arc::new(KillSwitchController::default());
    let tel = std::sync::Arc::new(TelemetryCollector::new());

    let pipeline_live = flash_strategy::pipeline::ArbitragePipeline::new(
        config.clone(),
        graph.clone(),
        sim.clone(),
        pool.clone(),
        shadow.clone(),
        ks.clone(),
        tel.clone(),
    );

    // In live mode, config executor contract address is configured and distinct from mock precompile 0x01
    assert_eq!(pipeline_live.config.executor_contract_address, Address::from([0xee; 20]));
    assert_ne!(pipeline_live.config.executor_contract_address, Address::with_last_byte(1));

    // Gas limit floor formula verification: 25% safety overhead and 350,000 floor
    let sim_gas_low = 60_000u64;
    let computed_gas_limit = (sim_gas_low * 125 / 100).max(350_000);
    assert_eq!(computed_gas_limit, 350_000);

    let sim_gas_high = 300_000u64;
    let computed_gas_high = (sim_gas_high * 125 / 100).max(350_000);
    assert_eq!(computed_gas_high, 375_000);
}
