# Pre-Production Security & Code Audit Report
## Arbitrum One & Base DeFi Flash Loan Arbitrage Engine

---

### Document Metadata

- **Executive Title**: Master Pre-Production Security, Architecture & Forensic Integrity Audit Report
- **Target Workspace**: `c:\Coding\Rust bot for crypto`
- **Target System**: Rust DeFi Multi-Chain Flash Loan Arbitrage Engine (Arbitrum One `42161` & Base `8453`)
- **Audit Type**: Adversarial, Zero-Trust Pre-Production Code, Bytecode & Runtime Audit
- **Auditor**: Multi-Stream Audit Synthesis Working Group (Streams 1–5 Synthesis)
- **Timestamp**: November 1, 2026 (2026-11-01T01:00:00Z)
- **Audit Constraints**: Strict Read-Only Mode (Zero Source Modifications; Deterministic Line Number Integrity)
- **Forensic Integrity Verdict**: 🔴 **INTEGRITY VIOLATION**
- **Live Readiness Verdict**: 🔴 **CRITICAL BLOCKER — DO NOT RUN IN `--live` MODE**

---

## 1. Executive Summary & Live Readiness Verdict

### Unambiguous Verdict: What happens if `--live` is run right now with funded EOAs?

**CATASTROPHIC IMMEDIATE FAILURE, IMMEDIATE ACCOUNT BRICKING, AND 100% CAPITAL LOSS.**

If an operator launches the bot today using `cargo run --release -- --live` with funded private keys and live RPC connections, the system will not execute a single profitable arbitrage transaction. Instead, it enters an immediate sequence of fatal execution cascades that deplete gas funds, brick signer accounts, and expose resting smart contract balances to external drainage:

```
[STARTUP]
  │
  ├── 1. Nonce 0 Initialization (signers.rs:47)
  │      └── Sequencer Mempool Rejection: {"code": -32000, "message": "nonce too low"}
  │          (Any funded EOA with transaction history fails immediately)
  │
  ├── 2. Corrupted Access List RLP Encoding (signers.rs:122-126)
  │      └── Node Parser Abort: "rlp: expected list, got byte string"
  │          (Writes 0x80 instead of 0xc0; 100% of signed EIP-1559 envelopes are invalid)
  │
  ├── 3. Precompile 0x01 Broadcast Target (pipeline.rs:196)
  │      └── Transactions Sent to 0x000...001 (ecrecover) Instead of Deployed Executor
  │          (Burns L2 priority bribes and execution gas; zero arbitrage swaps executed)
  │
  ├── 4. Severe Under-Gassed Out-of-Gas Reverts (pipeline.rs:169, engine.rs:141)
  │      └── Pool Bytecode Mock No-Op (250 gas) caps gas_limit at 100,000 gas
  │          (Real multi-hop swaps require 180,000–240,000 gas; 100% of mined txs revert OutOfGas)
  │
  ├── 5. Permanent Signer Bricking on Broadcast Errors (signers.rs:244, pipeline.rs:229)
  │      └── Local nonce increments prior to HTTP dispatch; network drops/rejections leave
  │          permanent nonce gaps, freezing all subsequent EOA transactions
  │
  ├── 6. Incomplete 3-Hop Route Execution (engine.rs:184-197, executor.yul)
  │      └── Hop 3 Dropped; Pool 2 outputs Token C to executor instead of WETH;
  │          contract balance check fails; 100% of triangular routes revert on-chain
  │
  ├── 7. Complete Omission of L1 Data / Blob Fees (engine.rs:228, pipeline.rs:169-189)
  │      └── Nitro/OP-Stack L1 Posting Fees ($0.50–$3.50) excluded from net profit;
  │          bot executes micro-trades yielding $0.20, bleeding $0.30–$2.70 net per transaction
  │
  └── 8. Contract Resting Balance Drainage (executor.yul:19-50, 114-121)
         └── Case 0 lacks caller check; untrusted balanceBefore from calldata;
             external MEV searchers drain all resting contract WETH via dummy targets
```

### Forensic Integrity Verdict

The workspace is in **🔴 INTEGRITY VIOLATION**. Multiple anti-stub, anti-facade, and anti-cheating mandates defined in `ORIGINAL_REQUEST.md` have been violated:
- **Simulation Profit Fallback Facade**: `crates/simulator/src/engine.rs` (lines 240–244) silently falls back to theoretical math sizing (`route.sizing.net_profit`) whenever the `revm` balance delta is zero.
- **Formulaic Synthetic Data in Discovery**: `crates/scanner/src/discovery.rs` synthesizes volume, liquidity, and volatility metrics using formulaic modulo arithmetic (`vol = 2_000_000.0 + (190 - k) * 120_000.0`), overwrites `top_100_pairs.json`, and loads a hardcoded list containing factory addresses as pool contracts.
- **Decoupled Self-Certifying Test Suite**: The 203 tests in `tests/Cargo.toml` do not depend on or test any workspace crate. They run against local mock implementations in `tests/common/` that assert trivial tautologies (e.g. `28 <= 30`, asserting an address is non-zero).

---

## 2. Remediation Scorecard (All 32 Items + Compiler Profile)

### Master Summary Matrix

| Category | Total Items | Verified Fixed (✅) | Partially Fixed / Flawed (⚠️) | Still Broken (❌) |
|---|:---:|:---:|:---:|:---:|
| **Critical (C1–C7)** | 7 | 4 | 2 | 1 |
| **High (H1–H8)** | 8 | 5 | 2 | 1 |
| **Medium (M1–M9)** | 9 | 7 | 2 | 0 |
| **Low (L1–L8)** | 8 | 7 | 1 | 0 |
| **Compiler Profile** | 1 | 0 | 1 | 0 |
| **TOTAL** | **33** | **23** | **8** | **2** |

---

### Detailed Item-by-Item Scorecard

#### [C1] Live On-Chain Pool Discovery via Multicall3
- **Status**: `⚠️ PARTIALLY FIXED / FLAWED`
- **Location**: `src/main.rs:144-170`, `crates/scanner/src/discovery.rs:800-1038, 1143, 1300-1343`, `top_100_pairs.json`
- **Code Citation**:
  ```rust
  // crates/scanner/src/discovery.rs:1300-1305
  let k = candidate_pairs.len();
  let vol = 2_000_000.0 + ((190 - k) as f64) * 120_000.0;
  let liq = 500_000.0 + ((190 - k) as f64) * 15_000.0;
  let sigma = 0.012 + ((k % 7) as f64) * 0.001;
  let avg_fee = 0.00175;
  ```
  ```rust
  // crates/scanner/src/discovery.rs:1143-1144
  DiscoveredPool::new(
      arbitrum::CAMELOT_V3_FACTORY, // BUG: Factory address used as pool address
      Venue::CamelotV3,
  ```
- **Technical Analysis & Evidence**:
  - **Verified Fixed**: Fake address generators (`0x{:040x}`, `i*1111`), synthetic reserves (`405 * d_eth`), and modulo price oscillation loops were removed from `src/main.rs`. Real factory calls (`getPair`, `getPool`) and Multicall3 `aggregate3` querying were implemented in `Multicall3Discovery::discover_core_pools()`.
  - **Remaining Flaws**:
    1. **Omission of `tickBitmap`**: Multicall3 state queries batch `getReserves()`, `slot0()`, and `liquidity()`, but completely omit `tickBitmap` words, violating the R1 scanner requirement.
    2. **Fabricated Metrics in `generate_top_100_pairs`**: `generate_top_100_pairs()` derives `vol`, `liq`, and `sigma` using synthetic formulas (`(190 - k) * 120_000.0`, `(k % 7) * 0.001`).
    3. **Missing Contract Pools in `top_100_pairs.json`**: The exported JSON contains only token pair addresses for Base (`pair`, `token0`, `token1`) and zero verified pool contract addresses or reserves. Arbitrum One pairs are absent.
    4. **Factory Address as Pool Address**: In `canonical_mainnet_pools`, Camelot V3 ARB/USDC (line 1143) uses `arbitrum::CAMELOT_V3_FACTORY` (`0x1a83a0433bc070782be741E533753b67f45d3b91`) instead of the deployed pool contract.

---

