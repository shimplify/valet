//! Libc function replacements for intercepting file operations.

use crate::helpers;
use crate::helpers::*;
use crate::learning;
use crate::persist::*;
use crate::state;
use crate::state::*;
use crate::statics;
use crate::statics::*;

/// Time-based backoff for waiting loops.
fn smart_wait(elapsed: std::time::Duration) {
    match elapsed.as_micros() {
        0..=20 => std::thread::yield_now(),
        21..=1000 => std::hint::spin_loop(),
        _ => std::thread::sleep(std::time::Duration::from_micros(10)),
    }
}
use crate::{shimmer_debug, shimmer_error, shimmer_info, shimmer_warn};
use ctor::ctor;
use ctor::dtor;
use std::collections::HashSet;
use std::ffi::*;
use std::os::fd::AsRawFd;
use std::sync::LazyLock;
use std::sync::atomic::Ordering;

#[cfg(feature = "debug-tracing")]
use backtrace::Backtrace;
#[cfg(feature = "debug-tracing")]
use chrono::Utc;
#[cfg(feature = "debug-tracing")]
use signal_hook::{consts::SIGINT, iterator::Signals};
#[cfg(feature = "debug-tracing")]
use std::collections::HashMap;
#[cfg(feature = "debug-tracing")]
use std::sync::Mutex;
#[cfg(feature = "debug-tracing")]
use std::sync::atomic::AtomicBool;
#[cfg(feature = "debug-tracing")]
use std::thread;

#[cfg(feature = "debug-tracing")]
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "debug-tracing")]
pub static THREAD_STATUS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

macro_rules! thread_checkpoint {
    ($location:expr) => {
        #[cfg(feature = "debug-tracing")]
        {
            if let Ok(mut status) = crate::replacements::THREAD_STATUS.lock() {
                status.insert(
                    format!("{:?}", std::thread::current().id()),
                    format!(
                        "{} - {}",
                        $location,
                        chrono::Utc::now().format("%H:%M:%S%.3f")
                    ),
                );
            }
        }
    };
}

pub(crate) use thread_checkpoint;

fn try_save_metadata() {
    // Non-blocking request to metadata save thread
    if METADATA_SAVE_REQUESTS.0.try_send(()).is_err() {
        shimmer_debug!("Metadata save channel full, skipping save request");
    }
}

#[cfg(feature = "debug-tracing")]
fn setup_signal_handler() {
    thread::spawn(|| {
        let mut signals = Signals::new([SIGINT]).unwrap();
        for sig in signals.forever() {
            if sig == SIGINT {
                print_debug_info();
                SHUTDOWN.store(true, Ordering::Relaxed);
                std::process::exit(1);
            }
        }
    });
}

#[cfg(feature = "debug-tracing")]
fn print_debug_info() {
    println!("\n=== SIGINT DEBUG INFO ===");
    println!("Timestamp: {}", Utc::now().format("%Y-%m-%d %H:%M:%S%.3f"));

    println!("\n=== Main Thread Backtrace ===");
    let bt = Backtrace::new();
    println!("{bt:?}");

    println!("\n=== Thread Checkpoints ===");
    if let Ok(status) = THREAD_STATUS.lock() {
        for (thread_id, location) in status.iter() {
            println!("Thread {thread_id}: {location}");
        }
    } else {
        println!("Failed to lock thread status");
    }

    println!("\n=== Channel Status ===");
    println!(
        "Write requests pending: {}",
        crate::persist::WRITE_REQUESTS.1.len()
    );
    println!(
        "Read requests pending: {}",
        crate::persist::READ_REQUESTS.1.len()
    );
    println!(
        "Active write successes: {}",
        crate::persist::WRITE_SUCCESS.len()
    );
    println!(
        "Active read responses: {}",
        crate::persist::READ_SUCCESS.len()
    );

    println!("\n=== Buffer Pool Status ===");
    println!("Buffer pool size: {}", crate::statics::BUFFER_POOL.len());
    println!(
        "File to buffer mappings: {}",
        crate::statics::FILE_TO_BUFFER_POOL.len()
    );
    let free_count = crate::statics::BUFFER_POOL
        .iter()
        .filter(|entry| entry.is_free)
        .count();
    println!("Free buffers: {free_count}");

    println!("\n=== Filesystem Status ===");
    println!("Tracked files: {}", crate::statics::FILESYSTEM.len());
    println!("FD to UID mappings: {}", crate::statics::FD_TO_UID.len());

    println!("\n=== Detailed Channel Analysis ===");
    println!(
        "Next WRITE_ID: {}",
        crate::state::WRITE_ID.load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "Next READ_ID: {}",
        crate::state::READ_ID.load(std::sync::atomic::Ordering::Relaxed)
    );

    println!("\n=== Active Success IDs ===");
    println!(
        "WRITE_SUCCESS IDs: {:?}",
        WRITE_SUCCESS
            .iter()
            .map(|entry| *entry.key())
            .collect::<Vec<_>>()
    );
    println!(
        "READ_SUCCESS IDs: {:?}",
        READ_SUCCESS
            .iter()
            .map(|entry| *entry.key())
            .collect::<Vec<_>>()
    );

    println!("\n=== File Descriptor Mappings ===");
    for entry in crate::statics::FD_TO_UID.iter() {
        println!("FD {} -> UID {}", entry.key(), entry.value());
    }

    println!("\n=== Active Buffer Mappings ===");
    for entry in crate::statics::FILE_TO_BUFFER_POOL.iter() {
        let buffer_size = crate::statics::BUFFER_POOL
            .get(entry.value())
            .map(|b| b.data.len())
            .unwrap_or(0);
        println!(
            "FD {} -> Buffer {} (size: {})",
            entry.key(),
            entry.value(),
            buffer_size
        );
    }

    println!("\n=== Debug info complete ===\n");
}

