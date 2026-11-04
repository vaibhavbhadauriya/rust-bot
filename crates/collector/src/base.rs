//! Base Flashblocks 200ms Sub-Block Consumer & Cumulative Gas Tracker
//! Authoritative source: ORIGINAL_REQUEST.md line 29, and tests/common/feed_oracle.rs

use alloy_primitives::{Address, Bytes, B256, U256};
use flash_core::types::Chain;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use crate::arbitrum::{DexCalldataInspector, FastTxDecoder};
use crate::feed::{MarketFeedPublisher, MarketUpdate};

pub const FLASHBLOCK_CADENCE_MS: u64 = 200;
pub const BLOCK_TIME_MS: u64 = 2000;
pub const SUB_BLOCKS_PER_BLOCK: u8 = 10;
pub const DEFAULT_BASE_GAS_LIMIT: u64 = 30_000_000;

/// Feature 9: Base Flashblock Gas Budget Tracker
/// Sub-blocks 0 through 9 (10 Flashblocks per 2s block, 200ms cadence)
/// Cumulative budget for sub-block j in [0, 9] is:
/// Budget(j) = ((j + 1) / 10) * total_block_gas_limit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashblockGasTracker {
    pub total_block_gas_limit: u64,
    pub cumulative_gas_spent: u64,
    pub current_sub_block_index: u8,
}

impl FlashblockGasTracker {
    pub fn new(total_limit: u64) -> Self {
        Self {
            total_block_gas_limit: total_limit,
            cumulative_gas_spent: 0,
            current_sub_block_index: 0,
        }
    }

    /// Computes allowable cumulative gas budget for sub-block j (0-indexed [0..9], clamped to 10)
    #[inline(always)]
    pub fn sub_block_gas_budget(&self, j: u8) -> u64 {
        let factor = (j as u64 + 1).min(SUB_BLOCKS_PER_BLOCK as u64);
        (self.total_block_gas_limit * factor) / (SUB_BLOCKS_PER_BLOCK as u64)
    }

    /// Attempts to budget gas for an incoming transaction in sub-block j.
    /// Returns Ok(cumulative_gas_spent) or Err if budget is exceeded.
    #[inline(always)]
    pub fn try_budget_tx(&mut self, j: u8, tx_gas: u64) -> Result<u64, &'static str> {
        self.current_sub_block_index = j;
        let allowed_budget = self.sub_block_gas_budget(j);
        match self.cumulative_gas_spent.checked_add(tx_gas) {
            Some(sum) if sum <= allowed_budget => {
                self.cumulative_gas_spent = sum;
                Ok(sum)
            }
            _ => Err("Flashblock gas budget exceeded for sub-block window"),
        }
    }

    /// Resets tracker at block boundary
    #[inline(always)]
    pub fn reset_block(&mut self) {
        self.cumulative_gas_spent = 0;
        self.current_sub_block_index = 0;
    }
}

pub const TOPIC_UNIV2_SYNC: B256 =
    alloy_primitives::b256!("1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1");
pub const TOPIC_UNIV3_SWAP: B256 =
    alloy_primitives::b256!("c42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67");
pub const TOPIC_PANCAKE_V3_SWAP: B256 =
    alloy_primitives::b256!("19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83");

#[derive(Debug, thiserror::Error)]
pub enum BaseFeedError {
    #[error("WebSocket connection error: {0}")]
    WebSocket(String),
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),
    #[error("Gas budget error: {0}")]
    GasBudget(String),
    #[error("Subscription error: {0}")]
    Subscription(String),
}

/// Flashblock wire message payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlashblockMessage {
    #[serde(alias = "blockNumber", default)]
    pub block_number: u64,
    #[serde(alias = "index", alias = "sub_block_index", default)]
    pub sub_block_index: u8,
    #[serde(alias = "baseFee", default)]
    pub base_fee: U256,
    #[serde(alias = "gasUsed", default)]
    pub gas_used: u64,
    #[serde(alias = "gasLimit", default)]
    pub gas_limit: u64,
    #[serde(default)]
    pub transactions: Vec<Bytes>,
}

