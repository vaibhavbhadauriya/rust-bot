//! 16-Zero Vanity CREATE2 Salt Miner
//!
//! Authoritative references:
//! - ORIGINAL_REQUEST.md lines 53-54: "16-zero vanity Pure Yul executor (8 leading zero bytes: 0x0000000000000000... to save 96 calldata gas per appearance)."
//! - PROJECT.md line 99: "mine_vanity_salt(deployer: Address, init_code: &[u8], target_zeros: usize) -> (B256, Address)"
//! - EIP-1014: `keccak256( 0xff ++ address ++ salt ++ keccak256(init_code) )[12..32]`
//! - EIP-2028: Calldata gas cost = 4 gas for zero byte, 16 gas for non-zero byte (savings = 12 gas/byte).

use alloy_primitives::{keccak256, Address, B256};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// CREATE2 prefix byte as defined by EIP-1014 (0xff).
pub const CREATE2_PREFIX: u8 = 0xff;

/// Total length of the CREATE2 preimage buffer:
/// 1 byte prefix + 20 bytes deployer + 32 bytes salt + 32 bytes init_code_hash = 85 bytes.
pub const CREATE2_PREIMAGE_LEN: usize = 1 + 20 + 32 + 32;

/// Gas cost of a non-zero byte in calldata under EIP-2028.
pub const GAS_PER_NONZERO_BYTE: u64 = 16;

/// Gas cost of a zero byte (0x00) in calldata under EIP-2028.
pub const GAS_PER_ZERO_BYTE: u64 = 4;

/// Gas savings per zero byte in calldata: 16 - 4 = 12 gas.
pub const GAS_SAVINGS_PER_ZERO_BYTE: u64 = GAS_PER_NONZERO_BYTE - GAS_PER_ZERO_BYTE;

/// Target zero bytes for 16 hex zeros: 16 / 2 = 8 zero bytes.
pub const VANITY_16_HEX_ZEROS_BYTES: usize = 8;

/// Expected gas savings per calldata appearance for a 16-hex-zero address:
/// 8 zero bytes * 12 gas/byte = 96 gas.
pub const VANITY_CALDATA_GAS_SAVINGS: u64 = (VANITY_16_HEX_ZEROS_BYTES as u64) * GAS_SAVINGS_PER_ZERO_BYTE;

/// Default iteration batch size before polling cancellation / termination atomic flag.
pub const DEFAULT_BATCH_SIZE: u64 = 4096;

/// Vanity mining error conditions.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VanityMinerError {
    #[error("Mining operation was cancelled via cancellation token")]
    Cancelled,
    #[error("Max iterations ({0}) exceeded without finding vanity address satisfying target")]
    MaxIterationsExceeded(u64),
    #[error("Invalid target zero bytes: {0} (maximum address size is 20 bytes)")]
    InvalidTarget(usize),
}

/// Statistics and result of a successful vanity mining search.
#[derive(Debug, Clone, PartialEq)]
pub struct MiningResult {
    /// Discovered 32-byte salt.
    pub salt: [u8; 32],
    /// Discovered salt represented as alloy B256.
    pub salt_b256: B256,
    /// Derived CREATE2 contract address.
    pub address: Address,
    /// Number of leading zero bytes achieved in the address (e.g. >= 8 for 16 hex zeros).
    pub zero_bytes: usize,
    /// Number of leading hex zeros achieved (zero_bytes * 2 or more).
    pub hex_zeros: usize,
    /// Total hash iterations evaluated across all worker threads.
    pub attempts: u64,
    /// Elapsed wall-clock duration of the mining operation.
    pub elapsed: Duration,
    /// Effective hash rate in hashes per second.
    pub hash_rate: f64,
}

/// High-throughput parallel CREATE2 vanity salt miner.
#[derive(Debug, Clone)]
pub struct VanityMiner {
    /// Deployer factory or contract address.
    deployer: Address,
    /// 32-byte Keccak-256 hash of the contract creation init code.
    init_code_hash: [u8; 32],
    /// Target count of leading zero bytes in derived address (default: 8, meaning 16 hex zeros).
    target_zero_bytes: usize,
    /// Number of worker threads to allocate for parallel search (0 = auto-detect hardware concurrency).
    thread_count: usize,
    /// Optional limit on total iterations across all threads.
    max_iterations: Option<u64>,
    /// Number of inner iterations per worker before polling the atomic stop flag.
    batch_size: u64,
    /// Additional 64-bit entropy seed to ensure unique salt search spaces across runs.
    worker_entropy: u64,
    /// Optional external cancellation token.
    cancellation_token: Option<Arc<AtomicBool>>,
}

