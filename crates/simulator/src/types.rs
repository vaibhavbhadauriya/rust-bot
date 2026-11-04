//! Core types and error definitions for embedded revm simulation.
//! Authoritative source: ORIGINAL_REQUEST.md lines 43-46, PROJECT.md lines 58, 93-94.

use alloy_primitives::{Address, Bytes, Log, I256, U256};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Outcome of an in-memory EVM arbitrage simulation run
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationOutcome {
    /// Whether the simulated transaction succeeded without revert or halt
    pub is_success: bool,
    /// Total gas consumed by the transaction in revm
    pub gas_used: u64,
    /// Estimated cost of gas charged in wei
    pub gas_cost_wei: U256,
    /// Gross profit extracted in base tokens (e.g. WETH)
    pub actual_profit: U256,
    /// Net yield after subtracting gas cost (may be negative if unprofitable)
    pub net_yield: I256,
    /// Calldata passed to the Pure Yul contract (124B for 2 hops, 163B for 3 hops)
    pub calldata: Bytes,
    /// Whether the 30-byte fast-revert guard was tripped
    pub fast_revert_tripped: bool,
    /// Logs emitted during execution
    pub logs: Vec<Log>,
    /// Returndata returned by execution
    pub return_data: Bytes,
}

/// Simulation error classifications
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SimError {
    #[error("EVM execution reverted: {0:?}")]
    Reverted(Bytes),
    #[error("Fast-revert guard tripped: Pool 1 reserves modified")]
    FastRevertTriggered,
    #[error("Atomic profit violation: profit {profit} < gas cost {gas_cost}")]
    Unprofitable { profit: U256, gas_cost: U256 },
    #[error("EVM execution halted: {0}")]
    Halted(String),
    #[error("Contract bytecode not found for address: {0}")]
    BytecodeNotFound(Address),
    #[error("Database error: {0}")]
    DatabaseError(String),
    #[error("Invalid route parameters: {0}")]
    InvalidRoute(String),
    #[error("Honeypot trap detected: {0}")]
    Honeypot(String),
}

/// State override applied to in-memory EVM accounts prior to simulation
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StateOverride {
    pub address: Address,
    pub balance: Option<U256>,
    pub nonce: Option<u64>,
    pub code: Option<Bytes>,
    pub storage: Vec<(U256, U256)>,
}

impl StateOverride {
    pub fn new(address: Address) -> Self {
        Self {
            address,
            balance: None,
            nonce: None,
            code: None,
            storage: Vec::new(),
        }
    }

    pub fn with_balance(mut self, balance: U256) -> Self {
        self.balance = Some(balance);
        self
    }

    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = Some(nonce);
        self
    }

    pub fn with_code(mut self, code: Bytes) -> Self {
        self.code = Some(code);
        self
    }

    pub fn with_storage_slot(mut self, slot: U256, value: U256) -> Self {
        self.storage.push((slot, value));
        self
    }
}
