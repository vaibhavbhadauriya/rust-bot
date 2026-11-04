//! Milestone 2 Adversarial Edge Cases & Mathematical Boundary Testing
//!
//! Validates:
//! 1. Multicall3 chunking logic: boundary cases with call counts of 0, 1, 149, 150, 151, 300, 450, 751, 1050.
//!    Verifies chunk boundaries do not drop calls, duplicate results, or scramble index alignment.
//!    Verifies per-chunk Tier A -> Tier B fallback resilience and partial non-fatal blank padding.
//! 2. Token decimal scaling across mixed pairs:
//!    - WBTC (8 decimals) vs USDC (6 decimals)
//!    - cbBTC (8 decimals) vs WETH (18 decimals)
//!    - USDC (6 decimals) vs WETH (18 decimals)
//!    Verifies no overflow, underflow, or precision truncation in unit reserve filters,
//!    CLMM virtual reserve derivation, and closed-form 2-pool optimal sizing.
//! 3. `MarketGraph::get_pool_tokens`:
//!    Verifies `Some((token0, token1))` for all registered pools and `None` for nonexistent pools
//!    across canonical mainnet catalogs (Arbitrum and Base), parallel pools, and concurrent stress load.

use alloy_primitives::{hex, Address, U256};
use flash_core::constants::{arbitrum, base, get_token_decimals};
use flash_core::types::{Chain, FeePips, PoolState, Venue};
use flash_math::virtual_reserves_from_clmm;
use flash_math::solve_2pool_optimal_raw;
use flash_scanner::discovery::{Call3, DiscoveredPool, Multicall3Discovery, MULTICALL_CHUNK_SIZE};
use flash_scanner::filters::filter_unit_reserves;
use flash_scanner::MarketGraph;
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Encodes an arbitrary slice of (bool success, Vec<u8> return_data) into exact Aggregate3 ABI return bytes
fn encode_aggregate3_test_return(items: &[(bool, Vec<u8>)]) -> Vec<u8> {
    let mut data = Vec::new();
    // Offset to array (0x20 = 32)
    let mut arr_off = [0u8; 32];
    arr_off[31] = 0x20;
    data.extend_from_slice(&arr_off);

    // Length of array
    let mut arr_len = [0u8; 32];
    arr_len[24..32].copy_from_slice(&(items.len() as u64).to_be_bytes());
    data.extend_from_slice(&arr_len);

    let head_size = items.len() * 32;
    let mut cur_tail = head_size;
    let mut encoded_elements = Vec::with_capacity(items.len());

    for (succ, ret_bytes) in items {
        let mut tup_off = [0u8; 32];
        tup_off[24..32].copy_from_slice(&(cur_tail as u64).to_be_bytes());
        data.extend_from_slice(&tup_off);

        let mut elem = Vec::new();
        // success word
        let mut succ_w = [0u8; 32];
        if *succ {
            succ_w[31] = 1;
        }
        elem.extend_from_slice(&succ_w);

        // bytes offset inside tuple (0x40 = 64)
        let mut b_off = [0u8; 32];
        b_off[31] = 0x40;
        elem.extend_from_slice(&b_off);

        // bytes length
        let mut b_len = [0u8; 32];
        b_len[24..32].copy_from_slice(&(ret_bytes.len() as u64).to_be_bytes());
        elem.extend_from_slice(&b_len);

        // bytes content + 32-byte padding
        elem.extend_from_slice(ret_bytes);
        let rem = ret_bytes.len() % 32;
        if rem != 0 {
            elem.extend_from_slice(&vec![0u8; 32 - rem]);
        }

        cur_tail += elem.len();
        encoded_elements.push(elem);
    }

    for elem in encoded_elements {
        data.extend_from_slice(&elem);
    }

    data
}

