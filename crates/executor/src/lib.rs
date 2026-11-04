//! High-Performance Flash Loan Routing & Pure Yul Calldata Encoding Engine
//!
//! Milestone 3 (Requirement R3):
//! - Priority 1-4 flash loan router (Morpho Blue -> Balancer V3 -> Uniswap V4 -> Aave V3 fallback)
//! - 100-byte tightly packed pure Yul calldata encoder/decoder with zero function selector dispatch
//! - Protocol-specific initiation and callback ABI serializers
//! - 30-byte fast-revert guard simulator & atomic profit check
//! - CREATE2 16-zero vanity salt miner
//! - Embedded pure Yul bytecode constants for revm in-memory simulation

pub mod bytecode;
pub mod calldata;
pub mod routing;
pub mod vanity;

pub use bytecode::{
    executor_init_code, executor_runtime_bytecode, executor_runtime_bytes, EXECUTOR_INIT_CODE,
    EXECUTOR_RUNTIME_BYTECODE,
};

pub use calldata::{
    check_atomic_profit, decode_124byte_calldata, decode_aave_v3_callback,
    decode_calldata, decode_morpho_callback, decode_unlock_callback,
    dispatch_action_by_callvalue, encode_124byte_calldata, encode_163byte_calldata,
    encode_aave_v3_callback, encode_aave_v3_flash_loan, encode_balancer_v3_callback,
    encode_balancer_v3_unlock, encode_calldata, encode_morpho_callback,
    encode_morpho_flash_loan, encode_univ4_callback, encode_univ4_unlock,
    simulate_fast_revert_guard, CallvalueAction, CalldataError, ExecutionParams,
    GuardOutcome,
};

pub use routing::{
    route_flash_loan, select_optimal_flash_route, FlashLoanProvider, FlashLoanRoute,
    ProviderLiquiditySnapshot, RoutingError, AAVE_FLASH_LOAN_SIMPLE_SELECTOR,
    AAVE_V3_CALLBACK_SELECTOR, BALANCER_UNLOCK_SELECTOR, BALANCER_V3_CALLBACK_SELECTOR,
    MORPHO_BLUE_CALLBACK_SELECTOR, MORPHO_FLASH_LOAN_SELECTOR, UNISWAP_V4_CALLBACK_SELECTOR,
    UNISWAP_V4_POOL_MANAGER, UNISWAP_V4_UNLOCK_SELECTOR,
};

pub use vanity::{
    calculate_calldata_gas_savings, compute_create2_address, compute_create2_address_from_code,
    count_leading_hex_zeros, count_leading_zero_bytes, extract_address_from_hash,
    has_leading_zero_bytes, mine_vanity_salt, MiningResult, VanityMiner, VanityMinerError,
    CREATE2_PREFIX, CREATE2_PREIMAGE_LEN, GAS_PER_NONZERO_BYTE, GAS_PER_ZERO_BYTE,
    GAS_SAVINGS_PER_ZERO_BYTE, VANITY_16_HEX_ZEROS_BYTES, VANITY_CALDATA_GAS_SAVINGS,
};
