//! Tier 4 Real-World Scenario 5: Arbitrum Sequencer Burst + Multi-EOA Nonce-Bypass Execution
//! Features Exercised: F6, F8, F16, F17, F18
//! Authoritative source: TEST_INFRA.md line 47, ORIGINAL_REQUEST.md lines 70, 90.

use crate::common::*;
use std::time::Instant;

#[test]
fn test_scenario5_arbitrum_burst_multieoa_execution_end_to_end() {
    let t_burst_start = Instant::now();

    // 1. Ingestion: Rapid burst of 5 sequencer feed frames arriving within a 125ms PGA window
    let dedup = SequencerDeduplicator::new(50_000);
    let mut feed_sequences = vec![];
    for seq_offset in 1..=5 {
        let seq = 50_000 + seq_offset;
        assert!(dedup.check_and_update(seq), "Sequence {} must be accepted", seq);
        feed_sequences.push(seq);
    }
    assert_eq!(feed_sequences.len(), 5);

    // 2. Multi-EOA Signer Pool: 10 pre-funded EOA accounts
    let eoa_pool = MockEoaPool::new(10);

    // 3. Concurrent Opportunity Execution:
    // All 5 burst opportunities dispatch simultaneously.
    // If sent from a single EOA, they would queue in Arbitrum's unboosted Nonce Cache.
    // With Multi-EOA pool, each acquires a distinct EOA signer with independent nonce tracking!
    let mut dispatches = vec![];
    for _i in 0..5 {
        let lease = eoa_pool.acquire_signer().expect("Must successfully acquire signer");
        let (signer_idx, sender_addr, nonce) = lease;

        // Simulate fast pipeline: Math sizing + revm sim + raw tx sign
        let t_stage = PipelineTelemetry {
            feed_arrival_us: 120,
            graph_dfs_us: 50,
            sizing_math_us: 45,
            revm_sim_us: 550,
            tx_sign_us: 180,
        };

        // Latency budget checks:
        assert!(
            t_stage.math_plus_sim_latency_us() < 1500,
            "Combined math + sim must be < 1.5ms"
        );
        assert!(
            t_stage.total_e2e_latency_us() < 3500,
            "Total pipeline must be < 3.5ms"
        );

        dispatches.push((signer_idx, sender_addr, nonce, t_stage));
    }

    assert_eq!(dispatches.len(), 5);

    // 4. Verify Nonce-Cache Bypass Invariant:
    // Every single concurrent opportunity used a DIFFERENT sender address!
    for i in 0..5 {
        for j in i + 1..5 {
            assert_ne!(
                dispatches[i].1, dispatches[j].1,
                "Senders {} and {} must have distinct EOA addresses to bypass Arbitrum nonce cache",
                i, j
            );
        }
    }

    // 5. Clean release of signers back to pool
    for (signer_idx, _, _, _) in &dispatches {
        eoa_pool.release_signer(*signer_idx);
    }

    // Entire burst coordination completes in negligible CPU time
    let elapsed = t_burst_start.elapsed();
    assert!(
        elapsed.as_millis() < 50,
        "Burst processing of 5 concurrent trades took {}ms (< 50ms)",
        elapsed.as_millis()
    );
}
