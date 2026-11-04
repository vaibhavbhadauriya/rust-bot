//! Comprehensive Integration and Verification Tests for flash-collector

use alloy_consensus::{Signed, TxEip1559, TxEip7702, TxEnvelope, TxLegacy};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, Signature, TxKind, B256, U256};
use base64::Engine;
use flash_collector::{
    unpack_arbitrum_batch, ArbitrumFeedHandler, ArbitrumTxType, BaseFlashblockHandler,
    BatchPayload, DexCalldataInspector, FastTxDecoder, FlashblockGasTracker, MarketFeedPublisher,
    MarketUpdate, MockTx, SequencerDeduplicator, TOPIC_UNIV2_SYNC, TOPIC_UNIV3_SWAP,
};
use flash_core::types::Chain;
use std::sync::Arc;
use std::thread;

#[test]
fn test_monotonic_deduplicator_concurrency() {
    let dedup = Arc::new(SequencerDeduplicator::new(1000));
    assert_eq!(dedup.current_sequence(), 1000);

    // Stale rejected
    assert!(!dedup.check_and_update(999));
    assert!(!dedup.check_and_update(1000));

    // Monotonic advance
    assert!(dedup.check_and_update(1001));
    assert_eq!(dedup.current_sequence(), 1001);

    // 10 concurrent threads racing on sequence numbers
    let mut handles = vec![];
    for i in 1..=10 {
        let d = Arc::clone(&dedup);
        handles.push(thread::spawn(move || {
            d.check_and_update(1000 + i * 10)
        }));
    }

    for h in handles {
        let _ = h.join();
    }

    assert!(dedup.current_sequence() >= 1010);
}

#[test]
fn test_recursive_nested_batch_deep_nesting() {
    let tx1 = MockTx {
        tx_type: ArbitrumTxType::Eip1559,
        pool_target: [0x11; 20],
        calldata_len: 128,
    };
    let tx2 = MockTx {
        tx_type: ArbitrumTxType::Legacy,
        pool_target: [0x22; 20],
        calldata_len: 64,
    };
    let tx3 = MockTx {
        tx_type: ArbitrumTxType::Eip7702,
        pool_target: [0x33; 20],
        calldata_len: 256,
    };

    // 4-level nested batch
    let batch4 = BatchPayload::Direct(tx3.clone());
    let batch3 = BatchPayload::NestedBatch(vec![batch4, BatchPayload::Direct(tx2.clone())]);
    let batch2 = BatchPayload::NestedBatch(vec![batch3]);
    let batch1 = BatchPayload::NestedBatch(vec![BatchPayload::Direct(tx1.clone()), batch2]);

    let flat = unpack_arbitrum_batch(&batch1);
    assert_eq!(flat.len(), 3);
    assert_eq!(flat[0], tx1);
    assert_eq!(flat[1], tx3);
    assert_eq!(flat[2], tx2);
}

#[test]
fn test_empty_batch_unpacking() {
    let empty = BatchPayload::NestedBatch(vec![]);
    let flat = unpack_arbitrum_batch(&empty);
    assert!(flat.is_empty());
}

#[test]
fn test_dex_calldata_inspector() {
    let univ2_swap_calldata = [0x02, 0x2c, 0x0d, 0x9f, 0x00, 0x00];
    assert!(DexCalldataInspector::is_dex_swap(&univ2_swap_calldata));
    assert_eq!(
        DexCalldataInspector::extract_selector(&univ2_swap_calldata),
        Some([0x02, 0x2c, 0x0d, 0x9f])
    );

    let univ3_swap_calldata = [0x12, 0x8a, 0xcb, 0x08, 0x11, 0x22];
    assert!(DexCalldataInspector::is_dex_swap(&univ3_swap_calldata));

    let router_swap = [0x38, 0xed, 0x17, 0x39, 0xaa];
    assert!(DexCalldataInspector::is_dex_swap(&router_swap));

    let non_dex = [0x11, 0x22, 0x33, 0x44];
    assert!(!DexCalldataInspector::is_dex_swap(&non_dex));
}

