//! Adversarial Stress & Boundary Verification Test Suite for Milestone 5
//!
//! Evaluates Kill Switch boundary conditions, fast-revert counter reset and latching invariants,
//! hot-path sub-10ns lock-free atomic latency, and microsecond telemetry budgets.
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 71-74, 90; PROJECT.md lines 63, 64, 76.

use flash_strategy::kill_switch::{
    KillSwitchController, KillSwitchError, DEFAULT_FEED_LAG_THRESHOLD_MS,
};
use flash_strategy::telemetry::{PipelineTelemetry, TelemetryCollector};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

// =========================================================================
// VECTOR 1: Feed Lag Exact Boundary Conditions & Parameter Sensitivity
// =========================================================================

#[test]
fn test_challenger_v1_01_feed_lag_exact_100ms_boundary_does_not_trip() {
    let ks = KillSwitchController::new();
    assert!(!ks.is_tripped(), "Initial state must be untripped");
    assert!(ks.assert_healthy().is_ok());

    // Exact boundary: 100ms must NOT trip according to spec (> 100ms)
    let tripped = ks.check_feed_lag(DEFAULT_FEED_LAG_THRESHOLD_MS);
    assert!(!tripped, "check_feed_lag(100) must return false");
    assert!(!ks.is_tripped(), "Kill switch must NOT trip at exactly 100ms");
    assert!(ks.assert_healthy().is_ok());
    assert_eq!(ks.total_trips(), 0);
    assert!(ks.trip_reason().is_none());
}

#[test]
fn test_challenger_v1_02_feed_lag_exact_101ms_boundary_must_trip() {
    let ks = KillSwitchController::new();

    // Exact boundary: 101ms MUST trip (> 100ms)
    let tripped = ks.check_feed_lag(DEFAULT_FEED_LAG_THRESHOLD_MS + 1);
    assert!(tripped, "check_feed_lag(101) must return true");
    assert!(ks.is_tripped(), "Kill switch MUST trip at exactly 101ms");
    assert_eq!(ks.total_trips(), 1);

    let err = ks.assert_healthy().unwrap_err();
    match err {
        KillSwitchError::Halted(reason) => {
            assert!(
                reason.contains("101ms") && reason.contains("100ms"),
                "Trip reason must detail the 101ms violation: {}",
                reason
            );
        }
    }
}

#[test]
fn test_challenger_v1_03_feed_lag_sub_threshold_gradient() {
    let ks = KillSwitchController::new();
    let sub_threshold_values = [0, 1, 10, 25, 50, 75, 90, 95, 98, 99, 100];

    for &lag in &sub_threshold_values {
        assert!(
            !ks.check_feed_lag(lag),
            "check_feed_lag({}) must not trip",
            lag
        );
        assert!(
            !ks.is_tripped(),
            "Kill switch must stay untripped for lag {}",
            lag
        );
    }
}

#[test]
fn test_challenger_v1_04_feed_lag_extreme_exceeded_values() {
    let test_cases = [102, 150, 200, 500, 1_000, 10_000, 60_000, u64::MAX];

    for &lag in &test_cases {
        let ks = KillSwitchController::new();
        assert!(
            ks.check_feed_lag(lag),
            "check_feed_lag({}) must trip",
            lag
        );
        assert!(ks.is_tripped(), "Kill switch must trip for lag {}", lag);
        assert_eq!(ks.total_trips(), 1);
    }
}

#[test]
fn test_challenger_v1_05_custom_feed_lag_thresholds() {
    // Custom threshold: 50ms
    let ks_50 = KillSwitchController::with_thresholds(50, 3);
    assert!(!ks_50.check_feed_lag(50), "50ms does not trip threshold of 50ms");
    assert!(ks_50.check_feed_lag(51), "51ms trips threshold of 50ms");

    // Custom threshold: 0ms (any lag > 0 trips)
    let ks_0 = KillSwitchController::with_thresholds(0, 3);
    assert!(!ks_0.check_feed_lag(0), "0ms does not trip threshold of 0ms");
    assert!(ks_0.check_feed_lag(1), "1ms trips threshold of 0ms");

    // Custom threshold: 250ms
    let ks_250 = KillSwitchController::with_thresholds(250, 3);
    assert!(!ks_250.check_feed_lag(250), "250ms does not trip threshold of 250ms");
    assert!(ks_250.check_feed_lag(251), "251ms trips threshold of 250ms");
}

