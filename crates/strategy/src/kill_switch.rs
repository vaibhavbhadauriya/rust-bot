//! High-Performance Automated Kill Switch Controller.
//!
//! Sub-10ns hot-path health verification while coordinating emergency shutdown across:
//! 1. Feed relay lag degradation exceeding 100ms (`lag_ms > 100`)
//! 2. Lost arbitrage races resulting in early revert guards (`consecutive_fast_reverts >= 3`)
//! 3. Manual administrative halt signals
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 71-74; PROJECT.md lines 63, 76.

use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use thiserror::Error;

/// Default feed relay lag threshold before triggering emergency halt (100 milliseconds)
pub const DEFAULT_FEED_LAG_THRESHOLD_MS: u64 = 100;

/// Default consecutive fast-revert count before triggering emergency halt (3 reverts)
pub const DEFAULT_FAST_REVERTS_THRESHOLD: usize = 3;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum KillSwitchError {
    #[error("Execution halted: Kill switch is tripped: {0}")]
    Halted(String),
}

/// Zero-overhead thread-safe Automated Kill Switch Controller.
#[derive(Debug)]
pub struct KillSwitchController {
    is_tripped: AtomicBool,
    consecutive_fast_reverts: AtomicUsize,
    total_trips: AtomicU64,
    feed_lag_threshold_ms: u64,
    max_consecutive_reverts: usize,
    trip_reason: RwLock<Option<String>>,
}

impl Default for KillSwitchController {
    fn default() -> Self {
        Self::new()
    }
}

impl KillSwitchController {
    /// Constructs a new `KillSwitchController` with default thresholds (100ms feed lag, 3 reverts).
    pub fn new() -> Self {
        Self::with_thresholds(DEFAULT_FEED_LAG_THRESHOLD_MS, DEFAULT_FAST_REVERTS_THRESHOLD)
    }

    /// Constructs a new `KillSwitchController` with custom thresholds.
    pub fn with_thresholds(feed_lag_threshold_ms: u64, max_consecutive_reverts: usize) -> Self {
        Self {
            is_tripped: AtomicBool::new(false),
            consecutive_fast_reverts: AtomicUsize::new(0),
            total_trips: AtomicU64::new(0),
            feed_lag_threshold_ms,
            max_consecutive_reverts,
            trip_reason: RwLock::new(None),
        }
    }

    /// Hot-path inspection: returns `true` if the kill switch is currently tripped.
    /// Uses Acquire ordering: compiles to a single non-locked `mov` instruction (< 2ns).
    #[inline(always)]
    pub fn is_tripped(&self) -> bool {
        self.is_tripped.load(Ordering::Acquire)
    }

    /// Hot-path health assertion: returns `Ok(())` if active, or `Err(KillSwitchError)` if tripped.
    #[inline(always)]
    pub fn assert_healthy(&self) -> Result<(), KillSwitchError> {
        if self.is_tripped.load(Ordering::Acquire) {
            let reason = self
                .trip_reason
                .read()
                .clone()
                .unwrap_or_else(|| "Kill switch active (reason unspecified)".to_string());
            return Err(KillSwitchError::Halted(reason));
        }
        Ok(())
    }

    /// Evaluates feed arrival lag against threshold.
    ///
    /// Automatically trips the kill switch if `lag_ms > feed_lag_threshold_ms` (100ms).
    /// Returns `true` if lag caused a trip OR if the kill switch was already tripped by lag.
    pub fn check_feed_lag(&self, lag_ms: u64) -> bool {
        if lag_ms > self.feed_lag_threshold_ms {
            let reason = format!(
                "Feed lag exceeded {}ms threshold: {}ms",
                self.feed_lag_threshold_ms, lag_ms
            );
            self.trip_internal(reason);
            true
        } else {
            false
        }
    }

    /// Records trade result from on-chain receipt or 30-byte fast-revert guard simulation.
    ///
    /// - If `is_fast_revert == true`: increments consecutive reverts. If `>= 3`, trips kill switch and returns `true`.
    /// - If `is_fast_revert == false`: resets consecutive revert counter to 0 (unless already latched in tripped state). Returns `false`.
    pub fn record_trade_result(&self, is_fast_revert: bool) -> bool {
        if is_fast_revert {
            let count = self.consecutive_fast_reverts.fetch_add(1, Ordering::SeqCst) + 1;
            if count >= self.max_consecutive_reverts {
                let reason = format!("Tripped on {} consecutive fast-reverts", count);
                self.trip_internal(reason);
                return true;
            }
        } else {
            // Note: successful trade resets consecutive counter, but latched tripped state cannot be un-tripped except via reset()
            if !self.is_tripped.load(Ordering::Acquire) {
                self.consecutive_fast_reverts.store(0, Ordering::SeqCst);
            }
        }
        false
    }

    /// Manually or programmatically triggers an emergency halt.
    pub fn trip(&self, reason: impl Into<String>) {
        self.trip_internal(reason.into());
    }

    fn trip_internal(&self, reason: String) {
        let was_tripped = self.is_tripped.swap(true, Ordering::SeqCst);
        if !was_tripped {
            self.total_trips.fetch_add(1, Ordering::Relaxed);
            *self.trip_reason.write() = Some(reason);
        }
    }

    /// Returns the human-readable reason why the kill switch was tripped, or `None` if healthy.
    pub fn trip_reason(&self) -> Option<String> {
        self.trip_reason.read().clone()
    }

    /// Returns the current number of consecutive fast reverts.
    #[inline(always)]
    pub fn consecutive_fast_reverts(&self) -> usize {
        self.consecutive_fast_reverts.load(Ordering::Acquire)
    }

    /// Returns the total number of times the kill switch was tripped across process lifetime.
    #[inline(always)]
    pub fn total_trips(&self) -> u64 {
        self.total_trips.load(Ordering::Relaxed)
    }

    /// Configured feed lag threshold in milliseconds.
    #[inline(always)]
    pub fn feed_lag_threshold_ms(&self) -> u64 {
        self.feed_lag_threshold_ms
    }

    /// Configured max consecutive fast reverts threshold.
    #[inline(always)]
    pub fn max_consecutive_reverts(&self) -> usize {
        self.max_consecutive_reverts
    }

    /// Resets the kill switch to healthy state.
    /// Clears the tripped flag, resets consecutive fast revert count, and removes the trip reason.
    pub fn reset(&self) {
        self.is_tripped.store(false, Ordering::SeqCst);
        self.consecutive_fast_reverts.store(0, Ordering::SeqCst);
        *self.trip_reason.write() = None;
    }
}
