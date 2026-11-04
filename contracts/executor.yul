object "FlashArbitrageExecutor" {
    code {
        // Constructor / Init code: copy runtime code to memory and return it
        datacopy(0, dataoffset("runtime"), datasize("runtime"))
        return(0, datasize("runtime"))
    }
    object "runtime" {
        code {
            // Immutable constants (Dynamically parameterized via build_executor_bytecode / build_executor_init_code)
            // Default: Arbitrum One (Chain ID 42161) WETH: 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1
            // Base (Chain ID 8453) WETH: 0x4200000000000000000000000000000000000006
            let OWNER := 0x1111111111111111111111111111111111111111
            let WETH  := 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1 // Canonical Arbitrum WETH
            let MORPHO_BLUE := 0xBBBBBbbBBb9cC5e90e3b3Af64bdAF62C37EEFFCb
            let BALANCER_V3_VAULT := 0xbA1333333333a1BA1108E8412f11850A5C319bA9
            let UNISWAP_V4_POOL_MANAGER := 0x000000000004444c5dc75cB358380D2e3dE08A90

            // Selector-less entry: switch on callvalue()
            // 0 wei = Arbitrage execution / flash loan callback
            // 1 wei = Owner ERC-20 sweep
            // 2 wei = Owner ETH sweep
            switch callvalue()
            case 0 {
                let csize := calldatasize()
                if lt(csize, 0x7c) {
                    revert(0, 0)
                }

                // Check for flash loan callback selectors
                // Morpho Blue onMorphoFlashLoan: 0x3a48e7e1
                // Balancer V3 / Uniswap V4 unlockCallback: 0x98dd202a
                let selector := shr(0xe0, calldataload(0x00))
                let dataOffset := 0x00

                switch selector
                case 0x3a48e7e1 {
                    // onMorphoFlashLoan: calldata has selector (4B), assets (32B), offset (32B), len (32B) -> data starts at 0x64
                    if iszero(eq(caller(), MORPHO_BLUE)) { revert(0, 0) }
                    dataOffset := 0x64
                }
                case 0x98dd202a {
                    // unlockCallback: calldata has selector (4B), offset (32B), len (32B) -> data starts at 0x44
                    if iszero(or(eq(caller(), BALANCER_V3_VAULT), eq(caller(), UNISWAP_V4_POOL_MANAGER))) { revert(0, 0) }
                    dataOffset := 0x44
                }
                default {
                    // Direct calldata execution: enforce OWNER
                    if iszero(eq(caller(), OWNER)) { revert(0, 0) }
                    dataOffset := 0x00
                }

                // Enforce minimum 124-byte payload (0x7c)
                if lt(csize, add(dataOffset, 0x7c)) {
                    revert(0, 0)
                }

                // --- Calldata Decoding via shr + calldataload (124-byte layout) ---
                let wethAmountToFirstMarket := shr(0x80, calldataload(dataOffset))
                let gasCost                 := shr(0x80, calldataload(add(dataOffset, 0x20)))
                let target1                 := shr(0x60, calldataload(add(dataOffset, 0x30)))
                let target2                 := shr(0x60, calldataload(add(dataOffset, 0x44)))
                let token0_amountOut        := shr(0x80, calldataload(add(dataOffset, 0x58)))
                let token0_amountOutIndex   := shr(0xf0, calldataload(add(dataOffset, 0x68)))
                let token1_amountOut        := shr(0x80, calldataload(add(dataOffset, 0x6a)))
                let token1_amountOutIndex   := shr(0xf0, calldataload(add(dataOffset, 0x7a)))

                // Optional 3-hop extensions
                let target3                 := 0
                let token2_amountOut        := 0
                let token2_amountOutIndex   := 0
                let hopCount                := 2

                if iszero(lt(csize, add(dataOffset, 0xa3))) {
                    target3                 := shr(0x60, calldataload(add(dataOffset, 0x7c)))
                    token2_amountOut        := shr(0x80, calldataload(add(dataOffset, 0x90)))
                    token2_amountOutIndex   := shr(0xf0, calldataload(add(dataOffset, 0xa0)))
                    hopCount                := shr(0xf8, calldataload(add(dataOffset, 0xa2)))
                }

                // --- Dynamic On-Chain Balance Measurement ---
                mstore(0x00, 0x70a0823100000000000000000000000000000000000000000000000000000000)
                mstore(0x04, address())
                let balCheck0 := staticcall(gas(), WETH, 0x00, 0x24, 0x00, 0x20)
                if iszero(balCheck0) { revert(0, 0) }
                let balanceBefore := mload(0x00)
                if or(eq(selector, 0x3a48e7e1), eq(selector, 0x98dd202a)) {
                    balanceBefore := sub(balanceBefore, wethAmountToFirstMarket)
                }

                // --- V-14: Fast-Revert Guard ---
                // Query Pool 1 reserves via staticcall to getReserves() (selector 0x0902f1ac)
                mstore(0x00, 0x0902f1ac00000000000000000000000000000000000000000000000000000000)
                let success := staticcall(gas(), target1, 0x00, 0x04, 0x20, 0x40)
                if iszero(success) { revert(0, 0) }
                let currentReserve0 := mload(0x20)
                let currentReserve1 := mload(0x40)
                let currentReserveOut := currentReserve1
                if iszero(token0_amountOutIndex) {
                    currentReserveOut := currentReserve0
                }
                if lt(currentReserveOut, token0_amountOut) {
                    revert(0, 0)
                }

                // --- Step 1: Transfer Borrowed WETH to Target 1 ---
                // WETH.transfer(target1, wethAmountToFirstMarket) - selector 0xa9059cbb
                mstore(0x00, 0xa9059cbb00000000000000000000000000000000000000000000000000000000)
                mstore(0x04, target1)
                mstore(0x24, wethAmountToFirstMarket)
                let xfer1 := call(gas(), WETH, 0, 0x00, 0x44, 0x00, 0x20)
                if iszero(xfer1) { revert(0, 0) }

                // --- Step 2: Execute Swap on Target 1 ---
                // swap(uint amount0Out, uint amount1Out, address to, bytes data) - selector 0x022c0d9f
                mstore(0x00, 0x022c0d9f00000000000000000000000000000000000000000000000000000000)
                switch token0_amountOutIndex
                case 0 {
                    mstore(0x04, token0_amountOut)
                    mstore(0x24, 0)
                }
                default {
                    mstore(0x04, 0)
                    mstore(0x24, token0_amountOut)
                }
                mstore(0x44, target2) // Output goes to Pool 2
                mstore(0x64, 0x80)    // Offset to empty bytes
                mstore(0x84, 0)       // Zero bytes length
                let swapOk1 := call(gas(), target1, 0, 0x00, 0xa4, 0x00, 0x00)
                if iszero(swapOk1) { revert(0, 0) }

                // --- Step 3: Execute Swap on Target 2 ---
                mstore(0x00, 0x022c0d9f00000000000000000000000000000000000000000000000000000000)
                switch token1_amountOutIndex
                case 0 {
                    mstore(0x04, token1_amountOut)
                    mstore(0x24, 0)
                }
                default {
                    mstore(0x04, 0)
                    mstore(0x24, token1_amountOut)
                }
                switch eq(hopCount, 3)
                case 1 {
                    mstore(0x44, target3) // Output goes to Pool 3
                }
                default {
                    mstore(0x44, address()) // Output WETH returns to executor
                }
                mstore(0x64, 0x80)
                mstore(0x84, 0)
                let swapOk2 := call(gas(), target2, 0, 0x00, 0xa4, 0x00, 0x00)
                if iszero(swapOk2) { revert(0, 0) }

                // --- Step 3b: Execute Swap on Target 3 (if 3-hop) ---
                if eq(hopCount, 3) {
                    mstore(0x00, 0x022c0d9f00000000000000000000000000000000000000000000000000000000)
                    switch token2_amountOutIndex
                    case 0 {
                        mstore(0x04, token2_amountOut)
                        mstore(0x24, 0)
                    }
                    default {
                        mstore(0x04, 0)
                        mstore(0x24, token2_amountOut)
                    }
                    mstore(0x44, address()) // Final output WETH returns to executor
                    mstore(0x64, 0x80)
                    mstore(0x84, 0)
                    let swapOk3 := call(gas(), target3, 0, 0x00, 0xa4, 0x00, 0x00)
                    if iszero(swapOk3) { revert(0, 0) }
                }

                // --- Step 4: Flash Loan Repayment & Settlement ---
                if eq(selector, 0x3a48e7e1) {
                    // Morpho Blue repayment
                    mstore(0x00, 0xa9059cbb00000000000000000000000000000000000000000000000000000000)
                    mstore(0x04, caller())
                    mstore(0x24, wethAmountToFirstMarket)
                    let repay := call(gas(), WETH, 0, 0x00, 0x44, 0x00, 0x20)
                    if iszero(repay) { revert(0, 0) }
                }

                if eq(selector, 0x98dd202a) {
                    // Balancer V3 Vault / Uniswap V4 PoolManager repayment & settlement
                    // 1. Transfer borrowed principal back to caller()
                    mstore(0x00, 0xa9059cbb00000000000000000000000000000000000000000000000000000000)
                    mstore(0x04, caller())
                    mstore(0x24, wethAmountToFirstMarket)
                    let repayBal := call(gas(), WETH, 0, 0x00, 0x44, 0x00, 0x20)
                    if iszero(repayBal) { revert(0, 0) }

                    // 2. Call settle() on caller() (selector 0x11da60e4)
                    mstore(0x00, 0x11da60e400000000000000000000000000000000000000000000000000000000)
                    pop(call(gas(), caller(), 0, 0x00, 0x04, 0x00, 0x00))
                }

                // --- Step 5: Atomic Profit Enforcement ---
                // Query WETH.balanceOf(address(this)) selector 0x70a08231
                mstore(0x00, 0x70a0823100000000000000000000000000000000000000000000000000000000)
                mstore(0x04, address())
                let balCheck := staticcall(gas(), WETH, 0x00, 0x24, 0x00, 0x20)
                if iszero(balCheck) { revert(0, 0) }
                let balanceAfter := mload(0x00)

                if iszero(gt(balanceAfter, balanceBefore)) {
                    mstore(0x00, 0)
                    revert(0x00, 0x20) // Revert with 32-byte error data
                }
                let wethProfit := sub(balanceAfter, balanceBefore)
                if lt(wethProfit, gasCost) {
                    mstore(0x00, wethProfit)
                    revert(0x00, 0x20) // Revert if net profit does not cover gas cost
                }

                stop()
            }
            case 1 {
                // Owner ERC-20 Sweep (callvalue == 1 wei)
                if iszero(eq(caller(), OWNER)) { revert(0, 0) }
                let token := shr(0x60, calldataload(0x00))
                let to    := shr(0x60, calldataload(0x14))
                mstore(0x00, 0x70a0823100000000000000000000000000000000000000000000000000000000)
                mstore(0x04, address())
                pop(staticcall(gas(), token, 0x00, 0x24, 0x00, 0x20))
                let bal := mload(0x00)
                mstore(0x00, 0xa9059cbb00000000000000000000000000000000000000000000000000000000)
                mstore(0x04, to)
                mstore(0x24, bal)
                let ok := call(gas(), token, 0, 0x00, 0x44, 0x00, 0x20)
                if iszero(ok) { revert(0, 0) }
                stop()
            }
            case 2 {
                // Owner ETH Sweep (callvalue == 2 wei)
                if iszero(eq(caller(), OWNER)) { revert(0, 0) }
                let ok := call(gas(), caller(), selfbalance(), 0, 0, 0, 0)
                if iszero(ok) { revert(0, 0) }
                stop()
            }
            default {
                revert(0, 0)
            }
        }
    }
}
