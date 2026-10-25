//! Logging infrastructure for termide.
//!
//! Provides a simple, thread-safe logging system with file output
//! and in-memory log storage for the debug panel.
//!
//! This module implements the `log` crate's `Log` trait, allowing use of
//! standard logging macros like `log::info!()`, `log::error!()`, etc.

use chrono::Local;
use log::{Level, LevelFilter, Log, Metadata, Record};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

// Re-export log macros for convenient use
pub use log::{debug, error, info, trace, warn};

/// Log entry
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Timestamp in HH:MM:SS format
    pub timestamp: String,
    /// Message level
    pub level: LogLevel,
    /// Message text
    pub message: String,
}

/// Log level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Convert log level to string
    pub fn to_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

impl std::str::FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "trace" => Ok(LogLevel::Trace),
            "debug" => Ok(LogLevel::Debug),
            "info" => Ok(LogLevel::Info),
            "warn" | "warning" => Ok(LogLevel::Warn),
            "error" => Ok(LogLevel::Error),
            _ => Err(format!("Unknown log level: {}", s)),
        }
    }
}

/// Convert from log crate's Level to our LogLevel
fn level_to_log_level(level: Level) -> LogLevel {
    match level {
        Level::Error => LogLevel::Error,
        Level::Warn => LogLevel::Warn,
        Level::Info => LogLevel::Info,
        Level::Debug => LogLevel::Debug,
        Level::Trace => LogLevel::Trace,
    }
}

/// Convert from our LogLevel to log crate's Level
fn log_level_to_level(level: LogLevel) -> Level {
    match level {
        LogLevel::Error => Level::Error,
        LogLevel::Warn => Level::Warn,
        LogLevel::Info => Level::Info,
        LogLevel::Debug => Level::Debug,
        LogLevel::Trace => Level::Trace,
    }
}

/// Convert from our LogLevel to log crate's LevelFilter
fn log_level_to_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::Error,
        LogLevel::Warn => LevelFilter::Warn,
        LogLevel::Info => LevelFilter::Info,
        LogLevel::Debug => LevelFilter::Debug,
        LogLevel::Trace => LevelFilter::Trace,
    }
}

/// Global logger state
#[derive(Debug)]
struct Logger {
    /// Debug log (last N messages)
    entries: VecDeque<LogEntry>,
    /// Maximum number of entries in log
    max_entries: usize,
    /// Minimum log level to record
    min_level: LogLevel,
    /// Log file path
    file_path: PathBuf,
}

impl Logger {
    /// Create new logger instance
    fn new(file_path: PathBuf, max_entries: usize, min_level: LogLevel) -> Self {
        // Create parent directory if it doesn't exist
        if let Some(parent) = file_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Clear log file on startup
        if let Ok(mut file) = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&file_path)
        {
            let _ = writeln!(file, "=== Termide Log Start ===");
        }

        Self {
            entries: VecDeque::new(),
            max_entries,
            min_level,
            file_path,
        }
    }

    /// Add entry to log
    fn add_entry(&mut self, level: LogLevel, message: String) {
        // Filter by minimum level
        if level < self.min_level {
            return;
        }

        let timestamp = Local::now().format("%H:%M:%S").to_string();
        let entry = LogEntry {
            timestamp: timestamp.clone(),
            level,
            message: message.clone(),
        };

        // Add to queue
        self.entries.push_back(entry);

        // Limit queue size
        while self.entries.len() > self.max_entries {
            self.entries.pop_front();
        }

        // Write to file (create if deleted)
        if let Ok(mut file) = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.file_path)
        {
            let _ = writeln!(file, "[{}] {}: {}", timestamp, level.to_str(), message);
        }
    }

    /// Get the total number of entries
    fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Get entries starting from a specific index (returns clones of only new entries)
    fn get_entries_from(&self, start_idx: usize) -> Vec<LogEntry> {
        self.entries.iter().skip(start_idx).cloned().collect()
    }
}

/// Global logger instance that persists for the application lifetime.
static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();

/// Static logger for registration with log crate
static TERMIDE_LOGGER: TermideLogger = TermideLogger;

/// Implementation of log::Log trait for integration with standard logging
struct TermideLogger;

impl Log for TermideLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        if let Some(logger) = LOGGER.get() {
            if let Ok(l) = logger.lock() {
                return log_level_to_level(l.min_level) >= metadata.level();
            }
        }
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            if let Some(logger) = LOGGER.get() {
                if let Ok(mut l) = logger.lock() {
                    let level = level_to_log_level(record.level());
                    l.add_entry(level, record.args().to_string());
                }
            }
        }
    }

    fn flush(&self) {}
}

/// Get or initialize the global logger instance
fn get_logger() -> &'static Mutex<Logger> {
    // If logger is not initialized, panic with a helpful message
    LOGGER
        .get()
        .expect("Logger not initialized. Call logger::init() first.")
}

/// Initialize the global logger
///
/// Must be called once at application startup before any logging functions.
/// Subsequent calls will be ignored.
///
/// # Arguments
///
/// * `file_path` - Path to the log file
/// * `max_entries` - Maximum number of log entries to keep in memory
/// * `min_level` - Minimum log level to record (Debug, Info, Warn, Error)
pub fn init(file_path: PathBuf, max_entries: usize, min_level: LogLevel) {
    LOGGER.get_or_init(|| Mutex::new(Logger::new(file_path, max_entries, min_level)));
    // Register with log crate (ignore error if already set)
    let _ = log::set_logger(&TERMIDE_LOGGER);
    log::set_max_level(log_level_to_filter(min_level));
}

/// Get the total number of log entries
///
/// Returns the count without cloning any entries.
pub fn entry_count() -> usize {
    if let Ok(logger) = get_logger().lock() {
        logger.entry_count()
    } else {
        0
    }
}

/// Get log entries starting from a specific index
///
/// Returns only entries from `start_idx` onwards, avoiding cloning earlier entries.
/// Used for incremental synchronization with panels.
pub fn get_entries_from(start_idx: usize) -> Vec<LogEntry> {
    if let Ok(logger) = get_logger().lock() {
        logger.get_entries_from(start_idx)
    } else {
        Vec::new()
    }
}
