//! High-performance Whole-Market Scanner, Dynamic Top-100 Scoring & In-Memory Multigraph.
//! Sub-crate providing:
//! - Pre-filters (hard reserve unit filters, honeypot & token security)
//! - Dynamic 60-second Sp pool/pair scoring, top-100 heap, and instant burst promotion
//! - Multi-DEX pool discovery across Arbitrum One and Base
//! - Directed multigraph G(V, E, P) with O(1) in-place reserve updates
//! - Targeted DFS cycle search up to 3 hops starting strictly from mutated vertices

pub mod dfs;
pub mod discovery;
pub mod filters;
pub mod graph;
pub mod scoring;

pub use dfs::{
    dfs_cyclic_opportunities, dfs_cyclic_opportunities_into, quick_3pool_spread_screen,
    scan_mutated_opportunities, RouteCandidate,
};
pub use discovery::{
    build_get_pair_call, build_get_pool_slipstream_call, build_get_pool_v3_call,
    build_get_reserves_call, build_liquidity_call, build_slot0_call, build_tick_bitmap_call,
    decode_address_word, decode_aggregate3_results, decode_liquidity_return,
    decode_reserves_return, decode_slipstream_pool_created, decode_slot0_return,
    decode_tick_bitmap_return, decode_v2_pair_created, decode_v3_pool_created, decode_v4_initialize,
    encode_aggregate3, Call3, DiscoveredPool, DiscoveryError, Multicall3Discovery,
    MulticallResult, PoolRegistry, TopPairMetadata, BALANCER_V3_POOL_REGISTERED_TOPIC0,
    MULTICALL3_ADDRESS, PAIR_CREATED_TOPIC0, SLIPSTREAM_POOL_CREATED_TOPIC0,
    TICK_BITMAP_SELECTOR, UNI_V3_POOL_CREATED_TOPIC0, UNI_V4_INITIALIZE_TOPIC0,
};
pub use filters::{
    filter_unit_reserves, inspect_honeypot_safety, HoneypotFilter, HoneypotSimulationReport,
    HoneypotViolation,
};
pub use graph::{DirectedEdge, MarketGraph};
pub use scoring::{
    compute_sp_score, PairId, PairTracker, Rolling60sWindow, ScoredPair, TimeBucket, Top100Ranker,
    VenueMetric,
};
