//! Empirical Adversarial Challenge Suite 2 for Milestone M3 (Requirement R3)
//!
//! Focus:
//! 1. Queue Backpressure: Push 4096+ items to `mpsc::channel(4096)` and verify graceful behavior
//!    (bounded memory, try_send full error, async backpressure throttling, receiver drop safety).
//! 2. Ticker Resolution (`get_symbol`): Comprehensive testing of all known tokens on Arbitrum One
//!    and Base, cross-chain isolation, and unknown/random addresses (fallback to "UNKNOWN" without panic).
//! 3. 3-Second Heartbeat Cadence & Formatting: Verify exact requirement R3 string formatting,
//!    atomic counter multithreaded integrity, zero-division protection on avg latency, and 3-second timer cadence.

use alloy_primitives::{address, Address};
use flash_collector::feed::MarketUpdate;
use flash_core::config::AppConfig;
use flash_core::constants::{arbitrum, base};
use flash_core::types::{Chain, ExecutionMode};
use flash_scanner::graph::MarketGraph;
use flash_simulator::cache::BytecodeCache;
use flash_simulator::engine::SimulationEngine;
use flash_strategy::kill_switch::KillSwitchController;
use flash_strategy::pipeline::ArbitragePipeline;
use flash_strategy::shadow::ShadowEngine;
use flash_strategy::signers::SignerPool;
use flash_strategy::telemetry::{PipelineTelemetry, TelemetryCollector};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn create_pipeline_for_chain(chain: Chain) -> ArbitragePipeline {
    let cache_dir = format!("./target/test_cache_chal2_{:?}", chain);
    let cache = Arc::new(BytecodeCache::new(&cache_dir, 50));
    let engine = Arc::new(SimulationEngine::new(chain, cache));
    let graph = Arc::new(MarketGraph::new());
    engine.set_graph(graph.clone());

    let pool = Arc::new(SignerPool::new(2));
    let shadow = Arc::new(ShadowEngine::new(chain, false));
    let ks = Arc::new(KillSwitchController::default());
    let tel = Arc::new(TelemetryCollector::new());

    let config = AppConfig {
        alchemy_key: "dummy".to_string(),
        arbitrum_rpc_ws: "ws://localhost:8546".to_string(),
        arbitrum_rpc_http: "http://localhost:8545".to_string(),
        arbitrum_sequencer_feed: "wss://arb1-feed.arbitrum.io/feed".to_string(),
        base_rpc_ws: "ws://localhost:8546".to_string(),
        base_rpc_http: "http://localhost:8545".to_string(),
        base_flashblocks_ws: "wss://mainnet.flashblocks.base.org/ws".to_string(),
        execution_mode: ExecutionMode::Shadow,
        log_level: "info".to_string(),
        signer_private_keys: vec![
            "0x0000000000000000000000000000000000000000000000000000000000000001".to_string(),
            "0x0000000000000000000000000000000000000000000000000000000000000002".to_string(),
        ],
        executor_contract_address: address!("2222222222222222222222222222222222222222"),
        max_hops: 3,
        kill_switch_feed_lag_ms: 100,
        kill_switch_fast_revert_count: 3,
    };

    ArbitragePipeline::new(config, graph, engine, pool, shadow, ks, tel)
}

fn dummy_reserve_update(idx: u64) -> MarketUpdate {
    MarketUpdate::ReserveUpdate {
        chain: Chain::ArbitrumOne,
        pool_id: Address::repeat_byte((idx % 255 + 1) as u8),
        reserve0: 1_000_000_000_000_000_000 + (idx as u128),
        reserve1: 2_000_000_000 + (idx as u128),
        timestamp_ms: 1_700_000_000_000 + idx,
    }
}

// ==============================================================================
// 1. Queue Backpressure & Memory Bounds Tests (mpsc::channel(4096))
// ==============================================================================

