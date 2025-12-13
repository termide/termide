//! Theme color definitions.

use ratatui::style::Color;

/// Application theme with semantic color assignments.
///
/// The theme uses a minimal 10-color palette:
/// - 2 base colors (bg, fg)
/// - 2 accented colors (accented_bg, accented_fg)
/// - 2 selection colors (selected_bg, selected_fg)
/// - 1 disabled color
/// - 3 semantic colors (success, warning, error)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// Theme name for display
    pub name: &'static str,

    // === Base (2 colors) ===
    /// Panel backgrounds
    pub bg: Color,
    /// Main text
    pub fg: Color,

    // === Accented (2 colors) ===
    /// Menu, status bar, cursor line background
    pub accented_bg: Color,
    /// Active borders, first letter in menu, selected file marker
    pub accented_fg: Color,

    // === Selection (2 colors) ===
    /// Selected item background (FM cursor, menu selection)
    pub selected_bg: Color,
    /// Selected item text
    pub selected_fg: Color,

    // === Disabled (1 color) ===
    /// Inactive elements, secondary text, separators
    pub disabled: Color,

    // === Semantic (3 colors) ===
    /// Success, git added, resource indicators <50%
    pub success: Color,
    /// Warning, git modified, resource indicators 50-75%, search highlight
    pub warning: Color,
    /// Error, git deleted, resource indicators >75%
    pub error: Color,
}

impl Default for Theme {
    fn default() -> Self {
        *Self::get_by_name("default")
    }
}
