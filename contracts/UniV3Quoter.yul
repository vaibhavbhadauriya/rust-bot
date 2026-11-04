// SPDX-License-Identifier: MIT
// Custom In-Memory UniV3Quoter Bytecode (Cancun EVM compatible)
// ABI: quoteExactInputSingle(address tokenIn, address tokenOut, uint24 fee, uint256 amountIn, uint160 sqrtPriceLimitX96)
//      -> (uint256 amountOut, uint160 sqrtPriceX96After, uint32 initializedTicksCrossed, uint256 gasEstimate)
// Selectors supported:
// - 0xf7729d43: quoteExactInputSingle(address,address,uint24,uint256,uint160)
// - 0xc6a5026a: quoteExactInputSingle((address,address,uint256,uint24,uint160))
object "UniV3Quoter" {
    code {
        // Constructor: copy runtime code to memory and return
        datacopy(0, dataoffset("runtime"), datasize("runtime"))
        return(0, datasize("runtime"))
    }
    object "runtime" {
        code {
            // Scratchpad memory layout:
            // 0x00..0x80: Return data (amountOut, sqrtPriceX96After, ticksCrossed, gasEstimate)
            // 0x80: x_virt
            // 0xa0: y_virt
            // 0xc0: amountInWithFee
            // 0xe0: liquidity
            // 0x100: sqrtPriceX96
            // 0x120: zeroForOne
            // 0x140: sqrtPriceLimitX96

            // Enforce minimum calldatasize: 4 selector + 5 * 32 = 164 (0xa4)
            if lt(calldatasize(), 0xa4) {
                revert(0, 0)
            }

            // Extract selector
            let selector := shr(0xe0, calldataload(0))
            let tokenIn := 0
            let tokenOut := 0
            let fee := 0
            let amountIn := 0
            let sqrtPriceLimitX96 := 0

            switch selector
            case 0xf7729d43 {
                // Flat params
                tokenIn := calldataload(0x04)
                tokenOut := calldataload(0x24)
                fee := calldataload(0x44)
                amountIn := calldataload(0x64)
                sqrtPriceLimitX96 := calldataload(0x84)
            }
            case 0xc6a5026a {
                // Tuple params: offset at 0x04 points to struct
                let base := add(0x04, calldataload(0x04))
                tokenIn := calldataload(base)
                tokenOut := calldataload(add(base, 0x20))
                amountIn := calldataload(add(base, 0x40))
                fee := calldataload(add(base, 0x60))
                sqrtPriceLimitX96 := calldataload(add(base, 0x80))
            }
            default {
                revert(0, 0)
            }

            // zeroForOne = tokenIn < tokenOut
            let zeroForOne := lt(tokenIn, tokenOut)

            // Load pool state: check slot 0 (sqrtPriceX96) and slot 1 (liquidity)
            let sqrtPriceX96 := sload(0)
            let liquidity := sload(1)

            // If slot 0 is zero, check pool-specific key: keccak256(tokenIn, tokenOut, fee)
            if iszero(sqrtPriceX96) {
                mstore(0x00, tokenIn)
                mstore(0x20, tokenOut)
                mstore(0x40, fee)
                let poolKey := keccak256(0x00, 0x60)
                sqrtPriceX96 := sload(poolKey)
                liquidity := sload(add(poolKey, 1))
            }

            // Validate non-zero state
            if or(iszero(sqrtPriceX96), iszero(liquidity)) {
                revert(0, 0)
            }

            // If amountIn == 0, return zero amountOut immediately
            if iszero(amountIn) {
                mstore(0x00, 0)
                mstore(0x20, sqrtPriceX96)
                mstore(0x40, 0)
                mstore(0x60, 105000)
                return(0x00, 0x80)
            }

            // Effective amount in: amountInWithFee = (amountIn * (1_000_000 - fee)) / 1_000_000
            let feeComplement := sub(1000000, fee)
            let amountInWithFee := div(mul(amountIn, feeComplement), 1000000)

            // Virtual reserves:
            // x_virt = (liquidity << 96) / sqrtPriceX96
            // y_virt = (liquidity * sqrtPriceX96) >> 96
            // Guard against 256-bit overflow on liquidity << 96
            if gt(shr(160, liquidity), 0) {
                revert(0, 0)
            }
            let x_virt := div(shl(96, liquidity), sqrtPriceX96)
            let y_virt := shr(96, mul(liquidity, sqrtPriceX96))

            let amountOut := 0
            let sqrtPriceX96After := 0
            let initializedTicksCrossed := 0

            switch zeroForOne
            case 1 {
                // Swapping token0 for token1
                // denom = x_virt + amountInWithFee
                let denom := add(x_virt, amountInWithFee)
                amountOut := div(mul(y_virt, amountInWithFee), denom)
                sqrtPriceX96After := div(shl(96, liquidity), denom)

                // Limit price check
                if and(gt(sqrtPriceLimitX96, 0), lt(sqrtPriceX96After, sqrtPriceLimitX96)) {
                    sqrtPriceX96After := sqrtPriceLimitX96
                    let deltaP := sub(sqrtPriceX96, sqrtPriceLimitX96)
                    amountOut := shr(96, mul(liquidity, deltaP))
                }
            }
            default {
                // Swapping token1 for token0
                // denom = y_virt + amountInWithFee
                let denom := add(y_virt, amountInWithFee)
                // Guard against 256-bit overflow on denom << 96
                if gt(shr(160, denom), 0) {
                    revert(0, 0)
                }
                amountOut := div(mul(x_virt, amountInWithFee), denom)
                sqrtPriceX96After := div(shl(96, denom), liquidity)

                // Limit price check
                if and(gt(sqrtPriceLimitX96, 0), gt(sqrtPriceX96After, sqrtPriceLimitX96)) {
                    sqrtPriceX96After := sqrtPriceLimitX96
                    let deltaP := sub(sqrtPriceLimitX96, sqrtPriceX96)
                    amountOut := div(mul(x_virt, deltaP), sqrtPriceLimitX96)
                }
            }

            // Gas estimate: 105,000 base + 26,000 per crossed tick
            let gasEstimate := add(105000, mul(initializedTicksCrossed, 26000))

            // Return 4-tuple: (amountOut, sqrtPriceX96After, initializedTicksCrossed, gasEstimate)
            mstore(0x00, amountOut)
            mstore(0x20, sqrtPriceX96After)
            mstore(0x40, initializedTicksCrossed)
            mstore(0x60, gasEstimate)
            return(0x00, 0x80)
        }
    }
}
