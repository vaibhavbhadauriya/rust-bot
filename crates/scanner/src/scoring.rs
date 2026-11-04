//! Dynamic 60-second pool and pair scoring formula (Sp), Top-100 priority queue maintenance,
//! and instant burst promotion.
//! Authoritative source: ORIGINAL_REQUEST.md lines 24-26, PROJECT.md lines 49.

use alloy_primitives::Address;
use flash_core::types::Venue;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// Canonical token pair identifier with strictly ordered addresses (token0 < token1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PairId {
    pub token0: Address,
    pub token1: Address,
}

impl PairId {
    #[inline(always)]
    pub fn new(a: Address, b: Address) -> Self {
        if a < b {
            Self {
                token0: a,
                token1: b,
            }
        } else {
            Self {
                token0: b,
                token1: a,
            }
        }
    }
}

/// Evaluates the dynamic Top-100 scoring formula:
/// Sp = (sigma / avg_fee) * ln(1 + volume) * min(L_k) * I(venues >= 2)
#[inline]
pub fn compute_sp_score(
    sigma: f64,
    avg_fee: f64,
    volume: f64,
    min_liquidity: f64,
    venues: usize,
) -> f64 {
    // 1. Guard against non-finite inputs (NaN, +inf, -inf)
    if !sigma.is_finite()
        || !avg_fee.is_finite()
        || !volume.is_finite()
        || !min_liquidity.is_finite()
    {
        return 0.0;
    }

    if venues < 2 {
        return 0.0;
    }
    if sigma <= 0.0 || volume <= 0.0 || min_liquidity <= 0.0 {
        return 0.0;
    }
    // Prevent division by zero with 1 PIP (1e-6) floor
    let fee_denom = if avg_fee <= 1e-6 { 1e-6 } else { avg_fee };
    let vol_damped = (1.0 + volume).ln();
    let score = (sigma / fee_denom) * vol_damped * min_liquidity;

    // Guard against non-finite output
    if score.is_finite() && score > 0.0 {
        score
    } else {
        0.0
    }
}

/// Discrete 1-second observation bucket inside the rolling 60-second circular buffer
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TimeBucket {
    pub timestamp_sec: u64,
    pub volume: f64,
    pub sum_price: f64,
    pub sum_sq_price: f64,
    pub count: u32,
}

/// Zero-allocation 60-second rolling window tracking volume and price volatility
#[derive(Debug, Clone)]
pub struct Rolling60sWindow {
    pub buckets: [TimeBucket; 60],
}

impl serde::Serialize for Rolling60sWindow {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(60))?;
        for b in &self.buckets {
            seq.serialize_element(b)?;
        }
        seq.end()
    }
}

impl<'de> serde::Deserialize<'de> for Rolling60sWindow {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Rolling60sWindow;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a sequence of 60 TimeBuckets")
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut buckets = [TimeBucket::default(); 60];
                let mut idx = 0;
                while let Some(elem) = seq.next_element()? {
                    if idx < 60 {
                        buckets[idx] = elem;
                        idx += 1;
                    }
                }
                Ok(Rolling60sWindow { buckets })
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}

impl Default for Rolling60sWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl Rolling60sWindow {
    pub fn new() -> Self {
        Self {
            buckets: [TimeBucket::default(); 60],
        }
    }

    /// Records a swap observation at the specified timestamp in seconds
    pub fn record_swap(&mut self, timestamp_sec: u64, price: f64, volume: f64) {
        if price <= 0.0 || volume < 0.0 {
            return;
        }
        let idx = (timestamp_sec % 60) as usize;
        let bucket = &mut self.buckets[idx];

        if bucket.timestamp_sec != timestamp_sec {
            // Overwrite stale bucket from previous minute
            bucket.timestamp_sec = timestamp_sec;
            bucket.volume = volume;
            bucket.sum_price = price;
            bucket.sum_sq_price = price * price;
            bucket.count = 1;
        } else {
            bucket.volume += volume;
            bucket.sum_price += price;
            bucket.sum_sq_price += price * price;
            bucket.count += 1;
        }
    }

    /// Computes aggregated volume and relative volatility sigma = std_dev / mean over the last 60 seconds
    pub fn compute_metrics(&self, current_timestamp_sec: u64) -> (f64, f64) {
        let mut total_vol = 0.0;
        let mut sum_p = 0.0;
        let mut sum_sq_p = 0.0;
        let mut total_count = 0u32;

        for bucket in &self.buckets {
            if bucket.count > 0 && current_timestamp_sec.saturating_sub(bucket.timestamp_sec) < 60 {
                total_vol += bucket.volume;
                sum_p += bucket.sum_price;
                sum_sq_p += bucket.sum_sq_price;
                total_count += bucket.count;
            }
        }

        if total_count < 2 {
            return (0.0, total_vol);
        }

        let n = total_count as f64;
        let mean = sum_p / n;
        if mean <= 0.0 {
            return (0.0, total_vol);
        }

        let variance = (sum_sq_p - (sum_p * sum_p) / n) / (n - 1.0);
        let std_dev = if variance > 0.0 { variance.sqrt() } else { 0.0 };
        let rel_volatility = std_dev / mean;

        (rel_volatility, total_vol)
    }
}

