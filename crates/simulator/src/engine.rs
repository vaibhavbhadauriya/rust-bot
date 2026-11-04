//! Embedded revm In-Memory Simulation Engine.
//! Executes candidate arbitrage routes, state overrides, 100-byte calldata dispatch, and profit extraction.
//! Authoritative source: ORIGINAL_REQUEST.md lines 43-46, 56-67, PROJECT.md lines 58, 93-94.

use alloy_primitives::{ruint::UintTryFrom, Address, Bytes, FixedBytes, I256, U256, U512};
use flash_core::constants::arbitrum::WETH as ARB_WETH;
use flash_core::types::{Chain, GasPrices};
use flash_executor::bytecode::build_executor_bytecode;
use flash_executor::calldata::{encode_calldata, ExecutionParams};
use flash_scanner::dfs::RouteCandidate;
use flash_scanner::graph::MarketGraph;
use parking_lot::RwLock;
use revm::{
    context::{Context, TxEnv},
    context_interface::result::ExecutionResult,
    database::in_memory_db::InMemoryDB,
    handler::{ExecuteEvm, MainBuilder, MainContext},
    inspector::InspectEvm,
    primitives::{hardfork::SpecId, StorageKey, TxKind},
    state::AccountInfo,
    Inspector,
};
use std::sync::Arc;

use crate::bytecode_mocks::*;
use crate::cache::BytecodeCache;
use crate::types::*;

pub struct SimulationEngine {
    pub chain: Chain,
    pub executor_address: Address,
    pub weth_address: Address,
    pub base_db: InMemoryDB,
    pub cache: Arc<BytecodeCache>,
    pub gas_prices: Arc<RwLock<GasPrices>>,
    pub graph: RwLock<Option<Arc<MarketGraph>>>,
}

impl SimulationEngine {
    pub fn new(chain: Chain, cache: Arc<BytecodeCache>) -> Self {
        let weth_address = match chain {
            Chain::ArbitrumOne => ARB_WETH,
            Chain::Base => flash_core::constants::base::WETH,
        };
        let executor_address = MOCK_EXECUTOR_ADDRESS;

        let mut base_db = InMemoryDB::default();

        // 1. Deploy Pure Yul Executor parameterized with chain_id and owner (V-15)
        let executor_raw = build_executor_bytecode(chain.id(), MOCK_CALLER_ADDRESS);
        let executor_code = revm::bytecode::Bytecode::new_raw(executor_raw);
        base_db.insert_account_info(
            executor_address,
            AccountInfo {
                code: Some(executor_code.clone()),
                balance: U256::ZERO,
                nonce: 1,
                code_hash: executor_code.hash_slow(),
                account_id: None,
            },
        );

        // 2. Deploy WETH contract with realistic balance tracking
        let weth_code = mock_realistic_weth_bytecode();
        base_db.insert_account_info(
            weth_address,
            AccountInfo {
                code: Some(weth_code.clone()),
                balance: U256::ZERO,
                nonce: 1,
                code_hash: weth_code.hash_slow(),
                account_id: None,
            },
        );

        // 3. Deploy UniV3Quoter runtime bytecode (V-16)
        let quoter_code = crate::quoter::quoter_runtime_bytecode();
        base_db.insert_account_info(
            crate::quoter::DEFAULT_QUOTER_ADDRESS,
            AccountInfo {
                code: Some(quoter_code.clone()),
                balance: U256::ZERO,
                nonce: 1,
                code_hash: quoter_code.hash_slow(),
                account_id: None,
            },
        );

        let gas_prices = Arc::new(RwLock::new(GasPrices::default_for_chain(chain)));
        let graph = RwLock::new(None);

        Self {
            chain,
            executor_address,
            weth_address,
            base_db,
            cache,
            gas_prices,
            graph,
        }
    }

    pub fn set_gas_prices(&self, prices: GasPrices) {
        *self.gas_prices.write() = prices;
    }

    pub fn gas_prices(&self) -> GasPrices {
        *self.gas_prices.read()
    }

