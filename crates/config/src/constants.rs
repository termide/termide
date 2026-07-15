//! Application constants.

/// Number of spinner animation frames.
pub const SPINNER_FRAMES_COUNT: usize = 10;

/// Spinner animation characters.
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Get current spinner frame based on system time (updates every 100ms).
///
/// Use this for animated spinners in panel titles and other UI elements.
pub fn spinner_frame() -> &'static str {
    let frame_idx = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        / 100) as usize
        % SPINNER_FRAMES.len();
    SPINNER_FRAMES[frame_idx]
}

/// File sizes.
pub const KILOBYTE: u64 = 1024;
pub const MEGABYTE: u64 = KILOBYTE * 1024;
pub const GIGABYTE: u64 = MEGABYTE * 1024;

// ===== UI and layout constants =====

/// Minimum terminal width for multi-panel mode.
pub const MIN_WIDTH_MULTI_PANEL: u16 = 100;

/// Minimum main panel width in multi-panel mode.
pub const MIN_MAIN_PANEL_WIDTH: u16 = 80;

// ===== Modal constants =====

/// Maximum modal width as percentage of screen width (default).
pub const MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT: f32 = 0.75;

/// Maximum modal width as percentage of screen width (wide modals).
pub const MODAL_MAX_WIDTH_PERCENTAGE_WIDE: f32 = 0.9;

/// Minimum modal width (wide modals).
pub const MODAL_MIN_WIDTH_WIDE: u16 = 30;

/// Modal total width addition with double border and padding.
pub const MODAL_PADDING_WITH_DOUBLE_BORDER: u16 = 8; // 4 (borders) + 4 (padding)

/// Spacing between modal buttons.
pub const MODAL_BUTTON_SPACING: u16 = 4;

/// Minimum width for values in info modals.
pub const MODAL_MIN_VALUE_WIDTH: usize = 20;

// ===== Performance and security constants =====

/// Maximum number of log entries.
pub const MAX_LOG_ENTRIES: usize = 1000;

/// Event update interval in milliseconds (42ms = ~24 FPS).
pub const EVENT_HANDLER_INTERVAL_MS: u64 = 42;

/// Idle tick rate in milliseconds (100ms = 10 FPS) — reduces CPU when no user input
/// while keeping key response latency under 100ms after idle.
pub const IDLE_TICK_MS: u64 = 100;

/// Duration of inactivity before switching to idle tick rate (in milliseconds).
pub const IDLE_THRESHOLD_MS: u64 = 500;

/// Double-click detection interval in milliseconds.
pub const DOUBLE_CLICK_INTERVAL_MS: u128 = 500;
