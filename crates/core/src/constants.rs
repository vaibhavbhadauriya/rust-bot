use alloy_primitives::{address, Address};
use crate::types::Chain;

// ==============================================================================
// Arbitrum One (Chain ID 42161) Canonical Addresses
// ==============================================================================
pub mod arbitrum {
    use super::*;

    pub const CHAIN_ID: u64 = 42161;

    // Tokens
    pub const WETH: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
    pub const USDC_NATIVE: Address = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");
    pub const USDC_BRIDGED: Address = address!("FF970A61A04b1cA14834A43f5dE4533eBDDB5CC8");
    pub const USDT: Address = address!("Fd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9");
    pub const WBTC: Address = address!("2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f");
    pub const CBBTC: Address = address!("cbB7C0000aB88B473b1f5aFd9ef808440eed33Bf");
    pub const ARB: Address = address!("912CE59144191C1204E64559FE8253a0e49E6548");
    pub const GMX: Address = address!("fc5A1A6EB076a2C7aD06eD22C90d7E710E35ad0a");
    pub const LINK: Address = address!("f97f4df75117a78c1A5a0DBb814Af92458539FB4");
    pub const UNI: Address = address!("Fa7F8980b0f1E64A2062791ce3b087aD44e6b528");
    pub const PENDLE: Address = address!("0c880f6761F1af8d9Aa9C466984b80DAb9a8c9e8");
    pub const DAI: Address = address!("DA10009cBd5D07dd0CeCc6616156627511d75131");
    pub const WSTETH: Address = address!("5979D7b546E38E414F7E9822514be443A4800529");
    pub const WEETH: Address = address!("35751007a407ca6FEFfE80b3cB397736D2cf4dbe");

    // Factories & Protocol Singletons
    pub const UNISWAP_V3_FACTORY: Address = address!("1F98431c8aD98523631AE4a59f267346ea31F984");
    pub const PANCAKESWAP_V3_FACTORY: Address = address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
    pub const SUSHISWAP_V2_FACTORY: Address = address!("c35DADB65012eC5796536bD9864eD8773aBc74C4");
    pub const CAMELOT_V2_FACTORY: Address = address!("6EcCab422D763aC031210895C81787E87B43A652");
    pub const CAMELOT_V3_FACTORY: Address = address!("BefC4b405041c5833f53412fF997ed2f697a2f37");
    pub const UNISWAP_V2_FACTORY: Address = address!("f1D7CC64Fb4452F05c498126312eBE29f30Fbcf9");
    pub const BALANCER_V3_VAULT: Address = address!("bA1333333333a1BA1108E8412f11850A5C319bA9");
    pub const MORPHO_BLUE: Address = address!("BBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb");
    pub const AAVE_V3_POOL: Address = address!("794a61358D6845594F94dc1DB02A252b5b4814aD");

    // Live Feed Endpoints
    pub const SEQUENCER_FEED_WS: &str = "wss://arb1-feed.arbitrum.io/feed";
    pub const FALLBACK_RPC_WS: &str = "wss://arbitrum-one-rpc.publicnode.com";
    pub const ARBITRUM_FALLBACK_RPC_WS: &str = "wss://arbitrum-one-rpc.publicnode.com";
}

// ==============================================================================
// Base (Chain ID 8453) Canonical Addresses
// ==============================================================================
pub mod base {
    use super::*;

    pub const CHAIN_ID: u64 = 8453;

