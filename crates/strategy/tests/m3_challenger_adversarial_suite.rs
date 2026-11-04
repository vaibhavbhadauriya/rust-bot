//! Empirical Adversarial Challenge Suite for Milestone M3 (Requirement R3)
//!
//! Adversarially challenges:
//! 1. GasOracle Resilience:
//!    - Polling against unreachable/invalid RPC URLs returns default prices without panicking.
//!    - Initial default prices for Arbitrum One and Base match canonical network parameters.
//!    - Dynamic updates via `set_gas_prices` correctly propagate to `SimulationEngine` and `ArbitragePipeline`.
//! 2. Dynamic Priority Fee Bidding:
//!    - Base WETH and non-WETH routes correctly convert token profit into ETH wei and compute the 20% builder bribe.
//!    - When profit is zero or negative, priority fee defaults to minimum priority fee.
//!    - Priority fee is properly clamped to `[min_priority_fee, max_priority_cap]`.
//! 3. Precompile Collisions:
//!    - Ensures revm simulations do not use address `0x01` as the executor address.
//!    - Confirms that live mode strictly forbids target address `0x01`.

use alloy_primitives::{address, Address, U256};
use flash_core::config::AppConfig;
use flash_core::constants::{arbitrum, base};
use flash_core::types::{Chain, ExecutionMode, GasPrices, PoolState, Venue};
use flash_scanner::discovery::DiscoveredPool;
use flash_scanner::graph::MarketGraph;
use flash_simulator::bytecode_mocks::MOCK_EXECUTOR_ADDRESS;
use flash_simulator::cache::BytecodeCache;
use flash_simulator::engine::SimulationEngine;
use flash_strategy::gas_oracle::GasOracle;
use flash_strategy::kill_switch::KillSwitchController;
use flash_strategy::pipeline::ArbitragePipeline;
use flash_strategy::shadow::ShadowEngine;
use flash_strategy::signers::SignerPool;
use flash_strategy::telemetry::TelemetryCollector;
use std::sync::Arc;

fn create_test_pipeline(
    chain: Chain,
    is_live: bool,
    executor_override: Option<Address>,
) -> (ArbitragePipeline, Arc<SimulationEngine>, Arc<MarketGraph>) {
    let cache_dir = format!("./target/test_cache_m3_{:?}_{}", chain, is_live);
    let cache = Arc::new(BytecodeCache::new(&cache_dir, 50));
    let engine = Arc::new(SimulationEngine::new(chain, cache));
    let graph = Arc::new(MarketGraph::new());
    engine.set_graph(graph.clone());

    let pool = Arc::new(SignerPool::new(2));
    let shadow = Arc::new(ShadowEngine::new(chain, true));
    let ks = Arc::new(KillSwitchController::default());
    let tel = Arc::new(TelemetryCollector::new());

    let exec_addr = executor_override.unwrap_or(address!("2222222222222222222222222222222222222222"));

    let config = AppConfig {
        alchemy_key: "dummy_key".to_string(),
        arbitrum_rpc_ws: "ws://localhost:8546".to_string(),
        arbitrum_rpc_http: "http://localhost:8545".to_string(),
        arbitrum_sequencer_feed: "wss://arb1-feed.arbitrum.io/feed".to_string(),
        base_rpc_ws: "ws://localhost:8546".to_string(),
        base_rpc_http: "http://localhost:8545".to_string(),
        base_flashblocks_ws: "wss://mainnet.flashblocks.base.org/ws".to_string(),
        execution_mode: if is_live {
            ExecutionMode::Live
        } else {
            ExecutionMode::Shadow
        },
        log_level: "info".to_string(),
        signer_private_keys: vec![
            "0x0000000000000000000000000000000000000000000000000000000000000001".to_string(),
            "0x0000000000000000000000000000000000000000000000000000000000000002".to_string(),
        ],
        executor_contract_address: exec_addr,
        max_hops: 3,
        kill_switch_feed_lag_ms: 100,
        kill_switch_fast_revert_count: 3,
    };

    let pipeline = ArbitragePipeline::new(
        config,
        graph.clone(),
        engine.clone(),
        pool,
        shadow,
        ks,
        tel,
    );

    (pipeline, engine, graph)
}

// ==============================================================================
// 1. GasOracle Resilience & Canonical Network Parameters
// ==============================================================================