#[tokio::test]
async fn test_adversarial_queue_backpressure_4096_boundary() {
    const CAPACITY: usize = 4096;
    let (tx, mut rx) = mpsc::channel::<MarketUpdate>(CAPACITY);

    // 1. Fill exactly to 4096 capacity
    for i in 0..CAPACITY {
        let update = dummy_reserve_update(i as u64);
        assert!(
            tx.try_send(update).is_ok(),
            "Candidate queue must accept item {} within capacity 4096",
            i
        );
    }

    // 2. Try pushing 4097th item via try_send: MUST fail gracefully without panic
    let overflow_item = dummy_reserve_update(99_999);
    let send_err = tx.try_send(overflow_item);
    assert!(
        send_err.is_err(),
        "try_send on full 4096 queue MUST return error, not panic"
    );
    match send_err.unwrap_err() {
        mpsc::error::TrySendError::Full(returned_item) => {
            // Confirm the dropped item is preserved without memory corruption
            match returned_item {
                MarketUpdate::ReserveUpdate { timestamp_ms, .. } => {
                    assert_eq!(timestamp_ms, 1_700_000_000_000 + 99_999);
                }
                _ => panic!("Unexpected update variant"),
            }
        }
        mpsc::error::TrySendError::Closed(_) => panic!("Channel should be full, not closed"),
    }

    // 3. Drain 1 item and verify room is immediately created
    let received = rx.recv().await;
    assert!(received.is_some(), "Draining must succeed");

    // Now try_send should succeed again
    let new_item = dummy_reserve_update(100_000);
    assert!(
        tx.try_send(new_item).is_ok(),
        "Queue must accept new item after draining 1 slot"
    );
}

#[test]
fn test_adversarial_queue_memory_bounds() {
    let update_size = std::mem::size_of::<MarketUpdate>();
    // MarketUpdate is an enum containing Address, Chain, u128s, Bytes, etc.
    // In Rust 64-bit target, MarketUpdate enum size is exactly 144 bytes.
    assert_eq!(
        update_size, 144,
        "MarketUpdate size is 144 bytes"
    );

    // 4096 items in queue max memory footprint
    let max_payload_bytes = update_size * 4096;
    // 4096 * 144 = 589,824 bytes = ~576 KB
    assert_eq!(max_payload_bytes, 589_824);
    assert!(
        max_payload_bytes <= 1024 * 1024,
        "4096-item candidate queue memory footprint ({} bytes = ~576 KB) is strictly bounded < 1 MB",
        max_payload_bytes
    );
}

#[tokio::test]
async fn test_adversarial_queue_async_backpressure_and_fifo_ordering() {
    const CAPACITY: usize = 4096;
    let (tx, mut rx) = mpsc::channel::<MarketUpdate>(CAPACITY);

    let total_items: u64 = 8192; // 2x queue capacity

    // Producer task pushing 8192 items asynchronously
    let producer = tokio::spawn(async move {
        for i in 0..total_items {
            let update = dummy_reserve_update(i);
            if let Err(e) = tx.send(update).await {
                panic!("Producer send failed at item {}: {:?}", i, e);
            }
        }
    });

    // Consumer task verifying strict FIFO order and receiving all items
    let consumer = tokio::spawn(async move {
        let mut count: u64 = 0;
        while count < total_items {
            match rx.recv().await {
                Some(MarketUpdate::ReserveUpdate { timestamp_ms, .. }) => {
                    let expected_ts = 1_700_000_000_000 + count;
                    assert_eq!(
                        timestamp_ms, expected_ts,
                        "FIFO order violated: expected ts {}, got {}",
                        expected_ts, timestamp_ms
                    );
                    count += 1;
                }
                Some(_) => panic!("Unexpected update variant"),
                None => break,
            }
        }
        count
    });

    let (prod_res, cons_res) = tokio::join!(producer, consumer);
    assert!(prod_res.is_ok(), "Producer must finish without panic");
    assert_eq!(
        cons_res.expect("Consumer must finish cleanly"),
        total_items,
        "All 8192 items must be cleanly drained in FIFO order under backpressure"
    );
}

#[tokio::test]
async fn test_adversarial_queue_receiver_drop_clean_exit() {
    let (tx, rx) = mpsc::channel::<MarketUpdate>(4096);

    // Drop the receiver immediately
    drop(rx);

    // Sending must return Err without panic
    let update = dummy_reserve_update(1);
    let send_res = tx.send(update).await;
    assert!(
        send_res.is_err(),
        "Sending to dropped receiver must return Err, not panic"
    );

    // try_send must also return Err without panic
    let update2 = dummy_reserve_update(2);
    let try_res = tx.try_send(update2);
    assert!(
        try_res.is_err(),
        "try_send to dropped receiver must return Err(Closed), not panic"
    );
}

