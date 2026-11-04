//! In-memory directed multigraph G(V, E, P) with O(1) in-place reserve updates.
//! Supports parallel pools between identical token pairs, CLMM virtual reserves,
//! and dirty vertex mutation tracking.
//! Authoritative source: ORIGINAL_REQUEST.md lines 24-26, 32-34, PROJECT.md lines 50, 87-90.

use alloy_primitives::{Address, U256};
use dashmap::DashMap;
use flash_core::types::{PoolReserves, PoolState, Venue};
use flash_math::virtual_reserves_from_clmm;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;

use crate::discovery::DiscoveredPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectedEdge {
    pub pool_id: Address,
    pub venue: Venue,
    pub token_in: Address,
    pub token_out: Address,
    pub reserve_in: u128,
    pub reserve_out: u128,
    pub fee_pips: u32,
    pub zero_for_one: bool,
}

impl DirectedEdge {
    #[inline(always)]
    pub fn to_pool_reserves(&self) -> PoolReserves {
        PoolReserves {
            reserve_in: self.reserve_in,
            reserve_out: self.reserve_out,
            fee_pips: self.fee_pips,
        }
    }
}

pub struct MarketGraph {
    /// Token address -> Contiguous vertex index (0..V)
    token_to_idx: DashMap<Address, usize>,
    /// Vertex index -> Token address
    idx_to_token: RwLock<Vec<Address>>,
    /// Directed adjacency list: vertex index -> list of outgoing edges
    adj: RwLock<Vec<Arc<[DirectedEdge]>>>,
    /// Pool ID -> [(vertex_idx, edge_idx_in_adj); 2] for O(1) reserve update
    pool_to_edges: DashMap<Address, [(usize, usize); 2]>,
    /// Pool ID -> (token0, token1) pair for O(1) lock-free token resolution
    pool_to_tokens: DashMap<Address, (Address, Address)>,
    /// Set of token vertices mutated since last search/drain
    mutated_vertices: RwLock<HashSet<Address>>,
}

impl Default for MarketGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl MarketGraph {
    pub fn new() -> Self {
        Self {
            token_to_idx: DashMap::new(),
            idx_to_token: RwLock::new(Vec::new()),
            adj: RwLock::new(Vec::new()),
            pool_to_edges: DashMap::new(),
            pool_to_tokens: DashMap::new(),
            mutated_vertices: RwLock::new(HashSet::new()),
        }
    }

    /// Retrieves or assigns a contiguous vertex index for the token
    fn get_or_create_vertex(&self, token: Address) -> usize {
        // Fast path: optimistic read from concurrent DashMap
        if let Some(idx) = self.token_to_idx.get(&token) {
            return *idx;
        }

        let mut idx_to_tok = self.idx_to_token.write();

        // Double check: re-verify after taking write lock to prevent duplicate vertex allocation
        if let Some(idx) = self.token_to_idx.get(&token) {
            return *idx;
        }

        let idx = idx_to_tok.len();
        idx_to_tok.push(token);

        let mut adj = self.adj.write();
        adj.push(Arc::from([]));
        drop(adj);

        self.token_to_idx.insert(token, idx);
        drop(idx_to_tok);

        idx
    }

    /// Adds a discovered pool into the directed multigraph, creating both forward & reverse edges
    pub fn add_pool(&self, pool: &DiscoveredPool) {
        if self.pool_to_edges.contains_key(&pool.id) {
            return;
        }

        // Step 1: Pre-register vertices BEFORE acquiring adj.write lock (preserves lock hierarchy: idx_to_token -> adj)
        let u = self.get_or_create_vertex(pool.token0);
        let v = self.get_or_create_vertex(pool.token1);

        let (r0, r1) = match &pool.state {
            PoolState::ConstantProduct { reserve0, reserve1 } => (*reserve0, *reserve1),
            PoolState::ConcentratedLiquidity {
                liquidity,
                sqrt_price_x96,
                ..
            } => virtual_reserves_from_clmm(*liquidity, *sqrt_price_x96),
        };

        let edge_0_to_1 = DirectedEdge {
            pool_id: pool.id,
            venue: pool.venue,
            token_in: pool.token0,
            token_out: pool.token1,
            reserve_in: r0,
            reserve_out: r1,
            fee_pips: pool.fee.0,
            zero_for_one: true,
        };

        let edge_1_to_0 = DirectedEdge {
            pool_id: pool.id,
            venue: pool.venue,
            token_in: pool.token1,
            token_out: pool.token0,
            reserve_in: r1,
            reserve_out: r0,
            fee_pips: pool.fee.0,
            zero_for_one: false,
        };

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
            self.pool_to_tokens
                .insert(pool.id, (pool.token0, pool.token1));
        }