    // Tokens
    pub const WETH: Address = address!("4200000000000000000000000000000000000006");
    pub const USDC_NATIVE: Address = address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913");
    pub const USDBC_BRIDGED: Address = address!("d9aAEc86B65D86f6A7B5B1b0c42FFA531710b6CA");
    pub const USDT: Address = address!("fde4C96c8593536E31F229EA8f37b2ADa2699bb2");
    pub const CBBTC: Address = address!("cbB7C0000aB88B473b1f5aFd9ef808440eed33Bf");
    pub const WBTC: Address = address!("0555E30da8f98308EdB960aa94C0Db47230d2B9c");
    pub const AERO: Address = address!("940181a94A35A4569E4529A3CDfB74e38FD98631");
    pub const DAI: Address = address!("50c5725949A6F0c72E6C4a641F24049A917DB0Cb");
    pub const WSTETH: Address = address!("c1CBa3fCea344f92D9239c08C0568f6F2F0ee452");
    pub const WEETH: Address = address!("04C0599Ae5D7B7F27a1F676d11f8F59B7976c150");
    pub const LINK: Address = address!("88Fb150BDc53A65fe94Dea0c9BA0a6dAf8C6e196");
    pub const UNI: Address = address!("c3De830EA07524a0761646a6a4e4be0e114a3C83");
    pub const PENDLE: Address = address!("A99F6e6785Da0F5d6fB42495Fe424BCE029Eeb3E");
    pub const CBETH: Address = address!("2Ae3F1Ec7F1F5012CFEab0185bfc7aa3cf0DEc22");
    pub const BRETT: Address = address!("532f27101965dd16442e59d40670FAF5ebb142E4");
    pub const DEGEN: Address = address!("4ed4E862860beD51a9570b96d89aF5E1B0Efefed");
    pub const TOSHI: Address = address!("AC1Bd2486aAf3B5C0fc3Fd868558b082a531B2B4");

    // Factories & Protocol Singletons
    pub const AERODROME_SLIPSTREAM_FACTORY: Address = address!("5e7BB104d84c7CB9B682AaC2F3d509f5F406809A");
    pub const AERODROME_V2_FACTORY: Address = address!("420DD381b31aEf6683db6B902084cB0FFECe40Da");
    pub const UNISWAP_V3_FACTORY: Address = address!("33128a8fC17869897dcE68Ed026d694621f6FDfD");
    pub const PANCAKESWAP_V3_FACTORY: Address = address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865");
    pub const SUSHISWAP_V2_FACTORY: Address = address!("71524B4f93c58fcbF659783284E38825f0622859");
    pub const UNISWAP_V2_FACTORY: Address = address!("8909Dc15e40173Ff4699343b6eB8132c65e18eC6");
    pub const BALANCER_V3_VAULT: Address = address!("bA1333333333a1BA1108E8412f11850A5C319bA9");
    pub const MORPHO_BLUE: Address = address!("BBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb");
    pub const AAVE_V3_POOL: Address = address!("A238Dd80C259a72e81d7e4664a9801593F98d1c5");

    // Live Feed Endpoints
    pub const FALLBACK_RPC_WS: &str = "wss://base-rpc.publicnode.com";
    pub const BASE_FALLBACK_RPC_WS: &str = "wss://base-rpc.publicnode.com";
}

// ==============================================================================
// Live Feed Endpoints & Fallback WebSockets
// ==============================================================================
pub const ARBITRUM_FALLBACK_RPC_WS: &str = "wss://arbitrum-one-rpc.publicnode.com";
pub const BASE_FALLBACK_RPC_WS: &str = "wss://base-rpc.publicnode.com";

// ==============================================================================
// Protocol Selectors & Execution Constants
// ==============================================================================
pub const GET_RESERVES_SELECTOR: [u8; 4] = [0x09, 0x02, 0xf1, 0xac];
pub const GET_PAIR_SELECTOR: [u8; 4] = [0xe6, 0xa4, 0x39, 0x05];
pub const BALANCE_OF_SELECTOR: [u8; 4] = [0x70, 0xa0, 0x82, 0x31];
pub const TRANSFER_SELECTOR: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb];

/// 100-byte tightly packed Pure Yul calldata length (0x64 bytes)
pub const CALLDATA_PACKED_LEN: usize = 100;

/// 30-byte fast-revert guard length
pub const FAST_REVERT_GUARD_LEN: usize = 30;

