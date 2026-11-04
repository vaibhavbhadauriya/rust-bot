//! Tier 4 Application Scenarios Test Runner
//! Alias test target supporting both `tier4_realworld_tests` and `tier4_application_scenarios`.

mod common;

#[path = "tier4_realworld_tests/mod.rs"]
mod tier4_realworld_tests;

#[test]
fn scenario_3() {
    tier4_realworld_tests::scenario3_morpho_frontrun_revert::test_scenario3_morpho_frontrun_fast_revert_end_to_end();
}

#[test]
fn scenario_4() {
    tier4_realworld_tests::scenario4_salmonella_honeypot::test_scenario4_salmonella_honeypot_defense_end_to_end();
}

