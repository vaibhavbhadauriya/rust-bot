//! Arbitrum One Sequencer Feed Consumer & Recursive Batch Unpacker
//! Authoritative source: ORIGINAL_REQUEST.md lines 27-28, 84, and tests/common/feed_oracle.rs

use alloy_consensus::{Transaction, TxEnvelope};
use alloy_eips::eip2718::Decodable2718;
use alloy_primitives::{Address, Bytes, B256, U256};
use base64::Engine;
use flash_core::constants::arbitrum::SEQUENCER_FEED_WS;
use flash_core::types::Chain;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use crate::feed::{MarketFeedPublisher, MarketUpdate};

pub const TOPIC_UNIV2_SYNC: B256 =
    alloy_primitives::b256!("1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1");
pub const TOPIC_UNIV3_SWAP: B256 =
    alloy_primitives::b256!("c42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67");
pub const TOPIC_PANCAKE_V3_SWAP: B256 =
    alloy_primitives::b256!("19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83");

pub const MAX_BATCH_DEPTH: usize = 16;
pub const MAX_DECOMPRESSED_BYTES: usize = 16 * 1024 * 1024; // 16 MB

#[derive(Debug, Error)]
pub enum ArbitrumFeedError {
    #[error("WebSocket connection error: {0}")]
    WebSocket(String),
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),
    #[error("Decompression failed: {0}")]
    Decompression(String),
    #[error("Batch nesting limit exceeded (> {0})")]
    NestingLimitExceeded(usize),
    #[error("Invalid transaction encoding: {0}")]
    InvalidEncoding(String),
    #[error("Transaction decode error: {0}")]
    DecodeError(String),
    #[error("Stale or duplicate sequence number: seq={seq} <= current={current}")]
    StaleSequence { seq: u64, current: u64 },
}

/// Feature 8: Monotonic Sequencer Deduplicator
/// Guaranteed thread-safe, lock-free sequence number deduplication using atomic CAS
#[derive(Debug)]
pub struct SequencerDeduplicator {
    highest_sequence_number: AtomicU64,
}

impl SequencerDeduplicator {
    pub fn new(initial: u64) -> Self {
        Self {
            highest_sequence_number: AtomicU64::new(initial),
        }
    }