// =========================================================================
// VECTOR 2: Fast-Revert Consecutive Counter & State Machine Invariants
// =========================================================================

#[test]
fn test_challenger_v2_01_exactly_two_consecutive_fast_reverts_does_not_trip() {
    let ks = KillSwitchController::new();
    assert_eq!(ks.consecutive_fast_reverts(), 0);

    // 1st fast-revert
    let trip_1 = ks.record_trade_result(true);
    assert!(!trip_1, "1st fast revert must not trip");
    assert!(!ks.is_tripped(), "Kill switch must not trip at 1 revert");
    assert_eq!(ks.consecutive_fast_reverts(), 1);
    assert!(ks.assert_healthy().is_ok());

    // 2nd fast-revert
    let trip_2 = ks.record_trade_result(true);
    assert!(!trip_2, "2nd fast revert must not trip");
    assert!(!ks.is_tripped(), "Kill switch must not trip at 2 reverts");
    assert_eq!(ks.consecutive_fast_reverts(), 2);
    assert!(ks.assert_healthy().is_ok());
    assert_eq!(ks.total_trips(), 0);
}

#[test]
fn test_challenger_v2_02_exactly_three_consecutive_fast_reverts_must_trip() {
    let ks = KillSwitchController::new();

    ks.record_trade_result(true);
    ks.record_trade_result(true);

    // 3rd fast-revert
    let trip_3 = ks.record_trade_result(true);
    assert!(trip_3, "3rd fast revert MUST trip");
    assert!(ks.is_tripped(), "Kill switch MUST trip at 3 consecutive fast reverts");
    assert_eq!(ks.consecutive_fast_reverts(), 3);
    assert_eq!(ks.total_trips(), 1);

    let err = ks.assert_healthy().unwrap_err();
    match err {
        KillSwitchError::Halted(reason) => {
            assert!(
                reason.contains("3 consecutive fast-reverts"),
                "Trip reason must detail 3 consecutive reverts: {}",
                reason
            );
        }
    }
}

#[test]
fn test_challenger_v2_03_honest_trade_resets_consecutive_counter_if_healthy() {
    let ks = KillSwitchController::new();

    // 2 reverts followed by 1 honest trade
    ks.record_trade_result(true);
    ks.record_trade_result(true);
    assert_eq!(ks.consecutive_fast_reverts(), 2);
    assert!(!ks.is_tripped());

    // Honest trade
    let tripped = ks.record_trade_result(false);
    assert!(!tripped);
    assert_eq!(
        ks.consecutive_fast_reverts(),
        0,
        "Honest trade must reset consecutive revert counter to 0"
    );
    assert!(!ks.is_tripped());

    // Another 2 fast reverts followed by honest trade
    ks.record_trade_result(true);
    ks.record_trade_result(true);
    assert_eq!(ks.consecutive_fast_reverts(), 2);
    assert!(!ks.is_tripped());

    ks.record_trade_result(false);
    assert_eq!(ks.consecutive_fast_reverts(), 0);
    assert!(!ks.is_tripped());

    // Rapid alternating pattern: revert, honest, revert, honest (1,000 iterations)
    for _ in 0..1_000 {
        ks.record_trade_result(true);
        assert!(!ks.is_tripped());
        ks.record_trade_result(false);
        assert_eq!(ks.consecutive_fast_reverts(), 0);
        assert!(!ks.is_tripped());
    }
    assert_eq!(ks.total_trips(), 0);
}

#[test]
fn test_challenger_v2_04_custom_revert_thresholds() {
    // Custom threshold: 1 revert trips immediately
    let ks_1 = KillSwitchController::with_thresholds(100, 1);
    assert!(ks_1.record_trade_result(true));
    assert!(ks_1.is_tripped());

    // Custom threshold: 5 reverts
    let ks_5 = KillSwitchController::with_thresholds(100, 5);
    for _ in 0..4 {
        assert!(!ks_5.record_trade_result(true));
        assert!(!ks_5.is_tripped());
    }
    assert_eq!(ks_5.consecutive_fast_reverts(), 4);
    assert!(ks_5.record_trade_result(true), "5th revert trips");
    assert!(ks_5.is_tripped());
}