#[test]
fn test_fast_tx_decoder() {
    let sig = Signature::from_scalars_and_parity(B256::repeat_byte(1), B256::repeat_byte(2), false);

    // Legacy tx prefix 0xc0
    let legacy_tx = TxLegacy {
        chain_id: Some(42161),
        nonce: 1,
        gas_price: 100_000_000,
        gas_limit: 100_000,
        to: TxKind::Call(Address::repeat_byte(0x11)),
        value: U256::ZERO,
        input: Bytes::from_static(&[0x01, 0x02]),
    };
    let legacy_env = TxEnvelope::Legacy(Signed::new_unchecked(legacy_tx, sig, B256::ZERO));
    let mut raw_legacy = Vec::new();
    legacy_env.encode_2718(&mut raw_legacy);
    let decoded = FastTxDecoder::decode(&raw_legacy).unwrap();
    assert_eq!(decoded.tx_type, ArbitrumTxType::Legacy);

    // EIP-1559 type 2
    let raw_1559 = create_sample_eip1559_swap_tx(
        Address::repeat_byte(0x22),
        250_000,
        2_000_000_000,
        100_000_000,
        Bytes::from_static(&[0x02, 0x2c, 0x0d, 0x9f]),
    );
    let decoded_1559 = FastTxDecoder::decode(&raw_1559).unwrap();
    assert_eq!(decoded_1559.tx_type, ArbitrumTxType::Eip1559);

    // EIP-7702 type 4
    let eip7702_tx = TxEip7702 {
        chain_id: 42161,
        nonce: 1,
        gas_limit: 300_000,
        max_fee_per_gas: 2_000_000_000,
        max_priority_fee_per_gas: 100_000_000,
        to: Address::repeat_byte(0x33),
        value: U256::ZERO,
        access_list: Default::default(),
        authorization_list: vec![],
        input: Bytes::from_static(&[0x77, 0x02]),
    };
    let eip7702_env = TxEnvelope::Eip7702(Signed::new_unchecked(eip7702_tx, sig, B256::ZERO));
    let mut raw_7702 = Vec::new();
    eip7702_env.encode_2718(&mut raw_7702);
    let decoded_7702 = FastTxDecoder::decode(&raw_7702).unwrap();
    assert_eq!(decoded_7702.tx_type, ArbitrumTxType::Eip7702);
}