/// Venue-specific liquidity tracking for a pair
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VenueMetric {
    pub venue: Venue,
    pub pool_address: Address,
    pub fee_rate: f64,
    pub liquidity: f64,
}

/// Complete real-time tracking data for a trading pair
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairTracker {
    pub pair_id: PairId,
    pub venues: HashMap<Venue, VenueMetric>,
    pub window: Rolling60sWindow,
    pub last_score: f64,
}

impl PairTracker {
    pub fn new(pair_id: PairId) -> Self {
        Self {
            pair_id,
            venues: HashMap::new(),
            window: Rolling60sWindow::new(),
            last_score: 0.0,
        }
    }

    pub fn update_venue(
        &mut self,
        venue: Venue,
        pool_address: Address,
        fee_rate: f64,
        liquidity: f64,
    ) {
        self.venues.insert(
            venue,
            VenueMetric {
                venue,
                pool_address,
                fee_rate,
                liquidity,
            },
        );
    }

    pub fn record_trade(&mut self, timestamp_sec: u64, price: f64, volume: f64) {
        self.window.record_swap(timestamp_sec, price, volume);
    }

    /// Evaluates current Sp score for the pair
    pub fn evaluate_score(&mut self, current_timestamp_sec: u64) -> f64 {
        let venue_count = self.venues.len();
        if venue_count < 2 {
            self.last_score = 0.0;
            return 0.0;
        }

        let mut sum_fee = 0.0;
        let mut min_liq = f64::INFINITY;

        for v in self.venues.values() {
            sum_fee += v.fee_rate;
            if v.liquidity < min_liq {
                min_liq = v.liquidity;
            }
        }

        let avg_fee = sum_fee / (venue_count as f64);
        let (sigma, volume) = self.window.compute_metrics(current_timestamp_sec);

        let score = compute_sp_score(sigma, avg_fee, volume, min_liq, venue_count);
        self.last_score = score;
        score
    }
}

/// Pair score entry for priority queue ranking
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScoredPair {
    pub pair_id: PairId,
    pub score: f64,
}

impl Eq for ScoredPair {}

impl Ord for ScoredPair {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| self.pair_id.cmp(&other.pair_id))
    }
}

impl PartialOrd for ScoredPair {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Min-heap comparator wrapper for top-K extraction
#[derive(Debug, Clone, Copy, PartialEq)]
struct MinScoredPair(ScoredPair);

impl Eq for MinScoredPair {}

impl Ord for MinScoredPair {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap
        other
            .0
            .score
            .total_cmp(&self.0.score)
            .then_with(|| self.0.pair_id.cmp(&other.0.pair_id))
    }
}

