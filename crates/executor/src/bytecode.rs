//! Embedded Pre-Compiled Bytecode for Pure Yul Arbitrage Executor
//!
//! Authoritative source: contracts/executor.yul compiled with solc 0.8.38 cancun optimizer.
//! Provides zero-dependency in-memory execution in revm.

use alloy_primitives::{hex, Address, Bytes};
use flash_core::constants::{arbitrum, base};
use revm::bytecode::Bytecode;

/// Raw EVM creation / init bytecode of the FlashArbitrageExecutor pure Yul contract.
/// Size: 1126 bytes.
pub const EXECUTOR_INIT_CODE: &[u8] = &hex!(
    "61045a8061000c5f395ff3fe3480156100a6578060011461004757600214610019575f"
    "80fd5b7311111111111111111111111111111111111111113303610043575f80808047"
    "335af11561004357005b5f80fd5b731111111111111111111111111111111111111111"
    "33036100435760205f60448180803560601c6370a0823160e01b825230600452858260"
    "2481845afa50815163a9059cbb60e01b835260143560601c6004526024525af1156100"
    "4357005b607c3610610043575f3560e01c8080633a48e7e114610415576398dd202a14"
    "6103de577311111111111111111111111111111111111111113303610043575f905b60"
    "7c82019081361061004357823560801c90603084013560601c604485013560601c6058"
    "86013560801c606887013560f01c95606a88013560801c925f915f945f9460029260a3"
    "8d013610156103b4575b506370a0823160e01b5f523060045260205f6024815f516020"
    "61043a5f395f51905f525afa15610043575f519a633a48e7e16398dd202a8b149a1498"
    "8a8a176103aa575b630240bc6b60e21b5f526040602060045f845afa15610043578260"
    "20516040519084156103a2575b50106100435763a9059cbb60e01b5f52806004528b60"
    "245260205f604481805f51602061043a5f395f51905f525af115610043575f8060a492"
    "8195829563022c0d9f60e01b8452831461039657826004526024525b88604452608060"
    "6452816084525af1156100435760a45f80600381958f95607a849763022c0d9f60e01b"
    "8652013560f01c841461038a57836004526024525b1495866001146103815730604452"
    "5b6080606452816084525af11561004357610338575b505050610307575b6102bf575b"
    "506370a0823160e01b5f523060045260205f6024815f51602061043a5f395f51905f52"
    "5afa15610043575f5190808211156102b7576020910391013560801c81106102b05700"
    "5b5f5260205ffd5b5f805260205ffd5b63a9059cbb60e01b5f52336004526024526020"
    "5f604481805f51602061043a5f395f51905f525af11561004357630476983960e21b5f"
    "9081528060048180335af15082610269565b63a9059cbb60e01b5f5233600452816024"
    "5260205f604481805f51602061043a5f395f51905f525af1610264575f80fd5b5f8060"
    "a4928195829563022c0d9f60e01b8452831461037557826004526024525b3060445260"
    "80606452816084525af1156100435785808061025c565b60045281602452610359565b"
    "87604452610247565b60045282602452610238565b600452816024526101f9565b9050"
    "5f6101a4565b9b8b90039b61017c565b60908d013560801c975060a08d013560f01c96"
    "503560601c945060a28c013560f81c92508c610138565b6e04444c5dc75cb358380d2e"
    "3de08a90331473ba1333333333a1ba1108e8412f11850a5c319ba93314171561004357"
    "6044906100e7565b5073bbbbbbbbbb9cc5e90e3b3af64bdaf62c37eeffcb3303610043"
    "576064906100e756fe00000000000000000000000082af49447d8a07e3bd95bd0d56f3"
    "5241523fbab1"
);

/// Raw EVM runtime bytecode of the FlashArbitrageExecutor pure Yul contract.
/// Size: 1114 bytes.
pub const EXECUTOR_RUNTIME_BYTECODE: &[u8] = &hex!(
    "3480156100a6578060011461004757600214610019575f80fd5b731111111111111111"
    "1111111111111111111111113303610043575f80808047335af11561004357005b5f80"
    "fd5b73111111111111111111111111111111111111111133036100435760205f604481"
    "80803560601c6370a0823160e01b8252306004528582602481845afa50815163a9059c"
    "bb60e01b835260143560601c6004526024525af11561004357005b607c361061004357"
    "5f3560e01c8080633a48e7e114610415576398dd202a146103de577311111111111111"
    "111111111111111111111111113303610043575f905b607c8201908136106100435782"
    "3560801c90603084013560601c604485013560601c605886013560801c606887013560"
    "f01c95606a88013560801c925f915f945f9460029260a38d013610156103b4575b5063"
    "70a0823160e01b5f523060045260205f6024815f51602061043a5f395f51905f525afa"
    "15610043575f519a633a48e7e16398dd202a8b149a14988a8a176103aa575b630240bc"
    "6b60e21b5f526040602060045f845afa1561004357826020516040519084156103a257"
    "5b50106100435763a9059cbb60e01b5f52806004528b60245260205f604481805f5160"
    "2061043a5f395f51905f525af115610043575f8060a4928195829563022c0d9f60e01b"
    "8452831461039657826004526024525b886044526080606452816084525af115610043"
    "5760a45f80600381958f95607a849763022c0d9f60e01b8652013560f01c841461038a"
    "57836004526024525b14958660011461038157306044525b6080606452816084525af1"
    "1561004357610338575b505050610307575b6102bf575b506370a0823160e01b5f5230"
    "60045260205f6024815f51602061043a5f395f51905f525afa15610043575f51908082"
    "11156102b7576020910391013560801c81106102b057005b5f5260205ffd5b5f805260"
    "205ffd5b63a9059cbb60e01b5f523360045260245260205f604481805f51602061043a"
    "5f395f51905f525af11561004357630476983960e21b5f9081528060048180335af150"
    "82610269565b63a9059cbb60e01b5f52336004528160245260205f604481805f516020"
    "61043a5f395f51905f525af1610264575f80fd5b5f8060a4928195829563022c0d9f60"
    "e01b8452831461037557826004526024525b306044526080606452816084525af11561"
    "00435785808061025c565b60045281602452610359565b87604452610247565b600452"
    "82602452610238565b600452816024526101f9565b90505f6101a4565b9b8b90039b61"
    "017c565b60908d013560801c975060a08d013560f01c96503560601c945060a28c0135"
    "60f81c92508c610138565b6e04444c5dc75cb358380d2e3de08a90331473ba13333333"
    "33a1ba1108e8412f11850a5c319ba933141715610043576044906100e7565b5073bbbb"
    "bbbbbb9cc5e90e3b3af64bdaf62c37eeffcb3303610043576064906100e756fe000000"
    "00000000000000000082af49447d8a07e3bd95bd0d56f35241523fbab1"
);

