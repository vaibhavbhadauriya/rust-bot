//! Pure Yul 100-Byte Calldata Oracle and Revert Simulator
//! Authoritative source: ORIGINAL_REQUEST.md lines 53-68, 87-88, and explorer_survey_3/handoff.md.

use alloy_primitives::{Address, U256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionParams {
    pub weth_amount_to_first_market: u64, // 0x00..0x08
    pub balance_before: u64,              // 0x08..0x10
    pub gas_cost: u64,                    // 0x10..0x18
    pub target1: Address,                 // 0x18..0x2c (20 bytes)
    pub target2: Address,                 // 0x2c..0x40 (20 bytes)
    pub token0_amount_out: u128,          // 0x40..0x50 (16 bytes)
    pub token0_amount_out_index: u16,     // 0x50..0x52 (2 bytes)
    pub token1_amount_out: u128,          // 0x52..0x62 (16 bytes)
    pub token1_amount_out_index: u16,     // 0x62..0x64 (2 bytes)
}

/// Exactly 100 bytes (0x64) tightly packed calldata encoder.
pub fn encode_oracle_calldata(params: &ExecutionParams) -> [u8; 100] {
    let mut data = [0u8; 100];

    // 0x00..0x08: wethAmountToFirstMarket (8B)
    data[0..8].copy_from_slice(&params.weth_amount_to_first_market.to_be_bytes());

    // 0x08..0x10: balanceBefore (8B)
    data[8..16].copy_from_slice(&params.balance_before.to_be_bytes());

    // 0x10..0x18: gasCost (8B)
    data[16..24].copy_from_slice(&params.gas_cost.to_be_bytes());

    // 0x18..0x2c: target1 (20B)
    data[24..44].copy_from_slice(params.target1.as_slice());

    // 0x2c..0x40: target2 (20B)
    data[44..64].copy_from_slice(params.target2.as_slice());

    // 0x40..0x50: token0_amountOut (16B)
    data[64..80].copy_from_slice(&params.token0_amount_out.to_be_bytes());

    // 0x50..0x52: token0_amountOutIndex (2B)
    data[80..82].copy_from_slice(&params.token0_amount_out_index.to_be_bytes());

    // 0x52..0x62: token1_amountOut (16B)
    data[82..98].copy_from_slice(&params.token1_amount_out.to_be_bytes());

    // 0x62..0x64: token1_amountOutIndex (2B)
    data[98..100].copy_from_slice(&params.token1_amount_out_index.to_be_bytes());

    data
}

/// Emulates Yul `shr` + `calldataload` instructions on 100-byte calldata.
pub fn decode_oracle_calldata(calldata: &[u8]) -> Result<ExecutionParams, &'static str> {
    if calldata.len() < 100 {
        return Err("Calldata too short (expected at least 100 bytes)");
    }

    // Helper: read 32-byte window starting at offset, shifted right by shift_bits
    let shr_calldataload = |offset: usize, shift_bits: usize| -> U256 {
        let mut window = [0u8; 32];
        let available = calldata.len().saturating_sub(offset).min(32);
        window[..available].copy_from_slice(&calldata[offset..offset + available]);
        let raw = U256::from_be_bytes(window);
        raw >> shift_bits
    };

    // 0x00: shr(0xc0, calldataload(0x00)) -> 192 bit shift leaves 64 bits
    let weth_amount = shr_calldataload(0x00, 0xc0).to::<u64>();

    // 0x08: shr(0xc0, calldataload(0x08))
    let balance_before = shr_calldataload(0x08, 0xc0).to::<u64>();

    // 0x10: shr(0xc0, calldataload(0x10))
    let gas_cost = shr_calldataload(0x10, 0xc0).to::<u64>();

    // 0x18: shr(0x60, calldataload(0x18)) -> 96 bit shift leaves 160 bits (20 bytes)
    let target1_raw = shr_calldataload(0x18, 0x60);
    let mut target1_bytes = [0u8; 20];
    let t1_full = target1_raw.to_be_bytes::<32>();
    target1_bytes.copy_from_slice(&t1_full[12..32]);
    let target1 = Address::from(target1_bytes);

    // 0x2c: shr(0x60, calldataload(0x2c))
    let target2_raw = shr_calldataload(0x2c, 0x60);
    let mut target2_bytes = [0u8; 20];
    let t2_full = target2_raw.to_be_bytes::<32>();
    target2_bytes.copy_from_slice(&t2_full[12..32]);
    let target2 = Address::from(target2_bytes);

    // 0x40: shr(0x80, calldataload(0x40)) -> 128 bit shift leaves 128 bits
    let token0_out = shr_calldataload(0x40, 0x80).to::<u128>();

    // 0x50: shr(0xf0, calldataload(0x50)) -> 240 bit shift leaves 16 bits
    let token0_idx = shr_calldataload(0x50, 0xf0).to::<u16>();

    // 0x52: shr(0x80, calldataload(0x52))
    let token1_out = shr_calldataload(0x52, 0x80).to::<u128>();

    // 0x62: shr(0xf0, calldataload(0x62))
    let token1_idx = shr_calldataload(0x62, 0xf0).to::<u16>();

    Ok(ExecutionParams {
        weth_amount_to_first_market: weth_amount,
        balance_before,
        gas_cost,
        target1,
        target2,
        token0_amount_out: token0_out,
        token0_amount_out_index: token0_idx,
        token1_amount_out: token1_out,
        token1_amount_out_index: token1_idx,
    })
}

/// Feature 11: 30-Byte Fast-Revert Guard Simulator
#[derive(Debug, PartialEq)]
pub enum GuardOutcome {
    Passed,
    FastRevert { gas_used: u64, gas_saved_percent: f64 },
}

pub fn simulate_fast_revert_guard(
    expected_reserve0: u128,
    expected_reserve1: u128,
    actual_reserve0: u128,
    actual_reserve1: u128,
) -> GuardOutcome {
    if expected_reserve0 == actual_reserve0 && expected_reserve1 == actual_reserve1 {
        GuardOutcome::Passed
    } else {
        // Fast-revert consumes staticcall + revert cost: ~2,300 gas
        // Full failed transaction consumes ~125,000 gas
        let fast_gas = 2_300u64;
        let full_gas = 125_000u64;
        let saved_pct = ((full_gas - fast_gas) as f64 / full_gas as f64) * 100.0;
        GuardOutcome::FastRevert {
            gas_used: fast_gas,
            gas_saved_percent: saved_pct,
        }
    }
}

/// Feature 11: Atomic Profit Enforcement Check
pub fn check_atomic_profit(weth_profit: u64, gas_cost: u64) -> Result<(), &'static str> {
    if weth_profit < gas_cost {
        Err("Atomic profit check failed: wethProfit < _gasCost (revert(0x0, 0x20))")
    } else {
        Ok(())
    }
}

/// Feature 12: CREATE2 Vanity Address Calculator
pub fn compute_create2_address(deployer: Address, salt: [u8; 32], init_code_hash: [u8; 32]) -> Address {
    flash_executor::vanity::compute_create2_address(deployer, salt, init_code_hash)
}

/// Counts leading zero bytes in an address.
pub fn count_leading_zero_bytes(addr: &Address) -> usize {
    flash_executor::vanity::count_leading_zero_bytes(addr)
}