impl VanityMiner {
    /// Create a new miner with deployer address and precomputed init code hash.
    pub fn new(deployer: Address, init_code_hash: [u8; 32]) -> Self {
        Self {
            deployer,
            init_code_hash,
            target_zero_bytes: VANITY_16_HEX_ZEROS_BYTES,
            thread_count: 0, // Auto-detect
            max_iterations: None,
            batch_size: DEFAULT_BATCH_SIZE,
            worker_entropy: 0xCAFE_BABE_DEAD_BEEF,
            cancellation_token: None,
        }
    }

    /// Create a new miner from raw bytecode by computing `keccak256(init_code)`.
    pub fn from_init_code(deployer: Address, init_code: &[u8]) -> Self {
        let hash = keccak256(init_code);
        Self::new(deployer, hash.0)
    }

    /// Set target leading zero bytes (e.g. 8 for 16 hex zeros, or 2 for instant test runs).
    pub fn with_target_zero_bytes(mut self, target: usize) -> Self {
        self.target_zero_bytes = target;
        self
    }

    /// Set target leading hex zeros (converts to zero bytes: e.g. 16 hex zeros -> 8 zero bytes).
    pub fn with_target_hex_zeros(mut self, hex_zeros: usize) -> Self {
        self.target_zero_bytes = (hex_zeros + 1) / 2;
        self
    }

    /// Set specific worker thread count (0 = auto-detect CPU cores).
    pub fn with_threads(mut self, threads: usize) -> Self {
        self.thread_count = threads;
        self
    }

    /// Set maximum cumulative iterations before timing out.
    pub fn with_max_iterations(mut self, max: u64) -> Self {
        self.max_iterations = Some(max);
        self
    }

    /// Set cancellation token for external shutdown / graceful abort.
    pub fn with_cancellation_token(mut self, token: Arc<AtomicBool>) -> Self {
        self.cancellation_token = Some(token);
        self
    }

    /// Set iteration batch size between cancellation checks.
    pub fn with_batch_size(mut self, batch_size: u64) -> Self {
        self.batch_size = if batch_size == 0 { DEFAULT_BATCH_SIZE } else { batch_size };
        self
    }

    /// Set custom worker entropy seed.
    pub fn with_worker_entropy(mut self, entropy: u64) -> Self {
        self.worker_entropy = entropy;
        self
    }

    /// Resolve effective thread count.
    fn effective_threads(&self) -> usize {
        if self.thread_count > 0 {
            self.thread_count
        } else {
            std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(4)
        }
    }

    /// Execute vanity salt search. Automatically uses multi-threading when thread count > 1.
    pub fn mine(&self) -> Result<MiningResult, VanityMinerError> {
        if self.target_zero_bytes > 20 {
            return Err(VanityMinerError::InvalidTarget(self.target_zero_bytes));
        }

        let threads = self.effective_threads();
        if threads <= 1 {
            self.mine_single_threaded()
        } else {
            self.mine_multi_threaded(threads)
        }
    }

    /// Single-threaded mining execution for deterministic testing and low-overhead searches.
    pub fn mine_single_threaded(&self) -> Result<MiningResult, VanityMinerError> {
        let start = Instant::now();
        let mut buffer = [0u8; CREATE2_PREIMAGE_LEN];
        buffer[0] = CREATE2_PREFIX;
        buffer[1..21].copy_from_slice(self.deployer.as_slice());
        buffer[53..85].copy_from_slice(&self.init_code_hash);

        // Salt structure:
        // bytes 21..29: worker entropy
        // bytes 29..37: 0
        // bytes 37..45: 0
        // bytes 45..53: 64-bit local nonce
        buffer[21..29].copy_from_slice(&self.worker_entropy.to_le_bytes());

        let mut nonce = 0u64;
        let max_iters = self.max_iterations.unwrap_or(u64::MAX);
        let batch_size = self.batch_size;

        loop {
            // Update 64-bit nonce slice directly in pre-allocated preimage buffer
            buffer[45..53].copy_from_slice(&nonce.to_le_bytes());

            let hash = keccak256(buffer);
            if has_leading_zero_bytes(&hash.0, self.target_zero_bytes) {
                let address = extract_address_from_hash(&hash.0);
                let zero_bytes = count_leading_zero_bytes(&address);
                if zero_bytes >= self.target_zero_bytes {
                    let mut salt = [0u8; 32];
                    salt.copy_from_slice(&buffer[21..53]);
                    let elapsed = start.elapsed();
                    let attempts = nonce + 1;
                    let hash_rate = (attempts as f64) / elapsed.as_secs_f64().max(1e-9);

                    return Ok(MiningResult {
                        salt,
                        salt_b256: B256::from(salt),
                        address,
                        zero_bytes,
                        hex_zeros: count_leading_hex_zeros(&address),
                        attempts,
                        elapsed,
                        hash_rate,
                    });
                }
            }

            nonce = nonce.wrapping_add(1);

            // Periodic cancellation and iteration check
            if nonce % batch_size == 0 {
                if let Some(token) = &self.cancellation_token {
                    if token.load(Ordering::Relaxed) {
                        return Err(VanityMinerError::Cancelled);
                    }
                }
                if nonce >= max_iters {
                    return Err(VanityMinerError::MaxIterationsExceeded(nonce));
                }
            }
        }
    }

