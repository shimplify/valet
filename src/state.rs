//! Data structures and state management for zoned devices.

use crate::{shimmer_error, shimmer_info};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::sync::OnceLock;
use std::{fmt, sync::atomic::AtomicU32};
#[macro_export]
macro_rules! MiB {
    ($input:literal) => {{ $input * 1024 * 1024 }};
}

#[macro_export]
macro_rules! KiB {
    ($input:literal) => {{ $input * 1024 }};
}

// Helper functions for conditional serialization
fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

fn is_all_false(v: &[bool]) -> bool {
    v.iter().all(|&b| !b)
}

/// Block : 4K Blocks
pub const BLOCK_SIZE: u32 = KiB!(4);

// Runtime configuration using OnceLock for thread-safe lazy initialization
static CONFIG: OnceLock<ZoneConfig> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
struct ZoneConfig {
    zone_num: u32,
    zone_size: u32,
    zone_capacity: u32,
}

/// Detects zone configuration from ZoneFS filesystem.
///
/// Discovers zone parameters by inspecting the ZoneFS mount:
/// - Zone count: Number of zone files in [`ZONE_PATH`]
/// - Zone size: Read from zone file metadata (`st_blocks * 512`)
/// - Zone capacity: Equals zone size for sequential zones
///
/// # Returns
///
/// A tuple `(zone_num, zone_size, zone_capacity)` in bytes.
///
/// # Errors
///
/// Returns an error if [`ZONE_PATH`] cannot be read, no zone files are found,
/// zone file metadata cannot be accessed, or zone file reports zero size.
fn detect_zone_config() -> Result<(u32, u32, u32), String> {
    // Count zone files
    let zone_count = match fs::read_dir(ZONE_PATH) {
        Ok(entries) => {
            let count = entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().map(|ft| ft.is_file()).unwrap_or(false))
                .count();
            if count == 0 {
                return Err(format!("No zone files found in {}", ZONE_PATH));
            }
            count as u32
        }
        Err(e) => return Err(format!("Failed to read {}: {}", ZONE_PATH, e)),
    };

    // Get zone size/capacity from first zone file's metadata
    // ZoneFS exposes zone size via stat()'s st_blocks field
    let zone_path = format!("{}0", ZONE_PATH);
    let (zone_size, zone_capacity) = match fs::metadata(&zone_path) {
        Ok(metadata) => {
            // st_blocks is in 512-byte sectors
            let size_bytes = metadata.blocks() * 512;
            if size_bytes == 0 {
                return Err(format!("Zone file {} reports zero size", zone_path));
            }
            // For sequential zones, capacity typically equals size
            (size_bytes as u32, size_bytes as u32)
        }
        Err(e) => return Err(format!("Failed to stat {}: {}", zone_path, e)),
    };

    Ok((zone_count, zone_size, zone_capacity))
}

/// Initializes zone configuration from ZoneFS.
///
/// Detects zone parameters from the mounted ZoneFS filesystem.
///
/// # Panics
///
/// Panics if ZoneFS is not mounted at [`ZONE_PATH`], permissions are insufficient,
/// or ZoneFS configuration is invalid.
pub fn init_config() {
    CONFIG.get_or_init(|| match detect_zone_config() {
        Ok((zone_num, zone_size, zone_capacity)) => {
            shimmer_info!(
                "Detected {} zones from {}, size={} bytes ({} MiB), capacity={} bytes ({} MiB)",
                zone_num,
                ZONE_PATH,
                zone_size,
                zone_size / 1024 / 1024,
                zone_capacity,
                zone_capacity / 1024 / 1024
            );
            ZoneConfig {
                zone_num,
                zone_size,
                zone_capacity,
            }
        }
        Err(e) => {
            shimmer_error!("Failed to detect zone configuration: {}", e);
            shimmer_error!("Ensure ZoneFS is mounted at {}", ZONE_PATH);
            panic!(
                "Zone configuration detection failed: {}. Ensure ZoneFS is mounted at {}",
                e, ZONE_PATH
            );
        }
    });
}

/// Returns the number of zones detected from ZoneFS.
///
/// # Panics
///
/// Panics if called before [`init_config()`].
pub fn get_zone_num() -> u32 {
    CONFIG
        .get()
        .expect("Zone configuration not initialized - init_config() must be called first")
        .zone_num
}

/// Returns the size of each zone in bytes.
///
/// # Panics
///
/// Panics if called before [`init_config()`].
pub fn get_zone_size() -> u32 {
    CONFIG
        .get()
        .expect("Zone configuration not initialized - init_config() must be called first")
        .zone_size
}

/// Returns the capacity of each zone in bytes.
///
/// For sequential zones, capacity equals zone size.
///
/// # Panics
///
/// Panics if called before [`init_config()`].
pub fn get_zone_capacity() -> u32 {
    CONFIG
        .get()
        .expect("Zone configuration not initialized - init_config() must be called first")
        .zone_capacity
}

/// Minimum write size the device accepts.
pub const CHUNK_SIZE: u32 = KiB!(64);

/// Minimum buffer size.
pub const MIN_BUFFER_SIZE: u32 = CHUNK_SIZE;

/// Preferred buffer size.
pub const PREFERRED_BUFFER_SIZE: usize = MiB!(16);

/// Aligns size to page boundary.
pub const fn align_to_page(size: u32) -> u32 {
    size.div_ceil(BLOCK_SIZE) * BLOCK_SIZE
}

