use chrono::Local;
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

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
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Parse log level from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "debug" => Some(LogLevel::Debug),
            "info" => Some(LogLevel::Info),
            "warn" | "warning" => Some(LogLevel::Warn),
            "error" => Some(LogLevel::Error),
            _ => None,
        }
    }

    /// Convert log level to string
    pub fn to_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
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
            let _ = writeln!(file, "=== TermIDE Log Start ===");
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

    /// Get all log entries
    fn get_entries(&self) -> Vec<LogEntry> {
        self.entries.iter().cloned().collect()
    }

    /// Set minimum log level
    #[allow(dead_code)]
    fn set_min_level(&mut self, level: LogLevel) {
        self.min_level = level;
    }
}

/// Global logger instance that persists for the application lifetime.
static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();

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
}

/// Set minimum log level dynamically
///
/// Updates the minimum log level filter.
/// Logs below this level will be ignored.
#[allow(dead_code)]
pub fn set_min_level(level: LogLevel) {
    if let Ok(mut logger) = get_logger().lock() {
        logger.set_min_level(level);
    }
}

/// Log a debug message
///
/// Debug messages are the lowest priority and are typically used for
/// detailed troubleshooting information.
///
/// # Example
///
/// ```ignore
/// logger::debug("Entering function foo()".to_string());
/// logger::debug(format!("Variable x = {}", x));
/// ```
pub fn debug(message: impl Into<String>) {
    if let Ok(mut logger) = get_logger().lock() {
        logger.add_entry(LogLevel::Debug, message.into());
    }
}

/// Log an informational message
///
/// Info messages are used for general informational messages about
/// application progress and state.
///
/// # Example
///
/// ```ignore
/// logger::info("Application started".to_string());
/// logger::info(format!("Loaded {} files", count));
/// ```
pub fn info(message: impl Into<String>) {
    if let Ok(mut logger) = get_logger().lock() {
        logger.add_entry(LogLevel::Info, message.into());
    }
}

/// Log a warning message
///
/// Warning messages indicate potentially problematic situations that
/// don't prevent the application from functioning.
///
/// # Example
///
/// ```ignore
/// logger::warn("Configuration file not found, using defaults".to_string());
/// logger::warn(format!("Deprecated API used: {}", api_name));
/// ```
pub fn warn(message: impl Into<String>) {
    if let Ok(mut logger) = get_logger().lock() {
        logger.add_entry(LogLevel::Warn, message.into());
    }
}

/// Log an error message
///
/// Error messages indicate serious problems that caused an operation
/// to fail.
///
/// # Example
///
/// ```ignore
/// logger::error("Failed to open file".to_string());
/// logger::error(format!("Error: {}", error));
/// ```
pub fn error(message: impl Into<String>) {
    if let Ok(mut logger) = get_logger().lock() {
        logger.add_entry(LogLevel::Error, message.into());
    }
}

/// Get all log entries
///
/// Returns a vector of all log entries currently stored in memory.
/// Used by the debug panel to display logs.
///
/// # Returns
///
/// Vector of `LogEntry` clones in chronological order
pub fn get_entries() -> Vec<LogEntry> {
    if let Ok(logger) = get_logger().lock() {
        logger.get_entries()
    } else {
        Vec::new()
    }
}
