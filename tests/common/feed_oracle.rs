//! Ingestion Feed Oracles: Arbitrum Sequencer Feed & Base Flashblocks
//! Authoritative source: ORIGINAL_REQUEST.md lines 27-29, 84, and explorer_survey_3/handoff.md.

pub use flash_collector::{
    unpack_arbitrum_batch, ArbitrumTxType, BatchPayload, FlashblockGasTracker, MockTx,
    SequencerDeduplicator,
};