    pub fn set_graph(&self, graph: Arc<MarketGraph>) {
        *self.graph.write() = Some(graph);
    }

    pub fn with_graph(self, graph: Arc<MarketGraph>) -> Self {
        *self.graph.write() = Some(graph);
        self
    }

    /// Injects custom state overrides into an in-memory database
    pub fn apply_state_overrides(&self, db: &mut InMemoryDB, overrides: &[StateOverride]) {
        for ovr in overrides {
            if ovr.balance.is_some() || ovr.nonce.is_some() || ovr.code.is_some() {
                let existing = db.load_account(ovr.address).ok().cloned().unwrap_or_default();
                let code = ovr
                    .code
                    .as_ref()
                    .map(|c| revm::bytecode::Bytecode::new_raw(c.clone()))
                    .or(existing.info.code);
                let code_hash = code.as_ref().map(|c| c.hash_slow()).unwrap_or(FixedBytes::ZERO);
                let balance = ovr.balance.unwrap_or(existing.info.balance);
                let nonce = ovr.nonce.unwrap_or(existing.info.nonce);

                db.insert_account_info(
                    ovr.address,
                    AccountInfo {
                        balance,
                        nonce,
                        code_hash,
                        code,
                        account_id: None,
                    },
                );
            }

            for (slot, value) in &ovr.storage {
                let _ = db.insert_account_storage(
                    ovr.address,
                    *slot,
                    *value,
                );
            }
        }
    }

    /// Sets up route balances and pool reserves into an isolated in-memory DB snapshot
    pub fn setup_route_state(
        &self,
        db: &mut InMemoryDB,
        route: &RouteCandidate,
        initial_weth_balance: U256,
    ) {
        // Populate caller and executor WETH balance in both realistic slot and slot 3
        let exec_slot = realistic_weth_balance_slot(self.executor_address);
        let _ = db.insert_account_storage(
            self.weth_address,
            StorageKey::from(exec_slot),
            initial_weth_balance,
        );
        let _ = db.insert_account_storage(
            self.weth_address,
            StorageKey::from(3),
            initial_weth_balance,
        );

        // If route borrows non-WETH token, deploy token contract if needed and fund executor
        if route.borrow_token != self.weth_address && route.borrow_token != Address::ZERO {
            let exec_borrow_slot = realistic_weth_balance_slot(self.executor_address);
            let _ = db.insert_account_storage(
                route.borrow_token,
                StorageKey::from(exec_borrow_slot),
                initial_weth_balance,
            );
            if db.load_account(route.borrow_token).ok().and_then(|a| a.info.code.as_ref()).is_none() {
                let token_code = mock_realistic_weth_bytecode();
                db.insert_account_info(
                    route.borrow_token,
                    AccountInfo {
                        code: Some(token_code.clone()),
                        balance: U256::ZERO,
                        nonce: 1,
                        code_hash: token_code.hash_slow(),
                        account_id: None,
                    },
                );
            }
        }

        // Populate pool bytecodes, storage slots 8, 9, 10, and token balances
        for hop in &route.hops {
            let pool_code = self
                .cache
                .get_bytecode(self.chain, hop.pool_id)
                .unwrap_or_else(|| mock_v2_pair_bytecode_for_weth(self.weth_address));

            db.insert_account_info(
                hop.pool_id,
                AccountInfo {
                    code: Some(pool_code.clone()),
                    balance: U256::ZERO,
                    nonce: 1,
                    code_hash: pool_code.hash_slow(),
                    account_id: None,
                },
            );

            // Directional reserve packing
            let (r0, r1) = if hop.zero_for_one {
                (hop.reserve_in, hop.reserve_out)
            } else {
                (hop.reserve_out, hop.reserve_in)
            };
            let packed_reserves: U256 = U256::from(r0) | (U256::from(r1) << 112);

            // Storage slot 8: directional packed reserves
            let _ = db.insert_account_storage(
                hop.pool_id,
                StorageKey::from(8),
                packed_reserves,
            );

            // Storage slot 9: token_out address
            let _ = db.insert_account_storage(
                hop.pool_id,
                StorageKey::from(9),
                U256::from_be_slice(hop.token_out.as_slice()),
            );

            // Storage slot 10: token_in address
            let _ = db.insert_account_storage(
                hop.pool_id,
                StorageKey::from(10),
                U256::from_be_slice(hop.token_in.as_slice()),
            );

            // Deploy token contract if not present (for intermediate and output tokens)
            if db.load_account(hop.token_out).ok().and_then(|a| a.info.code.as_ref()).is_none() {
                let token_code = mock_realistic_weth_bytecode();
                db.insert_account_info(
                    hop.token_out,
                    AccountInfo {
                        code: Some(token_code.clone()),
                        balance: U256::ZERO,
                        nonce: 1,
                        code_hash: token_code.hash_slow(),
                        account_id: None,
                    },
                );
            }

            // Pre-fund the pool's token_out balance so that pool.swap() -> tokenOut.transfer() succeeds
            let pool_token_slot = realistic_weth_balance_slot(hop.pool_id);
            let _ = db.insert_account_storage(
                hop.token_out,
                StorageKey::from(pool_token_slot),
                U256::from(100_000_000_000_000_000_000_000u128), // 100,000 tokens reserve
            );
        }
    }

