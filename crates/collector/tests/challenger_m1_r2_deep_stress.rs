//! Empirical Deep Stress & Verification Test Suite for Milestone 1 Round 2
//! Author: M1 Stream Challenger 1 Round 2 (teamwork_challenger)
//!
//! Scope:
//! 1. Mathematical boundary stress on checked U256 -> u128 downcasts in `process_dex_log`.
//! 2. Rapid connection drop & fallback resilience for Arbitrum and Base log stream handlers.
//! 3. High-concurrency race condition and channel saturation stress across multiple workers.
//! 4. Handshake compliance (`Arbitrum-Feed-Client-Version: 2`) and HTTP error code resilience.
//! 5. Base Flashblock sub-block lifecycle (gas budget reset at index 9) interleaved with live logs.

use alloy_primitives::{Address, U256};
use flash_collector::arbitrum::ArbitrumFeedHandler;
use flash_collector::base::{
    process_dex_log, process_flashblock, BaseLogStreamHandler, FlashblockGasTracker,
    FlashblockMessage, DEFAULT_BASE_GAS_LIMIT, TOPIC_PANCAKE_V3_SWAP, TOPIC_UNIV2_SYNC,
    TOPIC_UNIV3_SWAP,
};
use flash_collector::feed::{MarketFeedPublisher, MarketUpdate};
use flash_core::types::Chain;
use futures::{SinkExt, StreamExt};
use parking_lot::Mutex;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::Message;

// ==============================================================================
// 1. Mathematical Boundary Stress on Checked U256 -> u128 Downcasts
// ==============================================================================

#[test]
fn test_empirical_r2_downcast_boundary_stress() {
    let publisher = MarketFeedPublisher::new(64);
    let mut rx = publisher.subscribe();
    let pool = Address::repeat_byte(0x77);

    // 1. Boundary: reserve0 = u128::MAX, reserve1 = u128::MAX
    let mut data_max_u128 = [0u8; 64];
    data_max_u128[0..32].copy_from_slice(&U256::from(u128::MAX).to_be_bytes::<32>());
    data_max_u128[32..64].copy_from_slice(&U256::from(u128::MAX).to_be_bytes::<32>());
    assert!(
        process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &data_max_u128, &publisher),
        "u128::MAX reserves must be accepted"
    );
    match rx.try_recv().expect("Receive reserve update") {
        MarketUpdate::ReserveUpdate { reserve0, reserve1, .. } => {
            assert_eq!(reserve0, u128::MAX);
            assert_eq!(reserve1, u128::MAX);
        }
        _ => panic!("Expected ReserveUpdate"),
    }

    // 2. Boundary: reserve0 = u128::MAX + 1 (lower 128 bits 0, bit 128 is 1)
    let u128_max_plus_1 = U256::from(u128::MAX) + U256::from(1u64);
    let mut data_overflow_r0 = [0u8; 64];
    data_overflow_r0[0..32].copy_from_slice(&u128_max_plus_1.to_be_bytes::<32>());
    data_overflow_r0[32..64].copy_from_slice(&U256::from(100u64).to_be_bytes::<32>());
    let res = catch_unwind(AssertUnwindSafe(|| {
        process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &data_overflow_r0, &publisher)
    }));
    assert!(res.is_ok(), "u128::MAX + 1 must not panic");
    assert!(!res.unwrap(), "u128::MAX + 1 must return false");
    assert!(rx.try_recv().is_err(), "No event should be published on overflow");

    // 3. Boundary: reserve1 = u128::MAX + 1
    let mut data_overflow_r1 = [0u8; 64];
    data_overflow_r1[0..32].copy_from_slice(&U256::from(100u64).to_be_bytes::<32>());
    data_overflow_r1[32..64].copy_from_slice(&u128_max_plus_1.to_be_bytes::<32>());
    let res = catch_unwind(AssertUnwindSafe(|| {
        process_dex_log(Chain::Base, pool, &[TOPIC_UNIV2_SYNC], &data_overflow_r1, &publisher)
    }));
    assert!(res.is_ok(), "reserve1 overflow must not panic");
    assert!(!res.unwrap(), "reserve1 overflow must return false");

    // 4. Boundary: V3 liquidity = u128::MAX
    let mut data_v3_max = [0u8; 160];
    let sqrt_p = U256::from(123456789u64);
    data_v3_max[64..96].copy_from_slice(&sqrt_p.to_be_bytes::<32>());
    data_v3_max[96..128].copy_from_slice(&U256::from(u128::MAX).to_be_bytes::<32>());
    let tick_bytes = (-887272i32).to_be_bytes(); // Uniswap V3 MIN_TICK
    data_v3_max[156..160].copy_from_slice(&tick_bytes);

    assert!(
        process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV3_SWAP], &data_v3_max, &publisher),
        "V3 swap with u128::MAX liquidity must be accepted"
    );
    match rx.try_recv().expect("Receive tick update") {
        MarketUpdate::TickUpdate { sqrt_price_x96, current_tick, liquidity, .. } => {
            assert_eq!(sqrt_price_x96, sqrt_p);
            assert_eq!(liquidity, u128::MAX);
            assert_eq!(current_tick, -887272);
        }
        _ => panic!("Expected TickUpdate"),
    }

    // 5. Boundary: V3 liquidity = u128::MAX + 1
    let mut data_v3_overflow = [0u8; 160];
    data_v3_overflow[64..96].copy_from_slice(&sqrt_p.to_be_bytes::<32>());
    data_v3_overflow[96..128].copy_from_slice(&u128_max_plus_1.to_be_bytes::<32>());
    let res = catch_unwind(AssertUnwindSafe(|| {
        process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV3_SWAP], &data_v3_overflow, &publisher)
    }));
    assert!(res.is_ok(), "V3 liquidity overflow must not panic");
    assert!(!res.unwrap(), "V3 liquidity overflow must return false");

    // 6. Extreme tick boundaries: MAX_TICK (887272), i32::MIN, i32::MAX
    for test_tick in [887272i32, -887272i32, i32::MIN, i32::MAX, 0i32] {
        let mut d = [0u8; 160];
        d[64..96].copy_from_slice(&U256::from(999u64).to_be_bytes::<32>());
        d[96..128].copy_from_slice(&U256::from(500u64).to_be_bytes::<32>());
        d[156..160].copy_from_slice(&test_tick.to_be_bytes());

        assert!(process_dex_log(Chain::Base, pool, &[TOPIC_PANCAKE_V3_SWAP], &d, &publisher));
        match rx.try_recv().expect("Receive tick update") {
            MarketUpdate::TickUpdate { current_tick, .. } => {
                assert_eq!(current_tick, test_tick, "Tick mismatch for {}", test_tick);
            }
            _ => panic!("Expected TickUpdate"),
        }
    }
}

