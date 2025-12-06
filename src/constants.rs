/// Application constants
///
/// Number of spinner animation frames
pub const SPINNER_FRAMES_COUNT: usize = 8;

/// Spinner animation characters
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

/// File sizes
pub const KILOBYTE: u64 = 1024;
pub const MEGABYTE: u64 = KILOBYTE * 1024;
pub const GIGABYTE: u64 = MEGABYTE * 1024;

/// Maximum file size to open in editor (100 MB)
pub const MAX_EDITOR_FILE_SIZE: u64 = 100 * MEGABYTE;

/// Default file size threshold in MB for enabling smart features (5 MB)
/// Used as default value for Config.large_file_threshold_mb
pub const DEFAULT_LARGE_FILE_THRESHOLD_MB: u64 = 5;

// ===== UI and layout constants =====

/// Minimum terminal width for single panel mode
pub const MIN_WIDTH_SINGLE_PANEL: u16 = 80;

/// Minimum terminal width for multi-panel mode
pub const MIN_WIDTH_MULTI_PANEL: u16 = 100;

/// Minimum main panel width in multi-panel mode
pub const MIN_MAIN_PANEL_WIDTH: u16 = 80;

/// Default main panel width in multi-panel mode
pub const DEFAULT_MAIN_PANEL_WIDTH: u16 = 80;

// ===== Modal constants =====

/// Maximum modal width as percentage of screen width (default)
pub const MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT: f32 = 0.75;

/// Maximum modal width as percentage of screen width (wide modals)
pub const MODAL_MAX_WIDTH_PERCENTAGE_WIDE: f32 = 0.9;

/// Minimum modal width (default)
pub const MODAL_MIN_WIDTH_DEFAULT: u16 = 20;

/// Minimum modal width (wide modals)
pub const MODAL_MIN_WIDTH_WIDE: u16 = 30;

/// Modal total width addition with border and padding
pub const MODAL_PADDING_WITH_BORDER: u16 = 6; // 2 (border) + 4 (padding)

/// Modal total width addition with double border and padding
pub const MODAL_PADDING_WITH_DOUBLE_BORDER: u16 = 8; // 4 (borders) + 4 (padding)

/// Spacing between modal buttons
pub const MODAL_BUTTON_SPACING: u16 = 4;

/// Minimum width for values in info modals
pub const MODAL_MIN_VALUE_WIDTH: usize = 20;

// ===== Performance and security constants =====

/// Maximum recursion depth when copying directories
pub const MAX_DIRECTORY_COPY_DEPTH: usize = 100;

/// Maximum number of log entries
pub const MAX_LOG_ENTRIES: usize = 1000;

/// Event update interval in milliseconds (42ms = ~24 FPS)
pub const EVENT_HANDLER_INTERVAL_MS: u64 = 42;

/// Double-click detection interval in milliseconds
pub const DOUBLE_CLICK_INTERVAL_MS: u128 = 500;
