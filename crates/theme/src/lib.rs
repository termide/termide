//! Theme system for termide.
//!
//! Provides color theme management with support for custom TOML themes.

mod colors;
mod loader;

pub use colors::Theme;
pub use loader::load_theme;

use ratatui::style::Color;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

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

// Themes directory path (set by app on startup)
static THEMES_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Set the themes directory path (call this at app startup).
pub fn set_themes_dir(path: PathBuf) {
    let _ = THEMES_DIR.set(path);
}

/// Get themes directory path.
fn get_themes_dir() -> Option<&'static PathBuf> {
    THEMES_DIR.get()
}

/// Hardcoded fallback theme in case of parse errors.
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

/// Load theme from embedded TOML content.
fn load_theme_from_toml(content: &str, name: &'static str) -> Theme {
    match loader::load_theme_from_str(content, name) {
        Ok(theme) => theme,
        Err(e) => {
            eprintln!(
                "Failed to parse built-in theme '{}': {}. Using fallback theme.",
                name, e
            );
            get_hardcoded_fallback_theme(name)
        }
    }
}

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

/// Try to load user theme from config directory.
fn try_load_user_theme(name: &str) -> Option<&'static Theme> {
    let cache = USER_THEMES.get_or_init(|| Mutex::new(HashMap::new()));

    // Check if theme is already cached
    // Use ok() to gracefully handle poisoned mutex (return None instead of panicking)
    {
        let cache_lock = cache.lock().ok()?;
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

    let theme = load_theme(&theme_path).ok()?;

    // Leak the theme to get 'static reference
    let static_theme: &'static Theme = Box::leak(Box::new(theme));

    // Cache it (ignore if mutex is poisoned - theme already loaded, just won't be cached)
    if let Ok(mut cache_lock) = cache.lock() {
        cache_lock.insert(name.to_string(), static_theme);
    }

    Some(static_theme)
}

impl Theme {
    /// Get theme by name.
    ///
    /// First tries to load from user's config directory.
    /// If not found, falls back to built-in themes.
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

    /// Get list of all available themes.
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

    /// Get list of all theme names.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_loading() {
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
        if let Some(themes_dir) = get_themes_dir() {
            let darkgray_path = themes_dir.join("darkgray.toml");
            if darkgray_path.exists() {
                let darkgray = Theme::get_by_name("darkgray");
                assert_eq!(darkgray.name, "darkgray");
            }
        }
    }
}