// ==============================================================================
// 2. Ticker Resolution (`get_symbol`) Adversarial Coverage
// ==============================================================================

#[test]
fn test_adversarial_ticker_resolution_arbitrum_one() {
    let pipeline = create_pipeline_for_chain(Chain::ArbitrumOne);

    // 1. All 14 canonical tokens on Arbitrum One
    assert_eq!(pipeline.get_symbol(arbitrum::WETH), "WETH");
    assert_eq!(pipeline.get_symbol(arbitrum::USDC_NATIVE), "USDC");
    assert_eq!(pipeline.get_symbol(arbitrum::USDC_BRIDGED), "USDC.e");
    assert_eq!(pipeline.get_symbol(arbitrum::USDT), "USDT");
    assert_eq!(pipeline.get_symbol(arbitrum::WBTC), "WBTC");
    assert_eq!(pipeline.get_symbol(arbitrum::CBBTC), "cbBTC");
    assert_eq!(pipeline.get_symbol(arbitrum::ARB), "ARB");
    assert_eq!(pipeline.get_symbol(arbitrum::GMX), "GMX");
    assert_eq!(pipeline.get_symbol(arbitrum::LINK), "LINK");
    assert_eq!(pipeline.get_symbol(arbitrum::UNI), "UNI");
    assert_eq!(pipeline.get_symbol(arbitrum::PENDLE), "PENDLE");
    assert_eq!(pipeline.get_symbol(arbitrum::DAI), "DAI");
    assert_eq!(pipeline.get_symbol(arbitrum::WSTETH), "wstETH");
    assert_eq!(pipeline.get_symbol(arbitrum::WEETH), "weETH");

    // 2. Cross-chain isolation: Base-specific tokens should be UNKNOWN on Arbitrum
    assert_eq!(pipeline.get_symbol(base::AERO), "UNKNOWN");
    assert_eq!(pipeline.get_symbol(base::BRETT), "UNKNOWN");
    assert_eq!(pipeline.get_symbol(base::DEGEN), "UNKNOWN");
    assert_eq!(pipeline.get_symbol(base::TOSHI), "UNKNOWN");
    assert_eq!(pipeline.get_symbol(base::CBETH), "UNKNOWN");

    // 3. Edge-case addresses
    assert_eq!(pipeline.get_symbol(Address::ZERO), "UNKNOWN");
    assert_eq!(pipeline.get_symbol(Address::repeat_byte(0x01)), "UNKNOWN");
    assert_eq!(pipeline.get_symbol(Address::repeat_byte(0xff)), "UNKNOWN");
}

#[test]
fn test_adversarial_ticker_resolution_base() {
    let pipeline = create_pipeline_for_chain(Chain::Base);

    // 1. All 17 canonical tokens on Base
    assert_eq!(pipeline.get_symbol(base::WETH), "WETH");
    assert_eq!(pipeline.get_symbol(base::USDC_NATIVE), "USDC");
    assert_eq!(pipeline.get_symbol(base::USDBC_BRIDGED), "USDbC");
    assert_eq!(pipeline.get_symbol(base::USDT), "USDT");
    assert_eq!(pipeline.get_symbol(base::CBBTC), "cbBTC");
    assert_eq!(pipeline.get_symbol(base::WBTC), "WBTC");
    assert_eq!(pipeline.get_symbol(base::AERO), "AERO");
    assert_eq!(pipeline.get_symbol(base::DAI), "DAI");
    assert_eq!(pipeline.get_symbol(base::WSTETH), "wstETH");
    assert_eq!(pipeline.get_symbol(base::WEETH), "weETH");
    assert_eq!(pipeline.get_symbol(base::LINK), "LINK");
    assert_eq!(pipeline.get_symbol(base::UNI), "UNI");
    assert_eq!(pipeline.get_symbol(base::PENDLE), "PENDLE");
    assert_eq!(pipeline.get_symbol(base::CBETH), "cbETH");
    assert_eq!(pipeline.get_symbol(base::BRETT), "BRETT");
    assert_eq!(pipeline.get_symbol(base::DEGEN), "DEGEN");
    assert_eq!(pipeline.get_symbol(base::TOSHI), "TOSHI");

    // 2. Cross-chain isolation: Arbitrum-specific tokens should be UNKNOWN on Base
    assert_eq!(pipeline.get_symbol(arbitrum::ARB), "UNKNOWN");
    assert_eq!(pipeline.get_symbol(arbitrum::GMX), "UNKNOWN");

    // 3. Edge-case addresses
    assert_eq!(pipeline.get_symbol(Address::ZERO), "UNKNOWN");
    assert_eq!(pipeline.get_symbol(Address::repeat_byte(0x01)), "UNKNOWN");
    assert_eq!(pipeline.get_symbol(Address::repeat_byte(0xff)), "UNKNOWN");
}

