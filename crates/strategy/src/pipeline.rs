//! End-to-End Arbitrage Execution Pipeline connecting Collector, Scanner, Math, Simulator, Multi-EOA Signers, and Shadow Engine.
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 69-74, 90; PROJECT.md § Milestone 5.

use alloy_primitives::{Address, U256};
use flash_collector::feed::MarketUpdate;
use flash_core::config::AppConfig;
use flash_core::types::Chain;
use flash_scanner::dfs::dfs_cyclic_opportunities_into;
use flash_scanner::graph::MarketGraph;
use flash_simulator::engine::SimulationEngine;
use flash_simulator::honeypot::verify_token_integrity;
use flash_simulator::types::SimError;
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tracing::{info, warn};

use crate::kill_switch::KillSwitchController;
use crate::shadow::ShadowEngine;
use crate::signers::{Eip1559TransactionRequest, SignerError, SignerPool};
use crate::telemetry::{PipelineTelemetry, TelemetryCollector};

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("Kill switch tripped: {0}")]
    KillSwitchTripped(String),
    #[error("Simulation error: {0}")]
    Simulation(#[from] SimError),
    #[error("Signer pool exhausted: no available EOAs")]
    SignerPoolExhausted,
    #[error("Signer error: {0}")]
    Signer(#[from] SignerError),
    #[error("Honeypot violation: {0}")]
    Honeypot(String),
}

pub struct ArbitragePipeline {
    pub config: AppConfig,
    pub graph: Arc<MarketGraph>,
    pub simulator: Arc<SimulationEngine>,
    pub signer_pool: Arc<SignerPool>,
    pub shadow_engine: Arc<ShadowEngine>,
    pub kill_switch: Arc<KillSwitchController>,
    pub telemetry: Arc<TelemetryCollector>,
    pub http_client: reqwest::Client,
}

impl ArbitragePipeline {
    pub fn new(
        config: AppConfig,
        graph: Arc<MarketGraph>,
        simulator: Arc<SimulationEngine>,
        signer_pool: Arc<SignerPool>,
        shadow_engine: Arc<ShadowEngine>,
        kill_switch: Arc<KillSwitchController>,
        telemetry: Arc<TelemetryCollector>,
    ) -> Self {
        Self {
            config,
            graph,
            simulator,
            signer_pool,
            shadow_engine,
            kill_switch,
            telemetry,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(2000))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Processes streaming MarketUpdate through the full end-to-end pipeline (< 3.5ms).
    pub async fn process_update(
        &self,
        update: MarketUpdate,
    ) -> Result<Option<PipelineTelemetry>, PipelineError> {
        let t_start = Instant::now();

        // 1. Kill switch pre-check (<2ns)
        if self.kill_switch.is_tripped() {
            return Err(PipelineError::KillSwitchTripped(
                self.kill_switch
                    .trip_reason()
                    .unwrap_or_else(|| "Kill switch active".to_string()),
            ));
        }

        // 2. Feed lag measurement
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let (pool_id, update_timestamp_ms) = match &update {
            MarketUpdate::ReserveUpdate {
                pool_id,
                timestamp_ms,
                ..
            } => (*pool_id, *timestamp_ms),
            MarketUpdate::TickUpdate {
                pool_id,
                timestamp_ms,
                ..
            } => (*pool_id, *timestamp_ms),
            MarketUpdate::PendingTxMutation {
                target_pool,
                timestamp_ms,
                ..
            } => (*target_pool, *timestamp_ms),
            MarketUpdate::FlashblockStep { .. } => return Ok(None),
        };

        let lag_ms = now_ms.saturating_sub(update_timestamp_ms);
        if self.kill_switch.check_feed_lag(lag_ms) {
            warn!("Kill switch tripped: feed lag {}ms exceeded threshold", lag_ms);
            return Err(PipelineError::KillSwitchTripped(format!("Feed lag {}ms", lag_ms)));
        }

        // Apply update to in-memory graph (idempotent in-place mutation)
        match &update {
            MarketUpdate::ReserveUpdate { pool_id, reserve0, reserve1, .. } => {
                self.graph.update_reserves(*pool_id, *reserve0, *reserve1);
            }
            MarketUpdate::TickUpdate { pool_id, sqrt_price_x96, liquidity, .. } => {
                self.graph.update_clmm_reserves(*pool_id, *liquidity, *sqrt_price_x96);
            }
            _ => {}
        }

        // Resolve pool token vertices via O(1) lock-free lookup
        let (token0, token1) = match self.graph.get_pool_tokens(pool_id) {
            Some(pair) => pair,
            None => return Ok(None), // Untracked pool: exit cleanly
        };

        let feed_arrival_us = t_start.elapsed().as_micros() as u64;

        // 3. Targeted DFS Cycle Search for both token0 and token1 vertices
        let t_dfs = Instant::now();
        let mut candidates = Vec::with_capacity(16);
        dfs_cyclic_opportunities_into(
            &self.graph,
            token0,
            self.config.max_hops,
            0, // Morpho/Balancer zero fee
            &mut candidates,
        );
        if token1 != token0 {
            dfs_cyclic_opportunities_into(
                &self.graph,
                token1,
                self.config.max_hops,
                0,
                &mut candidates,
            );
        }
        let graph_dfs_us = t_dfs.elapsed().as_micros().max(1) as u64;

        self.telemetry.add_routes_screened(candidates.len() as u64);

        if candidates.is_empty() {
            return Ok(None);
        }

        let t_sizing = Instant::now();
        let best_candidate = candidates.into_iter().max_by_key(|c| c.net_profit).unwrap();
        // H8: Measure real elapsed duration of candidate selection & sizing math (no fake constant 15)
        let sizing_math_us = t_sizing.elapsed().as_micros().max(1) as u64;

        // 4. Honeypot check on borrow token
        if let Err(e) = verify_token_integrity(best_candidate.borrow_token) {
            warn!(token = ?best_candidate.borrow_token, "Discarding honeypot route: {:?}", e);
            return Err(PipelineError::Honeypot(format!("{:?}", e)));
        }

        // 5. In-Memory EVM Simulation (< 1.5ms)
        let t_sim = Instant::now();
        let sim_outcome = match self.simulator.simulate_arbitrage_route(
            &best_candidate,
            best_candidate.sizing.optimal_input,
        ) {
            Ok(outcome) => {
                self.kill_switch.record_trade_result(false);
                outcome
            }
            Err(SimError::FastRevertTriggered) => {
                self.kill_switch.record_trade_result(true);
                return Ok(None);
            }
            Err(SimError::Unprofitable { profit, gas_cost }) => {
                self.telemetry.inc_near_miss_spreads();

                let borrow_token = best_candidate.borrow_token;
                let decimals = flash_core::constants::get_token_decimals(self.simulator.chain, borrow_token);
                let opt_input = best_candidate.sizing.optimal_input;
                let gross_yield_raw = if profit > U256::ZERO { profit } else { best_candidate.sizing.gross_profit };

                let spread_bps = if !opt_input.is_zero() {
                    let num = gross_yield_raw.saturating_mul(U256::from(10_000u64));
                    num.checked_div(opt_input).unwrap_or(U256::ZERO).to::<u64>()
                } else {
                    0
                };

                let hop0 = &best_candidate.hops[0];
                let sym_in = self.get_symbol(hop0.token_in);
                let sym_out = self.get_symbol(hop0.token_out);
                let borrow_sym = self.get_symbol(borrow_token);
                let opt_input_f64 = opt_input.to::<u128>() as f64 / 10f64.powi(decimals as i32);

                let is_weth = borrow_token == flash_core::constants::arbitrum::WETH
                    || borrow_token == flash_core::constants::base::WETH;

                let gross_yield_eth_wei: u128 = if is_weth {
                    gross_yield_raw.to::<u128>()
                } else {
                    self.convert_token_profit_to_eth_wei(gross_yield_raw, borrow_token)
                };
                let gross_yield_usd = (gross_yield_eth_wei as f64 * 2500.0) / 1e18;

                let gas_cost_eth_wei: u128 = if is_weth {
                    gas_cost.to::<u128>()
                } else {
                    self.convert_token_profit_to_eth_wei(gas_cost, borrow_token)
                };
                let gas_threshold_usd = (gas_cost_eth_wei as f64 * 2500.0) / 1e18;

                info!(
                    "⚡ [SPREAD DETECTED - BELOW GAS] Pair: {}/{} | Spread: {} bps | Delta x*: {:.6} {} | Gross Yield: ${:.2} | Gas Threshold: ${:.2}",
                    sym_in, sym_out, spread_bps, opt_input_f64, borrow_sym, gross_yield_usd, gas_threshold_usd
                );

                return Ok(None);
            }
            Err(e) => return Err(PipelineError::Simulation(e)),
        };
        let revm_sim_us = t_sim.elapsed().as_micros() as u64;

        // 6. Multi-EOA Lease & Local Signing (< 50µs)
        let t_sign = Instant::now();
        let lease = self.signer_pool.acquire().ok_or(PipelineError::SignerPoolExhausted)?;

        let gas_limit = (sim_outcome.gas_used * 125 / 100).max(350_000);

        // H1 & V-12: Dynamic base fee and priority fee pricing
        let gas_prices = self.simulator.gas_prices();
        let base_fee_per_gas = gas_prices.l2_base_fee_wei;
        let min_priority_fee = gas_prices.l2_priority_fee_wei.max(50_000_000u128);  // >= 0.05 Gwei
        let max_priority_cap = 5_000_000_000u128; // 5 Gwei cap

        // V-13: Dynamic priority fee bidding for Arbitrum WETH, Base WETH, and non-WETH routes
        let is_weth = best_candidate.borrow_token == flash_core::constants::arbitrum::WETH
            || best_candidate.borrow_token == flash_core::constants::base::WETH;

        let profit_in_eth_wei: u128 = if is_weth {
            sim_outcome.actual_profit.to::<u128>()
        } else {
            self.convert_token_profit_to_eth_wei(
                sim_outcome.actual_profit,
                best_candidate.borrow_token,
            )
        };

        let priority_fee = if profit_in_eth_wei > 0 {
            let total_bribe = profit_in_eth_wei / 5; // 20% builder/sequencer bribe
            let per_gas_bribe = total_bribe / (gas_limit as u128);
            per_gas_bribe.clamp(min_priority_fee, max_priority_cap)
        } else {
            min_priority_fee
        };

        let max_fee_per_gas = (base_fee_per_gas * 2).saturating_add(priority_fee);

        // V-01: In live mode, send to configured executor contract (NEVER precompile 0x01)
        let target_to = if self.config.execution_mode.is_live() {
            self.config.executor_contract_address
        } else {
            self.simulator.executor_address
        };

        let tx_request = Eip1559TransactionRequest {
            chain_id: self.simulator.chain.id(),
            nonce: Some(lease.nonce()),
            max_priority_fee_per_gas: priority_fee,
            max_fee_per_gas,
            gas_limit,
            to: target_to,
            value: U256::ZERO,
            data: sim_outcome.calldata.clone(),
            access_list: vec![],
        };

        let signed_raw_tx = match self.signer_pool.sign_transaction(lease.index(), &tx_request) {
            Ok(tx) => tx,
            Err(e) => {
                lease.rollback_nonce();
                return Err(PipelineError::Signer(e));
            }
        };
        let tx_sign_us = t_sign.elapsed().as_micros() as u64;

        // 7. Record Microsecond Telemetry
        let telemetry = PipelineTelemetry {
            feed_arrival_us,
            graph_dfs_us,
            sizing_math_us,
            revm_sim_us,
            tx_sign_us,
        };
        self.telemetry.record(&telemetry);

        // Increment profitable hits and log bright banner
        self.telemetry.inc_profitable_hits();

        let borrow_token = best_candidate.borrow_token;
        let decimals = flash_core::constants::get_token_decimals(self.simulator.chain, borrow_token);
        let opt_input = best_candidate.sizing.optimal_input;
        let opt_input_f64 = opt_input.to::<u128>() as f64 / 10f64.powi(decimals as i32);
        let borrow_sym = self.get_symbol(borrow_token);

        let gross_profit_usd = (profit_in_eth_wei as f64 * 2500.0) / 1e18;
        let gas_cost_usd = (sim_outcome.gas_cost_wei.to::<u128>() as f64 * 2500.0) / 1e18;
        let net_profit_usd = gross_profit_usd - gas_cost_usd;

        let route_str = best_candidate
            .hops
            .iter()
            .map(|h| format!("{:#x}", h.pool_id))
            .collect::<Vec<_>>()
            .join(" -> ");

        info!(
            "\n================================================================================\n\
             🎯 [SHADOW ARBITRAGE HIT]\n\
             Chain:             {:?}\n\
             Route:             {}\n\
             Optimal Flash Loan Delta x*: {:.6} {}\n\
             Gross Profit ($):  ${:.2}\n\
             L1+L2 Gas Cost ($): ${:.2}\n\
             Net Profit ($):    ${:.2}\n\
             Latency Breakdown: DFS {} µs | Sizing {} µs | revm {} µs | Total {} µs\n\
             ================================================================================",
            self.simulator.chain,
            route_str,
            opt_input_f64,
            borrow_sym,
            gross_profit_usd,
            gas_cost_usd,
            net_profit_usd,
            graph_dfs_us,
            sizing_math_us,
            revm_sim_us,
            telemetry.total_e2e_latency_us()
        );

        // 8. Live Broadcast vs Shadow Paper-Trading (C7 Broadcaster)
        if self.config.execution_mode.is_live() {
            let rpc_endpoint = match self.simulator.chain {
                flash_core::types::Chain::ArbitrumOne => &self.config.arbitrum_rpc_http,
                flash_core::types::Chain::Base => &self.config.base_rpc_http,
            };
            let hex_tx = format!("0x{}", alloy_primitives::hex::encode(&signed_raw_tx.raw_bytes));
            let payload = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "eth_sendRawTransaction",
                "params": [hex_tx],
            });

            match self.http_client.post(rpc_endpoint).json(&payload).send().await {
                Ok(resp) => {
                    if let Ok(res_json) = resp.json::<serde_json::Value>().await {
                        if let Some(err) = res_json.get("error") {
                            warn!(
                                tx_hash = ?signed_raw_tx.tx_hash,
                                rpc_error = %err,
                                "Mempool rejected raw transaction broadcast"
                            );
                            lease.rollback_nonce();
                        } else if let Some(b_hash) = res_json.get("result").and_then(|r| r.as_str()) {
                            info!(
                                tx_hash = ?signed_raw_tx.tx_hash,
                                broadcast_hash = %b_hash,
                                sender = ?signed_raw_tx.sender,
                                nonce = signed_raw_tx.nonce,
                                "🚀 LIVE MEMPOOL BROADCAST CONFIRMED: tx accepted by node"
                            );
                        }
                    } else {
                        warn!(tx_hash = ?signed_raw_tx.tx_hash, "Failed to parse JSON RPC broadcast response");
                        lease.rollback_nonce();
                    }
                }
                Err(e) => {
                    warn!(
                        tx_hash = ?signed_raw_tx.tx_hash,
                        error = %e,
                        "Network failure during live raw transaction broadcast"
                    );
                    lease.rollback_nonce();
                }
            }
        } else {
            self.shadow_engine.register_simulated_opportunity(
                &best_candidate,
                &sim_outcome,
                telemetry,
                signed_raw_tx.raw_bytes.as_ref(),
                priority_fee,
                0,
            );
        }

        Ok(Some(telemetry))
    }

    /// Converts token profit (e.g. USDC, WBTC) into 18-decimal ETH wei for dynamic bribe sizing
    pub fn convert_token_profit_to_eth_wei(&self, profit: U256, token: Address) -> u128 {
        if profit.is_zero() {
            return 0;
        }

        let weth = match self.simulator.chain {
            Chain::ArbitrumOne => flash_core::constants::arbitrum::WETH,
            Chain::Base => flash_core::constants::base::WETH,
        };

        // Query MarketGraph for a token -> WETH edge
        let edges = self.graph.get_edges(token);
        for edge in edges.iter() {
            if edge.token_out == weth && edge.reserve_in > 0 {
                if let Some(converted) = flash_math::mul_div_u256(
                    profit,
                    U256::from(edge.reserve_out),
                    U256::from(edge.reserve_in),
                ) {
                    return converted.to::<u128>();
                }
            }
        }

        // Fallback: Decimal scaling based on known token constants and benchmark ETH prices ($2500/ETH)
        let decimals = flash_core::constants::get_token_decimals(self.simulator.chain, token);
        let eth_wei = match decimals {
            6 => {
                // USDC / USDT ($2500/ETH): profit_units * 10^12 / 2500
                profit
                    .saturating_mul(U256::from(1_000_000_000_000u64))
                    .checked_div(U256::from(2500))
                    .unwrap_or(U256::ZERO)
            }
            8 => {
                // WBTC / cbBTC (1 BTC = 25 ETH): satoshis * 10^18 / 4_000_000
                profit
                    .saturating_mul(U256::from(1_000_000_000_000_000_000u128))
                    .checked_div(U256::from(4_000_000))
                    .unwrap_or(U256::ZERO)
            }
            _ => profit,
        };

        eth_wei.to::<u128>()
    }

    /// Returns human-readable ticker symbol for standard tokens on Arbitrum One and Base
    pub fn get_symbol(&self, token: Address) -> &'static str {
        use flash_core::constants::{arbitrum, base};
        match self.simulator.chain {
            Chain::ArbitrumOne => {
                if token == arbitrum::WETH {
                    "WETH"
                } else if token == arbitrum::USDC_NATIVE {
                    "USDC"
                } else if token == arbitrum::USDC_BRIDGED {
                    "USDC.e"
                } else if token == arbitrum::USDT {
                    "USDT"
                } else if token == arbitrum::WBTC {
                    "WBTC"
                } else if token == arbitrum::CBBTC {
                    "cbBTC"
                } else if token == arbitrum::ARB {
                    "ARB"
                } else if token == arbitrum::GMX {
                    "GMX"
                } else if token == arbitrum::LINK {
                    "LINK"
                } else if token == arbitrum::UNI {
                    "UNI"
                } else if token == arbitrum::PENDLE {
                    "PENDLE"
                } else if token == arbitrum::DAI {
                    "DAI"
                } else if token == arbitrum::WSTETH {
                    "wstETH"
                } else if token == arbitrum::WEETH {
                    "weETH"
                } else {
                    "UNKNOWN"
                }
            }
            Chain::Base => {
                if token == base::WETH {
                    "WETH"
                } else if token == base::USDC_NATIVE {
                    "USDC"
                } else if token == base::USDBC_BRIDGED {
                    "USDbC"
                } else if token == base::USDT {
                    "USDT"
                } else if token == base::CBBTC {
                    "cbBTC"
                } else if token == base::WBTC {
                    "WBTC"
                } else if token == base::AERO {
                    "AERO"
                } else if token == base::DAI {
                    "DAI"
                } else if token == base::WSTETH {
                    "wstETH"
                } else if token == base::WEETH {
                    "weETH"
                } else if token == base::LINK {
                    "LINK"
                } else if token == base::UNI {
                    "UNI"
                } else if token == base::PENDLE {
                    "PENDLE"
                } else if token == base::CBETH {
                    "cbETH"
                } else if token == base::BRETT {
                    "BRETT"
                } else if token == base::DEGEN {
                    "DEGEN"
                } else if token == base::TOSHI {
                    "TOSHI"
                } else {
                    "UNKNOWN"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;
    use flash_core::constants::arbitrum;
    use flash_core::types::{FeePips, PoolState, Venue};
    use flash_scanner::discovery::DiscoveredPool;
    use flash_simulator::cache::BytecodeCache;

    #[tokio::test]
    async fn test_m3_pipeline_untracked_and_tracked_pool_resolution() {
        let chain = Chain::ArbitrumOne;
        let config = AppConfig {
            alchemy_key: "dummy".to_string(),
            arbitrum_rpc_ws: "".to_string(),
            arbitrum_rpc_http: "".to_string(),
            arbitrum_sequencer_feed: "".to_string(),
            base_rpc_ws: "".to_string(),
            base_rpc_http: "".to_string(),
            base_flashblocks_ws: "".to_string(),
            execution_mode: flash_core::types::ExecutionMode::Shadow,
            log_level: "info".to_string(),
            signer_private_keys: vec![],
            executor_contract_address: Address::ZERO,
            max_hops: 3,
            kill_switch_feed_lag_ms: 100,
            kill_switch_fast_revert_count: 3,
        };

        let graph = Arc::new(MarketGraph::new());
        let cache = Arc::new(BytecodeCache::new("./target/test_pipeline_cache", 10));
        let simulator = Arc::new(SimulationEngine::new(chain, cache));
        let signer_pool = Arc::new(SignerPool::new(1));
        let shadow_engine = Arc::new(ShadowEngine::new(chain, false));
        let kill_switch = Arc::new(KillSwitchController::default());
        let telemetry = Arc::new(TelemetryCollector::new());

        let pipeline = ArbitragePipeline::new(
            config,
            graph.clone(),
            simulator,
            signer_pool,
            shadow_engine,
            kill_switch,
            telemetry.clone(),
        );

        // 1. Untracked pool update must exit cleanly with Ok(None)
        let untracked_pool = address!("1111111111111111111111111111111111111111");
        let untracked_update = MarketUpdate::ReserveUpdate {
            chain,
            pool_id: untracked_pool,
            reserve0: 1_000_000,
            reserve1: 1_000_000,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };
        let res = pipeline.process_update(untracked_update).await;
        assert!(res.is_ok());
        assert!(res.unwrap().is_none());

        // 2. Tracked pool update: register pool in graph and verify resolution
        let pool_id = address!("2222222222222222222222222222222222222222");
        let pool = DiscoveredPool::new(
            pool_id,
            Venue::UniswapV2,
            chain,
            arbitrum::WETH,
            arbitrum::USDC_NATIVE,
            FeePips(3000),
            PoolState::ConstantProduct {
                reserve0: 10_000_000_000_000_000_000, // 10 WETH
                reserve1: 25_000_000_000,             // 25,000 USDC
            },
        );
        graph.add_pool(&pool);

        assert_eq!(
            graph.get_pool_tokens(pool_id),
            Some((arbitrum::WETH, arbitrum::USDC_NATIVE))
        );

        // Send reserve update for tracked pool
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let tracked_update = MarketUpdate::ReserveUpdate {
            chain,
            pool_id,
            reserve0: 10_500_000_000_000_000_000,
            reserve1: 24_500_000_000,
            timestamp_ms: now_ms,
        };

        let res_tracked = pipeline.process_update(tracked_update).await;
        assert!(res_tracked.is_ok());

        // Verify symbols resolution
        assert_eq!(pipeline.get_symbol(arbitrum::WETH), "WETH");
        assert_eq!(pipeline.get_symbol(arbitrum::USDC_NATIVE), "USDC");
        assert_eq!(pipeline.get_symbol(arbitrum::WBTC), "WBTC");
    }
}

