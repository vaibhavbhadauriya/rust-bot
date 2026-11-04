//! Feature 10 Boundaries: Pure Yul 100-Byte Calldata BVA Test Suite

use crate::common::*;
use alloy_primitives::Address;

#[test]
fn test_b10_01_all_zeros_calldata() {
    let zeros = [0u8; 100];
    let decoded = decode_oracle_calldata(&zeros).unwrap();

    assert_eq!(decoded.weth_amount_to_first_market, 0);
    assert_eq!(decoded.balance_before, 0);
    assert_eq!(decoded.gas_cost, 0);
    assert_eq!(decoded.target1, Address::ZERO);
    assert_eq!(decoded.target2, Address::ZERO);
    assert_eq!(decoded.token0_amount_out, 0);
    assert_eq!(decoded.token0_amount_out_index, 0);
    assert_eq!(decoded.token1_amount_out, 0);
    assert_eq!(decoded.token1_amount_out_index, 0);
}

#[test]
fn test_b10_02_all_ones_calldata() {
    let ones = [0xFFu8; 100];
    let decoded = decode_oracle_calldata(&ones).unwrap();

    assert_eq!(decoded.weth_amount_to_first_market, u64::MAX);
    assert_eq!(decoded.balance_before, u64::MAX);
    assert_eq!(decoded.gas_cost, u64::MAX);
    assert_eq!(decoded.token0_amount_out, u128::MAX);
    assert_eq!(decoded.token0_amount_out_index, u16::MAX);
    assert_eq!(decoded.token1_amount_out, u128::MAX);
    assert_eq!(decoded.token1_amount_out_index, u16::MAX);
}

#[test]
fn test_b10_03_boundary_size_99_bytes() {
    let data = [0u8; 99];
    assert!(decode_oracle_calldata(&data).is_err());
}

#[test]
fn test_b10_04_oversized_calldata_101_bytes() {
    // Calldata with trailing padding bytes still decodes first 100 bytes correctly
    let mut data = [0u8; 101];
    data[7] = 42;
    let decoded = decode_oracle_calldata(&data).unwrap();
    assert_eq!(decoded.weth_amount_to_first_market, 42);
}

#[test]
fn test_b10_05_max_u64_gas_cost() {
    let params = ExecutionParams {
        weth_amount_to_first_market: 1,
        balance_before: 2,
        gas_cost: u64::MAX,
        target1: Address::ZERO,
        target2: Address::ZERO,
        token0_amount_out: 0,
        token0_amount_out_index: 0,
        token1_amount_out: 0,
        token1_amount_out_index: 0,
    };
    let encoded = encode_oracle_calldata(&params);
    let decoded = decode_oracle_calldata(&encoded).unwrap();
    assert_eq!(decoded.gas_cost, u64::MAX);
}
