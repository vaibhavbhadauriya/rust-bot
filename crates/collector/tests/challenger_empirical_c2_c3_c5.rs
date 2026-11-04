//! Adversarial Empirical Challenge Suite for Milestone M_R3 (C2, C3, C5)
//! Tested by: m_r3_challenger_1

use alloy_consensus::{Signed, TxEip1559, TxEip2930, TxEnvelope, TxLegacy};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, Signature, TxKind, B256, U256};
use base64::Engine;
use flash_collector::{
    unpack_arbitrum_batch, unpack_l2_message, ArbitrumFeedError, ArbitrumFeedHandler,
    ArbitrumTxType, BaseFlashblockHandler, BatchPayload, DexCalldataInspector, FastTxDecoder,
    FlashblockGasTracker, FlashblockMessage, MarketFeedPublisher, MarketUpdate, MockTx,
    SequencerDeduplicator, TOPIC_UNIV2_SYNC, TOPIC_UNIV3_SWAP,
};
use flash_core::types::Chain;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

// ============================================================================
// CHALLENGE C3: Keccak selector verification for Uniswap V2 swap()
// ============================================================================

#[test]
fn test_c3_keccak_selector_oracle_and_bytecode_consistency() {
    // 1. Compute Keccak-256 of "swap(uint256,uint256,address,bytes)" from first principles
    let signature = "swap(uint256,uint256,address,bytes)";
    let hash = alloy_primitives::keccak256(signature.as_bytes());
    let computed_selector = [hash[0], hash[1], hash[2], hash[3]];

    // Expected selector: 0x022c0d9f
    assert_eq!(
        computed_selector,
        [0x02, 0x2c, 0x0d, 0x9f],
        "Computed selector MUST match 0x022c0d9f"
    );

    // Flawed selector 0x022c089f MUST be explicitly rejected
    let flawed_selector = [0x02, 0x2c, 0x08, 0x9f];
    assert_ne!(
        computed_selector, flawed_selector,
        "Computed selector MUST NOT match flawed 0x022c089f"
    );

    // 2. Verify DexCalldataInspector::UNIV2_SWAP constant
    assert_eq!(
        DexCalldataInspector::UNIV2_SWAP,
        [0x02, 0x2c, 0x0d, 0x9f],
        "DexCalldataInspector::UNIV2_SWAP must be 0x022c0d9f"
    );

    // 3. Verify calldata matching
    let valid_swap_calldata = vec![0x02, 0x2c, 0x0d, 0x9f, 0xaa, 0xbb, 0xcc];
    assert!(
        DexCalldataInspector::is_dex_swap(&valid_swap_calldata),
        "Valid Uniswap V2 calldata must be identified as DEX swap"
    );
    assert_eq!(
        DexCalldataInspector::extract_selector(&valid_swap_calldata),
        Some([0x02, 0x2c, 0x0d, 0x9f])
    );

    // Flawed selector in calldata must NOT match
    let flawed_swap_calldata = vec![0x02, 0x2c, 0x08, 0x9f, 0xaa, 0xbb, 0xcc];
    assert!(
        !DexCalldataInspector::is_dex_swap(&flawed_swap_calldata),
        "Flawed Uniswap V2 selector 0x022c089f must NOT be recognized as DEX swap"
    );
}

// ============================================================================
// CHALLENGE C5: Arbitrum Nitro Feed Unmarshaling, Batch Unpacking, RLP Decoding
// ============================================================================

fn make_dummy_sig() -> Signature {
    Signature::from_scalars_and_parity(B256::repeat_byte(0x11), B256::repeat_byte(0x22), false)
}

