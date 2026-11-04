//! Flash Collector: Pre-Confirmation Sequencer Feed Handlers & Ingestion
//!
//! Features:
//! - Arbitrum One Sequencer WebSocket Ingestion & Monotonic Dedup & Recursive Batch Unpacking (Feature 8)
//! - Base Flashblocks 200ms Sub-Block Consumer & Cumulative Gas Budgeting (Feature 9)
//! - Unified MarketUpdate Event Publisher for flash-scanner

pub mod arbitrum;
pub mod base;
pub mod feed;

pub use arbitrum::{
    unpack_arbitrum_batch, unpack_l2_message, ArbitrumFeedError, ArbitrumFeedHandler, ArbitrumTxType,
    BatchPayload, BroadcastFeedMessage, DecodedTx, DexCalldataInspector, FastTxDecoder, MockTx,
    SequencerDeduplicator, SequencerFeedItem, MAX_BATCH_DEPTH,
};
pub use base::{
    process_dex_log, process_flashblock, BaseFeedError, BaseFlashblockHandler, FlashblockGasTracker,
    FlashblockMessage, BLOCK_TIME_MS, DEFAULT_BASE_GAS_LIMIT, FLASHBLOCK_CADENCE_MS,
    SUB_BLOCKS_PER_BLOCK, TOPIC_UNIV2_SYNC, TOPIC_UNIV3_SWAP,
};
pub use feed::{MarketFeedPublisher, MarketUpdate};
