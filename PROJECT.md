# Project: Master Pre-Production Remediation

## Architecture
- **Workspace Crates**:
  - `flash-core`: Core types, chain constants, configuration and validation.
  - `flash-math`: Fixed-point closed form arbitrage math, Newton-Raphson integer sqrt, reserve scaling.
  - `flash-scanner`: Multicall3 discovery, pool registry, concurrent graph representation, and cycle detection.
  - `flash-collector`: WebSocket feeds for Arbitrum One pre-confirmations, Base Flashblocks, and DEX sync events.
  - `flash-simulator`: `revm` in-memory EVM simulation, balance delta verification, and concentrated liquidity quoter.
  - `flash-executor`: Calldata encoding, vanity address mining, flash loan routing, and runtime bytecode bindings.
  - `flash-strategy`: Signer pool, EIP-1559 signing, dynamic fee bidding, gas oracle, and execution pipeline.
- **Top-Level Binary**: `src/main.rs` (ingestion, graph state updates, candidate generation, and lifecycle orchestration).
- **Smart Contracts**: `contracts/executor.yul` (multi-hop flash loan executor) and `contracts/UniV3Quoter.yul` (concentrated liquidity quoter).
- **Test Suite**: `tests/Cargo.toml` wired to workspace crates with zero mock assertions.

---

## Feature Inventory

Every flaw and vulnerability from `ORIGINAL_REQUEST.md` and `PRE_PROD_AUDIT_REPORT.md` is catalogued and assigned to a milestone:

| # | Item | Description | Milestone | Source |
|---|------|-------------|:---------:|:------:|
| 1 | V-01 | Live broadcaster sets `to` to `self.simulator.executor_address` (0x01 precompile) instead of `config.executor_contract_address` | M1 | Audit Report §3 |
| 2 | V-26 | CLI `--live` bypasses zero-address fail-fast check in config | M1 | Audit Report §3 |
| 3 | M6 | Missing `validate_live_mode()` validation for executor contract address | M1 | ORIGINAL_REQUEST §R1 |
| 4 | V-10 | Empty access list RLP encodes as `0x80` byte string instead of `0xc0` RLP list | M1 | Audit Report §3 |
| 5 | V-08 | Zero startup nonce synchronization against `eth_getTransactionCount` | M1 | Audit Report §3 |
| 6 | V-09 | Local nonces increment before broadcast without rollback on error, permanently bricking signers | M1 | Audit Report §3 |
| 7 | V-23 | Missing `ecrecover` signature verification in signer self-test | M1 | Audit Report §3 |
| 8 | V-07 | Live gas limit floor set to `(sim_outcome.gas_used * 125 / 100).max(350_000)` | M1 | Audit Report §3 |
| 9 | V-02 | `case 0` in `executor.yul` lacks caller check and trusts calldata `balanceBefore` | M2 | Audit Report §3 |
| 10 | V-03 | Balancer V3 & Uniswap V4 callback (`0x98dd202a`) lacks repayment settlement logic | M2 | Audit Report §3 |
| 11 | V-04 | 3-hop triangular route dropped in calldata and `executor.yul` | M2 | Audit Report §3 |
| 12 | V-27 | Obsolete 100-byte calldata encoder exported and used | M2 | Audit Report §3 |
| 13 | V-14 | Yul fast-revert guard only checks `iszero(reserve0)` instead of expected output | M2 | Audit Report §3 |
| 14 | V-24 | Calldata `gas_cost` hardcoded to 50,000 wei instead of dynamic cost | M2 | Audit Report §3 |
| 15 | H5 | `UniV3Quoter.yul` fails to adjust `amountOut` and `amountInConsumed` on clamped price limits | M2 | Audit Report §2 |
| 16 | L8 | `UniV3Quoter.yul` shift guard on `shr(160, liquidity)` | M2 | Audit Report §2 |
| 17 | V-25 | `quoter.rs` embeds stale bytecode missing L8 shift guard | M2 | Audit Report §3 |
| 18 | V-16 | Recompile `executor.yul` into `bytecode.rs` and update PUSH20 parameter offsets | M2 | Audit Report §3 |
| 19 | C6 | Simulation profit validation via genuine revm balance delta without fallback | M3 | Audit Report §2 |
| 20 | V-05 | Delete fallback facade `else { route.sizing.net_profit }` in `engine.rs` | M3 | Audit Report §3 |
| 21 | V-15 | `SimulationEngine::new()` deploys unparameterized bytecode on Base | M3 | Audit Report §3 |
| 22 | V-22 | Pool bytecode mock no-op under-gasses simulation and bypasses storage mutations | M3 | Audit Report §3 |
| 23 | V-06 | Absent L1 calldata/blob posting fee accounting from `ArbGasInfo` and `GasPriceOracle` | M3 | Audit Report §3 |
| 24 | H1 | Eliminate hardcoded 20 Gwei gas price; poll live gas prices asynchronously | M3 | Audit Report §2 |
| 25 | V-12 | Hardcoded 0.1 Gwei base fee in pipeline and 20 Gwei in simulator | M3 | Audit Report §3 |
| 26 | V-11 | Normalize 18-decimal ETH total gas cost into borrow token native decimals | M3 | Audit Report §3 |
| 27 | V-13 | Base chain dynamic priority fee bidding (20% of net profit) disabled | M3 | Audit Report §3 |
| 28 | C1 | Live on-chain pool discovery via Multicall3; Camelot V3 pool address; delete synthetic loops | M4 | Audit Report §2 |
| 29 | V-17 | Multicall3 index out-of-bounds panic on unchecked slice indexing | M4 | Audit Report §3 |
| 30 | C5 | Delete heuristic `0x94` byte scanning fallback in `FastTxDecoder` | M4 | Audit Report §2 |
| 31 | V-21 | Remnant `0x94` scanning emits dummy transactions with gas_limit 0 | M4 | Audit Report §3 |
| 32 | V-18 | Vanity miner out-of-bounds slice panic when `n > addr_slice.len()` | M4 | Audit Report §3 |
| 33 | V-19 | Decouple WebSocket feed ingestion from simulation via watch/worker channel | M4 | Audit Report §3 |
| 34 | V-29 | Zero-allocation in-place reserve updates in `graph.rs` via `Arc::make_mut` | M4 | Audit Report §3 |
| 35 | V-20 | Wire `tests/Cargo.toml` to real workspace crates; eliminate mock test assertions | M4 | Audit Report §3 |
| 36 | V-28 | Compiler release profile `lto = "fat"` in root `Cargo.toml` | M4 | Audit Report §3 |
| 37 | M5 | Full workspace integration test suite passes 100% exit code 0 under release profile | M5 | Acceptance Criteria |

---

## Milestones

| # | Name | Scope | Dependencies | Status |
|---|------|-------|-------------|:------:|
| M0 | Survey & Architecture | Map all 29 vulnerabilities and remaining flaws down to exact line citations | None | DONE |
| M1 | Emergency Live & Signer Blockers | Fix live target address, CLI `--live` validation, empty access list RLP `0xc0`, startup nonce sync, error rollback, and gas limit floor | M0 | DONE |
| M2 | Smart Contracts & Calldata | `executor.yul` access control & dynamic balanceBefore, Balancer/V4 repayment, 3-hop routing, fast-revert guard, Quoter recompilation & bytecode bindings | M0 | PLANNED |
| M3 | revm Simulation & Gas Oracles | Real revm balance delta, delete fallback facade, realistic mock pool gas, dynamic L1+L2 gas oracle, decimal normalization, Base dynamic bidding | M1, M2 | PLANNED |
| M4 | Scanner, Concurrency & Tests | Multicall3 bounds, Camelot V3 pool address, delete synthetic loops & 0x94 scanner, vanity guard, feed decoupling, `tests/Cargo.toml` wiring, `lto="fat"` | M1, M2, M3 | PLANNED |
| M5 | Final Verification & Audit Gate | Full workspace release test verification (`cargo test --workspace --release`), Reviewer approval, Challenger proof, and Forensic Auditor CLEAN sign-off | M1, M2, M3, M4 | PLANNED |

