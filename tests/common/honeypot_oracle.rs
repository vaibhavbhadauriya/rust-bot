//! Honeypot Defense Oracle (Salmonella / Listeria Detector)
//! Authoritative source: ORIGINAL_REQUEST.md lines 43-46, 89, and explorer_survey_3/handoff.md.

use alloy_primitives::{Address, U256};

#[derive(Debug, Clone, PartialEq)]
pub enum HoneypotViolation {
    SalmonellaPartialDelivery { emitted: U256, delivered: U256 },
    OriginSandwichTrap,
    GasPriceSandwichTrap,
    ExcessiveFeeOnTransfer { fee_percent: f64 },
}

pub struct HoneypotSimulationReport {
    pub token: Address,
    pub emitted_amount: U256,
    pub storage_delta: U256,
    pub used_origin_opcode: bool,
    pub used_gasprice_opcode: bool,
}

pub fn inspect_honeypot_safety(report: &HoneypotSimulationReport) -> Result<(), HoneypotViolation> {
    // 1. Salmonella Check: Token emits standard Transfer event but delivers partial balance
    if report.storage_delta < report.emitted_amount {
        let delivered_ratio = if report.emitted_amount.is_zero() {
            0.0
        } else {
            (report.storage_delta.to::<u128>() as f64) / (report.emitted_amount.to::<u128>() as f64)
        };
        // Salmonella delivers negligible balance (e.g. 10% in spec AC3)
        if delivered_ratio <= 0.20 {
            return Err(HoneypotViolation::SalmonellaPartialDelivery {
                emitted: report.emitted_amount,
                delivered: report.storage_delta,
            });
        } else if delivered_ratio < 0.90 {
            // Fee on transfer exceeds 10% limit
            let fee_pct = (1.0 - delivered_ratio) * 100.0;
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