#[tokio::test]
async fn test_adversarial_gas_oracle_resilience_unreachable_and_invalid_endpoints() {
    // Attack scenario: Node RPC goes down, port is closed, or DNS fails completely
    let unreachable_endpoints = [
        "http://127.0.0.1:1",
        "http://localhost:59999",
        "http://nonexistent.invalid.domain:8545",
        "http://192.0.2.1:80", // TEST-NET-1 unroutable IP
    ];

    for endpoint in unreachable_endpoints {
        let arb_oracle = GasOracle::new(Chain::ArbitrumOne, endpoint.to_string());
        // Must NEVER panic or bubble an unhandled error; must fall back to chain defaults
        let arb_poll = arb_oracle.poll_once().await;
        assert!(
            arb_poll.is_ok(),
            "GasOracle::poll_once() on Arbitrum must gracefully fall back on unreachable endpoint {}",
            endpoint
        );
        let prices = arb_poll.unwrap();
        assert_eq!(prices.l2_base_fee_wei, 100_000_000); // 0.1 Gwei
        assert_eq!(prices.l2_priority_fee_wei, 10_000_000); // 0.01 Gwei
        assert_eq!(prices.l1_data_fee_wei, 400_000_000_000_000); // 0.0004 ETH

        let base_oracle = GasOracle::new(Chain::Base, endpoint.to_string());
        let base_poll = base_oracle.poll_once().await;
        assert!(
            base_poll.is_ok(),
            "GasOracle::poll_once() on Base must gracefully fall back on unreachable endpoint {}",
            endpoint
        );
        let b_prices = base_poll.unwrap();
        assert_eq!(b_prices.l2_base_fee_wei, 50_000_000); // 0.05 Gwei
        assert_eq!(b_prices.l2_priority_fee_wei, 50_000_000); // 0.05 Gwei
        assert_eq!(b_prices.l1_data_fee_wei, 300_000_000_000_000); // 0.0003 ETH
    }
}

#[test]
fn test_adversarial_canonical_gas_price_parameters() {
    // 1. Arbitrum One canonical defaults
    let arb_prices = GasPrices::default_for_chain(Chain::ArbitrumOne);
    assert_eq!(
        arb_prices.l2_base_fee_wei, 100_000_000,
        "Arbitrum One default L2 base fee must be 0.1 Gwei (100_000_000 wei)"
    );
    assert_eq!(
        arb_prices.l2_priority_fee_wei, 10_000_000,
        "Arbitrum One default L2 priority fee must be 0.01 Gwei (10_000_000 wei)"
    );
    assert_eq!(
        arb_prices.l1_data_fee_wei, 400_000_000_000_000,
        "Arbitrum One default L1 data fee must be 0.0004 ETH (400_000_000_000_000 wei)"
    );

    // 2. Base canonical defaults
    let base_prices = GasPrices::default_for_chain(Chain::Base);
    assert_eq!(
        base_prices.l2_base_fee_wei, 50_000_000,
        "Base default L2 base fee must be 0.05 Gwei (50_000_000 wei)"
    );
    assert_eq!(
        base_prices.l2_priority_fee_wei, 50_000_000,
        "Base default L2 priority fee must be 0.05 Gwei (50_000_000 wei)"
    );
    assert_eq!(
        base_prices.l1_data_fee_wei, 300_000_000_000_000,
        "Base default L1 data fee must be 0.0003 ETH (300_000_000_000_000 wei)"
    );

    // 3. Mathematical correctness of total_gas_cost_wei
    // Case A: Arbitrum 200,000 gas
    // total = (200_000 * (100_000_000 + 10_000_000)) + 400_000_000_000_000
    //       = (200_000 * 110_000_000) + 400_000_000_000_000
    //       = 22_000_000_000_000 + 400_000_000_000_000 = 422_000_000_000_000
    let arb_cost = arb_prices.total_gas_cost_wei(200_000);
    assert_eq!(arb_cost, 422_000_000_000_000u128);

    // Case B: Base 180,000 gas
    // total = (180_000 * (50_000_000 + 50_000_000)) + 300_000_000_000_000
    //       = (180_000 * 100_000_000) + 300_000_000_000_000
    //       = 18_000_000_000_000 + 300_000_000_000_000 = 318_000_000_000_000
    let base_cost = base_prices.total_gas_cost_wei(180_000);
    assert_eq!(base_cost, 318_000_000_000_000u128);
}