// ==============================================================================
// 2. Rapid Connection Drop & Fallback Resilience
// ==============================================================================

#[tokio::test]
async fn test_empirical_r2_rapid_fallback_cycling_under_stress() {
    let (shutdown_tx, _) = broadcast::channel::<()>(8);
    let publisher = MarketFeedPublisher::new(64);
    let mut consumer_rx = publisher.subscribe();

    // Fallback Mock Server on dynamic port
    let fb_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fb_port = fb_listener.local_addr().unwrap().port();
    let fb_url = format!("ws://127.0.0.1:{fb_port}");

    let mut fb_shutdown = shutdown_tx.subscribe();
    let target_pool = Address::repeat_byte(0x88);

    let fb_server = tokio::spawn(async move {
        tokio::select! {
            _ = fb_shutdown.recv() => {}
            res = fb_listener.accept() => {
                if let Ok((socket, _)) = res {
                    if let Ok(mut ws) = tokio_tungstenite::accept_async(socket).await {
                        // Read subscription request
                        if let Some(Ok(Message::Text(sub))) = ws.next().await {
                            assert!(sub.contains("eth_subscribe"));
                        }
                        // Emit a valid V2 Sync log frame
                        let mut data_bytes = [0u8; 64];
                        data_bytes[0..32].copy_from_slice(&U256::from(777_000u64).to_be_bytes::<32>());
                        data_bytes[32..64].copy_from_slice(&U256::from(888_000u64).to_be_bytes::<32>());
                        let data_hex = format!("0x{}", alloy_primitives::hex::encode(data_bytes));

                        let log_msg = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "eth_subscription",
                            "params": {
                                "subscription": "0xsub_fb",
                                "result": {
                                    "address": format!("0x{:x}", target_pool),
                                    "topics": [format!("0x{:x}", TOPIC_UNIV2_SYNC)],
                                    "data": data_hex
                                }
                            }
                        });
                        let _ = ws.send(Message::Text(log_msg.to_string().into())).await;

                        // Wait for shutdown
                        let _ = fb_shutdown.recv().await;
                    }
                }
            }
        }
    });

    // Primary endpoint: invalid port that refuses connections immediately
    let unreachable_primary = "ws://127.0.0.1:9";
    let handler = BaseLogStreamHandler::new(unreachable_primary, Some(fb_url.clone()));
    let client_shutdown = shutdown_tx.subscribe();
    let pub_clone = publisher.clone();

    let client_handle = tokio::spawn(async move {
        let _ = handler.run(pub_clone, client_shutdown).await;
    });

    // Verify client automatically falls back and receives message
    let received = tokio::time::timeout(Duration::from_secs(3), async {
        while let Ok(update) = consumer_rx.recv().await {
            if let MarketUpdate::ReserveUpdate { pool_id, reserve0, reserve1, .. } = update {
                if pool_id == target_pool {
                    assert_eq!(reserve0, 777_000);
                    assert_eq!(reserve1, 888_000);
                    return true;
                }
            }
        }
        false
    }).await;

    assert!(
        received.is_ok() && received.unwrap(),
        "BaseLogStreamHandler failed to receive frame from fallback server"
    );

    let _ = shutdown_tx.send(());
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        let _ = client_handle.await;
        let _ = fb_server.await;
    }).await;
}

