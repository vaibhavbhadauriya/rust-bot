//! Adversarial Stress Suite for Milestone M_R1
//! Targets:
//! 1. L1 & L2 (crates/core/src/types.rs): parse_units_str and format_units_str
//! 2. M5 & M6 (crates/core/src/config.rs): AppConfig::load and Config::from_env

use alloy_primitives::{Address, U256};
use flash_core::config::{AppConfig, Config, ConfigError};
use flash_core::types::Token;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// Global mutex to synchronize environment mutation across tests in this binary
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII helper that moves the process current directory to a clean temp directory
/// with NO `.env` file, and clears relevant environment variables.
struct IsolatedEnvScope {
    orig_dir: PathBuf,
    temp_dir: PathBuf,
}

impl IsolatedEnvScope {
    fn new(test_name: &str) -> Self {
        let orig_dir = env::current_dir().expect("Failed to get current dir");
        let temp_dir = env::temp_dir().join(format!("flash_core_stress_{}_{}", test_name, std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        env::set_current_dir(&temp_dir).expect("Failed to switch to temp dir");

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

// =========================================================================
// SECTION 1: L1 & L2 (types.rs) UNITS ADVERSARIAL STRESS TESTS
// =========================================================================

#[test]
fn test_stress_units_zero_decimals() {
    let token0 = Token::new(Address::ZERO, "D0", "ZeroDecimals", 0, 1);

    // Integers
    assert_eq!(token0.parse_units_str("0"), U256::ZERO);
    assert_eq!(token0.parse_units_str("1"), U256::from(1));
    assert_eq!(token0.parse_units_str("42"), U256::from(42));
    assert_eq!(token0.parse_units_str("1234567890"), U256::from(1234567890u64));

    // Fractional inputs should discard fractions when decimals == 0
    assert_eq!(token0.parse_units_str("0.5"), U256::ZERO);
    assert_eq!(token0.parse_units_str("42.999999999"), U256::from(42));
    assert_eq!(token0.parse_units_str("100.0000000000000000001"), U256::from(100));

    // Large numbers > u128::MAX (u128::MAX is 340282366920938463463374607431768211455)
    let u128_max_plus_1 = "340282366920938463463374607431768211456";
    let parsed_large = token0.parse_units_str(u128_max_plus_1);
    let expected_large = U256::from(u128::MAX) + U256::from(1);
    assert_eq!(parsed_large, expected_large);

    // format_units_str on zero decimals: MUST NOT have trailing dot
    assert_eq!(token0.format_units_str(U256::ZERO), "0");
    assert_eq!(token0.format_units_str(U256::from(1)), "1");
    assert_eq!(token0.format_units_str(U256::from(42)), "42");
    assert_eq!(token0.format_units_str(expected_large), u128_max_plus_1);

    // Roundtrip verification
    assert_eq!(token0.parse_units_str(&token0.format_units_str(expected_large)), expected_large);
}

#[test]
fn test_stress_units_18_decimals_standard_and_25_eth() {
    let weth = Token::new(Address::ZERO, "WETH", "WETH", 18, 1);

    assert_eq!(weth.parse_units_str("0"), U256::ZERO);
    assert_eq!(weth.parse_units_str("1"), U256::from(10u64.pow(18)));
    assert_eq!(weth.parse_units_str("1.0"), U256::from(10u64.pow(18)));
    assert_eq!(weth.parse_units_str("0.5"), U256::from(500_000_000_000_000_000u64));
    assert_eq!(weth.parse_units_str(".25"), U256::from(250_000_000_000_000_000u64));

    // 1 wei (smallest unit)
    assert_eq!(weth.parse_units_str("0.000000000000000001"), U256::from(1));
    assert_eq!(weth.format_units_str(U256::from(1)), "0.000000000000000001");

    // Critical 25 ETH acceptance criteria test case: 25 * 10^18 wei
    let twenty_five_eth_wei = U256::from(25u64) * U256::from(10u64.pow(18));
    assert_eq!(weth.parse_units_str("25.0"), twenty_five_eth_wei);
    assert_eq!(weth.parse_units_str("25"), twenty_five_eth_wei);
    assert_eq!(weth.format_units_str(twenty_five_eth_wei), "25.000000000000000000");

    // Exact high precision without rounding
    let exact_str = "123456789.987654321012345678";
    let parsed_exact = weth.parse_units_str(exact_str);
    assert_eq!(weth.format_units_str(parsed_exact), exact_str);

    // Number > u128::MAX in 18 decimals
    let large_int = U256::from(u128::MAX) + U256::from(1);
    let scale_18 = U256::from(10u64.pow(18));
    let large_wei = large_int * scale_18;
    let formatted = weth.format_units_str(large_wei);
    assert_eq!(weth.parse_units_str(&formatted), large_wei);
}

#[test]
fn test_stress_units_24_decimals() {
    let token24 = Token::new(Address::ZERO, "T24", "Token24", 24, 1);
    let scale_24 = U256::from(10u64).pow(U256::from(24));

    assert_eq!(token24.parse_units_str("1"), scale_24);
    assert_eq!(token24.parse_units_str("1.0"), scale_24);
    assert_eq!(token24.parse_units_str("0.000000000000000000000001"), U256::from(1));
    assert_eq!(token24.format_units_str(U256::from(1)), "0.000000000000000000000001");

    let exact_24 = "100.123456789012345678901234";
    let parsed = token24.parse_units_str(exact_24);
    assert_eq!(token24.format_units_str(parsed), exact_24);
}

#[test]
fn test_stress_units_fraction_with_30_decimal_places() {
    let weth18 = Token::new(Address::ZERO, "WETH", "WETH", 18, 1);
    let usdc6 = Token::new(Address::ZERO, "USDC", "USDC", 6, 1);
    let token0 = Token::new(Address::ZERO, "D0", "Zero", 0, 1);
    let token24 = Token::new(Address::ZERO, "T24", "Token24", 24, 1);

    // 30 decimal digits: "123456789012345678901234567890"
    let thirty_decimals = "1.123456789012345678901234567890";

    // 18 decimals token should truncate to first 18 decimal digits: "123456789012345678"
    let expected_18 = weth18.parse_units_str("1.123456789012345678");
    assert_eq!(weth18.parse_units_str(thirty_decimals), expected_18);

    // 6 decimals token should truncate to first 6 digits: "123456"
    let expected_6 = usdc6.parse_units_str("1.123456");
    assert_eq!(usdc6.parse_units_str(thirty_decimals), expected_6);

    // 0 decimals token should truncate all decimal digits
    assert_eq!(token0.parse_units_str(thirty_decimals), U256::from(1));

    // 24 decimals token should truncate to first 24 digits: "123456789012345678901234"
    let expected_24 = token24.parse_units_str("1.123456789012345678901234");
    assert_eq!(token24.parse_units_str(thirty_decimals), expected_24);
}

#[test]
fn test_stress_units_leading_trailing_whitespace() {
    let usdc = Token::new(Address::ZERO, "USDC", "USDC", 6, 1);

    assert_eq!(usdc.parse_units_str("   100   "), U256::from(100_000_000));
    assert_eq!(usdc.parse_units_str("\t\r\n 50.25 \n "), U256::from(50_250_000));
    assert_eq!(usdc.parse_units_str("   .75   "), U256::from(750_000));
    assert_eq!(usdc.parse_units_str("   +10.5   "), U256::from(10_500_000));

    // Whitespace only
    assert_eq!(usdc.parse_units_str("   "), U256::ZERO);
    assert_eq!(usdc.parse_units_str("\t\r\n"), U256::ZERO);
}

#[test]
fn test_stress_units_negative_inputs() {
    let usdc = Token::new(Address::ZERO, "USDC", "USDC", 6, 1);

    assert_eq!(usdc.parse_units_str("-1"), U256::ZERO);
    assert_eq!(usdc.parse_units_str("-0.5"), U256::ZERO);
    assert_eq!(usdc.parse_units_str("-0"), U256::ZERO);
    assert_eq!(usdc.parse_units_str("-1000000.000000"), U256::ZERO);
    assert_eq!(usdc.parse_units_str("   -42.5   "), U256::ZERO);
    assert_eq!(usdc.parse_units_str("-0.000000000000000001"), U256::ZERO);
}

#[test]
fn test_stress_units_empty_and_multiple_decimal_points() {
    let weth = Token::new(Address::ZERO, "WETH", "WETH", 18, 1);

    // Empty
    assert_eq!(weth.parse_units_str(""), U256::ZERO);

    // Multiple decimal points
    assert_eq!(weth.parse_units_str("1.2.3"), U256::ZERO);
    assert_eq!(weth.parse_units_str("1..2"), U256::ZERO);
    assert_eq!(weth.parse_units_str(".."), U256::ZERO);
    assert_eq!(weth.parse_units_str("..."), U256::ZERO);
    assert_eq!(weth.parse_units_str("0.0.0.0"), U256::ZERO);

    // Bare decimal point
    assert_eq!(weth.parse_units_str("."), U256::ZERO);
}

#[test]
fn test_stress_units_large_numbers_and_overflow_safety() {
    let token0 = Token::new(Address::ZERO, "D0", "Zero", 0, 1);
    let weth18 = Token::new(Address::ZERO, "WETH", "WETH", 18, 1);

    // U256::MAX has 78 decimal digits
    let u256_max_str = U256::MAX.to_string();
    assert_eq!(token0.parse_units_str(&u256_max_str), U256::MAX);
    assert_eq!(token0.format_units_str(U256::MAX), u256_max_str);

    // 80 decimal digits (exceeds U256::MAX)
    let overflow_digits = "99999999999999999999999999999999999999999999999999999999999999999999999999999999";
    // Should gracefully return U256::ZERO without panicking
    assert_eq!(token0.parse_units_str(overflow_digits), U256::ZERO);

    // Oversized scaling: multiplying U256::MAX by 10^18 saturates or returns U256::MAX safely
    let large_in_18 = weth18.parse_units_str(&u256_max_str);
    assert_eq!(large_in_18, U256::MAX);

    // Oversized decimals: decimals >= 78 causes 10^decimals to overflow U256
    let token78 = Token::new(Address::ZERO, "T78", "Token78", 78, 1);
    assert_eq!(token78.parse_units_str("1.0"), U256::ZERO);
}

#[test]
fn test_stress_units_zero_precision_loss_vs_f64() {
    // Standard IEEE 754 f64 loses precision above 2^53 (approx 9.007 * 10^15).
    // An 18-decimal token with 100.123456789012345678 has 20 significant digits.
    // parse_units_str must retain all 20 digits with ZERO precision loss.
    let weth = Token::new(Address::ZERO, "WETH", "WETH", 18, 1);
    let high_precision_input = "100.123456789012345678";
    let parsed = weth.parse_units_str(high_precision_input);

    // Exact expected U256: 100 * 10^18 + 123456789012345678
    let expected = (U256::from(100) * U256::from(10u64.pow(18))) + U256::from(123456789012345678u64);
    assert_eq!(parsed, expected, "Must match exact integer without float rounding");

    // Format back to string must recover exact original string
    assert_eq!(weth.format_units_str(parsed), high_precision_input);
}

// =========================================================================
// SECTION 2: M5 & M6 (config.rs) ADVERSARIAL STRESS TESTS
// =========================================================================

#[test]
fn test_stress_config_missing_and_whitespace_alchemy_key() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("missing_and_whitespace");

    // Case 1: Missing ALCHEMY_KEY
    match AppConfig::load() {
        Err(ConfigError::MissingVar(ref v)) => assert_eq!(v, "ALCHEMY_KEY"),
        other => panic!("Expected MissingVar for missing ALCHEMY_KEY, got {:?}", other),
    }

    match Config::from_env() {
        Err(ConfigError::MissingVar(ref v)) => assert_eq!(v, "ALCHEMY_KEY"),
        other => panic!("Expected MissingVar for missing ALCHEMY_KEY via from_env, got {:?}", other),
    }

    // Case 2: Whitespace-only variants
    let whitespace_variants = ["", " ", "   ", "\t", "\n", "\r\n", "  \t \r\n  "];
    for ws in whitespace_variants {
        env::set_var("ALCHEMY_KEY", ws);
        match AppConfig::load() {
            Err(ConfigError::MissingVar(ref v)) => assert_eq!(v, "ALCHEMY_KEY"),
            other => panic!("Expected MissingVar for whitespace ALCHEMY_KEY '{:?}', got {:?}", ws, other),
        }
        match Config::from_env() {
            Err(ConfigError::MissingVar(ref v)) => assert_eq!(v, "ALCHEMY_KEY"),
            other => panic!("Expected MissingVar for whitespace ALCHEMY_KEY '{:?}' via from_env, got {:?}", ws, other),
        }
    }
}

#[test]
fn test_stress_config_live_mode_executor_address_validation() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("live_mode_validation");

    env::set_var("ALCHEMY_KEY", "valid_test_key_abc");

    // 1. Live mode with missing executor address -> defaults to Address::ZERO -> must fail fast
    env::set_var("EXECUTION_MODE", "live");
    env::remove_var("EXECUTOR_CONTRACT_ADDRESS");
    match AppConfig::load() {
        Err(ConfigError::InvalidValue { ref var, ref reason }) => {
            assert_eq!(var, "EXECUTOR_CONTRACT_ADDRESS");
            assert!(reason.contains("Address::ZERO"));
        }
        other => panic!("Expected InvalidValue in live mode with missing address, got {:?}", other),
    }

    // 2. Live mode with explicit Address::ZERO -> must fail fast
    env::set_var("EXECUTOR_CONTRACT_ADDRESS", "0x0000000000000000000000000000000000000000");
    match AppConfig::load() {
        Err(ConfigError::InvalidValue { ref var, ref reason }) => {
            assert_eq!(var, "EXECUTOR_CONTRACT_ADDRESS");
            assert!(reason.contains("Address::ZERO"));
        }
        other => panic!("Expected InvalidValue in live mode with explicit zero address, got {:?}", other),
    }

    // 3. Live mode with valid non-zero executor address -> must succeed
    let valid_addr = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
    env::set_var("EXECUTOR_CONTRACT_ADDRESS", valid_addr);
    let cfg_live = AppConfig::load().expect("Live mode with non-zero address must succeed");
    assert_eq!(cfg_live.executor_contract_address, Address::from_str(valid_addr).unwrap());
    assert!(cfg_live.execution_mode.is_live());

    // 4. Shadow mode with Address::ZERO -> must succeed (Address::ZERO is permissible in shadow mode)
    env::set_var("EXECUTION_MODE", "shadow");
    env::set_var("EXECUTOR_CONTRACT_ADDRESS", "0x0000000000000000000000000000000000000000");
    let cfg_shadow = AppConfig::load().expect("Shadow mode with Address::ZERO must succeed");
    assert_eq!(cfg_shadow.executor_contract_address, Address::ZERO);
    assert!(!cfg_shadow.execution_mode.is_live());
}

#[test]
fn test_stress_config_concurrent_env_access_and_modifications() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _scope = IsolatedEnvScope::new("concurrent_env");

    env::set_var("ALCHEMY_KEY", "concurrent_test_key");
    env::set_var("EXECUTION_MODE", "shadow");

    let stop_signal = Arc::new(AtomicBool::new(false));

    // Spawn 2 background threads continuously modifying non-critical env vars
    let mut mutator_handles = Vec::new();
    for thread_id in 0..2 {
        let stop = Arc::clone(&stop_signal);
        mutator_handles.push(std::thread::spawn(move || {
            let mut iter: u64 = 0;
            while !stop.load(Ordering::Relaxed) {
                iter += 1;
                if thread_id == 0 {
                    let hops = (iter % 5 + 1).to_string();
                    env::set_var("MAX_HOPS", &hops);
                } else {
                    let level = if iter % 2 == 0 { "info" } else { "debug" };
                    env::set_var("LOG_LEVEL", level);
                }
                std::hint::spin_loop();
            }
        }));
    }

    // Spawn 10 concurrent reader threads invoking AppConfig::load() and Config::from_env()
    let mut reader_handles = Vec::new();
    for _ in 0..10 {
        reader_handles.push(std::thread::spawn(|| {
            for _ in 0..500 {
                let cfg1 = AppConfig::load().expect("Concurrent AppConfig::load must not panic");
                assert_eq!(cfg1.alchemy_key, "concurrent_test_key");
                assert!(!cfg1.execution_mode.is_live());

                let cfg2 = Config::from_env().expect("Concurrent Config::from_env must not panic");
                assert_eq!(cfg2.alchemy_key, "concurrent_test_key");
            }
        }));
    }

    // Wait for all readers to complete
    for h in reader_handles {
        h.join().expect("Reader thread should not panic");
    }

    // Signal mutators to stop and join
    stop_signal.store(true, Ordering::Relaxed);
    for h in mutator_handles {
        h.join().expect("Mutator thread should not panic");
    }
}