/// Target leading hex zeros for vanity CREATE2 executor address (8 zero bytes)
pub const VANITY_TARGET_ZEROS: usize = 16;

/// Units scaling
pub const PIP_SCALE: u64 = 1_000_000;
pub const BPS_SCALE: u64 = 10_000;

/// Default Uniswap V3 fee tiers in PIPs
pub const UNISWAP_V3_FEE_TIERS: [u32; 4] = [100, 500, 3000, 10000];

/// Returns canonical token decimals for known tokens across Arbitrum One and Base.
/// Returns 18 for native/standard ERC-20 tokens if unknown.
pub fn get_token_decimals(chain: Chain, token: Address) -> u8 {
    match chain {
        Chain::ArbitrumOne => {
            if token == arbitrum::USDC_NATIVE
                || token == arbitrum::USDC_BRIDGED
                || token == arbitrum::USDT
            {
                6
            } else if token == arbitrum::WBTC || token == arbitrum::CBBTC {
                8
            } else {
                18
            }
        }
        Chain::Base => {
            if token == base::USDC_NATIVE
                || token == base::USDBC_BRIDGED
                || token == base::USDT
            {
                6
            } else if token == base::CBBTC || token == base::WBTC {
                8
            } else {
                18
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_token_decimals() {
        assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::USDC_NATIVE), 6);
        assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::USDC_BRIDGED), 6);
        assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::USDT), 6);
        assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::WBTC), 8);
        assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::CBBTC), 8);
        assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::WETH), 18);
        assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::ARB), 18);
        assert_eq!(get_token_decimals(Chain::ArbitrumOne, arbitrum::PENDLE), 18);

        assert_eq!(get_token_decimals(Chain::Base, base::USDC_NATIVE), 6);
        assert_eq!(get_token_decimals(Chain::Base, base::USDBC_BRIDGED), 6);
        assert_eq!(get_token_decimals(Chain::Base, base::USDT), 6);
        assert_eq!(get_token_decimals(Chain::Base, base::CBBTC), 8);
        assert_eq!(get_token_decimals(Chain::Base, base::WBTC), 8);
        assert_eq!(get_token_decimals(Chain::Base, base::WETH), 18);
        assert_eq!(get_token_decimals(Chain::Base, base::AERO), 18);
    }

    #[test]
    fn test_canonical_factories() {
        assert_eq!(arbitrum::CAMELOT_V2_FACTORY, address!("6EcCab422D763aC031210895C81787E87B43A652"));
        assert_eq!(arbitrum::PANCAKESWAP_V3_FACTORY, address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865"));
        assert_eq!(base::PANCAKESWAP_V3_FACTORY, address!("0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865"));
        assert_eq!(arbitrum::UNISWAP_V2_FACTORY, address!("f1D7CC64Fb4452F05c498126312eBE29f30Fbcf9"));
        assert_eq!(base::UNISWAP_V2_FACTORY, address!("8909Dc15e40173Ff4699343b6eB8132c65e18eC6"));
    }

    #[test]
    fn test_fallback_rpc_ws_constants() {
        assert_eq!(ARBITRUM_FALLBACK_RPC_WS, "wss://arbitrum-one-rpc.publicnode.com");
        assert_eq!(arbitrum::FALLBACK_RPC_WS, "wss://arbitrum-one-rpc.publicnode.com");
        assert_eq!(arbitrum::ARBITRUM_FALLBACK_RPC_WS, "wss://arbitrum-one-rpc.publicnode.com");

        assert_eq!(BASE_FALLBACK_RPC_WS, "wss://base-rpc.publicnode.com");
        assert_eq!(base::FALLBACK_RPC_WS, "wss://base-rpc.publicnode.com");
        assert_eq!(base::BASE_FALLBACK_RPC_WS, "wss://base-rpc.publicnode.com");
    }
}

