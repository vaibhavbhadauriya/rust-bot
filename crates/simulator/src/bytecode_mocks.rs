//! Embedded mock ERC-20 and Uniswap V2 Pair bytecodes for offline tests.
//! Authoritative source: ORIGINAL_REQUEST.md lines 43-46, explorer_survey_3/handoff.md.

use alloy_primitives::{address, hex, Address, Bytes};
use revm::bytecode::Bytecode;

/// Canonical default addresses for mock simulation environment (non-precompile)
pub const MOCK_EXECUTOR_ADDRESS: Address = address!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
pub const MOCK_CALLER_ADDRESS: Address   = address!("1111111111111111111111111111111111111111");

/// Minimal Mock ERC-20 runtime bytecode for WETH & tokens in offline tests.
/// Responds to balanceOf(0x70a08231), transfer(0xa9059cbb), and approve(0x095ea7b3).
pub const MOCK_ERC20_RUNTIME_BYTECODE: &[u8] = &hex!(
    "6004361061005b575f3560e01c806370a0823114610060578063a9059cbb1461007e57"
    "8063095ea7b31461007e575b5f80fd5b60043560601c5f5260205f205460205f526020"
    "5ff35b600160205260205ff3"
);

/// Minimal Mock Uniswap V2 Pair runtime bytecode.
/// Responds to getReserves(0x0902f1ac) returning 64 bytes (reserve0 from slot 8, reserve1 = 1)
/// and swap(0x022c0d9f) returning success.
pub const MOCK_V2_PAIR_RUNTIME_BYTECODE: &[u8] = &hex!(
    "5f3560e01c80630902f1ac14601a5763022c0d9f146018575b00"
    "5b6008545f52600160205260405ff3"
);

/// Realistic Mock ERC-20 / WETH runtime bytecode with real balance tracking.
/// Responds to:
/// - balanceOf(0x70a08231): returns sload(keccak256(account, 0))
/// - transfer(0xa9059cbb): deducts amount from caller, adds to recipient, returns true
/// - approve(0x095ea7b3): returns true
pub const MOCK_REALISTIC_WETH_RUNTIME_BYTECODE: &[u8] = &hex!(
    "5f3560e01c806370a08231146021578063a9059cbb1460365760015f5260205ff3"
    "5b6004355f525f60205260405f20545f5260205ff3"
    "5b335f525f60205260405f206024358154808211606d57039055"
    "6004355f525f60205260405f20602435815401905560015f5260205ff35b5f5ffd"
);

/// Returns the mock ERC-20 runtime bytecode as a revm `Bytecode`
pub fn mock_erc20_bytecode() -> Bytecode {
    Bytecode::new_raw(Bytes::from_static(MOCK_ERC20_RUNTIME_BYTECODE))
}

/// Returns the mock V2 pair runtime bytecode as a revm `Bytecode`, defaulting to Arbitrum WETH.
pub fn mock_v2_pair_bytecode() -> Bytecode {
    mock_v2_pair_bytecode_for_weth(flash_core::constants::arbitrum::WETH)
}

fn push2_jump(code: &mut Vec<u8>, opcode: u8) -> usize {
    code.push(0x61); // PUSH2
    let pos = code.len();
    code.push(0x00);
    code.push(0x00);
    code.push(opcode); // 0x56 (JUMP) or 0x57 (JUMPI)
    pos
}

fn patch_jump(code: &mut [u8], pos: usize, target: u16) {
    code[pos..pos + 2].copy_from_slice(&target.to_be_bytes());
}