#[ctor]
/// Initializes Shimmer on library load.
fn setup() {
    // Initialize zone configuration
    state::init_config();

    shimmer_info!("Shimmer logging initialized");

    #[cfg(feature = "debug-tracing")]
    setup_signal_handler();

    let zone_num = state::get_zone_num();

    // Load existing metadata or initialize fresh state
    match crate::metadata::load_metadata() {
        Ok(true) => {
            shimmer_info!("Successfully loaded existing metadata");
        }
        Ok(false) => {
            shimmer_info!("No existing metadata found, initializing fresh state");
            for i in 0..zone_num {
                ZONE_METADATA_MAP.insert(i, Zone::new());
            }
            FREE_ZONE
                .lock()
                .unwrap()
                .append(&mut (2..zone_num).collect());
        }
        Err(e) => {
            shimmer_warn!("Failed to load metadata: {}, initializing fresh", e);
            for i in 0..zone_num {
                ZONE_METADATA_MAP.insert(i, Zone::new());
            }
            FREE_ZONE
                .lock()
                .unwrap()
                .append(&mut (0..zone_num).collect());
        }
    }
    let buff = LazyLock::force(&BUFFER_POOL);
    assert!(buff.is_empty());
    for i in 0..NUM_BUFFERS {
        BUFFER_POOL.entry(i as u32).or_default();
    }

    assign_zone(Stream::LOG);
    assign_zone(Stream::SSTable);

    shimmer_info!(
        "Starting background threads for write processing, garbage collection, and metadata saving"
    );

    std::thread::spawn(|| {
        write_thread();
    });

    #[cfg(not(feature = "disable-gc"))]
    std::thread::spawn(|| {
        garbage_collect();
    });

    std::thread::spawn(|| {
        metadata_save_thread();
    });

    std::thread::spawn(|| {
        shimmer_info!("Read thread task started");
        crate::persist::read_thread();
    });

    shimmer_info!(
        "Shimmer initialization complete - {} buffers, {} zones available",
        NUM_BUFFERS,
        zone_num
    );
}

#[dtor]
fn destroy() {
    shimmer_info!(
        "Total Bytes Written {} MiB",
        BYTES_WRITTEN.fetch_add(0, Ordering::Relaxed) / (1024 * 1024)
    );

    try_save_metadata();
}

// ---- OPEN, CLOSE, DELETE ----

redhook::hook! {
    unsafe fn open(pathname: *const libc::c_char, flags: libc::c_int, mode: libc::mode_t) -> libc::c_int => my_open {
        let fd: libc::c_int;
        let path: &str;
        unsafe {
            fd = redhook::real!(open)(pathname, flags, mode);
            path = CStr::from_ptr(pathname).to_str().unwrap();
        }

        // Track RocksDB files
        if path.ends_with(".sst") || path.ends_with(".log") || path.ends_with(".lsm") {
            // Extract filename for clustering prediction
            let filename = match std::path::Path::new(path).file_name() {
                Some(file) => file.to_string_lossy().to_string(),
                None => path.to_string(),
            };

            // Use clustering to predict stream type
            let stream_type = learning::get_stream(filename.clone(), flags, mode);
            FD_TO_STREAM.entry(fd).or_insert_with(|| stream_type);

            let id = PATH_TO_UID.entry(path.to_owned()).or_insert(UID.fetch_add(1,
                    Ordering::Relaxed)).to_owned();
            FD_TO_UID.insert(fd, id);
            FILESYSTEM.entry(id).or_insert_with(|| Files::new(0));

            shimmer_debug!("Opened file: {} as fd={}, uid={}, predicted_stream={:?}",
                          path, fd, id, stream_type);
        }
        fd
    }
}

