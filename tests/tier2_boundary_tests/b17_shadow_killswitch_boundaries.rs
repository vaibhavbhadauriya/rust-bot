//! Feature 17 Boundaries: Shadow Mode & Kill Switch BVA Test Suite

use crate::common::*;

#[test]
fn test_b17_01_feed_lag_exactly_100ms_boundary() {
    let ks = KillSwitchController::new();
    // 100ms is exactly on the boundary: condition is > 100ms
    assert!(!ks.check_feed_lag(100));
    assert!(!ks.is_tripped());
}

#[test]
fn test_b17_02_feed_lag_101ms_trips() {
    let ks = KillSwitchController::new();
    assert!(ks.check_feed_lag(101));
    assert!(ks.is_tripped());
}

#[test]
fn test_b17_03_two_consecutive_fast_reverts_boundary() {
    let ks = KillSwitchController::new();
    ks.record_trade_result(true);
    ks.record_trade_result(true);
    assert!(!ks.is_tripped(), "2 consecutive fast reverts must not trip (threshold is >= 3)");
}

#[test]
fn test_b17_04_three_consecutive_fast_reverts_boundary() {
    let ks = KillSwitchController::new();
    ks.record_trade_result(true);
    ks.record_trade_result(true);
    ks.record_trade_result(true);
    assert!(ks.is_tripped(), "3 consecutive fast reverts must trip");
}

#[test]
fn test_b17_05_kill_switch_latches_tripped_state() {
    let ks = KillSwitchController::new();
    ks.check_feed_lag(150);
    assert!(ks.is_tripped());

    // Subsequent normal event cannot un-trip
    ks.record_trade_result(false);
    assert!(ks.is_tripped(), "Kill switch must latch in tripped state until manual reset");
}