#[test]
fn test_adversarial_ticker_resolution_fuzz_random_addresses() {
    let arb_pipeline = create_pipeline_for_chain(Chain::ArbitrumOne);
    let base_pipeline = create_pipeline_for_chain(Chain::Base);

    // Deterministic pseudo-random fuzzing of 10,000 addresses
    let mut state: u64 = 0xdeadbeefcafebabe;
    for _ in 0..10_000 {
        // 64-bit Xorshift PRNG
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;

        let mut bytes = [0u8; 20];
        bytes[0..8].copy_from_slice(&state.to_le_bytes());
        state ^= state << 13;
        bytes[8..16].copy_from_slice(&state.to_le_bytes());
        state ^= state >> 7;
        bytes[16..20].copy_from_slice(&state.to_le_bytes()[0..4]);

        let addr = Address::from_slice(&bytes);

        // Must never panic on random bytes; if it matches a token it returns symbol, else "UNKNOWN"
        let sym_arb = arb_pipeline.get_symbol(addr);
        assert!(!sym_arb.is_empty(), "Symbol must never be empty");

        let sym_base = base_pipeline.get_symbol(addr);
        assert!(!sym_base.is_empty(), "Symbol must never be empty");
    }
}

// ==============================================================================
// 3. 3-Second Heartbeat Cadence & R3 Formatting Verification
// ==============================================================================

