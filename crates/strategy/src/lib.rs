//! High-Performance DeFi Arbitrage Strategy, Shadow Paper-Trading, Multi-EOA Pool & Microsecond Telemetry Engine.
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 69-74, 90; PROJECT.md § Milestone 5.

pub mod gas_oracle;
pub mod kill_switch;
pub mod pipeline;
pub mod shadow;
pub mod signers;
pub mod telemetry;

pub use gas_oracle::{GasOracle, GasOracleError, ARB_GAS_INFO, BASE_GAS_PRICE_ORACLE};
pub use kill_switch::{
    KillSwitchController, KillSwitchError, DEFAULT_FAST_REVERTS_THRESHOLD,
    DEFAULT_FEED_LAG_THRESHOLD_MS,
};
pub use pipeline::{ArbitragePipeline, PipelineError};
pub use shadow::{
    CompetingTx, ShadowEngine, ShadowPnLMetrics, ShadowPnLTracker, SyntheticMatchResult,
    SyntheticOrder, SyntheticOutcome,
};
pub use signers::{
    Eip1559TransactionRequest, EoaSigner, SignedRawTransaction, SignerError, SignerLease,
    SignerPool,
};
pub use telemetry::{PipelineStageTimings, PipelineTelemetry, TelemetryCollector};