// =========================================================================
// VECTOR 3: Latching Invariant & Explicit Reset Lifecycle
// =========================================================================

#[test]
fn test_challenger_v3_01_tripped_by_feed_lag_latches_against_honest_trades() {
    let ks = KillSwitchController::new();

    // Trip via feed lag
    ks.check_feed_lag(120);
    assert!(ks.is_tripped(), "Must be tripped by feed lag");

    // Attempt to un-trip via honest trade results: CANNOT un-trip
    for i in 0..50 {
        let tripped = ks.record_trade_result(false);
        assert!(!tripped, "record_trade_result(false) returns false");
        assert!(
            ks.is_tripped(),
            "Kill switch MUST remain tripped after honest trade #{}",
            i
        );
        assert!(
            ks.assert_healthy().is_err(),
            "assert_healthy must remain Err after honest trade #{}",
            i
        );
    }

    // Attempt to un-trip via healthy feed lag check: CANNOT un-trip
    for lag in [0, 10, 50, 99, 100] {
        ks.check_feed_lag(lag);
        assert!(
            ks.is_tripped(),
            "Kill switch MUST remain tripped after healthy lag check {}",
            lag
        );
    }
}

#[test]
fn test_challenger_v3_02_tripped_by_reverts_latches_against_honest_trades() {
    let ks = KillSwitchController::new();

    // 3 reverts trip the switch
    ks.record_trade_result(true);
    ks.record_trade_result(true);
    ks.record_trade_result(true);
    assert!(ks.is_tripped());

    // Attempt to un-trip via honest trade
    ks.record_trade_result(false);
    assert!(
        ks.is_tripped(),
        "Honest trade CANNOT un-trip a tripped kill switch"
    );
    assert!(ks.assert_healthy().is_err());
}

#[test]
fn test_challenger_v3_03_explicit_reset_fully_restores_healthy_state() {
    let ks = KillSwitchController::new();

    // Trip and verify
    ks.trip("Emergency test halt");
    assert!(ks.is_tripped());
    assert_eq!(ks.total_trips(), 1);
    assert!(ks.trip_reason().is_some());

    // Explicit reset
    ks.reset();

    // Verify completely restored state
    assert!(!ks.is_tripped(), "Must be untripped after reset()");
    assert!(ks.assert_healthy().is_ok(), "Must be healthy after reset()");
    assert_eq!(
        ks.consecutive_fast_reverts(),
        0,
        "Consecutive reverts must be 0 after reset()"
    );
    assert!(ks.trip_reason().is_none(), "Trip reason must be None after reset()");

    // Re-verify it trips properly again after reset
    ks.check_feed_lag(150);
    assert!(ks.is_tripped(), "Must be capable of tripping again after reset");
    assert_eq!(ks.total_trips(), 2, "total_trips must track lifetime trips across resets");
}

#[test]
fn test_challenger_v3_04_multiple_redundant_trips_idempotency() {
    let ks = KillSwitchController::new();

    ks.trip("First trip reason");
    assert_eq!(ks.total_trips(), 1);
    let original_reason = ks.trip_reason().unwrap();
    assert_eq!(original_reason, "First trip reason");

    // Second trip call while already tripped must NOT overwrite reason or double-count total_trips
    ks.trip("Second trip reason");
    assert_eq!(ks.total_trips(), 1, "Idempotent trip must not increment total_trips");
    assert_eq!(
        ks.trip_reason().unwrap(),
        "First trip reason",
        "Original trip reason must be preserved"
    );

    // Feed lag trip while already tripped
    ks.check_feed_lag(200);
    assert_eq!(ks.total_trips(), 1);
}

// =========================================================================
// VECTOR 4: Hot-Path Lock-Free Atomic Latency Empirical Verification
// =========================================================================

