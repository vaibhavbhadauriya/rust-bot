//! Multi-EOA Rotating Signer Pool & Arbitrum Nonce Cache Bypass.
//!
//! Authoritative source: ORIGINAL_REQUEST.md lines 69-70; PROJECT.md § Milestone 5 Feature 19.

use alloy_primitives::{keccak256, Address, Bytes, B256, U256};
use alloy_rlp::{Encodable, Header};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SignerError {
    #[error("Invalid private key format: {0}")]
    InvalidPrivateKey(String),
    #[error("Signer pool exhausted: all {0} signers are currently leased")]
    PoolExhausted(usize),
    #[error("Signer index {0} out of bounds (capacity: {1})")]
    IndexOutOfBounds(usize, usize),
    #[error("Signing failed: {0}")]
    SigningFailed(String),
    #[error("RLP encoding failed: {0}")]
    RlpError(String),
}

/// Individual pre-funded EOA signer holding key material and local monotonic nonce.
pub struct EoaSigner {
    pub index: usize,
    pub address: Address,
    private_key: [u8; 32],
    nonce: AtomicU64,
}

impl EoaSigner {
    pub fn new(index: usize, private_key: [u8; 32]) -> Result<Self, SignerError> {
        let signing_key = k256::ecdsa::SigningKey::from_bytes((&private_key).into())
            .map_err(|e| SignerError::InvalidPrivateKey(e.to_string()))?;
        let verifying_key = signing_key.verifying_key();
        let uncompressed = verifying_key.to_encoded_point(false);
        let hash = keccak256(&uncompressed.as_bytes()[1..65]);
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&hash[12..32]);
        let address = Address::from(addr);

        Ok(Self {
            index,
            address,
            private_key,
            nonce: AtomicU64::new(0),
        })
    }

    /// Fast deterministic mock constructor for testing (matches MockEoaPool with genuine key derivation).
    pub fn mock(index: usize) -> Self {
        let mut private_key = [0u8; 32];
        private_key[31] = (index + 1) as u8;
        Self::new(index, private_key).expect("Mock key is non-zero valid private key")
    }

    #[inline(always)]
    pub fn address(&self) -> Address {
        self.address
    }

    #[inline(always)]
    pub fn current_nonce(&self) -> u64 {
        self.nonce.load(Ordering::SeqCst)
    }

    #[inline(always)]
    pub fn reserve_nonce(&self) -> u64 {
        self.nonce.fetch_add(1, Ordering::SeqCst)
    }

    /// Rolls back the reserved nonce if broadcast fails or transaction is aborted.
    #[inline(always)]
    pub fn rollback_nonce(&self, expected_nonce: u64) {
        let _ = self.nonce.compare_exchange(
            expected_nonce + 1,
            expected_nonce,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }

    #[inline(always)]
    pub fn sync_nonce(&self, on_chain_nonce: u64) {
        self.nonce.fetch_max(on_chain_nonce, Ordering::SeqCst);
    }

    #[inline(always)]
    pub fn set_nonce(&self, nonce: u64) {
        self.nonce.store(nonce, Ordering::SeqCst);
    }

    #[inline(always)]
    pub fn private_key(&self) -> &[u8; 32] {
        &self.private_key
    }
}

/// EIP-1559 Transaction Request Structure
#[derive(Debug, Clone)]
pub struct Eip1559TransactionRequest {
    pub chain_id: u64,
    pub nonce: Option<u64>,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub gas_limit: u64,
    pub to: Address,
    pub value: U256,
    pub data: Bytes,
    pub access_list: Vec<(Address, Vec<B256>)>,
}

/// Signed Raw Transaction ready for mempool broadcast
#[derive(Debug, Clone)]
pub struct SignedRawTransaction {
    pub raw_bytes: Bytes,
    pub tx_hash: B256,
    pub sender: Address,
    pub nonce: u64,
    pub chain_id: u64,
}

/// Encodes an EIP-2930 / EIP-1559 access list into RLP format (L7 fix).
pub fn encode_access_list(access_list: &[(Address, Vec<B256>)], out: &mut Vec<u8>) {
    if access_list.is_empty() {
        Header {
            list: true,
            payload_length: 0,
        }
        .encode(out);
        return;
    }
    let mut items_payload = Vec::new();
    for (addr, storage_keys) in access_list {
        let mut item_payload = Vec::new();
        addr.encode(&mut item_payload);

        let mut keys_payload = Vec::new();
        for key in storage_keys {
            key.encode(&mut keys_payload);
        }
        Header {
            list: true,
            payload_length: keys_payload.len(),
        }
        .encode(&mut item_payload);
        item_payload.extend_from_slice(&keys_payload);

        Header {
            list: true,
            payload_length: item_payload.len(),
        }
        .encode(&mut items_payload);
        items_payload.extend_from_slice(&item_payload);
    }
    Header {
        list: true,
        payload_length: items_payload.len(),
    }
    .encode(out);
    out.extend_from_slice(&items_payload);
}

