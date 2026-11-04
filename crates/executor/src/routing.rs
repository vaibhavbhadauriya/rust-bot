//! Zero-Fee Singleton Flash Loan Routing Engine
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 48-52, TEST_INFRA.md line 23,
//! and tests/common/flash_loan_oracle.rs.
//!
//! Priority Hierarchy:
//! 1. Morpho Blue (0% fee singleton flash loan on Arbitrum & Base)
//! 2. Balancer V3 Vault (0% fee singleton flash loan via transient unlock)
//! 3. Uniswap V4 PoolManager (0% fee singleton flash loan via EIP-1153 unlock)
//! 4. Aave V3 Pool fallback (0.05% on Arbitrum, 0.09% on Base — strict last resort)

use alloy_primitives::{address, Address, U256};
use flash_core::constants::{arbitrum, base};
use flash_core::types::Chain;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Flash loan callback function selectors
pub const MORPHO_BLUE_CALLBACK_SELECTOR: [u8; 4] = [0x3a, 0x48, 0xe7, 0xe1]; // onMorphoFlashLoan(uint256,bytes)
pub const BALANCER_V3_CALLBACK_SELECTOR: [u8; 4] = [0x98, 0xdd, 0x20, 0x2a]; // unlockCallback(bytes)
pub const UNISWAP_V4_CALLBACK_SELECTOR: [u8; 4] = [0x98, 0xdd, 0x20, 0x2a];  // unlockCallback(bytes)
pub const AAVE_V3_CALLBACK_SELECTOR: [u8; 4] = [0x92, 0x93, 0xe4, 0xe7];     // executeOperation(address,uint256,uint256,address,bytes)

/// Flash loan initiation function selectors
pub const MORPHO_FLASH_LOAN_SELECTOR: [u8; 4] = [0x43, 0x98, 0x96, 0x76]; // flashLoan(address,uint256,bytes)
pub const BALANCER_UNLOCK_SELECTOR: [u8; 4] = [0x91, 0x10, 0x8b, 0x66];   // unlock(bytes)
pub const UNISWAP_V4_UNLOCK_SELECTOR: [u8; 4] = [0x91, 0x10, 0x8b, 0x66]; // unlock(bytes)
pub const AAVE_FLASH_LOAN_SIMPLE_SELECTOR: [u8; 4] = [0x42, 0x1e, 0x18, 0x1c]; // flashLoanSimple(address,address,uint256,bytes,uint16)

/// Deterministic Uniswap V4 PoolManager CREATE2 singleton address
pub const UNISWAP_V4_POOL_MANAGER: Address = address!("000000000004444c5dc75cB358380D2e3dE08A90");

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RoutingError {
    #[error("Insufficient liquidity across all flash loan venues for required amount {required}: morpho={morpho}, balancer={balancer}, univ4={univ4}, aave={aave}")]
    InsufficientLiquidity {
        required: u128,
        morpho: u128,
        balancer: u128,
        univ4: u128,
        aave: u128,
    },
    #[error("Unsupported token for flash loan: {0}")]
    UnsupportedToken(Address),
}

/// Available flash loan providers ordered by fee priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FlashLoanProvider {
    MorphoBlue,  // Priority 1: 0% fee
    BalancerV3,  // Priority 2: 0% fee (transient unlock)
    UniswapV4,   // Priority 3: 0% fee (EIP-1153 transient accounting)
    AaveV3,      // Priority 4: 0.05% - 0.09% fee (strict last resort)
}

impl FlashLoanProvider {
    #[inline(always)]
    pub const fn priority(&self) -> u8 {
        match self {
            Self::MorphoBlue => 1,
            Self::BalancerV3 => 2,
            Self::UniswapV4 => 3,
            Self::AaveV3 => 4,
        }
    }

    #[inline(always)]
    pub const fn is_zero_fee(&self) -> bool {
        matches!(self, Self::MorphoBlue | Self::BalancerV3 | Self::UniswapV4)
    }

    /// Protocol fee in PIPs (parts-per-million, 10^6 scale)
    /// Arbitrum: Aave charges 0.05% = 500 PIPs
    /// Base: Aave charges 0.09% = 900 PIPs
    pub fn protocol_fee_pips(&self) -> u32 {
        match self {
            Self::MorphoBlue => 0,
            Self::BalancerV3 => 0,
            Self::UniswapV4 => 0,
            Self::AaveV3 => 500, // Default 500 PIPs for test oracle compatibility
        }
    }