/// Returns the runtime bytecode wrapped in revm's Bytecode type for in-memory simulation.
pub fn executor_runtime_bytecode() -> Bytecode {
    Bytecode::new_raw(alloy_primitives::Bytes::from_static(EXECUTOR_RUNTIME_BYTECODE))
}

/// Returns the creation init code bytes.
pub fn executor_init_code() -> &'static [u8] {
    EXECUTOR_INIT_CODE
}

/// Returns the deployed runtime code bytes.
pub fn executor_runtime_bytes() -> &'static [u8] {
    EXECUTOR_RUNTIME_BYTECODE
}

/// Builds chain-parameterized pure Yul executor runtime bytecode.
/// Dynamically injects the canonical WETH address for the given chain_id (42161 -> Arbitrum, 8453 -> Base)
/// and sets the configured OWNER address.
pub fn build_executor_bytecode(chain_id: u64, owner: Address) -> Bytes {
    let weth = match chain_id {
        base::CHAIN_ID => base::WETH,
        _ => arbitrum::WETH,
    };

    let mut runtime = EXECUTOR_RUNTIME_BYTECODE.to_vec();

    // Inject OWNER at verified byte offsets 27, 73, and 203
    runtime[27..47].copy_from_slice(owner.as_slice());
    runtime[73..93].copy_from_slice(owner.as_slice());
    runtime[203..223].copy_from_slice(owner.as_slice());

    // Inject WETH at data pool byte offset 1094
    runtime[1094..1114].copy_from_slice(weth.as_slice());

    Bytes::from(runtime)
}

/// Builds chain-parameterized creation init code.
pub fn build_executor_init_code(chain_id: u64, owner: Address) -> Bytes {
    let weth = match chain_id {
        base::CHAIN_ID => base::WETH,
        _ => arbitrum::WETH,
    };

    let mut init = EXECUTOR_INIT_CODE.to_vec();

    // Inject OWNER at init offsets (12 + 27 = 39, 12 + 73 = 85, 12 + 203 = 215)
    init[39..59].copy_from_slice(owner.as_slice());
    init[85..105].copy_from_slice(owner.as_slice());
    init[215..235].copy_from_slice(owner.as_slice());

    // Inject WETH at init offset (12 + 1094 = 1106)
    init[1106..1126].copy_from_slice(weth.as_slice());

    Bytes::from(init)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    #[test]
    fn test_bytecode_constants_validity() {
        assert_eq!(EXECUTOR_INIT_CODE.len(), 1126);
        assert_eq!(EXECUTOR_RUNTIME_BYTECODE.len(), 1114);
        let bc = executor_runtime_bytecode();
        assert!(!bc.is_empty());
    }

    #[test]
    fn test_parameterized_bytecode_builder() {
        let test_owner = address!("3333333333333333333333333333333333333333");
        let arb_rt = build_executor_bytecode(42161, test_owner);
        assert_eq!(arb_rt.len(), 1114);
        assert_eq!(&arb_rt[27..47], test_owner.as_slice());
        assert_eq!(&arb_rt[73..93], test_owner.as_slice());
        assert_eq!(&arb_rt[203..223], test_owner.as_slice());
        assert_eq!(&arb_rt[1094..1114], arbitrum::WETH.as_slice());

        let base_rt = build_executor_bytecode(8453, test_owner);
        assert_eq!(base_rt.len(), 1114);
        assert_eq!(&base_rt[27..47], test_owner.as_slice());
        assert_eq!(&base_rt[73..93], test_owner.as_slice());
        assert_eq!(&base_rt[203..223], test_owner.as_slice());
        assert_eq!(&base_rt[1094..1114], base::WETH.as_slice());

        let arb_init = build_executor_init_code(42161, test_owner);
        assert_eq!(arb_init.len(), 1126);
        assert_eq!(&arb_init[39..59], test_owner.as_slice());
        assert_eq!(&arb_init[85..105], test_owner.as_slice());
        assert_eq!(&arb_init[215..235], test_owner.as_slice());
        assert_eq!(&arb_init[1106..1126], arbitrum::WETH.as_slice());

        let base_init = build_executor_init_code(8453, test_owner);
        assert_eq!(base_init.len(), 1126);
        assert_eq!(&base_init[39..59], test_owner.as_slice());
        assert_eq!(&base_init[85..105], test_owner.as_slice());
        assert_eq!(&base_init[215..235], test_owner.as_slice());
        assert_eq!(&base_init[1106..1126], base::WETH.as_slice());
    }
}