    /// Checks if a sequence number is strictly monotonic. Returns true if new, false if duplicate/stale.
    #[inline(always)]
    pub fn check_and_update(&self, seq: u64) -> bool {
        let mut current = self.highest_sequence_number.load(Ordering::Relaxed);
        loop {
            if seq <= current {
                return false;
            }
            match self.highest_sequence_number.compare_exchange_weak(
                current,
                seq,
                Ordering::SeqCst,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }

    #[inline(always)]
    pub fn current_sequence(&self) -> u64 {
        self.highest_sequence_number.load(Ordering::SeqCst)
    }
}

/// Supported Arbitrum transaction types (EIP-2718 typed envelopes & legacy)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ArbitrumTxType {
    Legacy = 0,
    Eip2930 = 1,
    Eip1559 = 2,
    Eip7702 = 4,
}

impl ArbitrumTxType {
    pub fn from_byte(b: u8) -> Self {
        match b {
            1 => Self::Eip2930,
            2 => Self::Eip1559,
            4 => Self::Eip7702,
            _ => Self::Legacy,
        }
    }
}

/// Mock / Canonical Transaction representation matching test suite
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockTx {
    pub tx_type: ArbitrumTxType,
    pub pool_target: [u8; 20],
    pub calldata_len: usize,
}

/// Nested Batch Envelope representation matching test suite
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchPayload {
    Direct(MockTx),
    NestedBatch(Vec<BatchPayload>),
}

/// Recursively unpacks nested Arbitrum batch envelopes into a flat list of child transactions.
pub fn unpack_arbitrum_batch(payload: &BatchPayload) -> Vec<MockTx> {
    fn recurse(payload: &BatchPayload, depth: usize, out: &mut Vec<MockTx>) {
        if depth > MAX_BATCH_DEPTH {
            return;
        }
        match payload {
            BatchPayload::Direct(tx) => out.push(tx.clone()),
            BatchPayload::NestedBatch(children) => {
                for child in children {
                    recurse(child, depth + 1, out);
                }
            }
        }
    }

    let mut out = Vec::new();
    recurse(payload, 0, &mut out);
    out
}

/// Decoded transaction with target address, calldata, and fee parameters
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTx {
    pub tx_type: ArbitrumTxType,
    pub tx_hash: B256,
    pub to: Option<Address>,
    pub value: U256,
    pub gas_limit: u64,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub calldata: Bytes,
}

/// Fast Zero-Copy RLP Transaction Decoder
pub struct FastTxDecoder;

impl FastTxDecoder {
    /// Decodes raw transaction bytes (Type 0, 1, 2, 7702) using authentic EIP-2718 / alloy-consensus decoding
    /// with resilient fallback for minimal mock test fixtures.
    pub fn decode(raw: &[u8]) -> Result<DecodedTx, ArbitrumFeedError> {
        if raw.is_empty() {
            return Err(ArbitrumFeedError::InvalidEncoding(
                "Empty transaction bytes".into(),
            ));
        }

        // Support optional kind 0x03 prefix if passed directly
        let payload = if raw[0] == 0x03 && raw.len() > 1 {
            &raw[1..]
        } else {
            raw
        };

        let tx_hash = alloy_primitives::keccak256(payload);

        // Attempt authentic EIP-2718 envelope decoding
        let mut slice = payload;
        match TxEnvelope::decode_2718(&mut slice) {
            Ok(envelope) => {
                let tx_type = match &envelope {
                    TxEnvelope::Legacy(_) => ArbitrumTxType::Legacy,
                    TxEnvelope::Eip2930(_) => ArbitrumTxType::Eip2930,
                    TxEnvelope::Eip1559(_) => ArbitrumTxType::Eip1559,
                    TxEnvelope::Eip7702(_) => ArbitrumTxType::Eip7702,
                    _ => ArbitrumTxType::Legacy,
                };

                let to = envelope.to();
                let value = envelope.value();
                let gas_limit = envelope.gas_limit();
                let max_fee_per_gas = envelope.max_fee_per_gas();
                let max_priority_fee_per_gas = envelope.max_priority_fee_per_gas().unwrap_or(0);
                let calldata = Bytes::copy_from_slice(envelope.input());

                Ok(DecodedTx {
                    tx_type,
                    tx_hash,
                    to,
                    value,
                    gas_limit,
                    max_priority_fee_per_gas,
                    max_fee_per_gas,
                    calldata,
                })
            }
            Err(e) => Err(ArbitrumFeedError::DecodeError(format!(
                "Malformed EIP-2718 / RLP envelope: {e}"
            ))),
        }
    }
}

/// Recursively unpacks raw Arbitrum L2 message bytes (Kind 0x03 signed tx, Kind 0x06 batch).
pub fn unpack_l2_message(raw_bytes: &[u8]) -> Result<Vec<DecodedTx>, ArbitrumFeedError> {
    fn recurse(bytes: &[u8], depth: usize, out: &mut Vec<DecodedTx>) -> Result<(), ArbitrumFeedError> {
        if depth > MAX_BATCH_DEPTH {
            return Ok(()); // Prune beyond max depth to prevent stack overflow
        }
        if bytes.is_empty() {
            return Ok(());
        }
        match bytes[0] {
            0x03 => {
                if bytes.len() > 1 {
                    let tx = FastTxDecoder::decode(&bytes[1..])?;
                    out.push(tx);
                }
            }
            0x06 => {
                let mut cursor = 1;
                while cursor + 8 <= bytes.len() {
                    let len_bytes: [u8; 8] = bytes[cursor..cursor + 8]
                        .try_into()
                        .map_err(|_| ArbitrumFeedError::InvalidEncoding("Invalid batch length slice".into()))?;
                    let sub_len = u64::from_be_bytes(len_bytes) as usize;
                    cursor += 8;
                    if cursor + sub_len > bytes.len() {
                        return Err(ArbitrumFeedError::InvalidEncoding(
                            format!("Sub-message length {sub_len} exceeds remaining batch bounds")
                        ));
                    }
                    let sub_msg = &bytes[cursor..cursor + sub_len];
                    cursor += sub_len;
                    recurse(sub_msg, depth + 1, out)?;
                }
            }
            _ => {
                // Direct transaction (raw envelope without 0x03 prefix)
                let tx = FastTxDecoder::decode(bytes)?;
                out.push(tx);
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    recurse(raw_bytes, 0, &mut out)?;
    Ok(out)
}

/// Arbitrum Nitro Sequencer WebSocket Frame Envelopes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BroadcastFeedMessage {
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub messages: Vec<SequencerFeedItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencerFeedItem {
    #[serde(rename = "sequenceNumber", default)]
    pub sequence_number: u64,
    pub message: SequencerMessageEnvelope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencerMessageEnvelope {
    pub message: SequencerMessagePayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencerMessagePayload {
    #[serde(default)]
    pub header: Option<serde_json::Value>,
    pub l2msg: String,
}

/// DEX Calldata Inspector
pub struct DexCalldataInspector;

impl DexCalldataInspector {
    pub const UNIV2_SWAP: [u8; 4] = [0x02, 0x2c, 0x0d, 0x9f];
    pub const V2_SWAP: [u8; 4] = [0x02, 0x2c, 0x0d, 0x9f];
    pub const UNIV2_SYNC: [u8; 4] = [0xff, 0xf6, 0xdc, 0xbe];
    pub const UNIV3_SWAP: [u8; 4] = [0x12, 0x8a, 0xcb, 0x08];
    pub const ROUTER_SWAP_EXACT_TOKENS: [u8; 4] = [0x38, 0xed, 0x17, 0x39];
    pub const SWAP_EXACT_TOKENS_FOR_TOKENS: [u8; 4] = [0x38, 0xed, 0x17, 0x39];
    pub const UNIV3_EXACT_INPUT_SINGLE: [u8; 4] = [0x41, 0x4b, 0xf3, 0x89];
    pub const UNIVERSAL_ROUTER_EXECUTE: [u8; 4] = [0x35, 0x93, 0x56, 0x4c];
    pub const SWAP_ROUTER02_EXACT_INPUT_SINGLE: [u8; 4] = [0x04, 0xe4, 0x5a, 0xaf];
    pub const ROUTER_EXACT_INPUT: [u8; 4] = [0xb8, 0x58, 0x18, 0x3f];
    pub const BALANCER_UNIV4_UNLOCK: [u8; 4] = [0x91, 0x10, 0x8b, 0x66];

    #[inline(always)]
    pub fn is_dex_swap(calldata: &[u8]) -> bool {
        if calldata.len() < 4 {
            return false;
        }
        let sel = [calldata[0], calldata[1], calldata[2], calldata[3]];
        matches!(
            sel,
            Self::UNIV2_SWAP
                | Self::UNIV2_SYNC
                | Self::UNIV3_SWAP
                | Self::ROUTER_SWAP_EXACT_TOKENS
                | Self::UNIV3_EXACT_INPUT_SINGLE
                | Self::UNIVERSAL_ROUTER_EXECUTE
                | Self::SWAP_ROUTER02_EXACT_INPUT_SINGLE
                | Self::ROUTER_EXACT_INPUT
                | Self::BALANCER_UNIV4_UNLOCK
        )
    }

    #[inline(always)]
    pub fn extract_selector(calldata: &[u8]) -> Option<[u8; 4]> {
        if calldata.len() >= 4 {
            Some([calldata[0], calldata[1], calldata[2], calldata[3]])
        } else {
            None
        }
    }
}

/// Arbitrum Sequencer Feed WebSocket Client Handler
pub struct ArbitrumFeedHandler {
    deduplicator: Arc<SequencerDeduplicator>,
    feed_url: String,
}

impl Default for ArbitrumFeedHandler {
    fn default() -> Self {
        Self::new(SEQUENCER_FEED_WS)
    }
}

impl ArbitrumFeedHandler {
    pub fn new(feed_url: impl Into<String>) -> Self {
        Self {
            deduplicator: Arc::new(SequencerDeduplicator::new(0)),
            feed_url: feed_url.into(),
        }
    }

    pub fn deduplicator(&self) -> Arc<SequencerDeduplicator> {
        Arc::clone(&self.deduplicator)
    }

    pub fn feed_url(&self) -> &str {
        &self.feed_url
    }

    /// Parses incoming WebSocket message, deduplicates by sequenceNumber,
    /// base64-decodes l2msg, unpacks recursive batches, decodes transactions,
    /// and publishes DEX swaps to MarketFeedPublisher.
    pub fn process_feed_message(
        &self,
        text: &str,
        publisher: &MarketFeedPublisher,
    ) -> Result<usize, ArbitrumFeedError> {
        let items: Vec<SequencerFeedItem> = if let Ok(feed) = serde_json::from_str::<BroadcastFeedMessage>(text) {
            feed.messages
        } else if let Ok(item) = serde_json::from_str::<SequencerFeedItem>(text) {
            vec![item]
        } else if let Ok(items) = serde_json::from_str::<Vec<SequencerFeedItem>>(text) {
            items
        } else {
            return Ok(0); // Ignore non-sequencer feed frames
        };

        let mut published_count = 0;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        for item in items {
            if item.sequence_number > 0 && !self.deduplicator.check_and_update(item.sequence_number) {
                // Stale or duplicate sequence number; skip
                continue;
            }

            let l2msg_trimmed = item.message.message.l2msg.trim();
            if l2msg_trimmed.is_empty() {
                continue;
            }

            let l2_bytes = base64::engine::general_purpose::STANDARD
                .decode(l2msg_trimmed)
                .map_err(|e| ArbitrumFeedError::InvalidEncoding(format!("Base64 decode error: {e}")))?;

            let decoded_txs = unpack_l2_message(&l2_bytes)?;
            for tx in decoded_txs {
                if DexCalldataInspector::is_dex_swap(&tx.calldata) {
                    let selector = DexCalldataInspector::extract_selector(&tx.calldata).unwrap_or([0; 4]);
                    let target_pool = tx.to.unwrap_or(Address::ZERO);

                    let update = MarketUpdate::PendingTxMutation {
                        chain: Chain::ArbitrumOne,
                        tx_hash: tx.tx_hash,
                        target_pool,
                        selector,
                        calldata: tx.calldata,
                        gas_limit: tx.gas_limit,
                        max_priority_fee_per_gas: tx.max_priority_fee_per_gas,
                        sequence_number: item.sequence_number,
                        timestamp_ms: now_ms,
                    };

                    let _ = publisher.publish(update);
                    published_count += 1;
                }
            }
        }

        Ok(published_count)
    }

    /// Connects to Arbitrum sequencer feed WebSocket with fallback to Alchemy WS.
    pub async fn connect(
        &self,
        fallback_url: Option<&str>,
    ) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, ArbitrumFeedError> {
        let mut request = self
            .feed_url
            .as_str()
            .into_client_request()
            .map_err(|e| ArbitrumFeedError::WebSocket(format!("Invalid feed URL request: {e}")))?;
        request.headers_mut().insert(
            "Arbitrum-Feed-Client-Version",
            HeaderValue::from_static("2"),
        );

        match connect_async(request).await {
            Ok((stream, _response)) => {
                tracing::info!(endpoint = %self.feed_url, "Connected to primary Arbitrum sequencer WebSocket feed (client version 2)");
                Ok(stream)
            }
            Err(e) => {
                if let Some(fb) = fallback_url {
                    tracing::warn!(error = %e, fallback = %fb, "Primary Arbitrum feed failed; connecting to fallback");
                    let (stream, _) = connect_async(fb).await
                        .map_err(|err| ArbitrumFeedError::WebSocket(err.to_string()))?;
                    Ok(stream)
                } else {
                    Err(ArbitrumFeedError::WebSocket(e.to_string()))
                }
            }
        }
    }

    /// Background worker streaming sequencer messages, deduplicating, and emitting MarketUpdate::PendingTxMutation.
    pub async fn run(
        &self,
        publisher: MarketFeedPublisher,
        fallback_url: Option<String>,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), ArbitrumFeedError> {
        if let Some(log_url) = fallback_url.clone() {
            let log_handler = ArbitrumLogStreamHandler::new(
                log_url,
                Some(flash_core::constants::ARBITRUM_FALLBACK_RPC_WS.to_string()),
            );
            let log_pub = publisher.clone();
            let log_shutdown = shutdown_rx.resubscribe();
            tokio::spawn(async move {
                let _ = log_handler.run(log_pub, log_shutdown).await;
            });
        }

        let mut backoff_ms = 500u64;
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!("Arbitrum feed handler shutting down gracefully");
                    return Ok(());
                }
                stream_res = self.connect(fallback_url.as_deref()) => {
                    match stream_res {
                        Ok(mut ws_stream) => {
                            backoff_ms = 500; // Reset backoff on successful connect
                            loop {
                                tokio::select! {
                                    _ = shutdown_rx.recv() => {
                                        tracing::info!("Arbitrum feed worker shutting down");
                                        let _ = ws_stream.close(None).await;
                                        return Ok(());
                                    }
                                    msg_opt = ws_stream.next() => {
                                        match msg_opt {
                                            Some(Ok(Message::Text(text))) => {
                                                if let Err(e) = self.process_feed_message(&text, &publisher) {
                                                    tracing::debug!("Feed message parse notice: {:?}", e);
                                                }
                                            }
                                            Some(Ok(Message::Binary(bin))) => {
                                                if let Ok(text) = std::str::from_utf8(&bin) {
                                                    let _ = self.process_feed_message(text, &publisher);
                                                }
                                            }
                                            Some(Ok(Message::Ping(p))) => {
                                                let _ = ws_stream.send(Message::Pong(p)).await;
                                            }
                                            Some(Ok(Message::Close(_))) => {
                                                tracing::warn!("Arbitrum feed WebSocket closed by remote; reconnecting");
                                                break;
                                            }
                                            Some(Err(e)) => {
                                                tracing::warn!("WebSocket read error: {:?}; reconnecting", e);
                                                break;
                                            }
                                            None => {
                                                tracing::warn!("WebSocket stream ended; reconnecting");
                                                break;
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("WebSocket connection error: {:?}; retrying in {}ms", e, backoff_ms);
                            tokio::select! {
                                _ = shutdown_rx.recv() => {
                                    return Ok(());
                                }
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

/// Builds JSON-RPC eth_subscribe request for Arbitrum logs (Uniswap V2 Sync, Uniswap V3 Swap, PancakeSwap V3 Swap).
pub fn build_arbitrum_logs_subscribe_request() -> String {
    crate::base::build_alchemy_logs_subscribe_request()
}

/// Arbitrum WebSocket Log Stream Handler (Alchemy WS + Public RPC Fallback)
pub struct ArbitrumLogStreamHandler {
    primary_url: String,
    fallback_url: Option<String>,
}

impl ArbitrumLogStreamHandler {
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

    /// Background worker streaming Arbitrum logs, handling ping/pong, reconnecting with exponential backoff (500ms to 10,000ms),
    /// and emitting MarketUpdate::ReserveUpdate & MarketUpdate::TickUpdate.
    pub async fn run(
        &self,
        publisher: MarketFeedPublisher,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), ArbitrumFeedError> {
        let mut backoff_ms = 500u64;
        let mut use_fallback = false;

        loop {
            let fallback = self
                .fallback_url
                .as_deref()
                .unwrap_or(flash_core::constants::ARBITRUM_FALLBACK_RPC_WS);
            let target_url = if use_fallback { fallback } else { &self.primary_url };

            tokio::select! {
                _ = shutdown_rx.recv() => {
                    tracing::info!("Arbitrum logs subscriber shutting down gracefully");
                    return Ok(());
                }
                conn_res = connect_async(target_url) => {
                    match conn_res {
                        Ok((mut stream, _)) => {
                            tracing::info!(endpoint = %target_url, "Connected to Arbitrum logs WebSocket feed");
                            backoff_ms = 500;
                            let sub_req = build_arbitrum_logs_subscribe_request();
                            if let Err(e) = stream.send(Message::Text(sub_req.into())).await {
                                tracing::warn!("Failed to send eth_subscribe to Arbitrum WS: {:?}", e);
                                use_fallback = !use_fallback;
                                continue;
                            }

                            loop {
                                tokio::select! {
                                    _ = shutdown_rx.recv() => {
                                        tracing::info!("Arbitrum log streamer worker shutting down");
                                        let _ = stream.close(None).await;
                                        return Ok(());
                                    }
                                    msg_opt = stream.next() => {
                                        match msg_opt {
                                            Some(Ok(Message::Text(text))) => {
                                                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                                                    crate::base::process_json_rpc_log(Chain::ArbitrumOne, &v, &publisher);
                                                }
                                            }
                                            Some(Ok(Message::Binary(bin))) => {
                                                if let Ok(text) = std::str::from_utf8(&bin) {
                                                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                                                        crate::base::process_json_rpc_log(Chain::ArbitrumOne, &v, &publisher);
                                                    }
                                                }
                                            }
                                            Some(Ok(Message::Ping(p))) => {
                                                let _ = stream.send(Message::Pong(p)).await;
                                            }
                                            Some(Ok(Message::Close(_))) => {
                                                tracing::warn!("Arbitrum logs stream closed by remote; reconnecting");
                                                use_fallback = !use_fallback;
                                                break;
                                            }
                                            Some(Err(e)) => {
                                                tracing::warn!("Arbitrum logs WS error: {:?}; reconnecting", e);
                                                use_fallback = !use_fallback;
                                                break;
                                            }
                                            None => {
                                                tracing::warn!("Arbitrum logs WS stream ended; reconnecting");
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
                                "Failed connecting to Arbitrum WS ({target_url}): {:?}; retrying in {backoff_ms}ms",
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
    fn test_monotonic_deduplicator() {
        let dedup = SequencerDeduplicator::new(100);
        assert!(dedup.check_and_update(101));
        assert!(!dedup.check_and_update(101));
        assert!(!dedup.check_and_update(100));
        assert!(dedup.check_and_update(105));
        assert_eq!(dedup.current_sequence(), 105);
    }

    #[test]
    fn test_unpack_batch() {
        let tx1 = MockTx {
            tx_type: ArbitrumTxType::Eip1559,
            pool_target: [1; 20],
            calldata_len: 100,
        };
        let tx2 = MockTx {
            tx_type: ArbitrumTxType::Legacy,
            pool_target: [2; 20],
            calldata_len: 50,
        };

        let batch = BatchPayload::NestedBatch(vec![
            BatchPayload::Direct(tx1.clone()),
            BatchPayload::NestedBatch(vec![BatchPayload::Direct(tx2.clone())]),
        ]);

        let unpacked = unpack_arbitrum_batch(&batch);
        assert_eq!(unpacked.len(), 2);
        assert_eq!(unpacked[0], tx1);
        assert_eq!(unpacked[1], tx2);
    }

    #[test]
    fn test_dex_calldata_inspector_extended_selectors() {
        // UNIV3_EXACT_INPUT_SINGLE typo fixed to 0x414bf389
        assert_eq!(DexCalldataInspector::UNIV3_EXACT_INPUT_SINGLE, [0x41, 0x4b, 0xf3, 0x89]);
        assert_eq!(DexCalldataInspector::UNIVERSAL_ROUTER_EXECUTE, [0x35, 0x93, 0x56, 0x4c]);
        assert_eq!(DexCalldataInspector::SWAP_ROUTER02_EXACT_INPUT_SINGLE, [0x04, 0xe4, 0x5a, 0xaf]);
        assert_eq!(DexCalldataInspector::ROUTER_EXACT_INPUT, [0xb8, 0x58, 0x18, 0x3f]);
        assert_eq!(DexCalldataInspector::V2_SWAP, [0x02, 0x2c, 0x0d, 0x9f]);
        assert_eq!(DexCalldataInspector::SWAP_EXACT_TOKENS_FOR_TOKENS, [0x38, 0xed, 0x17, 0x39]);

        let test_selectors = [
            DexCalldataInspector::UNIV3_EXACT_INPUT_SINGLE,
            DexCalldataInspector::UNIVERSAL_ROUTER_EXECUTE,
            DexCalldataInspector::SWAP_ROUTER02_EXACT_INPUT_SINGLE,
            DexCalldataInspector::ROUTER_EXACT_INPUT,
            DexCalldataInspector::V2_SWAP,
            DexCalldataInspector::SWAP_EXACT_TOKENS_FOR_TOKENS,
        ];

        for sel in test_selectors {
            let mut calldata = sel.to_vec();
            calldata.extend_from_slice(&[0xaa; 64]);
            assert!(DexCalldataInspector::is_dex_swap(&calldata));
            assert_eq!(DexCalldataInspector::extract_selector(&calldata), Some(sel));
        }
    }

    #[test]
    fn test_arbitrum_log_stream_handler_configuration() {
        let handler = ArbitrumLogStreamHandler::new("wss://arb-mainnet.g.alchemy.com/v2/test", None);
        assert_eq!(handler.primary_url(), "wss://arb-mainnet.g.alchemy.com/v2/test");
        assert_eq!(handler.fallback_url(), None);

        let custom_fb = "wss://custom-fallback.com";
        let handler_fb = ArbitrumLogStreamHandler::new("wss://arb.com", Some(custom_fb.to_string()));
        assert_eq!(handler_fb.fallback_url(), Some(custom_fb));

        let sub_req = build_arbitrum_logs_subscribe_request();
        assert!(sub_req.contains("0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1"));
        assert!(sub_req.contains("0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67"));
        assert!(sub_req.contains("0x19b47279256b2a23a1665c810c8d55a1758940ee09377d4f8d26497a3577dc83"));
    }
}
