//! Persistence layer for writing buffers to zones and reading from extents.

use crate::state::{CHUNK_SIZE, WriteResponse, align_to_page};
use crate::statics::{
    BUFFER_POOL, FD_TO_UID, FILE_TO_BUFFER_POOL, FILESYSTEM, READONLY_FILES, ZONE_LOCKS,
};
use crate::*;
use crate::{shimmer_debug, shimmer_info};
use dashmap::DashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::prelude::OpenOptionsExt;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, LazyLock, Mutex, atomic};
use thingbuf::mpsc::blocking::{Receiver, Sender, channel};

/// Total bytes written to zones.
pub static BYTES_WRITTEN: AtomicU64 = AtomicU64::new(0);

/// Channel for write requests to the write thread.
///
/// Write requests are sent through this channel from the main thread to be processed
/// asynchronously by the write thread. The channel capacity is 128 requests.
pub static WRITE_REQUESTS: LazyLock<(Sender<state::WriteRequest>, Receiver<state::WriteRequest>)> =
    LazyLock::new(|| channel(128));

/// Tracks completed writes by request ID.
///
/// Maps request IDs to their completion status. Used for synchronization between
/// the write thread and requesting threads.
pub static WRITE_SUCCESS: LazyLock<DashMap<u32, WriteResponse>> = LazyLock::new(DashMap::new);

/// Channel for read requests to the read thread.
///
/// Read requests are sent through this channel to be processed asynchronously
/// by the read thread. The channel capacity is 128 requests.
pub static READ_REQUESTS: LazyLock<(Sender<state::ReadRequest>, Receiver<state::ReadRequest>)> =
    LazyLock::new(|| channel(128));

/// Tracks completed reads by request ID.
///
/// Maps request IDs to their completion status and read data. Used for synchronization
/// between the read thread and requesting threads.
pub static READ_SUCCESS: LazyLock<DashMap<u32, state::ReadResponse>> = LazyLock::new(DashMap::new);

/// Writer for a zone file with offset tracking.
///
/// Wraps a zone file and tracks the current write offset for managing
/// sequential writes to ZNS zones.
pub struct ZoneWriter {
    /// The underlying zone file handle.
    pub file: File,
    /// Current write offset in the zone.
    pub current_offset: u64,
}

impl ZoneWriter {
    /// Creates a new `ZoneWriter` with the given file and initial offset.
    ///
    /// # Arguments
    /// * `file` - The zone file to write to
    /// * `initial_offset` - Starting offset (typically current file length)
    pub fn new(file: File, initial_offset: u64) -> Self {
        Self {
            file,
            current_offset: initial_offset,
        }
    }

    /// Writes all bytes from the buffer to the zone using direct syscalls.
    ///
    /// Bypasses the standard library's write implementation to use libc directly.
    /// Loops until all bytes are written.
    ///
    /// # Arguments
    /// * `buf` - Buffer to write
    ///
    /// # Errors
    /// Returns an error if the write syscall fails.
    pub fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        let fd = self.file.as_raw_fd();
        let mut written = 0;

        while written < buf.len() {
            let result = unsafe {
                libc::write(
                    fd,
                    buf[written..].as_ptr() as *const libc::c_void,
                    buf.len() - written,
                )
            };

            if result < 0 {
                return Err(std::io::Error::last_os_error());
            }

            written += result as usize;
        }

        Ok(())
    }

    /// Flushes any buffered data to the zone file.
    ///
    /// # Errors
    /// Returns an error if the flush operation fails.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }

    /// Sets the zone file length and updates the current offset.
    ///
    /// Used to pad zones to their full size when marking them as full.
    ///
    /// # Arguments
    /// * `size` - The new file size
    ///
    /// # Errors
    /// Returns an error if the set_len operation fails.
    pub fn set_len(&mut self, size: u64) -> std::io::Result<()> {
        let result = self.file.set_len(size);
        self.current_offset = size;
        result
    }
}

/// Maps zone numbers to their corresponding zone writers.
///
/// Maintains open file handles for zones currently being written to.
pub static WRITE_ZONES: LazyLock<DashMap<u32, ZoneWriter>> = LazyLock::new(DashMap::new);

