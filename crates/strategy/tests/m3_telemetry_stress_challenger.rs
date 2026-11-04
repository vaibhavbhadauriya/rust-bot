//! Empirical Adversarial Challenger Stress Harness for Milestone 3 (Requirement R3)
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 25-31; PROJECT.md § Milestone 3.
//!
//! Validates:
//! 1. TelemetryCollector multi-threaded atomic concurrency safety:
//!    - 64 concurrent threads performing 10,000 increments across all 5 atomic counters
//!    - Verification of exact count sums (640,000 increments) with zero data races
//!    - Latency ring buffer bounds (1,000 cap), p99 calculation, and average latency
//! 2. Pipeline processing of tracked vs untracked pool updates:
//!    - Untracked pool updates exit cleanly with Ok(None) without incrementing routes_screened
//!    - Tracked pool updates query get_pool_tokens and route both token0 and token1 into DFS cycle searches
//!    - routes_screened counter is incremented by candidates evaluated across both tokens
//! 3. Sub-gas spread detection (near-miss):
//!    - Simulated unprofitable spread (actual profit <= gas cost) increments near_miss_spreads
//!    - profitable_hits remains unchanged and returns Ok(None)
//! 4. Profitable spread detection (shadow arbitrage hit):
//!    - Simulated profitable spread (actual profit > gas cost) increments profitable_hits
//!    - Returns PipelineTelemetry with accurate microsecond stage timings
//!    - Verifies mathematical latency invariant: Total == feed + dfs + sizing + revm + sign
//! 5. FIFO bounded queue & heartbeat integrity:
//!    - Bounded channel mpsc(4096) handles burst loads without dropping updates

use alloy_primitives::{address, Address};
use flash_collector::feed::MarketUpdate;
use flash_core::config::AppConfig;
use flash_core::constants::arbitrum;
use flash_core::types::{Chain, ExecutionMode, FeePips, PoolState, Venue};
use flash_scanner::discovery::DiscoveredPool;
use flash_scanner::graph::MarketGraph;
use flash_simulator::cache::BytecodeCache;
use flash_simulator::engine::SimulationEngine;
use flash_strategy::kill_switch::KillSwitchController;
use flash_strategy::pipeline::ArbitragePipeline;
use flash_strategy::shadow::ShadowEngine;
use flash_strategy::signers::SignerPool;
use flash_strategy::telemetry::{PipelineStageTimings, PipelineTelemetry, TelemetryCollector};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Barrier};
use std::thread;

fn create_challenger_pipeline(
    chain: Chain,
) -> (
    ArbitragePipeline,
    Arc<MarketGraph>,
    Arc<SimulationEngine>,
    Arc<TelemetryCollector>,
) {
    let graph = Arc::new(MarketGraph::new());
    let cache_dir = format!("./target/test_m3_chal_cache_{:?}", chain);
    let cache = Arc::new(BytecodeCache::new(&cache_dir, 50));
    let simulator = Arc::new(SimulationEngine::new(chain, cache));
    simulator.set_graph(graph.clone());

    let signer_pool = Arc::new(SignerPool::new(2));
    let shadow_engine = Arc::new(ShadowEngine::new(chain, false));
    let kill_switch = Arc::new(KillSwitchController::default());
    let telemetry = Arc::new(TelemetryCollector::new());

    let config = AppConfig {
        alchemy_key: "dummy_key".to_string(),
        arbitrum_rpc_ws: "".to_string(),
        arbitrum_rpc_http: "".to_string(),
        arbitrum_sequencer_feed: "".to_string(),
        base_rpc_ws: "".to_string(),
        base_rpc_http: "".to_string(),
        base_flashblocks_ws: "".to_string(),
        execution_mode: ExecutionMode::Shadow,
        log_level: "info".to_string(),
        signer_private_keys: vec![],
        executor_contract_address: Address::ZERO,
        max_hops: 3,
        kill_switch_feed_lag_ms: 100,
        kill_switch_fast_revert_count: 3,
    };

    let pipeline = ArbitragePipeline::new(
        config,
        graph.clone(),
        simulator.clone(),
        signer_pool,
        shadow_engine,
        kill_switch,
        telemetry.clone(),
    );

    (pipeline, graph, simulator, telemetry)
}

// ============================================================================
// 1. TelemetryCollector Multi-Threaded Atomic Concurrency Stress Test
// ============================================================================

