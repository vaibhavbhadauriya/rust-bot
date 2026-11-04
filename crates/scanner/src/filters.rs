//! Hard reserve pre-filters and honeypot/security filters.
//! Authoritative source: ORIGINAL_REQUEST.md lines 20-23, PROJECT.md lines 48, 60.

use alloy_primitives::{address, Address, U256};
use flash_core::constants;
use flash_core::types::{Pool, PoolState};
use dashmap::DashSet;

/// Hard reserve pre-filter:
/// Discards any constant-product pool where decimal-adjusted reserve
/// R0 / 10^d0 < 1 or R1 / 10^d1 < 1.
///
/// Implemented using overflow-safe integer powers without floating-point division.
#[inline(always)]
pub fn filter_unit_reserves(r0: u128, d0: u8, r1: u128, d1: u8) -> bool {
    if r0 == 0 || r1 == 0 {
        return false;
    }
    let unit0 = match 10u128.checked_pow(d0 as u32) {
        Some(u) => u,
        None => return false,
    };
    let unit1 = match 10u128.checked_pow(d1 as u32) {
        Some(u) => u,
        None => return false,
    };
    r0 >= unit0 && r1 >= unit1
}

/// Honeypot violation classifications
#[derive(Debug, Clone, PartialEq)]
pub enum HoneypotViolation {
    ZeroAddress,
    IdenticalTokens,
    KnownMalicious(Address),
    FeeOnTransfer { fee_percent_bps: u32 },
    RebasingToken(Address),
    InsufficientReserves,
    SalmonellaPartialDelivery { emitted: U256, delivered: U256 },
    OriginSandwichTrap,
    GasPriceSandwichTrap,
    ExcessiveFeeOnTransfer { fee_percent: f64 },
}

/// Simulation report struct for integration with revm-trace Honeypot verification
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoneypotSimulationReport {
    pub token: Address,
    pub emitted_amount: U256,
    pub storage_delta: U256,
    pub used_origin_opcode: bool,
    pub used_gasprice_opcode: bool,
}

/// Inspects a revm-trace simulation report to detect Salmonella, Listeria, and MEV traps
pub fn inspect_honeypot_safety(report: &HoneypotSimulationReport) -> Result<(), HoneypotViolation> {
    // 1. Salmonella & Fee-on-Transfer Check: Token emits standard Transfer event but delivers partial balance
    if report.storage_delta < report.emitted_amount {
        // 20% Salmonella delivery check: storage_delta * 5 <= emitted_amount
        if report.storage_delta.checked_mul(U256::from(5)) <= Some(report.emitted_amount) {
            return Err(HoneypotViolation::SalmonellaPartialDelivery {
                emitted: report.emitted_amount,
                delivered: report.storage_delta,
            });
        } else if report.storage_delta.checked_mul(U256::from(10))
            < report.emitted_amount.checked_mul(U256::from(9))
        {
            // Fee on transfer exceeds 10% limit
            let fee_bps = ((report.emitted_amount.saturating_sub(report.storage_delta))
                .checked_mul(U256::from(10_000))
                .unwrap_or_default()
                / report.emitted_amount)
                .to::<u32>();
            let fee_pct = (fee_bps as f64) / 100.0;
            return Err(HoneypotViolation::ExcessiveFeeOnTransfer { fee_percent: fee_pct });
        }
    }

    // 2. MEV trap: ORIGIN opcode check
    if report.used_origin_opcode {
        return Err(HoneypotViolation::OriginSandwichTrap);
    }

    // 3. Sandwich trap: GASPRICE opcode check
    if report.used_gasprice_opcode {
        return Err(HoneypotViolation::GasPriceSandwichTrap);
    }

    Ok(())
}

/// Token Security & Honeypot Registry
#[derive(Debug, Clone, Default)]
pub struct HoneypotFilter {
    blacklist: DashSet<Address>,
    fee_on_transfer: DashSet<Address>,
    rebasing_tokens: DashSet<Address>,
    verified_safe: DashSet<Address>,
}

impl HoneypotFilter {
    pub fn new() -> Self {
        let filter = Self {
            blacklist: DashSet::new(),
            fee_on_transfer: DashSet::new(),
            rebasing_tokens: DashSet::new(),
            verified_safe: DashSet::new(),
        };
        filter.init_canonical_tokens();
        filter
    }

    fn init_canonical_tokens(&self) {
        // Register canonical verified safe tokens on Arbitrum
        self.verified_safe.insert(constants::arbitrum::WETH);
        self.verified_safe.insert(constants::arbitrum::USDC_NATIVE);
        self.verified_safe.insert(constants::arbitrum::USDC_BRIDGED);
        self.verified_safe.insert(constants::arbitrum::USDT);
        self.verified_safe.insert(constants::arbitrum::WBTC);
        self.verified_safe.insert(constants::arbitrum::ARB);

        // Register canonical verified safe tokens on Base
        self.verified_safe.insert(constants::base::WETH);
        self.verified_safe.insert(constants::base::USDC_NATIVE);
        self.verified_safe.insert(constants::base::USDBC_BRIDGED);
        self.verified_safe.insert(constants::base::CBBTC);
        self.verified_safe.insert(constants::base::AERO);

        // Dead / burn addresses blacklisted
        self.blacklist.insert(Address::ZERO);
        self.blacklist.insert(address!("000000000000000000000000000000000000dEaD"));
    }

