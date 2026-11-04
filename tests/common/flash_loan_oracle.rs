//! Flash Loan Routing Oracle
//! Authoritative source: ORIGINAL_REQUEST.md lines 48-52 and explorer_survey_3/handoff.md.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashLoanProvider {
    MorphoBlue,  // Priority 1: 0% fee
    BalancerV3,  // Priority 2: 0% fee (transient unlock)
    UniswapV4,   // Priority 3: 0% fee (EIP-1153 transient accounting)
    AaveV3,      // Priority 4: 0.05% - 0.09% fee (strict last resort)
}

impl FlashLoanProvider {
    pub fn protocol_fee_pips(&self) -> u32 {
        match self {
            Self::MorphoBlue => 0,
            Self::BalancerV3 => 0,
            Self::UniswapV4 => 0,
            Self::AaveV3 => 500, // 0.05% = 500 PIPs
        }
    }

    pub fn callback_selector(&self) -> &'static str {
        match self {
            Self::MorphoBlue => "0x3a48e7e1", // onMorphoFlashLoan(uint256,bytes)
            Self::BalancerV3 => "0x98dd202a", // unlockCallback(bytes)
            Self::UniswapV4 => "0x98dd202a",  // unlockCallback(bytes)
            Self::AaveV3 => "0x9293e4e7",     // executeOperation(...)
        }
    }
}

pub struct ProviderLiquiditySnapshot {
    pub morpho_liquidity: u128,
    pub balancer_liquidity: u128,
    pub univ4_liquidity: u128,
    pub aave_liquidity: u128,
}

/// Cascades down the flash loan priority ladder to find the first zero-fee provider with sufficient liquidity.
pub fn route_flash_loan(
    required_amount: u128,
    snapshot: &ProviderLiquiditySnapshot,
) -> Option<(FlashLoanProvider, u32)> {
    if snapshot.morpho_liquidity >= required_amount {
        Some((FlashLoanProvider::MorphoBlue, FlashLoanProvider::MorphoBlue.protocol_fee_pips()))
    } else if snapshot.balancer_liquidity >= required_amount {
        Some((FlashLoanProvider::BalancerV3, FlashLoanProvider::BalancerV3.protocol_fee_pips()))
    } else if snapshot.univ4_liquidity >= required_amount {
        Some((FlashLoanProvider::UniswapV4, FlashLoanProvider::UniswapV4.protocol_fee_pips()))
    } else if snapshot.aave_liquidity >= required_amount {
        Some((FlashLoanProvider::AaveV3, FlashLoanProvider::AaveV3.protocol_fee_pips()))
    } else {
        None // Insufficient liquidity across all venues
    }
}
