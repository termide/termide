use ratatui::style::Color;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

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
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Hardcoded fallback theme in case of parse errors
fn get_hardcoded_fallback_theme(name: &'static str) -> Theme {
    Theme {
        name,
        bg: Color::Black,
        fg: Color::White,
        accented_bg: Color::DarkGray,
        accented_fg: Color::Cyan,
        selected_bg: Color::Blue,
        selected_fg: Color::White,
        disabled: Color::Gray,
        success: Color::Green,
        warning: Color::Yellow,
        error: Color::Red,
    }
}

/// Load theme from embedded TOML content
fn load_theme_from_toml(content: &str, name: &'static str) -> Theme {
    match toml::from_str::<TomlTheme>(content) {
        Ok(toml_theme) => {
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
        Err(e) => {
            eprintln!(
                "Failed to parse built-in theme '{}': {}. Using fallback theme.",
                name, e
            );
            get_hardcoded_fallback_theme(name)
        }
    }
}

/// Get path to user themes directory
fn get_themes_dir() -> Option<PathBuf> {
    crate::xdg_dirs::get_config_dir()
        .ok()
        .map(|config_dir| config_dir.join("themes"))
}

/// Load theme from file
fn load_theme_from_file(path: PathBuf, name: &str) -> Option<Theme> {
    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<TomlTheme>(&content) {
            Ok(toml_theme) => {
                let c = &toml_theme.colors;
                // Use Box::leak to create a &'static str from the owned String
                let static_name = Box::leak(name.to_string().into_boxed_str());
                Some(Theme {
                    name: static_name,
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
                })
            }
            Err(e) => {
                eprintln!("Failed to parse theme file {:?}: {}", path, e);
                None
            }
        },
        Err(e) => {
            eprintln!("Failed to read theme file {:?}: {}", path, e);
            None
        }
    }
}

// Embed theme files at compile time
const THEME_ATOM_ONE_LIGHT_TOML: &str = include_str!("../themes/atom-one-light.toml");
const THEME_AYU_LIGHT_TOML: &str = include_str!("../themes/ayu-light.toml");
const THEME_DEFAULT_TOML: &str = include_str!("../themes/default.toml");
const THEME_DRACULA_TOML: &str = include_str!("../themes/dracula.toml");
const THEME_GITHUB_LIGHT_TOML: &str = include_str!("../themes/github-light.toml");
const THEME_MATERIAL_LIGHTER_TOML: &str = include_str!("../themes/material-lighter.toml");
const THEME_MIDNIGHT_TOML: &str = include_str!("../themes/midnight.toml");
const THEME_MONOKAI_TOML: &str = include_str!("../themes/monokai.toml");
const THEME_NORD_TOML: &str = include_str!("../themes/nord.toml");
const THEME_ONEDARK_TOML: &str = include_str!("../themes/onedark.toml");
const THEME_SOLARIZED_DARK_TOML: &str = include_str!("../themes/solarized-dark.toml");
const THEME_SOLARIZED_LIGHT_TOML: &str = include_str!("../themes/solarized-light.toml");

// Static theme instances
static THEME_ATOM_ONE_LIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_AYU_LIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_DEFAULT: OnceLock<Theme> = OnceLock::new();
static THEME_DRACULA: OnceLock<Theme> = OnceLock::new();
static THEME_GITHUB_LIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_MATERIAL_LIGHTER: OnceLock<Theme> = OnceLock::new();
static THEME_MIDNIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_MONOKAI: OnceLock<Theme> = OnceLock::new();
static THEME_NORD: OnceLock<Theme> = OnceLock::new();
static THEME_ONEDARK: OnceLock<Theme> = OnceLock::new();
static THEME_SOLARIZED_DARK: OnceLock<Theme> = OnceLock::new();
static THEME_SOLARIZED_LIGHT: OnceLock<Theme> = OnceLock::new();

// Cache for user-loaded themes
static USER_THEMES: OnceLock<Mutex<HashMap<String, &'static Theme>>> = OnceLock::new();

fn get_atom_one_light_theme() -> &'static Theme {
    THEME_ATOM_ONE_LIGHT
        .get_or_init(|| load_theme_from_toml(THEME_ATOM_ONE_LIGHT_TOML, "atom-one-light"))
}

fn get_ayu_light_theme() -> &'static Theme {
    THEME_AYU_LIGHT.get_or_init(|| load_theme_from_toml(THEME_AYU_LIGHT_TOML, "ayu-light"))
}

fn get_default_theme() -> &'static Theme {
    THEME_DEFAULT.get_or_init(|| load_theme_from_toml(THEME_DEFAULT_TOML, "default"))
}

fn get_dracula_theme() -> &'static Theme {
    THEME_DRACULA.get_or_init(|| load_theme_from_toml(THEME_DRACULA_TOML, "dracula"))
}

fn get_github_light_theme() -> &'static Theme {
    THEME_GITHUB_LIGHT.get_or_init(|| load_theme_from_toml(THEME_GITHUB_LIGHT_TOML, "github-light"))
}

