use crate::types::ExecutionMode;
use alloy_primitives::Address;
use std::env;
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Missing environment variable: {0}")]
    MissingVar(String),
    #[error("Invalid value for {var}: {reason}")]
    InvalidValue { var: String, reason: String },
    #[error("Address parse error for {var}: {source}")]
    AddressParse {
        var: String,
        source: alloy_primitives::hex::FromHexError,
    },
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub alchemy_key: String,
    pub arbitrum_rpc_ws: String,
    pub arbitrum_rpc_http: String,
    pub arbitrum_sequencer_feed: String,
    pub base_rpc_ws: String,
    pub base_rpc_http: String,
    pub base_flashblocks_ws: String,
    pub execution_mode: ExecutionMode,
    pub log_level: String,
    pub signer_private_keys: Vec<String>,
    pub executor_contract_address: Address,
    pub max_hops: usize,
    pub kill_switch_feed_lag_ms: u64,
    pub kill_switch_fast_revert_count: usize,
}

pub type Config = AppConfig;

impl AppConfig {
    /// Loads configuration from environment, aliasing `load`
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::load()
    }

    /// Loads configuration from .env file and environment variables
    pub fn load() -> Result<Self, ConfigError> {
        // Attempt loading .env; ignore failure if file does not exist
        let _ = dotenvy::dotenv();

        let alchemy_key = match env::var("ALCHEMY_KEY") {
            Ok(k) if !k.trim().is_empty() => k,
            _ => return Err(ConfigError::MissingVar("ALCHEMY_KEY".to_string())),
        };

        let arbitrum_rpc_ws = env::var("ARBITRUM_RPC_WS").unwrap_or_else(|_| {
            format!("wss://arb-mainnet.g.alchemy.com/v2/{}", alchemy_key)
        });

        let arbitrum_rpc_http = env::var("ARBITRUM_RPC_HTTP").unwrap_or_else(|_| {
            format!("https://arb-mainnet.g.alchemy.com/v2/{}", alchemy_key)
        });

        let arbitrum_sequencer_feed = env::var("ARBITRUM_SEQUENCER_FEED")
            .unwrap_or_else(|_| "wss://arb1-feed.arbitrum.io/feed".to_string());

        let base_rpc_ws = env::var("BASE_RPC_WS").unwrap_or_else(|_| {
            format!("wss://base-mainnet.g.alchemy.com/v2/{}", alchemy_key)
        });

        let base_rpc_http = env::var("BASE_RPC_HTTP").unwrap_or_else(|_| {
            format!("https://base-mainnet.g.alchemy.com/v2/{}", alchemy_key)
        });

        let base_flashblocks_ws = env::var("BASE_FLASHBLOCKS_WS").unwrap_or_else(|_| {
            format!("wss://base-mainnet.g.alchemy.com/v2/{}", alchemy_key)
        });

        let execution_mode = env::var("EXECUTION_MODE")
            .map(|s| ExecutionMode::from_str(&s))
            .unwrap_or(ExecutionMode::Shadow);

        let log_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

        let signer_private_keys = env::var("SIGNER_PRIVATE_KEYS")
            .map(|s| s.split(',').map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect())
            .unwrap_or_default();

        let executor_contract_address = match env::var("EXECUTOR_CONTRACT_ADDRESS") {
            Ok(addr_str) => Address::from_str(&addr_str).map_err(|e| ConfigError::AddressParse {
                var: "EXECUTOR_CONTRACT_ADDRESS".to_string(),
                source: e,
            })?,
            Err(_) => Address::ZERO,
        };

        let max_hops = env::var("MAX_HOPS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(3);

        let kill_switch_feed_lag_ms = env::var("KILL_SWITCH_FEED_LAG_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(100);

        let kill_switch_fast_revert_count = env::var("KILL_SWITCH_FAST_REVERT_COUNT")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(3);

        let config = Self {
            alchemy_key,
            arbitrum_rpc_ws,
            arbitrum_rpc_http,
            arbitrum_sequencer_feed,
            base_rpc_ws,
            base_rpc_http,
            base_flashblocks_ws,
            execution_mode,
            log_level,
            signer_private_keys,
            executor_contract_address,
            max_hops,
            kill_switch_feed_lag_ms,
            kill_switch_fast_revert_count,
        };

        config.validate_live_mode()?;

        Ok(config)
    }

    /// Validates live execution prerequisites (V-01, V-26).
    /// Ensures executor_contract_address is set to a valid non-zero contract
    /// and not the EVM ecrecover precompile (0x01).
    pub fn validate_live_mode(&self) -> Result<(), ConfigError> {
        if self.execution_mode.is_live() {
            if self.executor_contract_address == Address::ZERO
                || self.executor_contract_address == Address::with_last_byte(1)
            {
                return Err(ConfigError::InvalidValue {
                    var: "EXECUTOR_CONTRACT_ADDRESS".to_string(),
                    reason: "Executor address must not be Address::ZERO or precompile 0x01 in live mode".to_string(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_app_config_load_success() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var("EXECUTION_MODE", "shadow");
        if env::var("ALCHEMY_KEY").is_err() {
            env::set_var("ALCHEMY_KEY", "test_alchemy_key_123");
        }
        let config = AppConfig::load().expect("AppConfig::load should succeed");
        assert!(config.max_hops > 0);
        assert!(config.kill_switch_feed_lag_ms > 0);
        assert!(config.kill_switch_fast_revert_count > 0);
    }

    #[test]
    fn test_app_config_from_env_success() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var("EXECUTION_MODE", "shadow");
        if env::var("ALCHEMY_KEY").is_err() {
            env::set_var("ALCHEMY_KEY", "test_alchemy_key_123");
        }
        let config = Config::from_env().expect("Config::from_env should succeed");
        assert_eq!(config.max_hops, 3);
    }

    #[test]
    fn test_app_config_no_hardcoded_alchemy_fallback() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var("EXECUTION_MODE", "shadow");
        if env::var("ALCHEMY_KEY").is_err() {
            env::set_var("ALCHEMY_KEY", "test_alchemy_key_123");
        }
        let config = AppConfig::load().expect("AppConfig::load should succeed");
        let val = env::var("ALCHEMY_KEY").unwrap();
        assert_eq!(config.alchemy_key, val);
    }

    #[test]
    fn test_app_config_live_mode_zero_address_rejected() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var("ALCHEMY_KEY", "test_key");
        env::set_var("EXECUTION_MODE", "live");
        env::set_var("EXECUTOR_CONTRACT_ADDRESS", "0x0000000000000000000000000000000000000000");

        let res = AppConfig::load();
        assert!(matches!(
            res,
            Err(ConfigError::InvalidValue { ref var, .. }) if var == "EXECUTOR_CONTRACT_ADDRESS"
        ));

        // Restore to shadow mode
        env::set_var("EXECUTION_MODE", "shadow");
        env::remove_var("EXECUTOR_CONTRACT_ADDRESS");
    }

    #[test]
    fn test_app_config_live_mode_precompile_01_rejected() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        env::set_var("ALCHEMY_KEY", "test_key");
        env::set_var("EXECUTION_MODE", "live");
        env::set_var("EXECUTOR_CONTRACT_ADDRESS", "0x0000000000000000000000000000000000000001");

        let res = AppConfig::load();
        assert!(matches!(
            res,
            Err(ConfigError::InvalidValue { ref var, ref reason })
                if var == "EXECUTOR_CONTRACT_ADDRESS" && reason.contains("precompile 0x01")
        ));

        // Restore to shadow mode
        env::set_var("EXECUTION_MODE", "shadow");
        env::remove_var("EXECUTOR_CONTRACT_ADDRESS");
    }

    #[test]
    fn test_validate_live_mode_explicit() {
        let mut config = AppConfig {
            alchemy_key: "k".into(),
            arbitrum_rpc_ws: "".into(),
            arbitrum_rpc_http: "".into(),
            arbitrum_sequencer_feed: "".into(),
            base_rpc_ws: "".into(),
            base_rpc_http: "".into(),
            base_flashblocks_ws: "".into(),
            execution_mode: ExecutionMode::Shadow,
            log_level: "info".into(),
            signer_private_keys: vec![],
            executor_contract_address: Address::ZERO,
            max_hops: 3,
            kill_switch_feed_lag_ms: 100,
            kill_switch_fast_revert_count: 3,
        };

        // Shadow mode allows Address::ZERO
        assert!(config.validate_live_mode().is_ok());

        // Live mode rejects Address::ZERO
        config.execution_mode = ExecutionMode::Live;
        assert!(config.validate_live_mode().is_err());

        // Live mode rejects precompile 0x01
        config.executor_contract_address = Address::with_last_byte(1);
        assert!(config.validate_live_mode().is_err());

        // Live mode accepts real contract address
        config.executor_contract_address = Address::from([0x22; 20]);
        assert!(config.validate_live_mode().is_ok());
    }
}
