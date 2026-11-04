//! Shadow Paper-Trading Engine & Synthetic Priority Gas Auction Matching.
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 71-74; PROJECT.md § Milestone 5 Feature 20.

use alloy_primitives::{Address, B256, I256, U256};
use flash_core::types::Chain;
use flash_scanner::dfs::RouteCandidate;
use flash_simulator::types::SimulationOutcome;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, info};

use crate::telemetry::PipelineTelemetry;

/// Classification of synthetic execution matching outcome
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyntheticOutcome {
    /// No rival bots executed trades on the route's pools during this block
    UncontestedWin,
    /// Won Priority Gas Auction by bidding a higher maxPriorityFeePerGas
    WonPriorityGasAuction {
        competitor_bid: u128,
        our_bid: u128,
    },
    /// Lost Priority Gas Auction (rival bot bid higher priority fee)
    LostPriorityGasAuction {
        winning_competitor_bid: u128,
        our_bid: u128,
    },
    /// Lost due to sequencer feed latency arrival threshold
    LostOnLatency {
        latency_us: u64,
    },
    /// 30-Byte Fast Revert Guard prevented wasted execution gas
    FastRevertPrevented {
        competitor_bid: u128,
        gas_saved: u64,
    },
}

impl SyntheticOutcome {
    pub fn is_win(&self) -> bool {
        matches!(self, Self::UncontestedWin | Self::WonPriorityGasAuction { .. })
    }
}

/// Synthetic Order logged during Shadow Mode
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyntheticOrder {
    pub order_id: B256,
    pub timestamp_us: u64,
    pub block_number: u64,
    pub chain: Chain,
    pub target_pools: Vec<Address>,
    pub borrow_token: Address,
    pub optimal_input_amount: U256,
    pub theoretical_gross_profit: U256,
    pub theoretical_net_yield: I256,
    pub simulated_gas_used: u64,
    pub simulated_priority_fee_per_gas: u128,
    pub telemetry: PipelineTelemetry,
    pub signed_raw_tx_len: usize,
}

/// Mined competitor transaction extracted from block/sub-block
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompetingTx {
    pub tx_hash: B256,
    pub target_pool: Address,
    pub max_priority_fee_per_gas: u128,
    pub gas_used: u64,
    pub is_reverted: bool,
}

/// Result of evaluating a synthetic order against mined block data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyntheticMatchResult {
    pub order_id: B256,
    pub outcome: SyntheticOutcome,
    pub theoretical_pnl: I256,
    pub realized_synthetic_pnl: I256,
    pub telemetry: PipelineTelemetry,
}

/// Aggregated metrics comparing theoretical vs actual PnL
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ShadowPnLMetrics {
    pub total_simulated: u64,
    pub uncontested_wins: u64,
    pub pga_wins: u64,
    pub pga_losses: u64,
    pub fast_reverts_prevented: u64,

    pub cumulative_theoretical_pnl: I256,
    pub cumulative_realized_synthetic_pnl: I256,
    pub cumulative_simulated_gas_cost: U256,

    pub win_rate_percentage: f64,
    pub pnl_capture_ratio_percentage: f64,
}

/// Thread-safe tracker for shadow mode performance
#[derive(Debug, Default)]
pub struct ShadowPnLTracker {
    metrics: RwLock<ShadowPnLMetrics>,
}

impl ShadowPnLTracker {
    pub fn record_match(&self, order: &SyntheticOrder, outcome: &SyntheticOutcome, realized_pnl: I256) {
        let mut m = self.metrics.write();
        m.total_simulated += 1;
        m.cumulative_theoretical_pnl = m.cumulative_theoretical_pnl + order.theoretical_net_yield;
        m.cumulative_realized_synthetic_pnl = m.cumulative_realized_synthetic_pnl + realized_pnl;

        match outcome {
            SyntheticOutcome::UncontestedWin => m.uncontested_wins += 1,
            SyntheticOutcome::WonPriorityGasAuction { .. } => m.pga_wins += 1,
            SyntheticOutcome::LostPriorityGasAuction { .. } => m.pga_losses += 1,
            SyntheticOutcome::FastRevertPrevented { .. } => {
                m.pga_losses += 1;
                m.fast_reverts_prevented += 1;
            }
            SyntheticOutcome::LostOnLatency { .. } => m.pga_losses += 1,
        }

        let total_wins = m.uncontested_wins + m.pga_wins;
        m.win_rate_percentage = if m.total_simulated > 0 {
            (total_wins as f64 / m.total_simulated as f64) * 100.0
        } else {
            0.0
        };

        let theo_f64 = m.cumulative_theoretical_pnl.to_string().parse::<f64>().unwrap_or(1.0);
        let real_f64 = m.cumulative_realized_synthetic_pnl.to_string().parse::<f64>().unwrap_or(0.0);
        m.pnl_capture_ratio_percentage = if theo_f64 > 0.0 {
            (real_f64 / theo_f64) * 100.0
        } else {
            0.0
        };
    }

