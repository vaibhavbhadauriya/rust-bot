use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};

// Re-export core alloy primitives for all downstream crates
pub use alloy_primitives::{
    address, b256, hex, uint, Bytes, FixedBytes, B256, I256,
};

pub type TokenAddress = Address;
pub type PoolId = Address;

/// Fee represented in parts-per-million (PIPs, 10^6 scale).
/// 0.30% = 3,000 PIPs
/// 0.05% = 500 PIPs
/// 0.01% = 100 PIPs
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct FeePips(pub u32);

impl FeePips {
    pub const PIP_DENOMINATOR: u32 = 1_000_000;

    #[inline(always)]
    pub const fn from_pips(pips: u32) -> Self {
        Self(pips)
    }

    #[inline(always)]
    pub const fn from_bps(bps: u32) -> Self {
        Self(bps * 100)
    }

    #[inline(always)]
    pub fn from_percentage(pct: f64) -> Self {
        Self((pct * 10_000.0).round() as u32)
    }

    #[inline(always)]
    pub const fn as_pips(&self) -> u32 {
        self.0
    }

    /// Complement factor \gamma = 1 - fee
    #[inline(always)]
    pub const fn gamma_pips(&self) -> u32 {
        Self::PIP_DENOMINATOR.saturating_sub(self.0)
    }

    #[inline(always)]
    pub fn as_f64(&self) -> f64 {
        self.0 as f64 / Self::PIP_DENOMINATOR as f64
    }

    #[inline(always)]
    pub fn gamma_f64(&self) -> f64 {
        1.0 - self.as_f64()
    }
}

impl From<u32> for FeePips {
    fn from(val: u32) -> Self {
        Self(val)
    }
}

impl From<FeePips> for u32 {
    fn from(fee: FeePips) -> Self {
        fee.0
    }
}

/// Target blockchain identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u64)]
pub enum Chain {
    ArbitrumOne = 42161,
    Base = 8453,
}

impl Chain {
    pub const fn id(&self) -> u64 {
        *self as u64
    }

    pub const fn name(&self) -> &'static str {
        match self {
            Self::ArbitrumOne => "Arbitrum One",
            Self::Base => "Base",
        }
    }

    pub fn from_id(id: u64) -> Option<Self> {
        match id {
            42161 => Some(Self::ArbitrumOne),
            8453 => Some(Self::Base),
            _ => None,
        }
    }
}

/// DEX protocol venues supported across Arbitrum One and Base
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Venue {
    UniswapV2,
    UniswapV3,
    UniswapV4,
    SushiSwap,
    SushiSwapV2,
    CamelotV2,
    CamelotV3,
    AerodromeV2,
    AerodromeSlipstream,
    BalancerV3,
    MorphoBlue,
    AaveV3,
}

impl Venue {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::UniswapV2 => "Uniswap V2",
            Self::UniswapV3 => "Uniswap V3",
            Self::UniswapV4 => "Uniswap V4",
            Self::SushiSwap | Self::SushiSwapV2 => "SushiSwap",
            Self::CamelotV2 => "Camelot V2",
            Self::CamelotV3 => "Camelot V3",
            Self::AerodromeV2 => "Aerodrome V2",
            Self::AerodromeSlipstream => "Aerodrome Slipstream",
            Self::BalancerV3 => "Balancer V3",
            Self::MorphoBlue => "Morpho Blue",
            Self::AaveV3 => "Aave V3",
        }
    }

    pub const fn is_clmm(&self) -> bool {
        matches!(
            self,
            Self::UniswapV3 | Self::UniswapV4 | Self::CamelotV3 | Self::AerodromeSlipstream
        )
    }

    pub const fn is_cpmm(&self) -> bool {
        matches!(
            self,
            Self::UniswapV2 | Self::SushiSwap | Self::SushiSwapV2 | Self::CamelotV2 | Self::AerodromeV2
        )
    }

    pub const fn is_flash_loan_provider(&self) -> bool {
        matches!(
            self,
            Self::MorphoBlue | Self::BalancerV3 | Self::UniswapV4 | Self::AaveV3
        )
    }

    /// Flash loan priority order (1 = highest priority, 0% fee)
    pub const fn flash_loan_priority(&self) -> Option<u8> {
        match self {
            Self::MorphoBlue => Some(1),
            Self::BalancerV3 => Some(2),
            Self::UniswapV4 => Some(3),
            Self::AaveV3 => Some(4),
            _ => None,
        }
    }
}