/// Processes an incoming DEX event log (Uniswap V2 Sync, Uniswap V3 Swap, or PancakeSwap V3 Swap).
pub fn process_dex_log(
    chain: Chain,
    address: Address,
    topics: &[B256],
    data: &[u8],
    publisher: &MarketFeedPublisher,
) -> bool {
    if topics.is_empty() {
        return false;
    }
    let topic0 = topics[0];
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    if topic0 == TOPIC_UNIV2_SYNC {
        if data.len() < 64 {
            return false;
        }
        let r0_u256 = U256::from_be_slice(&data[0..32]);
        let r1_u256 = U256::from_be_slice(&data[32..64]);
        let (reserve0, reserve1) = match (u128::try_from(r0_u256), u128::try_from(r1_u256)) {
            (Ok(r0), Ok(r1)) => (r0, r1),
            _ => return false,
        };
        let update = MarketUpdate::ReserveUpdate {
            chain,
            pool_id: address,
            reserve0,
            reserve1,
            timestamp_ms: now_ms,
        };
        let _ = publisher.publish(update);
        return true;
    } else if topic0 == TOPIC_UNIV3_SWAP || topic0 == TOPIC_PANCAKE_V3_SWAP {
        if data.len() < 160 {
            return false;
        }
        let sqrt_price_x96 = U256::from_be_slice(&data[64..96]);
        let liquidity = match u128::try_from(U256::from_be_slice(&data[96..128])) {
            Ok(l) => l,
            Err(_) => return false,
        };
        let tick_bytes: [u8; 4] = data[156..160].try_into().unwrap_or([0; 4]);
        let current_tick = i32::from_be_bytes(tick_bytes);

        let update = MarketUpdate::TickUpdate {
            chain,
            pool_id: address,
            sqrt_price_x96,
            current_tick,
            liquidity,
            timestamp_ms: now_ms,
        };
        let _ = publisher.publish(update);
        return true;
    }

    false
}

/// Extracts address, topics, and data from JSON-RPC eth_subscription log frame and passes to process_dex_log.
pub fn process_json_rpc_log(
    chain: Chain,
    v: &serde_json::Value,
    publisher: &MarketFeedPublisher,
) -> bool {
    let log_candidate = if v.get("method").and_then(|m| m.as_str()) == Some("eth_subscription") {
        v.get("params").and_then(|p| p.get("result"))
    } else if v.get("topics").is_some() && v.get("data").is_some() {
        Some(v)
    } else {
        None
    };

    if let Some(log) = log_candidate {
        if let (Some(addr_str), Some(topics_arr), Some(data_str)) = (
            log.get("address").and_then(|a| a.as_str()),
            log.get("topics").and_then(|t| t.as_array()),
            log.get("data").and_then(|d| d.as_str()),
        ) {
            if let Ok(addr) = addr_str.parse::<Address>() {
                let topics: Vec<B256> = topics_arr
                    .iter()
                    .filter_map(|t| t.as_str().and_then(|s| s.parse::<B256>().ok()))
                    .collect();
                let raw_hex = data_str.trim_start_matches("0x");
                if let Ok(data) = alloy_primitives::hex::decode(raw_hex) {
                    return process_dex_log(chain, addr, &topics, &data, publisher);
                }
            }
        }
    }

    false
}

/// Processes a Base Flashblock message: updates gas budgeting, emits FlashblockStep,
/// decodes transactions with FastTxDecoder, and publishes PendingTxMutation for DEX swaps.
pub fn process_flashblock(
    fb: &FlashblockMessage,
    gas_tracker: &parking_lot::Mutex<FlashblockGasTracker>,
    publisher: &MarketFeedPublisher,
) -> usize {
    let (cumulative_gas_spent, gas_budget) = {
        let mut tracker = gas_tracker.lock();
        let _ = tracker.try_budget_tx(fb.sub_block_index, fb.gas_used);
        (
            tracker.cumulative_gas_spent,
            tracker.sub_block_gas_budget(fb.sub_block_index),
        )
    };

    let _ = publisher.publish(MarketUpdate::FlashblockStep {
        block_number: fb.block_number,
        sub_block_index: fb.sub_block_index,
        cumulative_gas_spent,
        gas_budget,
    });

    let mut swap_count = 0;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    for raw_tx in &fb.transactions {
        if let Ok(decoded) = FastTxDecoder::decode(raw_tx) {
            if DexCalldataInspector::is_dex_swap(&decoded.calldata) {
                let selector =
                    DexCalldataInspector::extract_selector(&decoded.calldata).unwrap_or([0; 4]);
                let target_pool = decoded.to.unwrap_or(Address::ZERO);

                let update = MarketUpdate::PendingTxMutation {
                    chain: Chain::Base,
                    tx_hash: decoded.tx_hash,
                    target_pool,
                    selector,
                    calldata: decoded.calldata,
                    gas_limit: decoded.gas_limit,
                    max_priority_fee_per_gas: decoded.max_priority_fee_per_gas,
                    sequence_number: ((fb.block_number as u64) << 8) | (fb.sub_block_index as u64),
                    timestamp_ms: now_ms,
                };
                let _ = publisher.publish(update);
                swap_count += 1;
            }
        }
    }

    if fb.sub_block_index >= 9 {
        gas_tracker.lock().reset_block();
    }

    swap_count
}

