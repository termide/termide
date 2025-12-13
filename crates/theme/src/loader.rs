//! Theme loading from TOML files.

use anyhow::Result;
use ratatui::style::Color;
use serde::Deserialize;
use std::path::Path;

use crate::Theme;

/// Color representation in TOML.
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

/// TOML theme colors structure.
#[derive(Debug, Clone, Deserialize)]
struct TomlColors {
    bg: TomlColor,
    fg: TomlColor,
    accented_bg: TomlColor,
    accented_fg: TomlColor,
    selected_bg: TomlColor,
    selected_fg: TomlColor,
    disabled: TomlColor,
    success: TomlColor,
    warning: TomlColor,
    error: TomlColor,
}

/// TOML theme structure.
#[derive(Debug, Clone, Deserialize)]
struct TomlTheme {
    name: String,
    colors: TomlColors,
}

/// Load theme from TOML file.
///
/// Returns the parsed theme with a leaked static name string.
pub fn load_theme(path: &Path) -> Result<Theme> {
    let content = std::fs::read_to_string(path)?;
    let toml_theme: TomlTheme = toml::from_str(&content)?;

    // Leak the name string to get 'static lifetime
    let name: &'static str = Box::leak(toml_theme.name.into_boxed_str());

    Ok(Theme {
        name,
        bg: toml_theme.colors.bg.to_color(),
        fg: toml_theme.colors.fg.to_color(),
        accented_bg: toml_theme.colors.accented_bg.to_color(),
        accented_fg: toml_theme.colors.accented_fg.to_color(),
        selected_bg: toml_theme.colors.selected_bg.to_color(),
        selected_fg: toml_theme.colors.selected_fg.to_color(),
        disabled: toml_theme.colors.disabled.to_color(),
        success: toml_theme.colors.success.to_color(),
        warning: toml_theme.colors.warning.to_color(),
        error: toml_theme.colors.error.to_color(),
    })
}

/// Load theme from TOML string with a static name.
pub fn load_theme_from_str(content: &str, name: &'static str) -> Result<Theme> {
    let toml_theme: TomlTheme = toml::from_str(content)?;

    Ok(Theme {
        name,
        bg: toml_theme.colors.bg.to_color(),
        fg: toml_theme.colors.fg.to_color(),
        accented_bg: toml_theme.colors.accented_bg.to_color(),
        accented_fg: toml_theme.colors.accented_fg.to_color(),
        selected_bg: toml_theme.colors.selected_bg.to_color(),
        selected_fg: toml_theme.colors.selected_fg.to_color(),
        disabled: toml_theme.colors.disabled.to_color(),
        success: toml_theme.colors.success.to_color(),
        warning: toml_theme.colors.warning.to_color(),
        error: toml_theme.colors.error.to_color(),
    })
}