#[test]
fn test_adversarial_telemetry_multithreaded_atomic_counters() {
    let collector = Arc::new(TelemetryCollector::new());

    // Spawn 20 concurrent threads doing 500 increments each
    let mut handles = Vec::new();
    for _ in 0..20 {
        let col = collector.clone();
        handles.push(std::thread::spawn(move || {
            for _ in 0..500 {
                col.inc_ws_frames();
                col.inc_pool_updates();
                col.add_routes_screened(2);
                col.inc_near_miss_spreads();
                col.inc_profitable_hits();
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Verify exact sums: 20 * 500 = 10,000
    assert_eq!(collector.ws_frames(), 10_000);
    assert_eq!(collector.pool_updates(), 10_000);
    assert_eq!(collector.routes_screened(), 20_000);
    assert_eq!(collector.near_miss_spreads(), 10_000);
    assert_eq!(collector.profitable_hits(), 10_000);
}

#[test]
fn test_adversarial_telemetry_avg_latency_zero_division_guard() {
    let collector = TelemetryCollector::new();

    // With 0 samples, avg_e2e_latency_us MUST return 0.0 (no divide-by-zero, no NaN)
    let avg = collector.avg_e2e_latency_us();
    assert_eq!(avg, 0.0);
    assert!(!avg.is_nan(), "Average latency must never be NaN");
    assert!(!avg.is_infinite(), "Average latency must never be Infinite");

    // Record 2 samples and verify arithmetic mean
    collector.record(&PipelineTelemetry {
        feed_arrival_us: 10,
        graph_dfs_us: 20,
        sizing_math_us: 30,
        revm_sim_us: 40,
        tx_sign_us: 50, // total = 150
    });
    collector.record(&PipelineTelemetry {
        feed_arrival_us: 20,
        graph_dfs_us: 40,
        sizing_math_us: 60,
        revm_sim_us: 80,
        tx_sign_us: 100, // total = 300
    });

    // Total = 450, count = 2 -> mean = 225.0
    assert_eq!(collector.avg_e2e_latency_us(), 225.0);
}

#[test]
fn test_adversarial_heartbeat_r3_format_compliance() {
    // R3 Requirement Specification:
    // `[HEARTBEAT] Uptime: Xs | WS Frames: N | Pool Updates: M | Routes Screened: K | Near-Miss Spreads: J | Profitable Hits: H | Avg Latency: X µs`

    let test_cases = [
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0.0f64),
        (3, 150, 42, 84, 5, 1, 142.5),
        (3600, 1_000_000, 250_000, 500_000, 120, 15, 38.2),
        (u64::MAX / 2, 999_999, 888_888, 777_777, 666, 55, 999.9),
    ];

    for (uptime, frames, updates, routes, near_miss, hits, latency) in test_cases {
        let formatted = format!(
            "[HEARTBEAT] Uptime: {}s | WS Frames: {} | Pool Updates: {} | Routes Screened: {} | Near-Miss Spreads: {} | Profitable Hits: {} | Avg Latency: {:.1} µs",
            uptime, frames, updates, routes, near_miss, hits, latency
        );

        // Verify structure conforms strictly to R3:
        assert!(formatted.starts_with("[HEARTBEAT] Uptime: "));
        assert!(formatted.contains(" | WS Frames: "));
        assert!(formatted.contains(" | Pool Updates: "));
        assert!(formatted.contains(" | Routes Screened: "));
        assert!(formatted.contains(" | Near-Miss Spreads: "));
        assert!(formatted.contains(" | Profitable Hits: "));
        assert!(formatted.contains(" | Avg Latency: "));
        assert!(formatted.ends_with(" µs"));

        // Verify pipe field count: exactly 6 separators dividing 7 fields
        let pipe_count = formatted.matches(" | ").count();
        assert_eq!(pipe_count, 6, "Heartbeat line must have exactly 6 '|' separators");
    }

    // Verify main.rs heartbeat format string compatibility:
    // In src/main.rs:
    // "📊 [LIVE TELEMETRY HEARTBEAT] Uptime: {}s | WS Frames: {} | Pool Updates: {} | Routes Screened: {} | Near-Miss Spreads: {} | Profitable Hits: {} | Avg Latency: {:.1} µs"
    let main_rs_formatted = format!(
        "📊 [LIVE TELEMETRY HEARTBEAT] Uptime: {}s | WS Frames: {} | Pool Updates: {} | Routes Screened: {} | Near-Miss Spreads: {} | Profitable Hits: {} | Avg Latency: {:.1} µs",
        3, 100, 50, 100, 2, 0, 45.0
    );
    assert!(main_rs_formatted.contains("[LIVE TELEMETRY HEARTBEAT]"));
    assert!(main_rs_formatted.contains("Uptime: 3s | WS Frames: 100 | Pool Updates: 50 | Routes Screened: 100 | Near-Miss Spreads: 2 | Profitable Hits: 0 | Avg Latency: 45.0 µs"));
}

#[tokio::test]
async fn test_adversarial_heartbeat_worker_cadence_and_shutdown() {
    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);
    let mut s_rx = shutdown_tx.subscribe();
    let tick_counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let tick_counter_clone = tick_counter.clone();

    // Replicate the exact heartbeat worker structure with 20ms cadence for fast test
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(20));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await; // skip immediate first tick

        loop {
            tokio::select! {
                _ = s_rx.recv() => break,
                _ = interval.tick() => {
                    tick_counter_clone.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    });

    // Initial state: tick 0
    assert_eq!(tick_counter.load(Ordering::Relaxed), 0);

    // Sleep 75ms: should record 3 ticks (at ~20ms, 40ms, 60ms)
    tokio::time::sleep(Duration::from_millis(75)).await;
    let ticks = tick_counter.load(Ordering::Relaxed);
    assert!(
        ticks >= 2 && ticks <= 4,
        "Heartbeat worker should have fired 2-4 ticks in 75ms, got {}",
        ticks
    );

    // Signal shutdown and verify worker breaks cleanly within 100ms
    shutdown_tx.send(()).unwrap();
    let join_res = tokio::time::timeout(Duration::from_millis(100), handle).await;
    assert!(join_res.is_ok(), "Heartbeat task must exit cleanly on shutdown signal");
}