redhook::hook! {
    unsafe fn close(fd: libc::c_int) -> libc::c_int => my_close {
        if fd < 5 {
            unsafe {
                return redhook::real!(close)(fd)
            }
        }
        if FILE_TO_BUFFER_POOL.contains_key(&fd) {
            shimmer_debug!("Closing shimmer-managed fd={}", fd);

            // Sync and queue final write
            let _ = handle_sync(fd, true);

            // Clear buffer mapping
            if let Some((_, buffer_num)) = FILE_TO_BUFFER_POOL.remove(&fd) {
                helpers::return_buffer(buffer_num);
                shimmer_debug!("Cleared write cache buffer {} for fd={} on close", buffer_num, fd);
            }
        }
            let _ = FD_TO_UID.remove(&fd);
            let _ = FD_TO_STREAM.remove(&fd);

        unsafe {
            redhook::real!(close)(fd)
        }
    }
}

redhook::hook! {
    unsafe fn unlink(pathname: *const libc::c_char) -> libc::c_int => my_unlink {
        let path: String;
        unsafe {
            path = CStr::from_ptr(pathname).to_str().unwrap().to_string();
        }
        if let Some ((_, file_id)) = statics::PATH_TO_UID.remove(&path) {
            shimmer_debug!("Unlinking shimmer file: {} (id: {})", path, file_id);
            if let Some ((_, file_metadata)) = statics::FILESYSTEM.remove(&file_id) {
                let mut zones_for_gc = HashSet::<u32>::new();

                for extent in &file_metadata.extents {
                    if let Some(mut zone_metadata) = statics::ZONE_METADATA_MAP.get_mut(&extent.zone) &&
                        zone_metadata.mark_extent_deleted(extent.zone_extent_num, extent.actual_size) {
                            shimmer_debug!(
                                 "Marked extent {} as deleted in zone {} (size: {})",
                                 extent.zone_extent_num,
                                 extent.zone,
                                 extent.actual_size
                             );
                             if zone_metadata.should_gc() {
                                 zones_for_gc.insert(extent.zone);
                             }

                        }
                }

                #[cfg(not(feature = "disable-gc"))]
                 for zone in zones_for_gc {
                     shimmer_info!("Triggering GC for zone {} (efficiency: {:.1}%)",
                         zone,
                         statics::ZONE_METADATA_MAP.get(&zone)
                             .map(|z| z.gc_efficiency() * 100.0)
                             .unwrap_or(0.0)
                     );
                     if let Err(e) = CLEANUP_REQUESTS.0.try_send(zone) {
                         shimmer_warn!("Failed to send GC request for zone {}: {:?}", zone, e);
                     }
                 }
            }

        }
        shimmer_debug!("Deleting {path}");
        unsafe {
            redhook::real!(unlink)(pathname)
        }
    }
}

// ---- WRITE FUNCTIONS ----

redhook::hook! {
    unsafe fn write(
        fd: libc::c_int,
        buf: *const libc::c_void,
        nbytes: libc::size_t
    ) -> libc::c_int => my_write {
        unsafe {
            handle_write(fd, buf, nbytes, 0, false)
        }
    }
}
redhook::hook! {
    unsafe fn pwrite(
        fd: libc::c_int,
        buf: *const libc::c_void,
        count: libc::size_t,
        offset: libc::off_t
    ) -> libc::c_int => my_pwrite {
        unsafe {
            handle_write(fd, buf, count, offset, true)
        }
    }
}

