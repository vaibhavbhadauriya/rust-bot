//! Dynamic Gas Oracle for Arbitrum One and Base.
//!
//! Polls live L2 base fee (`eth_gasPrice`), priority fee (`eth_maxPriorityFeePerGas`),
//! and L1 data/blob posting fees via network precompiles:
//! - Arbitrum One: `ArbGasInfo` at `0x000000000000000000000000000000000000006C`
//! - Base: `GasPriceOracle` at `0x420000000000000000000000000000000000000F`
//!
//! Authoritative source: ORIGINAL_REQUEST.md line 133, PRE_PROD_AUDIT_REPORT.md §V-06, V-12, H1.

use alloy_primitives::{address, Address};
use flash_core::types::{Chain, GasPrices};
use parking_lot::RwLock;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::debug;

pub const ARB_GAS_INFO: Address = address!("000000000000000000000000000000000000006C");
pub const BASE_GAS_PRICE_ORACLE: Address = address!("420000000000000000000000000000000000000F");

#[derive(Debug, Error)]
pub enum GasOracleError {
    #[error("HTTP client error: {0}")]
    Network(String),
    #[error("JSON-RPC error: {0}")]
    Rpc(String),
    #[error("Parse error: {0}")]
    Parse(String),
}

pub struct GasOracle {
    pub chain: Chain,
    pub rpc_url: String,
    pub client: reqwest::Client,
    pub prices: Arc<RwLock<GasPrices>>,
}

impl GasOracle {
    pub fn new(chain: Chain, rpc_url: String) -> Self {
        let prices = Arc::new(RwLock::new(GasPrices::default_for_chain(chain)));
        Self::new_with_prices(chain, rpc_url, prices)
    }

    pub fn new_with_prices(
        chain: Chain,
        rpc_url: String,
        prices: Arc<RwLock<GasPrices>>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(1500))
            .build()
            .unwrap_or_default();

