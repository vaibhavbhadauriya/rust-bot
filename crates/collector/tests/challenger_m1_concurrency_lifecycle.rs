//! Empirical Stress & Concurrency Lifecycle Test Suite for Milestone 1 (R1)
//! Author: M1 Stream Challenger 2 (teamwork_challenger)
//!
//! Scope:
//! 1. Concurrent lifecycle: Arbitrum and Base collectors running concurrently without port/channel conflicts or panics.
//! 2. Fallback mechanisms: ARBITRUM_FALLBACK_RPC_WS and BASE_FALLBACK_RPC_WS endpoint validity and connectability.
//! 3. Handshake compliance: Arbitrum-Feed-Client-Version: 2 header generation on WebSocket connect.
//! 4. Adversarial JSON-RPC deserialization stress: unexpected JSON payloads, missing fields, nulls, empty strings, zero panics.
//! 5. Bug reproduction harness: Uint conversion overflow panics on untrusted event logs.

use alloy_primitives::{Address, B256, U256};
use flash_collector::arbitrum::{
    ArbitrumFeedHandler, ArbitrumLogStreamHandler, TOPIC_UNIV2_SYNC as ARB_TOPIC_V2_SYNC,
};
use flash_collector::base::{
    process_dex_log, process_json_rpc_log, BaseFlashblockHandler, BaseLogStreamHandler,
    DEFAULT_BASE_GAS_LIMIT, TOPIC_PANCAKE_V3_SWAP, TOPIC_UNIV2_SYNC, TOPIC_UNIV3_SWAP,
};
use flash_collector::feed::{MarketFeedPublisher, MarketUpdate};
use flash_core::constants::{ARBITRUM_FALLBACK_RPC_WS, BASE_FALLBACK_RPC_WS};
use flash_core::types::Chain;
use futures::{SinkExt, StreamExt};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::Message;

// ==============================================================================
// 1. Header Generation Test: Arbitrum-Feed-Client-Version: 2
// ==============================================================================

#[tokio::test]
async fn test_empirical_arbitrum_feed_client_version_header() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("Bind mock server");
    let local_addr = listener.local_addr().expect("Local address");
    let ws_url = format!("ws://127.0.0.1:{}/feed", local_addr.port());

    let captured_version = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let captured_clone = Arc::clone(&captured_version);

    let server_handle = tokio::spawn(async move {
        if let Ok((socket, _)) = listener.accept().await {
            let callback = |req: &Request, resp: Response| {
                if let Some(val) = req.headers().get("arbitrum-feed-client-version") {
                    if let Ok(val_str) = val.to_str() {
                        let mut cap = captured_clone.try_lock().expect("Lock captured_version");
                        *cap = Some(val_str.to_string());
                    }
                }
                Ok(resp)
            };

            let _ = tokio_tungstenite::accept_hdr_async(socket, callback).await;
        }
    });

    let handler = ArbitrumFeedHandler::new(&ws_url);
    let connect_res = handler.connect(None).await;
    assert!(connect_res.is_ok(), "Handshake with mock server failed: {:?}", connect_res.err());

    let _ = server_handle.await;

    let version = captured_version.lock().await.clone();
    assert!(
        version.is_some(),
        "Arbitrum-Feed-Client-Version header was NOT transmitted during handshake"
    );
    assert_eq!(
        version.as_deref(),
        Some("2"),
        "Arbitrum-Feed-Client-Version header value must be exactly '2'"
    );
}

// ==============================================================================
// 2. Fallback Endpoints Syntax & Connectability Verification
// ==============================================================================

#[test]
fn test_empirical_fallback_rpc_ws_constants_integrity() {
    // 1. Verify exact string constants
    assert_eq!(
        ARBITRUM_FALLBACK_RPC_WS,
        "wss://arbitrum-one-rpc.publicnode.com"
    );
    assert_eq!(
        BASE_FALLBACK_RPC_WS,
        "wss://base-rpc.publicnode.com"
    );

    // 2. Verify submodule exports
    assert_eq!(
        flash_core::constants::arbitrum::FALLBACK_RPC_WS,
        ARBITRUM_FALLBACK_RPC_WS
    );
    assert_eq!(
        flash_core::constants::arbitrum::ARBITRUM_FALLBACK_RPC_WS,
        ARBITRUM_FALLBACK_RPC_WS
    );
    assert_eq!(
        flash_core::constants::base::FALLBACK_RPC_WS,
        BASE_FALLBACK_RPC_WS
    );
    assert_eq!(
        flash_core::constants::base::BASE_FALLBACK_RPC_WS,
        BASE_FALLBACK_RPC_WS
    );

    // 3. Syntax parsing check: IntoClientRequest
    for endpoint in [ARBITRUM_FALLBACK_RPC_WS, BASE_FALLBACK_RPC_WS] {
        assert!(
            endpoint.starts_with("wss://"),
            "Endpoint must use secure wss:// scheme: {}",
            endpoint
        );
        assert!(
            endpoint.contains(".publicnode.com"),
            "Endpoint must target publicnode.com: {}",
            endpoint
        );

        let req = endpoint.into_client_request();
        assert!(
            req.is_ok(),
            "Fallback endpoint {} must be a syntactically valid WebSocket request: {:?}",
            endpoint,
            req.err()
        );
        let uri = req.unwrap().uri().clone();
        assert_eq!(uri.scheme_str(), Some("wss"));
        assert!(uri.host().unwrap_or("").contains("publicnode.com"));
    }
}