/// ERC-20 Token definition
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Token {
    pub address: Address,
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    pub chain_id: u64,
}

impl Token {
    pub fn new(
        address: Address,
        symbol: impl Into<String>,
        name: impl Into<String>,
        decimals: u8,
        chain_id: u64,
    ) -> Self {
        Self {
            address,
            symbol: symbol.into(),
            name: name.into(),
            decimals,
            chain_id,
        }
    }

    #[inline(always)]
    pub fn unit(&self) -> u128 {
        10u128.pow(self.decimals as u32)
    }

    /// Check if raw reserve satisfies the unit reserve filter (R / 10^d >= 1)
    #[inline(always)]
    pub fn is_unit_reserve(&self, raw_reserve: u128) -> bool {
        raw_reserve >= self.unit()
    }

    pub fn format_units_str(&self, raw: U256) -> String {
        let dec = self.decimals as usize;
        let s = raw.to_string();
        if dec == 0 {
            return s;
        }
        if s.len() <= dec {
            let zeros = "0".repeat(dec - s.len());
            format!("0.{}{}", zeros, s)
        } else {
            let split_idx = s.len() - dec;
            format!("{}.{}", &s[..split_idx], &s[split_idx..])
        }
    }

    pub fn format_units(&self, raw: U256) -> f64 {
        self.format_units_str(raw).parse::<f64>().unwrap_or(0.0)
    }

    pub fn parse_units_str(&self, s: &str) -> U256 {
        let s = s.trim();
        if s.starts_with('-') || s.is_empty() {
            return U256::ZERO;
        }
        let s = s.strip_prefix('+').unwrap_or(s);
        let dec = self.decimals as usize;
        let mut parts = s.split('.');
        let int_str = parts.next().unwrap_or("0");
        let frac_str = parts.next().unwrap_or("");
        if parts.next().is_some() {
            return U256::ZERO;
        }

        let int_str = if int_str.is_empty() { "0" } else { int_str };
        let int_val = match U256::from_str_radix(int_str, 10) {
            Ok(v) => v,
            Err(_) => return U256::ZERO,
        };

        let scale = match U256::from(10u64).checked_pow(U256::from(self.decimals)) {
            Some(s) => s,
            None => return U256::ZERO,
        };
        let int_scaled = match int_val.checked_mul(scale) {
            Some(v) => v,
            None => return U256::MAX,
        };

        let frac_val = if dec == 0 || frac_str.is_empty() {
            U256::ZERO
        } else if frac_str.len() >= dec {
            match U256::from_str_radix(&frac_str[..dec], 10) {
                Ok(v) => v,
                Err(_) => return U256::ZERO,
            }
        } else {
            let zeros = "0".repeat(dec - frac_str.len());
            let combined = format!("{}{}", frac_str, zeros);
            match U256::from_str_radix(&combined, 10) {
                Ok(v) => v,
                Err(_) => return U256::ZERO,
            }
        };

        int_scaled.saturating_add(frac_val)
    }

    pub fn parse_units(&self, amount: f64) -> U256 {
        if amount <= 0.0 || !amount.is_finite() {
            return U256::ZERO;
        }
        let dec = self.decimals as usize;
        self.parse_units_str(&format!("{:.prec$}", amount, prec = dec.min(18)))
    }
}

/// Packed 128-bit reserves aligned to 32 bytes for cache efficiency
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Reserve {
    pub reserve0: u128,
    pub reserve1: u128,
    pub block_timestamp_last: u32,
}

impl Reserve {
    #[inline(always)]
    pub const fn new(reserve0: u128, reserve1: u128) -> Self {
        Self {
            reserve0,
            reserve1,
            block_timestamp_last: 0,
        }
    }

    /// Hard pre-filter: verify decimal-adjusted reserves R0 / 10^d0 >= 1 and R1 / 10^d1 >= 1
    #[inline(always)]
    pub fn is_valid_unit(&self, dec0: u8, dec1: u8) -> bool {
        let min0 = 10u128.checked_pow(dec0 as u32).unwrap_or(u128::MAX);
        let min1 = 10u128.checked_pow(dec1 as u32).unwrap_or(u128::MAX);
        self.reserve0 >= min0 && self.reserve1 >= min1
    }
}

