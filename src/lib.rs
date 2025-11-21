//! libshimmer is a dynamic library that Shims access from an application to a ZNS system
//! It can work either by adding a hint, or using a lightweight vfs layer to write to an underlying
//! ZoneFS system. The advantages of this approach are:
//! - Easy adoption of novel interfaces: no changes needed!
//! - Performance improvements, particularly for writes.

// Helper function to get current timestamp
pub fn get_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let secs = duration.as_secs();
    let nanos = duration.subsec_nanos();
    let millis = nanos / 1_000_000;

    // Convert to local time (simple approximation)
    let total_secs = secs % 86400; // seconds in a day
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
}

// ANSI color codes for terminal output
pub const RESET: &str = "\x1b[0m";
pub const GREEN: &str = "\x1b[32m"; // INFO
pub const YELLOW: &str = "\x1b[33m"; // WARN
pub const RED: &str = "\x1b[31m"; // ERROR
pub const CYAN: &str = "\x1b[36m"; // DEBUG
pub const MAGENTA: &str = "\x1b[35m"; // TRACE

// Safer logging macros that avoid complex timestamp formatting
#[macro_export]
macro_rules! shimmer_trace {
    ($($arg:tt)*) => {
        #[cfg(feature = "verbose-logging")]
        {
            use std::time::{SystemTime, UNIX_EPOCH};
            let millis = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() % 100000) // Last 5 digits for relative timing
                .unwrap_or(0);
            libc_print::std_name::println!("{:05} {}[TRACE]{} {}",
                millis,
                $crate::MAGENTA, $crate::RESET,
                format!($($arg)*));
        }
    };
}

#[macro_export]
macro_rules! shimmer_debug {
    ($($arg:tt)*) => {
        #[cfg(any(debug_assertions, feature = "verbose-logging"))]
        libc_print::std_name::println!("{} {}[DEBUG]{} {}",
            $crate::get_timestamp(),
            $crate::CYAN, $crate::RESET,
            format!($($arg)*));
    };
}

#[macro_export]
macro_rules! shimmer_info {
    ($($arg:tt)*) => {
        libc_print::std_name::println!("{} {}[INFO] {}{}",
            $crate::get_timestamp(),
            $crate::GREEN, $crate::RESET,
            format!($($arg)*));
    };
}

#[macro_export]
macro_rules! shimmer_warn {
    ($($arg:tt)*) => {
        libc_print::std_name::println!("{} {}[WARN] {}{}",
            $crate::get_timestamp(),
            $crate::YELLOW, $crate::RESET,
            format!($($arg)*));
    };
}

#[macro_export]
macro_rules! shimmer_error {
    ($($arg:tt)*) => {
        libc_print::std_name::println!("{} {}[ERROR]{} {}",
            $crate::get_timestamp(),
            $crate::RED, $crate::RESET,
            format!($($arg)*));
    };
}

/// Helper functions
pub mod helpers;
/// Clustering-based stream prediction
pub mod learning;
/// Metadata persistence
pub mod metadata;
/// Persistence Code
pub mod persist;
/// Contains replacements for `libc` functions
pub mod replacements;
/// State contains helpful primitives to maintain state for hints and for Zoned Storage
pub mod state;
/// Data Structures to maintain shimmer
pub mod statics;
