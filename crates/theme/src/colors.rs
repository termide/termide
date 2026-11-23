//! Theme color definitions.

use ratatui::style::Color;

/// Standard ANSI-16 color palette (used for Linux TTY/framebuffer console).
///
/// Indices 0-7 are the normal colors, 8-15 are bright variants.
const ANSI_16_PALETTE: [(u8, u8, u8); 16] = [
    (0, 0, 0),       // 0: Black
    (128, 0, 0),     // 1: Red
    (0, 128, 0),     // 2: Green
    (128, 128, 0),   // 3: Yellow
    (0, 0, 128),     // 4: Blue
    (128, 0, 128),   // 5: Magenta
    (0, 128, 128),   // 6: Cyan
    (192, 192, 192), // 7: White (light gray)
    (128, 128, 128), // 8: Bright Black (gray)
    (255, 0, 0),     // 9: Bright Red
    (0, 255, 0),     // 10: Bright Green
    (255, 255, 0),   // 11: Bright Yellow
    (0, 0, 255),     // 12: Bright Blue
    (255, 0, 255),   // 13: Bright Magenta
    (0, 255, 255),   // 14: Bright Cyan
    (255, 255, 255), // 15: Bright White
];

/// Convert RGB color to nearest ANSI-16 indexed color.
///
/// Uses Euclidean distance in RGB space to find the closest match.
pub fn rgb_to_ansi16(r: u8, g: u8, b: u8) -> Color {
    let mut best_idx = 0;
    let mut best_dist = u32::MAX;

    for (i, (pr, pg, pb)) in ANSI_16_PALETTE.iter().enumerate() {
        let dr = (r as i32 - *pr as i32).pow(2) as u32;
        let dg = (g as i32 - *pg as i32).pow(2) as u32;
        let db = (b as i32 - *pb as i32).pow(2) as u32;
        let dist = dr + dg + db;

        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
    }

    Color::Indexed(best_idx as u8)
}

/// Convert 256-color palette index to approximate RGB values.
///
/// Handles:
/// - 0-15: Standard ANSI colors
/// - 16-231: 6x6x6 color cube
/// - 232-255: Grayscale ramp
fn index_to_rgb(idx: u8) -> (u8, u8, u8) {
    if idx < 16 {
        ANSI_16_PALETTE[idx as usize]
    } else if idx < 232 {
        // 6x6x6 color cube (indices 16-231)
        let idx = idx - 16;
        let r = idx / 36;
        let g = (idx % 36) / 6;
        let b = idx % 6;
        // Convert 0-5 to 0-255 (0, 95, 135, 175, 215, 255)
        let to_val = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
        (to_val(r), to_val(g), to_val(b))
    } else {
        // Grayscale ramp (indices 232-255)
        // 24 shades from dark (8) to light (238)
        let gray = 8 + (idx - 232) * 10;
        (gray, gray, gray)
    }
}

/// Adapt a single color to ANSI-16 palette.
fn adapt_color_to_ansi16(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => rgb_to_ansi16(r, g, b),
        Color::Indexed(i) if i >= 16 => {
            // 256-color palette index -> convert to RGB first, then to ANSI-16
            let (r, g, b) = index_to_rgb(i);
            rgb_to_ansi16(r, g, b)
        }
        // Already ANSI-16 or named color - keep as is
        other => other,
    }
}

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

    // === Theme classification ===
    /// Optional override for light/dark classification (auto-detected from bg if None)
    pub is_light: Option<bool>,
}

impl Theme {
    /// Determine if this is a light theme.
    /// Uses explicit is_light field if set, otherwise auto-detects from bg luminance.
    pub fn is_light_theme(&self) -> bool {
        if let Some(is_light) = self.is_light {
            return is_light;
        }
        // Auto-detect using ITU-R BT.601 relative luminance formula
        match self.bg {
            Color::Rgb(r, g, b) => {
                let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
                luminance > 128.0
            }
            // Named light colors
            Color::White
            | Color::Gray
            | Color::LightRed
            | Color::LightGreen
            | Color::LightYellow
            | Color::LightBlue
            | Color::LightMagenta
            | Color::LightCyan => true,
            // Black, DarkGray, and saturated colors are considered dark
            _ => false,
        }
    }