/// Returns mock V2 pair runtime bytecode parameterized with the canonical WETH address.
/// Reads reserves from slot 8 (or unpacks directional packed reserves),
/// reads tokenOut from slot 9 (with WETH fallback), touches cold storage slots to model realistic gas (~80k gas/hop),
/// and executes IERC20(tokenOut).transfer(to, amountOut).
pub fn mock_v2_pair_bytecode_for_weth(weth: Address) -> Bytecode {
    let mut code = Vec::with_capacity(384);

    // --- Dispatcher ---
    // PUSH0 CALLDATALOAD PUSH1 0xe0 SHR
    code.extend_from_slice(&[0x5f, 0x35, 0x60, 0xe0, 0x1c]);
    // DUP1 PUSH4 0x0902f1ac EQ PUSH2 <get_reserves> JUMPI
    code.extend_from_slice(&[0x80, 0x63, 0x09, 0x02, 0xf1, 0xac, 0x14]);
    let jump_get_reserves = push2_jump(&mut code, 0x57);

    // PUSH4 0x022c0d9f EQ PUSH2 <swap> JUMPI
    code.extend_from_slice(&[0x63, 0x02, 0x2c, 0x0d, 0x9f, 0x14]);
    let jump_swap = push2_jump(&mut code, 0x57);

    // Fallback: REVERT (PUSH0 PUSH0 REVERT)
    code.extend_from_slice(&[0x5f, 0x5f, 0xfd]);

    // --- getReserves (0x0902f1ac) ---
    let dest_get_reserves = code.len() as u16;
    patch_jump(&mut code, jump_get_reserves, dest_get_reserves);
    code.push(0x5b); // JUMPDEST

    // Cold storage touches for gas: slots 14, 15, 16
    for slot in [0x0e, 0x0f, 0x10] {
        code.extend_from_slice(&[0x60, slot, 0x54, 0x50]);
    }

    // SLOAD slot 8 -> [packed]
    code.extend_from_slice(&[0x60, 0x08, 0x54]);
    // DUP1 PUSH1 0x70 SHR -> [upper, packed]
    code.extend_from_slice(&[0x80, 0x60, 0x70, 0x1c]);
    // DUP1 ISZERO PUSH2 <unpacked> JUMPI
    code.extend_from_slice(&[0x80, 0x15]);
    let jump_unpacked = push2_jump(&mut code, 0x57);

    // Packed branch:
    // Mask upper: PUSH14 0xff..ff AND -> [r1, packed]
    code.push(0x6d);
    code.extend_from_slice(&[0xff; 14]);
    code.push(0x16);
    // Swap: [packed, r1]
    code.push(0x90);
    // Mask lower: PUSH14 0xff..ff AND -> [r0, r1]
    code.push(0x6d);
    code.extend_from_slice(&[0xff; 14]);
    code.push(0x16);
    // Jump to store
    let jump_store = push2_jump(&mut code, 0x56);

    // Unpacked branch: slot 8 is r0, slot 9 is r1
    let dest_unpacked = code.len() as u16;
    patch_jump(&mut code, jump_unpacked, dest_unpacked);
    code.push(0x5b); // JUMPDEST [upper, packed]
    code.push(0x50); // POP upper -> [packed = r0]
    code.extend_from_slice(&[0x60, 0x09, 0x54]); // SLOAD slot 9 -> [r1, r0]
    code.push(0x90); // SWAP1 -> [r0, r1]

    // Store branch:
    let dest_store = code.len() as u16;
    patch_jump(&mut code, jump_store, dest_store);
    code.push(0x5b); // JUMPDEST [r0, r1]
    code.extend_from_slice(&[0x5f, 0x52]); // MSTORE(0, r0) -> [r1]
    code.extend_from_slice(&[0x60, 0x20, 0x52]); // MSTORE(0x20, r1)
    code.extend_from_slice(&[0x42, 0x60, 0x40, 0x52]); // MSTORE(0x40, timestamp)
    code.extend_from_slice(&[0x60, 0x60, 0x5f, 0xf3]); // RETURN(0, 0x60)

    // --- swap (0x022c0d9f) ---
    let dest_swap = code.len() as u16;
    patch_jump(&mut code, jump_swap, dest_swap);
    code.push(0x5b); // JUMPDEST

    // 1. Reentrancy lock (slot 0 = 2)
    code.extend_from_slice(&[0x60, 0x02, 0x5f, 0x55]);

    // 2. Cold storage touches (~80k gas per hop)
    for slot in [0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a] {
        code.extend_from_slice(&[0x60, slot, 0x54, 0x50]);
    }
    // SLOAD & SSTORE slot 8
    code.extend_from_slice(&[0x60, 0x08, 0x54, 0x60, 0x08, 0x55]);
    // SLOAD slot 10 (tokenIn)
    code.extend_from_slice(&[0x60, 0x0a, 0x54, 0x50]);

    // 3. Determine tokenOut: read slot 9
    code.extend_from_slice(&[0x60, 0x09, 0x54]); // [slot9]
    // Check if slot9 is a valid token address (shr(156, slot9) > 0)
    code.extend_from_slice(&[0x80, 0x60, 0x9c, 0x1c, 0x15]); // [is_zero, slot9]
    let jump_not_token = push2_jump(&mut code, 0x57);

    // Slot 9 is a valid token address:
    let jump_has_token = push2_jump(&mut code, 0x56);

    // Not a token address (legacy test fixture fallback):
    let dest_not_token = code.len() as u16;
    patch_jump(&mut code, jump_not_token, dest_not_token);
    code.push(0x5b); // JUMPDEST [slot9]
    code.push(0x50); // POP -> []

    // Check if calldataload(0x44) == caller()
    code.extend_from_slice(&[0x60, 0x44, 0x35, 0x33, 0x14]); // [to == caller]
    let jump_is_caller = push2_jump(&mut code, 0x57);

    // If to != caller, skip transfer completely (jump straight to unlock reentrancy)
    let jump_skip_transfer = push2_jump(&mut code, 0x56);

    // If to == caller: fallback to parameterized WETH
    let dest_is_caller = code.len() as u16;
    patch_jump(&mut code, jump_is_caller, dest_is_caller);
    code.push(0x5b); // JUMPDEST []
    code.push(0x73); // PUSH20 weth
    code.extend_from_slice(weth.as_slice()); // [weth]

    let dest_has_token = code.len() as u16;
    patch_jump(&mut code, jump_has_token, dest_has_token);
    code.push(0x5b); // JUMPDEST [tokenOut]

    // 4. Determine amountOut = amount0Out != 0 ? amount0Out : amount1Out
    code.extend_from_slice(&[0x60, 0x04, 0x35]); // CALLDATALOAD(4) -> [amount0, tokenOut]
    code.push(0x80); // DUP1
    let jump_has_amount0 = push2_jump(&mut code, 0x57);
    code.extend_from_slice(&[0x50, 0x60, 0x24, 0x35]); // POP, CALLDATALOAD(0x24) -> [amount1, tokenOut]
    let dest_has_amount0 = code.len() as u16;
    patch_jump(&mut code, jump_has_amount0, dest_has_amount0);
    code.push(0x5b); // JUMPDEST [amountOut, tokenOut]

    // 5. Build IERC20(tokenOut).transfer(to, amountOut) calldata in memory:
    // selector 0xa9059cbb at 0x00
    code.extend_from_slice(&[0x63, 0xa9, 0x05, 0x9c, 0xbb, 0x60, 0xe0, 0x1b, 0x5f, 0x52]);
    // to at 0x04 (from calldataload(0x44))
    code.extend_from_slice(&[0x60, 0x44, 0x35, 0x60, 0x04, 0x52]);
    // amountOut at 0x24
    code.extend_from_slice(&[0x60, 0x24, 0x52]); // [tokenOut]

    // 6. CALL tokenOut: call(gas(), tokenOut, 0, 0, 0x44, 0, 0x20)
    code.extend_from_slice(&[0x60, 0x20, 0x5f, 0x60, 0x44, 0x5f, 0x5f, 0x85, 0x5a, 0xf1, 0x50, 0x50]);

    // 7. Unlock reentrancy (slot 0 = 1)
    let dest_unlock = code.len() as u16;
    patch_jump(&mut code, jump_skip_transfer, dest_unlock);
    code.push(0x5b); // JUMPDEST
    code.extend_from_slice(&[0x60, 0x01, 0x5f, 0x55]);

    // 8. Return true (memory[0] = 1, return(0, 32))
    code.extend_from_slice(&[0x60, 0x01, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3]);

    Bytecode::new_raw(Bytes::from(code))
}

/// Returns realistic mock V2 pair runtime bytecode defaulting to Arbitrum WETH.
pub fn mock_realistic_v2_pair_bytecode() -> Bytecode {
    mock_v2_pair_bytecode_for_weth(flash_core::constants::arbitrum::WETH)
}


/// Returns the realistic mock WETH runtime bytecode as a revm `Bytecode`
pub fn mock_realistic_weth_bytecode() -> Bytecode {
    Bytecode::new_raw(Bytes::from_static(MOCK_REALISTIC_WETH_RUNTIME_BYTECODE))
}

/// Computes the storage slot for an account balance in `MOCK_REALISTIC_WETH_RUNTIME_BYTECODE`
pub fn realistic_weth_balance_slot(account: Address) -> alloy_primitives::U256 {
    let mut data = [0u8; 64];
    data[12..32].copy_from_slice(account.as_slice());
    let hash = alloy_primitives::keccak256(data);
    alloy_primitives::U256::from_be_bytes(hash.0)
}

