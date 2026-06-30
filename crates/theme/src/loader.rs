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
            // Match case-insensitively so `Reset`/`reset`, `White`/`white`,
            // etc. all resolve regardless of how the user typed them.
            TomlColor::Named(name) => match name.to_ascii_lowercase().as_str() {
                "black" => Color::Black,
                "red" => Color::Red,
                "green" => Color::Green,
                "yellow" => Color::Yellow,
                "blue" => Color::Blue,
                "magenta" => Color::Magenta,
                "cyan" => Color::Cyan,
                "gray" => Color::Gray,
                "darkgray" => Color::DarkGray,
                "lightred" => Color::LightRed,
                "lightgreen" => Color::LightGreen,
                "lightyellow" => Color::LightYellow,
                "lightblue" => Color::LightBlue,
                "lightmagenta" => Color::LightMagenta,
                "lightcyan" => Color::LightCyan,
                "white" => Color::White,
                "reset" => Color::Reset,
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
    /// Optional override for light/dark classification (auto-detected from bg if not specified)
    #[serde(default)]
    is_light: Option<bool>,
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
        is_light: toml_theme.is_light,
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
        is_light: toml_theme.is_light,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(s: &str) -> Color {
        TomlColor::Named(s.to_string()).to_color()
    }

    #[test]
    fn named_colors_are_case_insensitive() {
        assert_eq!(named("Reset"), Color::Reset);
        assert_eq!(named("reset"), Color::Reset);
        assert_eq!(named("RESET"), Color::Reset);
        assert_eq!(named("black"), Color::Black);
        assert_eq!(named("DARKGRAY"), Color::DarkGray);
        assert_eq!(named("LightBlue"), Color::LightBlue);
    }

    #[test]
    fn unknown_named_color_falls_back_to_white() {
        assert_eq!(named("chartreuse"), Color::White);
    }
}