    /// Chain-aware protocol fee in PIPs (10^6 scale)
    /// Arbitrum One (42161): Aave V3 charges 0.05% = 500 PIPs
    /// Base (8453): Aave V3 charges 0.09% = 900 PIPs
    #[inline(always)]
    pub fn fee_pips(&self, chain_id: u64) -> u32 {
        match self {
            Self::MorphoBlue | Self::BalancerV3 | Self::UniswapV4 => 0,
            Self::AaveV3 => match chain_id {
                base::CHAIN_ID => 900,
                _ => 500, // arbitrum::CHAIN_ID (42161) and default
            },
        }
    }

    /// Chain-aware protocol fee in PIPs
    pub fn protocol_fee_pips_for_chain(&self, chain: Chain) -> u32 {
        self.fee_pips(chain.id())
    }

    /// Callback selector string (used in E2E tests)
    pub const fn callback_selector(&self) -> &'static str {
        match self {
            Self::MorphoBlue => "0x3a48e7e1",
            Self::BalancerV3 => "0x98dd202a",
            Self::UniswapV4 => "0x98dd202a",
            Self::AaveV3 => "0x9293e4e7",
        }
    }

    /// 4-byte callback selector
    pub const fn callback_selector_bytes(&self) -> [u8; 4] {
        match self {
            Self::MorphoBlue => MORPHO_BLUE_CALLBACK_SELECTOR,
            Self::BalancerV3 => BALANCER_V3_CALLBACK_SELECTOR,
            Self::UniswapV4 => UNISWAP_V4_CALLBACK_SELECTOR,
            Self::AaveV3 => AAVE_V3_CALLBACK_SELECTOR,
        }
    }

    /// Canonical protocol singleton address on given chain
    pub fn contract_address(&self, chain: Chain) -> Address {
        match (self, chain) {
            (Self::MorphoBlue, Chain::ArbitrumOne) => arbitrum::MORPHO_BLUE,
            (Self::MorphoBlue, Chain::Base) => base::MORPHO_BLUE,
            (Self::BalancerV3, Chain::ArbitrumOne) => arbitrum::BALANCER_V3_VAULT,
            (Self::BalancerV3, Chain::Base) => base::BALANCER_V3_VAULT,
            (Self::UniswapV4, _) => UNISWAP_V4_POOL_MANAGER,
            (Self::AaveV3, Chain::ArbitrumOne) => arbitrum::AAVE_V3_POOL,
            (Self::AaveV3, Chain::Base) => base::AAVE_V3_POOL,
        }
    }
}

/// Instantaneous snapshot of available borrowable liquidity across flash loan venues
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderLiquiditySnapshot {
    pub morpho_liquidity: u128,
    pub balancer_liquidity: u128,
    pub univ4_liquidity: u128,
    pub aave_liquidity: u128,
}

impl ProviderLiquiditySnapshot {
    pub const fn new(
        morpho: u128,
        balancer: u128,
        univ4: u128,
        aave: u128,
    ) -> Self {
        Self {
            morpho_liquidity: morpho,
            balancer_liquidity: balancer,
            univ4_liquidity: univ4,
            aave_liquidity: aave,
        }
    }
}

/// Selected flash loan route details
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlashLoanRoute {
    pub provider: FlashLoanProvider,
    pub provider_address: Address,
    pub token: Address,
    pub amount: u128,
    pub fee_pips: u32,
    pub fee_amount: u128,
    pub callback_selector: [u8; 4],
}

/// Core priority router cascading down the flash loan priority ladder with explicit chain_id
pub fn route_flash_loan_for_chain(
    required_amount: u128,
    snapshot: &ProviderLiquiditySnapshot,
    chain_id: u64,
) -> Option<(FlashLoanProvider, u32)> {
    if snapshot.morpho_liquidity >= required_amount {
        Some((FlashLoanProvider::MorphoBlue, 0))
    } else if snapshot.balancer_liquidity >= required_amount {
        Some((FlashLoanProvider::BalancerV3, 0))
    } else if snapshot.univ4_liquidity >= required_amount {
        Some((FlashLoanProvider::UniswapV4, 0))
    } else if snapshot.aave_liquidity >= required_amount {
        Some((FlashLoanProvider::AaveV3, FlashLoanProvider::AaveV3.fee_pips(chain_id)))
    } else {
        None
    }
}

/// Core priority router cascading down the flash loan priority ladder.
/// Matches tests/common/flash_loan_oracle.rs interface verbatim. Defaults to Arbitrum One (42161).
pub fn route_flash_loan(
    required_amount: u128,
    snapshot: &ProviderLiquiditySnapshot,
) -> Option<(FlashLoanProvider, u32)> {
    route_flash_loan_for_chain(required_amount, snapshot, arbitrum::CHAIN_ID)
}