/// Multi-EOA rotating signer pool bypassing Arbitrum and Base nonce serialization caches.
pub struct SignerPool {
    signers: Vec<EoaSigner>,
    in_use: Vec<AtomicBool>,
    total_leases: AtomicUsize,
    capacity: usize,
    next_idx: AtomicUsize,
}

impl SignerPool {
    /// Constructs a pool with deterministic mock signers (matches MockEoaPool for testing).
    pub fn new(count: usize) -> Self {
        let capacity = count.min(64);
        let mut signers = Vec::with_capacity(capacity);
        let mut in_use = Vec::with_capacity(capacity);

        for i in 0..capacity {
            signers.push(EoaSigner::mock(i));
            in_use.push(AtomicBool::new(false));
        }

        Self {
            signers,
            in_use,
            total_leases: AtomicUsize::new(0),
            capacity,
            next_idx: AtomicUsize::new(0),
        }
    }

    /// Constructs a pool from an array of 32-byte private keys.
    pub fn from_keys(keys: &[[u8; 32]]) -> Result<Self, SignerError> {
        let capacity = keys.len().min(64);
        let mut signers = Vec::with_capacity(capacity);
        let mut in_use = Vec::with_capacity(capacity);

        for (i, key) in keys.iter().take(capacity).enumerate() {
            signers.push(EoaSigner::new(i, *key)?);
            in_use.push(AtomicBool::new(false));
        }

        Ok(Self {
            signers,
            in_use,
            total_leases: AtomicUsize::new(0),
            capacity,
            next_idx: AtomicUsize::new(0),
        })
    }