#[tokio::test]
async fn test_empirical_fallback_endpoints_live_connectability_probe() {
    // Probe TCP connectivity to both fallback endpoints on port 443 with 5-second timeout
    for host in ["arbitrum-one-rpc.publicnode.com", "base-rpc.publicnode.com"] {
        let addr = format!("{}:443", host);
        let connect_fut = tokio::net::TcpStream::connect(&addr);
        match tokio::time::timeout(Duration::from_secs(5), connect_fut).await {
            Ok(Ok(_stream)) => {
                // Succeeded!
            }
            Ok(Err(e)) => {
                panic!("Failed to connect to fallback RPC endpoint {}: {:?}", host, e);
            }
            Err(_) => {
                panic!("Connection to fallback RPC endpoint {} timed out (5s)", host);
            }
        }
    }
}

// ==============================================================================
// 3. Concurrent 4-Collector Lifecycle & Port/Channel Conflict Test
// ==============================================================================

#[tokio::test]
async fn test_empirical_concurrent_4_collector_lifecycle_and_clean_shutdown() {
    // Set up 4 local mock WebSocket servers on dynamically allocated ports
    let s1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let s2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let s3 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let s4 = TcpListener::bind("127.0.0.1:0").await.unwrap();

    let p1 = s1.local_addr().unwrap().port();
    let p2 = s2.local_addr().unwrap().port();
    let p3 = s3.local_addr().unwrap().port();
    let p4 = s4.local_addr().unwrap().port();

    let (shutdown_tx, _) = broadcast::channel::<()>(16);
    let publisher = MarketFeedPublisher::new(4096);
    let mut consumer_rx = publisher.subscribe();

    let pool_arb = Address::repeat_byte(0xaa);
    let pool_base = Address::repeat_byte(0xbb);

    // Mock Server 1: Arbitrum Sequencer Feed
    let mut s1_shutdown = shutdown_tx.subscribe();
    let s1_task = tokio::spawn(async move {
        tokio::select! {
            _ = s1_shutdown.recv() => {}
            res = s1.accept() => {
                if let Ok((stream, _)) = res {
                    if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                        let mut msg_bytes = vec![0x03]; // kind 0x03
                        let dummy_tx = [
                            0xf8, 0x48, 0x80, 0x80, 0x80, 0x94,
                            0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa,
                            0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa,
                            0x80, 0x88, 0x02, 0x2c, 0x0d, 0x9f, 0x01, 0x02, 0x03, 0x04,
                            0x1b, 0x80, 0x80
                        ];
                        msg_bytes.extend_from_slice(&dummy_tx);

                        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &msg_bytes);
                        let json = serde_json::json!({
                            "version": 1,
                            "messages": [{
                                "sequenceNumber": 5001,
                                "message": {
                                    "message": {
                                        "l2msg": b64
                                    }
                                }
                            }]
                        });
                        let _ = ws.send(Message::Text(json.to_string().into())).await;
                        let _ = s1_shutdown.recv().await;
                    }
                }
            }
        }
    });

    // Mock Server 2: Arbitrum Alchemy WS Logs
    let mut s2_shutdown = shutdown_tx.subscribe();
    let s2_task = tokio::spawn(async move {
        tokio::select! {
            _ = s2_shutdown.recv() => {}
            res = s2.accept() => {
                if let Ok((stream, _)) = res {
                    if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                        let _ = ws.next().await;
                        let mut data_bytes = [0u8; 64];
                        data_bytes[0..32].copy_from_slice(&U256::from(100_000u64).to_be_bytes::<32>());
                        data_bytes[32..64].copy_from_slice(&U256::from(200_000u64).to_be_bytes::<32>());
                        let data_hex = format!("0x{}", alloy_primitives::hex::encode(data_bytes));

                        let log_json = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "eth_subscription",
                            "params": {
                                "subscription": "0xsub1",
                                "result": {
                                    "address": format!("0x{:x}", pool_arb),
                                    "topics": [format!("0x{:x}", ARB_TOPIC_V2_SYNC)],
                                    "data": data_hex
                                }
                            }
                        });
                        let _ = ws.send(Message::Text(log_json.to_string().into())).await;
                        let _ = s2_shutdown.recv().await;
                    }
                }
            }
        }
    });

    // Mock Server 3: Base Flashblocks Feed
    let mut s3_shutdown = shutdown_tx.subscribe();
    let s3_task = tokio::spawn(async move {
        tokio::select! {
            _ = s3_shutdown.recv() => {}
            res = s3.accept() => {
                if let Ok((stream, _)) = res {
                    if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                        let fb_json = serde_json::json!({
                            "blockNumber": 123456,
                            "index": 1,
                            "gasUsed": 500_000,
                            "transactions": []
                        });
                        let _ = ws.send(Message::Text(fb_json.to_string().into())).await;
                        let _ = s3_shutdown.recv().await;
                    }
                }
            }
        }
    });

    // Mock Server 4: Base Alchemy WS Logs
    let mut s4_shutdown = shutdown_tx.subscribe();
    let s4_task = tokio::spawn(async move {
        tokio::select! {
            _ = s4_shutdown.recv() => {}
            res = s4.accept() => {
                if let Ok((stream, _)) = res {
                    if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                        let _ = ws.next().await;
                        let mut data_bytes = [0u8; 160];
                        data_bytes[64..96].copy_from_slice(&U256::from(5555u64).to_be_bytes::<32>());
                        data_bytes[96..128].copy_from_slice(&U256::from(8888u64).to_be_bytes::<32>());
                        data_bytes[156..160].copy_from_slice(&(-10i32).to_be_bytes());
                        let data_hex = format!("0x{}", alloy_primitives::hex::encode(data_bytes));

                        let log_json = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "eth_subscription",
                            "params": {
                                "subscription": "0xsub2",
                                "result": {
                                    "address": format!("0x{:x}", pool_base),
                                    "topics": [format!("0x{:x}", TOPIC_PANCAKE_V3_SWAP)],
                                    "data": data_hex
                                }
                            }
                        });
                        let _ = ws.send(Message::Text(log_json.to_string().into())).await;
                        let _ = s4_shutdown.recv().await;
                    }
                }
            }
        }
    });

    // Instantiate all 4 collectors concurrently
    let arb_feed = ArbitrumFeedHandler::new(format!("ws://127.0.0.1:{p1}"));
    let arb_logs = ArbitrumLogStreamHandler::new(format!("ws://127.0.0.1:{p2}"), None);
    let base_fb = BaseFlashblockHandler::new(format!("ws://127.0.0.1:{p3}"), DEFAULT_BASE_GAS_LIMIT);
    let base_logs = BaseLogStreamHandler::new(format!("ws://127.0.0.1:{p4}"), None);

    let pub1 = publisher.clone();
    let rx1 = shutdown_tx.subscribe();
    let c1_handle = tokio::spawn(async move { arb_feed.run(pub1, None, rx1).await });

    let pub2 = publisher.clone();
    let rx2 = shutdown_tx.subscribe();
    let c2_handle = tokio::spawn(async move { arb_logs.run(pub2, rx2).await });

    let pub3 = publisher.clone();
    let rx3 = shutdown_tx.subscribe();
    let c3_handle = tokio::spawn(async move { base_fb.run(pub3, None, rx3).await });

    let pub4 = publisher.clone();
    let rx4 = shutdown_tx.subscribe();
    let c4_handle = tokio::spawn(async move { base_logs.run(pub4, rx4).await });

    // Collect updates from consumer receiver
    let mut got_arb_reserve = false;
    let mut got_base_fb = false;
    let mut got_base_tick = false;

    let timeout_duration = Duration::from_secs(3);
    let start = tokio::time::Instant::now();

    while (!got_arb_reserve || !got_base_fb || !got_base_tick) && start.elapsed() < timeout_duration {
        if let Ok(Ok(update)) = tokio::time::timeout(Duration::from_millis(500), consumer_rx.recv()).await {
            match update {
                MarketUpdate::ReserveUpdate { chain, pool_id, reserve0, reserve1, .. } => {
                    if chain == Chain::ArbitrumOne && pool_id == pool_arb {
                        assert_eq!(reserve0, 100_000);
                        assert_eq!(reserve1, 200_000);
                        got_arb_reserve = true;
                    }
                }
                MarketUpdate::FlashblockStep { block_number, sub_block_index, .. } => {
                    if block_number == 123456 && sub_block_index == 1 {
                        got_base_fb = true;
                    }
                }
                MarketUpdate::TickUpdate { chain, pool_id, current_tick, liquidity, .. } => {
                    if chain == Chain::Base && pool_id == pool_base {
                        assert_eq!(current_tick, -10);
                        assert_eq!(liquidity, 8888);
                        got_base_tick = true;
                    }
                }
                _ => {}
            }
        }
    }

    assert!(got_arb_reserve, "Did not receive Arbitrum One ReserveUpdate");
    assert!(got_base_fb, "Did not receive Base FlashblockStep");
    assert!(got_base_tick, "Did not receive Base TickUpdate");

    // Send broadcast shutdown signal
    let _ = shutdown_tx.send(());

    // Verify all 4 collectors join cleanly within 2 seconds
    let join_all = async {
        let _ = c1_handle.await;
        let _ = c2_handle.await;
        let _ = c3_handle.await;
        let _ = c4_handle.await;
        let _ = s1_task.await;
        let _ = s2_task.await;
        let _ = s3_task.await;
        let _ = s4_task.await;
    };

    tokio::time::timeout(Duration::from_secs(2), join_all)
        .await
        .expect("All concurrent collectors and mock servers must gracefully shut down within 2 seconds");
}

