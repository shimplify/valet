//! Global state management for Shimmer
//!
//! Due to the nature of function interception via `LD_PRELOAD`, global state is required
//! to track file mappings, buffer pools, and zone allocations across all intercepted calls.
//!
//! All maps use `DashMap` for concurrent access without explicit locking.

use crate::state::*;
use dashmap::DashMap;
use std::collections::VecDeque;
use std::fs::File;
use std::io::BufReader;
use std::sync::{Arc, LazyLock, Mutex};

/// Maps file paths to their assigned UIDs (unique identifiers)
pub static PATH_TO_UID: LazyLock<Arc<DashMap<String, u32>>> =
    LazyLock::new(|| Arc::new(DashMap::new()));

/// Initial number of buffers in the pool (expands dynamically as needed)
pub static NUM_BUFFERS: usize = 16;

/// Buffer with usage tracking and pre-allocated capacity
pub struct TrackedBuffer {
    /// The actual buffer data
    pub data: Vec<u8>,
    /// Whether this buffer is currently available for allocation
    pub is_free: bool,
}

impl Default for TrackedBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackedBuffer {
    pub fn new() -> Self {
        Self {
            data: Vec::with_capacity(crate::state::PREFERRED_BUFFER_SIZE),
            is_free: true,
        }
    }
}

/// Global pool of buffers with usage tracking
///
/// Starts with [`NUM_BUFFERS`] buffers and expands dynamically when all buffers are in use.
pub static BUFFER_POOL: LazyLock<Arc<DashMap<u32, TrackedBuffer>>> =
    LazyLock::new(|| Arc::new(DashMap::new()));

/// Per-zone locks to serialize writes to the same zone
pub static ZONE_LOCKS: LazyLock<DashMap<u32, Arc<Mutex<()>>>> = LazyLock::new(DashMap::new);

/// Maps file descriptors to their assigned buffer IDs
pub static FILE_TO_BUFFER_POOL: LazyLock<Arc<DashMap<i32, u32>>> =
    LazyLock::new(|| Arc::new(DashMap::new()));

/// Maps file descriptors to their UIDs
pub static FD_TO_UID: LazyLock<DashMap<i32, u32>> = LazyLock::new(DashMap::new);

/// Maps file descriptors to their stream type (SSTable, LOG, etc.)
pub static FD_TO_STREAM: LazyLock<DashMap<i32, Stream>> = LazyLock::new(DashMap::new);

/// Maps UIDs to file metadata
pub static FILESYSTEM: LazyLock<DashMap<u32, Files>> = LazyLock::new(DashMap::new);

/// Maps zone IDs to their metadata (usage, deleted extents, etc.)
pub static ZONE_METADATA_MAP: LazyLock<DashMap<u32, Zone>> = LazyLock::new(DashMap::new);

/// Cache of open file handles for reading (UID -> (BufReader, fd))
///
/// Avoids re-opening files on subsequent reads
pub static READONLY_FILES: LazyLock<DashMap<u32, (BufReader<File>, i32)>> =
    LazyLock::new(DashMap::new);

/// Maps stream types to their currently active zone
pub static STREAM_TO_HINT: LazyLock<Arc<DashMap<Stream, u32>>> =
    LazyLock::new(|| Arc::new(DashMap::new()));

/// Queue of available zone IDs for allocation
///
/// Protected by a mutex since allocation must be atomic
pub static FREE_ZONE: Mutex<VecDeque<u32>> = Mutex::new(VecDeque::new());

// ---------------- END OF STATICS ----------------------------------------------------------