/// Builds JSON-RPC eth_subscribe request for Uniswap V2 Sync, Uniswap V3 Swap, and PancakeSwap V3 Swap logs.
pub fn build_alchemy_logs_subscribe_request() -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_subscribe",
        "params": [
            "logs",
            {
                "topics": [
                    [
                        format!("0x{:x}", TOPIC_UNIV2_SYNC),
                        format!("0x{:x}", TOPIC_UNIV3_SWAP),
                        format!("0x{:x}", TOPIC_PANCAKE_V3_SWAP)
                    ]
                ]
            }
        ]
    })
    .to_string()
}

/// Base Flashblock Stream Handler
pub struct BaseFlashblockHandler {
    gas_tracker: Arc<parking_lot::Mutex<FlashblockGasTracker>>,
    ws_endpoint: String,
}

impl BaseFlashblockHandler {
    pub fn new(ws_endpoint: impl Into<String>, total_gas_limit: u64) -> Self {
        Self {
            gas_tracker: Arc::new(parking_lot::Mutex::new(FlashblockGasTracker::new(
                total_gas_limit,
            ))),
            ws_endpoint: ws_endpoint.into(),
        }
    }

    pub fn tracker_arc(&self) -> Arc<parking_lot::Mutex<FlashblockGasTracker>> {
        Arc::clone(&self.gas_tracker)
    }

    pub fn tracker(&self) -> FlashblockGasTracker {
        self.gas_tracker.lock().clone()
    }

