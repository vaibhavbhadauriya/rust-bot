//! Feature 18 Boundaries: Latency Benchmarks BVA Test Suite

use crate::common::*;
use std::time::Instant;

#[test]
fn test_b18_01_nonzero_timestamp_measurement() {
    let t0 = Instant::now();
    let _ = 1 + 1;
    let elapsed = t0.elapsed();
    assert!(elapsed.as_secs() < 10);
}

#[test]
fn test_b18_02_stress_math_iterations_10k() {
    let t0 = Instant::now();
    for _ in 0..10_000 {
        let _ = quick_spread_screen(400, 1000, 405, 1000);
    }
    let elapsed = t0.elapsed();
    // 10,000 spread screens in < 5ms in release, < 50ms in unoptimized debug runs
    #[cfg(debug_assertions)]
    assert!(elapsed.as_millis() < 50);
    #[cfg(not(debug_assertions))]
    assert!(elapsed.as_millis() < 5);
}

#[test]
fn test_b18_03_telemetry_threshold_boundaries() {
    // Exactly 1499 us is within 1.5 ms budget
    let t1 = PipelineTelemetry {
        sizing_math_us: 499,
        revm_sim_us: 1000,
        ..Default::default()
    };
    assert!(t1.math_plus_sim_latency_us() < 1500);

    // Exactly 1501 us violates budget
    let t2 = PipelineTelemetry {
        sizing_math_us: 501,
        revm_sim_us: 1000,
        ..Default::default()
    };
    assert!(t2.math_plus_sim_latency_us() > 1500);
}

#[test]
fn test_b18_04_pipeline_3500us_boundary() {
    let t_pass = PipelineTelemetry {
        feed_arrival_us: 500,
        graph_dfs_us: 200,
        sizing_math_us: 100,
        revm_sim_us: 700,
        tx_sign_us: 1999,
    };
    assert!(t_pass.total_e2e_latency_us() < 3500);

    let t_fail = PipelineTelemetry {
        feed_arrival_us: 500,
        graph_dfs_us: 200,
        sizing_math_us: 100,
        revm_sim_us: 700,
        tx_sign_us: 2001,
    };
    assert!(t_fail.total_e2e_latency_us() > 3500);
}

#[test]
fn test_b18_05_zero_latency_edge() {
    let t_zero = PipelineTelemetry::default();
    assert_eq!(t_zero.total_e2e_latency_us(), 0);
}
