//! Adversarial Ingestion & Malformed Stream Test for FastTxDecoder (C5 & V-21)
//!
//! Validates:
//! 1. Byte streams specifically embedding 0x94 at random offsets are NEVER misinterpreted as valid txs or addresses.
//! 2. Corrupted, truncated, non-RLP, and byte-shifted streams strictly return Err(ArbitrumFeedError::DecodeError(_)).
//! 3. Zero panics occur under hostile and corrupted input streams.

use alloy_consensus::{Signed, TxEip1559, TxEnvelope};
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, Signature, TxKind, B256, U256};
use flash_collector::{ArbitrumFeedError, FastTxDecoder};

fn make_valid_eip1559_payload() -> Vec<u8> {
    let to_addr = Address::repeat_byte(0x55);
    let sig = Signature::from_scalars_and_parity(B256::repeat_byte(0x11), B256::repeat_byte(0x22), false);
    let tx = TxEip1559 {
        chain_id: 42161,
        nonce: 10,
        max_priority_fee_per_gas: 1_000_000_000,
        max_fee_per_gas: 2_000_000_000,
        gas_limit: 300_000,
        to: TxKind::Call(to_addr),
        value: U256::from(50_000),
        access_list: Default::default(),
        input: Bytes::from(vec![0x02, 0x2c, 0x0d, 0x9f, 0xAA, 0xBB]),
    };
    let env = TxEnvelope::Eip1559(Signed::new_unchecked(tx, sig, B256::ZERO));
    let mut raw = Vec::new();
    env.encode_2718(&mut raw);
    raw
}

#[test]
fn test_c5_v21_embedded_0x94_fallback_elimination() {
    // Legacy bug: When byte 0x94 occurred in input, decoder scanned for 0x94 + 20 bytes and returned a dummy Tx!
    // We now verify that embedding 0x94 anywhere in non-RLP streams strictly produces DecodeError.
    let base_bytes = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];

    for offset in 0..50 {
        let mut stream = Vec::new();
        stream.resize(offset, 0xAA);
        stream.push(0x94); // Inject 0x94
        // Inject 20 bytes representing what could look like an address
        stream.extend_from_slice(&[0x42; 20]);
        stream.extend_from_slice(&base_bytes);

        let res = FastTxDecoder::decode(&stream);
        assert!(
            matches!(res, Err(ArbitrumFeedError::DecodeError(_))),
            "Offset {}: Expected Err(ArbitrumFeedError::DecodeError), got {:?}",
            offset,
            res
        );
    }
}

#[test]
fn test_c5_v21_corrupted_and_truncated_streams() {
    let valid_raw = make_valid_eip1559_payload();
    assert!(FastTxDecoder::decode(&valid_raw).is_ok(), "Baseline valid tx must decode");

    // 1. Truncate from 0 up to len - 1
    for len in 0..valid_raw.len() {
        let truncated = &valid_raw[..len];
        let res = FastTxDecoder::decode(truncated);
        if len == 0 {
            assert!(
                matches!(res, Err(ArbitrumFeedError::InvalidEncoding(_))),
                "Empty payload: Expected InvalidEncoding, got {:?}",
                res
            );
        } else {
            assert!(
                matches!(res, Err(ArbitrumFeedError::DecodeError(_))),
                "Truncated len {}: Expected DecodeError, got {:?}",
                len,
                res
            );
        }
    }

    // 2. Shift stream by prepending garbage bytes
    for shift in 1..10 {
        let mut shifted = vec![0xEE; shift];
        shifted.extend_from_slice(&valid_raw);
        let res = FastTxDecoder::decode(&shifted);
        assert!(
            matches!(res, Err(ArbitrumFeedError::DecodeError(_))),
            "Shifted by {}: Expected DecodeError, got {:?}",
            shift,
            res
        );
    }

    // 3. Bitflip / corrupt intermediate bytes of valid envelope
    for i in 0..valid_raw.len() {
        let mut corrupted = valid_raw.clone();
        corrupted[i] ^= 0xFF; // Invert all bits in byte i
        let res = FastTxDecoder::decode(&corrupted);
        // Either decode error, or if still valid RLP (unlikely for arbitrary bitflip), must not panic
        if let Err(e) = res {
            assert!(matches!(e, ArbitrumFeedError::DecodeError(_)));
        }
    }
}

#[test]
fn test_c5_v21_fuzz_random_byte_streams() {
    let mut rng_state: u64 = 0x1337BEEF8BADF00D;
    for len in [1, 2, 4, 19, 20, 21, 32, 64, 128, 256, 512] {
        for _ in 0..100 {
            let mut payload = vec![0u8; len];
            for byte in payload.iter_mut() {
                rng_state = rng_state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                *byte = (rng_state >> 24) as u8;
            }
            // Embed 0x94 at arbitrary index
            let idx = (rng_state as usize) % len;
            payload[idx] = 0x94;

            let res = FastTxDecoder::decode(&payload);
            assert!(
                matches!(res, Err(ArbitrumFeedError::DecodeError(_))),
                "Random payload of len {} with 0x94 must return DecodeError, got {:?}",
                len,
                res
            );
        }
    }
}
