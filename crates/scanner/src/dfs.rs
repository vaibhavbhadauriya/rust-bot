//! Targeted Depth-First Search cycle discovery with multiplication-only screening.
//! Searches cycles up to 3 hops starting strictly from mutated vertices.
//! Connects candidates into analytical closed-form solvers from flash-math.
//! Authoritative source: ORIGINAL_REQUEST.md lines 31-41, PROJECT.md lines 50, 87-91.

use alloy_primitives::{Address, U256, U512};
use flash_math::{
    quick_spread_screen, solve_2pool_optimal, solve_3pool_triangular, TradeSizeOutput,
};

use crate::graph::{DirectedEdge, MarketGraph};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteCandidate {
    pub borrow_token: Address,
    pub hops: Vec<DirectedEdge>,
    pub sizing: TradeSizeOutput,
    pub net_profit: U256,
}

/// 512-bit multiplication screening for 3-pool triangular arbitrage:
/// Checks iff (R_in1 * R_in2) * R_in3 < (R_out1 * R_out2) * R_out3
#[inline(always)]
pub fn quick_3pool_spread_screen(
    r_in1: u128,
    r_out1: u128,
    r_in2: u128,
    r_out2: u128,
    r_in3: u128,
    r_out3: u128,
) -> bool {
    let in_prod: U512 = U256::from(r_in1).widening_mul(U256::from(r_in2)) * U512::from(r_in3);
    let out_prod: U512 = U256::from(r_out1).widening_mul(U256::from(r_out2)) * U512::from(r_out3);
    in_prod < out_prod
}

/// Searches cyclic arbitrage opportunities up to max_hops (2 or 3) starting strictly from `mutated_token`,
/// appending discovered candidates into the provided `out` buffer without intermediate sorting.
pub fn dfs_cyclic_opportunities_into(
    graph: &MarketGraph,
    mutated_token: Address,
    max_hops: usize,
    flash_fee_pips: u32,
    out: &mut Vec<RouteCandidate>,
) {
    if max_hops < 2 {
        return;
    }

    let edges_1 = graph.get_edges(mutated_token);
    if edges_1.is_empty() {
        return;
    }

    for edge1 in edges_1.iter() {
        let token_b = edge1.token_out;
        if token_b == mutated_token {
            continue; // Ignore self-loop
        }

        let edges_2 = graph.get_edges(token_b);

        for edge2 in edges_2.iter() {
            // =========================================================
            // 2-Hop Cycle Detection: mutated_token -> B -> mutated_token
            // =========================================================
            if edge2.token_out == mutated_token {
                // Multigraph constraint: distinct pools
                if edge1.pool_id == edge2.pool_id {
                    continue;
                }

                // Multiplication-Only Spread Screen (< 10 ns):
                // Condition: R_in1 * R_in2 < R_out1 * R_out2
                // mapped to quick_spread_screen(edge2.reserve_in, edge1.reserve_in, edge1.reserve_out, edge2.reserve_out)
                if !quick_spread_screen(
                    edge2.reserve_in,
                    edge1.reserve_in,
                    edge1.reserve_out,
                    edge2.reserve_out,
                ) {
                    continue; // Prune unprofitable route
                }

                // Screen passed -> Solve exact closed-form optimal sizing
                let p1 = edge1.to_pool_reserves();
                let p2 = edge2.to_pool_reserves();

                if let Some(sizing) = solve_2pool_optimal(&p1, &p2, flash_fee_pips) {
                    if !sizing.net_profit.is_zero() {
                        out.push(RouteCandidate {
                            borrow_token: mutated_token,
                            hops: vec![*edge1, *edge2],
                            net_profit: sizing.net_profit,
                            sizing,
                        });
                    }
                }
                continue;
            }

            // =========================================================
            // 3-Hop Cycle Detection: mutated_token -> B -> C -> mutated_token
            // =========================================================
            if max_hops >= 3 {
                let token_c = edge2.token_out;
                if token_c == mutated_token || token_c == token_b {
                    continue;
                }
                if edge2.pool_id == edge1.pool_id {
                    continue;
                }

                let edges_3 = graph.get_edges(token_c);
                for edge3 in edges_3.iter() {
                    if edge3.token_out != mutated_token {
                        continue;
                    }

                    // Multigraph constraint: all 3 pools distinct
                    if edge3.pool_id == edge1.pool_id || edge3.pool_id == edge2.pool_id {
                        continue;
                    }

                    // 3-Hop Multiplication-Only Spread Screen (< 20 ns):
                    if !quick_3pool_spread_screen(
                        edge1.reserve_in,
                        edge1.reserve_out,
                        edge2.reserve_in,
                        edge2.reserve_out,
                        edge3.reserve_in,
                        edge3.reserve_out,
                    ) {
                        continue; // Prune unprofitable triangular route
                    }

                    // Screen passed -> Solve 3-pool triangular reduction
                    let p1 = edge1.to_pool_reserves();
                    let p2 = edge2.to_pool_reserves();
                    let p3 = edge3.to_pool_reserves();

                    if let Some(sizing) = solve_3pool_triangular(&p1, &p2, &p3, flash_fee_pips) {
                        if !sizing.net_profit.is_zero() {
                            out.push(RouteCandidate {
                                borrow_token: mutated_token,
                                hops: vec![*edge1, *edge2, *edge3],
                                net_profit: sizing.net_profit,
                                sizing,
                            });
                        }
                    }
                }
            }
        }
    }
}

