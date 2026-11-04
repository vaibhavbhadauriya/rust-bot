//! Honeypot Transaction Inspector implementing revm::Inspector.
//! Authoritative source: ORIGINAL_REQUEST.md lines 43-46, 89; PROJECT.md line 60.

use alloy_primitives::{Address, Log, B256, U256, U512};
use revm::{
    interpreter::{
        interpreter_types::{InputsTr, InterpreterTypes, Jumps, StackTr},
        CallInputs, CallOutcome, Interpreter,
    },
    Inspector,
};
use std::collections::HashMap;

/// Standard ERC-20 Transfer topic0: keccak256("Transfer(address,address,uint256)")
pub const TRANSFER_TOPIC: B256 = B256::new([
    0xdd, 0xf2, 0x52, 0xad, 0x1b, 0xe2, 0xc8, 0x9b,
    0x69, 0xc2, 0xb0, 0x68, 0xfc, 0x37, 0x8d, 0xaa,
    0x95, 0x2b, 0xa7, 0xf1, 0x63, 0xc4, 0xa1, 0x16,
    0x28, 0xf5, 0x5a, 0x4d, 0xf5, 0x23, 0xb3, 0xef,
]);

/// EVM Opcodes
pub const OPCODE_ORIGIN: u8 = 0x32;
pub const OPCODE_GASPRICE: u8 = 0x3A;
pub const OPCODE_COINBASE: u8 = 0x41;
pub const OPCODE_SLOAD: u8 = 0x54;
pub const OPCODE_SSTORE: u8 = 0x55;
pub const OPCODE_REVERT: u8 = 0xFD;

#[derive(Debug, Clone, PartialEq)]
pub enum HoneypotViolation {
    SalmonellaPartialDelivery { emitted: U256, delivered: U256 },
    OriginSandwichTrap,
    GasPriceSandwichTrap,
    CoinbaseSandwichTrap,
    ExcessiveFeeOnTransfer { fee_percent: f64 },
    BlacklistedRecipient,
    ExecutionReverted { reason: String },
}

#[derive(Debug, Clone, Default)]
pub struct StorageMutation {
    pub slot: U256,
    pub old_value: U256,
    pub new_value: U256,
}

/// Transaction Inspector for Honeypot & Asset Flow Tracing
#[derive(Debug, Clone)]
pub struct HoneypotTxInspector {
    pub target_token: Address,
    pub recipient: Address,
    pub emitted_amount: U256,
    pub storage_delta: U256,
    pub storage_writes: HashMap<U256, StorageMutation>,
    pub used_origin_opcode: bool,
    pub used_gasprice_opcode: bool,
    pub used_coinbase_opcode: bool,
    pub reverted: bool,
    pub revert_reason: Option<String>,
    pub violation: Option<HoneypotViolation>,
}

impl HoneypotTxInspector {
    pub fn new(target_token: Address, recipient: Address) -> Self {
        Self {
            target_token,
            recipient,
            emitted_amount: U256::ZERO,
            storage_delta: U256::ZERO,
            storage_writes: HashMap::new(),
            used_origin_opcode: false,
            used_gasprice_opcode: false,
            used_coinbase_opcode: false,
            reverted: false,
            revert_reason: None,
            violation: None,
        }
    }

    /// Evaluates collected traces and storage deltas against Honeypot criteria
    pub fn evaluate_safety(&self) -> Result<(), HoneypotViolation> {
        if let Some(ref v) = self.violation {
            return Err(v.clone());
        }

        // 1. Salmonella & Fee-on-Transfer Check:
        if self.storage_delta < self.emitted_amount {
            // Salmonella trap detection: storage_delta * 5 <= emitted_amount (delivering <= 20%)
            if self.storage_delta.checked_mul(U256::from(5)).map_or(false, |val| val <= self.emitted_amount) {
                return Err(HoneypotViolation::SalmonellaPartialDelivery {
                    emitted: self.emitted_amount,
                    delivered: self.storage_delta,
                });
            }

            // Listeria / Fee-on-Transfer detection: storage_delta * 10 < emitted_amount * 9 (tax > 10%)
            let ten_delta = U512::from(self.storage_delta) * U512::from(10);
            let nine_emitted = U512::from(self.emitted_amount) * U512::from(9);
            if ten_delta < nine_emitted {
                let diff = self.emitted_amount.saturating_sub(self.storage_delta);
                let fee_pct = if !self.emitted_amount.is_zero() {
                    let scaled = (U512::from(diff) * U512::from(1_000_000))
                        / U512::from(self.emitted_amount);
                    (scaled.to::<u64>() as f64) / 10_000.0
                } else {
                    0.0
                };
                return Err(HoneypotViolation::ExcessiveFeeOnTransfer { fee_percent: fee_pct });
            }
        }

        if self.used_origin_opcode {
            return Err(HoneypotViolation::OriginSandwichTrap);
        }
        if self.used_gasprice_opcode {
            return Err(HoneypotViolation::GasPriceSandwichTrap);
        }
        if self.used_coinbase_opcode {
            return Err(HoneypotViolation::CoinbaseSandwichTrap);
        }

        Ok(())
    }
}