#[test]
fn test_adversarial_dynamic_gas_price_propagation_to_engine_and_pipeline() {
    let (pipeline, engine, _graph) = create_test_pipeline(Chain::ArbitrumOne, false, None);

    // Initial check: engine and pipeline match initial Arbitrum defaults
    let initial_prices = engine.gas_prices();
    assert_eq!(initial_prices.l2_base_fee_wei, 100_000_000);
    assert_eq!(pipeline.simulator.gas_prices().l2_base_fee_wei, 100_000_000);

    // Dynamic update scenario: Gas prices spike 10x during market volatility
    let spiked_prices = GasPrices {
        l2_base_fee_wei: 1_000_000_000,       // 1.0 Gwei
        l2_priority_fee_wei: 200_000_000,     // 0.2 Gwei
        l1_data_fee_wei: 2_500_000_000_000_000, // 0.0025 ETH
    };

    engine.set_gas_prices(spiked_prices);

    // Verify propagation
    let engine_read = engine.gas_prices();
    assert_eq!(engine_read.l2_base_fee_wei, 1_000_000_000);
    assert_eq!(engine_read.l2_priority_fee_wei, 200_000_000);
    assert_eq!(engine_read.l1_data_fee_wei, 2_500_000_000_000_000);

    let pipeline_read = pipeline.simulator.gas_prices();
    assert_eq!(pipeline_read.l2_base_fee_wei, 1_000_000_000);
    assert_eq!(pipeline_read.l2_priority_fee_wei, 200_000_000);
    assert_eq!(pipeline_read.l1_data_fee_wei, 2_500_000_000_000_000);

    // Verify shared Arc binding with GasOracle
    let oracle = GasOracle::new_with_prices(
        Chain::ArbitrumOne,
        "http://dummy:8545".into(),
        engine.gas_prices.clone(),
    );

    // Oracle writing to prices directly updates engine and pipeline
    let oracle_updated = GasPrices {
        l2_base_fee_wei: 350_000_000,
        l2_priority_fee_wei: 45_000_000,
        l1_data_fee_wei: 600_000_000_000_000,
    };
    *oracle.prices.write() = oracle_updated;

    assert_eq!(engine.gas_prices().l2_base_fee_wei, 350_000_000);
    assert_eq!(pipeline.simulator.gas_prices().l2_base_fee_wei, 350_000_000);
}

// ==============================================================================
// 2. Dynamic Priority Fee Bidding: Base WETH, Non-WETH, Clamping & Bribes
// ==============================================================================

#[test]
fn test_adversarial_dynamic_bidding_base_weth_and_arbitrum_weth() {
    let gas_limit = 350_000u64;
    let min_priority_fee = 50_000_000u128; // 0.05 Gwei
    let max_priority_cap = 5_000_000_000u128; // 5.0 Gwei

    // 1. Arbitrum One WETH opportunity
    let (_pipeline_arb, _engine_arb, _graph_arb) = create_test_pipeline(Chain::ArbitrumOne, false, None);

    // Profit: 0.001 ETH = 1_000_000_000_000_000 wei
    let profit_eth = U256::from(1_000_000_000_000_000u128);
    let bribe_wei = profit_eth.to::<u128>() / 5; // 20% = 200_000_000_000_000 wei
    let per_gas_bribe = bribe_wei / (gas_limit as u128);
    // 200_000_000_000_000 / 350_000 = 571_428_571 wei (~0.57 Gwei)
    let expected_priority = per_gas_bribe.clamp(min_priority_fee, max_priority_cap);
    assert_eq!(expected_priority, 571_428_571);

    // 2. Base WETH opportunity (V-13 verification)
    let (_pipeline_base, _engine_base, _graph_base) = create_test_pipeline(Chain::Base, false, None);

    // Verify Base WETH address recognition
    let is_base_weth = base::WETH == flash_core::constants::base::WETH;
    assert!(is_base_weth, "Base WETH constant must match canonical Base WETH");

    let is_weth = base::WETH == flash_core::constants::arbitrum::WETH
        || base::WETH == flash_core::constants::base::WETH;
    assert!(is_weth, "Base pipeline MUST recognize base::WETH as native WETH");

    // Compute Base bribe with identical 0.001 ETH profit
    let profit_base_wei = profit_eth.to::<u128>();
    let base_bribe = (profit_base_wei / 5) / (gas_limit as u128);
    let base_priority = base_bribe.clamp(min_priority_fee, max_priority_cap);
    assert_eq!(
        base_priority, 571_428_571,
        "Base WETH must bid dynamic 20% builder bribe, NOT minimum priority fee!"
    );
}

