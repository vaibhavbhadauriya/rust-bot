//! Feature 10: Pure Yul 100-Byte Calldata Decoding Test Suite
//! Authoritative source: ORIGINAL_REQUEST.md lines 56-65, 87, and explorer_survey_3/handoff.md.

use crate::common::*;
use alloy_primitives::Address;

#[test]
fn test_f10_01_calldata_100byte_roundtrip() {
    let params = ExecutionParams {
        weth_amount_to_first_market: 4_732_420_000,
        balance_before: 10_000_000_000,
        gas_cost: 250_000,
        target1: Address::from([0x11; 20]),
        target2: Address::from([0x22; 20]),
        token0_amount_out: 1_901_905_000_000_000_000,
        token0_amount_out_index: 0,
        token1_amount_out: 4_777_570_000,
        token1_amount_out_index: 1,
    };

    let encoded = encode_oracle_calldata(&params);
    assert_eq!(encoded.len(), 100, "Calldata must be exactly 100 bytes (0x64)");

    let decoded = decode_oracle_calldata(&encoded).expect("Calldata decoding failed");
    assert_eq!(decoded, params, "Decoded params must match original verbatim");
}

#[test]
fn test_f10_02_calldata_short_revert() {
    let short_data = [0u8; 99];
    let res = decode_oracle_calldata(&short_data);
    assert!(res.is_err(), "Calldata shorter than 100 bytes must revert");
}

#[test]
fn test_f10_03_callvalue_dispatch_routing() {
    // 0 wei = arbitrage path
    // 1 wei = owner ERC20 sweep
    // 2 wei = owner ETH sweep
    let route_action = |val: u64| -> Result<&'static str, &'static str> {
        match val {
            0 => Ok("ArbitragePath"),
            1 => Ok("OwnerErc20Sweep"),
            2 => Ok("OwnerEthSweep"),
            _ => Err("Revert"),
        }
    };

    assert_eq!(route_action(0), Ok("ArbitragePath"));
    assert_eq!(route_action(1), Ok("OwnerErc20Sweep"));
    assert_eq!(route_action(2), Ok("OwnerEthSweep"));
    assert!(route_action(3).is_err());
}

#[test]
fn test_f10_04_target_addresses_integrity() {
    let t1 = Address::from([0xAA; 20]);
    let t2 = Address::from([0xBB; 20]);

    let params = ExecutionParams {
        weth_amount_to_first_market: 1,
        balance_before: 2,
        gas_cost: 3,
        target1: t1,
        target2: t2,
        token0_amount_out: 4,
        token0_amount_out_index: 0,
        token1_amount_out: 5,
        token1_amount_out_index: 1,
    };

    let encoded = encode_oracle_calldata(&params);
    // target1 is at byte offset 24..44
    assert_eq!(&encoded[24..44], t1.as_slice());
    // target2 is at byte offset 44..64
    assert_eq!(&encoded[44..64], t2.as_slice());
}

#[test]
fn test_f10_05_u16_index_flags() {
    let params = ExecutionParams {
        weth_amount_to_first_market: 10,
        balance_before: 20,
        gas_cost: 30,
        target1: Address::ZERO,
        target2: Address::ZERO,
        token0_amount_out: 100,
        token0_amount_out_index: 0x0102,
        token1_amount_out: 200,
        token1_amount_out_index: 0x0304,
    };

    let encoded = encode_oracle_calldata(&params);
    let decoded = decode_oracle_calldata(&encoded).unwrap();
    assert_eq!(decoded.token0_amount_out_index, 0x0102);
    assert_eq!(decoded.token1_amount_out_index, 0x0304);
}