        Self {
            chain,
            rpc_url,
            client,
            prices,
        }
    }

    pub fn current_prices(&self) -> GasPrices {
        *self.prices.read()
    }

    /// Single poll iteration querying L2 gas prices and L1 data posting fee.
    /// On error, retains existing or default prices and returns Ok(existing).
    pub async fn poll_once(&self) -> Result<GasPrices, GasOracleError> {
        let current = *self.prices.read();

        let l2_base_fee = self.query_gas_price().await.unwrap_or(current.l2_base_fee_wei);
        let l2_tip = self.query_max_priority_fee().await.unwrap_or(current.l2_priority_fee_wei);
        let l1_fee = self.query_l1_data_fee().await.unwrap_or(current.l1_data_fee_wei);

        let new_prices = GasPrices {
            l2_base_fee_wei: l2_base_fee.max(1_000_000), // at least 0.001 Gwei
            l2_priority_fee_wei: l2_tip.max(1_000_000),  // at least 0.001 Gwei
            l1_data_fee_wei: l1_fee,
        };

        *self.prices.write() = new_prices;
        Ok(new_prices)
    }

    pub async fn query_gas_price(&self) -> Result<u128, GasOracleError> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_gasPrice",
            "params": []
        });

        let res: serde_json::Value = self
            .client
            .post(&self.rpc_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| GasOracleError::Network(e.to_string()))?
            .json()
            .await
            .map_err(|e| GasOracleError::Parse(e.to_string()))?;

        if let Some(err) = res.get("error") {
            return Err(GasOracleError::Rpc(err.to_string()));
        }

        let hex_str = res["result"]
            .as_str()
            .ok_or_else(|| GasOracleError::Parse("Missing result field".into()))?;

        u128::from_str_radix(hex_str.trim_start_matches("0x"), 16)
            .map_err(|e| GasOracleError::Parse(e.to_string()))
    }

    pub async fn query_max_priority_fee(&self) -> Result<u128, GasOracleError> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "eth_maxPriorityFeePerGas",
            "params": []
        });

        let res: serde_json::Value = self
            .client
            .post(&self.rpc_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| GasOracleError::Network(e.to_string()))?
            .json()
            .await
            .map_err(|e| GasOracleError::Parse(e.to_string()))?;

        if let Some(err) = res.get("error") {
            return Err(GasOracleError::Rpc(err.to_string()));
        }

        let hex_str = res["result"]
            .as_str()
            .ok_or_else(|| GasOracleError::Parse("Missing result field".into()))?;

        u128::from_str_radix(hex_str.trim_start_matches("0x"), 16)
            .map_err(|e| GasOracleError::Parse(e.to_string()))
    }

    pub async fn query_l1_data_fee(&self) -> Result<u128, GasOracleError> {
        match self.chain {
            Chain::ArbitrumOne => {
                // ArbGasInfo.getPricesInWei() selector: 0x27c191de
                let payload = json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "eth_call",
                    "params": [{
                        "to": format!("{:#x}", ARB_GAS_INFO),
                        "data": "0x27c191de"
                    }, "latest"]
                });

                let res: serde_json::Value = self
                    .client
                    .post(&self.rpc_url)
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|e| GasOracleError::Network(e.to_string()))?
                    .json()
                    .await
                    .map_err(|e| GasOracleError::Parse(e.to_string()))?;

                let hex_str = res["result"]
                    .as_str()
                    .ok_or_else(|| GasOracleError::Parse("Missing result field".into()))?;

                let clean = hex_str.trim_start_matches("0x");
                if clean.len() >= 128 {
                    // Word 1: perL1CalldataByte in wei (chars 64..128)
                    let per_byte = u128::from_str_radix(&clean[64..128], 16)
                        .map_err(|e| GasOracleError::Parse(e.to_string()))?;
                    // Standard ~124 byte arbitrage transaction calldata
                    Ok(per_byte.saturating_mul(124))
                } else {
                    Ok(self.prices.read().l1_data_fee_wei)
                }
            }
            Chain::Base => {
                // Base GasPriceOracle.l1BaseFee() selector: 0x519b4b9b
                let payload = json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "eth_call",
                    "params": [{
                        "to": format!("{:#x}", BASE_GAS_PRICE_ORACLE),
                        "data": "0x519b4b9b"
                    }, "latest"]
                });

                let res: serde_json::Value = self
                    .client
                    .post(&self.rpc_url)
                    .json(&payload)
                    .send()
                    .await
                    .map_err(|e| GasOracleError::Network(e.to_string()))?
                    .json()
                    .await
                    .map_err(|e| GasOracleError::Parse(e.to_string()))?;

                let hex_str = res["result"]
                    .as_str()
                    .ok_or_else(|| GasOracleError::Parse("Missing result field".into()))?;

                let clean = hex_str.trim_start_matches("0x");
                let l1_base_fee = u128::from_str_radix(clean, 16)
                    .map_err(|e| GasOracleError::Parse(e.to_string()))?;
                // Base standard L1 execution gas ~1,600 gas
                Ok(l1_base_fee.saturating_mul(1600))
            }
        }
    }

    /// Spawns a background worker updating gas prices every 1000ms
    pub fn start_polling(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(1000));
            loop {
                interval.tick().await;
                if let Err(e) = self.poll_once().await {
                    debug!("GasOracle background update error: {}", e);
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_oracle_initialization() {
        let oracle = GasOracle::new(Chain::ArbitrumOne, "http://localhost:8545".into());
        let prices = oracle.current_prices();
        assert_eq!(prices.l2_base_fee_wei, 100_000_000);
        assert_eq!(prices.l2_priority_fee_wei, 10_000_000);
        assert_eq!(prices.l1_data_fee_wei, 400_000_000_000_000);

        let base_oracle = GasOracle::new(Chain::Base, "http://localhost:8545".into());
        let base_prices = base_oracle.current_prices();
        assert_eq!(base_prices.l2_base_fee_wei, 50_000_000);
        assert_eq!(base_prices.l2_priority_fee_wei, 50_000_000);
        assert_eq!(base_prices.l1_data_fee_wei, 300_000_000_000_000);
    }

    #[tokio::test]
    async fn test_gas_oracle_poll_fallback_on_unreachable_rpc() {
        let oracle = GasOracle::new(Chain::ArbitrumOne, "http://127.0.0.1:9".into());
        // Should not panic, but gracefully return fallback default prices
        let res = oracle.poll_once().await;
        assert!(res.is_ok());
        let prices = res.unwrap();
        assert_eq!(prices.l2_base_fee_wei, 100_000_000);
        assert_eq!(prices.l2_priority_fee_wei, 10_000_000);
        assert_eq!(prices.l1_data_fee_wei, 400_000_000_000_000);
    }
}