    /// Constructs a pool from hex-encoded private keys (e.g. from AppConfig).
    pub fn from_hex_keys(hex_keys: &[String]) -> Result<Self, SignerError> {
        let mut raw_keys = Vec::with_capacity(hex_keys.len());
        for key_str in hex_keys {
            let clean = key_str.trim().trim_start_matches("0x");
            let bytes = alloy_primitives::hex::decode(clean)
                .map_err(|e| SignerError::InvalidPrivateKey(e.to_string()))?;
            if bytes.len() != 32 {
                return Err(SignerError::InvalidPrivateKey(format!(
                    "Key must be 32 bytes, got {}",
                    bytes.len()
                )));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            raw_keys.push(key);
        }
        Self::from_keys(&raw_keys)
    }

    /// Primary acquisition method: returns (signer_index, address, nonce).
    /// Fully non-blocking lock-free CAS loop (<15ns) with true round-robin rotation.
    pub fn acquire_signer(&self) -> Option<(usize, Address, u64)> {
        if self.capacity == 0 {
            return None;
        }

        let start = self.next_idx.fetch_add(1, Ordering::Relaxed) % self.capacity;
        for offset in 0..self.capacity {
            let i = (start + offset) % self.capacity;
            if self.in_use[i]
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
            {
                let signer = &self.signers[i];
                let nonce = signer.reserve_nonce();
                self.total_leases.fetch_add(1, Ordering::Relaxed);
                return Some((i, signer.address, nonce));
            }
        }
        None
    }

    /// RAII lease acquisition returning a SignerLease guard.
    pub fn acquire<'a>(&'a self) -> Option<SignerLease<'a>> {
        let (index, address, nonce) = self.acquire_signer()?;
        Some(SignerLease {
            pool: self,
            index,
            address,
            nonce,
            disarmed: false,
        })
    }

    /// Releases a leased signer back to the pool (safe on out-of-bounds index).
    pub fn release_signer(&self, index: usize) {
        if index < self.capacity {
            self.in_use[index].store(false, Ordering::SeqCst);
        }
    }

    #[inline(always)]
    pub fn current_nonce(&self, index: usize) -> u64 {
        if index < self.capacity {
            self.signers[index].current_nonce()
        } else {
            0
        }
    }

    #[inline(always)]
    pub fn sync_nonce(&self, index: usize, on_chain_nonce: u64) {
        if index < self.capacity {
            self.signers[index].sync_nonce(on_chain_nonce);
        }
    }

    #[inline(always)]
    pub fn set_nonce(&self, index: usize, nonce: u64) {
        if index < self.capacity {
            self.signers[index].set_nonce(nonce);
        }
    }

    #[inline(always)]
    pub fn signer(&self, index: usize) -> Option<&EoaSigner> {
        self.signers.get(index)
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline(always)]
    pub fn total_leases(&self) -> usize {
        self.total_leases.load(Ordering::Relaxed)
    }

    /// Rolls back the nonce of the signer at `index` to `expected_nonce` if it equals `expected_nonce + 1`.
    #[inline(always)]
    pub fn rollback_nonce(&self, index: usize, expected_nonce: u64) {
        if index < self.capacity {
            self.signers[index].rollback_nonce(expected_nonce);
        }
    }

    /// Queries the on-chain pending transaction count (`eth_getTransactionCount`) for every signer in the pool (V-08).
    /// In offline / mock test environments, errors are handled gracefully and existing nonces are retained.
    pub async fn sync_nonces_from_rpc(&self, rpc_url: &str) -> Result<(), SignerError> {
        if rpc_url.is_empty() || self.capacity == 0 {
            return Ok(());
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| SignerError::SigningFailed(format!("HTTP client build failed: {e}")))?;

        for (i, signer) in self.signers.iter().enumerate() {
            let payload = serde_json::json!({
                "jsonrpc": "2.0",
                "id": i as u64,
                "method": "eth_getTransactionCount",
                "params": [format!("{:#x}", signer.address), "pending"],
            });

            let resp = match client.post(rpc_url).json(&payload).send().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(
                        signer_index = i,
                        error = %e,
                        "Failed to reach RPC endpoint for nonce sync; retaining local nonce"
                    );
                    return Err(SignerError::SigningFailed(format!("RPC call failed: {e}")));
                }
            };

            let json: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| SignerError::SigningFailed(format!("Failed to parse RPC response: {e}")))?;

            if let Some(err) = json.get("error") {
                return Err(SignerError::SigningFailed(format!(
                    "eth_getTransactionCount error: {err}"
                )));
            }

            if let Some(result_hex) = json.get("result").and_then(|r| r.as_str()) {
                let clean = result_hex.trim_start_matches("0x");
                let on_chain_nonce = u64::from_str_radix(clean, 16)
                    .map_err(|e| SignerError::SigningFailed(format!("Invalid nonce hex {result_hex}: {e}")))?;
                signer.sync_nonce(on_chain_nonce);
                tracing::info!(
                    signer_index = i,
                    address = ?signer.address,
                    on_chain_nonce,
                    "Synchronized EOA nonce from on-chain RPC"
                );
            }
        }
        Ok(())
    }

    /// Signs an EIP-1559 transaction locally in memory with zero network roundtrips (~25µs).
    pub fn sign_transaction(
        &self,
        index: usize,
        tx: &Eip1559TransactionRequest,
    ) -> Result<SignedRawTransaction, SignerError> {
        let signer = self
            .signers
            .get(index)
            .ok_or(SignerError::IndexOutOfBounds(index, self.capacity))?;

        let nonce = tx.nonce.unwrap_or_else(|| signer.current_nonce());

        // 1. Build unsigned EIP-1559 payload:
        // [chain_id, nonce, max_priority_fee, max_fee, gas_limit, to, value, data, access_list]
        let mut unsigned_payload = Vec::with_capacity(256);
        tx.chain_id.encode(&mut unsigned_payload);
        nonce.encode(&mut unsigned_payload);
        tx.max_priority_fee_per_gas.encode(&mut unsigned_payload);
        tx.max_fee_per_gas.encode(&mut unsigned_payload);
        tx.gas_limit.encode(&mut unsigned_payload);
        tx.to.encode(&mut unsigned_payload);
        tx.value.encode(&mut unsigned_payload);
        tx.data.as_ref().encode(&mut unsigned_payload);
        // L7 fix: encode real access_list if present, otherwise 0xc0
        encode_access_list(&tx.access_list, &mut unsigned_payload);

        let mut unsigned_rlp = Vec::with_capacity(unsigned_payload.len() + 8);
        Header {
            list: true,
            payload_length: unsigned_payload.len(),
        }
        .encode(&mut unsigned_rlp);
        unsigned_rlp.extend_from_slice(&unsigned_payload);

        // Compute transaction hash preimage: 0x02 || rlp(...)
        let mut sighash_preimage = Vec::with_capacity(1 + unsigned_rlp.len());
        sighash_preimage.push(0x02);
        sighash_preimage.extend_from_slice(&unsigned_rlp);
        let sighash = keccak256(&sighash_preimage);

        // Sign locally using k256 recoverable ECDSA
        let signing_key = k256::ecdsa::SigningKey::from_bytes((&signer.private_key).into())
            .map_err(|e| SignerError::SigningFailed(e.to_string()))?;
        let (signature, recovery_id) = signing_key
            .sign_prehash_recoverable(sighash.as_slice())
            .map_err(|e| SignerError::SigningFailed(e.to_string()))?;

        // V-23: Cryptographic verification ensuring recovered address matches signer.address
        let recovered_address = recover_signer(sighash.as_slice(), &signature, recovery_id)?;
        if recovered_address != signer.address {
            return Err(SignerError::SigningFailed(format!(
                "Recovered signer mismatch: expected {:?}, got {:?}",
                signer.address, recovered_address
            )));
        }

        let y_parity: u8 = recovery_id.to_byte();
        let r_bytes = signature.r().to_bytes();
        let s_bytes = signature.s().to_bytes();

        let r = U256::from_be_slice(&r_bytes);
        let s = U256::from_be_slice(&s_bytes);

        // 2. Build signed EIP-1559 payload:
        // [chain_id, nonce, max_priority_fee, max_fee, gas_limit, to, value, data, access_list, y_parity, r, s]
        let mut signed_payload = Vec::with_capacity(unsigned_payload.len() + 70);
        signed_payload.extend_from_slice(&unsigned_payload);
        y_parity.encode(&mut signed_payload);
        r.encode(&mut signed_payload);
        s.encode(&mut signed_payload);

        let mut signed_rlp = Vec::with_capacity(signed_payload.len() + 8);
        Header {
            list: true,
            payload_length: signed_payload.len(),
        }
        .encode(&mut signed_rlp);
        signed_rlp.extend_from_slice(&signed_payload);

        // 0x02 || rlp(signed_list)
        let mut raw_bytes_vec = Vec::with_capacity(1 + signed_rlp.len());
        raw_bytes_vec.push(0x02);
        raw_bytes_vec.extend_from_slice(&signed_rlp);

        let raw_bytes = Bytes::from(raw_bytes_vec);
        let tx_hash = keccak256(&raw_bytes);

        Ok(SignedRawTransaction {
            raw_bytes,
            tx_hash,
            sender: signer.address,
            nonce,
            chain_id: tx.chain_id,
        })
    }
}