    /// Adapt theme colors for ANSI-16 limited palette (Linux TTY/framebuffer).
    ///
    /// Converts all RGB and 256-color values to the nearest ANSI-16 color,
    /// making the theme usable on terminals with limited color support.
    pub fn adapt_for_ansi16(&self) -> Theme {
        Theme {
            name: self.name,
            bg: adapt_color_to_ansi16(self.bg),
            fg: adapt_color_to_ansi16(self.fg),
            accented_bg: adapt_color_to_ansi16(self.accented_bg),
            accented_fg: adapt_color_to_ansi16(self.accented_fg),
            selected_bg: adapt_color_to_ansi16(self.selected_bg),
            selected_fg: adapt_color_to_ansi16(self.selected_fg),
            disabled: adapt_color_to_ansi16(self.disabled),
            success: adapt_color_to_ansi16(self.success),
            warning: adapt_color_to_ansi16(self.warning),
            error: adapt_color_to_ansi16(self.error),
            is_light: self.is_light,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        *Self::get_by_name("windows-xp")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adapt_color_to_ansi16_rgb() {
        // RGB color should be converted
        let adapted = adapt_color_to_ansi16(Color::Rgb(0, 0, 0));
        assert_eq!(adapted, Color::Indexed(0));

        let adapted = adapt_color_to_ansi16(Color::Rgb(255, 255, 255));
        assert_eq!(adapted, Color::Indexed(15));
    }

    #[test]
    fn test_adapt_color_to_ansi16_indexed() {
        // Indexed color >= 16 should be converted
        // Index 232 is grayscale dark (8, 8, 8) - should map to black
        let adapted = adapt_color_to_ansi16(Color::Indexed(232));
        assert_eq!(adapted, Color::Indexed(0));

        // Indexed color < 16 should stay the same
        let adapted = adapt_color_to_ansi16(Color::Indexed(5));
        assert_eq!(adapted, Color::Indexed(5));
    }

    #[test]
    fn test_adapt_color_to_ansi16_named() {
        // Named colors should stay the same
        let adapted = adapt_color_to_ansi16(Color::Red);
        assert_eq!(adapted, Color::Red);

        let adapted = adapt_color_to_ansi16(Color::White);
        assert_eq!(adapted, Color::White);
    }

    #[test]
    fn test_index_to_rgb_standard() {
        // Standard ANSI colors
        assert_eq!(index_to_rgb(0), (0, 0, 0)); // Black
        assert_eq!(index_to_rgb(1), (128, 0, 0)); // Red
        assert_eq!(index_to_rgb(15), (255, 255, 255)); // Bright white
    }

    #[test]
    fn test_index_to_rgb_color_cube() {
        // Color cube index 16 should be (0, 0, 0)
        assert_eq!(index_to_rgb(16), (0, 0, 0));
        // Index 231 is the last color cube entry (5,5,5) = (255, 255, 255)
        assert_eq!(index_to_rgb(231), (255, 255, 255));
    }

    #[test]
    fn test_index_to_rgb_grayscale() {
        // Grayscale ramp: 232 = gray 8, 255 = gray 238
        assert_eq!(index_to_rgb(232), (8, 8, 8));
        assert_eq!(index_to_rgb(255), (238, 238, 238));
    }

    #[test]
    fn test_theme_adapt_for_ansi16() {
        let theme = Theme {
            name: "test",
            bg: Color::Rgb(30, 30, 30), // Dark gray -> should become black
            fg: Color::Rgb(220, 220, 220), // Light gray -> should become white
            accented_bg: Color::Rgb(50, 50, 100), // Dark blue-ish
            accented_fg: Color::Rgb(0, 200, 200), // Cyan-ish
            selected_bg: Color::Rgb(0, 0, 200), // Blue
            selected_fg: Color::Rgb(255, 255, 255), // White
            disabled: Color::Rgb(100, 100, 100), // Gray
            success: Color::Rgb(0, 200, 0), // Green
            warning: Color::Rgb(200, 200, 0), // Yellow
            error: Color::Rgb(200, 0, 0), // Red
            is_light: Some(false),
        };

        let adapted = theme.adapt_for_ansi16();

        // All colors should now be Indexed
        assert!(matches!(adapted.bg, Color::Indexed(_)));
        assert!(matches!(adapted.fg, Color::Indexed(_)));
        assert!(matches!(adapted.accented_bg, Color::Indexed(_)));
        assert!(matches!(adapted.accented_fg, Color::Indexed(_)));
        assert!(matches!(adapted.selected_bg, Color::Indexed(_)));
        assert!(matches!(adapted.selected_fg, Color::Indexed(_)));
        assert!(matches!(adapted.disabled, Color::Indexed(_)));
        assert!(matches!(adapted.success, Color::Indexed(_)));
        assert!(matches!(adapted.warning, Color::Indexed(_)));
        assert!(matches!(adapted.error, Color::Indexed(_)));

        // Name and is_light should be preserved
        assert_eq!(adapted.name, "test");
        assert_eq!(adapted.is_light, Some(false));
    }
}
