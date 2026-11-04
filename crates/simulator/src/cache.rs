//! Two-tier persistent BytecodeCache combining sub-microsecond in-memory LRU and cacache disk storage.
//! Authoritative source: ORIGINAL_REQUEST.md lines 43-44, PROJECT.md line 58.

use alloy_primitives::{Address, Bytes};
use flash_core::types::Chain;
use parking_lot::RwLock;
use revm::bytecode::Bytecode;
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("Disk read error: {0}")]
    DiskRead(String),
    #[error("Disk write error: {0}")]
    DiskWrite(String),
}

/// Doubly-linked LRU entry for O(1) recency updates
struct LruEntry {
    bytecode: Bytecode,
    prev: Option<(u64, Address)>,
    next: Option<(u64, Address)>,
}

/// Hot in-memory LRU cache layer (< 50 ns access time, strictly O(1) operations)
pub struct MemoryLru {
    capacity: usize,
    map: HashMap<(u64, Address), LruEntry>,
    head: Option<(u64, Address)>, // oldest (least recently used)
    tail: Option<(u64, Address)>, // newest (most recently used)
}

impl MemoryLru {
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            capacity: cap,
            map: HashMap::with_capacity(cap),
            head: None,
            tail: None,
        }
    }

    /// Read without updating LRU order (safe for parallel concurrent reads)
    #[inline(always)]
    pub fn peek(&self, chain_id: u64, addr: Address) -> Option<Bytecode> {
        self.map.get(&(chain_id, addr)).map(|e| e.bytecode.clone())
    }

    fn detach(&mut self, key: &(u64, Address)) {
        let (prev, next) = match self.map.get(key) {
            Some(entry) => (entry.prev, entry.next),
            None => return,
        };

        if let Some(p) = prev {
            if let Some(p_entry) = self.map.get_mut(&p) {
                p_entry.next = next;
            }
        } else {
            self.head = next;
        }

        if let Some(n) = next {
            if let Some(n_entry) = self.map.get_mut(&n) {
                n_entry.prev = prev;
            }
        } else {
            self.tail = prev;
        }
    }

    fn attach_tail(&mut self, key: (u64, Address)) {
        let old_tail = self.tail;
        if let Some(t) = old_tail {
            if let Some(t_entry) = self.map.get_mut(&t) {
                t_entry.next = Some(key);
            }
        } else {
            self.head = Some(key);
        }
        self.tail = Some(key);
        if let Some(entry) = self.map.get_mut(&key) {
            entry.prev = old_tail;
            entry.next = None;
        }
    }

    pub fn get(&mut self, chain_id: u64, addr: Address) -> Option<Bytecode> {
        let key = (chain_id, addr);
        if self.map.contains_key(&key) {
            self.detach(&key);
            self.attach_tail(key);
            self.map.get(&key).map(|e| e.bytecode.clone())
        } else {
            None
        }
    }

    pub fn put(&mut self, chain_id: u64, addr: Address, bytecode: Bytecode) {
        let key = (chain_id, addr);
        if self.map.contains_key(&key) {
            self.detach(&key);
            if let Some(entry) = self.map.get_mut(&key) {
                entry.bytecode = bytecode;
            }
            self.attach_tail(key);
        } else {
            if self.map.len() >= self.capacity {
                if let Some(oldest) = self.head {
                    self.detach(&oldest);
                    self.map.remove(&oldest);
                }
            }
            self.map.insert(
                key,
                LruEntry {
                    bytecode,
                    prev: None,
                    next: None,
                },
            );
            self.attach_tail(key);
        }
    }

    #[inline(always)]
    pub fn contains(&self, chain_id: u64, addr: Address) -> bool {
        self.map.contains_key(&(chain_id, addr))
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.head = None;
        self.tail = None;
    }
}

/// Two-Tier Persistent Bytecode Cache (Memory LRU + cacache disk storage)
pub struct BytecodeCache {
    disk_path: PathBuf,
    memory: RwLock<MemoryLru>,
}

impl BytecodeCache {
    pub fn new(disk_path: impl Into<PathBuf>, memory_capacity: usize) -> Self {
        Self {
            disk_path: disk_path.into(),
            memory: RwLock::new(MemoryLru::new(memory_capacity)),
        }
    }

    /// Canonical cache key format: `format!("{}:{}", chain_id, address)`
    #[inline(always)]
    pub fn format_key(chain_id: u64, address: Address) -> String {
        format!("{}:{}", chain_id, address)
    }

    /// Read-through lookup: Memory LRU -> cacache disk
    /// M8: Takes a read lock first on cache hits to allow unblocked concurrent reads
    pub fn get_bytecode(&self, chain: Chain, address: Address) -> Option<Bytecode> {
        let chain_id = chain.id();

        // 1. Hot memory check under read lock (< 50 ns, non-blocking)
        {
            let mem = self.memory.read();
            if let Some(code) = mem.peek(chain_id, address) {
                return Some(code);
            }
        }

        // 2. Cold disk check (< 30 µs)
        let disk_key = Self::format_key(chain_id, address);
        if let Ok(raw_bytes) = cacache::read_sync(&self.disk_path, &disk_key) {
            let bytecode = Bytecode::new_raw(Bytes::from(raw_bytes));
            let mut mem = self.memory.write();
            mem.put(chain_id, address, bytecode.clone());
            return Some(bytecode);
        }

        None
    }

    /// Store bytecode in both Memory LRU and cacache disk
    pub fn put_bytecode(&self, chain: Chain, address: Address, code: Bytes) -> Result<(), CacheError> {
        let chain_id = chain.id();
        let bytecode = Bytecode::new_raw(code.clone());

        // 1. Memory LRU store
        {
            let mut mem = self.memory.write();
            mem.put(chain_id, address, bytecode);
        }

        // 2. Persistent disk store
        let disk_key = Self::format_key(chain_id, address);
        cacache::write_sync(&self.disk_path, &disk_key, code.as_ref())
            .map_err(|e| CacheError::DiskWrite(e.to_string()))?;

        Ok(())
    }

    /// Check if bytecode exists in memory cache
    pub fn contains_memory(&self, chain: Chain, address: Address) -> bool {
        self.memory.read().contains(chain.id(), address)
    }

    /// Clear in-memory cache without purging disk
    pub fn clear_memory(&self) {
        self.memory.write().clear();
    }
}