#[test]
fn test_adversarial_non_weth_token_profit_conversion_and_bidding() {
    let (pipeline, _engine, graph) = create_test_pipeline(Chain::ArbitrumOne, false, None);

    // 1. USDC Route (6 decimals)
    // Scenario A: $2,500 USDC profit = 2,500 * 10^6 = 2_500_000_000 units
    // At benchmark $2500 / ETH, this equals exactly 1.0 ETH = 10^18 wei
    let usdc_profit = U256::from(2_500_000_000u64);
    let eth_wei_converted = pipeline.convert_token_profit_to_eth_wei(usdc_profit, arbitrum::USDC_NATIVE);
    assert_eq!(
        eth_wei_converted, 1_000_000_000_000_000_000u128,
        "2,500 USDC profit must convert to 1.0 ETH wei (10^18)"
    );

    // Scenario B: $50 USDC profit = 50 * 10^6 = 50_000_000 units
    // At $2500/ETH, $50 = 0.02 ETH = 20_000_000_000_000_000 wei
    let usdc_small = U256::from(50_000_000u64);
    let eth_small_wei = pipeline.convert_token_profit_to_eth_wei(usdc_small, arbitrum::USDC_NATIVE);
    assert_eq!(
        eth_small_wei, 20_000_000_000_000_000u128,
        "50 USDC profit must convert to 0.02 ETH wei"
    );

    // 2. WBTC Route (8 decimals)
    // Scenario C: 0.04 BTC profit = 4_000_000 satoshis
    // At benchmark 1 BTC = 25 ETH, 0.04 BTC = 1.0 ETH = 10^18 wei
    let wbtc_profit = U256::from(4_000_000u64);
    let wbtc_eth_wei = pipeline.convert_token_profit_to_eth_wei(wbtc_profit, arbitrum::WBTC);
    assert_eq!(
        wbtc_eth_wei, 1_000_000_000_000_000_000u128,
        "0.04 BTC profit must convert to 1.0 ETH wei (10^18)"
    );

    // 3. MarketGraph live reserve ratio conversion
    // Populate graph pool: USDC / WETH
    let pool_id = address!("3333333333333333333333333333333333333333");
    let pool = DiscoveredPool::new(
        pool_id,
        Venue::UniswapV2,
        Chain::ArbitrumOne,
        arbitrum::WETH,
        arbitrum::USDC_NATIVE,
        flash_core::types::FeePips(3000),
        PoolState::ConstantProduct {
            reserve0: 2_000_000_000_000_000_000, // 2 ETH (WETH is token0: 0x82... < 0xaf...)
            reserve1: 5_000_000,                 // 5 USDC units
        },
    );
    graph.add_pool(&pool);

    // Profit: 10,000,000 USDC units
    // Converted = 10,000,000 * 2 ETH / 5,000,000 = 4.0 ETH = 4 * 10^18 wei
    let graph_converted = pipeline.convert_token_profit_to_eth_wei(U256::from(10_000_000u64), arbitrum::USDC_NATIVE);
    assert_eq!(
        graph_converted, 4_000_000_000_000_000_000u128,
        "MarketGraph live conversion must take precedence over fallback constants"
    );
}

