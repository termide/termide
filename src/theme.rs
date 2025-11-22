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
    // Base
    bg: TomlColor,
    fg: TomlColor,

    // Accented
    accented_bg: TomlColor,
    accented_fg: TomlColor,

    // Selection
    selected_bg: TomlColor,
    selected_fg: TomlColor,

    // Disabled
    disabled: TomlColor,

    // Semantic
    success: TomlColor,
    warning: TomlColor,
    error: TomlColor,
}

/// TOML theme structure
#[derive(Debug, Clone, Deserialize)]
struct TomlTheme {
    #[allow(dead_code)]
    name: String,
    colors: TomlColors,
}

/// Application theme
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    #[allow(dead_code)]
    pub name: &'static str,

    // Base (2 colors)
    pub bg: Color, // Panel backgrounds
    pub fg: Color, // Main text

    // Accented (2 colors)
    pub accented_bg: Color, // Menu, status bar, cursor line background
    pub accented_fg: Color, // Active borders, first letter in menu, selected file marker

    // Selection (2 colors)
    pub selected_bg: Color, // Selected item background (FM cursor, menu selection)
    pub selected_fg: Color, // Selected item text

    // Disabled (1 color)
    pub disabled: Color, // Inactive elements, secondary text, separators

    // Semantic (3 colors)
    pub success: Color, // Success, git added, resource indicators <50%
    pub warning: Color, // Warning, git modified, resource indicators 50-75%, search highlight
    pub error: Color,   // Error, git deleted, resource indicators >75%
}

/// Load theme from embedded TOML content
fn load_theme_from_toml(content: &str, name: &'static str) -> Theme {
    let toml_theme: TomlTheme =
        toml::from_str(content).unwrap_or_else(|e| panic!("Failed to parse theme {}: {}", name, e));

    let c = &toml_theme.colors;
    Theme {
        name,
        bg: c.bg.to_color(),
        fg: c.fg.to_color(),
        accented_bg: c.accented_bg.to_color(),
        accented_fg: c.accented_fg.to_color(),
        selected_bg: c.selected_bg.to_color(),
        selected_fg: c.selected_fg.to_color(),
        disabled: c.disabled.to_color(),
        success: c.success.to_color(),
        warning: c.warning.to_color(),
        error: c.error.to_color(),
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
    #[allow(dead_code)]
    pub fn all_themes() -> Vec<&'static Theme> {
        vec![
            get_default_theme(),
            get_midnight_theme(),
            get_light_theme(),
            get_bluesky_theme(),
        ]
    }

    /// Get list of all theme names
    #[allow(dead_code)]
    pub fn all_theme_names() -> &'static [&'static str] {
        &["default", "midnight", "light", "bluesky"]
    }
}

impl Default for Theme {
    fn default() -> Self {
        *get_default_theme()
    }
}