    pub fn snapshot(&self) -> ShadowPnLMetrics {
        self.metrics.read().clone()
    }
}

/// Shadow Paper-Trading Engine
pub struct ShadowEngine {
    pub chain: Chain,
    pub is_live: AtomicBool,
    pub tracker: ShadowPnLTracker,
    pending_orders: Mutex<VecDeque<SyntheticOrder>>,
    max_pending_orders: usize,
}

impl ShadowEngine {
    pub fn new(chain: Chain, is_live: bool) -> Self {
        Self {
            chain,
            is_live: AtomicBool::new(is_live),
            tracker: ShadowPnLTracker::default(),
            pending_orders: Mutex::new(VecDeque::with_capacity(1024)),
            max_pending_orders: 10_000,
        }
    }

    #[inline(always)]
    pub fn is_live(&self) -> bool {
        self.is_live.load(Ordering::SeqCst)
    }

    /// Enqueues a simulated opportunity for synthetic block matching
    pub fn register_simulated_opportunity(
        &self,
        candidate: &RouteCandidate,
        sim_outcome: &SimulationOutcome,
        telemetry: PipelineTelemetry,
        signed_raw_tx: &[u8],
        priority_fee: u128,
        block_number: u64,
    ) -> B256 {
        let order_id = B256::from_slice(&alloy_primitives::keccak256(signed_raw_tx)[..]);
        let target_pools = candidate.hops.iter().map(|h| h.pool_id).collect();

        let order = SyntheticOrder {
            order_id,
            timestamp_us: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros() as u64,
            block_number,
            chain: self.chain,
            target_pools,
            borrow_token: candidate.borrow_token,
            optimal_input_amount: candidate.sizing.optimal_input,
            theoretical_gross_profit: sim_outcome.actual_profit,
            theoretical_net_yield: sim_outcome.net_yield,
            simulated_gas_used: sim_outcome.gas_used,
            simulated_priority_fee_per_gas: priority_fee,
            telemetry,
            signed_raw_tx_len: signed_raw_tx.len(),
        };

        debug!(
            order_id = ?order.order_id,
            theoretical_net_yield = %order.theoretical_net_yield,
            simulated_priority_fee = order.simulated_priority_fee_per_gas,
            total_latency_us = telemetry.total_e2e_latency_us(),
            "Enqueued synthetic paper-trading order"
        );

        let mut queue = self.pending_orders.lock();
        if queue.len() >= self.max_pending_orders {
            queue.pop_front();
        }
        queue.push_back(order);

        order_id
    }

    /// Matches pending synthetic orders against mined block transactions
    pub fn match_mined_block(
        &self,
        block_number: u64,
        competing_txs: &[CompetingTx],
    ) -> Vec<SyntheticMatchResult> {
        let mut queue = self.pending_orders.lock();
        let mut results = Vec::new();
        let mut retained = VecDeque::new();

        while let Some(order) = queue.pop_front() {
            // Prune expired orders older than 10 blocks
            if block_number > order.block_number + 10 {
                continue;
            }

            // If block is earlier than order, keep waiting
            if block_number < order.block_number {
                retained.push_back(order);
                continue;
            }

            let pool_competitors: Vec<&CompetingTx> = competing_txs
                .iter()
                .filter(|tx| order.target_pools.contains(&tx.target_pool))
                .collect();

            let outcome = if pool_competitors.is_empty() {
                SyntheticOutcome::UncontestedWin
            } else {
                let max_comp_bid = pool_competitors
                    .iter()
                    .map(|tx| tx.max_priority_fee_per_gas)
                    .max()
                    .unwrap_or(0);

                if order.simulated_priority_fee_per_gas > max_comp_bid {
                    SyntheticOutcome::WonPriorityGasAuction {
                        competitor_bid: max_comp_bid,
                        our_bid: order.simulated_priority_fee_per_gas,
                    }
                } else {
                    SyntheticOutcome::FastRevertPrevented {
                        competitor_bid: max_comp_bid,
                        gas_saved: order.simulated_gas_used,
                    }
                }
            };

            let realized_pnl = if outcome.is_win() {
                order.theoretical_net_yield
            } else {
                I256::ZERO
            };

            self.tracker.record_match(&order, &outcome, realized_pnl);

            info!(
                order_id = ?order.order_id,
                outcome = ?outcome,
                theoretical_pnl = %order.theoretical_net_yield,
                realized_pnl = %realized_pnl,
                "Synthetic execution match completed"
            );

            results.push(SyntheticMatchResult {
                order_id: order.order_id,
                outcome,
                theoretical_pnl: order.theoretical_net_yield,
                realized_synthetic_pnl: realized_pnl,
                telemetry: order.telemetry,
            });
        }

        *queue = retained;
        results
    }
}