/// Writes a variable-size buffer to a zone using O_DIRECT.
///
/// Allocates an aligned buffer (required for O_DIRECT), copies the data,
/// and writes it in CHUNK_SIZE pieces for efficiency.
///
/// # Arguments
/// * `writer` - The zone writer to write to
/// * `buffer` - The data to write
///
/// # Errors
/// Returns an error if buffer allocation or write operations fail.
fn write_variable_size_buffer(writer: &mut ZoneWriter, buffer: &[u8]) -> std::io::Result<()> {
    use std::alloc::{Layout, alloc, dealloc};

    // Allocate a single aligned buffer to copy data into for O_DIRECT writes
    // O_DIRECT requires buffer address aligned to 4096 bytes
    let total_size = buffer.len();
    let layout = Layout::from_size_align(total_size, 4096).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid buffer layout")
    })?;

    let aligned_ptr = unsafe { alloc(layout) };
    if aligned_ptr.is_null() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::OutOfMemory,
            "Failed to allocate aligned buffer",
        ));
    }

    // Copy data to aligned buffer
    unsafe {
        std::ptr::copy_nonoverlapping(buffer.as_ptr(), aligned_ptr, total_size);
    }

    let aligned_buffer = unsafe { std::slice::from_raw_parts(aligned_ptr, total_size) };
    let mut remaining = aligned_buffer;

    // Write in CHUNK_SIZE pieces for efficiency
    while remaining.len() >= CHUNK_SIZE as usize {
        let (chunk, rest) = remaining.split_at(CHUNK_SIZE as usize);
        if let Err(e) = writer.write_all(chunk) {
            shimmer_error!(
                "Failed to write chunk ({} bytes): {} (error kind: {:?})",
                chunk.len(),
                e,
                e.kind()
            );
            unsafe {
                dealloc(aligned_ptr, layout);
            }
            return Err(e);
        }
        if let Err(e) = writer.flush() {
            shimmer_error!(
                "Failed to flush after chunk write: {} (error kind: {:?})",
                e,
                e.kind()
            );
            unsafe {
                dealloc(aligned_ptr, layout);
            }
            return Err(e);
        }
        remaining = rest;
    }

    // Write remainder (should be aligned to page boundary)
    if !remaining.is_empty() {
        if let Err(e) = writer.write_all(remaining) {
            shimmer_error!(
                "Failed to write remainder ({} bytes): {} (error kind: {:?})",
                remaining.len(),
                e,
                e.kind()
            );
            unsafe {
                dealloc(aligned_ptr, layout);
            }
            return Err(e);
        }
        if let Err(e) = writer.flush() {
            shimmer_error!(
                "Failed to flush after remainder write: {} (error kind: {:?})",
                e,
                e.kind()
            );
            unsafe {
                dealloc(aligned_ptr, layout);
            }
            return Err(e);
        }
    }

    // Free aligned buffer
    unsafe {
        dealloc(aligned_ptr, layout);
    }

    Ok(())
}

/// Clones buffer data for writing while keeping the original in the write cache.
///
/// This macro retrieves a buffer from the buffer pool and clones its data.
/// The original buffer remains in the write cache to serve read requests
/// while the clone is being written to the zone.
///
/// # Arguments
/// * `$write_request` - The write request containing the buffer ID
///
/// # Returns
/// A cloned `Vec<u8>` containing the buffer data
#[macro_export]
macro_rules! getbuf {
    ($write_request:ident) => {{
        // Clone buffer data for writing, keep original in write cache for reads
        let collection = statics::BUFFER_POOL
            .get(&$write_request.buffer)
            .unwrap()
            .data
            .clone();

        shimmer_debug!(
            "Cloned buffer {} for write, keeping original in write cache",
            $write_request.buffer
        );
        collection
    }};
}

/// Channel for garbage collection requests.
///
/// Zone numbers are sent through this channel to be garbage collected
/// (reset and returned to the free pool). Channel capacity is 8.
#[cfg(not(feature = "disable-gc"))]
pub static CLEANUP_REQUESTS: LazyLock<(Sender<u32>, Receiver<u32>)> = LazyLock::new(|| channel(8));

