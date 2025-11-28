/// Application constants
///
/// Default file manager width in multi-panel mode
pub const DEFAULT_FM_WIDTH: u16 = 30;

/// Number of spinner animation frames
pub const SPINNER_FRAMES_COUNT: usize = 8;

/// Spinner animation characters
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// File sizes
pub const KILOBYTE: u64 = 1024;
pub const MEGABYTE: u64 = KILOBYTE * 1024;
pub const GIGABYTE: u64 = MEGABYTE * 1024;

/// Minimum file manager panel width to display size and time columns
pub const MIN_WIDTH_FOR_EXTENDED_VIEW: usize = 70;

/// Maximum file size to open in editor (100 MB)
pub const MAX_EDITOR_FILE_SIZE: u64 = 100 * MEGABYTE;

// ===== UI and layout constants =====

/// Minimum terminal width for single panel mode
pub const MIN_WIDTH_SINGLE_PANEL: u16 = 80;

/// Minimum terminal width for multi-panel mode
pub const MIN_WIDTH_MULTI_PANEL: u16 = 100;

/// Minimum main panel width in multi-panel mode
pub const MIN_MAIN_PANEL_WIDTH: u16 = 80;

/// Default main panel width in multi-panel mode
pub const DEFAULT_MAIN_PANEL_WIDTH: u16 = 80;

/// Settings modal window width
#[allow(dead_code)]
pub const SETTINGS_MODAL_WIDTH: u16 = 50;

// ===== Performance and security constants =====

/// Maximum recursion depth when copying directories
pub const MAX_DIRECTORY_COPY_DEPTH: usize = 100;

/// Maximum number of log entries
pub const MAX_LOG_ENTRIES: usize = 1000;

/// Event update interval in milliseconds (42ms = ~24 FPS)
pub const EVENT_HANDLER_INTERVAL_MS: u64 = 42;

/// Double-click detection interval in milliseconds
pub const DOUBLE_CLICK_INTERVAL_MS: u128 = 500;
