//! Core types and traits for termide panels.
//!
//! This crate provides the foundational abstractions for building panels
//! in termide without coupling them to the application state.

/// Full version string for display: the crate version plus the git commit it
/// was built from, e.g. `"0.23.1 (50b81b1a)"`. Falls back to just the crate
/// version when git is unavailable at build time. Stamped by `build.rs`.
pub const VERSION: &str = env!("TERMIDE_VERSION");

pub mod command;
pub mod event;
pub mod hotkey_table;
pub mod key_chord;
pub mod panel;
pub mod scrollbar;
pub mod terminal_caps;
pub mod terminal_modes;
pub mod util;
pub mod wide_cells;

pub use command::{CommandResult, PanelCommand};
pub use event::{
    ChecklistItem, ConfirmAction, ConflictResolution, Event, EventHandler, GitOperationType,
    InputAction, PanelEvent, ReferenceLocation, SelectAction, SplitDirection, VimPanelDirection,
};
pub use hotkey_table::HotkeyTable;
pub use key_chord::KeyChord;
pub use panel::{
    HeightMode, Panel, PanelConfig, PanelState, RenderContext, Searchable, SegmentKind,
    StatusSegment, ThemeColors, WidthPreference,
};
pub use scrollbar::{ScrollAxis, ScrollBarGeometry, ScrollBars};
pub use terminal_caps::{
    get_terminal_caps, init_icon_mode, init_terminal_caps, refresh_terminal_caps, use_emoji_icons,
    ColorDepth, TerminalCaps,
};
pub use terminal_modes::{
    adopt_variation_selector_width, enter_terminal_modes, leave_terminal_modes,
    probe_variation_selector_width, variation_selector_width_override, VS16_WIDTH_ENV,
};
pub use wide_cells::mark_variation_selector_tails;
// Re-export keyboard primitives so panels can stay on `termide_core` as
// their dependency surface.
pub use termide_keyboard::{KeyNormalizer, KeyboardCaps};

// Re-export theme and config for convenience
pub use termide_config::Config;
pub use termide_config::LinkOpen;
pub use termide_theme::Theme;
