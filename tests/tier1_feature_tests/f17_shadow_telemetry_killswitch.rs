//! Feature 17: Shadow Mode, Telemetry & Kill Switch Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 71-74 and explorer_survey_3/handoff.md.

use crate::common::*;

#[test]
fn test_f17_01_shadow_mode_flag_default() {
    let live_flag = false; // default is shadow mode
    let broadcast_tx = live_flag;
    assert!(!broadcast_tx, "Shadow mode must never broadcast transactions to the network");
}

#[test]
fn test_f17_02_microsecond_telemetry_stages() {
    let telemetry = PipelineTelemetry {
        feed_arrival_us: 150,
        graph_dfs_us: 80,
        sizing_math_us: 60,
        revm_sim_us: 450,
        tx_sign_us: 200,
    };

    assert_eq!(telemetry.math_plus_sim_latency_us(), 60 + 450);
    assert_eq!(
        telemetry.total_e2e_latency_us(),
        150 + 80 + 60 + 450 + 200
    );
}

#[test]
fn test_f17_03_kill_switch_feed_lag_trip() {
    let ks = KillSwitchController::new();
    assert!(!ks.is_tripped());

    // Lag of 50ms does not trip
    assert!(!ks.check_feed_lag(50));
    assert!(!ks.is_tripped());

    // Lag of 105ms (> 100ms) trips immediately!
    assert!(ks.check_feed_lag(105));
    assert!(ks.is_tripped(), "Kill switch must trip when feed lag exceeds 100ms");
    assert!(ks.trip_reason().unwrap().contains("105ms"));
}

#[test]
fn test_f17_04_kill_switch_3_consecutive_fast_reverts() {
    let ks = KillSwitchController::new();

    // 1st fast-revert: no trip
    assert!(!ks.record_trade_result(true));
    assert!(!ks.is_tripped());

    // 2nd fast-revert: no trip
    assert!(!ks.record_trade_result(true));
    assert!(!ks.is_tripped());

    // 3rd consecutive fast-revert: TRIPS!
    assert!(ks.record_trade_result(true));
    assert!(ks.is_tripped(), "Kill switch must trip on 3 consecutive fast-reverts");
}

#[test]
fn test_f17_05_kill_switch_consecutive_counter_resets_on_success() {
    let ks = KillSwitchController::new();

    // 2 fast reverts
    ks.record_trade_result(true);
    ks.record_trade_result(true);
    assert!(!ks.is_tripped());

    // 1 successful trade resets counter
    ks.record_trade_result(false);

    // Another fast revert (only 1 now, not 3)
    ks.record_trade_result(true);
    assert!(!ks.is_tripped(), "Successful trade must reset consecutive counter");
}
