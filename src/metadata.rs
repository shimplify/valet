//! Metadata persistence for state recovery.

use crate::state::*;
use crate::statics::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

const METADATA_PATH: &str = "/mnt/shimmer/shimmer_metadata.json";
const METADATA_BACKUP: &str = "/mnt/shimmer/shimmer_metadata.json.backup";
static METADATA_SAVE_MUTEX: Mutex<()> = Mutex::new(());
#[derive(Serialize, Deserialize, Debug)]
pub struct ShimmerMetadata {
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub path_to_uid: HashMap<String, u32>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub filesystem: HashMap<u32, Files>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub zone_metadata: HashMap<u32, Zone>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub free_zones: Vec<u32>,
    pub next_uid: u32,
}

impl ShimmerMetadata {
    /// Collects current state from global variables.
    pub fn from_globals() -> Self {
        // Collect data from globals
        let path_to_uid = PATH_TO_UID
            .iter()
            .map(|entry| (entry.key().clone(), *entry.value()))
            .collect::<HashMap<String, u32>>();

        let filesystem = FILESYSTEM
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect::<HashMap<u32, Files>>();

        let zone_metadata = ZONE_METADATA_MAP
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect::<HashMap<u32, Zone>>();

        let free_zones = FREE_ZONE.lock().unwrap().iter().cloned().collect();

        let next_uid = UID.load(Ordering::SeqCst);

        Self {
            path_to_uid,
            filesystem,
            zone_metadata,
            free_zones,
            next_uid,
        }
    }

    /// Restores state to global variables.
    pub fn restore_to_globals(&self) {
        // Clear existing state
        PATH_TO_UID.clear();
        FILESYSTEM.clear();
        ZONE_METADATA_MAP.clear();
        FREE_ZONE.lock().unwrap().clear();

        // Restore state
        for (path, uid) in &self.path_to_uid {
            PATH_TO_UID.insert(path.clone(), *uid);
        }

        for (uid, files) in &self.filesystem {
            FILESYSTEM.insert(*uid, files.clone());
        }

        for (zone_id, zone) in &self.zone_metadata {
            ZONE_METADATA_MAP.insert(*zone_id, zone.clone());
        }

        let mut free_zone_guard = FREE_ZONE.lock().unwrap();
        for zone in &self.free_zones {
            free_zone_guard.push_back(*zone);
        }
        drop(free_zone_guard);

        UID.store(self.next_uid, Ordering::SeqCst);
    }
}

/// Saves current metadata to disk atomically.
pub fn save_metadata() -> io::Result<()> {
    // Acquire save lock, skip if already in progress
    let _lock = match METADATA_SAVE_MUTEX.try_lock() {
        Ok(lock) => {
            shimmer_debug!("Acquired metadata save lock");
            lock
        }
        Err(_) => {
            shimmer_debug!("Metadata save already in progress, skipping");
            return Ok(());
        }
    };

    let metadata = match std::panic::catch_unwind(ShimmerMetadata::from_globals) {
        Ok(data) => data,
        Err(_) => {
            return Err(io::Error::other("Failed to collect metadata"));
        }
    };

    let json_data = match serde_json::to_string_pretty(&metadata) {
        Ok(data) => data,
        Err(e) => {
            shimmer_error!("Serialization failed: {}", e);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Serialization failed",
            ));
        }
    };

    // Write to temporary file for atomic rename
    let temp_path = format!("{METADATA_PATH}.tmp");
    shimmer_debug!("Writing to {temp_path}");
    let mut file = File::create(&temp_path)?;
    file.write_all(json_data.as_bytes())
        .expect("Failed to write data");
    file.sync_all().expect("Failed to sync metadata changes");

    // Atomic rename
    if fs::exists(&temp_path).expect("Unable to verify path") {
        shimmer_debug!("Renaming {temp_path} to {METADATA_PATH}");
        fs::rename(temp_path, METADATA_PATH).expect("Failed to rename metadata");
    }

    Ok(())
}

/// Loads metadata from disk and restores to global state.
///
/// # Returns
///
/// `true` if metadata was loaded, `false` if no metadata file exists.
pub fn load_metadata() -> io::Result<bool> {
    if !Path::new(METADATA_PATH).exists() {
        return Ok(false);
    }

    let json_data = match fs::read_to_string(METADATA_PATH) {
        Ok(data) => data,
        Err(_) => {
            // Try backup if main file fails
            if Path::new(METADATA_BACKUP).exists() {
                fs::read_to_string(METADATA_BACKUP)?
            } else {
                return Ok(false);
            }
        }
    };

    let metadata: ShimmerMetadata = serde_json::from_str(&json_data)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    metadata.restore_to_globals();
    Ok(true)
}

/// Removes metadata files.
pub fn cleanup_metadata() -> io::Result<()> {
    let _ = fs::remove_file(METADATA_PATH);
    let _ = fs::remove_file(METADATA_BACKUP);
    Ok(())
}