// ==============================================================================
// 4. Automatic Fallback Mechanism Test
// ==============================================================================

#[tokio::test]
async fn test_empirical_fallback_reconnection_when_primary_unavailable() {
    let fb_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fb_port = fb_listener.local_addr().unwrap().port();
    let fb_url = format!("ws://127.0.0.1:{fb_port}");

    let fb_connected = Arc::new(tokio::sync::Mutex::new(false));
    let fb_flag = Arc::clone(&fb_connected);

    let (shutdown_tx, _) = broadcast::channel::<()>(8);
    let mut s_rx = shutdown_tx.subscribe();

    let fb_server = tokio::spawn(async move {
        tokio::select! {
            _ = s_rx.recv() => {}
            res = fb_listener.accept() => {
                if let Ok((stream, _)) = res {
                    if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                        *fb_flag.lock().await = true;
                        if let Some(Ok(Message::Text(sub_msg))) = ws.next().await {
                            assert!(sub_msg.contains("eth_subscribe"));
                        }
                        let _ = s_rx.recv().await;
                    }
                }
            }
        }
    });

    let bad_primary = "ws://127.0.0.1:1";
    let publisher = MarketFeedPublisher::new(16);
    let handler = ArbitrumLogStreamHandler::new(bad_primary, Some(fb_url.clone()));
    let rx = shutdown_tx.subscribe();

    let client_handle = tokio::spawn(async move {
        let _ = handler.run(publisher, rx).await;
    });

    let start = tokio::time::Instant::now();
    while !*fb_connected.lock().await && start.elapsed() < Duration::from_secs(3) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        *fb_connected.lock().await,
        "ArbitrumLogStreamHandler failed to automatically failover to fallback URL"
    );

    let _ = shutdown_tx.send(());
    let _ = client_handle.await;
    let _ = fb_server.await;
}