    /// Normalizes 18-decimal ETH gas cost into native units of the borrow token
    pub fn convert_gas_cost_to_token(&self, gas_cost_wei: U256, borrow_token: Address) -> U256 {
        // If borrowing WETH, 1:1 mapping (both 18 decimals)
        if borrow_token == self.weth_address || borrow_token == Address::ZERO {
            return gas_cost_wei;
        }

        // 1. Try querying MarketGraph for live WETH / borrow_token pool reserve ratio
        if let Some(graph) = self.graph.read().as_ref() {
            let edges = graph.get_edges(borrow_token);
            for edge in edges.iter() {
                if edge.token_out == self.weth_address && edge.reserve_out > 0 {
                    // gas_cost_token = (gas_cost_wei * reserve_token) / reserve_weth
                    if let Some(converted) = flash_math::mul_div_u256(
                        gas_cost_wei,
                        U256::from(edge.reserve_in),
                        U256::from(edge.reserve_out),
                    ) {
                        return converted;
                    }
                }
            }
        }

        // 2. Fallback: Decimal scaling based on known token constants and benchmark ETH prices
        let decimals = flash_core::constants::get_token_decimals(self.chain, borrow_token);
        match decimals {
            6 => {
                // USDC / USDT ($2500/ETH): wei * 2500 / 10^12
                gas_cost_wei
                    .saturating_mul(U256::from(2500))
                    .checked_div(U256::from(1_000_000_000_000u64))
                    .unwrap_or(U256::ZERO)
            }
            8 => {
                // WBTC / cbBTC (1 BTC = 25 ETH, 0.04 BTC/ETH = 4_000_000 satoshis): wei * 4_000_000 / 10^18
                gas_cost_wei
                    .saturating_mul(U256::from(4_000_000))
                    .checked_div(U256::from(1_000_000_000_000_000_000u128))
                    .unwrap_or(U256::ZERO)
            }
            _ => gas_cost_wei,
        }
    }

    /// Alias for simulate_arbitrage_route for API convenience
    pub fn simulate_execution(
        &self,
        route: &RouteCandidate,
        trade_size: U256,
    ) -> Result<SimulationOutcome, SimError> {
        self.simulate_arbitrage_route(route, trade_size)
    }