#[test]
fn test_c5_authentic_rlp_decoding_all_types() {
    let to_pool = Address::repeat_byte(0x42);
    let calldata = Bytes::from(vec![0x02, 0x2c, 0x0d, 0x9f, 0x01, 0x02, 0x03, 0x04]);
    let sig = make_dummy_sig();

    // 1. Legacy (Type 0)
    let legacy_tx = TxLegacy {
        chain_id: Some(42161),
        nonce: 5,
        gas_price: 100_000_000,
        gas_limit: 150_000,
        to: TxKind::Call(to_pool),
        value: U256::from(100_000_000_000u64),
        input: calldata.clone(),
    };
    let legacy_env = TxEnvelope::Legacy(Signed::new_unchecked(legacy_tx, sig, B256::ZERO));
    let mut raw_legacy = Vec::new();
    legacy_env.encode_2718(&mut raw_legacy);

    let decoded_legacy = FastTxDecoder::decode(&raw_legacy).expect("Decode legacy tx");
    assert_eq!(decoded_legacy.tx_type, ArbitrumTxType::Legacy);
    assert_eq!(decoded_legacy.to, Some(to_pool));
    assert_eq!(decoded_legacy.gas_limit, 150_000);
    assert_eq!(decoded_legacy.value, U256::from(100_000_000_000u64));
    assert_eq!(decoded_legacy.calldata, calldata);
    assert!(DexCalldataInspector::is_dex_swap(&decoded_legacy.calldata));

    // 2. EIP-2930 (Type 1)
    let eip2930_tx = TxEip2930 {
        chain_id: 42161,
        nonce: 6,
        gas_price: 200_000_000,
        gas_limit: 220_000,
        to: TxKind::Call(to_pool),
        value: U256::from(500_000_000_000u64),
        access_list: Default::default(),
        input: calldata.clone(),
    };
    let eip2930_env = TxEnvelope::Eip2930(Signed::new_unchecked(eip2930_tx, sig, B256::ZERO));
    let mut raw_eip2930 = Vec::new();
    eip2930_env.encode_2718(&mut raw_eip2930);

    let decoded_eip2930 = FastTxDecoder::decode(&raw_eip2930).expect("Decode EIP-2930 tx");
    assert_eq!(decoded_eip2930.tx_type, ArbitrumTxType::Eip2930);
    assert_eq!(decoded_eip2930.to, Some(to_pool));
    assert_eq!(decoded_eip2930.gas_limit, 220_000);
    assert_eq!(decoded_eip2930.value, U256::from(500_000_000_000u64));
    assert_eq!(decoded_eip2930.calldata, calldata);

    // 3. EIP-1559 (Type 2)
    let eip1559_tx = TxEip1559 {
        chain_id: 42161,
        nonce: 7,
        gas_limit: 450_000,
        max_fee_per_gas: 3_000_000_000,
        max_priority_fee_per_gas: 250_000_000,
        to: TxKind::Call(to_pool),
        value: U256::from(2_000_000_000_000_000_000u128), // 2 ETH
        access_list: Default::default(),
        input: calldata.clone(),
    };
    let eip1559_env = TxEnvelope::Eip1559(Signed::new_unchecked(eip1559_tx, sig, B256::ZERO));
    let mut raw_eip1559 = Vec::new();
    eip1559_env.encode_2718(&mut raw_eip1559);

    let decoded_eip1559 = FastTxDecoder::decode(&raw_eip1559).expect("Decode EIP-1559 tx");
    assert_eq!(decoded_eip1559.tx_type, ArbitrumTxType::Eip1559);
    assert_eq!(decoded_eip1559.to, Some(to_pool));
    assert_eq!(decoded_eip1559.gas_limit, 450_000);
    assert_ne!(decoded_eip1559.gas_limit, 21_000, "Must NOT be hardcoded to 21,000");
    assert_eq!(decoded_eip1559.max_fee_per_gas, 3_000_000_000);
    assert_eq!(decoded_eip1559.max_priority_fee_per_gas, 250_000_000);
    assert_eq!(decoded_eip1559.value, U256::from(2_000_000_000_000_000_000u128));
    assert_eq!(decoded_eip1559.calldata, calldata);
}