/// Channel for metadata save requests.
///
/// Used to trigger background saves of filesystem metadata to disk.
/// Channel capacity is 16.
pub static METADATA_SAVE_REQUESTS: LazyLock<(Sender<()>, Receiver<()>)> =
    LazyLock::new(|| channel(16));

/// Garbage collection thread that resets zones and returns them to the free pool.
///
/// Runs in a loop receiving zone numbers through `CLEANUP_REQUESTS`,
/// truncating the zone file, and adding the zone back to `FREE_ZONE`.
/// Only available when the `disable-gc` feature is not enabled.
#[cfg(not(feature = "disable-gc"))]
pub fn garbage_collect() {
    loop {
        let zone = CLEANUP_REQUESTS.1.recv().unwrap();
        let zonefile = OpenOptions::new()
            .write(true)
            .truncate(true)
            .custom_flags(libc::O_DIRECT)
            .open(format!("{}{zone}", state::ZONE_PATH))
            .unwrap();
        zonefile.set_len(0).unwrap();
        statics::FREE_ZONE.lock().unwrap().push_back(zone);
        drop(zonefile);
    }
}

/// Background thread that handles metadata persistence.
///
/// Runs in a loop waiting for save requests through `METADATA_SAVE_REQUESTS`.
/// When triggered, saves the current filesystem metadata to disk.
pub fn metadata_save_thread() {
    crate::replacements::thread_checkpoint!("metadata_save_thread: started");

    loop {
        crate::replacements::thread_checkpoint!("metadata_save_thread: waiting for save request");
        METADATA_SAVE_REQUESTS.1.recv().unwrap();

        crate::replacements::thread_checkpoint!("metadata_save_thread: saving metadata");
        if let Err(e) = crate::metadata::save_metadata() {
            shimmer_error!("Background metadata save failed: {}", e);
        } else {
            shimmer_debug!("Background metadata save completed");
        }
    }
}

/// Attempts to read data from the write cache buffer.
///
/// This function is used as a fallback when data hasn't been persisted to zones yet.
/// It looks up the buffer associated with the file descriptor and reads directly
/// from the in-memory buffer.
///
/// # Arguments
/// * `fd` - File descriptor
/// * `buf` - Destination buffer to copy data into
/// * `count` - Number of bytes to read
/// * `file_offset` - Logical file offset to read from
/// * `buffer_start_offset` - Offset where this buffer starts in the logical file
/// * `buffer_offset` - Offset in the destination buffer to write to
/// * `debug_context` - Context string for debugging
///
/// # Returns
/// Number of bytes read from the write cache (0 if not found)
fn try_read_from_buffer(
    fd: i32,
    buf: *mut libc::c_void,
    count: usize,
    file_offset: i64,
    buffer_start_offset: i64,
    buffer_offset: usize,
    debug_context: &str,
) -> usize {
    // Look up buffer through FILE_TO_BUFFER_POOL (write cache)
    if let Some(buffer_num) = FILE_TO_BUFFER_POOL.get(&fd) {
        let buffer = BUFFER_POOL.get(&buffer_num).unwrap();
        let buffer_relative_offset = file_offset - buffer_start_offset;

        shimmer_debug!(
            "Buffer read attempt: fd={} context={} file_offset=0x{:x} buffer_start=0x{:x} buffer_len=0x{:x}",
            fd,
            debug_context,
            file_offset,
            buffer_start_offset,
            buffer.data.len()
        );

        if buffer_relative_offset >= 0 && (buffer_relative_offset as usize) < buffer.data.len() {
            let start_pos = buffer_relative_offset as usize;
            let end_pos = std::cmp::min(start_pos + count, buffer.data.len());
            let bytes_to_copy = end_pos - start_pos;

            if bytes_to_copy > 0 {
                let buf_ptr = (buf as usize + buffer_offset) as *mut u8;
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        buffer.data.as_ptr().add(start_pos),
                        buf_ptr,
                        bytes_to_copy,
                    );
                }
                shimmer_debug!(
                    "Read {} bytes from write cache ({}) file_offset=0x{:x} buffer_start=0x{:x}",
                    bytes_to_copy,
                    debug_context,
                    file_offset,
                    buffer_start_offset
                );
                return bytes_to_copy;
            }
        }
    }

    shimmer_error!(
        "No write cache buffer found for fd={} in context={}",
        fd,
        debug_context
    );
    0
}