    /// Simulates candidate route execution in-memory (< 1.5ms)
    pub fn simulate_arbitrage_route(
        &self,
        route: &RouteCandidate,
        trade_size: U256,
    ) -> Result<SimulationOutcome, SimError> {
        if route.hops.len() < 2 {
            return Err(SimError::InvalidRoute("Route must have >= 2 hops".into()));
        }

        // 1. Create isolated DB snapshot
        let mut sim_db = self.base_db.clone();
        self.setup_route_state(&mut sim_db, route, trade_size);

        // 2. Calldata construction (supports 2-hop and 3-hop)
        let hop1 = &route.hops[0];
        let hop2 = &route.hops[1];

        let est_gas_used: u64 = if route.hops.len() >= 3 { 250_000 } else { 180_000 };
        let est_gas_cost_wei = self.gas_prices().total_gas_cost_wei(est_gas_used);
        let target_token = if route.borrow_token != Address::ZERO {
            route.borrow_token
        } else {
            self.weth_address
        };
        let est_gas_cost_token = self.convert_gas_cost_to_token(est_gas_cost_wei, target_token);
        let gas_cost_val = est_gas_cost_token.to::<u128>();
        let gas_cost_val = if gas_cost_val > 0 {
            gas_cost_val
        } else {
            18_000_000_000_000u128
        };

        let mut params = ExecutionParams {
            weth_amount_to_first_market: trade_size.to::<u128>(),
            balance_before: 0,
            gas_cost: gas_cost_val,
            target1: hop1.pool_id,
            target2: hop2.pool_id,
            token0_amount_out: route.sizing.expected_intermediate.to::<u128>(),
            token0_amount_out_index: if hop1.zero_for_one { 1 } else { 0 },
            token1_amount_out: route.sizing.expected_output.to::<u128>(),
            token1_amount_out_index: if hop2.zero_for_one { 1 } else { 0 },
            ..Default::default()
        };

        if route.hops.len() >= 3 {
            let hop3 = &route.hops[2];
            params.target3 = hop3.pool_id;
            // Compute genuine Hop 1 intermediate output (Token B) from trade_size through Pool 1
            if hop1.reserve_in > 0 && hop1.reserve_out > 0 {
                let f = U256::from(flash_core::constants::PIP_SCALE);
                let gamma = f.saturating_sub(U256::from(hop1.fee_pips));
                let x_a = U256::from(hop1.reserve_in);
                let y_a = U256::from(hop1.reserve_out);
                let dx = trade_size;
                let num_dy: U512 = y_a.widening_mul(gamma) * U512::from(dx);
                let den_dy: U512 = x_a.widening_mul(f) + gamma.widening_mul(dx);
                if !den_dy.is_zero() {
                    if let Ok(dy) = U256::uint_try_from(num_dy / den_dy) {
                        params.token0_amount_out = dy.to::<u128>();
                    }
                }
            }
            // Hop 2 output is the intermediate token between Pool 2 and Pool 3
            params.token1_amount_out = route.sizing.expected_intermediate.to::<u128>();
            // Hop 3 output is the final output token back to executor
            params.token2_amount_out = route.sizing.expected_output.to::<u128>();
            params.token2_amount_out_index = if hop3.zero_for_one { 1 } else { 0 };
            params.hop_count = 3;
        }

        let calldata_bytes = encode_calldata(&params);

        // 3. Build TxEnv (H2: 1_200_000 gas limit)
        let tx = TxEnv {
            caller: MOCK_CALLER_ADDRESS,
            gas_limit: 1_200_000,
            gas_price: 0,
            kind: TxKind::Call(self.executor_address),
            value: U256::ZERO,
            data: Bytes::from(calldata_bytes.clone()),
            ..Default::default()
        };

        // 4. Execute EVM in Cancun mode
        let ctx = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
            .with_db(sim_db);

        let mut evm = ctx.build_mainnet();
        let exec_result_and_state = evm
            .transact(tx)
            .map_err(|e| SimError::DatabaseError(format!("EVM transaction error: {:?}", e)))?;

        let execution_result = exec_result_and_state.result;

        // 5. Parse Outcome
        match execution_result {
            ExecutionResult::Success { gas, logs, output, .. } => {
                let gas_used = gas.tx_gas_used();
                let gas_prices = self.gas_prices();
                let total_gas_cost_wei = gas_prices.total_gas_cost_wei(gas_used);

                // C6: Calculate actual profit strictly from storage balance delta - ZERO FALLBACK FACADE
                let exec_slot = realistic_weth_balance_slot(self.executor_address);
                let target_token = if route.borrow_token != Address::ZERO {
                    route.borrow_token
                } else {
                    self.weth_address
                };

                let post_balance = exec_result_and_state
                    .state
                    .get(&target_token)
                    .and_then(|acc| acc.storage.get(&StorageKey::from(exec_slot)))
                    .map(|s| s.present_value())
                    .unwrap_or(U256::ZERO);

                let actual_profit = post_balance.saturating_sub(trade_size);

                // V-11: Decimal normalization of gas cost into borrow token units
                let gas_cost_in_borrow_token = self.convert_gas_cost_to_token(
                    total_gas_cost_wei,
                    target_token,
                );

                // Strictly reject unprofitable trades
                if actual_profit <= gas_cost_in_borrow_token || actual_profit.is_zero() {
                    return Err(SimError::Unprofitable {
                        profit: actual_profit,
                        gas_cost: gas_cost_in_borrow_token,
                    });
                }

                let net_yield = I256::from_raw(actual_profit) - I256::from_raw(gas_cost_in_borrow_token);

                Ok(SimulationOutcome {
                    is_success: true,
                    gas_used,
                    gas_cost_wei: total_gas_cost_wei,
                    actual_profit,
                    net_yield,
                    calldata: Bytes::from(calldata_bytes),
                    fast_revert_tripped: false,
                    logs,
                    return_data: output.into_data(),
                })
            }
            ExecutionResult::Revert { output, .. } => {
                if output.is_empty() {
                    Err(SimError::FastRevertTriggered)
                } else if output.len() == 32 {
                    let profit = U256::from_be_slice(&output);
                    Err(SimError::Unprofitable {
                        profit,
                        gas_cost: U256::from(gas_cost_val),
                    })
                } else {
                    Err(SimError::Reverted(output))
                }
            }
            ExecutionResult::Halt { reason, .. } => {
                Err(SimError::Halted(format!("{:?}", reason)))
            }
        }
    }