/// Simplified pool reserves for analytical math calculations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PoolReserves {
    pub reserve_in: u128,
    pub reserve_out: u128,
    pub fee_pips: u32,
}

impl PoolReserves {
    #[inline(always)]
    pub const fn new(reserve_in: u128, reserve_out: u128, fee_pips: u32) -> Self {
        Self {
            reserve_in,
            reserve_out,
            fee_pips,
        }
    }
}

/// Internal state of a liquidity pool
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PoolState {
    ConstantProduct {
        reserve0: u128,
        reserve1: u128,
    },
    ConcentratedLiquidity {
        liquidity: u128,
        sqrt_price_x96: U256,
        current_tick: i32,
        tick_spacing: i32,
    },
}

/// Canonical pool representation across all venues
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pool {
    pub id: Address,
    pub venue: Venue,
    pub chain_id: u64,
    pub token0: Address,
    pub token1: Address,
    pub fee: FeePips,
    pub state: PoolState,
}

impl Pool {
    pub fn get_reserves(&self) -> Option<(u128, u128)> {
        match &self.state {
            PoolState::ConstantProduct { reserve0, reserve1 } => Some((*reserve0, *reserve1)),
            PoolState::ConcentratedLiquidity { .. } => None,
        }
    }

    pub fn other_token(&self, token: Address) -> Option<Address> {
        if self.token0 == token {
            Some(self.token1)
        } else if self.token1 == token {
            Some(self.token0)
        } else {
            None
        }
    }

    pub fn reserves_for_tokens(&self, token_in: Address, token_out: Address) -> Option<(u128, u128)> {
        match &self.state {
            PoolState::ConstantProduct { reserve0, reserve1 } => {
                if self.token0 == token_in && self.token1 == token_out {
                    Some((*reserve0, *reserve1))
                } else if self.token1 == token_in && self.token0 == token_out {
                    Some((*reserve1, *reserve0))
                } else {
                    None
                }
            }
            PoolState::ConcentratedLiquidity { .. } => None,
        }
    }
}

/// Runtime execution mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionMode {
    Shadow,
    Live,
}

impl ExecutionMode {
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live)
    }

    pub fn from_str(s: &str) -> Self {
        if s.eq_ignore_ascii_case("live") {
            Self::Live
        } else {
            Self::Shadow
        }
    }
}

/// Dynamic gas prices and L1 calldata fee state for Arbitrum One and Base
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GasPrices {
    /// L2 base fee in wei
    pub l2_base_fee_wei: u128,
    /// L2 recommended priority fee in wei
    pub l2_priority_fee_wei: u128,
    /// L1 data posting fee in wei for a standard ~124-byte transaction
    pub l1_data_fee_wei: u128,
}

impl GasPrices {
    pub fn default_for_chain(chain: Chain) -> Self {
        match chain {
            Chain::ArbitrumOne => Self {
                l2_base_fee_wei: 100_000_000,         // 0.1 Gwei
                l2_priority_fee_wei: 10_000_000,      // 0.01 Gwei
                l1_data_fee_wei: 400_000_000_000_000, // ~0.0004 ETH (~$1.00)
            },
            Chain::Base => Self {
                l2_base_fee_wei: 50_000_000,          // 0.05 Gwei
                l2_priority_fee_wei: 50_000_000,      // 0.05 Gwei
                l1_data_fee_wei: 300_000_000_000_000, // ~0.0003 ETH (~$0.75)
            },
        }
    }

    pub fn total_gas_cost_wei(&self, l2_gas_used: u64) -> U256 {
        let l2_gas_price = self.l2_base_fee_wei.saturating_add(self.l2_priority_fee_wei);
        let l2_cost = U256::from(l2_gas_used as u128).saturating_mul(U256::from(l2_gas_price));
        l2_cost.saturating_add(U256::from(self.l1_data_fee_wei))
    }
}