#[test]
fn test_c5_batch_unpacking_recursion_and_boundary_checks() {
    let dummy_tx = MockTx {
        tx_type: ArbitrumTxType::Eip1559,
        pool_target: [0x55; 20],
        calldata_len: 120,
    };

    // 1. Boundary: exactly depth 16
    let mut batch_16 = BatchPayload::Direct(dummy_tx.clone());
    for _ in 0..16 {
        batch_16 = BatchPayload::NestedBatch(vec![batch_16]);
    }
    let res_16 = unpack_arbitrum_batch(&batch_16);
    assert_eq!(res_16.len(), 1, "Depth 16 must unpack the transaction");

    // 2. Boundary: depth 17 (> 16 limit)
    let mut batch_17 = BatchPayload::Direct(dummy_tx.clone());
    for _ in 0..17 {
        batch_17 = BatchPayload::NestedBatch(vec![batch_17]);
    }
    let res_17 = unpack_arbitrum_batch(&batch_17);
    assert!(
        res_17.is_empty(),
        "Depth 17 must be safely pruned without stack overflow"
    );

    // 3. Extreme depth: 100 levels
    let mut batch_100 = BatchPayload::Direct(dummy_tx.clone());
    for _ in 0..100 {
        batch_100 = BatchPayload::NestedBatch(vec![batch_100]);
    }
    let res_100 = unpack_arbitrum_batch(&batch_100);
    assert!(
        res_100.is_empty(),
        "Depth 100 must be safely pruned without stack overflow"
    );

    // 4. Empty batch
    let empty_batch = BatchPayload::NestedBatch(vec![]);
    let res_empty = unpack_arbitrum_batch(&empty_batch);
    assert!(res_empty.is_empty());
}

#[test]
fn test_c5_unpack_l2_message_edge_cases() {
    // 1. Empty message
    let res_empty = unpack_l2_message(&[]);
    assert!(res_empty.is_ok());
    assert!(res_empty.unwrap().is_empty());

    // 2. Kind 0x06 with truncated length (< 8 bytes after prefix)
    let truncated_len = [0x06, 0x00, 0x01];
    let res_trunc = unpack_l2_message(&truncated_len);
    assert!(res_trunc.is_ok());
    assert!(res_trunc.unwrap().is_empty());

    // 3. Kind 0x06 with length exceeding buffer
    let mut overlength = vec![0x06];
    overlength.extend_from_slice(&1000u64.to_be_bytes()); // says 1000 bytes
    overlength.extend_from_slice(&[0x01, 0x02, 0x03]); // only 3 bytes
    let res_over = unpack_l2_message(&overlength);
    assert!(
        matches!(res_over, Err(ArbitrumFeedError::InvalidEncoding(_))),
        "Sub-message length exceeding buffer bounds must return InvalidEncoding"
    );

    // 4. Kind 0x03 (signed tx) with empty body
    let empty_signed = [0x03];
    let res_empty_signed = unpack_l2_message(&empty_signed);
    assert!(res_empty_signed.is_ok());
    assert!(res_empty_signed.unwrap().is_empty());

    // 5. Corrupted RLP bytes inside direct transaction
    let garbage_tx = [0xff, 0xff, 0x12, 0x34];
    let res_garbage = unpack_l2_message(&garbage_tx);
    assert!(
        matches!(res_garbage, Err(ArbitrumFeedError::DecodeError(_))),
        "Strict decoding rejects corrupted raw bytes with DecodeError"
    );
}