/// Main write thread that processes write requests and persists data to zones.
///
/// This is the core persistence thread that:
/// 1. Receives write requests from `WRITE_REQUESTS` channel
/// 2. Allocates zones and manages zone writers
/// 3. Aligns buffers to page boundaries for O_DIRECT writes
/// 4. Writes data to zones in chunks
/// 5. Updates filesystem metadata with extent information
/// 6. Handles zone rotation when capacity is reached
/// 7. Signals completion through `WRITE_SUCCESS`
///
/// Runs in an infinite loop processing write requests sequentially.
pub fn write_thread() {
    crate::replacements::thread_checkpoint!("write_thread: started");

    loop {
        crate::replacements::thread_checkpoint!("write_thread: waiting for request");
        let write_request = WRITE_REQUESTS.1.recv().unwrap();

        crate::replacements::thread_checkpoint!("write_thread: got request");
        let current_stream = write_request.stream;
        let zone = *statics::STREAM_TO_HINT
            .get(&current_stream)
            .unwrap()
            .value();

        crate::replacements::thread_checkpoint!("write_thread: acquiring zone lock");

        let zone_lock = ZONE_LOCKS
            .entry(zone)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = zone_lock.lock().unwrap();

        crate::replacements::thread_checkpoint!("write_thread: opening zone writer");
        let mut writer = WRITE_ZONES.entry(zone).or_insert_with(|| {
            shimmer_info!("Opening new zone {} for stream {:?}", zone, current_stream);
            let file = OpenOptions::new()
                .append(true)
                .create(false)
                .custom_flags(libc::O_DIRECT)
                .open(format!("{}{}", state::ZONE_PATH, zone))
                .unwrap();

            let initial_offset = file.metadata().unwrap().len();
            shimmer_trace!(
                "Zone {} opened with initial offset: {}",
                zone,
                initial_offset
            );
            ZoneWriter::new(file, initial_offset)
        });

        crate::replacements::thread_checkpoint!("write_thread: getting buffer");
        shimmer_trace!(
            "EXTENT ALLOC: file={}, zone={}, offset=0x{:x}, stream={:?}",
            write_request.file,
            zone,
            writer.current_offset,
            current_stream
        );
        let mut buffer: Vec<u8> = getbuf!(write_request);

        crate::replacements::thread_checkpoint!("write_thread: processing buffer");
        //  let file_len = buffer.len();

        let actual_data_size = buffer.len() as u32;

        if buffer.is_empty() {
            crate::replacements::thread_checkpoint!("write_thread: empty buffer, skipping");
            let write_response =
                WriteResponse::new(write_request.id, write_request.fd, write_request.buffer);
            // return_buffer(write_request.buffer);
            WRITE_SUCCESS.insert(write_request.id, write_response);
            continue;
        }

        crate::replacements::thread_checkpoint!("write_thread: aligning buffer");

        let aligned_size = align_to_page(actual_data_size) as usize;
        if buffer.len() != aligned_size {
            buffer.resize(aligned_size, 0);
            shimmer_debug!(
                "Aligned buffer from {} to {} bytes ({}K padding)",
                actual_data_size,
                aligned_size,
                (aligned_size - actual_data_size as usize) / 1024
            );
        }

        // Check if write would exceed zone capacity
        if writer.current_offset + aligned_size as u64 > state::get_zone_capacity() as u64 {
            shimmer_info!(
                "Write would exceed zone {} capacity (offset=0x{:x}, size={}, capacity={}), rotating to new zone",
                zone,
                writer.current_offset,
                aligned_size,
                state::get_zone_capacity()
            );

            if let Some(mut zone_metadata) = statics::ZONE_METADATA_MAP.get_mut(&zone) {
                zone_metadata.mark_full();
            }
            let _ = writer.set_len(state::get_zone_size() as u64);
            drop(writer);
            helpers::assign_zone(current_stream);
            WRITE_ZONES.remove(&zone);

            // Retry this write request with the new zone
            if WRITE_REQUESTS.0.try_send(write_request).is_err() {
                shimmer_error!("Failed to re-queue write request after zone rotation");
                panic!("Write queue full after zone rotation");
            }
            continue;
        }

        crate::replacements::thread_checkpoint!("write_thread: updating filesystem");
        statics::FILESYSTEM
            .entry(write_request.file)
            .and_modify(|f| {
                f.file_size += actual_data_size;
                // Get or initialize zone metadata
                let zone_extent_num = statics::ZONE_METADATA_MAP
                    .entry(zone)
                    .or_default()
                    .add_extent();
                f.extents.push(state::Extents::new(
                    zone,
                    writer.current_offset as u32,
                    actual_data_size,
                    zone_extent_num,
                ));
            })
            .or_insert_with(|| {
                let mut file = state::Files::new(actual_data_size);
                let zone_extent_num = statics::ZONE_METADATA_MAP
                    .entry(zone)
                    .or_default()
                    .add_extent();
                file.extents.push(state::Extents::new(
                    zone,
                    writer.current_offset as u32,
                    actual_data_size,
                    zone_extent_num,
                ));
                file
            });
        crate::replacements::thread_checkpoint!("write_thread: writing to zone");
        shimmer_trace!(
            "Persisting {} aligned to {} at offset 0x{:x}",
            actual_data_size,
            aligned_size,
            writer.current_offset
        );
        if let Err(e) = write_variable_size_buffer(&mut writer, &buffer) {
            shimmer_error!(
                "FATAL: Failed to write to zone {}: {} (error kind: {:?})",
                zone,
                e,
                e.kind()
            );
            panic!("Zone write failed: {e}");
        }

        crate::replacements::thread_checkpoint!("write_thread: updating counters");

        BYTES_WRITTEN.fetch_add(aligned_size as u64, atomic::Ordering::Relaxed);
        writer.current_offset += aligned_size as u64;

        crate::replacements::thread_checkpoint!("write_thread: checking zone rotation");
        if writer.current_offset >= state::get_zone_capacity() as u64 {
            shimmer_info!(
                "Zone {} reached capacity, rotating to new zone for stream {:?}",
                zone,
                current_stream
            );

            if let Some(mut zone_metadata) = statics::ZONE_METADATA_MAP.get_mut(&zone) {
                zone_metadata.mark_full();
            }
            let _ = writer.set_len(state::get_zone_size() as u64);
            drop(writer);
            helpers::assign_zone(current_stream);
            let current_zone = WRITE_ZONES.remove(&zone).unwrap();
            drop(current_zone);
        }

        crate::replacements::thread_checkpoint!("write_thread: marking success");
        let write_response =
            WriteResponse::new(write_request.id, write_request.fd, write_request.buffer);
        WRITE_SUCCESS.insert(write_request.id, write_response);
    }
}