/// Searches cyclic arbitrage opportunities up to max_hops (2 or 3) starting strictly from `mutated_token`
pub fn dfs_cyclic_opportunities(
    graph: &MarketGraph,
    mutated_token: Address,
    max_hops: usize,
    flash_fee_pips: u32,
) -> Vec<RouteCandidate> {
    let mut candidates = Vec::new();
    dfs_cyclic_opportunities_into(graph, mutated_token, max_hops, flash_fee_pips, &mut candidates);
    // Sort opportunities by highest net profit descending
    candidates.sort_by(|a, b| b.net_profit.cmp(&a.net_profit));
    candidates
}

/// Drains all mutated vertices from the graph and evaluates cyclic opportunities with a single final sort
pub fn scan_mutated_opportunities(
    graph: &MarketGraph,
    max_hops: usize,
    flash_fee_pips: u32,
) -> Vec<RouteCandidate> {
    let mutated = graph.take_mutated_vertices();
    let mut all_routes = Vec::new();
    for token in mutated {
        dfs_cyclic_opportunities_into(graph, token, max_hops, flash_fee_pips, &mut all_routes);
    }
    // Single descending sort over entire combined set; NO redundant intermediate sorting
    all_routes.sort_by(|a, b| b.net_profit.cmp(&a.net_profit));
    all_routes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::DiscoveredPool;
    use flash_core::types::{Chain, FeePips, PoolState, Venue};

    #[test]
    fn test_3pool_spread_screen() {
        // Profitable 3-hop: (100 * 200 * 300) = 6_000_000 < (110 * 210 * 310) = 7_161_000
        assert!(quick_3pool_spread_screen(100, 110, 200, 210, 300, 310));
        // Unprofitable: inputs >= outputs
        assert!(!quick_3pool_spread_screen(100, 90, 200, 200, 300, 300));
    }

    #[test]
    fn test_2pool_arbitrage_discovery() {
        let graph = MarketGraph::new();
        let token_a = Address::from([0x01; 20]);
        let token_b = Address::from([0x02; 20]);

        let d_a = 1_000_000u128; // 6 decimals (e.g. USDC)
        let d_b = 1_000_000_000_000_000_000u128; // 18 decimals (e.g. WETH)

        // Pool 1: 1M token A, 405 token B (f=3000)
        let p1 = DiscoveredPool::new(
            Address::from([0x11; 20]),
            Venue::UniswapV2,
            Chain::ArbitrumOne,
            token_a,
            token_b,
            FeePips::from_bps(30),
            PoolState::ConstantProduct {
                reserve0: 1_000_000 * d_a,
                reserve1: 405 * d_b,
            },
        );
        // Pool 2: 1M token A, 395 token B (f=3000)
        let p2 = DiscoveredPool::new(
            Address::from([0x22; 20]),
            Venue::SushiSwap,
            Chain::ArbitrumOne,
            token_a,
            token_b,
            FeePips::from_bps(30),
            PoolState::ConstantProduct {
                reserve0: 1_000_000 * d_a,
                reserve1: 395 * d_b,
            },
        );

        graph.add_pool(&p1);
        graph.add_pool(&p2);

        let candidates = dfs_cyclic_opportunities(&graph, token_a, 2, 0);
        assert!(!candidates.is_empty(), "Should discover 2-pool arbitrage");
        assert!(candidates[0].net_profit > U256::ZERO);
    }
}