#[tokio::test]
async fn test_empirical_base_fallback_reconnection() {
    let fb_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fb_port = fb_listener.local_addr().unwrap().port();
    let fb_url = format!("ws://127.0.0.1:{fb_port}");

    let fb_connected = Arc::new(tokio::sync::Mutex::new(false));
    let fb_flag = Arc::clone(&fb_connected);

    let (shutdown_tx, _) = broadcast::channel::<()>(8);
    let mut s_rx = shutdown_tx.subscribe();

    let fb_server = tokio::spawn(async move {
        tokio::select! {
            _ = s_rx.recv() => {}
            res = fb_listener.accept() => {
                if let Ok((stream, _)) = res {
                    if let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await {
                        *fb_flag.lock().await = true;
                        if let Some(Ok(Message::Text(sub_msg))) = ws.next().await {
                            assert!(sub_msg.contains("eth_subscribe"));
                        }
                        let _ = s_rx.recv().await;
                    }
                }
            }
        }
    });

    let bad_primary = "ws://127.0.0.1:1";
    let publisher = MarketFeedPublisher::new(16);
    let handler = BaseLogStreamHandler::new(bad_primary, Some(fb_url.clone()));
    let rx = shutdown_tx.subscribe();

    let client_handle = tokio::spawn(async move {
        let _ = handler.run(publisher, rx).await;
    });

    let start = tokio::time::Instant::now();
    while !*fb_connected.lock().await && start.elapsed() < Duration::from_secs(3) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        *fb_connected.lock().await,
        "BaseLogStreamHandler failed to automatically failover to fallback URL"
    );

    let _ = shutdown_tx.send(());
    let _ = client_handle.await;
    let _ = fb_server.await;
}

// ==============================================================================
// 5. Stress Test JSON-RPC Log Message Deserialization (Hostile / Corrupt Inputs)
// ==============================================================================

#[test]
fn test_empirical_json_rpc_deserialization_stress_and_fuzz() {
    let publisher = MarketFeedPublisher::new(64);
    let chains = [Chain::ArbitrumOne, Chain::Base];

    let hostile_payloads = vec![
        // 1. Primitive nulls and literals
        serde_json::Value::Null,
        serde_json::json!(true),
        serde_json::json!(false),
        serde_json::json!(0),
        serde_json::json!(-1),
        serde_json::json!(123456789),
        serde_json::json!(3.1415926535),
        serde_json::json!(""),
        serde_json::json!("random text without json structure"),
        serde_json::json!([]),
        serde_json::json!([null, null, null]),
        serde_json::json!([1, 2, 3]),

        // 2. Empty or partial objects
        serde_json::json!({}),
        serde_json::json!({"jsonrpc": "2.0"}),
        serde_json::json!({"id": 1, "result": null}),
        serde_json::json!({"method": "eth_subscription"}),
        serde_json::json!({"method": "eth_subscription", "params": null}),
        serde_json::json!({"method": "eth_subscription", "params": {}}),
        serde_json::json!({"method": "eth_subscription", "params": {"result": null}}),
        serde_json::json!({"method": "eth_subscription", "params": {"result": {}}}),

        // 3. Null fields in result
        serde_json::json!({
            "method": "eth_subscription",
            "params": {
                "result": {
                    "address": null,
                    "topics": null,
                    "data": null
                }
            }
        }),

        // 4. Corrupt field types in result
        serde_json::json!({
            "method": "eth_subscription",
            "params": {
                "result": {
                    "address": 12345,
                    "topics": "not an array",
                    "data": true
                }
            }
        }),
        serde_json::json!({
            "method": "eth_subscription",
            "params": {
                "result": {
                    "address": [1, 2, 3],
                    "topics": [null, 123, "not_a_hash", {}],
                    "data": []
                }
            }
        }),

        // 5. Malformed hex strings
        serde_json::json!({
            "address": "0x",
            "topics": ["0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"],
            "data": "0x"
        }),
        serde_json::json!({
            "address": "0x123",
            "topics": ["0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"],
            "data": "0x123"
        }),
        serde_json::json!({
            "address": "0xGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGGG",
            "topics": ["0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"],
            "data": "0xZZZZ"
        }),
        serde_json::json!({
            "address": "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
            "topics": ["0xnot_a_valid_b256_hash"],
            "data": "0x00"
        }),

        // 6. Truncated data for V2 Sync (needs 64 bytes)
        serde_json::json!({
            "address": "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
            "topics": ["0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"],
            "data": "0x0000000000000000000000000000000000000000000000000000000000000001"
        }),

        // 7. Truncated data for V3 Swap (needs 160 bytes)
        serde_json::json!({
            "address": "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
            "topics": ["0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67"],
            "data": "0x0000000000000000000000000000000000000000000000000000000000000001"
        }),

        // 8. Truncated data for PancakeSwap V3 (needs 160 bytes)
        serde_json::json!({
            "address": "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
            "topics": ["0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83"],
            "data": "0x00000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000002"
        }),

        // 9. Valid length payload with normal values
        serde_json::json!({
            "address": "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1",
            "topics": ["0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"],
            "data": "0x00000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000002"
        }),

        // 10. Deeply nested JSON injection
        serde_json::json!({
            "method": "eth_subscription",
            "params": {
                "result": {
                    "method": "eth_subscription",
                    "params": {
                        "result": null
                    }
                }
            }
        }),
    ];

    for chain in chains {
        for (idx, payload) in hostile_payloads.iter().enumerate() {
            let p_ref = &publisher;
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_json_rpc_log(chain, payload, p_ref)
            }));
            assert!(
                res.is_ok(),
                "Panic detected on chain {:?} for hostile payload index {}: {:?}",
                chain,
                idx,
                payload
            );
            if idx != hostile_payloads.len() - 2 {
                // All clearly corrupt payloads must return false
                assert!(
                    !res.unwrap(),
                    "Hostile payload index {} should have returned false but returned true",
                    idx
                );
            } else {
                // Valid payload should successfully parse
                assert!(
                    res.unwrap(),
                    "Valid payload index {} should have returned true but returned false",
                    idx
                );
            }
        }
    }
}