impl<CTX, INTR: InterpreterTypes> Inspector<CTX, INTR> for HoneypotTxInspector
where
    INTR::Bytecode: Jumps,
    INTR::Stack: StackTr,
    INTR::Input: InputsTr,
{
    fn step(&mut self, interp: &mut Interpreter<INTR>, _context: &mut CTX) {
        let opcode = interp.bytecode.opcode();
        match opcode {
            OPCODE_ORIGIN => {
                self.used_origin_opcode = true;
                if self.violation.is_none() {
                    self.violation = Some(HoneypotViolation::OriginSandwichTrap);
                }
            }
            OPCODE_GASPRICE => {
                self.used_gasprice_opcode = true;
                if self.violation.is_none() {
                    self.violation = Some(HoneypotViolation::GasPriceSandwichTrap);
                }
            }
            OPCODE_COINBASE => {
                self.used_coinbase_opcode = true;
                if self.violation.is_none() {
                    self.violation = Some(HoneypotViolation::CoinbaseSandwichTrap);
                }
            }
            OPCODE_SSTORE => {
                let target = interp.input.target_address();
                if target == self.target_token {
                    let stack = interp.stack.data();
                    let len = stack.len();
                    if len >= 2 {
                        let slot = stack[len - 1];
                        let val = stack[len - 2];
                        let mutation = self.storage_writes.entry(slot).or_default();
                        mutation.slot = slot;
                        mutation.new_value = val;
                    }
                }
            }
            _ => {}
        }
    }

    fn log(&mut self, _context: &mut CTX, log: Log) {
        if log.address == self.target_token && !log.topics().is_empty() && log.topics()[0] == TRANSFER_TOPIC {
            if log.topics().len() >= 3 {
                let to = Address::from_word(log.topics()[2]);
                if to == self.recipient && log.data.data.len() >= 32 {
                    let amount = U256::from_be_slice(&log.data.data[..32]);
                    self.emitted_amount = self.emitted_amount.saturating_add(amount);
                }
            }
        }
    }

    fn call_end(&mut self, _context: &mut CTX, _inputs: &CallInputs, outcome: &mut CallOutcome) {
        if outcome.result.result.is_revert() {
            self.reverted = true;
            let reason = decode_revert_reason(&outcome.result.output);
            self.revert_reason = Some(reason.clone());
            if self.violation.is_none() {
                if reason.to_lowercase().contains("blacklist") || reason.to_lowercase().contains("blocked") {
                    self.violation = Some(HoneypotViolation::BlacklistedRecipient);
                } else {
                    self.violation = Some(HoneypotViolation::ExecutionReverted { reason });
                }
            }
        }
    }
}

/// Decodes standard ABI error strings and panic codes from revert bytes
pub fn decode_revert_reason(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "Empty revert".to_string();
    }
    // Error(string) selector: 0x08c379a0
    if bytes.len() >= 68 && bytes[0..4] == [0x08, 0xc3, 0x79, 0xa0] {
        let len = U256::from_be_slice(&bytes[36..68]).to::<usize>();
        if bytes.len() >= 68 + len {
            if let Ok(s) = std::str::from_utf8(&bytes[68..68 + len]) {
                return s.to_string();
            }
        }
    }
    // Panic(uint256) selector: 0x4e487b71
    if bytes.len() >= 36 && bytes[0..4] == [0x4e, 0x48, 0x7b, 0x71] {
        let code = U256::from_be_slice(&bytes[4..36]);
        return format!("Panic(0x{:x})", code);
    }
    // Fallback: try direct UTF-8 string
    if let Ok(s) = std::str::from_utf8(bytes) {
        let trimmed = s.trim();
        if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
            return trimmed.to_string();
        }
    }
    format!("Custom error (0x{})", alloy_primitives::hex::encode(bytes))
}