/// Decodes the calls from Multicall3 aggregate3 calldata to inspect what was transmitted
fn decode_calls_from_calldata(raw: &[u8]) -> Vec<Vec<u8>> {
    if raw.len() < 68 {
        return Vec::new();
    }
    let array_len = usize::try_from(U256::from_be_slice(&raw[36..68])).unwrap_or(0);
    let elements_start = 68;
    let mut calls_data = Vec::with_capacity(array_len);

    for i in 0..array_len {
        let offset_pos = elements_start + i * 32;
        if offset_pos + 32 > raw.len() {
            break;
        }
        let tuple_rel_offset = usize::try_from(U256::from_be_slice(&raw[offset_pos..offset_pos + 32])).unwrap_or(0);
        let tuple_pos = elements_start + tuple_rel_offset;
        if tuple_pos + 96 > raw.len() {
            break;
        }
        // word 2 is call_data offset
        let cd_rel_offset = usize::try_from(U256::from_be_slice(&raw[tuple_pos + 64..tuple_pos + 96])).unwrap_or(0);
        let cd_pos = tuple_pos + cd_rel_offset;
        if cd_pos + 32 > raw.len() {
            break;
        }
        let cd_len = usize::try_from(U256::from_be_slice(&raw[cd_pos..cd_pos + 32])).unwrap_or(0);
        let cd_start = cd_pos + 32;
        if cd_start + cd_len > raw.len() {
            break;
        }
        calls_data.push(raw[cd_start..cd_start + cd_len].to_vec());
    }

    calls_data
}

/// Spawns an empirical mock JSON-RPC HTTP server for Multicall3 chunking tests
async fn spawn_mock_multicall_server(
    recorded_chunks: Arc<Mutex<Vec<usize>>>,
    fail_chunks: Vec<usize>,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("Bind mock server");
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{}/rpc", port);

    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };

            let recorded_chunks = Arc::clone(&recorded_chunks);
            let fail_chunks = fail_chunks.clone();

            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut temp = [0u8; 8192];
                let header_end;
                let mut content_len = 0;

                loop {
                    let n = match socket.read(&mut temp).await {
                        Ok(0) => return,
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    buf.extend_from_slice(&temp[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        header_end = pos + 4;
                        let headers = String::from_utf8_lossy(&buf[..pos]);
                        for line in headers.lines() {
                            if line.to_lowercase().starts_with("content-length:") {
                                if let Some(val) = line.split(':').nth(1) {
                                    content_len = val.trim().parse::<usize>().unwrap_or(0);
                                }
                            }
                        }
                        break;
                    }
                }

                while buf.len() < header_end + content_len {
                    let n = match socket.read(&mut temp).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    buf.extend_from_slice(&temp[..n]);
                }

                if buf.len() < header_end + content_len {
                    return;
                }

                let body_slice = &buf[header_end..header_end + content_len];
                let val: serde_json::Value = match serde_json::from_slice(body_slice) {
                    Ok(v) => v,
                    Err(_) => return,
                };

                let data_hex = val["params"][0]["data"].as_str().unwrap_or("");
                let clean_hex = data_hex.strip_prefix("0x").unwrap_or(data_hex);
                let calldata = hex::decode(clean_hex).unwrap_or_default();

                let calls_data = decode_calls_from_calldata(&calldata);
                let num_calls = calls_data.len();

                let logical_chunk = if !calls_data.is_empty() && calls_data[0].len() >= 32 {
                    let idx = u64::from_be_bytes(calls_data[0][24..32].try_into().unwrap_or_default()) as usize;
                    idx / 150
                } else {
                    0
                };

                if fail_chunks.contains(&logical_chunk) {
                    let resp = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(resp.as_bytes()).await;
                    return;
                }

                recorded_chunks.lock().push(num_calls);

                let mut items = Vec::with_capacity(num_calls);
                for cd in calls_data {
                    items.push((true, cd));
                }

                let encoded_res = encode_aggregate3_test_return(&items);
                let hex_res = format!("0x{}", hex::encode(encoded_res));
                let resp_json = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": hex_res,
                });
                let resp_str = serde_json::to_string(&resp_json).unwrap();
                let http_resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_str.len(),
                    resp_str
                );
                let _ = socket.write_all(http_resp.as_bytes()).await;
            });
        }
    });

    (url, handle)
}

// =========================================================================
// REQUIREMENT 1: Multicall3 Chunking Logic Boundaries
// Boundary cases: 0, 1, 149, 150, 151, 300, 450, 751, 1050
// =========================================================================

