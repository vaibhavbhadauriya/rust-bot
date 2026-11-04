//! High-Level Honeypot Detector & Token Verification API.
//! Authoritative source: PROJECT.md § Interface Contracts (line 94), ORIGINAL_REQUEST.md lines 23, 45, 89.

use crate::inspector::HoneypotViolation;
use alloy_primitives::{Address, U256, U512};
use flash_core::constants;
use std::collections::HashSet;

/// Interoperability report struct matching tests/common/honeypot_oracle.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoneypotSimulationReport {
    pub token: Address,
    pub emitted_amount: U256,
    pub storage_delta: U256,
    pub used_origin_opcode: bool,
    pub used_gasprice_opcode: bool,
}

/// Evaluates a HoneypotSimulationReport (fully compatible with scanner & test suites)
pub fn inspect_honeypot_safety(report: &HoneypotSimulationReport) -> Result<(), HoneypotViolation> {
    if report.storage_delta < report.emitted_amount {
        // 20% Salmonella delivery check: storage_delta * 5 <= emitted_amount (delivering <= 20%)
        // M1 fix: Use checked_mul map_or to eliminate Option comparison bug (None <= Some) on overflow
        if report.storage_delta.checked_mul(U256::from(5)).map_or(false, |val| val <= report.emitted_amount) {
            return Err(HoneypotViolation::SalmonellaPartialDelivery {
                emitted: report.emitted_amount,
                delivered: report.storage_delta,
            });
        }

        // Listeria check: fee on transfer > 10% (storage_delta * 10 < emitted_amount * 9)
        // M1 & M2 fix: Use U512 to prevent any overflow on extreme U256 values and avoid u128 truncation panic
        let ten_delta = U512::from(report.storage_delta) * U512::from(10);
        let nine_emitted = U512::from(report.emitted_amount) * U512::from(9);
        if ten_delta < nine_emitted {
            let diff = report.emitted_amount.saturating_sub(report.storage_delta);
            let fee_pct = if !report.emitted_amount.is_zero() {
                let scaled = (U512::from(diff) * U512::from(1_000_000))
                    / U512::from(report.emitted_amount);
                (scaled.to::<u64>() as f64) / 10_000.0
            } else {
                0.0
            };
            return Err(HoneypotViolation::ExcessiveFeeOnTransfer { fee_percent: fee_pct });
        }
    }

    if report.used_origin_opcode {
        return Err(HoneypotViolation::OriginSandwichTrap);
    }
    if report.used_gasprice_opcode {
        return Err(HoneypotViolation::GasPriceSandwichTrap);
    }

    Ok(())
}

/// Fast pre-whitelist check for canonical Arbitrum and Base tokens (<1µs latency)
#[inline(always)]
pub fn is_canonical_safe_token(token: Address) -> bool {
    // Arbitrum canonical tokens
    if token == constants::arbitrum::WETH
        || token == constants::arbitrum::USDC_NATIVE
        || token == constants::arbitrum::USDC_BRIDGED
        || token == constants::arbitrum::USDT
        || token == constants::arbitrum::WBTC
        || token == constants::arbitrum::ARB
    {
        return true;
    }

    // Base canonical tokens
    if token == constants::base::WETH
        || token == constants::base::USDC_NATIVE
        || token == constants::base::USDBC_BRIDGED
        || token == constants::base::CBBTC
        || token == constants::base::AERO
    {
        return true;
    }

    false
}

/// Token Honeypot Security Registry
#[derive(Debug, Clone, Default)]
pub struct HoneypotDetector {
    blacklist: HashSet<Address>,
    verified_safe: HashSet<Address>,
}

impl HoneypotDetector {
    pub fn new() -> Self {
        let mut detector = Self::default();
        detector.blacklist.insert(Address::ZERO);
        detector
    }

    pub fn mark_safe(&mut self, token: Address) {
        self.verified_safe.insert(token);
        self.blacklist.remove(&token);
    }

    pub fn mark_malicious(&mut self, token: Address) {
        self.blacklist.insert(token);
        self.verified_safe.remove(&token);
    }

    pub fn is_known_safe(&self, token: Address) -> bool {
        is_canonical_safe_token(token) || self.verified_safe.contains(&token)
    }

    pub fn is_known_malicious(&self, token: Address) -> bool {
        token.is_zero() || self.blacklist.contains(&token)
    }

    /// Verifies token integrity as specified in PROJECT.md:
    /// `verify_token_integrity(token: TokenAddress) -> Result<bool, HoneypotViolation>`
    pub fn verify_token_integrity(&self, token: Address) -> Result<bool, HoneypotViolation> {
        if is_canonical_safe_token(token) || self.verified_safe.contains(&token) {
            return Ok(true);
        }
        if self.blacklist.contains(&token) || token.is_zero() {
            return Err(HoneypotViolation::BlacklistedRecipient);
        }

        Ok(true)
    }
}

/// Direct standalone API for strategy & scanner integration
#[inline]
pub fn verify_token_integrity(token: Address) -> Result<bool, HoneypotViolation> {
    if is_canonical_safe_token(token) {
        return Ok(true);
    }
    if token.is_zero() {
        return Err(HoneypotViolation::BlacklistedRecipient);
    }
    Ok(true)
}