/// Maximum number of zones that can be open simultaneously.
pub const MAX_OPEN_ZONES: u32 = 6;

/// ZoneFS mount path.
pub const ZONE_PATH: &str = "/mnt/zonefs/seq/";

/// Increasing sequence of file identifiers.
pub static UID: AtomicU32 = AtomicU32::new(0);
pub static READ_ID: AtomicU32 = AtomicU32::new(0);
pub static WRITE_ID: AtomicU32 = AtomicU32::new(0);

/// File stream types for hint interface.
#[derive(Debug, Default, PartialEq, Eq, Hash, Clone, Copy, Serialize, Deserialize)]
pub enum Stream {
    SSTable,
    LOG,
    LSM,
    #[default]
    Undefined,
}
impl fmt::Display for Stream {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Stream::LOG => write!(f, "Log"),
            Stream::SSTable => write!(f, "SSTable"),
            Stream::LSM => write!(f, "LSM"),
            Stream::Undefined => write!(f, "Undefined"),
        }
    }
}

/// Metadata for a file extent on a zone.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Extents {
    pub zone: u32,
    pub offset: u32,
    pub actual_size: u32,
    pub zone_extent_num: u32,
}
impl Extents {
    pub fn new(zone: u32, offset: u32, actual_size: u32, zone_extent_num: u32) -> Self {
        Self {
            zone,
            offset,
            actual_size,
            zone_extent_num,
        }
    }
}

/// Metadata for a mapped file on a zone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Files {
    #[serde(skip_serializing_if = "is_zero_u32", default)]
    pub file_size: u32,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub extents: Vec<Extents>,
}

impl Files {
    pub fn new(file_size: u32) -> Self {
        Self {
            file_size,
            extents: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct LockedBuffer {
    pub buffer: Vec<u8>,
    pub lock: bool,
}
impl LockedBuffer {
    pub fn new(buffer: Vec<u8>, lock: bool) -> Self {
        Self { buffer, lock }
    }
}
/// Write request for persisting buffered data to a zone.
#[derive(Debug, Clone, Default)]
pub struct WriteRequest {
    pub id: u32,
    pub file: u32,
    pub buffer: u32,
    pub stream: Stream,
    pub fd: i32,
}
impl WriteRequest {
    pub fn new(id: u32, file: u32, buffer: u32, stream: Stream, fd: i32) -> Self {
        Self {
            id,
            file,
            buffer,
            stream,
            fd,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WriteResponse {
    pub id: u32,
    pub fd: i32,
    pub buffer: u32,
}
impl WriteResponse {
    pub fn new(id: u32, fd: i32, buffer: u32) -> Self {
        Self { id, fd, buffer }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ReadRequest {
    pub id: u32,
    pub fd: i32,
    pub count: usize,
    pub offset: i64,
    pub buf_ptr: usize,
}

#[derive(Debug, Clone)]
pub struct ReadResponse {
    pub bytes_read: usize,
    pub success: bool,
}
/// Zone metadata and state tracking.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize, Deserialize)]
pub struct Zone {
    /// Total number of extents allocated in this zone.
    #[serde(skip_serializing_if = "is_zero_u32", default)]
    pub extent_count: u32,

    /// Tracks deleted extents (true = deleted, false = active).
    #[serde(skip_serializing_if = "is_all_false", default)]
    pub deleted_extents: Vec<bool>,

    /// Indicates if the zone has reached capacity.
    pub is_full: bool,

    /// Total deleted bytes for garbage collection decisions.
    #[serde(skip_serializing_if = "is_zero_u32", default)]
    pub deleted_bytes: u32,
}

impl Zone {
    pub fn new() -> Self {
        Self {
            extent_count: 0,
            deleted_extents: Vec::new(),
            is_full: false,
            deleted_bytes: 0,
        }
    }
    /// Records a new extent allocation and returns its number.
    pub fn add_extent(&mut self) -> u32 {
        let extent_num = self.extent_count;
        self.extent_count += 1;

        // Ensure the deleted_extents vector is large enough
        if self.deleted_extents.len() < self.extent_count as usize {
            self.deleted_extents
                .resize(self.extent_count as usize, false);
        }

        extent_num
    }

    /// Marks an extent as deleted and updates deleted bytes.
    pub fn mark_extent_deleted(&mut self, extent_num: u32, extent_size: u32) -> bool {
        if extent_num < self.deleted_extents.len() as u32
            && !self.deleted_extents[extent_num as usize]
        {
            self.deleted_extents[extent_num as usize] = true;
            self.deleted_bytes += extent_size;
            return true;
        }
        false
    }

    /// Returns true if zone is full and all extents are deleted.
    pub fn should_gc(&self) -> bool {
        self.is_full && self.deleted_extents.iter().all(|&deleted| deleted)
    }

    /// Returns the percentage of deleted extents in the zone.
    pub fn gc_efficiency(&self) -> f32 {
        if self.extent_count == 0 {
            return 0.0;
        }

        let deleted_count = self
            .deleted_extents
            .iter()
            .filter(|&&deleted| deleted)
            .count();
        deleted_count as f32 / self.extent_count as f32
    }

    /// Marks the zone as full.
    pub fn mark_full(&mut self) {
        self.is_full = true;
    }
}
impl Default for Zone {
    fn default() -> Self {
        Self::new()
    }
}