    pub fn tracker_mut(&mut self) -> parking_lot::MutexGuard<'_, FlashblockGasTracker> {
        self.gas_tracker.lock()
    }

    pub fn ws_endpoint(&self) -> &str {
        &self.ws_endpoint
    }

    /// Parses incoming WebSocket message text, handling both Flashblock sub-blocks
    /// and Alchemy log subscription notifications.
    pub fn process_message_text(
        &self,
        text: &str,
        publisher: &MarketFeedPublisher,
    ) -> Result<bool, BaseFeedError> {
        let v: serde_json::Value = match serde_json::from_str(text) {
            Ok(val) => val,
            Err(_) => return Ok(false),
        };

        if process_json_rpc_log(Chain::Base, &v, publisher) {
            return Ok(true);
        }

        // Check for Flashblock payload
        let fb_val = if let Some(res) = v.get("params").and_then(|p| p.get("result")) {
            res
        } else if let Some(res) = v.get("result") {
            res
        } else {
            &v
        };

        if let Ok(fb) = serde_json::from_value::<FlashblockMessage>(fb_val.clone()) {
            if fb.block_number > 0 || !fb.transactions.is_empty() {
                process_flashblock(&fb, &self.gas_tracker, publisher);
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Connects to Base Flashblocks WebSocket with fallback.
    pub async fn connect(
        &self,
        fallback_url: Option<&str>,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        BaseFeedError,
    > {
        match connect_async(&self.ws_endpoint).await {
            Ok((stream, _)) => {
                tracing::info!(
                    endpoint = %self.ws_endpoint,
                    "Connected to primary Base Flashblocks feed"
                );
                Ok(stream)
            }
            Err(e) => {
                if let Some(fb) = fallback_url {
                    tracing::warn!(
                        error = %e,
                        fallback = %fb,
                        "Primary Base feed failed; connecting to fallback"
                    );
                    let (stream, _) = connect_async(fb)
                        .await
                        .map_err(|err| BaseFeedError::WebSocket(err.to_string()))?;
                    Ok(stream)
                } else {
                    Err(BaseFeedError::WebSocket(e.to_string()))
                }
            }
        }
    }


    /// Background worker streaming Base Flashblocks and Alchemy WS logs.
    pub async fn run(
        &self,
        publisher: MarketFeedPublisher,
        fallback_url: Option<String>,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), BaseFeedError> {
        let alchemy_shutdown_rx = shutdown_rx.resubscribe();
        let alchemy_pub = publisher.clone();
        let fallback_clone = fallback_url.clone();

        if let Some(alchemy_url) = fallback_clone {
            let log_handler = BaseLogStreamHandler::new(
                alchemy_url,
                Some(flash_core::constants::BASE_FALLBACK_RPC_WS.to_string()),
            );
            tokio::spawn(async move {
                let _ = log_handler.run(alchemy_pub, alchemy_shutdown_rx).await;
            });
        }

        let mut backoff_ms = 500u64;
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!("Base Flashblock handler shutting down gracefully");
                    return Ok(());
                }
                stream_res = self.connect(fallback_url.as_deref()) => {
                    match stream_res {
                        Ok(mut ws_stream) => {
                            backoff_ms = 500;
                            loop {
                                tokio::select! {
                                    _ = shutdown_rx.recv() => {
                                        tracing::info!("Base Flashblock worker shutting down");
                                        let _ = ws_stream.close(None).await;
                                        return Ok(());
                                    }
                                    msg_opt = ws_stream.next() => {
                                        match msg_opt {
                                            Some(Ok(Message::Text(text))) => {
                                                let _ = self.process_message_text(&text, &publisher);
                                            }
                                            Some(Ok(Message::Binary(bin))) => {
                                                if let Ok(text) = std::str::from_utf8(&bin) {
                                                    let _ = self.process_message_text(text, &publisher);
                                                }
                                            }
                                            Some(Ok(Message::Ping(p))) => {
                                                let _ = ws_stream.send(Message::Pong(p)).await;
                                            }
                                            Some(Ok(Message::Close(_))) => {
                                                tracing::warn!("Base Flashblock WebSocket closed by remote; reconnecting");
                                                break;
                                            }
                                            Some(Err(e)) => {
                                                tracing::warn!("Base Flashblock WebSocket read error: {:?}; reconnecting", e);
                                                break;
                                            }
                                            None => {
                                                tracing::warn!("Base Flashblock stream ended; reconnecting");
                                                break;
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Base Flashblock connection error: {:?}; retrying in {}ms", e, backoff_ms);
                            tokio::select! {
                                _ = shutdown_rx.recv() => return Ok(()),
                                _ = tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)) => {}
                            }
                            backoff_ms = (backoff_ms * 2).min(10_000);
                        }
                    }
                }
            }
        }
    }
}

/// Base WebSocket Log Stream Handler (Alchemy WS + Public RPC Fallback)
pub struct BaseLogStreamHandler {
    primary_url: String,
    fallback_url: Option<String>,
}

impl BaseLogStreamHandler {
    pub fn new(primary_url: impl Into<String>, fallback_url: Option<String>) -> Self {
        Self {
            primary_url: primary_url.into(),
            fallback_url,
        }
    }

    pub fn primary_url(&self) -> &str {
        &self.primary_url
    }

    pub fn fallback_url(&self) -> Option<&str> {
        self.fallback_url.as_deref()
    }

    /// Background worker streaming Base logs, handling ping/pong, reconnecting with exponential backoff (500ms to 10,000ms),
    /// and emitting MarketUpdate::ReserveUpdate & MarketUpdate::TickUpdate.
    pub async fn run(
        &self,
        publisher: MarketFeedPublisher,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), BaseFeedError> {
        let mut backoff_ms = 500u64;
        let mut use_fallback = false;

        loop {
            let fallback = self
                .fallback_url
                .as_deref()
                .unwrap_or(flash_core::constants::BASE_FALLBACK_RPC_WS);
            let target_url = if use_fallback { fallback } else { &self.primary_url };

            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!("Base logs subscriber shutting down gracefully");
                    return Ok(());
                }
                conn_res = connect_async(target_url) => {
                    match conn_res {
                        Ok((mut stream, _)) => {
                            tracing::info!(endpoint = %target_url, "Connected to Base logs WebSocket feed");
                            backoff_ms = 500;
                            let sub_req = build_alchemy_logs_subscribe_request();
                            if let Err(e) = stream.send(Message::Text(sub_req.into())).await {
                                tracing::warn!("Failed to send eth_subscribe to Base WS: {:?}", e);
                                use_fallback = !use_fallback;
                                continue;
                            }

                            loop {
                                tokio::select! {
                                    _ = shutdown_rx.recv() => {
                                        tracing::info!("Base log streamer worker shutting down");
                                        let _ = stream.close(None).await;
                                        return Ok(());
                                    }
                                    msg_opt = stream.next() => {
                                        match msg_opt {
                                            Some(Ok(Message::Text(text))) => {
                                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                                                    process_json_rpc_log(Chain::Base, &v, &publisher);
                                                }
                                            }
                                            Some(Ok(Message::Binary(bin))) => {
                                                if let Ok(text) = std::str::from_utf8(&bin) {
                                                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                                                        process_json_rpc_log(Chain::Base, &v, &publisher);
                                                    }
                                                }
                                            }
                                            Some(Ok(Message::Ping(p))) => {
                                                let _ = stream.send(Message::Pong(p)).await;
                                            }
                                            Some(Ok(Message::Close(_))) => {
                                                tracing::warn!("Base logs stream closed by remote; reconnecting");
                                                use_fallback = !use_fallback;
                                                break;
                                            }
                                            Some(Err(e)) => {
                                                tracing::warn!("Base logs WS error: {:?}; reconnecting", e);
                                                use_fallback = !use_fallback;
                                                break;
                                            }
                                            None => {
                                                tracing::warn!("Base logs WS stream ended; reconnecting");
                                                use_fallback = !use_fallback;
                                                break;
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed connecting to Base WS ({target_url}): {:?}; retrying in {backoff_ms}ms",
                                e
                            );
                            use_fallback = !use_fallback;
                            tokio::select! {
                                _ = shutdown_rx.recv() => return Ok(()),
                                _ = tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)) => {}
                            }
                            backoff_ms = (backoff_ms * 2).min(10_000);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flashblock_budgeting() {
        let mut tracker = FlashblockGasTracker::new(30_000_000);
        assert_eq!(tracker.sub_block_gas_budget(0), 3_000_000);
        assert_eq!(tracker.sub_block_gas_budget(4), 15_000_000);
        assert_eq!(tracker.sub_block_gas_budget(9), 30_000_000);
        assert_eq!(tracker.sub_block_gas_budget(12), 30_000_000);

        assert!(tracker.try_budget_tx(0, 2_000_000).is_ok());
        assert!(tracker.try_budget_tx(0, 2_000_000).is_err());
        tracker.reset_block();
        assert_eq!(tracker.cumulative_gas_spent, 0);
    }

    #[test]
    fn test_gas_budget_overflow_and_wrapping_safety() {
        let mut tracker = FlashblockGasTracker::new(30_000_000);
        tracker.cumulative_gas_spent = 10_000_000;

        // Extreme tx_gas that causes arithmetic overflow
        assert!(tracker.try_budget_tx(0, u64::MAX).is_err());
        assert_eq!(tracker.cumulative_gas_spent, 10_000_000);

        // Value that in unchecked release mode wraps modulo 2^64 to 4_999_999 <= 30_000_000
        let malicious_gas = u64::MAX - 5_000_000;
        assert!(tracker.try_budget_tx(9, malicious_gas).is_err());
        assert_eq!(tracker.cumulative_gas_spent, 10_000_000);
    }

    #[test]
    fn test_topic_pancake_v3_swap_and_subscribe_request() {
        assert_eq!(
            TOPIC_PANCAKE_V3_SWAP,
            alloy_primitives::b256!("19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83")
        );
        let req = build_alchemy_logs_subscribe_request();
        assert!(req.contains("0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"));
        assert!(req.contains("0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67"));
        assert!(req.contains("0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83"));
    }

    #[test]
    fn test_process_dex_log_pancake_v3_multichain() {
        let publisher = MarketFeedPublisher::new(8);
        let mut rx = publisher.subscribe();
        let pool = Address::repeat_byte(0x77);

        let mut data = [0u8; 160];
        let sqrt_price_x96 = U256::from(1000u64);
        let liquidity = 5000u128;
        let tick = 250i32;
        data[64..96].copy_from_slice(&sqrt_price_x96.to_be_bytes::<32>());
        data[96..128].copy_from_slice(&U256::from(liquidity).to_be_bytes::<32>());
        data[156..160].copy_from_slice(&tick.to_be_bytes());

        // Process for Arbitrum One
        assert!(process_dex_log(
            Chain::ArbitrumOne,
            pool,
            &[TOPIC_PANCAKE_V3_SWAP],
            &data,
            &publisher
        ));
        let update = rx.try_recv().expect("Should receive TickUpdate on Arbitrum");
        match update {
            MarketUpdate::TickUpdate {
                chain,
                pool_id,
                sqrt_price_x96: sp,
                current_tick,
                liquidity: liq,
                ..
            } => {
                assert_eq!(chain, Chain::ArbitrumOne);
                assert_eq!(pool_id, pool);
                assert_eq!(sp, sqrt_price_x96);
                assert_eq!(current_tick, 250);
                assert_eq!(liq, 5000);
            }
            other => panic!("Unexpected update: {:?}", other),
        }

        // Process for Base
        assert!(process_dex_log(
            Chain::Base,
            pool,
            &[TOPIC_PANCAKE_V3_SWAP],
            &data,
            &publisher
        ));
        let update_base = rx.try_recv().expect("Should receive TickUpdate on Base");
        match update_base {
            MarketUpdate::TickUpdate { chain, .. } => {
                assert_eq!(chain, Chain::Base);
            }
            other => panic!("Unexpected update: {:?}", other),
        }
    }

    #[test]
    fn test_process_dex_log_checked_u128_overflow_protection() {
        let publisher = MarketFeedPublisher::new(8);
        let pool = Address::repeat_byte(0x88);

        // V2 Sync: reserve0 > u128::MAX (e.g. U256::MAX)
        let mut data_overflow_r0 = [0u8; 64];
        data_overflow_r0[0..32].fill(0xff);
        data_overflow_r0[32..64].copy_from_slice(&U256::from(100u64).to_be_bytes::<32>());
        assert!(
            !process_dex_log(Chain::Base, pool, &[TOPIC_UNIV2_SYNC], &data_overflow_r0, &publisher),
            "reserve0 > u128::MAX must return false"
        );

        // V2 Sync: reserve1 > u128::MAX
        let mut data_overflow_r1 = [0u8; 64];
        data_overflow_r1[0..32].copy_from_slice(&U256::from(100u64).to_be_bytes::<32>());
        data_overflow_r1[32..64].fill(0xff);
        assert!(
            !process_dex_log(Chain::Base, pool, &[TOPIC_UNIV2_SYNC], &data_overflow_r1, &publisher),
            "reserve1 > u128::MAX must return false"
        );

        // V3 Swap: liquidity > u128::MAX
        let mut data_overflow_liq = [0u8; 160];
        data_overflow_liq[64..96].copy_from_slice(&U256::from(1000u64).to_be_bytes::<32>());
        data_overflow_liq[96..128].fill(0xff);
        assert!(
            !process_dex_log(Chain::Base, pool, &[TOPIC_UNIV3_SWAP], &data_overflow_liq, &publisher),
            "liquidity > u128::MAX must return false"
        );

        // Valid boundary: reserve0 and reserve1 exactly u128::MAX
        let mut data_boundary_v2 = [0u8; 64];
        data_boundary_v2[0..32].copy_from_slice(&U256::from(u128::MAX).to_be_bytes::<32>());
        data_boundary_v2[32..64].copy_from_slice(&U256::from(u128::MAX).to_be_bytes::<32>());
        assert!(
            process_dex_log(Chain::Base, pool, &[TOPIC_UNIV2_SYNC], &data_boundary_v2, &publisher),
            "reserve0/1 == u128::MAX must succeed"
        );
    }
}