#[test]
fn test_base_flashblock_gas_budgeting() {
    let limit = 30_000_000u64;
    let mut tracker = FlashblockGasTracker::new(limit);

    // Sub-block 0 (1/10th limit = 3,000,000)
    assert_eq!(tracker.sub_block_gas_budget(0), 3_000_000);
    // Sub-block 4 (5/10th limit = 15,000,000)
    assert_eq!(tracker.sub_block_gas_budget(4), 15_000_000);
    // Sub-block 9 (10/10th limit = 30,000,000)
    assert_eq!(tracker.sub_block_gas_budget(9), 30_000_000);
    // Clamping beyond 9
    assert_eq!(tracker.sub_block_gas_budget(15), 30_000_000);

    // Budgeting in sub-block 0
    assert!(tracker.try_budget_tx(0, 1_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 1_000_000);

    assert!(tracker.try_budget_tx(0, 1_500_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 2_500_000);

    // Exceeds 3,000,000 in sub-block 0
    assert!(tracker.try_budget_tx(0, 600_000).is_err());

    // Advance to sub-block 1 (budget 6,000,000)
    assert!(tracker.try_budget_tx(1, 1_000_000).is_ok());
    assert_eq!(tracker.cumulative_gas_spent, 3_500_000);

    // Block boundary reset
    tracker.reset_block();
    assert_eq!(tracker.cumulative_gas_spent, 0);
    assert_eq!(tracker.current_sub_block_index, 0);
}

#[test]
fn test_market_feed_broadcast() {
    let publisher = MarketFeedPublisher::new(32);
    let mut rx = publisher.subscribe();

    let tx_mutation = MarketUpdate::PendingTxMutation {
        chain: Chain::ArbitrumOne,
        tx_hash: B256::repeat_byte(0xaa),
        target_pool: Address::repeat_byte(0xbb),
        selector: [0x02, 0x2c, 0x0d, 0x9f],
        calldata: Bytes::from(vec![0x02, 0x2c, 0x0d, 0x9f]),
        gas_limit: 250_000,
        max_priority_fee_per_gas: 1_000_000_000,
        sequence_number: 12345,
        timestamp_ms: 100_000,
    };

    assert!(publisher.publish(tx_mutation.clone()).is_ok());
    let received = rx.try_recv().unwrap();
    assert_eq!(received, tx_mutation);
}

fn create_sample_eip1559_swap_tx(
    to: Address,
    gas_limit: u64,
    max_fee: u128,
    priority_fee: u128,
    calldata: Bytes,
) -> Vec<u8> {
    let tx = TxEip1559 {
        chain_id: 42161,
        nonce: 1,
        gas_limit,
        max_fee_per_gas: max_fee,
        max_priority_fee_per_gas: priority_fee,
        to: TxKind::Call(to),
        value: U256::from(1_500_000_000_000_000_000u64), // 1.5 ETH
        access_list: Default::default(),
        input: calldata,
    };
    let sig = Signature::from_scalars_and_parity(B256::repeat_byte(1), B256::repeat_byte(2), false);
    let signed = Signed::new_unchecked(tx, sig, B256::ZERO);
    let envelope = TxEnvelope::Eip1559(signed);
    let mut encoded = Vec::new();
    envelope.encode_2718(&mut encoded);
    encoded
}

#[test]
fn test_authentic_eip1559_rlp_decoding() {
    let to_pool = Address::repeat_byte(0x77);
    let swap_calldata = Bytes::from(vec![0x02, 0x2c, 0x0d, 0x9f, 0x01, 0x02, 0x03, 0x04]);
    let raw = create_sample_eip1559_swap_tx(to_pool, 350_000, 2_500_000_000, 150_000_000, swap_calldata.clone());

    let decoded = FastTxDecoder::decode(&raw).expect("Must decode real EIP-1559 envelope");
    assert_eq!(decoded.tx_type, ArbitrumTxType::Eip1559);
    assert_eq!(decoded.to, Some(to_pool));
    assert_eq!(decoded.gas_limit, 350_000);
    assert_eq!(decoded.max_fee_per_gas, 2_500_000_000);
    assert_eq!(decoded.max_priority_fee_per_gas, 150_000_000);
    assert_eq!(decoded.value, U256::from(1_500_000_000_000_000_000u64));
    assert_eq!(decoded.calldata, swap_calldata);
    assert!(DexCalldataInspector::is_dex_swap(&decoded.calldata));
    assert_eq!(DexCalldataInspector::extract_selector(&decoded.calldata), Some([0x02, 0x2c, 0x0d, 0x9f]));
}

#[test]
fn test_arbitrum_nitro_json_base64_batch_unpacking() {
    let to_pool = Address::repeat_byte(0x88);
    let swap_calldata = Bytes::from(vec![0x02, 0x2c, 0x0d, 0x9f, 0xaa, 0xbb]);
    let raw_tx = create_sample_eip1559_swap_tx(to_pool, 210_000, 1_000_000_000, 50_000_000, swap_calldata);

    // Kind 0x03: Signed transaction
    let mut signed_msg = vec![0x03];
    signed_msg.extend_from_slice(&raw_tx);

    // Kind 0x06: L2 Batch containing sub-message
    let mut batch_msg = vec![0x06];
    batch_msg.extend_from_slice(&(signed_msg.len() as u64).to_be_bytes());
    batch_msg.extend_from_slice(&signed_msg);

    // Base64 encode
    let l2msg_b64 = base64::engine::general_purpose::STANDARD.encode(&batch_msg);

    let json_frame = format!(
        r#"{{"version":1,"messages":[{{"sequenceNumber":5001,"message":{{"message":{{"header":null,"l2msg":"{}"}}}}}}]}}"#,
        l2msg_b64
    );

    let handler = ArbitrumFeedHandler::new("wss://arb1-feed.arbitrum.io/feed");
    let publisher = MarketFeedPublisher::new(16);
    let mut rx = publisher.subscribe();

    // 1. First arrival (monotonic sequence 5001) -> processed
    let processed = handler.process_feed_message(&json_frame, &publisher).expect("Process feed message");
    assert_eq!(processed, 1);

    let update = rx.try_recv().expect("Should receive PendingTxMutation");
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
            assert_eq!(gas_limit, 210_000);
            assert_eq!(max_priority_fee_per_gas, 50_000_000);
            assert_eq!(sequence_number, 5001);
        }
        other => panic!("Unexpected update: {:?}", other),
    }

    // 2. Duplicate arrival (sequence 5001 again) -> deduplicated and dropped
    let processed_dup = handler.process_feed_message(&json_frame, &publisher).expect("Process dup");
    assert_eq!(processed_dup, 0);
    assert!(rx.try_recv().is_err());

    // 3. Stale sequence arrival (sequence 5000 < 5001) -> dropped
    let stale_json = json_frame.replace("5001", "5000");
    let processed_stale = handler.process_feed_message(&stale_json, &publisher).expect("Process stale");
    assert_eq!(processed_stale, 0);
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_base_flashblock_processing_and_pending_tx() {
    let to_pool = Address::repeat_byte(0x99);
    let swap_calldata = Bytes::from(vec![0x02, 0x2c, 0x0d, 0x9f, 0x12, 0x34]);
    let raw_tx = create_sample_eip1559_swap_tx(to_pool, 300_000, 2_000_000_000, 100_000_000, swap_calldata);
    let tx_hex = format!("0x{}", alloy_primitives::hex::encode(&raw_tx));

    let fb_json = format!(
        r#"{{"blockNumber":18456000,"index":2,"gasUsed":2500000,"gasLimit":30000000,"transactions":["{}"]}}"#,
        tx_hex
    );

    let handler = BaseFlashblockHandler::new("wss://mainnet.flashblocks.base.org/ws", 30_000_000);
    let publisher = MarketFeedPublisher::new(16);
    let mut rx = publisher.subscribe();

    let handled = handler.process_message_text(&fb_json, &publisher).expect("Process flashblock");
    assert!(handled);

    // Expect FlashblockStep
    let step = rx.try_recv().expect("Should receive FlashblockStep");
    match step {
        MarketUpdate::FlashblockStep {
            block_number,
            sub_block_index,
            cumulative_gas_spent,
            gas_budget,
        } => {
            assert_eq!(block_number, 18456000);
            assert_eq!(sub_block_index, 2);
            assert_eq!(cumulative_gas_spent, 2_500_000);
            assert_eq!(gas_budget, 9_000_000); // index 2: (2+1)/10 * 30M = 9M
        }
        other => panic!("Unexpected update: {:?}", other),
    }

    // Expect PendingTxMutation
    let pending = rx.try_recv().expect("Should receive PendingTxMutation");
    match pending {
        MarketUpdate::PendingTxMutation {
            chain,
            target_pool,
            selector,
            gas_limit,
            max_priority_fee_per_gas,
            ..
        } => {
            assert_eq!(chain, Chain::Base);
            assert_eq!(target_pool, to_pool);
            assert_eq!(selector, [0x02, 0x2c, 0x0d, 0x9f]);
            assert_eq!(gas_limit, 300_000);
            assert_eq!(max_priority_fee_per_gas, 100_000_000);
        }
        other => panic!("Unexpected update: {:?}", other),
    }
}

#[test]
fn test_base_alchemy_logs_sync_and_swap_events() {
    let handler = BaseFlashblockHandler::new("wss://mainnet.flashblocks.base.org/ws", 30_000_000);
    let publisher = MarketFeedPublisher::new(16);
    let mut rx = publisher.subscribe();

    // 1. Uniswap V2 Sync Event
    let sync_pool = Address::repeat_byte(0x55);
    // reserve0 = 50 ETH (50 * 10^18), reserve1 = 150_000 USDC (150_000 * 10^6)
    let r0_u256 = U256::from(50_000_000_000_000_000_000u128);
    let r1_u256 = U256::from(150_000_000_000u128);
    let mut sync_data = [0u8; 64];
    sync_data[0..32].copy_from_slice(&r0_u256.to_be_bytes::<32>());
    sync_data[32..64].copy_from_slice(&r1_u256.to_be_bytes::<32>());
    let sync_data_hex = format!("0x{}", alloy_primitives::hex::encode(sync_data));

    let sync_json = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0x1","result":{{"address":"0x{:x}","topics":["0x{:x}"],"data":"{}"}}}}}}"#,
        sync_pool, TOPIC_UNIV2_SYNC, sync_data_hex
    );

    let processed = handler.process_message_text(&sync_json, &publisher).expect("Process sync");
    assert!(processed);

    let update = rx.try_recv().expect("Should receive ReserveUpdate");
    match update {
        MarketUpdate::ReserveUpdate {
            chain,
            pool_id,
            reserve0,
            reserve1,
            ..
        } => {
            assert_eq!(chain, Chain::Base);
            assert_eq!(pool_id, sync_pool);
            assert_eq!(reserve0, 50_000_000_000_000_000_000);
            assert_eq!(reserve1, 150_000_000_000);
        }
        other => panic!("Unexpected update: {:?}", other),
    }

    // 2. Uniswap V3 Swap Event
    let v3_pool = Address::repeat_byte(0x66);
    let sqrt_price_x96 = U256::from(2500000000000000u64);
    let liquidity = 50_000_000_000u128;
    let tick = -12345i32;

    let mut v3_data = [0u8; 160];
    v3_data[64..96].copy_from_slice(&sqrt_price_x96.to_be_bytes::<32>());
    v3_data[96..128].copy_from_slice(&U256::from(liquidity).to_be_bytes::<32>());
    let sign_byte = if tick < 0 { 0xff } else { 0x00 };
    v3_data[128..156].fill(sign_byte);
    v3_data[156..160].copy_from_slice(&tick.to_be_bytes());
    let v3_data_hex = format!("0x{}", alloy_primitives::hex::encode(v3_data));

    let swap_json = format!(
        r#"{{"jsonrpc":"2.0","method":"eth_subscription","params":{{"subscription":"0x2","result":{{"address":"0x{:x}","topics":["0x{:x}"],"data":"{}"}}}}}}"#,
        v3_pool, TOPIC_UNIV3_SWAP, v3_data_hex
    );

    let processed_v3 = handler.process_message_text(&swap_json, &publisher).expect("Process v3 swap");
    assert!(processed_v3);

    let update_v3 = rx.try_recv().expect("Should receive TickUpdate");
    match update_v3 {
        MarketUpdate::TickUpdate {
            chain,
            pool_id,
            sqrt_price_x96: sp,
            current_tick,
            liquidity: liq,
            ..
        } => {
            assert_eq!(chain, Chain::Base);
            assert_eq!(pool_id, v3_pool);
            assert_eq!(sp, sqrt_price_x96);
            assert_eq!(current_tick, -12345);
            assert_eq!(liq, 50_000_000_000);
        }
        other => panic!("Unexpected update: {:?}", other),
    }
}

#[tokio::test]
async fn test_handlers_graceful_shutdown() {
    let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
    let publisher = MarketFeedPublisher::new(8);

    let arb_handler = ArbitrumFeedHandler::new("ws://127.0.0.1:9");
    let base_handler = BaseFlashblockHandler::new("ws://127.0.0.1:9", 30_000_000);

    let arb_shutdown = shutdown_tx.subscribe();
    let arb_pub = publisher.clone();
    let arb_handle = tokio::spawn(async move {
        let _ = arb_handler.run(arb_pub, None, arb_shutdown).await;
    });

    let base_shutdown = shutdown_tx.subscribe();
    let base_pub = publisher.clone();
    let base_handle = tokio::spawn(async move {
        let _ = base_handler.run(base_pub, None, base_shutdown).await;
    });

    // Send graceful shutdown signal
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    let _ = shutdown_tx.send(());

    let arb_res = tokio::time::timeout(tokio::time::Duration::from_millis(2000), arb_handle).await;
    assert!(arb_res.is_ok(), "Arbitrum handler should shut down within timeout");

    let base_res = tokio::time::timeout(tokio::time::Duration::from_millis(2000), base_handle).await;
    assert!(base_res.is_ok(), "Base handler should shut down within timeout");
}