#[test]
fn test_c5_full_sequencer_frame_with_nested_batches() {
    let to_pool = Address::repeat_byte(0x99);
    let swap_calldata = Bytes::from(vec![0x02, 0x2c, 0x0d, 0x9f, 0xfe, 0xed]);
    let sig = make_dummy_sig();

    // Create a real EIP-1559 tx
    let eip1559_tx = TxEip1559 {
        chain_id: 42161,
        nonce: 10,
        gas_limit: 300_000,
        max_fee_per_gas: 2_000_000_000,
        max_priority_fee_per_gas: 100_000_000,
        to: TxKind::Call(to_pool),
        value: U256::ZERO,
        access_list: Default::default(),
        input: swap_calldata.clone(),
    };
    let envelope = TxEnvelope::Eip1559(Signed::new_unchecked(eip1559_tx, sig, B256::ZERO));
    let mut raw_tx = Vec::new();
    envelope.encode_2718(&mut raw_tx);

    // Wrap in Kind 0x03
    let mut signed_msg = vec![0x03];
    signed_msg.extend_from_slice(&raw_tx);

    // Wrap in Inner Kind 0x06
    let mut inner_batch = vec![0x06];
    inner_batch.extend_from_slice(&(signed_msg.len() as u64).to_be_bytes());
    inner_batch.extend_from_slice(&signed_msg);

    // Wrap in Outer Kind 0x06
    let mut outer_batch = vec![0x06];
    outer_batch.extend_from_slice(&(inner_batch.len() as u64).to_be_bytes());
    outer_batch.extend_from_slice(&inner_batch);

    // Base64 encode
    let l2msg_b64 = base64::engine::general_purpose::STANDARD.encode(&outer_batch);

    let json_frame = format!(
        r#"{{"version":1,"messages":[{{"sequenceNumber":8888,"message":{{"message":{{"header":null,"l2msg":"{}"}}}}}}]}}"#,
        l2msg_b64
    );

    let handler = ArbitrumFeedHandler::new("wss://arb1-feed.arbitrum.io/feed");
    let publisher = MarketFeedPublisher::new(16);
    let mut rx = publisher.subscribe();

    let processed = handler.process_feed_message(&json_frame, &publisher).expect("Process frame");
    assert_eq!(processed, 1, "Should successfully unpack nested batch");

    let update = rx.try_recv().expect("Should receive update");
    match update {
        MarketUpdate::PendingTxMutation {
            chain,
            target_pool,
            selector,
            gas_limit,
            max_priority_fee_per_gas,
            sequence_number,
            ..
        } => {
            assert_eq!(chain, Chain::ArbitrumOne);
            assert_eq!(target_pool, to_pool);
            assert_eq!(selector, [0x02, 0x2c, 0x0d, 0x9f]);
            assert_eq!(gas_limit, 300_000);
            assert_eq!(max_priority_fee_per_gas, 100_000_000);
            assert_eq!(sequence_number, 8888);
        }
        other => panic!("Unexpected update: {:?}", other),
    }
}

// ============================================================================
// CHALLENGE C2: Concurrency, Sequence Deduplication, Gas Budgeting, WebSocket
// ============================================================================