#[test]
fn test_challenger_v4_01_hot_path_is_tripped_latency() {
    let ks = KillSwitchController::new();

    // Warm up
    for _ in 0..10_000 {
        std::hint::black_box(ks.is_tripped());
    }

    // Benchmark 1,000,000 iterations
    let iterations = 1_000_000;
    let t0 = Instant::now();
    for _ in 0..iterations {
        let _ = std::hint::black_box(ks.is_tripped());
    }
    let elapsed = t0.elapsed();
    let ns_per_call = elapsed.as_nanos() as f64 / iterations as f64;

    println!(
        "is_tripped() latency: {:.2} ns/call across {} iterations (total: {:?})",
        ns_per_call, iterations, elapsed
    );

    // In release mode this is ~1-2ns; in unoptimized debug test profile, allow generous upper bound < 200ns
    let max_budget_ns = if cfg!(debug_assertions) { 200.0 } else { 10.0 };
    assert!(
        ns_per_call < max_budget_ns,
        "is_tripped() took {:.2}ns, exceeding budget of {}ns",
        ns_per_call,
        max_budget_ns
    );
}

#[test]
fn test_challenger_v4_02_hot_path_assert_healthy_latency() {
    let ks = KillSwitchController::new();

    // Warm up
    for _ in 0..10_000 {
        let _ = std::hint::black_box(ks.assert_healthy());
    }

    let iterations = 1_000_000;
    let t0 = Instant::now();
    for _ in 0..iterations {
        let _ = std::hint::black_box(ks.assert_healthy());
    }
    let elapsed = t0.elapsed();
    let ns_per_call = elapsed.as_nanos() as f64 / iterations as f64;

    println!(
        "assert_healthy() latency: {:.2} ns/call across {} iterations (total: {:?})",
        ns_per_call, iterations, elapsed
    );

    let max_budget_ns = if cfg!(debug_assertions) { 250.0 } else { 15.0 };
    assert!(
        ns_per_call < max_budget_ns,
        "assert_healthy() took {:.2}ns, exceeding budget of {}ns",
        ns_per_call,
        max_budget_ns
    );
}

// =========================================================================
// VECTOR 5: High-Concurrency & Multi-Threaded Contention Stress
// =========================================================================

#[test]
fn test_challenger_v5_01_multithreaded_concurrent_readers_and_writers() {
    let ks = Arc::new(KillSwitchController::new());
    let stop_signal = Arc::new(AtomicBool::new(false));

    let mut handles = Vec::new();

    // Spawn 8 reader threads continuously polling is_tripped()
    for _ in 0..8 {
        let ks_clone = Arc::clone(&ks);
        let stop_clone = Arc::clone(&stop_signal);
        handles.push(thread::spawn(move || {
            let mut read_count = 0u64;
            while !stop_clone.load(Ordering::Relaxed) {
                let _ = std::hint::black_box(ks_clone.is_tripped());
                read_count += 1;
            }
            read_count
        }));
    }

    // Spawn 4 writer threads recording trades
    for id in 0..4 {
        let ks_clone = Arc::clone(&ks);
        let stop_clone = Arc::clone(&stop_signal);
        handles.push(thread::spawn(move || {
            let mut trade_count = 0u64;
            while !stop_clone.load(Ordering::Relaxed) {
                // Mix of honest trades and fast reverts
                if id % 2 == 0 {
                    ks_clone.record_trade_result(false);
                } else {
                    ks_clone.check_feed_lag(95);
                }
                trade_count += 1;
            }
            trade_count
        }));
    }

    // Let threads hammer the controller for 200 milliseconds
    thread::sleep(std::time::Duration::from_millis(200));

    // Stop and join all threads
    stop_signal.store(true, Ordering::SeqCst);
    let mut total_ops = 0u64;
    for handle in handles {
        total_ops += handle.join().expect("Thread joined cleanly");
    }

    println!(
        "High-concurrency kill switch test completed {} total operations without panic or race",
        total_ops
    );
    assert!(!ks.is_tripped(), "Feed lag 95ms and honest trades must not trip");
}