/// Recovers the Ethereum address from an EIP-1559 sighash, signature, and recovery ID (V-23).
pub fn recover_signer(
    sighash: &[u8],
    signature: &k256::ecdsa::Signature,
    recovery_id: k256::ecdsa::RecoveryId,
) -> Result<Address, SignerError> {
    let recovered_key = k256::ecdsa::VerifyingKey::recover_from_prehash(
        sighash,
        signature,
        recovery_id,
    )
    .map_err(|e| SignerError::SigningFailed(format!("Signature recovery failed: {e}")))?;

    let uncompressed = recovered_key.to_encoded_point(false);
    let hash = keccak256(&uncompressed.as_bytes()[1..65]);
    let mut recovered_addr_bytes = [0u8; 20];
    recovered_addr_bytes.copy_from_slice(&hash[12..32]);
    Ok(Address::from(recovered_addr_bytes))
}

/// RAII Lease Guard for an acquired EOA signer.
pub struct SignerLease<'a> {
    pool: &'a SignerPool,
    index: usize,
    address: Address,
    nonce: u64,
    disarmed: bool,
}

impl<'a> SignerLease<'a> {
    #[inline(always)]
    pub fn index(&self) -> usize {
        self.index
    }

    #[inline(always)]
    pub fn address(&self) -> Address {
        self.address
    }

    #[inline(always)]
    pub fn nonce(&self) -> u64 {
        self.nonce
    }

    /// Rolls back the leased nonce on the underlying signer (V-09).
    #[inline(always)]
    pub fn rollback_nonce(&self) {
        if self.index < self.pool.capacity {
            self.pool.signers[self.index].rollback_nonce(self.nonce);
        }
    }

    pub fn release(mut self) {
        if !self.disarmed {
            self.disarmed = true;
            self.pool.release_signer(self.index);
        }
    }
}

impl<'a> Drop for SignerLease<'a> {
    fn drop(&mut self) {
        if !self.disarmed {
            self.pool.release_signer(self.index);
        }
    }
}