    /// Parallel multi-threaded mining execution using disjoint worker nonce spaces.
    pub fn mine_multi_threaded(&self, num_threads: usize) -> Result<MiningResult, VanityMinerError> {
        let start = Instant::now();
        let found_flag = Arc::new(AtomicBool::new(false));
        let total_attempts = Arc::new(AtomicU64::new(0));
        let solution_holder = Arc::new(parking_lot::Mutex::new(None::<MiningResult>));

        let max_per_thread = self.max_iterations.map(|m| m / (num_threads as u64) + 1);

        let mut handles = Vec::with_capacity(num_threads);

        for worker_id in 0..num_threads {
            let deployer = self.deployer;
            let init_code_hash = self.init_code_hash;
            let target_zero_bytes = self.target_zero_bytes;
            let batch_size = self.batch_size;
            let worker_entropy = self.worker_entropy;
            let found_flag = Arc::clone(&found_flag);
            let total_attempts = Arc::clone(&total_attempts);
            let solution_holder = Arc::clone(&solution_holder);
            let cancel_token = self.cancellation_token.clone();

            let handle = std::thread::Builder::new()
                .name(format!("vanity-miner-{}", worker_id))
                .spawn(move || {
                    let mut buffer = [0u8; CREATE2_PREIMAGE_LEN];
                    buffer[0] = CREATE2_PREFIX;
                    buffer[1..21].copy_from_slice(deployer.as_slice());
                    buffer[53..85].copy_from_slice(&init_code_hash);

                    // Disjoint salt partitioning:
                    // bytes 21..29: worker entropy
                    // bytes 29..37: worker_id (guarantees disjoint salt space across threads)
                    // bytes 37..45: thread start offset
                    // bytes 45..53: 64-bit local worker nonce
                    buffer[21..29].copy_from_slice(&worker_entropy.to_le_bytes());
                    buffer[29..37].copy_from_slice(&(worker_id as u64).to_le_bytes());

                    let mut local_nonce = 0u64;
                    let max_iters = max_per_thread.unwrap_or(u64::MAX);

                    loop {
                        buffer[45..53].copy_from_slice(&local_nonce.to_le_bytes());

                        let hash = keccak256(buffer);
                        if has_leading_zero_bytes(&hash.0, target_zero_bytes) {
                            let address = extract_address_from_hash(&hash.0);
                            let zero_bytes = count_leading_zero_bytes(&address);
                            if zero_bytes >= target_zero_bytes {
                                // First thread to signal success claims solution
                                if !found_flag.swap(true, Ordering::SeqCst) {
                                    let mut salt = [0u8; 32];
                                    salt.copy_from_slice(&buffer[21..53]);
                                    let elapsed = start.elapsed();
                                    total_attempts.fetch_add(local_nonce + 1, Ordering::Relaxed);
                                    let cumulative = total_attempts.load(Ordering::Relaxed);
                                    let hash_rate = (cumulative as f64) / elapsed.as_secs_f64().max(1e-9);

                                    let result = MiningResult {
                                        salt,
                                        salt_b256: B256::from(salt),
                                        address,
                                        zero_bytes,
                                        hex_zeros: count_leading_hex_zeros(&address),
                                        attempts: cumulative,
                                        elapsed,
                                        hash_rate,
                                    };
                                    *solution_holder.lock() = Some(result);
                                }
                                break;
                            }
                        }

                        local_nonce = local_nonce.wrapping_add(1);

                        if local_nonce % batch_size == 0 {
                            if found_flag.load(Ordering::Relaxed) {
                                total_attempts.fetch_add(batch_size, Ordering::Relaxed);
                                break;
                            }
                            if let Some(token) = &cancel_token {
                                if token.load(Ordering::Relaxed) {
                                    break;
                                }
                            }
                            if local_nonce >= max_iters {
                                break;
                            }
                            total_attempts.fetch_add(batch_size, Ordering::Relaxed);
                        }
                    }
                })
                .expect("Failed to spawn vanity miner worker thread");

            handles.push(handle);
        }

        // Wait for all worker threads to complete
        for handle in handles {
            let _ = handle.join();
        }

        let maybe_solution = solution_holder.lock().take();
        if let Some(res) = maybe_solution {
            Ok(res)
        } else if let Some(token) = &self.cancellation_token {
            if token.load(Ordering::Relaxed) {
                Err(VanityMinerError::Cancelled)
            } else {
                let attempts = total_attempts.load(Ordering::Relaxed);
                Err(VanityMinerError::MaxIterationsExceeded(attempts))
            }
        } else {
            let attempts = total_attempts.load(Ordering::Relaxed);
            Err(VanityMinerError::MaxIterationsExceeded(attempts))
        }
    }
}

