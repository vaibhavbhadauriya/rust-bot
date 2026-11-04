//! Microsecond Pipeline Telemetry Stage Timings & Latency Accounting.
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 71-74, 90; PROJECT.md § Milestone 5 Feature 20.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineStageTimings {
    pub feed_arrival_us: u64,
    pub graph_dfs_us: u64,
    pub sizing_math_us: u64,
    pub revm_sim_us: u64,
    pub tx_sign_us: u64,
}

/// Pipeline Telemetry recording elapsed duration across execution stages (in microseconds).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineTelemetry {
    /// Ingestion and feed frame unpack duration (µs)
    pub feed_arrival_us: u64,
    /// Targeted DFS cycle search duration (µs)
    pub graph_dfs_us: u64,
    /// Closed-form analytical sizing calculation duration (µs)
    pub sizing_math_us: u64,
    /// Cancun in-memory EVM revm simulation duration (µs)
    pub revm_sim_us: u64,
    /// 100-byte calldata formatting & local secp256k1 signing duration (µs)
    pub tx_sign_us: u64,
}

impl PipelineTelemetry {
    pub const MAX_MATH_SIM_BUDGET_US: u64 = 1500; // 1.5 ms
    pub const MAX_E2E_BUDGET_US: u64 = 3500;      // 3.5 ms

    #[inline(always)]
    pub fn new(stages: PipelineStageTimings) -> Self {
        Self {
            feed_arrival_us: stages.feed_arrival_us,
            graph_dfs_us: stages.graph_dfs_us,
            sizing_math_us: stages.sizing_math_us,
            revm_sim_us: stages.revm_sim_us,
            tx_sign_us: stages.tx_sign_us,
        }
    }

    /// Analytical math + embedded revm Cancun simulation latency (AC3 budget: < 1500 µs)
    #[inline(always)]
    pub fn math_plus_sim_latency_us(&self) -> u64 {
        self.sizing_math_us.saturating_add(self.revm_sim_us)
    }

    /// Full end-to-end shadow pipeline latency: feed arrival to signed raw tx (AC3 budget: < 3500 µs)
    #[inline(always)]
    pub fn total_e2e_latency_us(&self) -> u64 {
        self.feed_arrival_us
            .saturating_add(self.graph_dfs_us)
            .saturating_add(self.sizing_math_us)
            .saturating_add(self.revm_sim_us)
            .saturating_add(self.tx_sign_us)
    }

    #[inline(always)]
    pub fn is_math_plus_sim_compliant(&self) -> bool {
        self.math_plus_sim_latency_us() < Self::MAX_MATH_SIM_BUDGET_US
    }

    #[inline(always)]
    pub fn is_e2e_compliant(&self) -> bool {
        self.total_e2e_latency_us() < Self::MAX_E2E_BUDGET_US
    }
}

/// Aggregates and tracks telemetry statistics and percentiles across opportunities.
#[derive(Debug)]
pub struct TelemetryCollector {
    pub sample_count: AtomicU64,
    pub math_sim_violations: AtomicU64,
    pub e2e_violations: AtomicU64,

    // Real-Time M3 Telemetry Counters
    pub ws_frames: AtomicU64,
    pub pool_updates: AtomicU64,
    pub routes_screened: AtomicU64,
    pub near_miss_spreads: AtomicU64,
    pub profitable_hits: AtomicU64,

    sum_feed_arrival_us: AtomicU64,
    sum_graph_dfs_us: AtomicU64,
    sum_sizing_math_us: AtomicU64,
    sum_revm_sim_us: AtomicU64,
    sum_tx_sign_us: AtomicU64,

    recent_e2e_latencies: RwLock<VecDeque<u64>>,
    start_time: Instant,
}

impl Default for TelemetryCollector {
    fn default() -> Self {
        Self {
            sample_count: AtomicU64::new(0),
            math_sim_violations: AtomicU64::new(0),
            e2e_violations: AtomicU64::new(0),
            ws_frames: AtomicU64::new(0),
            pool_updates: AtomicU64::new(0),
            routes_screened: AtomicU64::new(0),
            near_miss_spreads: AtomicU64::new(0),
            profitable_hits: AtomicU64::new(0),
            sum_feed_arrival_us: AtomicU64::new(0),
            sum_graph_dfs_us: AtomicU64::new(0),
            sum_sizing_math_us: AtomicU64::new(0),
            sum_revm_sim_us: AtomicU64::new(0),
            sum_tx_sign_us: AtomicU64::new(0),
            recent_e2e_latencies: RwLock::new(VecDeque::new()),
            start_time: Instant::now(),
        }
    }
}