fn get_material_lighter_theme() -> &'static Theme {
    THEME_MATERIAL_LIGHTER
        .get_or_init(|| load_theme_from_toml(THEME_MATERIAL_LIGHTER_TOML, "material-lighter"))
}

fn get_midnight_theme() -> &'static Theme {
    THEME_MIDNIGHT.get_or_init(|| load_theme_from_toml(THEME_MIDNIGHT_TOML, "midnight"))
}

fn get_monokai_theme() -> &'static Theme {
    THEME_MONOKAI.get_or_init(|| load_theme_from_toml(THEME_MONOKAI_TOML, "monokai"))
}

fn get_nord_theme() -> &'static Theme {
    THEME_NORD.get_or_init(|| load_theme_from_toml(THEME_NORD_TOML, "nord"))
}

fn get_onedark_theme() -> &'static Theme {
    THEME_ONEDARK.get_or_init(|| load_theme_from_toml(THEME_ONEDARK_TOML, "onedark"))
}

fn get_solarized_dark_theme() -> &'static Theme {
    THEME_SOLARIZED_DARK
        .get_or_init(|| load_theme_from_toml(THEME_SOLARIZED_DARK_TOML, "solarized-dark"))
}

fn get_solarized_light_theme() -> &'static Theme {
    THEME_SOLARIZED_LIGHT
        .get_or_init(|| load_theme_from_toml(THEME_SOLARIZED_LIGHT_TOML, "solarized-light"))
}

/// Try to load user theme from config directory
fn try_load_user_theme(name: &str) -> Option<&'static Theme> {
    // Get or initialize the cache
    let cache = USER_THEMES.get_or_init(|| Mutex::new(HashMap::new()));

    // Check if theme is already cached
    {
        let cache_lock = cache.lock().unwrap();
        if let Some(theme) = cache_lock.get(name) {
            return Some(*theme);
        }
    }

    // Try to load from file
    let themes_dir = get_themes_dir()?;
    let theme_path = themes_dir.join(format!("{}.toml", name));

    if !theme_path.exists() {
        return None;
    }

    let theme = load_theme_from_file(theme_path, name)?;

    // Leak the theme to get 'static reference
    let static_theme: &'static Theme = Box::leak(Box::new(theme));

    // Cache it
    let mut cache_lock = cache.lock().unwrap();
    cache_lock.insert(name.to_string(), static_theme);

    Some(static_theme)
}

impl Theme {
    /// Get theme by name
    /// First tries to load from user's config directory (~/.config/termide/themes/)
    /// If not found, falls back to built-in themes
    pub fn get_by_name(name: &str) -> &'static Theme {
        // Try to load user theme first
        if let Some(theme) = try_load_user_theme(name) {
            return theme;
        }

        // Fall back to built-in themes
        match name {
            "atom-one-light" => get_atom_one_light_theme(),
            "ayu-light" => get_ayu_light_theme(),
            "default" => get_default_theme(),
            "dracula" => get_dracula_theme(),
            "github-light" => get_github_light_theme(),
            "material-lighter" => get_material_lighter_theme(),
            "midnight" => get_midnight_theme(),
            "monokai" => get_monokai_theme(),
            "nord" => get_nord_theme(),
            "onedark" => get_onedark_theme(),
            "solarized-dark" => get_solarized_dark_theme(),
            "solarized-light" => get_solarized_light_theme(),
            _ => get_default_theme(),
        }
    }

    /// Get list of all available themes
    #[allow(dead_code)]
    pub fn all_themes() -> Vec<&'static Theme> {
        vec![
            get_atom_one_light_theme(),
            get_ayu_light_theme(),
            get_default_theme(),
            get_dracula_theme(),
            get_github_light_theme(),
            get_material_lighter_theme(),
            get_midnight_theme(),
            get_monokai_theme(),
            get_nord_theme(),
            get_onedark_theme(),
            get_solarized_dark_theme(),
            get_solarized_light_theme(),
        ]
    }

    /// Get list of all theme names
    #[allow(dead_code)]
    pub fn all_theme_names() -> &'static [&'static str] {
        &[
            "atom-one-light",
            "ayu-light",
            "default",
            "dracula",
            "github-light",
            "material-lighter",
            "midnight",
            "monokai",
            "nord",
            "onedark",
            "solarized-dark",
            "solarized-light",
        ]
    }
}

impl Default for Theme {
    fn default() -> Self {
        *get_default_theme()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_loading() {
        // Test built-in themes
        let default = Theme::get_by_name("default");
        assert_eq!(default.name, "default");

        let midnight = Theme::get_by_name("midnight");
        assert_eq!(midnight.name, "midnight");

        // Test fallback for unknown theme
        let unknown = Theme::get_by_name("nonexistent");
        assert_eq!(unknown.name, "default");
    }

    #[test]
    fn test_user_theme_loading() {
        // This test will only pass if darkgray.toml exists in ~/.config/termide/themes/
        // You can skip this test if the file doesn't exist
        if let Some(themes_dir) = get_themes_dir() {
            let darkgray_path = themes_dir.join("darkgray.toml");
            if darkgray_path.exists() {
                let darkgray = Theme::get_by_name("darkgray");
                assert_eq!(darkgray.name, "darkgray");
            }
        }
    }
}