/// Performs a read operation from extents and write cache.
///
/// This function handles reading data by:
/// 1. Checking if the file belongs to Shimmer
/// 2. Finding the extent(s) containing the requested offset
/// 3. Reading from zone files (extents) via direct syscalls
/// 4. Falling back to write cache for data not yet persisted
/// 5. Handling reads that span multiple extents
///
/// # Arguments
/// * `fd` - File descriptor to read from
/// * `buf` - Buffer to read data into
/// * `count` - Number of bytes to read
/// * `offset` - Offset in the logical file to read from
///
/// # Returns
/// Number of bytes successfully read
fn perform_read(fd: i32, buf: *mut libc::c_void, count: usize, offset: i64) -> usize {
    crate::replacements::thread_checkpoint!("perform_read: started");
    if !FD_TO_UID.contains_key(&fd) {
        crate::replacements::thread_checkpoint!("perform_read: not shimmer file, using syscall");
        shimmer_debug!("Skipped read - not shimmer file");
        unsafe {
            return libc::syscall(libc::SYS_pread64, fd, buf, count, offset) as usize;
        }
    }

    crate::replacements::thread_checkpoint!("perform_read: getting file info");
    let id = *FD_TO_UID.get(&fd).unwrap().value();
    let file_extents = &FILESYSTEM.get(&id).unwrap().extents;

    if file_extents.is_empty() {
        crate::replacements::thread_checkpoint!("perform_read: no extents, trying buffer");
        let bytes_read = try_read_from_buffer(fd, buf, count, offset, 0, 0, "no extents");
        if bytes_read > 0 {
            crate::replacements::thread_checkpoint!("perform_read: buffer read success");
            return bytes_read;
        }
        crate::replacements::thread_checkpoint!("perform_read: no buffer data");
        shimmer_error!("No file extents");
        return 0;
    }

    crate::replacements::thread_checkpoint!("perform_read: processing extents");
    let mut remaining_bytes = count;
    let mut buffer_offset: usize = 0;
    let mut total_read: usize = 0;

    // Find extent containing the starting offset
    let mut logical_offset = 0i64;
    let mut extent_idx = file_extents.len(); // Start with invalid index

    crate::replacements::thread_checkpoint!("perform_read: starting extent loop");
    for (idx, extent) in file_extents.iter().enumerate() {
        shimmer_debug!(
            "Checking extent {}: logical_offset={}, extent.actual_size={}, offset={}",
            idx,
            logical_offset,
            extent.actual_size,
            offset
        );
        if offset >= logical_offset && offset < logical_offset + extent.actual_size as i64 {
            shimmer_debug!("Found extent {} for offset {}", idx, offset);
            extent_idx = idx;
            break;
        }
        logical_offset += extent.actual_size as i64;
    }

    // Try buffer read if offset is beyond all extents
    if extent_idx >= file_extents.len() {
        shimmer_debug!("Trying to read from buffer, extent_idx={}", extent_idx);
        let bytes_read =
            try_read_from_buffer(fd, buf, count, offset, logical_offset, 0, "beyond extents");
        return bytes_read;
    }

    // Read from extents sequentially
    while remaining_bytes > 0 && extent_idx < file_extents.len() {
        let extent = file_extents[extent_idx];

        // Offset within this extent
        let offset_in_extent = offset + total_read as i64 - logical_offset;

        // Skip if past this extent
        if offset_in_extent >= extent.actual_size as i64 {
            shimmer_warn!(
                "Reading from multiple extents {offset_in_extent} > {}",
                extent.actual_size
            );
            logical_offset += extent.actual_size as i64;
            extent_idx += 1;
            continue;
        }

        // Open zone file for reading
        let zone_fd = READONLY_FILES
            .entry(extent.zone)
            .or_insert_with(|| {
                let new_file = BufReader::new(
                    std::fs::OpenOptions::new()
                        .read(true)
                        .open(format!("{}{}", state::ZONE_PATH, extent.zone))
                        .unwrap(),
                );
                let new_fd = new_file.get_ref().as_raw_fd();
                (new_file, new_fd)
            })
            .1;

        // Calculate bytes to read from this extent
        let bytes_available_in_extent = extent.actual_size as i64 - offset_in_extent;
        let bytes_to_read =
            std::cmp::min(remaining_bytes as i64, bytes_available_in_extent) as usize;
        let zone_offset = extent.offset as i64 + offset_in_extent;

        shimmer_debug!(
            "Reading {} bytes from extent {} (zone {}) at zone_offset 0x{:x} (file_offset=0x{:x})",
            bytes_to_read,
            extent_idx,
            extent.zone,
            zone_offset,
            offset + total_read as i64
        );

        // Perform zone read
        let buf_ptr = (buf as usize + buffer_offset) as *mut libc::c_void;
        let bytes_read: isize;
        unsafe {
            bytes_read = libc::syscall(
                libc::SYS_pread64,
                zone_fd,
                buf_ptr,
                bytes_to_read,
                zone_offset,
            ) as isize;
        }

        if bytes_read <= 0 {
            shimmer_error!("Read error {bytes_read}");
            break;
        }

        total_read += bytes_read as usize;
        remaining_bytes -= bytes_read as usize;
        buffer_offset += bytes_read as usize;

        // Handle short read
        if bytes_read < bytes_to_read as isize {
            shimmer_error!("Hit EOF in zone");
            break;
        }

        // Move to next extent if finished
        if offset_in_extent + bytes_read as i64 >= extent.actual_size as i64 {
            logical_offset += extent.actual_size as i64;
            extent_idx += 1;
        }
    }

    // Try buffer read for remaining bytes after extents
    if remaining_bytes > 0 && extent_idx >= file_extents.len() {
        let buffer_start_offset = logical_offset;
        let bytes_read = try_read_from_buffer(
            fd,
            buf,
            remaining_bytes,
            offset + total_read as i64,
            buffer_start_offset,
            buffer_offset,
            "continuation",
        );
        total_read += bytes_read;
    }

    crate::replacements::thread_checkpoint!("perform_read: completed extent processing");
    total_read
}