fn queue_buffer_for_write(fd: i32, buffer_num: u32, stream: Stream) {
    let request_id = WRITE_ID.fetch_add(1, Ordering::Relaxed);
    let request = WriteRequest::new(
        request_id,
        *FD_TO_UID.get(&fd).unwrap().value(),
        buffer_num,
        stream,
        fd,
    );

    match WRITE_REQUESTS.0.try_send(request) {
        Ok(()) => {
            shimmer_debug!(
                "Queued write request {request_id} for fd={}, buffer={}",
                fd,
                buffer_num
            );
            let start_time = std::time::Instant::now();
            loop {
                if let Some(write_response) = WRITE_SUCCESS.remove(&request_id) {
                    let _freed_buffer = write_response.1.buffer;
                    shimmer_debug!(
                        "Write {} completed for buffer {}",
                        write_response.0,
                        _freed_buffer
                    );
                    shimmer_debug!(
                        "Active requests: {}, Uncleared: {}, Free Buffers: {}, FD_BUFFER {} ",
                        WRITE_REQUESTS.0.len(),
                        WRITE_SUCCESS.len(),
                        BUFFER_POOL.iter().filter(|f| f.is_free).count(),
                        FILE_TO_BUFFER_POOL.len()
                    );
                    return;
                }
                if start_time.elapsed().as_secs() >= 30 {
                    shimmer_error!("Write timeout for request_id={}", request_id);
                    panic!("Write timed out");
                }
                std::thread::yield_now();
            }
        }
        Err(_) => {
            shimmer_error!("Write queue full! Unable to send request for fd={}", fd);
            panic!();
        }
    }
}

fn append_to_buffer(fd: i32, mut buffer_num: u32, stream: Stream, data: &[u8]) {
    let mut remaining_data = data;
    let mut flag = false;

    while !remaining_data.is_empty() {
        let current_len = {
            let current_buffer = BUFFER_POOL.get(&buffer_num).unwrap();
            current_buffer.data.len()
        };

        let available_space;
        let max_single_buffer = PREFERRED_BUFFER_SIZE;
        if current_len >= max_single_buffer {
            shimmer_debug!("Queueing {buffer_num} for write");
            queue_buffer_for_write(fd, buffer_num, stream);
            buffer_num = get_unused_buffer();
            shimmer_debug!("Allocating buffer {buffer_num} for {fd}");
            FILE_TO_BUFFER_POOL.insert(fd, buffer_num);
            available_space = max_single_buffer;
        } else {
            available_space = max_single_buffer - current_len;
        }

        if available_space >= remaining_data.len() {
            if flag {
                shimmer_debug!(
                    "Buffer fill: current_len={}, available_space={}, remaining_data.len()={}, max_single_buffer={}",
                    current_len,
                    available_space,
                    remaining_data.len(),
                    max_single_buffer
                );
            }
            BUFFER_POOL.entry(buffer_num).and_modify(|buffer| {
                buffer.data.extend_from_slice(remaining_data);
            });
            break;
        } else {
            shimmer_debug!(
                "Buffer fill: current_len={}, available_space={}, remaining_data.len()={}, max_single_buffer={}",
                current_len,
                available_space,
                remaining_data.len(),
                max_single_buffer
            );
            let (chunk, rest) = remaining_data.split_at(available_space);
            BUFFER_POOL.entry(buffer_num).and_modify(|buffer| {
                shimmer_debug!("Buffer length is {}", buffer.data.len() + chunk.len());
                buffer.data.extend_from_slice(chunk);
            });
            shimmer_debug!("Queuing {buffer_num} for write");
            queue_buffer_for_write(fd, buffer_num, stream);
            let new_buffer_num = get_unused_buffer();
            shimmer_debug!("Allocating buffer {new_buffer_num} for {fd}");
            FILE_TO_BUFFER_POOL.insert(fd, new_buffer_num);
            buffer_num = new_buffer_num;
            flag = true;

            remaining_data = rest;
        }
    }
}

/// Handles write operations for tracked files.
///
/// Buffers data for tracked files and queues writes to zones. Non-tracked files
/// are passed through to the real system call.
///
/// # Safety
///
/// Dereferences raw pointer to read buffer data.
pub unsafe fn handle_write(
    fd: libc::c_int,
    buf: *const libc::c_void,
    nbytes: libc::size_t,
    offset: libc::off_t,
    is_pwrite: bool,
) -> libc::c_int {
    if !FD_TO_STREAM.contains_key(&fd) {
        return if is_pwrite {
            unsafe { redhook::real!(pwrite)(fd, buf, nbytes, offset) }
        } else {
            unsafe { redhook::real!(write)(fd, buf, nbytes) }
        };
    }
    if nbytes == 4096 {
        unsafe {
            redhook::real!(pwrite)(fd, buf, nbytes, offset);
        }
    }
    let current_stream = *FD_TO_STREAM.get(&fd).unwrap().value();
    let buffer_num = *FILE_TO_BUFFER_POOL
        .entry(fd)
        .or_insert_with(get_unused_buffer)
        .value();

    let input_data;
    unsafe {
        input_data = core::slice::from_raw_parts(buf as *const u8, nbytes);
    }
    append_to_buffer(fd, buffer_num, current_stream, input_data);

    nbytes.try_into().unwrap()
}