// ==============================================================================
// 6. Bug Reproduction Harness: U256::to::<u128> Overflow Panic
// ==============================================================================

#[test]
fn test_empirical_reproduce_uint_overflow_panic_in_process_dex_log() {
    let publisher = MarketFeedPublisher::new(16);
    let pool = Address::repeat_byte(0x55);

    // Scenario A: Malicious V2 Sync log with reserve0 > u128::MAX (e.g. U256::MAX)
    // In Solidity: emit Sync(type(uint256).max, 100);
    let mut data_overflow_r0 = [0u8; 64];
    data_overflow_r0[0..32].copy_from_slice(&[0xff; 32]); // U256::MAX > u128::MAX
    data_overflow_r0[32..64].copy_from_slice(&U256::from(100u64).to_be_bytes::<32>());

    let p_ref = &publisher;
    let res_r0 = catch_unwind(AssertUnwindSafe(|| {
        process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV2_SYNC], &data_overflow_r0, p_ref)
    }));
    assert!(
        res_r0.is_ok(),
        "reserve0 > u128::MAX must not panic via U256::to::<u128>()!"
    );
    assert!(
        !res_r0.unwrap(),
        "reserve0 > u128::MAX must return false gracefully!"
    );

    // Scenario B: Malicious V2 Sync log with reserve1 > u128::MAX
    let mut data_overflow_r1 = [0u8; 64];
    data_overflow_r1[0..32].copy_from_slice(&U256::from(100u64).to_be_bytes::<32>());
    data_overflow_r1[32..64].copy_from_slice(&[0xff; 32]); // U256::MAX > u128::MAX

    let res_r1 = catch_unwind(AssertUnwindSafe(|| {
        process_dex_log(Chain::Base, pool, &[TOPIC_UNIV2_SYNC], &data_overflow_r1, p_ref)
    }));
    assert!(
        res_r1.is_ok(),
        "reserve1 > u128::MAX must not panic via U256::to::<u128>()!"
    );
    assert!(
        !res_r1.unwrap(),
        "reserve1 > u128::MAX must return false gracefully!"
    );

    // Scenario C: Malicious V3 Swap log with liquidity > u128::MAX
    let mut data_overflow_liq = [0u8; 160];
    data_overflow_liq[64..96].copy_from_slice(&U256::from(1000u64).to_be_bytes::<32>());
    data_overflow_liq[96..128].copy_from_slice(&[0xff; 32]); // liquidity > u128::MAX

    let res_liq = catch_unwind(AssertUnwindSafe(|| {
        process_dex_log(Chain::ArbitrumOne, pool, &[TOPIC_UNIV3_SWAP], &data_overflow_liq, p_ref)
    }));
    assert!(
        res_liq.is_ok(),
        "liquidity > u128::MAX must not panic via U256::to::<u128>()!"
    );
    assert!(
        !res_liq.unwrap(),
        "liquidity > u128::MAX must return false gracefully!"
    );
}

#[test]
fn test_empirical_base_flashblock_message_text_fuzz() {
    let handler = BaseFlashblockHandler::new("ws://127.0.0.1:8545", DEFAULT_BASE_GAS_LIMIT);
    let publisher = MarketFeedPublisher::new(16);

    let bad_texts = [
        "",
        " ",
        "\0\0\0",
        "not json",
        "<!DOCTYPE html><html><body>502 Bad Gateway</body></html>",
        "{\"jsonrpc\": \"2.0\", \"error\": {\"code\": -32000, \"message\": \"rate limit exceeded\"}}",
        "{\"result\": null}",
        "{\"params\": {\"result\": null}}",
        "{\"blockNumber\": \"not a number\"}",
        "{\"transactions\": [\"0xinvalidhex\"]}",
        "null",
        "[]",
        "{}",
    ];

    for text in bad_texts {
        let p_ref = &publisher;
        let h_ref = &handler;
        let res = catch_unwind(AssertUnwindSafe(|| {
            h_ref.process_message_text(text, p_ref)
        }));
        assert!(res.is_ok(), "Panic on text: {:?}", text);
        let ret = res.unwrap();
        assert!(ret.is_ok(), "Error on text: {:?}", text);
        assert!(!ret.unwrap(), "Expected false for invalid input: {:?}", text);
    }
}