#[tokio::test]
async fn test_multicall3_chunking_exact_boundaries() {
    assert_eq!(MULTICALL_CHUNK_SIZE, 150, "MULTICALL_CHUNK_SIZE must be exactly 150");

    let boundary_counts = [0usize, 1, 149, 150, 151, 300, 450, 751, 1050];

    for &total_calls in &boundary_counts {
        let recorded_chunks = Arc::new(Mutex::new(Vec::new()));
        let (rpc_url, server_handle) =
            spawn_mock_multicall_server(Arc::clone(&recorded_chunks), Vec::new()).await;

        let discovery = Multicall3Discovery::new(Chain::ArbitrumOne, rpc_url);

        // Build calls where each call has a unique payload encoding its index
        let mut calls = Vec::with_capacity(total_calls);
        for i in 0..total_calls {
            let mut payload = vec![0u8; 32];
            payload[24..32].copy_from_slice(&(i as u64).to_be_bytes());
            calls.push(Call3 {
                target: Address::from([0x77; 20]),
                allow_failure: true,
                call_data: payload,
            });
        }

        let results = discovery
            .execute_multicall(&calls)
            .await
            .expect("execute_multicall must succeed across mock chunks");

        // 1. Result count must exactly match call count
        assert_eq!(
            results.len(),
            total_calls,
            "Result count mismatch for total_calls = {}",
            total_calls
        );

        // 2. Index alignment check: result i must correspond exactly to call i
        for (i, res) in results.iter().enumerate() {
            assert!(res.success, "Call {} must be marked successful", i);
            assert_eq!(
                res.return_data, calls[i].call_data,
                "Scrambled index alignment or corrupted data at call index {}/{}",
                i, total_calls
            );
        }

        // 3. Verify server-side chunk sizes and count
        let recorded = recorded_chunks.lock().clone();
        if total_calls == 0 {
            assert!(recorded.is_empty(), "0 calls must not trigger any RPC requests");
        } else {
            let expected_chunk_count = (total_calls + MULTICALL_CHUNK_SIZE - 1) / MULTICALL_CHUNK_SIZE;
            assert_eq!(
                recorded.len(),
                expected_chunk_count,
                "Expected {} chunks for {} calls, server received {}",
                expected_chunk_count,
                total_calls,
                recorded.len()
            );

            // Verify individual chunk lengths
            for (chunk_idx, &chunk_len) in recorded.iter().enumerate() {
                if chunk_idx == recorded.len() - 1 {
                    let expected_last_len = total_calls % MULTICALL_CHUNK_SIZE;
                    let expected_last_len = if expected_last_len == 0 {
                        MULTICALL_CHUNK_SIZE
                    } else {
                        expected_last_len
                    };
                    assert_eq!(
                        chunk_len, expected_last_len,
                        "Last chunk length mismatch for total_calls = {}",
                        total_calls
                    );
                } else {
                    assert_eq!(
                        chunk_len, MULTICALL_CHUNK_SIZE,
                        "Intermediate chunks must have size MULTICALL_CHUNK_SIZE (150)"
                    );
                }
            }
        }

        server_handle.abort();
    }
}

#[tokio::test]
async fn test_multicall3_chunking_per_chunk_fallback_resilience() {
    // 301 calls: Chunk 0 (150), Chunk 1 (150), Chunk 2 (1)
    let total_calls = 301;
    let mut calls = Vec::with_capacity(total_calls);
    for i in 0..total_calls {
        let mut payload = vec![0u8; 32];
        payload[24..32].copy_from_slice(&(i as u64).to_be_bytes());
        calls.push(Call3 {
            target: Address::from([0x88; 20]),
            allow_failure: true,
            call_data: payload,
        });
    }

    // Tier A server: fails on Chunk 1 and Chunk 2
    let recorded_a = Arc::new(Mutex::new(Vec::new()));
    let (url_a, handle_a) =
        spawn_mock_multicall_server(Arc::clone(&recorded_a), vec![1, 2]).await;

    // Tier B server: succeeds on Chunk 1, fails on Chunk 2
    let recorded_b = Arc::new(Mutex::new(Vec::new()));
    let (url_b, handle_b) =
        spawn_mock_multicall_server(Arc::clone(&recorded_b), vec![2]).await;

    let discovery = Multicall3Discovery::with_fallback(Chain::ArbitrumOne, url_a, url_b);

    let results = discovery
        .execute_multicall(&calls)
        .await
        .expect("execute_multicall must complete with non-fatal chunk fallback");

    assert_eq!(results.len(), total_calls, "Total results must remain 301");

    // Chunk 0 (0..150): Succeeded on Tier A
    for i in 0..150 {
        assert!(results[i].success);
        assert_eq!(results[i].return_data, calls[i].call_data);
    }

    // Chunk 1 (150..300): Succeeded on Tier B fallback
    for i in 150..300 {
        assert!(results[i].success);
        assert_eq!(results[i].return_data, calls[i].call_data);
    }

    // Chunk 2 (300): Failed on both Tier A and Tier B, padded with non-fatal blank result
    assert!(!results[300].success);
    assert!(results[300].return_data.is_empty());

    handle_a.abort();
    handle_b.abort();
}