/// Performs readahead for the specified file range.
///
/// Issues readahead syscalls to the underlying zone files for the extents
/// that overlap with the requested range. This hints to the kernel to
/// prefetch data into the page cache.
///
/// # Arguments
/// * `fd` - File descriptor to readahead for
/// * `offset` - Starting offset in the logical file
/// * `count` - Number of bytes to readahead
///
/// # Returns
/// Always returns 0
pub fn perform_readahead(fd: i32, offset: i64, count: usize) -> i32 {
    let id = *FD_TO_UID.get(&fd).unwrap().value();
    let file_extents = &FILESYSTEM.get(&id).unwrap().extents;

    if file_extents.is_empty() {
        shimmer_debug!("No extents for readahead");
        return 0;
    }

    // Find extents containing the readahead range
    let mut logical_offset = 0i64;
    let end_offset = offset + count as i64;

    for extent in file_extents.iter() {
        let extent_start = logical_offset;
        let extent_end = logical_offset + extent.actual_size as i64;

        // Check extent overlap with readahead range
        if offset < extent_end && end_offset > extent_start {
            // Open zone file for readahead
            let zone_fd = READONLY_FILES
                .entry(extent.zone)
                .or_insert_with(|| {
                    let new_file = BufReader::new(
                        std::fs::OpenOptions::new()
                            .read(true)
                            .open(format!("{}{}", state::ZONE_PATH, extent.zone))
                            .unwrap(),
                    );
                    let new_fd = new_file.get_ref().as_raw_fd();
                    (new_file, new_fd)
                })
                .1;

            // Calculate overlap region
            let ra_start_in_extent = std::cmp::max(0, offset - extent_start);
            let ra_end_in_extent =
                std::cmp::min(extent.actual_size as i64, end_offset - extent_start);
            let ra_count = (ra_end_in_extent - ra_start_in_extent) as usize;

            if ra_count > 0 {
                let zone_offset = extent.offset as i64 + ra_start_in_extent;
                shimmer_debug!(
                    "Readahead {} bytes from extent (zone {}) at zone_offset 0x{:x}",
                    ra_count,
                    extent.zone,
                    zone_offset
                );

                // Issue readahead syscall
                unsafe {
                    libc::syscall(libc::SYS_readahead, zone_fd, zone_offset, ra_count);
                }
            }
        }

        logical_offset += extent.actual_size as i64;

        // Stop if past readahead range
        if logical_offset >= end_offset {
            break;
        }
    }

    0
}