// ==============================================================================
// 7. Exhaustive Boundary Stress Matrix: u128::MAX, u128::MAX + 1, U256::MAX, Zeros, Truncated
// ==============================================================================

#[test]
fn test_empirical_exhaustive_u128_overflow_and_boundary_matrix() {
    let pool = Address::repeat_byte(0x77);
    let chains = [Chain::ArbitrumOne, Chain::Base];

    let val_zero = U256::ZERO;
    let val_one = U256::from(1u64);
    let val_u128_max = U256::from(u128::MAX);
    let val_u128_max_plus_1 = val_u128_max + val_one;
    let val_u256_max = U256::MAX;

    // Verify mathematical invariants of test fixtures
    assert_eq!(u128::try_from(val_u128_max).unwrap(), u128::MAX);
    assert!(u128::try_from(val_u128_max_plus_1).is_err());
    assert!(u128::try_from(val_u256_max).is_err());

    for chain in chains {
        // ----------------------------------------------------------------------
        // Case 1: Exact boundary u128::MAX must SUCCEED gracefully without panic
        // ----------------------------------------------------------------------
        {
            let publisher = MarketFeedPublisher::new(16);
            let mut rx = publisher.subscribe();

            // V2 Sync: reserve0 = u128::MAX, reserve1 = 0
            let mut data = [0u8; 64];
            data[0..32].copy_from_slice(&val_u128_max.to_be_bytes::<32>());
            data[32..64].copy_from_slice(&val_zero.to_be_bytes::<32>());
            let p_ref = &publisher;
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_dex_log(chain, pool, &[TOPIC_UNIV2_SYNC], &data, p_ref)
            }));
            assert!(res.is_ok(), "Panic on V2 Sync reserve0 = u128::MAX");
            assert!(res.unwrap(), "V2 Sync reserve0 = u128::MAX must return true");
            let update = rx.try_recv().expect("Should receive update");
            match update {
                MarketUpdate::ReserveUpdate { reserve0, reserve1, .. } => {
                    assert_eq!(reserve0, u128::MAX);
                    assert_eq!(reserve1, 0);
                }
                other => panic!("Unexpected update: {:?}", other),
            }

            // V2 Sync: reserve0 = 0, reserve1 = u128::MAX
            data[0..32].copy_from_slice(&val_zero.to_be_bytes::<32>());
            data[32..64].copy_from_slice(&val_u128_max.to_be_bytes::<32>());
            assert!(process_dex_log(chain, pool, &[TOPIC_UNIV2_SYNC], &data, &publisher));
            let update = rx.try_recv().expect("Should receive update");
            match update {
                MarketUpdate::ReserveUpdate { reserve0, reserve1, .. } => {
                    assert_eq!(reserve0, 0);
                    assert_eq!(reserve1, u128::MAX);
                }
                other => panic!("Unexpected update: {:?}", other),
            }

            // V2 Sync: reserve0 = u128::MAX, reserve1 = u128::MAX
            data[0..32].copy_from_slice(&val_u128_max.to_be_bytes::<32>());
            data[32..64].copy_from_slice(&val_u128_max.to_be_bytes::<32>());
            assert!(process_dex_log(chain, pool, &[TOPIC_UNIV2_SYNC], &data, &publisher));
            let update = rx.try_recv().expect("Should receive update");
            match update {
                MarketUpdate::ReserveUpdate { reserve0, reserve1, .. } => {
                    assert_eq!(reserve0, u128::MAX);
                    assert_eq!(reserve1, u128::MAX);
                }
                other => panic!("Unexpected update: {:?}", other),
            }

            // V3 Swap: liquidity = u128::MAX
            let mut v3_data = [0u8; 160];
            v3_data[64..96].copy_from_slice(&U256::from(1000u64).to_be_bytes::<32>());
            v3_data[96..128].copy_from_slice(&val_u128_max.to_be_bytes::<32>());
            v3_data[156..160].copy_from_slice(&(-887272i32).to_be_bytes());
            assert!(process_dex_log(chain, pool, &[TOPIC_UNIV3_SWAP], &v3_data, &publisher));
            let update = rx.try_recv().expect("Should receive update");
            match update {
                MarketUpdate::TickUpdate { liquidity, current_tick, .. } => {
                    assert_eq!(liquidity, u128::MAX);
                    assert_eq!(current_tick, -887272);
                }
                other => panic!("Unexpected update: {:?}", other),
            }

            // PancakeSwap V3: liquidity = u128::MAX
            assert!(process_dex_log(chain, pool, &[TOPIC_PANCAKE_V3_SWAP], &v3_data, &publisher));
            let update = rx.try_recv().expect("Should receive update");
            match update {
                MarketUpdate::TickUpdate { liquidity, .. } => {
                    assert_eq!(liquidity, u128::MAX);
                }
                other => panic!("Unexpected update: {:?}", other),
            }
        }

        // ----------------------------------------------------------------------
        // Case 2: Boundary u128::MAX + 1 must REJECT (return false) without panic
        // ----------------------------------------------------------------------
        {
            let publisher = MarketFeedPublisher::new(16);
            let p_ref = &publisher;

            // V2 Sync: reserve0 = u128::MAX + 1
            let mut data = [0u8; 64];
            data[0..32].copy_from_slice(&val_u128_max_plus_1.to_be_bytes::<32>());
            data[32..64].copy_from_slice(&val_one.to_be_bytes::<32>());
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_dex_log(chain, pool, &[TOPIC_UNIV2_SYNC], &data, p_ref)
            }));
            assert!(res.is_ok(), "Panic on reserve0 = u128::MAX + 1");
            assert!(!res.unwrap(), "reserve0 = u128::MAX + 1 must return false");

            // V2 Sync: reserve1 = u128::MAX + 1
            data[0..32].copy_from_slice(&val_one.to_be_bytes::<32>());
            data[32..64].copy_from_slice(&val_u128_max_plus_1.to_be_bytes::<32>());
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_dex_log(chain, pool, &[TOPIC_UNIV2_SYNC], &data, p_ref)
            }));
            assert!(res.is_ok(), "Panic on reserve1 = u128::MAX + 1");
            assert!(!res.unwrap(), "reserve1 = u128::MAX + 1 must return false");

            // V2 Sync: both = u128::MAX + 1
            data[0..32].copy_from_slice(&val_u128_max_plus_1.to_be_bytes::<32>());
            data[32..64].copy_from_slice(&val_u128_max_plus_1.to_be_bytes::<32>());
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_dex_log(chain, pool, &[TOPIC_UNIV2_SYNC], &data, p_ref)
            }));
            assert!(res.is_ok(), "Panic on both = u128::MAX + 1");
            assert!(!res.unwrap(), "both = u128::MAX + 1 must return false");

            // V3 Swap: liquidity = u128::MAX + 1
            let mut v3_data = [0u8; 160];
            v3_data[96..128].copy_from_slice(&val_u128_max_plus_1.to_be_bytes::<32>());
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_dex_log(chain, pool, &[TOPIC_UNIV3_SWAP], &v3_data, p_ref)
            }));
            assert!(res.is_ok(), "Panic on V3 liquidity = u128::MAX + 1");
            assert!(!res.unwrap(), "V3 liquidity = u128::MAX + 1 must return false");

            // PancakeSwap V3: liquidity = u128::MAX + 1
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_dex_log(chain, pool, &[TOPIC_PANCAKE_V3_SWAP], &v3_data, p_ref)
            }));
            assert!(res.is_ok(), "Panic on Pancake V3 liquidity = u128::MAX + 1");
            assert!(!res.unwrap(), "Pancake V3 liquidity = u128::MAX + 1 must return false");
        }

        // ----------------------------------------------------------------------
        // Case 3: Boundary U256::MAX must REJECT (return false) without panic
        // ----------------------------------------------------------------------
        {
            let publisher = MarketFeedPublisher::new(16);
            let p_ref = &publisher;

            // V2 Sync: reserve0 = U256::MAX
            let mut data = [0u8; 64];
            data[0..32].copy_from_slice(&val_u256_max.to_be_bytes::<32>());
            data[32..64].copy_from_slice(&val_one.to_be_bytes::<32>());
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_dex_log(chain, pool, &[TOPIC_UNIV2_SYNC], &data, p_ref)
            }));
            assert!(res.is_ok(), "Panic on reserve0 = U256::MAX");
            assert!(!res.unwrap(), "reserve0 = U256::MAX must return false");

            // V2 Sync: reserve1 = U256::MAX
            data[0..32].copy_from_slice(&val_one.to_be_bytes::<32>());
            data[32..64].copy_from_slice(&val_u256_max.to_be_bytes::<32>());
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_dex_log(chain, pool, &[TOPIC_UNIV2_SYNC], &data, p_ref)
            }));
            assert!(res.is_ok(), "Panic on reserve1 = U256::MAX");
            assert!(!res.unwrap(), "reserve1 = U256::MAX must return false");

            // V3 Swap: liquidity = U256::MAX
            let mut v3_data = [0u8; 160];
            v3_data[96..128].copy_from_slice(&val_u256_max.to_be_bytes::<32>());
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_dex_log(chain, pool, &[TOPIC_UNIV3_SWAP], &v3_data, p_ref)
            }));
            assert!(res.is_ok(), "Panic on V3 liquidity = U256::MAX");
            assert!(!res.unwrap(), "V3 liquidity = U256::MAX must return false");
        }

        // ----------------------------------------------------------------------
        // Case 4: Zero Values must SUCCEED gracefully without panic
        // ----------------------------------------------------------------------
        {
            let publisher = MarketFeedPublisher::new(16);
            let mut rx = publisher.subscribe();

            let data_zero = [0u8; 64];
            assert!(process_dex_log(chain, pool, &[TOPIC_UNIV2_SYNC], &data_zero, &publisher));
            let update = rx.try_recv().expect("Should receive zero update");
            match update {
                MarketUpdate::ReserveUpdate { reserve0, reserve1, .. } => {
                    assert_eq!(reserve0, 0);
                    assert_eq!(reserve1, 0);
                }
                other => panic!("Unexpected update: {:?}", other),
            }

            let v3_zero = [0u8; 160];
            assert!(process_dex_log(chain, pool, &[TOPIC_UNIV3_SWAP], &v3_zero, &publisher));
            let update = rx.try_recv().expect("Should receive zero V3 update");
            match update {
                MarketUpdate::TickUpdate { liquidity, current_tick, .. } => {
                    assert_eq!(liquidity, 0);
                    assert_eq!(current_tick, 0);
                }
                other => panic!("Unexpected update: {:?}", other),
            }
        }

        // ----------------------------------------------------------------------
        // Case 5: Truncated Inputs (< 64 for V2, < 160 for V3) must REJECT
        // ----------------------------------------------------------------------
        {
            let publisher = MarketFeedPublisher::new(16);
            let p_ref = &publisher;

            let truncated_lens = [0, 1, 15, 31, 32, 48, 63];
            for len in truncated_lens {
                let slice = vec![0x11u8; len];
                let res = catch_unwind(AssertUnwindSafe(|| {
                    process_dex_log(chain, pool, &[TOPIC_UNIV2_SYNC], &slice, p_ref)
                }));
                assert!(res.is_ok(), "Panic on truncated V2 slice len {}", len);
                assert!(!res.unwrap(), "Truncated V2 slice len {} must return false", len);
            }

            let v3_truncated_lens = [0, 1, 63, 64, 95, 96, 127, 128, 159];
            for len in v3_truncated_lens {
                let slice = vec![0x22u8; len];
                let res = catch_unwind(AssertUnwindSafe(|| {
                    process_dex_log(chain, pool, &[TOPIC_UNIV3_SWAP], &slice, p_ref)
                }));
                assert!(res.is_ok(), "Panic on truncated V3 slice len {}", len);
                assert!(!res.unwrap(), "Truncated V3 slice len {} must return false", len);

                let res_pancake = catch_unwind(AssertUnwindSafe(|| {
                    process_dex_log(chain, pool, &[TOPIC_PANCAKE_V3_SWAP], &slice, p_ref)
                }));
                assert!(res_pancake.is_ok(), "Panic on truncated Pancake V3 slice len {}", len);
                assert!(!res_pancake.unwrap(), "Truncated Pancake V3 slice len {} must return false", len);
            }
        }

        // ----------------------------------------------------------------------
        // Case 6: Corrupted Topics & Missing Data must REJECT
        // ----------------------------------------------------------------------
        {
            let publisher = MarketFeedPublisher::new(16);
            let p_ref = &publisher;
            let normal_data = [0u8; 160];

            // Empty topics slice
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_dex_log(chain, pool, &[], &normal_data, p_ref)
            }));
            assert!(res.is_ok());
            assert!(!res.unwrap(), "Empty topics must return false");

            // Unknown topic
            let unknown_topic = B256::repeat_byte(0xef);
            let res = catch_unwind(AssertUnwindSafe(|| {
                process_dex_log(chain, pool, &[unknown_topic], &normal_data, p_ref)
            }));
            assert!(res.is_ok());
            assert!(!res.unwrap(), "Unknown topic must return false");
        }
    }
}