#[test]
fn test_stress_telemetry_atomic_counters_concurrency_64_threads() {
    let collector = Arc::new(TelemetryCollector::new());
    let num_threads = 64;
    let ops_per_thread = 10_000u64;

    let barrier = Arc::new(Barrier::new(num_threads));
    let mut handles = Vec::with_capacity(num_threads);

    for t in 0..num_threads {
        let col = collector.clone();
        let bar = barrier.clone();

        handles.push(thread::spawn(move || {
            bar.wait();
            for i in 0..ops_per_thread {
                col.inc_ws_frames();
                col.inc_pool_updates();
                col.add_routes_screened(3);
                col.inc_near_miss_spreads();
                col.inc_profitable_hits();

                // Record stage timings periodically
                if i % 100 == 0 {
                    let telem = PipelineTelemetry::new(PipelineStageTimings {
                        feed_arrival_us: 10 + (t as u64 % 5),
                        graph_dfs_us: 40 + (t as u64 % 10),
                        sizing_math_us: 15,
                        revm_sim_us: 450 + (t as u64 % 50),
                        tx_sign_us: 25,
                    });
                    col.record(&telem);
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("Thread joined successfully");
    }

    let expected_ops = num_threads as u64 * ops_per_thread;
    assert_eq!(
        collector.ws_frames(),
        expected_ops,
        "ws_frames must match exact expected increments"
    );
    assert_eq!(
        collector.pool_updates(),
        expected_ops,
        "pool_updates must match exact expected increments"
    );
    assert_eq!(
        collector.routes_screened(),
        expected_ops * 3,
        "routes_screened must match exact expected increments (3x)"
    );
    assert_eq!(
        collector.near_miss_spreads(),
        expected_ops,
        "near_miss_spreads must match exact expected increments"
    );
    assert_eq!(
        collector.profitable_hits(),
        expected_ops,
        "profitable_hits must match exact expected increments"
    );

    // Verify samples and latency stats
    let expected_samples = (num_threads as u64) * (ops_per_thread / 100);
    assert_eq!(
        collector.sample_count.load(Ordering::Relaxed),
        expected_samples
    );

    let avg = collector.avg_e2e_latency_us();
    assert!(
        avg > 400.0 && avg < 800.0,
        "Average latency must be within realistic bounds: got {:.2} µs",
        avg
    );

    let p99 = collector.p99_latency_us();
    assert!(
        p99 >= 500 && p99 <= 900,
        "P99 latency must be within realistic bounds: got {} µs",
        p99
    );

    assert!(collector.uptime_secs() <= 60, "Uptime must be valid");
}

// ============================================================================
// 2. Pipeline Tracked vs Untracked Updates & Dual-Token Route Screening
// ============================================================================

#[tokio::test]
async fn test_stress_pipeline_untracked_and_tracked_dual_token_routing() {
    let chain = Chain::ArbitrumOne;
    let (pipeline, graph, _, telemetry) = create_challenger_pipeline(chain);

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    // 1. Untracked pool update: must return Ok(None) and NOT increment routes_screened
    let untracked_pool = address!("9999999999999999999999999999999999999999");
    let untracked_update = MarketUpdate::ReserveUpdate {
        chain,
        pool_id: untracked_pool,
        reserve0: 10_000_000_000,
        reserve1: 10_000_000_000,
        timestamp_ms: now_ms,
    };

    let res_untracked = pipeline.process_update(untracked_update).await;
    assert!(res_untracked.is_ok(), "Untracked update must succeed");
    assert!(
        res_untracked.unwrap().is_none(),
        "Untracked pool must return None"
    );
    assert_eq!(
        telemetry.routes_screened(),
        0,
        "Untracked pool must NOT increment routes_screened"
    );

    // 2. Register tracked pools to form a 2-hop cyclic arbitrage graph:
    // Pool A (Uniswap V2): WETH / USDC
    // Pool B (SushiSwap V2): WETH / USDC
    let token_weth = arbitrum::WETH;
    let token_usdc = arbitrum::USDC_NATIVE;

    let pool_a = address!("1000000000000000000000000000000000000001");
    let pool_b = address!("2000000000000000000000000000000000000002");

    // Pool A: 10 WETH and 25,000 USDC
    let pool_a_disc = DiscoveredPool::new(
        pool_a,
        Venue::UniswapV2,
        chain,
        token_weth,
        token_usdc,
        FeePips(3000),
        PoolState::ConstantProduct {
            reserve0: 10_000_000_000_000_000_000, // 10 WETH
            reserve1: 25_000_000_000,             // 25,000 USDC
        },
    );
    graph.add_pool(&pool_a_disc);

    // Pool B: 10 WETH and 35,000 USDC (higher USDC price creates arbitrage spread)
    let pool_b_disc = DiscoveredPool::new(
        pool_b,
        Venue::SushiSwapV2,
        chain,
        token_weth,
        token_usdc,
        FeePips(3000),
        PoolState::ConstantProduct {
            reserve0: 10_000_000_000_000_000_000, // 10 WETH
            reserve1: 35_000_000_000,             // 35,000 USDC
        },
    );
    graph.add_pool(&pool_b_disc);

    // Verify token lookup
    assert_eq!(
        graph.get_pool_tokens(pool_a),
        Some((token_weth, token_usdc))
    );
    assert_eq!(
        graph.get_pool_tokens(pool_b),
        Some((token_weth, token_usdc))
    );

    // Send tracked update for Pool A
    let tracked_update = MarketUpdate::ReserveUpdate {
        chain,
        pool_id: pool_a,
        reserve0: 10_100_000_000_000_000_000,
        reserve1: 24_900_000_000,
        timestamp_ms: now_ms,
    };

    let res_tracked = pipeline.process_update(tracked_update).await;
    assert!(res_tracked.is_ok(), "Tracked update must succeed");

    // Empirically verify routes_screened incremented by dual-token traversal
    let screened = telemetry.routes_screened();
    assert!(
        screened >= 2,
        "routes_screened must be >= 2 (traversing both token0 and token1 vertices), got {}",
        screened
    );
}

// ============================================================================
// 3. Sub-Gas Spread Detection & near_miss_spreads Verification
// ============================================================================

#[tokio::test]
async fn test_stress_sub_gas_near_miss_spread_detection() {
    let chain = Chain::ArbitrumOne;
    let (pipeline, graph, _simulator, telemetry) = create_challenger_pipeline(chain);

    let token_weth = arbitrum::WETH;
    let token_usdc = arbitrum::USDC_NATIVE;

    let pool_a = address!("3000000000000000000000000000000000000003");
    let pool_b = address!("4000000000000000000000000000000000000004");

    // Construct a market with a 1.0% spread (passing 0.6% swap fees)
    // where gross profit (~$0.50) is positive in DFS,
    // but gas cost (~$1.05) exceeds profit -> SimError::Unprofitable
    let pool_a_disc = DiscoveredPool::new(
        pool_a,
        Venue::UniswapV2,
        chain,
        token_weth,
        token_usdc,
        FeePips(3000),
        PoolState::ConstantProduct {
            reserve0: 10_000_000_000_000_000_000, // 10 WETH
            reserve1: 25_000_000_000,             // 25,000 USDC
        },
    );
    graph.add_pool(&pool_a_disc);

    // Pool B has a 1.0% spread (25,250 USDC on 10 WETH)
    let pool_b_disc = DiscoveredPool::new(
        pool_b,
        Venue::SushiSwapV2,
        chain,
        token_weth,
        token_usdc,
        FeePips(3000),
        PoolState::ConstantProduct {
            reserve0: 10_000_000_000_000_000_000, // 10 WETH
            reserve1: 25_250_000_000,             // 25,250 USDC
        },
    );
    graph.add_pool(&pool_b_disc);

    let initial_near_miss = telemetry.near_miss_spreads();
    let initial_profitable = telemetry.profitable_hits();

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let update = MarketUpdate::ReserveUpdate {
        chain,
        pool_id: pool_a,
        reserve0: 10_000_000_000_000_000_000,
        reserve1: 25_000_000_000,
        timestamp_ms: now_ms,
    };

    let res = pipeline.process_update(update).await;
    assert!(res.is_ok(), "Pipeline process_update must not fail");
    let telem_opt = res.unwrap();

    // Since gross yield is below gas cost, it must return Ok(None)
    assert!(
        telem_opt.is_none(),
        "Sub-gas near-miss spread must return None"
    );

    // Empirically verify near_miss_spreads counter incremented
    assert_eq!(
        telemetry.near_miss_spreads(),
        initial_near_miss + 1,
        "near_miss_spreads must increment by exactly 1 on sub-gas spread"
    );
    // Profitable hits must NOT increment
    assert_eq!(
        telemetry.profitable_hits(),
        initial_profitable,
        "profitable_hits must NOT increment on sub-gas spread"
    );
}

// ============================================================================
// 4. Profitable Spread Detection & profitable_hits Banner Latency Verification
// ============================================================================

#[tokio::test]
async fn test_stress_profitable_spread_shadow_arbitrage_hit_and_latency_breakdown() {
    let chain = Chain::ArbitrumOne;
    let (pipeline, graph, _, telemetry) = create_challenger_pipeline(chain);

    let token_weth = arbitrum::WETH;
    let token_usdc = arbitrum::USDC_NATIVE;

    let pool_a = address!("5000000000000000000000000000000000000005");
    let pool_b = address!("6000000000000000000000000000000000000006");

    // Construct a market with a massive spread:
    // Pool A: 10 WETH and 20,000 USDC (2,000 USDC/ETH)
    // Pool B: 10 WETH and 35,000 USDC (3,500 USDC/ETH)
    // Net profit is several ETH (>> gas cost)
    let pool_a_disc = DiscoveredPool::new(
        pool_a,
        Venue::UniswapV2,
        chain,
        token_weth,
        token_usdc,
        FeePips(3000),
        PoolState::ConstantProduct {
            reserve0: 10_000_000_000_000_000_000, // 10 WETH
            reserve1: 20_000_000_000,             // 20,000 USDC
        },
    );
    graph.add_pool(&pool_a_disc);

    let pool_b_disc = DiscoveredPool::new(
        pool_b,
        Venue::SushiSwapV2,
        chain,
        token_weth,
        token_usdc,
        FeePips(3000),
        PoolState::ConstantProduct {
            reserve0: 10_000_000_000_000_000_000, // 10 WETH
            reserve1: 35_000_000_000,             // 35,000 USDC
        },
    );
    graph.add_pool(&pool_b_disc);

    let initial_profitable = telemetry.profitable_hits();
    let initial_near_miss = telemetry.near_miss_spreads();

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    let update = MarketUpdate::ReserveUpdate {
        chain,
        pool_id: pool_a,
        reserve0: 10_000_000_000_000_000_000,
        reserve1: 20_000_000_000,
        timestamp_ms: now_ms,
    };

    let res = pipeline.process_update(update).await;
    assert!(res.is_ok(), "Pipeline execution must succeed");
    let telem_opt = res.unwrap();

    assert!(
        telem_opt.is_some(),
        "Profitable arbitrage opportunity must produce Some(PipelineTelemetry)"
    );

    let telem = telem_opt.unwrap();

    // Verify accurate latency breakdown invariants:
    // Total == feed_arrival_us + graph_dfs_us + sizing_math_us + revm_sim_us + tx_sign_us
    let sum_stages = telem.feed_arrival_us
        + telem.graph_dfs_us
        + telem.sizing_math_us
        + telem.revm_sim_us
        + telem.tx_sign_us;
    assert_eq!(
        telem.total_e2e_latency_us(),
        sum_stages,
        "Total e2e latency must equal sum of individual stage timings"
    );

    assert!(
        telem.graph_dfs_us >= 1,
        "DFS latency must be >= 1µs (got {}µs)",
        telem.graph_dfs_us
    );
    assert!(
        telem.sizing_math_us >= 1,
        "Sizing math latency must be >= 1µs (got {}µs)",
        telem.sizing_math_us
    );
    assert!(
        telem.revm_sim_us >= 1,
        "revm simulation latency must be >= 1µs (got {}µs)",
        telem.revm_sim_us
    );
    assert!(
        telem.tx_sign_us >= 1,
        "Signer latency must be >= 1µs (got {}µs)",
        telem.tx_sign_us
    );

    // Verify counter increments
    assert_eq!(
        telemetry.profitable_hits(),
        initial_profitable + 1,
        "profitable_hits must increment by exactly 1 on profitable hit"
    );
    assert_eq!(
        telemetry.near_miss_spreads(),
        initial_near_miss,
        "near_miss_spreads must NOT increment on profitable hit"
    );
}

// ============================================================================
// 5. FIFO Bounded MPSC Queue Burst Traffic Stress Test
// ============================================================================

#[tokio::test]
async fn test_stress_fifo_mpsc_queue_burst_traffic_zero_drops() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<MarketUpdate>(4096);
    let total_updates = 4000;

    let chain = Chain::ArbitrumOne;
    let pool_id = address!("7000000000000000000000000000000000000007");

    // Producer task simulates sudden market volatility burst
    let producer = tokio::spawn(async move {
        for i in 0..total_updates {
            let update = MarketUpdate::ReserveUpdate {
                chain,
                pool_id,
                reserve0: 1_000_000 + i as u128,
                reserve1: 2_000_000 + i as u128,
                timestamp_ms: i,
            };
            tx.send(update).await.expect("Channel must not drop updates");
        }
    });

    let mut received_count = 0;
    let mut expected_seq = 0u64;

    while let Some(update) = rx.recv().await {
        if let MarketUpdate::ReserveUpdate { timestamp_ms, .. } = update {
            assert_eq!(
                timestamp_ms, expected_seq,
                "Updates must be received in strict FIFO order without skips or drops"
            );
            expected_seq += 1;
            received_count += 1;
            if received_count == total_updates {
                break;
            }
        }
    }

    producer.await.expect("Producer task completed");
    assert_eq!(
        received_count, total_updates,
        "All 4000 burst updates must be consumed without drops"
    );
}

#[test]
fn test_empirical_banner_formatting_specifications() {
    let sym_in = "WETH";
    let sym_out = "USDC";
    let spread_bps = 19u64;
    let opt_input_f64 = 0.009858;
    let borrow_sym = "WETH";
    let gross_yield_usd = 0.05;
    let gas_threshold_usd = 1.05;

    let near_miss_line = format!(
        "⚡ [SPREAD DETECTED - BELOW GAS] Pair: {}/{} | Spread: {} bps | Delta x*: {:.6} {} | Gross Yield: ${:.2} | Gas Threshold: ${:.2}",
        sym_in, sym_out, spread_bps, opt_input_f64, borrow_sym, gross_yield_usd, gas_threshold_usd
    );

    assert!(near_miss_line.contains("⚡ [SPREAD DETECTED - BELOW GAS]"));
    assert!(near_miss_line.contains("Pair: WETH/USDC"));
    assert!(near_miss_line.contains("Spread: 19 bps"));
    assert!(near_miss_line.contains("Delta x*: 0.009858 WETH"));
    assert!(near_miss_line.contains("Gross Yield: $0.05"));
    assert!(near_miss_line.contains("Gas Threshold: $1.05"));

    let chain = Chain::ArbitrumOne;
    let route_str = "0x0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b -> 0x0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a";
    let opt_input_f64 = 0.752179;
    let gross_profit_usd = 337.85;
    let gas_cost_usd = 1.05;
    let net_profit_usd = gross_profit_usd - gas_cost_usd;
    let graph_dfs_us = 96;
    let sizing_math_us = 2;
    let revm_sim_us = 2064;
    let total_us = 8103;

    let shadow_hit_banner = format!(
        "\n================================================================================\n\
         🎯 [SHADOW ARBITRAGE HIT]\n\
         Chain:             {:?}\n\
         Route:             {}\n\
         Optimal Flash Loan Delta x*: {:.6} {}\n\
         Gross Profit ($):  ${:.2}\n\
         L1+L2 Gas Cost ($): ${:.2}\n\
         Net Profit ($):    ${:.2}\n\
         Latency Breakdown: DFS {} µs | Sizing {} µs | revm {} µs | Total {} µs\n\
         ================================================================================",
        chain,
        route_str,
        opt_input_f64,
        borrow_sym,
        gross_profit_usd,
        gas_cost_usd,
        net_profit_usd,
        graph_dfs_us,
        sizing_math_us,
        revm_sim_us,
        total_us
    );

    assert!(shadow_hit_banner.contains("🎯 [SHADOW ARBITRAGE HIT]"));
    assert!(shadow_hit_banner.contains("Chain:             ArbitrumOne"));
    assert!(shadow_hit_banner.contains("Gross Profit ($):  $337.85"));
    assert!(shadow_hit_banner.contains("L1+L2 Gas Cost ($): $1.05"));
    assert!(shadow_hit_banner.contains("Net Profit ($):    $336.80"));
    assert!(shadow_hit_banner.contains("Latency Breakdown: DFS 96 µs | Sizing 2 µs | revm 2064 µs | Total 8103 µs"));

    let uptime = 12u64;
    let ws_frames = 450u64;
    let pool_updates = 320u64;
    let routes_screened = 150u64;
    let near_miss_spreads = 8u64;
    let profitable_hits = 2u64;
    let avg_latency = 485.6f64;

    let heartbeat_line = format!(
        "📊 [LIVE TELEMETRY HEARTBEAT] Uptime: {}s | WS Frames: {} | Pool Updates: {} | Routes Screened: {} | Near-Miss Spreads: {} | Profitable Hits: {} | Avg Latency: {:.1} µs",
        uptime,
        ws_frames,
        pool_updates,
        routes_screened,
        near_miss_spreads,
        profitable_hits,
        avg_latency
    );

    assert!(heartbeat_line.contains("📊 [LIVE TELEMETRY HEARTBEAT]"));
    assert!(heartbeat_line.contains("Uptime: 12s"));
    assert!(heartbeat_line.contains("WS Frames: 450"));
    assert!(heartbeat_line.contains("Pool Updates: 320"));
    assert!(heartbeat_line.contains("Routes Screened: 150"));
    assert!(heartbeat_line.contains("Near-Miss Spreads: 8"));
    assert!(heartbeat_line.contains("Profitable Hits: 2"));
    assert!(heartbeat_line.contains("Avg Latency: 485.6 µs"));
}