/// Chain-aware optimal flash loan route selector with fee calculation
pub fn select_optimal_flash_route(
    chain: Chain,
    token: Address,
    required_amount: u128,
    snapshot: &ProviderLiquiditySnapshot,
) -> Result<FlashLoanRoute, RoutingError> {
    let (provider, fee_pips) = if snapshot.morpho_liquidity >= required_amount {
        (FlashLoanProvider::MorphoBlue, 0)
    } else if snapshot.balancer_liquidity >= required_amount {
        (FlashLoanProvider::BalancerV3, 0)
    } else if snapshot.univ4_liquidity >= required_amount {
        (FlashLoanProvider::UniswapV4, 0)
    } else if snapshot.aave_liquidity >= required_amount {
        (FlashLoanProvider::AaveV3, FlashLoanProvider::AaveV3.protocol_fee_pips_for_chain(chain))
    } else {
        return Err(RoutingError::InsufficientLiquidity {
            required: required_amount,
            morpho: snapshot.morpho_liquidity,
            balancer: snapshot.balancer_liquidity,
            univ4: snapshot.univ4_liquidity,
            aave: snapshot.aave_liquidity,
        });
    };

    let fee_amount = if fee_pips == 0 {
        0
    } else {
        let raw = (U256::from(required_amount) * U256::from(fee_pips) + U256::from(999_999)) / U256::from(1_000_000);
        raw.to::<u128>()
    };

    Ok(FlashLoanRoute {
        provider,
        provider_address: provider.contract_address(chain),
        token,
        amount: required_amount,
        fee_pips,
        fee_amount,
        callback_selector: provider.callback_selector_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_hierarchy_ordering() {
        assert_eq!(FlashLoanProvider::MorphoBlue.priority(), 1);
        assert_eq!(FlashLoanProvider::BalancerV3.priority(), 2);
        assert_eq!(FlashLoanProvider::UniswapV4.priority(), 3);
        assert_eq!(FlashLoanProvider::AaveV3.priority(), 4);
    }

    #[test]
    fn test_zero_fee_properties() {
        assert!(FlashLoanProvider::MorphoBlue.is_zero_fee());
        assert!(FlashLoanProvider::BalancerV3.is_zero_fee());
        assert!(FlashLoanProvider::UniswapV4.is_zero_fee());
        assert!(!FlashLoanProvider::AaveV3.is_zero_fee());
    }

    #[test]
    fn test_chain_specific_aave_fee() {
        assert_eq!(FlashLoanProvider::AaveV3.protocol_fee_pips_for_chain(Chain::ArbitrumOne), 500);
        assert_eq!(FlashLoanProvider::AaveV3.protocol_fee_pips_for_chain(Chain::Base), 900);
        assert_eq!(FlashLoanProvider::AaveV3.fee_pips(42161), 500);
        assert_eq!(FlashLoanProvider::AaveV3.fee_pips(8453), 900);
        assert_eq!(FlashLoanProvider::MorphoBlue.fee_pips(8453), 0);
    }

    #[test]
    fn test_route_flash_loan_for_chain() {
        let snapshot = ProviderLiquiditySnapshot {
            morpho_liquidity: 100,
            balancer_liquidity: 500,
            univ4_liquidity: 1000,
            aave_liquidity: 2000,
        };

        let (p_arb, f_arb) = route_flash_loan_for_chain(1500, &snapshot, 42161).unwrap();
        assert_eq!(p_arb, FlashLoanProvider::AaveV3);
        assert_eq!(f_arb, 500);

        let (p_base, f_base) = route_flash_loan_for_chain(1500, &snapshot, 8453).unwrap();
        assert_eq!(p_base, FlashLoanProvider::AaveV3);
        assert_eq!(f_base, 900);
    }

    #[test]
    fn test_route_selection_cascading() {
        let snapshot = ProviderLiquiditySnapshot {
            morpho_liquidity: 100,
            balancer_liquidity: 500,
            univ4_liquidity: 1000,
            aave_liquidity: 2000,
        };

        // Within Morpho
        let (p1, f1) = route_flash_loan(50, &snapshot).unwrap();
        assert_eq!(p1, FlashLoanProvider::MorphoBlue);
        assert_eq!(f1, 0);

        // Exceeds Morpho, fits Balancer
        let (p2, f2) = route_flash_loan(200, &snapshot).unwrap();
        assert_eq!(p2, FlashLoanProvider::BalancerV3);
        assert_eq!(f2, 0);

        // Exceeds Balancer, fits UniV4
        let (p3, f3) = route_flash_loan(800, &snapshot).unwrap();
        assert_eq!(p3, FlashLoanProvider::UniswapV4);
        assert_eq!(f3, 0);

        // Exceeds UniV4, fits Aave
        let (p4, f4) = route_flash_loan(1500, &snapshot).unwrap();
        assert_eq!(p4, FlashLoanProvider::AaveV3);
        assert_eq!(f4, 500);

        // Exceeds all
        assert!(route_flash_loan(3000, &snapshot).is_none());
    }
}