impl Default for GasPrices {
    fn default() -> Self {
        Self::default_for_chain(Chain::ArbitrumOne)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_units_zero_decimals_no_trailing_dot() {
        let token0 = Token::new(Address::ZERO, "D0", "Zero Decimals", 0, 1);
        let raw = U256::from(42);
        let formatted_str = token0.format_units_str(raw);
        assert_eq!(formatted_str, "42");
        assert!(!formatted_str.contains('.'));
        assert_eq!(token0.format_units(raw), 42.0);
    }

    #[test]
    fn test_format_units_str_and_format_units_with_decimals() {
        let usdc = Token::new(Address::ZERO, "USDC", "USDC", 6, 1);
        assert_eq!(usdc.format_units_str(U256::from(1_500_000)), "1.500000");
        assert_eq!(usdc.format_units(U256::from(1_500_000)), 1.5);

        assert_eq!(usdc.format_units_str(U256::from(50_000)), "0.050000");
        assert_eq!(usdc.format_units(U256::from(50_000)), 0.05);

        assert_eq!(usdc.format_units_str(U256::ZERO), "0.000000");
        assert_eq!(usdc.format_units(U256::ZERO), 0.0);
    }

    #[test]
    fn test_parse_units_str_exact_integers_and_fractions() {
        let weth = Token::new(Address::ZERO, "WETH", "WETH", 18, 1);
        assert_eq!(
            weth.parse_units_str("1.0"),
            U256::from(1_000_000_000_000_000_000u64)
        );
        assert_eq!(
            weth.parse_units_str("1"),
            U256::from(1_000_000_000_000_000_000u64)
        );
        assert_eq!(
            weth.parse_units_str("0.5"),
            U256::from(500_000_000_000_000_000u64)
        );
        assert_eq!(
            weth.parse_units_str(".25"),
            U256::from(250_000_000_000_000_000u64)
        );

        let usdc = Token::new(Address::ZERO, "USDC", "USDC", 6, 1);
        assert_eq!(usdc.parse_units_str("123.456789"), U256::from(123_456_789));
        // Truncate excess decimal places beyond precision
        assert_eq!(usdc.parse_units_str("123.456789999"), U256::from(123_456_789));

        // Zero decimals token
        let token0 = Token::new(Address::ZERO, "D0", "Zero Decimals", 0, 1);
        assert_eq!(token0.parse_units_str("12345"), U256::from(12345));
        assert_eq!(token0.parse_units_str("12345.67"), U256::from(12345));
    }

    #[test]
    fn test_parse_units_str_invalid_inputs() {
        let usdc = Token::new(Address::ZERO, "USDC", "USDC", 6, 1);
        assert_eq!(usdc.parse_units_str(""), U256::ZERO);
        assert_eq!(usdc.parse_units_str("-10.5"), U256::ZERO);
        assert_eq!(usdc.parse_units_str("1.2.3"), U256::ZERO);
        assert_eq!(usdc.parse_units_str("invalid"), U256::ZERO);
    }

    #[test]
    fn test_parse_units_f64_wrapper() {
        let usdc = Token::new(Address::ZERO, "USDC", "USDC", 6, 1);
        assert_eq!(usdc.parse_units(100.5), U256::from(100_500_000));
        assert_eq!(usdc.parse_units(-5.0), U256::ZERO);
        assert_eq!(usdc.parse_units(f64::NAN), U256::ZERO);
    }

    #[test]
    fn test_gas_prices_defaults_and_total_cost() {
        let arb_prices = GasPrices::default_for_chain(Chain::ArbitrumOne);
        assert_eq!(arb_prices.l2_base_fee_wei, 100_000_000);
        assert_eq!(arb_prices.l2_priority_fee_wei, 10_000_000);
        assert_eq!(arb_prices.l1_data_fee_wei, 400_000_000_000_000);

        // 200,000 gas at 0.11 Gwei (110_000_000 wei) = 22,000,000,000,000 wei L2 cost
        // + 400,000,000,000,000 wei L1 fee = 422,000,000,000,000 wei
        let total = arb_prices.total_gas_cost_wei(200_000);
        assert_eq!(total, U256::from(422_000_000_000_000u64));

        let base_prices = GasPrices::default_for_chain(Chain::Base);
        assert_eq!(base_prices.l2_base_fee_wei, 50_000_000);
        assert_eq!(base_prices.l2_priority_fee_wei, 50_000_000);
        assert_eq!(base_prices.l1_data_fee_wei, 300_000_000_000_000);

        // 200,000 gas at 0.1 Gwei (100_000_000 wei) = 20,000,000,000,000 wei L2 cost
        // + 300,000,000,000,000 wei L1 fee = 320,000,000,000,000 wei
        let base_total = base_prices.total_gas_cost_wei(200_000);
        assert_eq!(base_total, U256::from(320_000_000_000_000u64));
    }
}