/// Main read thread that processes read requests asynchronously.
///
/// Receives read requests from the `READ_REQUESTS` channel, processes them
/// by calling `perform_read`, and stores the results in `READ_SUCCESS`.
/// This allows read operations to be handled asynchronously in the background.
///
/// Runs in an infinite loop processing read requests sequentially.
pub fn read_thread() {
    crate::replacements::thread_checkpoint!("read_thread: started");

    loop {
        crate::replacements::thread_checkpoint!("read_thread: waiting for request");
        let read_request = READ_REQUESTS.1.recv().unwrap();

        crate::replacements::thread_checkpoint!("read_thread: got request");

        shimmer_debug!(
            "Read thread processing fd={}, offset=0x{:x}, count={}",
            read_request.fd,
            read_request.offset,
            read_request.count,
        );

        crate::replacements::thread_checkpoint!("read_thread: performing read");
        let bytes_read: usize = perform_read(
            read_request.fd,
            read_request.buf_ptr as *mut libc::c_void,
            read_request.count,
            read_request.offset,
        );

        crate::replacements::thread_checkpoint!("read_thread: creating response");

        let response = state::ReadResponse {
            bytes_read,
            success: bytes_read > 0,
        };

        crate::replacements::thread_checkpoint!("read_thread: sending response");
        READ_SUCCESS.insert(read_request.id, response);
    }
}