// =========================================================================
// REQUIREMENT 2: Token Decimal Scaling Across Mixed Pairs
// Mixed pairs: WBTC (8) vs USDC (6), cbBTC (8) vs WETH (18), USDC (6) vs WETH (18)
// =========================================================================

#[test]
fn test_mixed_pair_decimals_mappings() {
    // Arbitrum
    assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::WBTC), 8);
    assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::CBBTC), 8);
    assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::USDC_NATIVE), 6);
    assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::USDC_BRIDGED), 6);
    assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::USDT), 6);
    assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::WETH), 18);

    // Base
    assert_eq!(get_token_decimals(Chain::Base, base::CBBTC), 8);
    assert_eq!(get_token_decimals(Chain::Base, base::WBTC), 8);
    assert_eq!(get_token_decimals(Chain::Base, base::USDC_NATIVE), 6);
    assert_eq!(get_token_decimals(Chain::Base, base::USDBC_BRIDGED), 6);
    assert_eq!(get_token_decimals(Chain::Base, base::USDT), 6);
    assert_eq!(get_token_decimals(Chain::Base, base::WETH), 18);
}

#[test]
fn test_mixed_pair_filter_unit_reserves() {
    // 1. WBTC 8 decimals vs USDC 6 decimals
    let unit_wbtc = 100_000_000u128; // 10^8
    let unit_usdc = 1_000_000u128;   // 10^6

    // Exact boundary
    assert!(filter_unit_reserves(unit_wbtc, 8, unit_usdc, 6));
    assert!(filter_unit_reserves(unit_usdc, 6, unit_wbtc, 8)); // inverted order

    // Off-by-one underflow boundaries
    assert!(!filter_unit_reserves(unit_wbtc - 1, 8, unit_usdc, 6));
    assert!(!filter_unit_reserves(unit_wbtc, 8, unit_usdc - 1, 6));
    assert!(!filter_unit_reserves(0, 8, unit_usdc, 6));
    assert!(!filter_unit_reserves(unit_wbtc, 8, 0, 6));

    // 2. cbBTC 8 decimals vs WETH 18 decimals
    let unit_cbbtc = 100_000_000u128; // 10^8
    let unit_weth = 1_000_000_000_000_000_000u128; // 10^18

    assert!(filter_unit_reserves(unit_cbbtc, 8, unit_weth, 18));
    assert!(filter_unit_reserves(unit_weth, 18, unit_cbbtc, 8));
    assert!(!filter_unit_reserves(unit_cbbtc - 1, 8, unit_weth, 18));
    assert!(!filter_unit_reserves(unit_cbbtc, 8, unit_weth - 1, 18));

    // 3. USDC 6 decimals vs WETH 18 decimals
    assert!(filter_unit_reserves(unit_usdc, 6, unit_weth, 18));
    assert!(!filter_unit_reserves(unit_usdc - 1, 6, unit_weth, 18));
    assert!(!filter_unit_reserves(unit_usdc, 6, unit_weth - 1, 18));

    // 4. Extreme reserve boundaries
    assert!(filter_unit_reserves(u128::MAX, 8, u128::MAX, 6));
    assert!(filter_unit_reserves(u128::MAX, 8, u128::MAX, 18));
    assert!(filter_unit_reserves(u128::MAX, 38, u128::MAX, 38));

    // Overflow protection: decimal >= 39 returns false cleanly without panicking
    assert!(!filter_unit_reserves(u128::MAX, 39, u128::MAX, 6));
    assert!(!filter_unit_reserves(u128::MAX, 8, u128::MAX, 39));
    assert!(!filter_unit_reserves(u128::MAX, 255, u128::MAX, 255));
}

