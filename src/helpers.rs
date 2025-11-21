//! Helper utilities for buffer and zone management.

use crate::persist::WRITE_ZONES;
use crate::state::*;
use crate::statics::TrackedBuffer;
use crate::statics::*;

/// Assigns a zone to a stream by allocating a free zone from the pool.
///
/// Divides zones and allocates them based on the stream and zone fullness.
/// Allocates two zones at a time, moving on if either fills up.
///
/// # Panics
///
/// Panics if there are no free zones available in the pool.
pub fn assign_zone(input: Stream) {
    STREAM_TO_HINT
        .entry(input)
        .and_modify(|f| *f = FREE_ZONE.lock().unwrap().pop_front().unwrap())
        .or_insert_with(|| FREE_ZONE.lock().unwrap().pop_front().unwrap());
}

/// Gets the next available buffer from the buffer pool.
///
/// Searches for a free buffer in the pool and marks it as used. If no free
/// buffers exist, creates and returns a new buffer.
///
/// # Returns
///
/// The buffer ID that can be used for buffering write operations.
pub fn get_unused_buffer() -> u32 {
    // Find first free buffer in one step
    if let Some(mut entry) = BUFFER_POOL.iter_mut().find(|entry| entry.is_free) {
        let buffer_id = *entry.key();
        entry.is_free = false;
        // shimmer_warn!("Found free buffer {buffer_id}");
        return buffer_id;
    }

    // No free buffer found, drop the reference and create a new one
    let new_id = BUFFER_POOL.len() as u32;
    //shimmer_warn!("Creating new buffer {new_id}");
    let mut new_buffer = TrackedBuffer::new();
    new_buffer.is_free = false; // Mark as used immediately
    BUFFER_POOL.insert(new_id, new_buffer);
    new_id
}

/// Returns a buffer to the free pool for reuse.
///
/// Clears the buffer's data and marks it as free.
pub fn return_buffer(buffer_id: u32) {
    //shimmer_warn!("Freeing buffer {buffer_id}");
    if let Some(mut buffer) = BUFFER_POOL.get_mut(&buffer_id) {
        buffer.data.clear();
        buffer.is_free = true;
    }
}

/// Calculates the utilization percentage of a zone.
///
/// # Returns
///
/// The utilization percentage (0-100). Returns 0 if the zone doesn't exist.
pub fn get_zone_utilization(zone: u32) -> u32 {
    if let Some(zone_writer) = WRITE_ZONES.get(&zone) {
        zone_writer.current_offset as u32 * 100 / get_zone_capacity()
    } else {
        0
    }
}

/// Checks if a buffer of the given size can fit in the specified zone.
///
/// Accounts for page alignment when determining available capacity.
///
/// # Returns
///
/// `true` if the buffer will fit in the zone's remaining capacity.
pub fn can_fit_buffer(zone: u32, buffer_size: u32) -> bool {
    let aligned_size = align_to_page(buffer_size);
    if let Some(zone_writer) = WRITE_ZONES.get(&zone) {
        zone_writer.current_offset + aligned_size as u64 <= get_zone_capacity() as u64
    } else {
        true // Empty zone can fit anything reasonable
    }
}