#[test]
fn test_challenger_v5_02_concurrent_trip_and_reset_cycles() {
    let ks = Arc::new(KillSwitchController::new());
    let iterations = 1000;

    let ks_tripper = Arc::clone(&ks);
    let t1 = thread::spawn(move || {
        for _ in 0..iterations {
            ks_tripper.trip("Concurrent trip");
            thread::yield_now();
        }
    });

    let ks_resetter = Arc::clone(&ks);
    let t2 = thread::spawn(move || {
        for _ in 0..iterations {
            ks_resetter.reset();
            thread::yield_now();
        }
    });

    t1.join().unwrap();
    t2.join().unwrap();

    // Final reset to ensure deterministic end
    ks.reset();
    assert!(!ks.is_tripped());
    assert!(ks.assert_healthy().is_ok());
    assert!(ks.total_trips() > 0, "At least some trips occurred");
}

// =========================================================================
// VECTOR 6: Latency Budget Telemetry Calculations & Boundary Precision
// =========================================================================

#[test]
fn test_challenger_v6_01_telemetry_exact_budget_boundaries() {
    // 1. Math + Sim Budget: 1500 us
    // Boundary 1: 1499 us (< 1500 us) -> PASS
    let t_1499 = PipelineTelemetry {
        sizing_math_us: 499,
        revm_sim_us: 1000,
        ..Default::default()
    };
    assert_eq!(t_1499.math_plus_sim_latency_us(), 1499);
    assert!(t_1499.math_plus_sim_latency_us() < 1500);

    // Boundary 2: 1500 us (exactly on budget, NOT < 1500 us) -> BUDGET REACHED
    let t_1500 = PipelineTelemetry {
        sizing_math_us: 500,
        revm_sim_us: 1000,
        ..Default::default()
    };
    assert_eq!(t_1500.math_plus_sim_latency_us(), 1500);
    assert!(!(t_1500.math_plus_sim_latency_us() < 1500));

    // Boundary 3: 1501 us (> 1500 us) -> EXCEEDED
    let t_1501 = PipelineTelemetry {
        sizing_math_us: 501,
        revm_sim_us: 1000,
        ..Default::default()
    };
    assert_eq!(t_1501.math_plus_sim_latency_us(), 1501);
    assert!(t_1501.math_plus_sim_latency_us() > 1500);

    // 2. Full Shadow Pipeline Budget: 3500 us
    // Boundary 1: 3499 us (< 3500 us) -> PASS
    let t_3499 = PipelineTelemetry {
        feed_arrival_us: 500,
        graph_dfs_us: 200,
        sizing_math_us: 100,
        revm_sim_us: 700,
        tx_sign_us: 1999,
    };
    assert_eq!(t_3499.total_e2e_latency_us(), 3499);
    assert!(t_3499.total_e2e_latency_us() < 3500);

    // Boundary 2: 3500 us (exactly on budget) -> NOT < 3500 us
    let t_3500 = PipelineTelemetry {
        feed_arrival_us: 500,
        graph_dfs_us: 200,
        sizing_math_us: 100,
        revm_sim_us: 700,
        tx_sign_us: 2000,
    };
    assert_eq!(t_3500.total_e2e_latency_us(), 3500);
    assert!(!(t_3500.total_e2e_latency_us() < 3500));

    // Boundary 3: 3501 us (> 3500 us) -> EXCEEDED
    let t_3501 = PipelineTelemetry {
        feed_arrival_us: 500,
        graph_dfs_us: 200,
        sizing_math_us: 100,
        revm_sim_us: 700,
        tx_sign_us: 2001,
    };
    assert_eq!(t_3501.total_e2e_latency_us(), 3501);
    assert!(t_3501.total_e2e_latency_us() > 3500);
}

#[test]
fn test_challenger_v6_02_telemetry_collector_percentiles_and_outliers() {
    let collector = TelemetryCollector::new();

    // Record 100 samples with varying latencies
    for i in 1..=100 {
        let telemetry = PipelineTelemetry {
            feed_arrival_us: 10,
            graph_dfs_us: 10,
            sizing_math_us: 20,
            revm_sim_us: 50,
            tx_sign_us: i * 10, // 10us to 1000us
        };
        collector.record(&telemetry);
    }

    assert_eq!(collector.sample_count.load(Ordering::Relaxed), 100);
    assert!(collector.avg_e2e_latency_us() > 0.0);
    let p99 = collector.p99_latency_us();
    assert!(p99 >= 900, "P99 should reflect high-percentile latency: {}", p99);
}
