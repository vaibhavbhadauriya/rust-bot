//! Embedded revm in-memory simulation engine, UniV3 quoter bytecode, two-tier bytecode caching,
//! and honeypot TxInspector defense.
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 43-46, 89; PROJECT.md § Milestone 4.

pub mod bytecode_mocks;
pub mod cache;
pub mod engine;
pub mod honeypot;
pub mod inspector;
pub mod quoter;
pub mod types;

pub use bytecode_mocks::*;
pub use cache::{BytecodeCache, CacheError, MemoryLru};
pub use engine::SimulationEngine;
pub use honeypot::{
    inspect_honeypot_safety, is_canonical_safe_token, verify_token_integrity,
    HoneypotDetector, HoneypotSimulationReport,
};
pub use inspector::{HoneypotTxInspector, HoneypotViolation, StorageMutation};
pub use quoter::{
    calculate_rpc_reduction_percentage, compute_pool_storage_key, decode_quote_output,
    encode_quote_exact_input_single, execute_evm_quote, inject_pool_state,
    inject_quoter_bytecode, quote_analytical_clmm, quoter_runtime_bytecode,
    verify_quoter_parity, QuoteExactInputSingleParams, QuoteOutput, QuoterError,
    SimulatedPoolState, DEFAULT_QUOTER_ADDRESS, QUOTE_EXACT_INPUT_SINGLE_SELECTOR,
    QUOTE_EXACT_INPUT_SINGLE_V2_SELECTOR, QUOTER_RUNTIME_BYTECODE, TEST_QUOTER_ADDRESS,
};
pub use types::{SimError, SimulationOutcome, StateOverride};