#[test]
fn test_mixed_pair_clmm_virtual_reserves_scaling() {
    // Scenario 1: WBTC (token0, 8 decimals) vs USDC (token1, 6 decimals)
    // Realistic market price: 1 WBTC = $95,000 USDC
    // Raw price P_raw = (95,000 * 10^6) / (10^8) = 950.
    // sqrt(P_raw) = sqrt(950) ≈ 30.822070014844882
    // sqrtPriceX96 = floor(sqrt(P_raw) * 2^96)
    let q96 = U256::from(1) << 96;
    let sqrt_950_approx = 30822070014844882u128; // scaled by 10^15
    let sqrt_price_x96 = (q96 * U256::from(sqrt_950_approx)) / U256::from(10u128.pow(15));

    // Deep liquidity: L = 50 * 10^8 (depth of 50 WBTC)
    let liquidity = 5_000_000_000u128;

    let (r0, r1) = virtual_reserves_from_clmm(liquidity, sqrt_price_x96);

    // Verify both reserves are strictly positive and satisfy unit reserves
    assert!(r0 > 0);
    assert!(r1 > 0);
    assert!(
        filter_unit_reserves(r0, 8, r1, 6),
        "CLMM WBTC/USDC reserves failed unit reserve check: r0={}, r1={}",
        r0, r1
    );

    // Verify implied price ratio: (r1 / 10^6) / (r0 / 10^8) ≈ 95,000 USDC/WBTC
    let r0_tokens = (r0 as f64) / 1e8;
    let r1_tokens = (r1 as f64) / 1e6;
    let price_ratio = r1_tokens / r0_tokens;
    assert!(
        (price_ratio - 95_000.0).abs() < 50.0,
        "Price ratio expected ~95000, got {}",
        price_ratio
    );

    // Scenario 2: cbBTC (token0, 8 decimals) vs WETH (token1, 18 decimals)
    // Realistic market price: 1 cbBTC = 30 WETH
    // Raw price P_raw = (30 * 10^18) / (10^8) = 3 * 10^11.
    // sqrt(P_raw) = sqrt(300,000,000,000) ≈ 547722.5575
    let sqrt_raw = 5477225575u128; // scaled by 10^4
    let sqrt_price_cbbtc_weth = (q96 * U256::from(sqrt_raw)) / U256::from(10000u64);

    let liquidity_cbbtc = 100_000_000_000_000u128; // 10^14
    let (r0_cbbtc, r1_weth) = virtual_reserves_from_clmm(liquidity_cbbtc, sqrt_price_cbbtc_weth);

    assert!(r0_cbbtc > 0);
    assert!(r1_weth > 0);
    assert!(
        filter_unit_reserves(r0_cbbtc, 8, r1_weth, 18),
        "CLMM cbBTC/WETH reserves failed unit reserve check: r0={}, r1={}",
        r0_cbbtc, r1_weth
    );

    let cbbtc_tokens = (r0_cbbtc as f64) / 1e8;
    let weth_tokens = (r1_weth as f64) / 1e18;
    let eth_per_btc = weth_tokens / cbbtc_tokens;
    assert!(
        (eth_per_btc - 30.0).abs() < 0.1,
        "Expected ~30 WETH/cbBTC, got {}",
        eth_per_btc
    );
}

#[test]
fn test_mixed_pair_2pool_optimal_sizing_exactness() {
    // 2-pool arbitrage between WBTC (8 dec) and USDC (6 dec)
    // Pool A: 10 WBTC (10^9 sat) and 970,000 USDC (9.7 * 10^11 micro-USDC) -> Price = 97,000 USDC/WBTC
    // Pool B: 10 WBTC (10^9 sat) and 950,000 USDC (9.5 * 10^11 micro-USDC) -> Price = 95,000 USDC/WBTC (~2.1% spread)
    let x_a = U256::from(10u128.pow(9));
    let y_a = U256::from(970_000 * 10u128.pow(6));
    let x_b = U256::from(10u128.pow(9));
    let y_b = U256::from(950_000 * 10u128.pow(6));

    let fee_30bps = 3000u32; // 0.3%
    let opt = solve_2pool_optimal_raw(x_a, y_a, fee_30bps, x_b, y_b, fee_30bps, 0)
        .expect("Must solve profitable 2-pool spread for WBTC/USDC");

    // Optimal input dx* is in satoshis (Token X)
    assert!(opt.optimal_input > U256::ZERO);
    assert!(opt.expected_output > opt.optimal_input);
    assert!(opt.gross_profit > U256::ZERO);
    assert!(opt.net_profit > U256::ZERO);
    assert_eq!(opt.net_profit, opt.gross_profit); // 0 flash fee

    // Verify scale: optimal input for 10 WBTC pool should be reasonable fraction (e.g. 0.01 - 0.2 WBTC)
    let opt_wbtc = (opt.optimal_input.to::<u128>() as f64) / 1e8;
    assert!(
        opt_wbtc > 0.001 && opt_wbtc < 1.0,
        "Optimal input should be between 0.001 and 1.0 WBTC, got {}",
        opt_wbtc
    );

    // 2-pool arbitrage between cbBTC (8 dec) and WETH (18 dec)
    // Pool A: 10 cbBTC and 310 WETH -> Price = 31 WETH / cbBTC
    // Pool B: 10 cbBTC and 300 WETH -> Price = 30 WETH / cbBTC
    let x_a_btc = U256::from(10u128.pow(9));
    let y_a_eth = U256::from(310 * 10u128.pow(18));
    let x_b_btc = U256::from(10u128.pow(9));
    let y_b_eth = U256::from(300 * 10u128.pow(18));

    let opt_eth = solve_2pool_optimal_raw(x_a_btc, y_a_eth, fee_30bps, x_b_btc, y_b_eth, fee_30bps, 0)
        .expect("Must solve profitable 2-pool spread for cbBTC/WETH");

    assert!(opt_eth.optimal_input > U256::ZERO);
    assert!(opt_eth.expected_output > opt_eth.optimal_input);
    assert!(opt_eth.net_profit > U256::ZERO);
}

