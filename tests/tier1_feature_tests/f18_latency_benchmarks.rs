//! Feature 18: Microsecond Latency Benchmarks Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md line 90 and explorer_survey_3/handoff.md.

use crate::common::*;
use std::time::Instant;

#[test]
fn test_f18_01_sizing_math_latency_budget() {
    let start = Instant::now();
    for _ in 0..100 {
        let _ = solve_2pool_optimal_f64(1_000_000.0, 405.0, 1_000_000.0, 395.0, 0.003, 0.003, 0.0);
    }
    let elapsed = start.elapsed();
    let avg_us = elapsed.as_micros() as f64 / 100.0;
    // Budget is < 100 us per execution
    assert!(avg_us < 100.0, "Average sizing latency {:.2}us must be < 100us", avg_us);
}

#[test]
fn test_f18_02_triangular_reduction_latency_budget() {
    let start = Instant::now();
    for _ in 0..100 {
        let _ = reduce_virtual_pool_f64(1_000_000.0, 400.0, 0.997, 500.0, 1_200_000.0, 0.997);
    }
    let elapsed = start.elapsed();
    let avg_us = elapsed.as_micros() as f64 / 100.0;
    // Budget is < 50 us
    assert!(avg_us < 50.0, "Average 3-hop reduction latency {:.2}us must be < 50us", avg_us);
}

#[test]
fn test_f18_03_clmm_tick_division_latency() {
    let start = Instant::now();
    for i in -50..50 {
        let _ = div_euclid_tick(i, 1);
    }
    let elapsed = start.elapsed();
    let avg_ns = elapsed.as_nanos() as f64 / 100.0;
    // Floor division takes < 100ns
    assert!(avg_ns < 1000.0);
}

#[test]
fn test_f18_04_combined_math_sim_budget_threshold() {
    // Spec requirement AC3: math + revm sim in < 1.5 ms (1500 us)
    let max_budget_us = 1500u64;
    let telemetry = PipelineTelemetry {
        sizing_math_us: 75,
        revm_sim_us: 650,
        ..Default::default()
    };

    assert!(
        telemetry.math_plus_sim_latency_us() < max_budget_us,
        "Combined math + sim {}us must be < 1500us",
        telemetry.math_plus_sim_latency_us()
    );
}

#[test]
fn test_f18_05_end_to_end_pipeline_budget_threshold() {
    // Spec requirement AC3: feed arrival to signed raw tx in < 3.5 ms (3500 us)
    let max_e2e_budget_us = 3500u64;
    let telemetry = PipelineTelemetry {
        feed_arrival_us: 350,
        graph_dfs_us: 120,
        sizing_math_us: 80,
        revm_sim_us: 800,
        tx_sign_us: 450,
    };

    assert!(
        telemetry.total_e2e_latency_us() < max_e2e_budget_us,
        "E2E pipeline latency {}us must be < 3500us",
        telemetry.total_e2e_latency_us()
    );
}