// ==============================================================================
// 8. Arbitrum Sequencer Feed Hostile Payload Fuzzing
// ==============================================================================

#[test]
fn test_empirical_arbitrum_feed_hostile_fuzz() {
    let handler = ArbitrumFeedHandler::new("ws://127.0.0.1:8547");
    let publisher = MarketFeedPublisher::new(16);

    let hostile_frames = [
        "",
        " ",
        "null",
        "{}",
        "[]",
        "{\"version\": 1}",
        "{\"version\": 1, \"messages\": null}",
        "{\"version\": 1, \"messages\": []}",
        // Non-base64 l2msg
        "{\"version\": 1, \"messages\": [{\"sequenceNumber\": 10, \"message\": {\"message\": {\"l2msg\": \"not!valid!base64???\"}}}]}",
        // Empty base64 l2msg
        "{\"version\": 1, \"messages\": [{\"sequenceNumber\": 11, \"message\": {\"message\": {\"l2msg\": \"\"}}}]}",
        // Base64 containing single byte 0x06 (truncated batch header)
        "{\"version\": 1, \"messages\": [{\"sequenceNumber\": 12, \"message\": {\"message\": {\"l2msg\": \"Bg==\"}}}]}",
        // Base64 containing single byte 0x03 (truncated tx header)
        "{\"version\": 1, \"messages\": [{\"sequenceNumber\": 13, \"message\": {\"message\": {\"l2msg\": \"Aw==\"}}}]}",
        // Base64 containing invalid sub-message length exceeding buffer
        "{\"version\": 1, \"messages\": [{\"sequenceNumber\": 14, \"message\": {\"message\": {\"l2msg\": \"BgAAAAAAAAAB\"}}}]}",
        // Garbage RLP envelope
        "{\"version\": 1, \"messages\": [{\"sequenceNumber\": 15, \"message\": {\"message\": {\"l2msg\": \"A/3+/v7+/v7+\"}}}]}",
    ];

    for frame in hostile_frames {
        let p_ref = &publisher;
        let h_ref = &handler;
        let res = catch_unwind(AssertUnwindSafe(|| {
            h_ref.process_feed_message(frame, p_ref)
        }));
        assert!(res.is_ok(), "Panic on hostile Arbitrum feed frame: {:?}", frame);
    }
}