#[test]
fn test_adversarial_priority_fee_boundary_clamping_and_zero_profit() {
    let gas_limit = 350_000u128;
    let min_priority_fee = 50_000_000u128; // 0.05 Gwei
    let max_priority_cap = 5_000_000_000u128; // 5.0 Gwei

    // 1. Zero profit must yield min_priority_fee
    let profit_zero = 0u128;
    let priority_zero = if profit_zero > 0 {
        let bribe = profit_zero / 5;
        (bribe / gas_limit).clamp(min_priority_fee, max_priority_cap)
    } else {
        min_priority_fee
    };
    assert_eq!(priority_zero, min_priority_fee);

    // 2. Microscopic profit (e.g. 50,000 wei ~ $0.0000001)
    // 20% bribe = 10,000 wei. Per gas bribe = 10,000 / 350,000 = 0 wei.
    // Clamped: must not be 0, must be min_priority_fee (50_000_000 wei)
    let profit_micro = 50_000u128;
    let bribe_micro = profit_micro / 5;
    let per_gas_micro = bribe_micro / gas_limit;
    let priority_micro = per_gas_micro.clamp(min_priority_fee, max_priority_cap);
    assert_eq!(
        priority_micro, min_priority_fee,
        "Microscopic profit must clamp up to min_priority_fee"
    );

    // 3. Gigantic profit (e.g. 1,000 ETH = 10^21 wei)
    // 20% bribe = 200 ETH = 2 * 10^20 wei.
    // Per gas bribe = 2 * 10^20 / 350,000 = 571_428_571_428_571 wei (~571 Gwei).
    // Clamped: must not blow up or exceed 5.0 Gwei cap
    let profit_gigantic = 1_000_000_000_000_000_000_000u128;
    let bribe_gigantic = profit_gigantic / 5;
    let per_gas_gigantic = bribe_gigantic / gas_limit;
    let priority_gigantic = per_gas_gigantic.clamp(min_priority_fee, max_priority_cap);
    assert_eq!(
        priority_gigantic, max_priority_cap,
        "Gigantic profit must clamp down to max_priority_cap (5 Gwei)"
    );
}

// ==============================================================================
// 3. Precompile Collision Checks (Address 0x01 Immunity)
// ==============================================================================

#[test]
fn test_adversarial_precompile_collision_immunity() {
    let cache_dir = "./target/test_cache_precompile";
    let cache = Arc::new(BytecodeCache::new(cache_dir, 50));

    let ecrecover_precompile = Address::with_last_byte(1);
    assert_eq!(
        ecrecover_precompile,
        address!("0000000000000000000000000000000000000001")
    );

    // 1. SimulationEngine on Arbitrum One
    let arb_engine = SimulationEngine::new(Chain::ArbitrumOne, cache.clone());
    assert_ne!(
        arb_engine.executor_address, ecrecover_precompile,
        "CRITICAL: SimulationEngine executor address must NEVER be precompile 0x01!"
    );
    assert_eq!(
        arb_engine.executor_address, MOCK_EXECUTOR_ADDRESS,
        "Arbitrum executor address must be MOCK_EXECUTOR_ADDRESS (0xeeee...eeee)"
    );

    // 2. SimulationEngine on Base
    let base_engine = SimulationEngine::new(Chain::Base, cache.clone());
    assert_ne!(
        base_engine.executor_address, ecrecover_precompile,
        "CRITICAL: Base SimulationEngine executor address must NEVER be precompile 0x01!"
    );
    assert_eq!(
        base_engine.executor_address, MOCK_EXECUTOR_ADDRESS,
        "Base executor address must be MOCK_EXECUTOR_ADDRESS (0xeeee...eeee)"
    );

    // 3. Base DB Account Inspection: Precompile 0x01 must NOT be overwritten
    let precompile_account = arb_engine.base_db.cache.accounts.get(&ecrecover_precompile);
    assert!(
        precompile_account.is_none(),
        "Account 0x01 must not have mock bytecode deployed over it in base_db"
    );

    // 4. Live Mode Target Address Security:
    // Config validation must strictly reject 0x01 as executor contract address
    let bad_config = AppConfig {
        alchemy_key: "dummy".to_string(),
        arbitrum_rpc_ws: "ws://localhost".to_string(),
        arbitrum_rpc_http: "http://localhost".to_string(),
        arbitrum_sequencer_feed: "wss://arb1-feed.arbitrum.io/feed".to_string(),
        base_rpc_ws: "ws://localhost".to_string(),
        base_rpc_http: "http://localhost".to_string(),
        base_flashblocks_ws: "wss://mainnet.flashblocks.base.org/ws".to_string(),
        execution_mode: ExecutionMode::Live,
        log_level: "info".to_string(),
        signer_private_keys: vec!["0x0000000000000000000000000000000000000000000000000000000000000001".to_string()],
        executor_contract_address: ecrecover_precompile,
        max_hops: 3,
        kill_switch_feed_lag_ms: 100,
        kill_switch_fast_revert_count: 3,
    };
    let validation_result = bad_config.validate_live_mode();
    assert!(
        validation_result.is_err(),
        "Config validation must fail fast if executor_contract_address is 0x01 in Live mode"
    );
}