/// Compute CREATE2 address deterministically from deployer, salt, and init_code_hash.
/// Formula: `keccak256(0xff ++ deployer ++ salt ++ init_code_hash)[12..32]`.
#[inline]
pub fn compute_create2_address(deployer: Address, salt: [u8; 32], init_code_hash: [u8; 32]) -> Address {
    let mut buffer = [0u8; CREATE2_PREIMAGE_LEN];
    buffer[0] = CREATE2_PREFIX;
    buffer[1..21].copy_from_slice(deployer.as_slice());
    buffer[21..53].copy_from_slice(&salt);
    buffer[53..85].copy_from_slice(&init_code_hash);

    let hash = keccak256(buffer);
    extract_address_from_hash(&hash.0)
}

/// Compute CREATE2 address from unhashed init code bytecode.
#[inline]
pub fn compute_create2_address_from_code(deployer: Address, salt: [u8; 32], init_code: &[u8]) -> Address {
    let hash = keccak256(init_code);
    compute_create2_address(deployer, salt, hash.0)
}

/// Extract 20-byte EVM address from indices 12..32 of a 32-byte Keccak-256 hash.
#[inline(always)]
pub fn extract_address_from_hash(hash: &[u8; 32]) -> Address {
    let mut addr_bytes = [0u8; 20];
    addr_bytes.copy_from_slice(&hash[12..32]);
    Address::from(addr_bytes)
}

/// Fast branchless / integer check for leading zero bytes directly on raw hash.
/// Indices 12..32 of the hash correspond to the address.
#[inline(always)]
pub fn has_leading_zero_bytes(hash: &[u8; 32], target_zero_bytes: usize) -> bool {
    let addr_slice = &hash[12..32];
    if target_zero_bytes > addr_slice.len() {
        return false;
    }
    match target_zero_bytes {
        0 => true,
        1 => addr_slice[0] == 0,
        2 => u16::from_ne_bytes(addr_slice[0..2].try_into().unwrap()) == 0,
        3 => u16::from_ne_bytes(addr_slice[0..2].try_into().unwrap()) == 0 && addr_slice[2] == 0,
        4 => u32::from_ne_bytes(addr_slice[0..4].try_into().unwrap()) == 0,
        5 => u32::from_ne_bytes(addr_slice[0..4].try_into().unwrap()) == 0 && addr_slice[4] == 0,
        6 => {
            u32::from_ne_bytes(addr_slice[0..4].try_into().unwrap()) == 0
                && u16::from_ne_bytes(addr_slice[4..6].try_into().unwrap()) == 0
        }
        7 => {
            u32::from_ne_bytes(addr_slice[0..4].try_into().unwrap()) == 0
                && u16::from_ne_bytes(addr_slice[4..6].try_into().unwrap()) == 0
                && addr_slice[6] == 0
        }
        8 => u64::from_ne_bytes(addr_slice[0..8].try_into().unwrap()) == 0,
        n => {
            if n > addr_slice.len() {
                return false;
            }
            addr_slice[..n].iter().all(|&b| b == 0)
        }
    }
}

/// Count contiguous leading zero bytes in an address.
#[inline(always)]
pub fn count_leading_zero_bytes(addr: &Address) -> usize {
    addr.as_slice().iter().take_while(|&&b| b == 0).count()
}