    pub fn add_malicious_token(&self, token: Address) {
        self.blacklist.insert(token);
        self.verified_safe.remove(&token);
    }

    pub fn add_fee_on_transfer_token(&self, token: Address) {
        self.fee_on_transfer.insert(token);
    }

    pub fn add_rebasing_token(&self, token: Address) {
        self.rebasing_tokens.insert(token);
    }

    pub fn mark_verified_safe(&self, token: Address) {
        self.blacklist.remove(&token);
        self.verified_safe.insert(token);
    }

    /// Verifies that a single token passes honeypot and safety pre-filters
    pub fn check_token_safety(&self, token: Address) -> Result<(), HoneypotViolation> {
        if token == Address::ZERO {
            return Err(HoneypotViolation::ZeroAddress);
        }
        if self.blacklist.contains(&token) {
            return Err(HoneypotViolation::KnownMalicious(token));
        }
        if self.rebasing_tokens.contains(&token) {
            return Err(HoneypotViolation::RebasingToken(token));
        }
        if self.fee_on_transfer.contains(&token) {
            return Err(HoneypotViolation::FeeOnTransfer {
                fee_percent_bps: 1000,
            });
        }
        Ok(())
    }

    /// Verifies token pair safety (no self-loops, neither token blacklisted)
    pub fn check_pair_safety(&self, token0: Address, token1: Address) -> Result<(), HoneypotViolation> {
        if token0 == token1 {
            return Err(HoneypotViolation::IdenticalTokens);
        }
        self.check_token_safety(token0)?;
        self.check_token_safety(token1)?;
        Ok(())
    }

    /// Evaluates pool against reserve pre-filters and token honeypot filters
    pub fn check_pool(
        &self,
        pool: &Pool,
        token0_dec: u8,
        token1_dec: u8,
    ) -> Result<(), HoneypotViolation> {
        self.check_pair_safety(pool.token0, pool.token1)?;

        match &pool.state {
            PoolState::ConstantProduct { reserve0, reserve1 } => {
                if !filter_unit_reserves(*reserve0, token0_dec, *reserve1, token1_dec) {
                    return Err(HoneypotViolation::InsufficientReserves);
                }
            }
            PoolState::ConcentratedLiquidity {
                liquidity,
                sqrt_price_x96,
                ..
            } => {
                if *liquidity == 0 || sqrt_price_x96.is_zero() {
                    return Err(HoneypotViolation::InsufficientReserves);
                }
                let (x_virt, y_virt) =
                    flash_math::virtual_reserves_from_clmm(*liquidity, *sqrt_price_x96);
                if !filter_unit_reserves(x_virt, token0_dec, y_virt, token1_dec) {
                    return Err(HoneypotViolation::InsufficientReserves);
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_reserve_filter() {
        assert!(!filter_unit_reserves(999_999, 6, 1_000_000, 6));
        assert!(filter_unit_reserves(1_000_000, 6, 1_000_000, 6));
        assert!(!filter_unit_reserves(0, 0, 10, 0));
        assert!(filter_unit_reserves(1, 0, 1, 0));
        assert!(filter_unit_reserves(u128::MAX, 18, u128::MAX, 18));
    }

    #[test]
    fn test_honeypot_filter_checks() {
        let filter = HoneypotFilter::new();
        assert_eq!(
            filter.check_token_safety(Address::ZERO),
            Err(HoneypotViolation::ZeroAddress)
        );

        let bad_token = Address::from([0x99; 20]);
        filter.add_malicious_token(bad_token);
        assert_eq!(
            filter.check_token_safety(bad_token),
            Err(HoneypotViolation::KnownMalicious(bad_token))
        );

        let same = Address::from([0x11; 20]);
        assert_eq!(
            filter.check_pair_safety(same, same),
            Err(HoneypotViolation::IdenticalTokens)
        );
    }

    #[test]
    fn test_honeypot_high_supply_u256_no_truncation() {
        let large_base = U256::from(1) << 130; // > u128::MAX

        // Honest delivery (> 99.99%) must PASS
        let honest_report = HoneypotSimulationReport {
            token: Address::from([0xaa; 20]),
            emitted_amount: large_base + U256::from(100),
            storage_delta: large_base + U256::from(10),
            used_origin_opcode: false,
            used_gasprice_opcode: false,
        };
        assert!(inspect_honeypot_safety(&honest_report).is_ok());

        // Actual Salmonella (10% delivered) on U256 amount must FAIL
        let salmonella_report = HoneypotSimulationReport {
            token: Address::from([0xbb; 20]),
            emitted_amount: large_base,
            storage_delta: large_base / U256::from(10),
            used_origin_opcode: false,
            used_gasprice_opcode: false,
        };
        assert_eq!(
            inspect_honeypot_safety(&salmonella_report),
            Err(HoneypotViolation::SalmonellaPartialDelivery {
                emitted: large_base,
                delivered: large_base / U256::from(10),
            })
        );
    }
}