---

## Interface Contracts

### 1. Calldata Layout (124B for 2 Hops, 160B for 3 Hops)
- **Rust Struct**: `ExecutionParams` in `crates/executor/src/calldata.rs`
- **Fields**:
  - `weth_amount_to_first_market`: `u128` (16 bytes, offset `0x00`)
  - `balance_before`: `u128` (16 bytes, offset `0x10`)
  - `gas_cost`: `u128` (16 bytes, offset `0x20`)
  - `target1`: `Address` (20 bytes, offset `0x30`)
  - `target2`: `Address` (20 bytes, offset `0x44`)
  - `token0_amount_out`: `u128` (16 bytes, offset `0x58`)
  - `token0_amount_out_index`: `u16` (2 bytes, offset `0x68`)
  - `token1_amount_out`: `u128` (16 bytes, offset `0x6a`)
  - `token1_amount_out_index`: `u16` (2 bytes, offset `0x7a`)
  - Optional 3-hop fields (offset `0x7c`):
    - `target3`: `Address` (20 bytes, offset `0x7c`)
    - `token2_amount_out`: `u128` (16 bytes, offset `0x90`)
    - `token2_amount_out_index`: `u16` (2 bytes, offset `0xa0`)
- **Yul Calldata Decoder**: `contracts/executor.yul` checks `calldatasize >= 0x7c`. When `calldatasize >= 0xa0`, executes 3-hop route routing Pool 1 -> Pool 2 -> Pool 3 -> `address(this)`.

### 2. Bytecode Bindings & Offsets
- `build_executor_bytecode(chain_id: u64, owner: Address) -> Bytes`:
  - `OWNER` PUSH20 injected at runtime offsets `27..47`, `73..93`, and `203..223`.
  - `WETH` PUSH20 injected at runtime offset `1077..1097`.
- `QUOTER_RUNTIME_BYTECODE`: 354 bytes compiled from `contracts/UniV3Quoter.yul` with `shr(160, liquidity)` shift guard and clamped `amountOut` recalculation.

### 3. Gas Oracle & Normalization
- `GasOracle::poll_gas_prices(&self) -> Result<GasPrices, GasOracleError>`
- `GasPrices`: `l2_gas_price: u128`, `l1_data_fee_wei: u128`, `base_fee_per_gas: u128`, `max_priority_fee_per_gas: u128`
- `DecimalNormalizer::normalize_gas_to_token(gas_wei: U256, token: Address, graph: &MarketGraph) -> U256`

---

## Code Layout & File Boundaries

| Milestone | Exclusively Owned Files |
|-----------|------------------------|
| **M1** | `crates/strategy/src/pipeline.rs`, `crates/strategy/src/signers.rs`, `crates/core/src/config.rs`, `src/main.rs` (startup validation/sync sections) |
| **M2** | `contracts/executor.yul`, `contracts/UniV3Quoter.yul`, `crates/executor/src/bytecode.rs`, `crates/executor/src/calldata.rs`, `crates/simulator/src/quoter.rs` |
| **M3** | `crates/simulator/src/engine.rs`, `crates/simulator/src/bytecode_mocks.rs`, `crates/strategy/src/gas_oracle.rs` (new), `crates/strategy/src/pipeline.rs` (pricing & bidding sections) |
| **M4** | `crates/scanner/src/discovery.rs`, `crates/collector/src/arbitrum.rs`, `crates/executor/src/vanity.rs`, `src/main.rs` (concurrency decoupling), `crates/scanner/src/graph.rs`, `tests/Cargo.toml`, root `Cargo.toml` |
| **M5** | Integration and verification across all crates and contracts |
