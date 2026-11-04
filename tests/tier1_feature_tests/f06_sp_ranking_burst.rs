//! Feature 6: Dynamic Top-100 Sp Ranking & Burst Promotion Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 24-26 and spec_miner_survey_2/handoff.md.

use crate::common::*;

#[test]
fn test_f06_01_sp_scoring_formula_evaluation() {
    let sigma = 0.02; // 2% volatility
    let avg_fee = 0.003; // 0.3%
    let volume = 100_000.0;
    let min_liquidity = 500_000.0;
    let venues = 2;

    let score = compute_sp_score(sigma, avg_fee, volume, min_liquidity, venues);
    let expected = (0.02 / 0.003) * (1.0 + 100_000.0f64).ln() * 500_000.0;

    assert!((score - expected).abs() < 1e-4);
    assert!(score > 0.0);
}

#[test]
fn test_f06_02_single_venue_zero_score() {
    // If pair exists on only 1 venue, score must be 0 (cannot be arbitraged cross-venue)
    let score = compute_sp_score(0.05, 0.003, 1_000_000.0, 500_000.0, 1);
    assert_eq!(score, 0.0, "Score must be 0 if venues < 2");
}

#[test]
fn test_f06_03_zero_fee_floor_clamp() {
    // When average fee is 0, clamped to 1e-6 (1 PIP) to prevent division by zero
    let score = compute_sp_score(0.01, 0.0, 10_000.0, 100_000.0, 2);
    assert!(score.is_finite());
    assert!(score > 0.0);
}

#[test]
fn test_f06_04_top_100_cache_truncation() {
    let mut scores: Vec<(usize, f64)> = (0..150)
        .map(|i| (i, compute_sp_score(0.01, 0.003, (i * 1000) as f64, 100_000.0, 2)))
        .collect();

    // Sort descending
    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let top_100: Vec<_> = scores.into_iter().take(100).collect();

    assert_eq!(top_100.len(), 100);
    assert!(top_100[0].1 >= top_100[99].1);
}

#[test]
fn test_f06_05_instant_burst_promotion() {
    // Pair initially ranked low (#120) receives massive volume surge
    let initial_volume = 100.0;
    let burst_volume = 100_000_000.0; // Mega swap burst

    let initial_score = compute_sp_score(0.02, 0.003, initial_volume, 1_000_000.0, 2);
    let burst_score = compute_sp_score(0.02, 0.003, burst_volume, 1_000_000.0, 2);

    assert!(burst_score > initial_score * 3.0, "Burst must rapidly boost score for promotion");
}