impl TelemetryCollector {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    #[inline(always)]
    pub fn inc_ws_frames(&self) {
        self.ws_frames.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn inc_pool_updates(&self) {
        self.pool_updates.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn add_routes_screened(&self, count: u64) {
        self.routes_screened.fetch_add(count, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn inc_routes_screened(&self, count: u64) {
        self.add_routes_screened(count);
    }

    #[inline(always)]
    pub fn inc_near_miss_spreads(&self) {
        self.near_miss_spreads.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn inc_profitable_hits(&self) {
        self.profitable_hits.fetch_add(1, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn ws_frames(&self) -> u64 {
        self.ws_frames.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn pool_updates(&self) -> u64 {
        self.pool_updates.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn routes_screened(&self) -> u64 {
        self.routes_screened.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn near_miss_spreads(&self) -> u64 {
        self.near_miss_spreads.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn profitable_hits(&self) -> u64 {
        self.profitable_hits.load(Ordering::Relaxed)
    }

    pub fn record(&self, telemetry: &PipelineTelemetry) {
        self.sample_count.fetch_add(1, Ordering::Relaxed);

        if !telemetry.is_math_plus_sim_compliant() {
            self.math_sim_violations.fetch_add(1, Ordering::Relaxed);
        }
        if !telemetry.is_e2e_compliant() {
            self.e2e_violations.fetch_add(1, Ordering::Relaxed);
        }

        self.sum_feed_arrival_us
            .fetch_add(telemetry.feed_arrival_us, Ordering::Relaxed);
        self.sum_graph_dfs_us
            .fetch_add(telemetry.graph_dfs_us, Ordering::Relaxed);
        self.sum_sizing_math_us
            .fetch_add(telemetry.sizing_math_us, Ordering::Relaxed);
        self.sum_revm_sim_us
            .fetch_add(telemetry.revm_sim_us, Ordering::Relaxed);
        self.sum_tx_sign_us
            .fetch_add(telemetry.tx_sign_us, Ordering::Relaxed);

        let mut latencies = self.recent_e2e_latencies.write();
        if latencies.len() >= 1000 {
            latencies.pop_front();
        }
        latencies.push_back(telemetry.total_e2e_latency_us());
    }

    pub fn p99_latency_us(&self) -> u64 {
        let latencies: Vec<u64> = {
            let guard = self.recent_e2e_latencies.read();
            if guard.is_empty() {
                return 0;
            }
            guard.iter().copied().collect()
        };
        let mut sorted = latencies;
        sorted.sort_unstable();
        let idx = ((sorted.len() as f64) * 0.99).floor() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    pub fn avg_e2e_latency_us(&self) -> f64 {
        let count = self.sample_count.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }
        let total = self.sum_feed_arrival_us.load(Ordering::Relaxed)
            + self.sum_graph_dfs_us.load(Ordering::Relaxed)
            + self.sum_sizing_math_us.load(Ordering::Relaxed)
            + self.sum_revm_sim_us.load(Ordering::Relaxed)
            + self.sum_tx_sign_us.load(Ordering::Relaxed);
        total as f64 / count as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_m3_telemetry_atomic_counters_and_getters() {
        let collector = TelemetryCollector::new();

        assert_eq!(collector.ws_frames(), 0);
        assert_eq!(collector.pool_updates(), 0);
        assert_eq!(collector.routes_screened(), 0);
        assert_eq!(collector.near_miss_spreads(), 0);
        assert_eq!(collector.profitable_hits(), 0);
        assert_eq!(collector.avg_e2e_latency_us(), 0.0);

        collector.inc_ws_frames();
        collector.inc_ws_frames();
        assert_eq!(collector.ws_frames(), 2);
        assert_eq!(collector.ws_frames.load(Ordering::Relaxed), 2);

        collector.inc_pool_updates();
        assert_eq!(collector.pool_updates(), 1);

        collector.add_routes_screened(15);
        collector.inc_routes_screened(5);
        assert_eq!(collector.routes_screened(), 20);

        collector.inc_near_miss_spreads();
        collector.inc_near_miss_spreads();
        collector.inc_near_miss_spreads();
        assert_eq!(collector.near_miss_spreads(), 3);

        collector.inc_profitable_hits();
        assert_eq!(collector.profitable_hits(), 1);

        assert!(collector.uptime_secs() <= 5);
    }
}
