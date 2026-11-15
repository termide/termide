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
    /// Where the log goes when no file is set in the config.
    default_path: PathBuf,
}

impl Logger {
    /// Create new logger instance: the log goes to `file_override` when
    /// set, else to `default_path`, and that file starts empty.
    fn new(
        default_path: PathBuf,
        file_override: Option<PathBuf>,
        max_entries: usize,
        min_level: LogLevel,
    ) -> Self {
        let file_path = file_override.unwrap_or_else(|| default_path.clone());
        let mut logger = Self {
            entries: VecDeque::new(),
            max_entries,
            min_level,
            file_path,
            default_path,
        };
        logger.start_file(true, "=== Termide Log Start ===");
        logger
    }

    /// Open the log file with `header` as its first new line, emptied first
    /// when `truncate`. The journal panel still shows the entries kept in
    /// memory; a file that cannot be written is said there.
    fn start_file(&mut self, truncate: bool, header: &str) {
        let opened = self
            .file_path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| {
                OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(truncate)
                    .append(!truncate)
                    .open(&self.file_path)
            })
            .and_then(|mut file| writeln!(file, "{header}"));
        if let Err(error) = opened {
            let message = format!(
                "cannot write the log file {}: {error}",
                self.file_path.display()
            );
            self.add_entry(LogLevel::Error, message);
        }
    }

    /// Send the log to `file_override`, or back to the default file when
    /// `None`. The new file is appended to, not emptied: it may be a log
    /// the user keeps. Nothing happens when the file stays the same.
    fn set_file(&mut self, file_override: Option<PathBuf>) {
        let path = file_override.unwrap_or_else(|| self.default_path.clone());
        if path == self.file_path {
            return;
        }
        let previous = std::mem::replace(&mut self.file_path, path);
        self.start_file(
            false,
            &format!("=== Termide Log continued from {} ===", previous.display()),
        );
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
/// * `default_path` - Path to the log file when the config sets none
/// * `file_override` - Path to the log file the config sets, if any
/// * `max_entries` - Maximum number of log entries to keep in memory
/// * `min_level` - Minimum log level to record (Debug, Info, Warn, Error)
pub fn init(
    default_path: PathBuf,
    file_override: Option<PathBuf>,
    max_entries: usize,
    min_level: LogLevel,
) {
    LOGGER.get_or_init(|| {
        Mutex::new(Logger::new(
            default_path,
            file_override,
            max_entries,
            min_level,
        ))
    });
    // Register with log crate (ignore error if already set)
    let _ = log::set_logger(&TERMIDE_LOGGER);
    log::set_max_level(log_level_to_filter(min_level));
}

/// Change the minimum level recorded, from now on.
pub fn set_min_level(min_level: LogLevel) {
    if let Some(logger) = LOGGER.get() {
        if let Ok(mut logger) = logger.lock() {
            logger.min_level = min_level;
        }
    }
    log::set_max_level(log_level_to_filter(min_level));
}

/// Write the log to `file_override` from now on, or back to the default
/// file given to [`init`] when `None`. What was written so far stays where
/// it is.
pub fn set_file_path(file_override: Option<PathBuf>) {
    if let Some(logger) = LOGGER.get() {
        if let Ok(mut logger) = logger.lock() {
            logger.set_file(file_override);
        }
    }
}

/// Follow the log file when the directory `from` holding it moves to `to`:
/// it is opened by path for every line, which would otherwise bring the old
/// directory back.
pub fn relocate_file(from: &std::path::Path, to: &std::path::Path) {
    if let Some(logger) = LOGGER.get() {
        if let Ok(mut logger) = logger.lock() {
            if let Ok(rel) = logger.file_path.strip_prefix(from) {
                logger.file_path = to.join(rel);
            }
            if let Ok(rel) = logger.default_path.strip_prefix(from) {
                logger.default_path = to.join(rel);
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_file_that_cannot_be_written_is_reported_in_the_journal() {
        let dir = std::env::temp_dir().join(format!("termide-logger-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // A plain file where the log's directory should be.
        let blocker = dir.join("not-a-dir");
        fs::write(&blocker, "").unwrap();
        let logger = Logger::new(blocker.join("termide.log"), None, 10, LogLevel::Info);
        let entries = logger.get_entries_from(0);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, LogLevel::Error);
        assert!(entries[0].message.contains("cannot write the log file"));
    }

    #[test]
    fn the_log_file_can_be_changed_and_set_back_to_the_default() {
        let dir = tempfile_dir("switch");
        let default = dir.join("default.log");
        let custom = dir.join("sub").join("custom.log");
        let mut logger = Logger::new(default.clone(), None, 10, LogLevel::Info);
        logger.add_entry(LogLevel::Info, "first".into());

        logger.set_file(Some(custom.clone()));
        logger.add_entry(LogLevel::Info, "second".into());
        logger.set_file(None);
        logger.add_entry(LogLevel::Info, "third".into());

        let default_text = fs::read_to_string(&default).unwrap();
        let custom_text = fs::read_to_string(&custom).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert!(default_text.contains("first") && default_text.contains("third"));
        assert!(!default_text.contains("second"));
        assert!(default_text.contains("continued from"));
        assert!(custom_text.contains("second") && !custom_text.contains("first"));
        assert!(custom_text.contains("continued from"));
        assert_eq!(logger.entry_count(), 3, "no file error was reported");
    }

    #[test]
    fn a_configured_log_file_is_used_from_the_start() {
        let dir = tempfile_dir("override");
        let default = dir.join("default.log");
        let custom = dir.join("custom.log");
        let mut logger = Logger::new(default.clone(), Some(custom.clone()), 10, LogLevel::Info);
        logger.add_entry(LogLevel::Info, "hello".into());
        let custom_text = fs::read_to_string(&custom).unwrap();
        let default_exists = default.exists();
        fs::remove_dir_all(&dir).unwrap();
        assert!(custom_text.contains("hello"));
        assert!(!default_exists);
    }

    #[test]
    fn a_raised_minimum_level_drops_lower_entries() {
        let dir = tempfile_dir("level");
        let mut logger = Logger::new(dir.join("l.log"), None, 10, LogLevel::Info);
        logger.add_entry(LogLevel::Info, "kept".into());
        logger.min_level = LogLevel::Warn;
        logger.add_entry(LogLevel::Info, "dropped".into());
        logger.add_entry(LogLevel::Warn, "warned".into());
        fs::remove_dir_all(&dir).unwrap();
        let messages: Vec<_> = logger
            .get_entries_from(0)
            .into_iter()
            .map(|e| e.message)
            .collect();
        assert_eq!(messages, ["kept", "warned"]);
    }

    /// A fresh directory for one test.
    fn tempfile_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("termide-logger-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