impl PartialOrd for MinScoredPair {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Top-100 Priority Queue & Set Maintenance Engine
#[derive(Debug, Clone)]
pub struct Top100Ranker {
    pub capacity: usize,
    pub active_pairs: HashSet<PairId>,
    pub ranked_list: Vec<ScoredPair>,
    pub min_active_score: f64,
}

impl Default for Top100Ranker {
    fn default() -> Self {
        Self::new(100)
    }
}

impl Top100Ranker {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            active_pairs: HashSet::with_capacity(capacity),
            ranked_list: Vec::with_capacity(capacity),
            min_active_score: 0.0,
        }
    }

    /// Full 60-second epoch re-ranking over all tracked pairs
    pub fn recompute_top_100(
        &mut self,
        trackers: &mut HashMap<PairId, PairTracker>,
        current_timestamp_sec: u64,
    ) -> &[ScoredPair] {
        let mut min_heap: BinaryHeap<MinScoredPair> =
            BinaryHeap::with_capacity(self.capacity + 1);

        for (pair_id, tracker) in trackers.iter_mut() {
            let score = tracker.evaluate_score(current_timestamp_sec);
            if score <= 0.0 {
                continue;
            }

            let entry = ScoredPair {
                pair_id: *pair_id,
                score,
            };

            if min_heap.len() < self.capacity {
                min_heap.push(MinScoredPair(entry));
            } else if let Some(min_top) = min_heap.peek() {
                if score > min_top.0.score {
                    min_heap.pop();
                    min_heap.push(MinScoredPair(entry));
                }
            }
        }

        // Extract sorted descending
        let mut ranked = Vec::with_capacity(min_heap.len());
        while let Some(min_entry) = min_heap.pop() {
            ranked.push(min_entry.0);
        }
        ranked.reverse(); // Now descending

        self.active_pairs.clear();
        for item in &ranked {
            self.active_pairs.insert(item.pair_id);
        }

        self.min_active_score = ranked.last().map(|p| p.score).unwrap_or(0.0);
        self.ranked_list = ranked;

        &self.ranked_list
    }

    /// Instant Burst Promotion:
    /// Promotes an unranked pair immediately if sudden volume/volatility spike occurs.
    ///
    /// Returns true if the pair was promoted into the active evaluation set.
    pub fn check_instant_burst_promotion(
        &mut self,
        pair_id: PairId,
        burst_score: f64,
        surge_ratio: f64,
    ) -> bool {
        // If already active, nothing to promote
        if self.active_pairs.contains(&pair_id) {
            return false;
        }

        // Promotion trigger condition:
        // 1. Score exceeds current 100th threshold, OR
        // 2. Surge ratio >= 3.0 and score is non-trivial (> min_active_score * 0.80)
        let triggers = (burst_score > self.min_active_score)
            || (surge_ratio >= 3.0 && burst_score > (self.min_active_score * 0.80));

        if triggers && burst_score > 0.0 {
            self.active_pairs.insert(pair_id);

            let entry = ScoredPair {
                pair_id,
                score: burst_score,
            };
            self.ranked_list.push(entry);
            self.ranked_list.sort_by(|a, b| b.cmp(a));

            if self.ranked_list.len() > self.capacity {
                let evicted = self.ranked_list.pop().unwrap();
                self.active_pairs.remove(&evicted.pair_id);
            }

            self.min_active_score = self.ranked_list.last().map(|p| p.score).unwrap_or(0.0);
            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sp_scoring_formula() {
        let sigma = 0.02;
        let avg_fee = 0.003;
        let volume = 100_000.0;
        let min_liquidity = 500_000.0;
        let venues = 2;

        let score = compute_sp_score(sigma, avg_fee, volume, min_liquidity, venues);
        let expected = (0.02 / 0.003) * (1.0 + 100_000.0f64).ln() * 500_000.0;
        assert!((score - expected).abs() < 1e-4);
        assert!(score > 0.0);
    }

    #[test]
    fn test_single_venue_zero() {
        let score = compute_sp_score(0.05, 0.003, 1_000_000.0, 500_000.0, 1);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_zero_fee_clamp() {
        let score = compute_sp_score(0.01, 0.0, 10_000.0, 100_000.0, 2);
        assert!(score.is_finite());
        assert!(score > 0.0);
    }

    #[test]
    fn test_top_100_ranker_and_burst_promotion() {
        let mut ranker = Top100Ranker::new(5);
        let mut trackers = HashMap::new();

        for i in 1..=10 {
            let pair_id =
                PairId::new(Address::from([i as u8; 20]), Address::from([0xff; 20]));
            let mut tracker = PairTracker::new(pair_id);
            tracker.update_venue(Venue::UniswapV3, Address::ZERO, 0.003, 100_000.0);
            tracker.update_venue(Venue::SushiSwap, Address::ZERO, 0.003, 100_000.0);
            tracker.record_trade(100, 100.0, (i * 10_000) as f64);
            tracker.record_trade(101, 102.0, (i * 10_000) as f64);
            trackers.insert(pair_id, tracker);
        }

        let ranked = ranker.recompute_top_100(&mut trackers, 105);
        assert_eq!(ranked.len(), 5);
        assert_eq!(ranker.active_pairs.len(), 5);

        // Instant burst promotion
        let unranked_id =
            PairId::new(Address::from([0xaa; 20]), Address::from([0xbb; 20]));
        let promoted =
            ranker.check_instant_burst_promotion(unranked_id, 999_999_999.0, 10.0);
        assert!(promoted);
        assert!(ranker.active_pairs.contains(&unranked_id));
        assert_eq!(ranker.ranked_list.len(), 5);
    }

    #[test]
    fn test_sp_scoring_nan_and_infinity_safety() {
        // Inputs with NaN must return 0.0
        assert_eq!(compute_sp_score(f64::NAN, 0.003, 100_000.0, 500_000.0, 2), 0.0);
        assert_eq!(compute_sp_score(0.02, f64::NAN, 100_000.0, 500_000.0, 2), 0.0);
        assert_eq!(compute_sp_score(0.02, 0.003, f64::NAN, 500_000.0, 2), 0.0);
        assert_eq!(compute_sp_score(0.02, 0.003, 100_000.0, f64::NAN, 2), 0.0);

        // Inputs with Infinity must return 0.0
        assert_eq!(compute_sp_score(f64::INFINITY, 0.003, 100_000.0, 500_000.0, 2), 0.0);
        assert_eq!(compute_sp_score(0.02, f64::INFINITY, 100_000.0, 500_000.0, 2), 0.0);
        assert_eq!(compute_sp_score(0.02, 0.003, f64::INFINITY, 500_000.0, 2), 0.0);
        assert_eq!(compute_sp_score(0.02, 0.003, 100_000.0, f64::INFINITY, 2), 0.0);
        assert_eq!(compute_sp_score(f64::NEG_INFINITY, 0.003, 100_000.0, 500_000.0, 2), 0.0);

        // Astronomical product that overflows float to +inf must return 0.0
        assert_eq!(compute_sp_score(1e300, 1e-6, 1e300, 1e300, 2), 0.0);
    }
}