    /// Generic raw EVM execution helper for custom transactions
    pub fn execute_raw(
        &self,
        tx: TxEnv,
        overrides: &[StateOverride],
    ) -> Result<ExecutionResult, SimError> {
        let mut sim_db = self.base_db.clone();
        self.apply_state_overrides(&mut sim_db, overrides);

        let ctx = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
            .with_db(sim_db);

        let mut evm = ctx.build_mainnet();
        let exec_result_and_state = evm
            .transact(tx)
            .map_err(|e| SimError::DatabaseError(format!("EVM transaction error: {:?}", e)))?;

        Ok(exec_result_and_state.result)
    }

    /// Executes transaction with an inspector attached (e.g. HoneypotTxInspector)
    pub fn inspect_raw<INSP>(
        &self,
        tx: TxEnv,
        inspector: INSP,
        overrides: &[StateOverride],
    ) -> Result<(ExecutionResult, INSP), SimError>
    where
        INSP: Inspector<
            Context<
                revm::context::BlockEnv,
                TxEnv,
                revm::context::CfgEnv,
                InMemoryDB,
                revm::context::Journal<InMemoryDB>,
                (),
            >,
            revm::interpreter::interpreter::EthInterpreter,
        >,
    {
        let mut sim_db = self.base_db.clone();
        self.apply_state_overrides(&mut sim_db, overrides);

        let ctx = Context::mainnet()
            .modify_cfg_chained(|cfg| cfg.set_spec_and_mainnet_gas_params(SpecId::CANCUN))
            .with_db(sim_db);

        let mut evm = ctx.build_mainnet_with_inspector(inspector);
        let exec_result_and_state = evm
            .inspect_tx(tx)
            .map_err(|e| SimError::DatabaseError(format!("EVM inspection error: {:?}", e)))?;

        let (result, inspector) = (exec_result_and_state.result, evm.inspector);
        Ok((result, inspector))
    }
}