#### [C2] Live WebSocket Feeds for Arbitrum & Base
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/collector/src/arbitrum.rs:441-520`, `crates/collector/src/base.rs:332-450`
- **Code Citation**:
  ```rust
  // crates/collector/src/arbitrum.rs:441-450
  pub async fn connect(
      &self,
      fallback_url: Option<&str>,
  ) -> Result<tokio_tungstenite::WebSocketStream<...>, ArbitrumFeedError> {
      match connect_async(&self.feed_url).await {
          Ok((stream, _response)) => {
              tracing::info!(endpoint = %self.feed_url, "Connected to primary Arbitrum sequencer WebSocket feed");
              Ok(stream)
          }
  ```
  ```rust
  // crates/collector/src/base.rs:219-236
  pub fn build_alchemy_logs_subscribe_request() -> String {
      serde_json::json!({
          "jsonrpc": "2.0", "id": 1, "method": "eth_subscribe",
          "params": ["logs", { "topics": [[ format!("0x{:x}", TOPIC_UNIV2_SYNC), format!("0x{:x}", TOPIC_UNIV3_SWAP) ]] }]
      }).to_string()
  }
  ```
- **Technical Analysis & Evidence**:
  - `ArbitrumFeedHandler::connect()` and `run()` use `tokio-tungstenite` to connect to `wss://arb1-feed.arbitrum.io/feed` with automatic fallback to Alchemy WebSocket. Sequence numbers are deduplicated lock-free via `SequencerDeduplicator` (`compare_exchange_weak` on `AtomicU64`).
  - `BaseFlashblockHandler::connect()` and `run()` stream Base Flashblocks and spawn an Alchemy WebSocket subscription for Uniswap V2 `Sync` (`0x1c411e9a96e071241c2f21f7726b17ae89e3cab4c78be50e062b03a9fffbbad1`) and Uniswap V3 `Swap` (`0xc42079f94a6350d7e6235f29174924f928cc2ac818eb64fed8004e115fbcca67`).

---

#### [C3] Uniswap V2 `swap()` Function Selector
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/collector/src/arbitrum.rs:312`, `contracts/executor.yul:81, 98`, `crates/executor/src/bytecode.rs:23, 25, 49, 51`
- **Code Citation**:
  ```rust
  // crates/collector/src/arbitrum.rs:312
  pub const UNIV2_SWAP: [u8; 4] = [0x02, 0x2c, 0x0d, 0x9f];
  ```
  ```yul
  // contracts/executor.yul:81, 98
  mstore(0x00, 0x022c0d9f00000000000000000000000000000000000000000000000000000000)
  ```
  ```rust
  // crates/executor/src/bytecode.rs:23, 49
  // "63022c0d9f" (PUSH4 0x022c0d9f)
  ```
- **Technical Analysis & Evidence**:
  - Exact Keccak-256 selector of `swap(uint256,uint256,address,bytes)` is verified as `0x022c0d9f`.
  - All occurrences in `arbitrum.rs`, `executor.yul`, and precompiled bytecode (`EXECUTOR_INIT_CODE` and `EXECUTOR_RUNTIME_BYTECODE`) match `0x022c0d9f`. The flawed selector `0x022c089f` exists only in regression test assertions verifying rejection.

---

#### [C4] Uniswap V3 `PoolCreated` Topic0 Hash
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/scanner/src/discovery.rs:17-18`
- **Code Citation**:
  ```rust
  // crates/scanner/src/discovery.rs:17-18
  pub const UNI_V3_POOL_CREATED_TOPIC0: B256 =
      b256!("783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118");
  ```
- **Technical Analysis & Evidence**:
  - The Keccak-256 hash of `PoolCreated(address,address,uint24,int24,address)` is `0x783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118`.
  - The constant matches verbatim. The corrupted hash `783cca1c04124a23a8e62df2694056dd2e0035e227d2b40730d586ab66724ec4` appears solely in adversarial unit test `challenger_m_r4_adversarial.rs:38`.

---

#### [C5] Real Arbitrum Feed JSON/Base64 Unwrapping + `alloy-rlp` Decoding
- **Status**: `⚠️ PARTIALLY FIXED / FLAWED`
- **Location**: `crates/collector/src/arbitrum.rs:148-229, 232-278, 382-436`
- **Code Citation**:
  ```rust
  // crates/collector/src/arbitrum.rs:201-229
  // Resilient fallback for minimal mock test fixtures
  let first_byte = payload[0];
  let (tx_type, inner_payload) = if first_byte >= 0xc0 {
      (ArbitrumTxType::Legacy, payload)
  } else {
      (ArbitrumTxType::from_byte(first_byte), &payload[1..])
  };

  let mut to = None;
  if !inner_payload.is_empty() && inner_payload[0] >= 0xc0 && inner_payload.len() >= 21 {
      for i in 0..=inner_payload.len().saturating_sub(21) {
          if inner_payload[i] == 0x94 {
              to = Some(Address::from_slice(&inner_payload[i + 1..i + 21]));
              break;
          }
      }
  }

  Ok(DecodedTx {
      tx_type,
      tx_hash,
      to,
      value: U256::ZERO,
      gas_limit: 0,
      max_priority_fee_per_gas: 0,
      max_fee_per_gas: 0,
      calldata: Bytes::copy_from_slice(inner_payload),
  })
  ```
- **Technical Analysis & Evidence**:
  - **Verified Fixed**: `process_feed_message()` properly unpacks JSON envelopes (`BroadcastFeedMessage` and `SequencerFeedItem`), decodes Base64 `l2msg`, recursively unpacks batch kinds `6` and `3`, and invokes `TxEnvelope::decode_2718` to extract `to`, `value`, `gas_limit`, `max_fee_per_gas`, and `calldata`.
  - **Remaining Flaw**: Remnant heuristic `0x94` byte scanning was not removed. It remains as an active fallback stub in `FastTxDecoder::decode` (lines 201–229). When invoked on non-standard envelopes, it scans for byte `0x94` to infer recipient address and emits a dummy transaction with `gas_limit: 0` and unparsed calldata.

---

#### [C6] Simulation Profit Validation via `revm` Balance Deltas
- **Status**: `❌ STILL BROKEN`
- **Location**: `crates/simulator/src/engine.rs:230-258`
- **Code Citation**:
  ```rust
  // crates/simulator/src/engine.rs:239-247
  let balance_delta = post_balance.saturating_sub(trade_size);
  let actual_profit = if balance_delta > U256::ZERO {
      balance_delta
  } else {
      route.sizing.net_profit
  };

  let net_yield = I256::from_raw(actual_profit) - I256::from_raw(gas_cost_wei);
  ```
  ```rust
  // crates/simulator/src/engine.rs:248-258
  Ok(SimulationOutcome {
      is_success: true,
      gas_used,
      gas_cost_wei,
      actual_profit,
      net_yield,
      calldata: calldata_124,
      fast_revert_tripped: false,
      logs,
      return_data: output.into_data(),
  })
  ```
- **Technical Analysis & Evidence**:
  - **Critical Stub Fallback**: Requirement C6 mandated: *"Delete `let actual_profit = route.sizing.net_profit;`... reject any trade where revm execution profit falls below gas_cost."*
  - Line 243 falls back to `route.sizing.net_profit` whenever `balance_delta <= 0`. If `revm` execution yields zero balance change (because mock contracts do not execute real balance transfers), the engine mirrors theoretical math sizing as genuine profit.
  - **Unprofitable Trades Allowed**: Lines 248–258 return `Ok(SimulationOutcome)` unconditionally upon `ExecutionResult::Success`, even if `net_yield < 0` or `actual_profit < gas_cost_wei`. The engine fails to reject unprofitable trades.
  - **Hardcoded 20 Gwei Gas Cost**: Line 228 hardcodes `gas_cost_wei = U256::from(gas_used * 20_000_000_000u64)` (20 Gwei).

---

#### [C7] Parameterized WETH & Owner Address + Live Broadcaster
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/executor/src/bytecode.rs:79-118`, `crates/strategy/src/pipeline.rs:216-258`
- **Code Citation**:
  ```rust
  // crates/executor/src/bytecode.rs:79-97
  pub fn build_executor_bytecode(chain_id: u64, owner: Address) -> Bytes {
      let weth = match chain_id {
          base::CHAIN_ID => base::WETH,
          _ => arbitrum::WETH,
      };
      let mut runtime = EXECUTOR_RUNTIME_BYTECODE.to_vec();
      runtime[27..47].copy_from_slice(owner.as_slice());
      runtime[73..93].copy_from_slice(owner.as_slice());
      runtime[321..341].copy_from_slice(weth.as_slice());
      runtime[503..523].copy_from_slice(weth.as_slice());
      runtime[599..619].copy_from_slice(weth.as_slice());
      Bytes::from(runtime)
  }
  ```
- **Technical Analysis & Evidence**:
  - `build_executor_bytecode` and `build_executor_init_code` dynamically patch the 20-byte WETH address (`0x82aF49447D8a07e3bd95BD0d56f35241523fBab1` on Arbitrum, `0x4200000000000000000000000000000000000006` on Base) and the owner address into verified byte offsets without opcode alignment drift.
  - In `pipeline.rs` lines 216–258, live raw transaction broadcasting via `eth_sendRawTransaction` is implemented with JSON-RPC error handling. Default execution mode is Shadow (non-broadcasting).

---

#### [H1] Dynamic Gas & Priority Fee Pricing
- **Status**: `❌ STILL BROKEN`
- **Location**: `crates/strategy/src/pipeline.rs:171-189`, `crates/simulator/src/engine.rs:228`
- **Code Citation**:
  ```rust
  // crates/strategy/src/pipeline.rs:173-186
  let base_fee_per_gas = 100_000_000u128; // 0.1 Gwei baseline on Arbitrum/Base
  let min_priority_fee = 50_000_000u128;  // 0.05 Gwei minimum
  let max_priority_cap = 5_000_000_000u128; // 5 Gwei cap

  let priority_fee = if sim_outcome.actual_profit > U256::ZERO
      && best_candidate.borrow_token == flash_core::constants::arbitrum::WETH
  {
      let profit_wei: u128 = sim_outcome.actual_profit.to::<u128>();
      let total_bribe = profit_wei / 5; // 20% builder/sequencer bribe
      let per_gas_bribe = total_bribe / (gas_limit as u128);
      per_gas_bribe.clamp(min_priority_fee, max_priority_cap)
  } else {
      min_priority_fee
  };
  ```
  ```rust
  // crates/simulator/src/engine.rs:228
  let gas_cost_wei = U256::from(gas_used * 20_000_000_000u64); // 20 gwei estimate
  ```
- **Technical Analysis & Evidence**:
  - H1 mandated: *"eliminate hardcoded 2 Gwei / 20 Gwei. Poll live eth_gasPrice / eth_maxPriorityFeePerGas asynchronously, dynamically bidding a configurable percentage of expected net profit."*
  - Zero live RPC/WebSocket gas polling is implemented. `base_fee_per_gas` is hardcoded to `100_000_000u128` (0.1 Gwei) in `pipeline.rs`, and simulation gas cost in `engine.rs` is hardcoded to 20 Gwei (`20_000_000_000u64`).
  - **Base Chain Flaw**: Line 178 tests `best_candidate.borrow_token == arbitrum::WETH`. On Base (`Chain::Base`), `borrow_token` is `0x4200000000000000000000000000000000000006`, causing the condition to always evaluate `false`. Hence, Base transactions NEVER compute dynamic priority bribes and always submit the absolute minimum 0.05 Gwei fee.

---

#### [H2] Simulation Gas Limit
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/simulator/src/engine.rs:204`
- **Code Citation**:
  ```rust
  // crates/simulator/src/engine.rs:204
  gas_limit: 1_200_000,
  ```
- **Technical Analysis & Evidence**:
  - Simulation gas limit was increased from 500,000 to 1,200,000 to support multi-hop concentrated liquidity cycles.

---

#### [H3] Directional Reserve Packing
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/simulator/src/engine.rs:154-165`
- **Code Citation**:
  ```rust
  // crates/simulator/src/engine.rs:154-160
  // H3: Pack reserves into slot 8 directionally checking hop.zero_for_one:
  let (r0, r1) = if hop.zero_for_one {
      (hop.reserve_in, hop.reserve_out)
  } else {
      (hop.reserve_out, hop.reserve_in)
  };
  let packed_reserves: U256 = U256::from(r0) | (U256::from(r1) << 112);
  ```
- **Technical Analysis & Evidence**:
  - Reserves are correctly assigned to `(r0, r1)` conditionally based on `hop.zero_for_one`, preventing inverted reserve packing into storage slot 8.

---

#### [H4] Synchronized u128 Trade Size Widening Across Rust AND Yul
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `contracts/executor.yul:22, 51-60`, `crates/executor/src/calldata.rs:28-39, 51-82`, `crates/executor/src/bytecode.rs:18, 43`
- **Code Citation**:
  ```yul
  // contracts/executor.yul:52-60
  let wethAmountToFirstMarket := shr(0x80, calldataload(dataOffset))
  let balanceBefore           := shr(0x80, calldataload(add(dataOffset, 0x10)))
  let gasCost                 := shr(0x80, calldataload(add(dataOffset, 0x20)))
  let target1                 := shr(0x60, calldataload(add(dataOffset, 0x30)))
  let target2                 := shr(0x60, calldataload(add(dataOffset, 0x44)))
  let token0_amountOut        := shr(0x80, calldataload(add(dataOffset, 0x58)))
  let token0_amountOutIndex   := shr(0xf0, calldataload(add(dataOffset, 0x68)))
  let token1_amountOut        := shr(0x80, calldataload(add(dataOffset, 0x6a)))
  let token1_amountOutIndex   := shr(0xf0, calldataload(add(dataOffset, 0x7a)))
  ```
  ```rust
  // crates/executor/src/calldata.rs:51-81
  pub fn encode_124byte_calldata(params: &ExecutionParams) -> [u8; 124] { ... }
  ```
- **Technical Analysis & Evidence**:
  - Calldata payload was widened from 100 bytes to 124 bytes (`0x7c`), expanding `wethAmountToFirstMarket`, `balanceBefore`, and `gasCost` to 16 bytes each.
  - Bytecode in `crates/executor/src/bytecode.rs` checks `calldatasize >= 0x7c` and matches all offsets exactly.
  - End-to-end 25 ETH trade simulation without `u64` truncation is verified in integration test `crates/simulator/tests/revm_25eth_trade_test.rs`.

---

#### [H5] Quoter Clamped Output Adjustment
- **Status**: `⚠️ PARTIALLY FIXED / FLAWED`
- **Location**: `crates/simulator/src/quoter.rs:342-359`, `contracts/UniV3Quoter.yul:118-121, 134-137`
- **Code Citation**:
  ```rust
  // crates/simulator/src/quoter.rs:343-359
  if !sqrt_price_limit_x96.is_zero() {
      if zero_for_one && sqrt_price_x96_after < sqrt_price_limit_x96 {
          sqrt_price_x96_after = sqrt_price_limit_x96;
          let delta_p = pool.sqrt_price_x96.saturating_sub(sqrt_price_limit_x96);
          if let Some(out) = mul_div_u256(U256::from(pool.liquidity), delta_p, U256::from(1) << 96) {
              amount_out = out;
          }
      ...
  ```
  ```yul
  // contracts/UniV3Quoter.yul:118-121
  if and(gt(sqrtPriceLimitX96, 0), lt(sqrtPriceX96After, sqrtPriceLimitX96)) {
      sqrtPriceX96After := sqrtPriceLimitX96
  }
  ```
- **Technical Analysis & Evidence**:
  - The Rust analytical solver in `quoter.rs` recomputes `amount_out` up to the limit boundary.
  - **Yul Discrepancy**: In `contracts/UniV3Quoter.yul`, when `sqrtPriceX96After` hits `sqrtPriceLimitX96`, it clamps `sqrtPriceX96After := sqrtPriceLimitX96` but does NOT adjust `amountOut`. The Yul quoter returns the full unclamped `amountOut`. Furthermore, `amount_in_consumed` is not returned.

---

#### [H6] Chain-Aware Flash Loan Fees
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/executor/src/routing.rs:88-96`
- **Code Citation**:
  ```rust
  // crates/executor/src/routing.rs:88-96
  pub fn fee_pips(&self, chain_id: u64) -> u32 {
      match self {
          Self::MorphoBlue | Self::BalancerV3 | Self::UniswapV4 => 0,
          Self::AaveV3 => match chain_id {
              base::CHAIN_ID => 900,
              _ => 500, // arbitrum::CHAIN_ID (42161) and default
          },
      }
  }
  ```
- **Technical Analysis & Evidence**:
  - `fee_pips(&self, chain_id: u64)` returns 0 for zero-fee singletons, 500 PIPs (0.05%) for Aave on Arbitrum (`42161`), and 900 PIPs (0.09%) on Base (`8453`).

---

#### [H7] Atomic Graph Locking
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/scanner/src/graph.rs:138-160`
- **Code Citation**:
  ```rust
  // crates/scanner/src/graph.rs:138-160
  // Step 2: Atomic Check-and-Insert under continuous adj write lock (Fix H7)
  {
      let mut adj = self.adj.write();
      if self.pool_to_edges.contains_key(&pool.id) {
          return; // Double-check under write lock eliminates TOCTOU
      }

      let mut u_edges = Vec::with_capacity(adj[u].len() + 1);
      u_edges.extend_from_slice(&adj[u]);
      let edge_idx_u = u_edges.len();
      u_edges.push(edge_0_to_1);
      adj[u] = Arc::from(u_edges);

      let mut v_edges = Vec::with_capacity(adj[v].len() + 1);
      v_edges.extend_from_slice(&adj[v]);
      let edge_idx_v = v_edges.len();
      v_edges.push(edge_1_to_0);
      adj[v] = Arc::from(v_edges);

      // Step 3: Insert into pool_to_edges INSIDE the critical section
      self.pool_to_edges
          .insert(pool.id, [(u, edge_idx_u), (v, edge_idx_v)]);
  }
  ```
- **Technical Analysis & Evidence**:
  - Write lock on `adj` is continuously held while performing duplicate check, edge buffer allocation, and insertion into `pool_to_edges`.

---

#### [H8] Real Sizing Latency Measurement
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/strategy/src/pipeline.rs:136-139`
- **Code Citation**:
  ```rust
  // crates/strategy/src/pipeline.rs:136-139
  let t_sizing = Instant::now();
  let best_candidate = candidates.into_iter().max_by_key(|c| c.net_profit).unwrap();
  // H8: Measure real elapsed duration of candidate selection & sizing math (no fake constant 15)
  let sizing_math_us = t_sizing.elapsed().as_micros().max(1) as u64;
  ```
- **Technical Analysis & Evidence**:
  - Hardcoded `let sizing_math_us = 15;` was eliminated and replaced with genuine `Instant::now()` elapsed microsecond measurement.

---

#### [M1 & M2] Honeypot Overflow Semantics
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/simulator/src/honeypot.rs:21-46`
- **Code Citation**:
  ```rust
  // crates/simulator/src/honeypot.rs:24, 33-35
  if report.storage_delta.checked_mul(U256::from(5)).map_or(false, |val| val <= report.emitted_amount) { ... }

  let ten_delta = U512::from(report.storage_delta) * U512::from(10);
  let nine_emitted = U512::from(report.emitted_amount) * U512::from(9);
  if ten_delta < nine_emitted { ... }
  ```
- **Technical Analysis & Evidence**:
  - Threshold calculations in `crates/simulator/src/honeypot.rs` were promoted to `U512` for Listeria checks and `checked_mul` with `map_or(false)` for Salmonella, preventing `Option` ordering panic/false positives.

---

#### [M3 & M4] O(1) Telemetry Ring Buffer
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/strategy/src/telemetry.rs:7, 89, 118-123, 125-137`
- **Code Citation**:
  ```rust
  // crates/strategy/src/telemetry.rs:118-122
  let mut latencies = self.recent_e2e_latencies.write();
  if latencies.len() >= 1000 {
      latencies.pop_front();
  }
  latencies.push_back(telemetry.total_e2e_latency_us());
  ```
  ```rust
  // crates/strategy/src/telemetry.rs:125-134
  pub fn p99_latency_us(&self) -> u64 {
      let latencies: Vec<u64> = {
          let guard = self.recent_e2e_latencies.read();
          if guard.is_empty() { return 0; }
          guard.iter().copied().collect()
      };
      let mut sorted = latencies;
      sorted.sort_unstable();
  ```
- **Technical Analysis & Evidence**:
  - Telemetry ring buffer uses `VecDeque<u64>` with `pop_front()` in O(1).
  - `p99_latency_us()` acquires a non-blocking `RwLock::read()`, snapshots values, releases the lock, and sorts unstable outside the critical section.

---

#### [M5 & M6] Fail-Fast Configuration
- **Status**: `⚠️ PARTIALLY FIXED / FLAWED`
- **Location**: `crates/core/src/config.rs:51-54, 89-102`, `src/main.rs:89-93`
- **Code Citation**:
  ```rust
  // crates/core/src/config.rs:97-102
  if execution_mode.is_live() && executor_contract_address == Address::ZERO {
      return Err(ConfigError::InvalidValue {
          var: "EXECUTOR_CONTRACT_ADDRESS".to_string(),
          reason: "Executor address must not be Address::ZERO in live mode".to_string(),
      });
  }
  ```
  ```rust
  // src/main.rs:89-93
  if args.live {
      warn!("============================================================");
      warn!("⚠️  LIVE NETWORK BROADCASTING ENABLED. Real capital at risk!");
      warn!("============================================================");
      config.execution_mode = ExecutionMode::Live;
  }
  ```
- **Technical Analysis & Evidence**:
  - `AppConfig::load()` returns `Result<Config, ConfigError>` and fails fast on missing `ALCHEMY_KEY`.
  - **CLI Bypass Flaw**: When the bot is launched with `--live` flag, `AppConfig::load()` is called first (when `EXECUTION_MODE` in `.env` is typically `shadow`). The check `execution_mode.is_live() && executor_contract_address == Address::ZERO` does NOT trigger. Then `src/main.rs:93` sets `config.execution_mode = ExecutionMode::Live` WITHOUT validating `executor_contract_address`. The bot proceeds in live mode with `executor_contract_address == Address::ZERO`.

---

#### [M7 & M8] O(1) Bytecode Cache
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/simulator/src/cache.rs:28-150, 173-194`
- **Code Citation**:
  ```rust
  // crates/simulator/src/cache.rs:177-182
  {
      let mem = self.memory.read();
      if let Some(code) = mem.peek(chain_id, address) {
          return Some(code);
      }
  }
  ```
- **Technical Analysis & Evidence**:
  - `MemoryLru` combines `HashMap` and a doubly-linked list (`prev`/`next`) for O(1) peek/put/eviction.
  - `get_bytecode()` takes a read lock first (`self.memory.read()`), avoiding write lock contention on cache hits.

---

#### [M9] Thread-Safe Filters
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/scanner/src/filters.rs:7, 94-99`
- **Code Citation**:
  ```rust
  // crates/scanner/src/filters.rs:94-99
  pub struct HoneypotFilter {
      blacklist: DashSet<Address>,
      fee_on_transfer: DashSet<Address>,
      rebasing_tokens: DashSet<Address>,
      verified_safe: DashSet<Address>,
  }
  ```
- **Technical Analysis & Evidence**:
  - Standard `HashSet` was replaced with concurrent `DashSet<Address>` across all filter registries.

---

#### [L1 & L2] String-to-U256 Unit Parsing
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/core/src/types.rs:205-218, 224-270`
- **Code Citation**:
  ```rust
  // crates/core/src/types.rs:208-210
  if dec == 0 {
      return s;
  }
  ```
  ```rust
  // crates/core/src/types.rs:224-270
  pub fn parse_units_str(&self, s: &str) -> U256 { ... }
  ```
- **Technical Analysis & Evidence**:
  - `format_units_str` omits trailing decimal dots when `decimals == 0`.
  - `parse_units_str` performs exact string-to-`U256` arithmetic using radix-10 integer and fractional slices, eliminating `f64` precision loss.

---

#### [L3 & L4] Inlining & Scale Constants
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/math/src/mul_div.rs:37, 54`, `crates/math/src/closed_form.rs:5`
- **Code Citation**:
  ```rust
  // crates/math/src/mul_div.rs:37, 54
  #[inline(always)]
  pub fn sqrt_u256(n: U256) -> U256 { ... }

  #[inline(always)]
  pub fn sqrt_u512(n: U512) -> U256 { ... }
  ```
  ```rust
  // crates/math/src/closed_form.rs:5
  pub use flash_core::constants::PIP_SCALE;
  ```
- **Technical Analysis & Evidence**:
  - `#[inline(always)]` is applied to both `sqrt_u256` and `sqrt_u512`. `PIP_SCALE` is imported from `flash_core::constants`.

---

#### [L5 & L6] DFS Allocation & Sorting
- **Status**: `✅ VERIFIED FIXED`
- **Location**: `crates/scanner/src/graph.rs:245`, `crates/scanner/src/dfs.rs:39-45, 178-186`
- **Code Citation**:
  ```rust
  // crates/scanner/src/graph.rs:245
  pub fn get_edges(&self, token: Address) -> Arc<[DirectedEdge]>
  ```
  ```rust
  // crates/scanner/src/dfs.rs:183-185
  // Single descending sort over entire combined set; NO redundant intermediate sorting
  all_routes.sort_by(|a, b| b.net_profit.cmp(&a.net_profit));
  ```
- **Technical Analysis & Evidence**:
  - `get_edges` returns an `Arc<[DirectedEdge]>` slice reference, eliminating per-hop heap allocations. Redundant intermediate sorting in `dfs.rs` was removed.

---

#### [L7] Access List Encoding
- **Status**: `✅ VERIFIED FIXED` (Implementation present; flawed RLP logic documented under V-10)
- **Location**: `crates/strategy/src/signers.rs:122-157, 334`
- **Code Citation**:
  ```rust
  // crates/strategy/src/signers.rs:122-127
  pub fn encode_access_list(access_list: &[(Address, Vec<B256>)], out: &mut Vec<u8>) {
      if access_list.is_empty() {
          let empty_list: [u8; 0] = [];
          empty_list.as_slice().encode(out);
          return;
      }
  ```
- **Technical Analysis & Evidence**:
  - RLP serialization for EIP-2930 / EIP-1559 access lists is implemented and invoked during EIP-1559 transaction signing. (The RLP empty list bug is evaluated under V-10).

---

#### [L8] Quoter Shift Guard
- **Status**: `⚠️ PARTIALLY FIXED / FLAWED`
- **Location**: `contracts/UniV3Quoter.yul:100`, `crates/simulator/src/quoter.rs:38-49`, `crates/simulator/tests/challenger_quoter_l8_test.rs:13-32`
- **Code Citation**:
  ```yul
  // contracts/UniV3Quoter.yul:99-102
  // Guard against 256-bit overflow on liquidity << 96
  if gt(shr(160, liquidity), 0) {
      revert(0, 0)
  }
  ```
  ```rust
  // crates/simulator/tests/challenger_quoter_l8_test.rs:20-31
  let shr_160 = [0x60, 0xa0, 0x1c];
  let has_shr_160 = QUOTER_RUNTIME_BYTECODE.windows(3).any(|w| w == shr_160);
  assert_eq!(QUOTER_RUNTIME_BYTECODE.len(), 329);
  assert!(!has_shr_160, "Compiled QUOTER_RUNTIME_BYTECODE does not contain shr(160, ...) guard!");
  ```
- **Technical Analysis & Evidence**:
  - While `contracts/UniV3Quoter.yul` was edited to add `if gt(shr(160, liquidity), 0) { revert(0, 0) }`, the compiled bytecode constant `QUOTER_RUNTIME_BYTECODE` (329 bytes) embedded in `crates/simulator/src/quoter.rs` was never recompiled.
  - The challenger test `test_l8_quoter_bytecode_inspection` confirms that `QUOTER_RUNTIME_BYTECODE` lacks the opcode sequence `[0x60, 0xa0, 0x1c]` (`shr(160, ...)`). In-memory EVM simulation executes without this guard.

---

#### [Compiler Profile] Whole-Program LTO
- **Status**: `⚠️ PARTIALLY FIXED / FLAWED`
- **Location**: root `Cargo.toml:90, 97`
- **Code Citation**:
  ```toml
  # Cargo.toml:89-94
  [profile.release]
  lto = "thin"
  codegen-units = 1
  opt-level = 3
  panic = "abort"
  ```
- **Technical Analysis & Evidence**:
  - Compiler profile instruction specified: *"In root Cargo.toml, set `lto = "fat"` for maximum whole-program optimization."*
  - Root `Cargo.toml` lines 90 and 97 specify `lto = "thin"`.

---

## 3. Categorized Vulnerability & Risk Inventory

---

### 🔴 Critical Severity Vulnerabilities (V-01 – V-10)

#### V-01: Live Broadcaster Targets Precompile 0x01
- **File**: `crates/strategy/src/pipeline.rs:196`, `crates/simulator/src/bytecode_mocks.rs:8`
- **Vulnerable Code**:
  ```rust
  // crates/strategy/src/pipeline.rs:196
  let tx_request = Eip1559TransactionRequest {
      chain_id: self.simulator.chain.id(),
      nonce: Some(lease.nonce()),
      max_priority_fee_per_gas: priority_fee,
      max_fee_per_gas,
      gas_limit,
      to: self.simulator.executor_address, // <-- Resolves to 0x000...001
      value: U256::ZERO,
      data: Bytes::copy_from_slice(&sim_outcome.calldata),
      access_list: vec![],
  };
  ```
- **Failure Mechanism & Financial Impact**:
  `SimulationEngine::new()` sets `executor_address` to `MOCK_EXECUTOR_ADDRESS` (`0x0000000000000000000000000000000000000001`, the EVM `ecrecover` precompile). In `pipeline.rs`, live broadcast transactions set `to: self.simulator.executor_address` instead of `self.config.executor_contract_address`. Every live transaction sent by the bot is addressed to the `ecrecover` precompile. The precompile fails on the input calldata, executing zero arbitrage swaps while burning 100% of transaction fees from signer EOAs.

---

#### V-02: Smart Contract Case 0 Resting Balance Drain
- **File**: `contracts/executor.yul:19-50, 114-121`
- **Vulnerable Code**:
  ```yul
  // contracts/executor.yul:19-27, 53
  switch callvalue()
  case 0 {
      let csize := calldatasize()
      if lt(csize, 0x7c) { revert(0, 0) }
      let selector := shr(0xe0, calldataload(0x00))
      let dataOffset := 0x00
      ...
      let balanceBefore := shr(0x80, calldataload(add(dataOffset, 0x10)))
  ```
- **Failure Mechanism & Financial Impact**:
  `case 0` contains zero caller access control (`caller() == OWNER`). Furthermore, `balanceBefore` is trusted directly from calldata rather than measured dynamically via `balanceOf(address(this))`. An external attacker can invoke `case 0` with `balanceBefore = 0`, `gasCost = 0`, and a dummy `target1` contract. The executor transfers its sitting WETH to `target1`, observes `balanceAfter > 0`, and completes with `stop()`. Any accumulated profits or resting balances in the contract are instantly drained by external searchers.

---

#### V-03: Balancer V3 & Uniswap V4 Missing Repayment Logic
- **File**: `contracts/executor.yul:37-40, 108, 114-121`
- **Vulnerable Code**:
  ```yul
  // contracts/executor.yul:37-40
  case 0x98dd202a {
      dataOffset := 0x44
  }
  ...
  // Step 4: Flash Loan Repayment only handles Morpho Blue:
  if eq(selector, 0x3a48e7e1) {
      ...
  }
  // No branch for 0x98dd202a!
  ```
- **Failure Mechanism & Financial Impact**:
  The `unlockCallback` (`0x98dd202a`) for Balancer V3 Vault and Uniswap V4 PoolManager contains zero repayment or settlement logic. Step 3 deposits swap output to `address(this)`, and Step 4 skips repayment. When control returns to the Balancer Vault or Uniswap PoolManager, the vault checks whether the flash loan debt was settled. Because 0 tokens were repaid, the Vault reverts on-chain with `VaultDebtNotZero`. 100% of Balancer V3 and Uniswap V4 flash loan transactions revert on-chain.

---

#### V-04: Incomplete 3-Hop Triangular Route Execution / Dropped 3rd Hop
- **File**: `crates/simulator/src/engine.rs:184-197`, `crates/executor/src/calldata.rs:27-39`, `contracts/executor.yul:80-112`
- **Vulnerable Code**:
  ```rust
  // crates/simulator/src/engine.rs:184-197
  let hop1 = &route.hops[0];
  let hop2 = &route.hops[1];

  let params = ExecutionParams {
      weth_amount_to_first_market: trade_size.to::<u128>(),
      balance_before: 0,
      gas_cost: 50_000,
      target1: hop1.pool_id,
      target2: hop2.pool_id,
      token0_amount_out: route.sizing.expected_intermediate.to::<u128>(),
      token0_amount_out_index: if hop1.zero_for_one { 1 } else { 0 },
      token1_amount_out: route.sizing.expected_output.to::<u128>(),
      token1_amount_out_index: if hop2.zero_for_one { 1 } else { 0 },
  };
  ```
- **Failure Mechanism & Financial Impact**:
  `dfs.rs` routinely discovers 3-hop triangular cycles (`A -> B -> C -> A`). However, `engine.rs` only unpacks `hop1` and `hop2`, dropping `hop3`. Neither `calldata.rs` nor `executor.yul` supports `target3`. Pool 2 is instructed to output Token C directly to the executor, which expects WETH. The contract queries its WETH balance, finds 0, and reverts on-chain. 100% of 3-hop triangular routes fail on-chain.

---

#### V-05: Simulation Profit Fallback Facade
- **File**: `crates/simulator/src/engine.rs:240-244`
- **Vulnerable Code**:
  ```rust
  // crates/simulator/src/engine.rs:239-245
  let balance_delta = post_balance.saturating_sub(trade_size);
  let actual_profit = if balance_delta > U256::ZERO {
      balance_delta
  } else {
      route.sizing.net_profit // <-- Fallback facade violates C6 and R1
  };
  ```
- **Failure Mechanism & Financial Impact**:
  When `revm` simulates trades against mock pool contracts, no actual token balances are transferred, resulting in `balance_delta == 0`. Instead of failing, `engine.rs` substitutes the theoretical math profit `route.sizing.net_profit`. This bypasses empirical EVM verification, providing false confidence in broken, unexecutable, or unprofitable routes.

---

#### V-06: Absent L1 Data Fee Accounting
- **File**: `crates/simulator/src/engine.rs:228`, `crates/strategy/src/pipeline.rs:169-189`
- **Vulnerable Code**:
  ```rust
  // crates/simulator/src/engine.rs:246
  let net_yield = I256::from_raw(actual_profit) - I256::from_raw(gas_cost_wei);
  ```
- **Failure Mechanism & Financial Impact**:
  Arbitrum Nitro charges L1 calldata posting fees via `ArbGasInfo` (`0x6C`), and Base OP-Stack charges via `GasPriceOracle` (`0x4200...000F`). Neither precompile is queried. When Ethereum mainnet base fees spike, L1 calldata posting costs range from $0.50 to $3.50 per transaction. The bot executes micro-trades yielding $0.20 nominal L2 profit, losing $0.30 to $3.30 net on every transaction.

---

#### V-07: Out-of-Gas Reverts from Pool Stub Simulation Gas Cap
- **File**: `crates/strategy/src/pipeline.rs:169`, `crates/simulator/src/bytecode_mocks.rs:19-25`
- **Vulnerable Code**:
  ```rust
  // crates/strategy/src/pipeline.rs:169
  let gas_limit = sim_outcome.gas_used.max(100_000);
  ```
- **Failure Mechanism & Financial Impact**:
  `revm` simulates against `mock_v2_pair_bytecode`, whose `swap()` function executes an opcode `STOP` consuming only ~250 gas. Simulated gas for 2 hops is only ~45,000 gas, so `pipeline.rs` sets `gas_limit = 100,000`. Real on-chain multi-hop swaps require 180,000–240,000 gas. Every mined transaction runs out of gas (`OutOfGas`), forfeiting 100% of gas fees to the block builder with zero state execution.

---

#### V-08: Zero Startup Nonce Synchronization
- **File**: `crates/strategy/src/signers.rs:47`, `src/main.rs:125`
- **Vulnerable Code**:
  ```rust
  // crates/strategy/src/signers.rs:47
  nonce: AtomicU64::new(0),
  ```
- **Failure Mechanism & Financial Impact**:
  Signers initialize their local nonces to 0 without querying `eth_getTransactionCount` via RPC. If a funded EOA has any prior transaction history on Arbitrum or Base, its on-chain nonce is $N \ge 1$. The sequencer immediately rejects all transactions from that EOA with `{"code": -32000, "message": "nonce too low"}`.

---

#### V-09: Permanent Signer Bricking on Broadcast Errors
- **File**: `crates/strategy/src/signers.rs:244`, `crates/strategy/src/pipeline.rs:229-258`
- **Vulnerable Code**:
  ```rust
  // crates/strategy/src/signers.rs:244
  let nonce = signer.reserve_nonce(); // Atomically increments fetch_add(1)
  ```
- **Failure Mechanism & Financial Impact**:
  The signer's nonce is incremented before network transmission. If an RPC timeout, rate limit (HTTP 429), or mempool drop occurs, the transaction is never mined, and the on-chain nonce remains $N$. The signer's local nonce is now $N+1$. Every subsequent transaction signed by this EOA will be rejected or held indefinitely due to an unrecoverable nonce gap, permanently bricking the signer account.

---

#### V-10: Corrupted EIP-1559 Access List RLP Encoding
- **File**: `crates/strategy/src/signers.rs:122-126, 333-334`
- **Vulnerable Code**:
  ```rust
  // crates/strategy/src/signers.rs:122-126
  pub fn encode_access_list(access_list: &[(Address, Vec<B256>)], out: &mut Vec<u8>) {
      if access_list.is_empty() {
          let empty_list: [u8; 0] = [];
          empty_list.as_slice().encode(out); // Encodes as byte string (0x80) instead of list (0xc0)
          return;
      }
  ```
- **Failure Mechanism & Financial Impact**:
  In EIP-1559, an access list is strictly an RLP List. An empty access list must be encoded as `0xc0`. Encoding `empty_list.as_slice()` encodes an empty byte string (`0x80`). Standard EVM nodes (Nitro, OP-Geth, Reth) reject the envelope immediately with: `rlp: expected list, got byte string for AccessList`. 100% of signed EIP-1559 transactions fail node deserialization.

---

### 🟠 High Severity Vulnerabilities (V-11 – V-20)

#### V-11: Base-Token vs ETH Gas Decimal Normalization Mismatch
- **File**: `crates/simulator/src/engine.rs:246`, `contracts/executor.yul:13, 76, 127`
- **Vulnerable Code**:
  ```rust
  // crates/simulator/src/engine.rs:246
  let net_yield = I256::from_raw(actual_profit) - I256::from_raw(gas_cost_wei);
  ```
- **Failure Mechanism & Financial Impact**:
  18-decimal ETH gas wei is subtracted directly from `actual_profit`. If borrowing 6-decimal USDC, 100 USDC profit is `100_000_000` units, while 150,000 gas @ 20 Gwei is `3,000_000_000_000_000` wei. The resulting `net_yield` is `-2,999,999,900,000,000`, causing the simulation to reject highly profitable non-WETH arbitrage trades. Furthermore, `executor.yul` hardcodes WETH transfers and balance queries, failing on all non-WETH routes.

---

#### V-12: Hardcoded Gas Prices Violating H1
- **File**: `crates/simulator/src/engine.rs:228`, `crates/strategy/src/pipeline.rs:173`
- **Vulnerable Code**:
  ```rust
  // crates/simulator/src/engine.rs:228
  let gas_cost_wei = U256::from(gas_used * 20_000_000_000u64); // 20 Gwei estimate
  // crates/strategy/src/pipeline.rs:173
  let base_fee_per_gas = 100_000_000u128; // 0.1 Gwei baseline
  ```
- **Failure Mechanism & Financial Impact**:
  Violates Requirement H1. Live base fees are not polled. The 20 Gwei estimate in `engine.rs` inflates gas costs by 200x–2000x over actual L2 fees (0.01–0.1 Gwei), discarding viable micro-arbitrage opportunities.

---

#### V-13: Base Chain Dynamic Fee Bidding Disablement
- **File**: `crates/strategy/src/pipeline.rs:177-179`
- **Vulnerable Code**:
  ```rust
  let priority_fee = if sim_outcome.actual_profit > U256::ZERO
      && best_candidate.borrow_token == flash_core::constants::arbitrum::WETH
  { ... }
  ```
- **Failure Mechanism & Financial Impact**:
  The priority fee calculation checks `best_candidate.borrow_token == arbitrum::WETH`. On Base, the borrow token is Base WETH (`0x4200...0006`), so the condition is always false. The bot submits the absolute minimum priority fee (0.05 Gwei), losing 100% of competitive Base Flashblocks to rival MEV bots.

---

#### V-14: Non-Functional Fast-Revert Guard in Yul
- **File**: `contracts/executor.yul:63-70`
- **Vulnerable Code**:
  ```yul
  let currentReserve0 := mload(0x20)
  let currentReserve1 := mload(0x40)
  if iszero(currentReserve0) { revert(0, 0) }
  ```
- **Failure Mechanism & Financial Impact**:
  The contract queries `getReserves()` but only checks `iszero(currentReserve0)`. It never compares actual reserves against expected reserves from calldata (expected reserves are omitted from the 124-byte layout). When front-run by competing bots, the transaction executes full swaps and reverts only at the profit check, wasting full execution gas.

---

#### V-15: SimulationEngine Deploys Unparameterized Bytecode on Base
- **File**: `crates/simulator/src/engine.rs:46-57`
- **Vulnerable Code**:
  ```rust
  // crates/simulator/src/engine.rs:46
  let executor_code = executor_runtime_bytecode(); // Ignores chain_id and owner
  ```
- **Failure Mechanism & Financial Impact**:
  `SimulationEngine::new()` deploys static `EXECUTOR_RUNTIME_BYTECODE`, which contains Arbitrum WETH (`0x82aF...`) and dummy owner (`0x1111...`). When running simulations for Base, the executor invokes the Arbitrum WETH address, causing simulation state desynchronization.

---

#### V-16: UniV3Quoter Never Deployed in SimulationEngine
- **File**: `crates/simulator/src/engine.rs:36-78`, `crates/simulator/src/quoter.rs:25-49`
- **Failure Mechanism & Financial Impact**:
  `SimulationEngine::new()` never deploys `QUOTER_RUNTIME_BYTECODE` into `base_db`. All hops are simulated as V2 pools with storage slot 8 reserves. Furthermore, `executor.yul` only contains V2 swap selector `0x022c0d9f`. Any route involving Uniswap V3 or Slipstream fails immediately on-chain.

---

#### V-17: Multicall3 Index Out-of-Bounds Panic
- **File**: `crates/scanner/src/discovery.rs:929-936`
- **Vulnerable Code**:
  ```rust
  for (idx, res) in results.into_iter().enumerate() {
      if res.success {
          if let Some(pool_addr) = decode_address_word(&res.return_data) {
              resolved.push(ResolvedPoolMeta {
                  address: pool_addr,
                  query: queries[idx].clone(), // <-- UNCHECKED SLICE INDEXING
              });
  ```
- **Failure Mechanism & Financial Impact**:
  If a malformed, malicious, or proxy RPC endpoint returns an array with more items than `queries.len()`, `queries[idx]` panics with an out-of-bounds error. Under `panic = "abort"`, this instantly terminates the bot process during discovery.

---

#### V-18: Vanity Miner Out-of-Bounds Slice Panic
- **File**: `crates/executor/src/vanity.rs:415`
- **Vulnerable Code**:
  ```rust
  n => addr_slice[..n].iter().all(|&b| b == 0),
  ```
- **Failure Mechanism & Financial Impact**:
  `addr_slice` has length 20. If `has_leading_zero_bytes` is called with `target_zero_bytes > 20`, slicing `addr_slice[..n]` panics with an index out-of-range error, terminating the process under `panic = "abort"`.

---

#### V-19: Sequential Pipeline DoS & Permanent Kill Switch Trip on 10,000 tx/s
- **File**: `src/main.rs:265-334`, `crates/strategy/src/pipeline.rs:114-118`
- **Failure Mechanism & Financial Impact**:
  `src/main.rs` processes market updates sequentially on a single Tokio task. When volatility surges to 10,000 tx/s, candidate simulations cause the single consumer to lag behind the 4,096-item broadcast buffer. Messages experience queueing delay, causing `now_ms.saturating_sub(update_timestamp_ms)` to exceed the 100ms feed lag threshold. The Kill Switch trips and latches permanently, taking the bot offline during peak arbitrage opportunities.

---

#### V-20: Decoupled Self-Certifying Test Suite
- **File**: `tests/Cargo.toml`, `tests/tier1_feature_tests/*.rs`
- **Failure Mechanism & Financial Impact**:
  `tests/Cargo.toml` has no dependencies on workspace crates. The 203 tests run against mock copies in `tests/common/` that assert trivial tautologies (e.g., `28 <= 30`), providing zero verification of the production codebase.

---

### 🟡 Medium Severity Vulnerabilities (V-21 – V-26)

#### V-21: Heuristic `0x94` Byte Scanning Fallback in FastTxDecoder
- **File**: `crates/collector/src/arbitrum.rs:201-229`
- **Failure Mechanism**: When `TxEnvelope::decode_2718` fails, the decoder falls back to scanning for byte `0x94` to infer recipient addresses, producing dummy transactions with `gas_limit: 0`.

#### V-22: Cold vs Warm Storage SLOAD Underestimation
- **File**: `crates/simulator/src/engine.rs:179-217`
- **Failure Mechanism**: Simulation against empty mock contracts bypasses 14 cold SLOADs and 6 cold SSTOREs, underestimating real execution gas by over 60,000 gas.

#### V-23: Missing Signature Recovery Verification
- **File**: `crates/strategy/src/signers.rs:350-395`
- **Failure Mechanism**: `sign_transaction` signs payloads but never executes an `ecrecover` sanity check to confirm the recovered address matches `signer.address` before broadcast.

#### V-24: On-Chain Gas Cost Guard Hardcoded to 50,000 wei
- **File**: `crates/simulator/src/engine.rs:190`, `contracts/executor.yul:136`
- **Failure Mechanism**: Calldata `gas_cost` is hardcoded to 50,000 wei ($5 \times 10^{-14}$ ETH). The contract guard `lt(wethProfit, gasCost)` fails to protect against real transaction gas costs ($0.05–$0.50).

#### V-25: Stale Quoter Bytecode Lacking Shift Guard
- **File**: `contracts/UniV3Quoter.yul:100`, `crates/simulator/src/quoter.rs:38-49`
- **Failure Mechanism**: `UniV3Quoter.yul` was patched with a shift guard (`shr(160, liquidity)`), but `QUOTER_RUNTIME_BYTECODE` in `quoter.rs` was never recompiled, leaving in-memory EVM simulations unguarded.

#### V-26: CLI `--live` Bypasses Zero Address Fail-Fast
- **File**: `src/main.rs:89-93`, `crates/core/src/config.rs:97-102`
- **Failure Mechanism**: Passing `--live` sets `config.execution_mode = ExecutionMode::Live` after configuration loading, bypassing the `executor_contract_address != Address::ZERO` check and broadcasting to `0x000...`.

---

### 🟢 Low Severity Vulnerabilities (V-27 – V-29)

#### V-27: Obsolete 100-byte Calldata Encoder Exported
- **File**: `crates/executor/src/calldata.rs:84-117`
- **Finding**: `encode_100byte_calldata` is exported and used in benchmarks, but `executor.yul` strictly requires 124 bytes (`0x7c`), causing immediate reverts if called.

#### V-28: Compiler Profile `lto = "thin"` instead of `fat`
- **File**: `Cargo.toml:90, 97`
- **Finding**: Root `Cargo.toml` specifies `lto = "thin"` instead of `lto = "fat"`, reducing whole-program cross-crate inlining optimization.

#### V-29: Unnecessary Heap Reallocations Under Graph Write Lock
- **File**: `crates/scanner/src/graph.rs:183-203`
- **Finding**: `update_reserves()` clones edge slices into new `Vec`s and reallocates `Arc`s under exclusive write locks, causing latency jitter for concurrent readers during market bursts.

---

## 4. Programmatic & Disassembly Verification Evidence

### 1. Calldata Layout & Bit Shift Synchronization

The codebase was migrated under remediation H4 from a 100-byte layout to a 124-byte layout (`0x7c`).

| Field | Byte Offset | Size | Rust Encoder (`encode_124byte_calldata`) | Yul Source (`contracts/executor.yul`) | Decompiled Bytecode (`bytecode.rs`) | Status |
|---|---|:---:|---|---|---|:---:|
| `wethAmountToFirstMarket` | `0x00..0x10` | 16B | `data[0..16] = be_bytes` | `shr(0x80, calldataload(dataOffset))` | `00d7: DUP2; CALLDATALOAD; PUSH1 0x80; SHR` | ✅ Valid |
| `balanceBefore` | `0x10..0x20` | 16B | `data[16..32] = be_bytes` | `shr(0x80, calldataload(add(dataOffset, 0x10)))` | `00dc: PUSH1 0x10; DUP4; ADD; CALLDATALOAD; PUSH1 0x80; SHR` | ✅ Valid |
| `gasCost` | `0x20..0x30` | 16B | `data[32..48] = be_bytes` | `shr(0x80, calldataload(add(dataOffset, 0x20)))` | `021d: PUSH1 0x20; ... ADD; CALLDATALOAD; PUSH1 0x80; SHR` | ✅ Valid |
| `target1` | `0x30..0x44` | 20B | `data[48..68] = as_slice` | `shr(0x60, calldataload(add(dataOffset, 0x30)))` | `00e5: PUSH1 0x30; DUP5; ADD; CALLDATALOAD; PUSH1 0x60; SHR` | ✅ Valid |
| `target2` | `0x44..0x58` | 20B | `data[68..88] = as_slice` | `shr(0x60, calldataload(add(dataOffset, 0x44)))` | `00ed: PUSH1 0x44; DUP6; ADD; CALLDATALOAD; PUSH1 0x60; SHR` | ✅ Valid |
| `token0_amountOut` | `0x58..0x68` | 16B | `data[88..104] = be_bytes` | `shr(0x80, calldataload(add(dataOffset, 0x58)))` | `00f5: PUSH1 0x58; DUP7; ADD; CALLDATALOAD; PUSH1 0x80; SHR` | ✅ Valid |
| `token0_amountOutIndex` | `0x68..0x6a` | 2B | `data[104..106] = be_bytes` | `shr(0xf0, calldataload(add(dataOffset, 0x68)))` | `016f: PUSH1 0x68; DUP14; ADD; CALLDATALOAD; PUSH1 0xf0; SHR` | ✅ Valid |
| `token1_amountOut` | `0x6a..0x7a` | 16B | `data[106..122] = be_bytes` | `shr(0x80, calldataload(add(dataOffset, 0x6a)))` | `00fd: PUSH1 0x6a; DUP8; ADD; CALLDATALOAD; PUSH1 0x80; SHR` | ✅ Valid |
| `token1_amountOutIndex` | `0x7a..0x7c` | 2B | `data[122..124] = be_bytes` | `shr(0xf0, calldataload(add(dataOffset, 0x7a)))` | `01ac: PUSH1 0x7a; DUP12; ADD; CALLDATALOAD; PUSH1 0xf0; SHR` | ✅ Valid |

---

### 2. Opcode Disassembly & Bytecode Offset Verification

`crates/executor/src/bytecode.rs` dynamic parameter patching slots:

| Parameter | Bytecode Array | Offset Range | Preceding Opcode | Target Data Pre-Injection | Trailing Opcode | Alignment Status |
|---|---|:---:|:---:|:---:|:---:|:---:|
| `OWNER` | Runtime | `27..47` | `0x73` (PUSH20) | `1111111111111111111111111111111111111111` | `0x33` (CALLER) | ✅ Valid PUSH20 Slot |
| `OWNER` | Runtime | `73..93` | `0x73` (PUSH20) | `1111111111111111111111111111111111111111` | `0x33` (CALLER) | ✅ Valid PUSH20 Slot |
| `WETH` | Runtime | `321..341` | `0x73` (PUSH20) | `82af49447d8a07e3bd95bd0d56f35241523fbab1` | `0x5a` (GAS) | ✅ Valid PUSH20 Slot |
| `WETH` | Runtime | `503..523` | `0x73` (PUSH20) | `82af49447d8a07e3bd95bd0d56f35241523fbab1` | `0x5a` (GAS) | ✅ Valid PUSH20 Slot |
| `WETH` | Runtime | `599..619` | `0x73` (PUSH20) | `82af49447d8a07e3bd95bd0d56f35241523fbab1` | `0x5a` (GAS) | ✅ Valid PUSH20 Slot |
| `OWNER` | Init Code | `39..59` | `0x73` (PUSH20) | `1111111111111111111111111111111111111111` | `0x33` (CALLER) | ✅ Valid PUSH20 Slot |
| `OWNER` | Init Code | `85..105` | `0x73` (PUSH20) | `1111111111111111111111111111111111111111` | `0x33` (CALLER) | ✅ Valid PUSH20 Slot |
| `WETH` | Init Code | `333..353` | `0x73` (PUSH20) | `82af49447d8a07e3bd95bd0d56f35241523fbab1` | `0x5a` (GAS) | ✅ Valid PUSH20 Slot |
| `WETH` | Init Code | `515..535` | `0x73` (PUSH20) | `82af49447d8a07e3bd95bd0d56f35241523fbab1` | `0x5a` (GAS) | ✅ Valid PUSH20 Slot |
| `WETH` | Init Code | `611..631` | `0x73` (PUSH20) | `82af49447d8a07e3bd95bd0d56f35241523fbab1` | `0x5a` (GAS) | ✅ Valid PUSH20 Slot |

---

### 3. Grep Verification Logs & Pattern Confirmation

1. **Synthetic Reserve Scan**:
   - `405 *` — Verified removed from `src/main.rs` and `discovery.rs`.
   - `{:040x}` — 0 matches across the entire workspace.
   - `iteration %` — Price oscillation loop confirmed deleted.
2. **Selector & Topic Exactness**:
   - `0x022c0d9f` — Confirmed exact Uniswap V2 selector in `arbitrum.rs`, `executor.yul`, and `bytecode.rs`.
   - `0x022c089f` — Flawed selector confirmed absent from all production code (appears only in negative test assertions).
   - `783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118` — Confirmed exact `PoolCreated` topic0 in `discovery.rs:18`.
3. **Panic & Safety Scan**:
   - 4 `.unwrap()` calls in production paths (all guarded or fixed-size array conversions).
   - 1 `.expect(...)` call in `vanity.rs:337` on thread spawning.
   - 0 unbounded channels in production.

---

### 4. Financial Loss Models & Real Trade Quantification

#### Model A: Moderate Ethereum Mainnet Base Fee (Arbitrum One)
- **Scenario Parameters**:
  - Ethereum Mainnet L1 Base Fee: 50 Gwei ($5 \times 10^{-8}$ ETH/gas)
  - Arbitrum L2 Base Fee: 0.1 Gwei ($10^{-10}$ ETH/gas)
  - Raw Transaction Size: 245 bytes (Brotli compresses to ~190 bytes)
  - L1 Calldata Posting Gas: $190 \times 16 + 800\text{ (overhead)} = 3,840\text{ L1 gas}$
- **Cost Accounting**:
  - L1 Posting Cost: $3,840 \times 50\text{ Gwei} = 0.000192\text{ ETH} \approx \$0.576$ (at ETH = $3,000)
  - L2 Execution Cost (185,000 gas): $185,000 \times 0.1\text{ Gwei} = 0.0000185\text{ ETH} \approx \$0.055$
  - Total Actual Cost: $\$0.576 + \$0.055 = \mathbf{\$0.631}$
  - Bot Modeled Cost: Modeled purely as L2 execution = $\mathbf{\$0.055}$
- **Financial Outcome**:
  - Gross Arbitrage Profit: $0.00014\text{ ETH} \approx \$0.420$
  - Bot Decision: Gross ($0.420) > Modeled Gas ($0.055) $\implies$ **DISPATCHES TRADE**
  - Actual Settlement: Gross ($0.420) - Actual Gas ($0.631) = **$-\$0.211$ NET LOSS PER TRADE**
  - Daily Loss at 300 executions: **$-\$63.30/\text{day}$ ($-\$1,899/\text{month}$)**

#### Model B: High Volatility & Blob Congestion Spike (Base OP-Stack)
- **Scenario Parameters**:
  - Ethereum Mainnet Base Fee: 120 Gwei; Blob Base Fee spikes to 45 Gwei
  - Base Fjord FastLZ compressed transaction size: 205 bytes
  - Base L1 Data Fee: $\approx 0.00075\text{ ETH} \approx \$2.25$
  - Base L2 Execution Fee: $200,000 \times 0.05\text{ Gwei} = 0.00001\text{ ETH} \approx \$0.03$
- **Financial Outcome**:
  - Gross Arbitrage Profit: $0.0005\text{ ETH} \approx \$1.50$
  - Bot Modeled Gas: $\$0.03 \implies$ Modeled Net Profit = $+\$1.47 \implies$ **DISPATCHES TRADE**
  - Actual Settlement: $+\$1.50 - \$0.03 - \$2.25 = \mathbf{-\$0.78\text{ NET LOSS}}$

#### Model C: Reverted Execution Exposure
- When a trade fast-reverts on-chain, L2 execution consumes ~28,000 gas ($\approx \$0.008$).
- **L1 calldata posting fees are non-refundable**: the sequencer deducts the full L1 posting cost ($0.50 to $2.25) regardless of whether execution succeeded or reverted.
- At 100 fast-reverts per day during volatile markets, the bot burns **$50 to $225 per day** in unmodeled L1 fees.

---

## 5. Prioritized Remediation Roadmap

---

### Phase 1: Critical Deployment Blockers (Emergency Fixes)
*These fixes are mandatory before ANY live network connection or funded test:*

1. **Fix Live Broadcaster Target (`pipeline.rs:196`)**:
   Route transactions to `self.config.executor_contract_address` in live mode rather than `self.simulator.executor_address` (`0x000...001`).
2. **Fix Access List RLP Encoding (`signers.rs:122-126`)**:
   Encode empty access lists with `Header { list: true, payload_length: 0 }.encode(out)` (`0xc0`) instead of `empty_list.as_slice().encode(out)` (`0x80`).
3. **Implement Startup Nonce Synchronization (`signers.rs`, `main.rs`)**:
   Query `eth_getTransactionCount(signer.address, "pending")` for every signer in `SignerPool` during startup initialization.
4. **Implement Nonce Rollback & Replacement on Error (`pipeline.rs`, `signers.rs`)**:
   Roll back local nonces or issue EIP-1559 speedup transactions upon RPC broadcast timeouts or sequencer rejections.
5. **Secure Smart Contract Case 0 & Measure `balanceBefore` Dynamically (`contracts/executor.yul`)**:
   Enforce `require(caller() == OWNER)` in `case 0` for direct arbitrage calls. Measure `balanceBefore` on-chain via `WETH.balanceOf(address(this))` rather than trusting calldata.
6. **Implement Balancer V3 & Uniswap V4 Settlement (`contracts/executor.yul`)**:
   In `unlockCallback` (`0x98dd202a`), add repayment logic transferring `wethAmountToFirstMarket` to `caller()` / calling `vault.settle()`.
7. **Eliminate Profit Fallback Facade (`crates/simulator/src/engine.rs:240-244`)**:
   Remove `else { route.sizing.net_profit }`. Fail simulation if post-execution balance delta is non-positive or less than gas costs.
8. **Set Realistic Live Gas Limits (`pipeline.rs:169`)**:
   Do not cap live gas limit at `sim_outcome.gas_used.max(100_000)`. Set a realistic minimum of `300_000 gas` for multi-hop swaps.

---

### Phase 2: High & Medium Severity Architectural & Safety Hardening

1. **Implement 3-Hop Triangular Route Calldata & Yul Execution**:
   Extend `ExecutionParams` in `calldata.rs` and `contracts/executor.yul` to support `target3`, `token2_amountOut`, and `is_three_hop`. Route output of Pool 2 to Pool 3, and Pool 3 to the executor.
2. **Implement L1 Data Fee Accounting**:
   Integrate `ArbGasInfo` (`0x6C`) on Arbitrum and `GasPriceOracle` (`0x4200...000F`) on Base. Deduct L1 calldata posting fees from expected gross profit before sizing and dispatch.
3. **Implement Dynamic Gas & Priority Fee Polling (H1)**:
   Poll live `eth_gasPrice` and `eth_maxPriorityFeePerGas`. Remove hardcoded 20 Gwei in `engine.rs` and 0.1 Gwei in `pipeline.rs`. Include Base WETH in dynamic priority fee bribe calculations.
4. **Normalize Token Decimals vs Gas Wei**:
   Convert 18-decimal ETH gas costs into the borrow asset's decimals using the current pool exchange rate before calculating `net_yield`.
5. **Parameterize SimulationEngine on Base Chain**:
   Call `build_executor_bytecode(chain_id, owner)` in `SimulationEngine::new()` so Base simulations deploy Base WETH bytecode.
6. **Deploy UniV3Quoter in SimulationEngine & Support V3 Swaps in Yul**:
   Deploy `QUOTER_RUNTIME_BYTECODE` in `SimulationEngine`. Add branches in `executor.yul` supporting Uniswap V3 `swap(0x128acb08)`.
7. **Fix Bounds Checks in Multicall3 and Vanity Miner**:
   Replace `queries[idx]` with `queries.get(idx)` in `discovery.rs:934`. Add `if target_zero_bytes > 20 { return false; }` in `vanity.rs:415`.
8. **Decouple Market Feed Ingestion from Pipeline Simulation (`main.rs`)**:
   Separate graph reserve updates into a dedicated sub-microsecond ingestion task, offloading pipeline simulation and HTTP broadcast to asynchronous worker tasks to prevent broadcast buffer overflow and false Kill Switch trips.

---

### Phase 3: Test Suite Integration & Production Certification

1. **Re-Architect Test Suite (`tests/Cargo.toml`)**:
   Add workspace dependencies (`flash-core`, `flash-math`, `flash-scanner`, `flash-collector`, `flash-simulator`, `flash-executor`, `flash-strategy`) to `tests/Cargo.toml`. Replace mock tests with authentic tests exercising genuine codebase logic.
2. **Recompile `UniV3Quoter.yul` into `quoter.rs`**:
   Recompile Yul source so `QUOTER_RUNTIME_BYTECODE` includes the `shr(160, liquidity)` shift guard and limit price clamping.
3. **Clean Up Remnant Fallbacks**:
   Remove heuristic `0x94` byte scanning in `arbitrum.rs:201-229`. Deprecate and remove obsolete 100-byte calldata encoder in `calldata.rs:84`.
4. **Set Whole-Program LTO (`Cargo.toml`)**:
   Configure `lto = "fat"` in root `Cargo.toml` for release builds.

---

## 6. Audit Sign-Off & Attestation

This consolidated report has been synthesized from the independent findings of all five audit streams:
- **Stream 1**: Remediation Scorecard & Anti-Stub / Anti-Fake Verification
- **Stream 2**: Smart Contract, Calldata & Bytecode Synchronization
- **Stream 3**: L2 Gas, revm State Fidelity & Multi-EOA Signers
- **Stream 4**: Panic Safety, Concurrency & DoS
- **Stream 5**: Forensic Integrity Audit & Binary Verdict

The codebase in its current state presents extreme financial and operational risks. **Under no circumstances should the bot be deployed to mainnet or operated with funded private keys until Phase 1 and Phase 2 remediations are completed and independently verified.**

*Report finalized on November 1, 2026. All source files remained strictly unmodified during this audit.*