// ==============================================================================
// 3. High-Concurrency Race Condition and Channel Saturation Stress
// ==============================================================================

#[tokio::test]
async fn test_empirical_r2_high_concurrency_race_and_saturation() {
    let publisher = MarketFeedPublisher::new(128); // Deliberately constrained buffer
    let mut rx_active = publisher.subscribe();
    let mut rx_slow = publisher.subscribe(); // Deliberately slow subscriber

    let total_tasks = 16;
    let iterations_per_task = 500;
    let mut handles = Vec::new();

    // Spawn 16 concurrent producers hammering process_dex_log with interleaved valid & corrupt frames
    for task_id in 0..total_tasks {
        let pub_clone = publisher.clone();
        handles.push(tokio::spawn(async move {
            let pool = Address::repeat_byte((task_id as u8).wrapping_add(1));
            for i in 0..iterations_per_task {
                if i % 3 == 0 {
                    // Valid V2 sync
                    let mut data = [0u8; 64];
                    data[0..32].copy_from_slice(&U256::from(i as u64).to_be_bytes::<32>());
                    data[32..64].copy_from_slice(&U256::from((i * 2) as u64).to_be_bytes::<32>());
                    let _ = process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &data, &pub_clone);
                } else if i % 3 == 1 {
                    // Valid V3 swap
                    let mut data = [0u8; 160];
                    data[64..96].copy_from_slice(&U256::from(1000u64).to_be_bytes::<32>());
                    data[96..128].copy_from_slice(&U256::from(5000u64).to_be_bytes::<32>());
                    data[156..160].copy_from_slice(&(-20i32).to_be_bytes());
                    let _ = process_dex_log(Chain::Base, pool, &[TOPIC_UNIV3_SWAP], &data, &pub_clone);
                } else {
                    // Adversarial frame: reserve0 overflow
                    let data = [0xffu8; 64];
                    let _ = process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &data, &pub_clone);
                }
            }
        }));
    }

    // Drain active receiver while tasks are producing
    let drain_handle = tokio::spawn(async move {
        let mut count = 0;
        let start = tokio::time::Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            match tokio::time::timeout(Duration::from_millis(100), rx_active.recv()).await {
                Ok(Ok(_)) => count += 1,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    // Lag is normal and expected when channel saturates
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => break,
                Err(_) => {
                    // 100ms timeout with no new items, check if we received enough
                    if count > 0 {
                        break;
                    }
                }
            }
            if count >= (total_tasks * iterations_per_task * 2 / 3) / 2 {
                break;
            }
        }
        count
    });

    // Wait for all producer tasks to complete cleanly without panic or deadlock
    for h in handles {
        let res = h.await;
        assert!(res.is_ok(), "Producer task panicked!");
    }

    let received_count = drain_handle.await.expect("Drain task completed");
    assert!(received_count > 0, "Active subscriber must have processed updates");

    // Check slow subscriber lagged state
    let slow_res = rx_slow.recv().await;
    assert!(
        matches!(slow_res, Err(broadcast::error::RecvError::Lagged(_)) | Ok(_)),
        "Slow subscriber should either lag or receive updates without crashing"
    );
}

