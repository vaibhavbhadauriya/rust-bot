//! Unified Market Update Event Publisher
//! Emits streaming market events to flash-scanner
//! Authoritative source: ORIGINAL_REQUEST.md lines 27-30, PROJECT.md lines 4-6, 28, 52-53.

use alloy_primitives::{Address, Bytes, B256, U256};
use flash_core::types::Chain;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Unified Market Update Event
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketUpdate {
    /// Live reserve update for CPMM pool
    ReserveUpdate {
        chain: Chain,
        pool_id: Address,
        reserve0: u128,
        reserve1: u128,
        timestamp_ms: u64,
    },
    /// Concentrated liquidity pool tick / price update
    TickUpdate {
        chain: Chain,
        pool_id: Address,
        sqrt_price_x96: U256,
        current_tick: i32,
        liquidity: u128,
        timestamp_ms: u64,
    },
    /// Unconfirmed / pre-confirmed transaction mutating a DEX pool
    PendingTxMutation {
        chain: Chain,
        tx_hash: B256,
        target_pool: Address,
        selector: [u8; 4],
        calldata: Bytes,
        gas_limit: u64,
        max_priority_fee_per_gas: u128,
        sequence_number: u64,
        timestamp_ms: u64,
    },
    /// Base Flashblock boundary progress event
    FlashblockStep {
        block_number: u64,
        sub_block_index: u8,
        cumulative_gas_spent: u64,
        gas_budget: u64,
    },
}

/// Unified Event Publisher
#[derive(Clone, Debug)]
pub struct MarketFeedPublisher {
    sender: broadcast::Sender<MarketUpdate>,
}

impl MarketFeedPublisher {
    pub const DEFAULT_CAPACITY: usize = 65_536;

    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    #[inline(always)]
    pub fn publish(
        &self,
        update: MarketUpdate,
    ) -> Result<usize, broadcast::error::SendError<MarketUpdate>> {
        self.sender.send(update)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<MarketUpdate> {
        self.sender.subscribe()
    }

    #[inline(always)]
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for MarketFeedPublisher {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feed_publisher() {
        let publisher = MarketFeedPublisher::new(16);
        let mut rx = publisher.subscribe();

        let update = MarketUpdate::ReserveUpdate {
            chain: Chain::ArbitrumOne,
            pool_id: Address::ZERO,
            reserve0: 100,
            reserve1: 200,
            timestamp_ms: 1000,
        };

        assert!(publisher.publish(update.clone()).is_ok());
        let received = rx.try_recv().unwrap();
        assert_eq!(received, update);
    }
}