        // Mark both vertices as mutated upon initial pool addition
        let mut mutated = self.mutated_vertices.write();
        mutated.insert(pool.token0);
        mutated.insert(pool.token1);
    }

    /// O(1) in-place reserve update for CPMM pools
    pub fn update_reserves(&self, pool_id: Address, reserve0: u128, reserve1: u128) -> bool {
        let edge_locs = match self.pool_to_edges.get(&pool_id) {
            Some(locs) => *locs,
            None => return false,
        };

        let [(u, idx_u), (v, idx_v)] = edge_locs;

        let (t0, t1) = {
            let mut adj = self.adj.write();

            // Edge 0->1: update reserves and extract token addresses directly without locking idx_to_token
            let (t0, t1) = if let Some(edges_u) = adj.get_mut(u) {
                if idx_u < edges_u.len() {
                    let slice = Arc::make_mut(edges_u);
                    slice[idx_u].reserve_in = reserve0;
                    slice[idx_u].reserve_out = reserve1;
                    (slice[idx_u].token_in, slice[idx_u].token_out)
                } else {
                    return false;
                }
            } else {
                return false;
            };

            // Edge 1->0: update reverse reserves
            if let Some(edges_v) = adj.get_mut(v) {
                if idx_v < edges_v.len() {
                    let slice = Arc::make_mut(edges_v);
                    slice[idx_v].reserve_in = reserve1;
                    slice[idx_v].reserve_out = reserve0;
                } else {
                    return false;
                }
            } else {
                return false;
            }

            (t0, t1)
        }; // adj write lock explicitly dropped here before acquiring mutated_vertices

        let mut mutated = self.mutated_vertices.write();
        mutated.insert(t0);
        mutated.insert(t1);

        true
    }

    /// O(1) in-place state update for CLMM pools with virtual reserve recalculation
    pub fn update_clmm_reserves(
        &self,
        pool_id: Address,
        liquidity: u128,
        sqrt_price_x96: U256,
    ) -> bool {
        let (r0, r1) = virtual_reserves_from_clmm(liquidity, sqrt_price_x96);
        self.update_reserves(pool_id, r0, r1)
    }

    /// Marks a specific token address as mutated
    pub fn mark_mutated(&self, token: Address) {
        self.mutated_vertices.write().insert(token);
    }

    /// Drains and returns all mutated vertices for targeted DFS search
    pub fn take_mutated_vertices(&self) -> Vec<Address> {
        let mut mutated = self.mutated_vertices.write();
        mutated.drain().collect()
    }

    /// Returns outgoing edges from a given token (zero heap allocation via Arc clone)
    #[inline(always)]
    pub fn get_edges(&self, token: Address) -> Arc<[DirectedEdge]> {
        let u = match self.token_to_idx.get(&token) {
            Some(entry) => *entry.value(),
            None => return Arc::from([]),
        };
        // DashMap shard read lock dropped above before acquiring adj.read lock to prevent ABBA deadlocks
        let adj = self.adj.read();
        if let Some(edges) = adj.get(u) {
            return Arc::clone(edges);
        }
        Arc::from([])
    }

    pub fn vertex_count(&self) -> usize {
        self.idx_to_token.read().len()
    }

    pub fn pool_count(&self) -> usize {
        self.pool_to_edges.len()
    }

    /// O(1) lock-free token pair resolution for a given pool
    #[inline(always)]
    pub fn get_pool_tokens(&self, pool_id: Address) -> Option<(Address, Address)> {
        if let Some(entry) = self.pool_to_tokens.get(&pool_id) {
            return Some(*entry.value());
        }
        // Fallback: recover tokens from idx_to_token if registered via edge pointers
        let locs = self.pool_to_edges.get(&pool_id)?;
        let [(u, _), (v, _)] = *locs;
        let idx_map = self.idx_to_token.read();
        let t0 = *idx_map.get(u)?;
        let t1 = *idx_map.get(v)?;
        Some((t0, t1))
    }

    #[inline(always)]
    pub fn has_pool(&self, pool_id: &Address) -> bool {
        self.pool_to_edges.contains_key(pool_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flash_core::types::{Chain, FeePips};

    #[test]
    fn test_graph_add_and_update() {
        let graph = MarketGraph::new();
        let pool = DiscoveredPool::new(
            Address::from([0x10; 20]),
            Venue::UniswapV2,
            Chain::ArbitrumOne,
            Address::from([0x01; 20]),
            Address::from([0x02; 20]),
            FeePips::from_bps(30),
            PoolState::ConstantProduct {
                reserve0: 1_000,
                reserve1: 2_000,
            },
        );

        graph.add_pool(&pool);
        assert_eq!(graph.vertex_count(), 2);
        assert_eq!(graph.pool_count(), 1);

        let mutated = graph.take_mutated_vertices();
        assert_eq!(mutated.len(), 2);
        assert!(graph.take_mutated_vertices().is_empty());

        let edges = graph.get_edges(Address::from([0x01; 20]));
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].reserve_in, 1_000);
        assert_eq!(edges[0].reserve_out, 2_000);

        // Update reserves in O(1)
        assert!(graph.update_reserves(pool.id, 1_500, 2_500));
        let edges = graph.get_edges(Address::from([0x01; 20]));
        assert_eq!(edges[0].reserve_in, 1_500);
        assert_eq!(edges[0].reserve_out, 2_500);
    }

    #[test]
    fn test_graph_get_pool_tokens() {
        let graph = MarketGraph::new();
        let pool = DiscoveredPool::new(
            Address::from([0x10; 20]),
            Venue::UniswapV2,
            Chain::ArbitrumOne,
            Address::from([0x01; 20]),
            Address::from([0x02; 20]),
            FeePips::from_bps(30),
            PoolState::ConstantProduct {
                reserve0: 1_000,
                reserve1: 2_000,
            },
        );

        assert_eq!(graph.get_pool_tokens(pool.id), None);
        assert!(!graph.has_pool(&pool.id));

        graph.add_pool(&pool);
        assert!(graph.has_pool(&pool.id));
        assert_eq!(
            graph.get_pool_tokens(pool.id),
            Some((Address::from([0x01; 20]), Address::from([0x02; 20])))
        );
        assert_eq!(graph.get_pool_tokens(Address::from([0x99; 20])), None);
    }
}