// ==============================================================================
// 4. Handshake Header Verification & HTTP Status Code Resilience
// ==============================================================================

#[tokio::test]
async fn test_empirical_r2_header_verification_and_http_status_codes() {
    // 1. Verify exact header transmission: Arbitrum-Feed-Client-Version: 2
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let feed_url = format!("ws://127.0.0.1:{port}/feed");

    let server_task = tokio::spawn(async move {
        if let Ok((socket, _)) = listener.accept().await {
            let callback = |req: &Request, resp: Response| {
                let header = req.headers().get("Arbitrum-Feed-Client-Version");
                assert!(header.is_some(), "Header must be present");
                assert_eq!(header.unwrap().to_str().unwrap(), "2");
                Ok(resp)
            };
            let _ = tokio_tungstenite::accept_hdr_async(socket, callback).await;
        }
    });

    let handler = ArbitrumFeedHandler::new(&feed_url);
    let connect_res = handler.connect(None).await;
    assert!(connect_res.is_ok(), "Handshake should succeed with mock server");
    let _ = server_task.await;

    // 2. Verify server rejecting handshake with HTTP 429 / 500 does NOT panic handler
    let listener_err = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port_err = listener_err.local_addr().unwrap().port();
    let feed_url_err = format!("ws://127.0.0.1:{port_err}/feed");

    let server_err_task = tokio::spawn(async move {
        if let Ok((mut socket, _)) = listener_err.accept().await {
            use tokio::io::AsyncWriteExt;
            let response = "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n";
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });

    let handler_err = ArbitrumFeedHandler::new(&feed_url_err);
    use futures::FutureExt;
    let res = AssertUnwindSafe(handler_err.connect(None)).catch_unwind().await;
    assert!(res.is_ok(), "Handshake error must not panic");
    assert!(res.unwrap().is_err(), "HTTP 429 must return Err");
    let _ = server_err_task.await;
}

// ==============================================================================
// 5. Base Flashblock Lifecycle & Interleaved Log Processing
// ==============================================================================

#[test]
fn test_empirical_r2_base_flashblock_lifecycle_and_gas_reset() {
    let gas_tracker = Arc::new(Mutex::new(FlashblockGasTracker::new(DEFAULT_BASE_GAS_LIMIT)));
    let publisher = MarketFeedPublisher::new(64);
    let mut rx = publisher.subscribe();

    // Simulate 10 sub-blocks: index 0 to 9
    for sub_idx in 0..10 {
        let fb = FlashblockMessage {
            block_number: 1_000_000,
            sub_block_index: sub_idx,
            gas_used: 1_000_000,
            gas_limit: 30_000_000,
            base_fee: U256::ZERO,
            transactions: vec![],
        };

        process_flashblock(&fb, &gas_tracker, &publisher);

        let update = rx.try_recv().expect("Receive FlashblockStep");
        match update {
            MarketUpdate::FlashblockStep { sub_block_index, cumulative_gas_spent, .. } => {
                assert_eq!(sub_block_index, sub_idx);
                if sub_idx < 9 {
                    assert_eq!(cumulative_gas_spent, (sub_idx as u64 + 1) * 1_000_000);
                }
            }
            _ => panic!("Expected FlashblockStep"),
        }
    }

    // At index 9, gas_tracker should be reset
    let tracker = gas_tracker.lock().clone();
    assert_eq!(
        tracker.cumulative_gas_spent, 0,
        "Gas tracker cumulative_gas_spent must reset to 0 after sub-block 9"
    );
    assert_eq!(
        tracker.current_sub_block_index, 0,
        "Gas tracker current_sub_block_index must reset to 0 after sub-block 9"
    );
}