#[test]
fn test_c2_dedup_concurrency_heavy_contention() {
    let dedup = Arc::new(SequencerDeduplicator::new(1000));
    let win_counter = Arc::new(AtomicUsize::new(0));
    let num_threads = 32;
    let mut handles = Vec::new();

    for _thread_idx in 0..num_threads {
        let d = Arc::clone(&dedup);
        let counter = Arc::clone(&win_counter);
        handles.push(thread::spawn(move || {
            for round in 1..=100 {
                let seq = 1000 + (round * 5);
                if d.check_and_update(seq) {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
                // Try backward sequence
                assert!(!d.check_and_update(seq - 1));
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Monotonicity verified
    assert_eq!(dedup.current_sequence(), 1500);
    // Sequence 1500 must now be rejected
    assert!(!dedup.check_and_update(1500));
    // Sequence 1501 must be accepted
    assert!(dedup.check_and_update(1501));
    assert_eq!(dedup.current_sequence(), 1501);
}

#[test]
fn test_c2_flashblock_gas_budgeting_edge_cases() {
    let mut tracker = FlashblockGasTracker::new(30_000_000);

    // Verify sub-block 0 has exactly 3,000,000 budget
    assert_eq!(tracker.sub_block_gas_budget(0), 3_000_000);

    // Spend exactly 3,000,000
    assert!(tracker.try_budget_tx(0, 3_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 3_000_000);

    // Over budget by 1
    assert!(tracker.try_budget_tx(0, 1).is_err());
    assert_eq!(tracker.cumulative_gas_spent, 3_000_000);

    // Arithmetic overflow protection with malicious tx_gas
    assert!(tracker.try_budget_tx(0, u64::MAX).is_err());
    assert_eq!(tracker.cumulative_gas_spent, 3_000_000);

    // Wrapping overflow protection: (3_000_000 + (u64::MAX - 1_000_000)) wraps in release if unchecked
    let wrapping_gas = u64::MAX - 1_000_000;
    assert!(tracker.try_budget_tx(9, wrapping_gas).is_err());
    assert_eq!(tracker.cumulative_gas_spent, 3_000_000);

    // Reset block
    tracker.reset_block();
    assert_eq!(tracker.cumulative_gas_spent, 0);
    assert_eq!(tracker.current_sub_block_index, 0);
}

#[test]
fn test_c2_base_flashblock_alchemy_dex_log_ingestion() {
    let handler = BaseFlashblockHandler::new("wss://mainnet.flashblocks.base.org/ws", 30_000_000);
    let publisher = MarketFeedPublisher::new(16);
    let mut rx = publisher.subscribe();

    // 1. Process Flashblock message
    let fb = FlashblockMessage {
        block_number: 20000000,
        sub_block_index: 0,
        base_fee: U256::from(100000000u64),
        gas_used: 1_200_000,
        gas_limit: 30_000_000,
        transactions: vec![],
    };
    let fb_json = serde_json::to_string(&fb).unwrap();
    let handled = handler.process_message_text(&fb_json, &publisher).expect("Process flashblock");
    assert!(handled);

    let step = rx.try_recv().expect("Should receive FlashblockStep");
    match step {
        MarketUpdate::FlashblockStep {
            block_number,
            sub_block_index,
            cumulative_gas_spent,
            gas_budget,
        } => {
            assert_eq!(block_number, 20000000);
            assert_eq!(sub_block_index, 0);
            assert_eq!(cumulative_gas_spent, 1_200_000);
            assert_eq!(gas_budget, 3_000_000);
        }
        other => panic!("Unexpected update: {:?}", other),
    }

    // 2. Process Alchemy Uniswap V2 Sync Log
    let pool_addr = Address::repeat_byte(0x33);
    let r0 = U256::from(10_000_000_000_000_000_000u128); // 10 ETH
    let r1 = U256::from(30_000_000_000u128); // 30,000 USDC
    let mut data = [0u8; 64];
    data[0..32].copy_from_slice(&r0.to_be_bytes::<32>());
    data[32..64].copy_from_slice(&r1.to_be_bytes::<32>());
    let data_hex = format!("0x{}", alloy_primitives::hex::encode(data));

    let sync_json = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0x1","result":{{"address":"0x{:x}","topics":["0x{:x}"],"data":"{}"}}}}}}"#,
        pool_addr, TOPIC_UNIV2_SYNC, data_hex
    );

    let log_handled = handler.process_message_text(&sync_json, &publisher).expect("Process log");
    assert!(log_handled);

    let sync_update = rx.try_recv().expect("Should receive ReserveUpdate");
    match sync_update {
        MarketUpdate::ReserveUpdate {
            chain,
            pool_id,
            reserve0,
            reserve1,
            ..
        } => {
            assert_eq!(chain, Chain::Base);
            assert_eq!(pool_id, pool_addr);
            assert_eq!(reserve0, 10_000_000_000_000_000_000);
            assert_eq!(reserve1, 30_000_000_000);
        }
        other => panic!("Unexpected update: {:?}", other),
    }

    // 3. Process Alchemy Uniswap V3 Swap Log
    let v3_pool_addr = Address::repeat_byte(0x44);
    let sqrt_price_x96 = U256::from(1900000000000000u64);
    let liquidity = 75_000_000_000u128;
    let tick = 25000i32;

    let mut v3_data = [0u8; 160];
    v3_data[64..96].copy_from_slice(&sqrt_price_x96.to_be_bytes::<32>());
    v3_data[96..128].copy_from_slice(&U256::from(liquidity).to_be_bytes::<32>());
    v3_data[156..160].copy_from_slice(&tick.to_be_bytes());
    let v3_data_hex = format!("0x{}", alloy_primitives::hex::encode(v3_data));

    let swap_json = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0x2","result":{{"address":"0x{:x}","topics":["0x{:x}"],"data":"{}"}}}}}}"#,
        v3_pool_addr, TOPIC_UNIV3_SWAP, v3_data_hex
    );

    let swap_handled = handler.process_message_text(&swap_json, &publisher).expect("Process v3 swap");
    assert!(swap_handled);

    let v3_update = rx.try_recv().expect("Should receive TickUpdate");
    match v3_update {
        MarketUpdate::TickUpdate {
            chain,
            pool_id,
            sqrt_price_x96: sp,
            current_tick,
            liquidity: liq,
            ..
        } => {
            assert_eq!(chain, Chain::Base);
            assert_eq!(pool_id, v3_pool_addr);
            assert_eq!(sp, sqrt_price_x96);
            assert_eq!(current_tick, 25000);
            assert_eq!(liq, 75_000_000_000);
        }
        other => panic!("Unexpected update: {:?}", other),
    }
}