/// Count leading hex zeros in an address (e.g. byte 0x05 has 1 hex zero, byte 0x00 has 2).
pub fn count_leading_hex_zeros(addr: &Address) -> usize {
    let mut hex_zeros = 0;
    for &byte in addr.as_slice() {
        if byte == 0 {
            hex_zeros += 2;
        } else if byte < 0x10 {
            hex_zeros += 1;
            break;
        } else {
            break;
        }
    }
    hex_zeros
}

/// Calculate calldata gas savings for a given number of zero bytes under EIP-2028:
/// Savings = zero_bytes * (16 - 4) = zero_bytes * 12 gas.
#[inline(always)]
pub fn calculate_calldata_gas_savings(zero_bytes: usize) -> u64 {
    (zero_bytes as u64) * GAS_SAVINGS_PER_ZERO_BYTE
}

/// Canonical PROJECT.md public interface contract:
/// `mine_vanity_salt(deployer: Address, init_code: &[u8], target_zeros: usize) -> (B256, Address)`
///
/// Automatically handles zero bytes vs hex zeros:
/// - If `target_zeros == 16`: targets 16 hex zeros (8 zero bytes).
/// - If `target_zeros <= 8`: targets `target_zeros` zero bytes.
/// - If `8 < target_zeros < 16`: normalizes as `(target_zeros + 1) / 2` zero bytes.
pub fn mine_vanity_salt(deployer: Address, init_code: &[u8], target_zeros: usize) -> (B256, Address) {
    let target_zero_bytes = if target_zeros == 16 {
        8
    } else if target_zeros <= 8 {
        target_zeros
    } else {
        (target_zeros + 1) / 2
    };

    let miner = VanityMiner::from_init_code(deployer, init_code)
        .with_target_zero_bytes(target_zero_bytes);

    let result = miner.mine().expect("Vanity mining failed");
    (result.salt_b256, result.address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create2_derivation_determinism() {
        let deployer = Address::from([0x11; 20]);
        let salt = [0u8; 32];
        let init_code_hash = [0xAA; 32];

        let addr1 = compute_create2_address(deployer, salt, init_code_hash);
        let addr2 = compute_create2_address(deployer, salt, init_code_hash);

        assert_eq!(addr1, addr2);
        assert_ne!(addr1, Address::ZERO);
    }

    #[test]
    fn test_calldata_gas_savings_values() {
        assert_eq!(calculate_calldata_gas_savings(8), 96);
        assert_eq!(calculate_calldata_gas_savings(0), 0);
        assert_eq!(calculate_calldata_gas_savings(1), 12);
    }

    #[test]
    fn test_count_leading_hex_zeros() {
        let mut bytes = [0xff; 20];
        bytes[0] = 0x00;
        bytes[1] = 0x07;
        let addr = Address::from(bytes);
        assert_eq!(count_leading_hex_zeros(&addr), 3); // 2 from byte 0, 1 from byte 1 (0x07 < 0x10)
    }

    #[test]
    fn test_mine_vanity_target_2_bytes_multi_threaded() {
        let deployer = Address::from([0x77; 20]);
        let init_code_hash = [0x88; 32];

        let miner = VanityMiner::new(deployer, init_code_hash)
            .with_target_zero_bytes(2)
            .with_threads(4)
            .with_batch_size(1024);

        let res = miner.mine().expect("Mining 2 zero bytes should succeed quickly");
        assert!(res.zero_bytes >= 2);
        assert_eq!(compute_create2_address(deployer, res.salt, init_code_hash), res.address);
    }

    #[test]
    fn test_cancellation_token() {
        let deployer = Address::from([0x77; 20]);
        let init_code_hash = [0x88; 32];
        let token = Arc::new(AtomicBool::new(false));

        // Trigger cancellation immediately
        token.store(true, Ordering::SeqCst);

        let miner = VanityMiner::new(deployer, init_code_hash)
            .with_target_zero_bytes(8)
            .with_threads(2)
            .with_batch_size(128)
            .with_cancellation_token(token);

        let res = miner.mine();
        assert_eq!(res.unwrap_err(), VanityMinerError::Cancelled);
    }

    #[test]
    fn test_has_leading_zero_bytes_out_of_bounds_no_panic() {
        let hash = [0u8; 32];
        // Slicing past 20 bytes (21, 32, 100) must safely return false without panicking
        assert!(!has_leading_zero_bytes(&hash, 21));
        assert!(!has_leading_zero_bytes(&hash, 32));
        assert!(!has_leading_zero_bytes(&hash, 100));
        // Valid bounds check
        assert!(has_leading_zero_bytes(&hash, 20));
        assert!(has_leading_zero_bytes(&hash, 8));
        assert!(has_leading_zero_bytes(&hash, 0));
    }
}