// ---- READ FUNCTIONS ----

redhook::hook! {
    unsafe fn read(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        nbytes: libc::size_t
    ) -> libc::c_int => my_read {
        unsafe {
            redhook::real!(read)(fd, buf, nbytes)
        }
    }
}

redhook::hook! {
    unsafe fn pread(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        count: libc::size_t,
        offset: libc::off_t) -> libc::size_t => my_pread
    {
        if !FD_TO_UID.contains_key(&fd) {
            shimmer_debug!("Skipped read");
            unsafe {
                return redhook::real!(pread)(fd, buf, count, offset);
            }
        }
        let request_id = READ_ID.fetch_add(1, Ordering::Relaxed);
        let request = state::ReadRequest {
            id: request_id,
            fd,
            count,
            offset,
            buf_ptr: buf as usize,
        };

        shimmer_debug!("Sending read request id={}, fd={}, offset=0x{:x}, count={}",
                      request_id, fd, offset, count);
        match READ_REQUESTS.0.try_send(request) {
            Ok(()) => {},
            Err(_) => {
                shimmer_error!("Unable to queue read request");
                panic!();
            }
        }

        let start_time = std::time::Instant::now();
        loop {
            if let Some(response) = READ_SUCCESS.remove(&request_id) {
                let bytes_read = response.1.bytes_read;
                shimmer_debug!("Read request id={} completed, bytes_read={}", request_id, bytes_read);
                return bytes_read;
            }
            if start_time.elapsed().as_secs() >= 5 {
                shimmer_error!("READ TIMEOUT: request_id={} waiting {}s",
                              request_id, start_time.elapsed().as_secs());
                panic!("Read timed out");
            }
            smart_wait(start_time.elapsed());
        }
    }
}

// ---- MEMORY MAPPING ----

redhook::hook! {
    unsafe fn mmap(
        addr: *mut libc::c_void,
        length: libc::size_t,
        prot: libc::c_int,
        flags: libc::c_int,
        fd: libc::c_int,
        offset: libc::off_t
    ) -> *mut libc::c_void => my_mmap
{
        if fd < 5 {
            unsafe {
                return redhook::real!(mmap)(addr, length, prot, flags, fd, offset);
            }
        }
        if !FD_TO_UID.contains_key(&fd) {
            shimmer_debug!("Skipped mmap - not shimmer file");
            unsafe {
                return redhook::real!(mmap)(addr, length, prot, flags, fd, offset);
            }
        }

        shimmer_debug!("mmap request: fd={}, offset=0x{:x}, length={}", fd, offset, length);
        let id = *FD_TO_UID.get(&fd).unwrap().value();
        let file_info = match FILESYSTEM.get(&id) {
            Some(info) => info,
            None => {
                shimmer_error!("mmap: No file info for shimmer fd {}", fd);
                return libc::MAP_FAILED;
            }
        };

        if file_info.extents.is_empty() {
            shimmer_error!("mmap: No extents for shimmer fd {}", fd);
            return libc::MAP_FAILED;
        }
        let mut logical_offset = 0i64;
        let mut target_extent = None;

        for extent in &file_info.extents {
            let extent_start = logical_offset;
            let extent_end = logical_offset + extent.actual_size as i64;

            if offset >= extent_start && offset < extent_end {
                target_extent = Some((extent, extent_start));
                break;
            }
            logical_offset += extent.actual_size as i64;
        }

        let (extent, extent_start) = match target_extent {
            Some(data) => data,
            None => {
                shimmer_error!("mmap: Offset 0x{:x} beyond file bounds for fd {}", offset, fd);
                return libc::MAP_FAILED;
            }
        };
        let zone_fd = READONLY_FILES
            .entry(extent.zone)
            .or_insert_with(|| {
                let new_file = std::io::BufReader::new(
                    std::fs::OpenOptions::new()
                        .read(true)
                        .open(format!("{}{}", state::ZONE_PATH, extent.zone))
                        .unwrap(),
                );
                let new_fd = new_file.get_ref().as_raw_fd();
                (new_file, new_fd)
            })
            .1;
        let offset_in_extent = offset - extent_start;
        let zone_offset = extent.offset as i64 + offset_in_extent;
        let remaining_in_extent = extent.actual_size as i64 - offset_in_extent;
        if length as i64 > remaining_in_extent {
            shimmer_warn!(
                "mmap: Requested length {} exceeds extent boundary (only {} bytes available)",
                length, remaining_in_extent
            );
        }

        shimmer_debug!(
            "Redirecting mmap: fd {} -> zone_fd {} (zone {}), offset 0x{:x} -> 0x{:x}",
            fd, zone_fd, extent.zone, offset, zone_offset
        );
        unsafe {
            redhook::real!(mmap)(addr, length, prot, flags, zone_fd, zone_offset)
        }
    }
}

