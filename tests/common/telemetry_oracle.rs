//! Telemetry, Multi-EOA Pool & Kill Switch Oracle
//! Authoritative source: ORIGINAL_REQUEST.md lines 69-74, 90, and explorer_survey_3/handoff.md.

use alloy_primitives::Address;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

/// Feature 16: Multi-EOA Signer Pool
pub struct MockEoaPool {
    addresses: Vec<Address>,
    nonces: Vec<AtomicU64>,
    in_use: Vec<AtomicBool>,
    total_leases: AtomicUsize,
}

impl MockEoaPool {
    pub fn new(count: usize) -> Self {
        let mut addresses = Vec::with_capacity(count);
        let mut nonces = Vec::with_capacity(count);
        let mut in_use = Vec::with_capacity(count);

        for i in 0..count {
            let mut addr = [0u8; 20];
            addr[19] = (i + 1) as u8;
            addresses.push(Address::from(addr));
            nonces.push(AtomicU64::new(0));
            in_use.push(AtomicBool::new(false));
        }

        Self {
            addresses,
            nonces,
            in_use,
            total_leases: AtomicUsize::new(0),
        }
    }

    /// Acquires an available EOA lease without collision. Returns (signer_index, address, nonce).
    pub fn acquire_signer(&self) -> Option<(usize, Address, u64)> {
        for (i, in_flight) in self.in_use.iter().enumerate() {
            if in_flight
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                let nonce = self.nonces[i].fetch_add(1, Ordering::SeqCst);
                self.total_leases.fetch_add(1, Ordering::Relaxed);
                return Some((i, self.addresses[i], nonce));
            }
        }
        None // All signers currently leased
    }

    pub fn release_signer(&self, index: usize) {
        if index < self.in_use.len() {
            self.in_use[index].store(false, Ordering::SeqCst);
        }
    }

    pub fn current_nonce(&self, index: usize) -> u64 {
        self.nonces[index].load(Ordering::SeqCst)
    }
}

/// Feature 17: Microsecond Pipeline Telemetry & Kill Switch
#[derive(Debug, Default, Clone)]
pub struct PipelineTelemetry {
    pub feed_arrival_us: u64,
    pub graph_dfs_us: u64,
    pub sizing_math_us: u64,
    pub revm_sim_us: u64,
    pub tx_sign_us: u64,
}

impl PipelineTelemetry {
    pub fn math_plus_sim_latency_us(&self) -> u64 {
        self.sizing_math_us + self.revm_sim_us
    }

    pub fn total_e2e_latency_us(&self) -> u64 {
        self.feed_arrival_us
            + self.graph_dfs_us
            + self.sizing_math_us
            + self.revm_sim_us
            + self.tx_sign_us
    }
}

pub struct KillSwitchController {
    consecutive_fast_reverts: AtomicUsize,
    is_tripped: AtomicBool,
    trip_reason: Mutex<Option<String>>,
}

impl KillSwitchController {
    pub fn new() -> Self {
        Self {
            consecutive_fast_reverts: AtomicUsize::new(0),
            is_tripped: AtomicBool::new(false),
            trip_reason: Mutex::new(None),
        }
    }

    pub fn is_tripped(&self) -> bool {
        self.is_tripped.load(Ordering::SeqCst)
    }

    pub fn check_feed_lag(&self, lag_ms: u64) -> bool {
        if lag_ms > 100 {
            self.is_tripped.store(true, Ordering::SeqCst);
            *self.trip_reason.lock().unwrap() =
                Some(format!("Feed lag exceeded 100ms threshold: {}ms", lag_ms));
            true
        } else {
            false
        }
    }

    pub fn record_trade_result(&self, is_fast_revert: bool) -> bool {
        if is_fast_revert {
            let count = self.consecutive_fast_reverts.fetch_add(1, Ordering::SeqCst) + 1;
            if count >= 3 {
                self.is_tripped.store(true, Ordering::SeqCst);
                *self.trip_reason.lock().unwrap() =
                    Some(format!("Tripped on {} consecutive fast-reverts", count));
                return true;
            }
        } else {
            self.consecutive_fast_reverts.store(0, Ordering::SeqCst);
        }
        false
    }

    pub fn trip_reason(&self) -> Option<String> {
        self.trip_reason.lock().unwrap().clone()
    }
}