// =========================================================================
// REQUIREMENT 3: MarketGraph::get_pool_tokens Verification
// Returns Some((token0, token1)) for all registered pools and None for nonexistent
// =========================================================================

#[test]
fn test_get_pool_tokens_complete_semantics() {
    let graph = MarketGraph::new();

    // 1. Unregistered pool queries on empty graph must return None
    assert_eq!(graph.get_pool_tokens(Address::ZERO), None);
    assert_eq!(graph.get_pool_tokens(Address::from([0x01; 20])), None);
    assert!(!graph.has_pool(&Address::ZERO));
    assert!(!graph.has_pool(&Address::from([0x01; 20])));

    // 2. Register all Arbitrum canonical mainnet pools (55 pools)
    let arb_pools = Multicall3Discovery::canonical_mainnet_pools(Chain::ArbitrumOne);
    assert!(
        arb_pools.len() >= 50,
        "Arbitrum canonical catalog must have at least 50 pools, found {}",
        arb_pools.len()
    );

    for pool in &arb_pools {
        assert_eq!(graph.get_pool_tokens(pool.id), None);
        graph.add_pool(pool);
        assert!(graph.has_pool(&pool.id));
        assert_eq!(
            graph.get_pool_tokens(pool.id),
            Some((pool.token0, pool.token1)),
            "Pool {:?} must resolve to Some(({:?}, {:?}))",
            pool.id, pool.token0, pool.token1
        );
    }

    // 3. Register all Base canonical mainnet pools (52 pools)
    let base_pools = Multicall3Discovery::canonical_mainnet_pools(Chain::Base);
    assert!(
        base_pools.len() >= 50,
        "Base canonical catalog must have at least 50 pools, found {}",
        base_pools.len()
    );

    for pool in &base_pools {
        if !graph.has_pool(&pool.id) {
            graph.add_pool(pool);
        }
        assert!(graph.has_pool(&pool.id));
        assert_eq!(
            graph.get_pool_tokens(pool.id),
            Some((pool.token0, pool.token1)),
            "Base Pool {:?} must resolve to Some(({:?}, {:?}))",
            pool.id, pool.token0, pool.token1
        );
    }

    // 4. Parallel pools test: multiple distinct pools between the same token pair
    let token_x = Address::from([0xaa; 20]);
    let token_y = Address::from([0xbb; 20]);

    let pool_v3_500 = DiscoveredPool::new(
        Address::from([0x50; 20]),
        Venue::UniswapV3,
        Chain::ArbitrumOne,
        token_x,
        token_y,
        FeePips::from_pips(500),
        PoolState::ConstantProduct {
            reserve0: 10_000_000,
            reserve1: 20_000_000,
        },
    );

    let pool_v3_3000 = DiscoveredPool::new(
        Address::from([0x30; 20]),
        Venue::UniswapV3,
        Chain::ArbitrumOne,
        token_x,
        token_y,
        FeePips::from_pips(3000),
        PoolState::ConstantProduct {
            reserve0: 15_000_000,
            reserve1: 30_000_000,
        },
    );

    let pool_sushi = DiscoveredPool::new(
        Address::from([0x55; 20]),
        Venue::SushiSwap,
        Chain::ArbitrumOne,
        token_x,
        token_y,
        FeePips::from_bps(30),
        PoolState::ConstantProduct {
            reserve0: 12_000_000,
            reserve1: 24_000_000,
        },
    );

    graph.add_pool(&pool_v3_500);
    graph.add_pool(&pool_v3_3000);
    graph.add_pool(&pool_sushi);

    assert_eq!(graph.get_pool_tokens(pool_v3_500.id), Some((token_x, token_y)));
    assert_eq!(graph.get_pool_tokens(pool_v3_3000.id), Some((token_x, token_y)));
    assert_eq!(graph.get_pool_tokens(pool_sushi.id), Some((token_x, token_y)));

    // 5. Test 200 non-existent random addresses
    for i in 100u8..=254 {
        let fake_addr = Address::from([i; 20]);
        if fake_addr != pool_v3_500.id
            && fake_addr != pool_v3_3000.id
            && fake_addr != pool_sushi.id
            && !arb_pools.iter().any(|p| p.id == fake_addr)
            && !base_pools.iter().any(|p| p.id == fake_addr)
        {
            assert_eq!(
                graph.get_pool_tokens(fake_addr),
                None,
                "Nonexistent address {:?} must return None",
                fake_addr
            );
            assert!(
                !graph.has_pool(&fake_addr),
                "Nonexistent address {:?} must return false for has_pool",
                fake_addr
            );
        }
    }
}