// ---- ADDITIONAL READ FUNCTIONS ----

redhook::hook! {
    unsafe fn __pread_chk(
        fd: libc::c_int,
        buf: *mut libc::c_void,
        count: libc::size_t,
        offset: libc::off_t,
        buflen: libc::size_t
    ) -> libc::ssize_t => my_pread_chk {
        shimmer_debug!("__pread_chk request: fd={}, offset=0x{:x}, count={}", fd, offset, count);
        unsafe {
            if buflen < count {
                return -1;
            }
            my_pread(fd, buf, count, offset) as isize
        }
    }
}

// ---- SYNC FUNCTIONS ----

redhook::hook! {
    unsafe fn fsync(fd: libc::c_int) -> libc::c_int => my_fsync  {
        handle_sync(fd,true)
    }
}

redhook::hook! {
    unsafe fn fdatasync(fd: libc::c_int) -> libc::c_int => my_fdatasync {
        handle_sync(fd,false)
    }
}

/// Handles sync operations by queuing buffered writes.
pub fn handle_sync(fd: libc::c_int, is_fsync: bool) -> libc::c_int {
    thread_checkpoint!("handle_sync: started");

    if FILE_TO_BUFFER_POOL.contains_key(&fd) {
        thread_checkpoint!("handle_sync: found buffer for fd");
        let buffer: u32 = *FILE_TO_BUFFER_POOL.get(&fd).unwrap().value();
        shimmer_debug!(
            "handle_sync attempting to sync buffer {} for fd {}",
            buffer,
            fd
        );

        queue_buffer_for_write(fd, buffer, FD_TO_STREAM.get(&fd).unwrap().to_owned());
    }

    thread_checkpoint!("handle_sync: no buffer, calling real sync");
    if is_fsync {
        unsafe { redhook::real!(fsync)(fd) }
    } else {
        unsafe { redhook::real!(fdatasync)(fd) }
    }
}

redhook::hook! {
    unsafe fn sync_file_range(fd : libc::c_int, offset: libc::off_t, nbytes: libc::off_t, flags:
        libc::uintmax_t)-> libc::c_int => my_sync {
        if FD_TO_UID.contains_key(&fd){
            return 0;
        }
        unsafe {
            redhook::real!(sync_file_range)(fd, offset,nbytes,flags)
        }

    }
}

// ---- OTHER FS OPERATIONS ----

redhook::hook! {
    unsafe fn readahead(fd: libc::c_int, offset: libc::off64_t, count: libc::size_t) -> libc::ssize_t => my_readahead {
        shimmer_debug!("Readahead request: fd={}, offset=0x{:x}, count={}", fd, offset, count);
        if !FD_TO_UID.contains_key(&fd) {
            shimmer_debug!("Skipped readahead - not shimmer file");
            unsafe {
                return redhook::real!(readahead)(fd, offset, count);
            }
        }
        let result = perform_readahead(fd, offset, count);
        result as libc::ssize_t
    }
}

redhook::hook! {
    unsafe fn fallocate(fd: libc::c_int, mode: libc::c_int, offset: libc::off_t, len: libc::off_t) -> libc::c_int => my_fallocate {
        if FILE_TO_BUFFER_POOL.contains_key(&fd) {
            return 0
        }
        unsafe {
            redhook::real!(fallocate)(fd, mode, offset, len)
        }
    }
}

redhook::hook! {
    unsafe fn ftruncate(fd: libc::c_int, length: libc::off_t) -> libc::c_int => my_ftruncate {
        if FILE_TO_BUFFER_POOL.contains_key(&fd){
         return 0
        }
        unsafe {
            redhook::real!(ftruncate)(fd,length)
        }
    }
}
