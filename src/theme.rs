use ratatui::style::Color;
use serde::Deserialize;
use std::sync::OnceLock;

/// Color representation in TOML
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum TomlColor {
    Named(String),
    Rgb { rgb: [u8; 3] },
}

impl TomlColor {
    fn to_color(&self) -> Color {
        match self {
            TomlColor::Named(name) => match name.as_str() {
                "Black" => Color::Black,
                "Red" => Color::Red,
                "Green" => Color::Green,
                "Yellow" => Color::Yellow,
                "Blue" => Color::Blue,
                "Magenta" => Color::Magenta,
                "Cyan" => Color::Cyan,
                "Gray" => Color::Gray,
                "DarkGray" => Color::DarkGray,
                "LightRed" => Color::LightRed,
                "LightGreen" => Color::LightGreen,
                "LightYellow" => Color::LightYellow,
                "LightBlue" => Color::LightBlue,
                "LightMagenta" => Color::LightMagenta,
                "LightCyan" => Color::LightCyan,
                "White" => Color::White,
                _ => Color::White,
            },
            TomlColor::Rgb { rgb } => Color::Rgb(rgb[0], rgb[1], rgb[2]),
        }
    }
}

/// TOML theme colors structure
#[derive(Debug, Clone, Deserialize)]
struct TomlColors {
    background: TomlColor,
    text_primary: TomlColor,
    text_secondary: TomlColor,
    accent_primary: TomlColor,
    accent_secondary: TomlColor,
    highlight: TomlColor,

    selection_bg: TomlColor,
    selection_fg: TomlColor,
    selected_item: TomlColor,
    cursor_line_bg: TomlColor,

    status_bar_bg: TomlColor,

    success_bg: TomlColor,
    success_fg: TomlColor,
    error_bg: TomlColor,
    error_fg: TomlColor,
    git_modified: TomlColor,
    git_added: TomlColor,
    git_deleted: TomlColor,
}

/// TOML theme structure
#[derive(Debug, Clone, Deserialize)]
struct TomlTheme {
    name: String,
    colors: TomlColors,
}

/// Application theme
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub name: &'static str,

    // Base colors
    pub background: Color,              // Application and modal background
    pub text_primary: Color,            // Primary text
    pub text_secondary: Color,          // Secondary/muted text
    pub accent_primary: Color,          // Primary accent color (active elements)
    pub accent_secondary: Color,        // Secondary accent color (inactive elements)
    pub highlight: Color,               // Highlight/selection color

    // Interactive elements
    pub selection_bg: Color,            // Selected item background (menu, FM cursor)
    pub selection_fg: Color,            // Selected item text
    pub selected_item: Color,           // Selected file marker
    pub cursor_line_bg: Color,          // Cursor line highlight in editor

    // Status bar
    pub status_bar_bg: Color,           // Status bar background

    // Semantic colors (status messages and Git)
    pub success_bg: Color,
    pub success_fg: Color,
    pub error_bg: Color,
    pub error_fg: Color,
    pub git_modified: Color,
    pub git_added: Color,
    pub git_deleted: Color,
}

/// Load theme from embedded TOML content
fn load_theme_from_toml(content: &str, name: &'static str) -> Theme {
    let toml_theme: TomlTheme = toml::from_str(content)
        .unwrap_or_else(|e| panic!("Failed to parse theme {}: {}", name, e));

    let c = &toml_theme.colors;
    Theme {
        name,
        background: c.background.to_color(),
        text_primary: c.text_primary.to_color(),
        text_secondary: c.text_secondary.to_color(),
        accent_primary: c.accent_primary.to_color(),
        accent_secondary: c.accent_secondary.to_color(),
        highlight: c.highlight.to_color(),
        selection_bg: c.selection_bg.to_color(),
        selection_fg: c.selection_fg.to_color(),
        selected_item: c.selected_item.to_color(),
        cursor_line_bg: c.cursor_line_bg.to_color(),
        status_bar_bg: c.status_bar_bg.to_color(),
        success_bg: c.success_bg.to_color(),
        success_fg: c.success_fg.to_color(),
        error_bg: c.error_bg.to_color(),
        error_fg: c.error_fg.to_color(),
        git_modified: c.git_modified.to_color(),
        git_added: c.git_added.to_color(),
        git_deleted: c.git_deleted.to_color(),
    }
}

// Embed theme files at compile time
const THEME_DEFAULT_TOML: &str = include_str!("../themes/default.toml");
const THEME_MIDNIGHT_TOML: &str = include_str!("../themes/midnight.toml");
const THEME_LIGHT_TOML: &str = include_str!("../themes/light.toml");
const THEME_BLUESKY_TOML: &str = include_str!("../themes/bluesky.toml");

// Static theme instances
static THEME_DEFAULT: OnceLock<Theme> = OnceLock::new();
static THEME_MIDNIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_LIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_BLUESKY: OnceLock<Theme> = OnceLock::new();

fn get_default_theme() -> &'static Theme {
    THEME_DEFAULT.get_or_init(|| load_theme_from_toml(THEME_DEFAULT_TOML, "default"))
}

fn get_midnight_theme() -> &'static Theme {
    THEME_MIDNIGHT.get_or_init(|| load_theme_from_toml(THEME_MIDNIGHT_TOML, "midnight"))
}

fn get_light_theme() -> &'static Theme {
    THEME_LIGHT.get_or_init(|| load_theme_from_toml(THEME_LIGHT_TOML, "light"))
}

fn get_bluesky_theme() -> &'static Theme {
    THEME_BLUESKY.get_or_init(|| load_theme_from_toml(THEME_BLUESKY_TOML, "bluesky"))
}

impl Theme {
    /// Get theme by name
    pub fn get_by_name(name: &str) -> &'static Theme {
        match name {
            "default" => get_default_theme(),
            "midnight" => get_midnight_theme(),
            "light" => get_light_theme(),
            "bluesky" => get_bluesky_theme(),
            _ => get_default_theme(),
        }
    }

    /// Get list of all available themes
    pub fn all_themes() -> Vec<&'static Theme> {
        vec![
            get_default_theme(),
            get_midnight_theme(),
            get_light_theme(),
            get_bluesky_theme(),
        ]
    }

    /// Get list of all theme names
    pub fn all_theme_names() -> &'static [&'static str] {
        &["default", "midnight", "light", "bluesky"]
    }
}

impl Default for Theme {
    fn default() -> Self {
        *get_default_theme()
    }
}