#[test]
fn test_get_pool_tokens_concurrent_stress() {
    let graph = Arc::new(MarketGraph::new());
    let num_writer_threads = 16;
    let pools_per_thread = 25;
    let num_reader_threads = 16;

    let total_pools = num_writer_threads * pools_per_thread;
    let mut all_pools = Vec::with_capacity(total_pools);

    for i in 0..total_pools {
        let mut pool_bytes = [0u8; 20];
        pool_bytes[16..20].copy_from_slice(&(i as u32 + 1000).to_be_bytes());
        let pool_id = Address::from(pool_bytes);

        let mut t0_bytes = [0u8; 20];
        t0_bytes[19] = (i % 20 + 1) as u8;
        let token0 = Address::from(t0_bytes);

        let mut t1_bytes = [0u8; 20];
        t1_bytes[19] = ((i + 1) % 20 + 1) as u8;
        let token1 = Address::from(t1_bytes);

        all_pools.push(DiscoveredPool::new(
            pool_id,
            Venue::UniswapV2,
            Chain::ArbitrumOne,
            token0,
            token1,
            FeePips::from_bps(30),
            PoolState::ConstantProduct {
                reserve0: 10_000_000,
                reserve1: 20_000_000,
            },
        ));
    }

    let all_pools = Arc::new(all_pools);
    let mut handles = Vec::new();

    // Writers
    for t_idx in 0..num_writer_threads {
        let g = Arc::clone(&graph);
        let pools = Arc::clone(&all_pools);
        handles.push(std::thread::spawn(move || {
            let start = t_idx * pools_per_thread;
            let end = start + pools_per_thread;
            for idx in start..end {
                g.add_pool(&pools[idx]);
            }
        }));
    }

    // Concurrent Readers
    for r_idx in 0..num_reader_threads {
        let g = Arc::clone(&graph);
        let pools = Arc::clone(&all_pools);
        handles.push(std::thread::spawn(move || {
            for iter in 0..500 {
                let random_idx = (r_idx * 17 + iter * 31) % pools.len();
                let pool = &pools[random_idx];
                if g.has_pool(&pool.id) {
                    let resolved = g.get_pool_tokens(pool.id);
                    assert!(
                        resolved.is_some(),
                        "has_pool was true but get_pool_tokens returned None"
                    );
                    let (t0, t1) = resolved.unwrap();
                    assert_eq!((t0, t1), (pool.token0, pool.token1));
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("Thread panicked in get_pool_tokens stress test");
    }

    // Post-stress validation: all total_pools must be present and correctly resolved
    assert_eq!(graph.pool_count(), total_pools);
    for pool in all_pools.iter() {
        assert_eq!(graph.get_pool_tokens(pool.id), Some((pool.token0, pool.token1)));
    }
}
