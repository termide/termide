//! Theme system for termide.
//!
//! Provides color theme management with support for custom TOML themes.

mod colors;
mod loader;

pub use colors::{rgb_to_ansi16, Theme};
pub use loader::load_theme;

use ratatui::style::Color;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Global flag indicating if themes should be adapted for ANSI-16 palette.
/// Set this early in application startup before loading any themes.
static ANSI16_MODE: AtomicBool = AtomicBool::new(false);

/// Cache for adapted themes (separate from user themes cache).
static ADAPTED_THEMES: OnceLock<Mutex<HashMap<String, &'static Theme>>> = OnceLock::new();

/// Enable ANSI-16 color adaptation mode.
///
/// When enabled, all themes loaded via `Theme::get_by_name()` will be
/// automatically adapted to use only ANSI-16 colors (for Linux TTY/framebuffer).
///
/// Call this once at application startup, before loading any themes.
pub fn set_ansi16_mode(enabled: bool) {
    ANSI16_MODE.store(enabled, Ordering::Release);
}

/// Check if ANSI-16 mode is enabled.
pub fn is_ansi16_mode() -> bool {
    ANSI16_MODE.load(Ordering::Acquire)
}

/// Get or create an adapted theme for ANSI-16 mode.
fn get_or_create_adapted_theme(name: &str, base_theme: &'static Theme) -> &'static Theme {
    let cache = ADAPTED_THEMES.get_or_init(|| Mutex::new(HashMap::new()));

    // Check if adapted theme is already cached
    if let Ok(cache_lock) = cache.lock() {
        if let Some(theme) = cache_lock.get(name) {
            return theme;
        }
    }

    // Create adapted theme
    let adapted = base_theme.adapt_for_ansi16();

    // Leak to get 'static reference
    let static_theme: &'static Theme = Box::leak(Box::new(adapted));

    // Cache it (ignore if mutex is poisoned)
    if let Ok(mut cache_lock) = cache.lock() {
        cache_lock.insert(name.to_string(), static_theme);
    }

    static_theme
}

// Embed theme files at compile time
const THEME_ATOM_ONE_LIGHT_TOML: &str = include_str!("../themes/atom-one-light.toml");
const THEME_AYU_LIGHT_TOML: &str = include_str!("../themes/ayu-light.toml");
const THEME_DOS_NAVIGATOR_TOML: &str = include_str!("../themes/dos-navigator.toml");
const THEME_DRACULA_TOML: &str = include_str!("../themes/dracula.toml");
const THEME_FAR_MANAGER_TOML: &str = include_str!("../themes/far-manager.toml");
const THEME_GITHUB_LIGHT_TOML: &str = include_str!("../themes/github-light.toml");
const THEME_MACOS_DARK_TOML: &str = include_str!("../themes/macos-dark.toml");
const THEME_MACOS_LIGHT_TOML: &str = include_str!("../themes/macos-light.toml");
const THEME_MANUSCRIPT_TOML: &str = include_str!("../themes/manuscript.toml");
const THEME_MATERIAL_LIGHTER_TOML: &str = include_str!("../themes/material-lighter.toml");
const THEME_MATRIX_TOML: &str = include_str!("../themes/matrix.toml");
const THEME_MIDNIGHT_TOML: &str = include_str!("../themes/midnight.toml");
const THEME_MONOKAI_TOML: &str = include_str!("../themes/monokai.toml");
const THEME_NORD_TOML: &str = include_str!("../themes/nord.toml");
const THEME_NORTON_COMMANDER_TOML: &str = include_str!("../themes/norton-commander.toml");
const THEME_ONEDARK_TOML: &str = include_str!("../themes/onedark.toml");
const THEME_PIP_BOY_TOML: &str = include_str!("../themes/pip-boy.toml");
const THEME_SOLARIZED_DARK_TOML: &str = include_str!("../themes/solarized-dark.toml");
const THEME_TERMINAL_TOML: &str = include_str!("../themes/terminal.toml");
const THEME_TERMINATOR_TOML: &str = include_str!("../themes/terminator.toml");
const THEME_SOLARIZED_LIGHT_TOML: &str = include_str!("../themes/solarized-light.toml");
const THEME_VOLKOV_COMMANDER_TOML: &str = include_str!("../themes/volkov-commander.toml");
const THEME_WINDOWS_95_TOML: &str = include_str!("../themes/windows-95.toml");
const THEME_WINDOWS_98_TOML: &str = include_str!("../themes/windows-98.toml");
const THEME_WINDOWS_XP_TOML: &str = include_str!("../themes/windows-xp.toml");
const THEME_AYU_DARK_TOML: &str = include_str!("../themes/ayu-dark.toml");
const THEME_CATPPUCCIN_MACCHIATO_TOML: &str = include_str!("../themes/catppuccin-macchiato.toml");
const THEME_EVERFOREST_TOML: &str = include_str!("../themes/everforest.toml");
const THEME_GITHUB_DARK_TOML: &str = include_str!("../themes/github-dark.toml");
const THEME_GRUVBOX_TOML: &str = include_str!("../themes/gruvbox.toml");
const THEME_KANAGAWA_TOML: &str = include_str!("../themes/kanagawa.toml");
const THEME_MATERIAL_OCEAN_TOML: &str = include_str!("../themes/material-ocean.toml");
const THEME_ROSEPINE_TOML: &str = include_str!("../themes/rosepine.toml");
const THEME_TOKYONIGHT_TOML: &str = include_str!("../themes/tokyonight.toml");
const THEME_BILLIARD_TOML: &str = include_str!("../themes/billiard.toml");
const THEME_GREEN_BACKS_TOML: &str = include_str!("../themes/green-backs.toml");
const THEME_PINKY_PIE_TOML: &str = include_str!("../themes/pinky-pie.toml");
const THEME_BLUE_SKY_TOML: &str = include_str!("../themes/blue-sky.toml");
const THEME_BLACK_EARTH_TOML: &str = include_str!("../themes/black-earth.toml");
const THEME_MOONLIT_DEW_TOML: &str = include_str!("../themes/moonlit-dew.toml");
const THEME_FIREFLIES_TOML: &str = include_str!("../themes/fireflies.toml");
const THEME_DRY_PATH_TOML: &str = include_str!("../themes/dry-path.toml");
const THEME_MORNING_MIST_TOML: &str = include_str!("../themes/morning-mist.toml");
const THEME_SUMMER_MEADOW_TOML: &str = include_str!("../themes/summer-meadow.toml");

// Static theme instances
static THEME_ATOM_ONE_LIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_AYU_LIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_DOS_NAVIGATOR: OnceLock<Theme> = OnceLock::new();
static THEME_DRACULA: OnceLock<Theme> = OnceLock::new();
static THEME_FAR_MANAGER: OnceLock<Theme> = OnceLock::new();
static THEME_GITHUB_LIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_MACOS_DARK: OnceLock<Theme> = OnceLock::new();
static THEME_MACOS_LIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_MANUSCRIPT: OnceLock<Theme> = OnceLock::new();
static THEME_MATERIAL_LIGHTER: OnceLock<Theme> = OnceLock::new();
static THEME_MATRIX: OnceLock<Theme> = OnceLock::new();
static THEME_MIDNIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_MONOKAI: OnceLock<Theme> = OnceLock::new();
static THEME_NORD: OnceLock<Theme> = OnceLock::new();
static THEME_NORTON_COMMANDER: OnceLock<Theme> = OnceLock::new();
static THEME_ONEDARK: OnceLock<Theme> = OnceLock::new();
static THEME_PIP_BOY: OnceLock<Theme> = OnceLock::new();
static THEME_SOLARIZED_DARK: OnceLock<Theme> = OnceLock::new();
static THEME_TERMINAL: OnceLock<Theme> = OnceLock::new();
static THEME_TERMINATOR: OnceLock<Theme> = OnceLock::new();
static THEME_SOLARIZED_LIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_VOLKOV_COMMANDER: OnceLock<Theme> = OnceLock::new();
static THEME_WINDOWS_95: OnceLock<Theme> = OnceLock::new();
static THEME_WINDOWS_98: OnceLock<Theme> = OnceLock::new();
static THEME_WINDOWS_XP: OnceLock<Theme> = OnceLock::new();
static THEME_AYU_DARK: OnceLock<Theme> = OnceLock::new();
static THEME_CATPPUCCIN_MACCHIATO: OnceLock<Theme> = OnceLock::new();
static THEME_EVERFOREST: OnceLock<Theme> = OnceLock::new();
static THEME_GITHUB_DARK: OnceLock<Theme> = OnceLock::new();
static THEME_GRUVBOX: OnceLock<Theme> = OnceLock::new();
static THEME_KANAGAWA: OnceLock<Theme> = OnceLock::new();
static THEME_MATERIAL_OCEAN: OnceLock<Theme> = OnceLock::new();
static THEME_ROSEPINE: OnceLock<Theme> = OnceLock::new();
static THEME_TOKYONIGHT: OnceLock<Theme> = OnceLock::new();
static THEME_BILLIARD: OnceLock<Theme> = OnceLock::new();
static THEME_GREEN_BACKS: OnceLock<Theme> = OnceLock::new();
static THEME_PINKY_PIE: OnceLock<Theme> = OnceLock::new();
static THEME_BLUE_SKY: OnceLock<Theme> = OnceLock::new();
static THEME_BLACK_EARTH: OnceLock<Theme> = OnceLock::new();
static THEME_MOONLIT_DEW: OnceLock<Theme> = OnceLock::new();
static THEME_FIREFLIES: OnceLock<Theme> = OnceLock::new();
static THEME_DRY_PATH: OnceLock<Theme> = OnceLock::new();
static THEME_MORNING_MIST: OnceLock<Theme> = OnceLock::new();
static THEME_SUMMER_MEADOW: OnceLock<Theme> = OnceLock::new();

/// Entry in the built-in themes registry.
/// Single source of truth for all built-in theme configurations.
struct BuiltinThemeEntry {
    name: &'static str,
    content: &'static str,
    storage: &'static OnceLock<Theme>,
}

/// Registry of all built-in themes.
/// Adding a new theme requires only adding an entry here
/// (plus the TOML file and static OnceLock above).
static BUILTIN_THEMES: &[BuiltinThemeEntry] = &[
    BuiltinThemeEntry {
        name: "atom-one-light",
        content: THEME_ATOM_ONE_LIGHT_TOML,
        storage: &THEME_ATOM_ONE_LIGHT,
    },
    BuiltinThemeEntry {
        name: "ayu-dark",
        content: THEME_AYU_DARK_TOML,
        storage: &THEME_AYU_DARK,
    },
    BuiltinThemeEntry {
        name: "ayu-light",
        content: THEME_AYU_LIGHT_TOML,
        storage: &THEME_AYU_LIGHT,
    },
    BuiltinThemeEntry {
        name: "billiard",
        content: THEME_BILLIARD_TOML,
        storage: &THEME_BILLIARD,
    },
    BuiltinThemeEntry {
        name: "black-earth",
        content: THEME_BLACK_EARTH_TOML,
        storage: &THEME_BLACK_EARTH,
    },
    BuiltinThemeEntry {
        name: "blue-sky",
        content: THEME_BLUE_SKY_TOML,
        storage: &THEME_BLUE_SKY,
    },
    BuiltinThemeEntry {
        name: "catppuccin-macchiato",
        content: THEME_CATPPUCCIN_MACCHIATO_TOML,
        storage: &THEME_CATPPUCCIN_MACCHIATO,
    },
    BuiltinThemeEntry {
        name: "dos-navigator",
        content: THEME_DOS_NAVIGATOR_TOML,
        storage: &THEME_DOS_NAVIGATOR,
    },
    BuiltinThemeEntry {
        name: "dry-path",
        content: THEME_DRY_PATH_TOML,
        storage: &THEME_DRY_PATH,
    },
    BuiltinThemeEntry {
        name: "everforest",
        content: THEME_EVERFOREST_TOML,
        storage: &THEME_EVERFOREST,
    },
    BuiltinThemeEntry {
        name: "dracula",
        content: THEME_DRACULA_TOML,
        storage: &THEME_DRACULA,
    },
    BuiltinThemeEntry {
        name: "far-manager",
        content: THEME_FAR_MANAGER_TOML,
        storage: &THEME_FAR_MANAGER,
    },
    BuiltinThemeEntry {
        name: "fireflies",
        content: THEME_FIREFLIES_TOML,
        storage: &THEME_FIREFLIES,
    },
    BuiltinThemeEntry {
        name: "github-dark",
        content: THEME_GITHUB_DARK_TOML,
        storage: &THEME_GITHUB_DARK,
    },
    BuiltinThemeEntry {
        name: "github-light",
        content: THEME_GITHUB_LIGHT_TOML,
        storage: &THEME_GITHUB_LIGHT,
    },
    BuiltinThemeEntry {
        name: "green-backs",
        content: THEME_GREEN_BACKS_TOML,
        storage: &THEME_GREEN_BACKS,
    },
    BuiltinThemeEntry {
        name: "gruvbox",
        content: THEME_GRUVBOX_TOML,
        storage: &THEME_GRUVBOX,
    },
    BuiltinThemeEntry {
        name: "kanagawa",
        content: THEME_KANAGAWA_TOML,
        storage: &THEME_KANAGAWA,
    },
    BuiltinThemeEntry {
        name: "macos-dark",
        content: THEME_MACOS_DARK_TOML,
        storage: &THEME_MACOS_DARK,
    },
    BuiltinThemeEntry {
        name: "macos-light",
        content: THEME_MACOS_LIGHT_TOML,
        storage: &THEME_MACOS_LIGHT,
    },
    BuiltinThemeEntry {
        name: "manuscript",
        content: THEME_MANUSCRIPT_TOML,
        storage: &THEME_MANUSCRIPT,
    },
    BuiltinThemeEntry {
        name: "material-lighter",
        content: THEME_MATERIAL_LIGHTER_TOML,
        storage: &THEME_MATERIAL_LIGHTER,
    },
    BuiltinThemeEntry {
        name: "material-ocean",
        content: THEME_MATERIAL_OCEAN_TOML,
        storage: &THEME_MATERIAL_OCEAN,
    },
    BuiltinThemeEntry {
        name: "matrix",
        content: THEME_MATRIX_TOML,
        storage: &THEME_MATRIX,
    },
    BuiltinThemeEntry {
        name: "midnight",
        content: THEME_MIDNIGHT_TOML,
        storage: &THEME_MIDNIGHT,
    },
    BuiltinThemeEntry {
        name: "monokai",
        content: THEME_MONOKAI_TOML,
        storage: &THEME_MONOKAI,
    },
    BuiltinThemeEntry {
        name: "moonlit-dew",
        content: THEME_MOONLIT_DEW_TOML,
        storage: &THEME_MOONLIT_DEW,
    },
    BuiltinThemeEntry {
        name: "morning-mist",
        content: THEME_MORNING_MIST_TOML,
        storage: &THEME_MORNING_MIST,
    },
    BuiltinThemeEntry {
        name: "nord",
        content: THEME_NORD_TOML,
        storage: &THEME_NORD,
    },
    BuiltinThemeEntry {
        name: "norton-commander",
        content: THEME_NORTON_COMMANDER_TOML,
        storage: &THEME_NORTON_COMMANDER,
    },
    BuiltinThemeEntry {
        name: "onedark",
        content: THEME_ONEDARK_TOML,
        storage: &THEME_ONEDARK,
    },
    BuiltinThemeEntry {
        name: "rosepine",
        content: THEME_ROSEPINE_TOML,
        storage: &THEME_ROSEPINE,
    },
    BuiltinThemeEntry {
        name: "pip-boy",
        content: THEME_PIP_BOY_TOML,
        storage: &THEME_PIP_BOY,
    },
    BuiltinThemeEntry {
        name: "pinky-pie",
        content: THEME_PINKY_PIE_TOML,
        storage: &THEME_PINKY_PIE,
    },
    BuiltinThemeEntry {
        name: "solarized-dark",
        content: THEME_SOLARIZED_DARK_TOML,
        storage: &THEME_SOLARIZED_DARK,
    },
    BuiltinThemeEntry {
        name: "solarized-light",
        content: THEME_SOLARIZED_LIGHT_TOML,
        storage: &THEME_SOLARIZED_LIGHT,
    },
    BuiltinThemeEntry {
        name: "summer-meadow",
        content: THEME_SUMMER_MEADOW_TOML,
        storage: &THEME_SUMMER_MEADOW,
    },
    BuiltinThemeEntry {
        name: "terminal",
        content: THEME_TERMINAL_TOML,
        storage: &THEME_TERMINAL,
    },
    BuiltinThemeEntry {
        name: "tokyonight",
        content: THEME_TOKYONIGHT_TOML,
        storage: &THEME_TOKYONIGHT,
    },
    BuiltinThemeEntry {
        name: "terminator",
        content: THEME_TERMINATOR_TOML,
        storage: &THEME_TERMINATOR,
    },
    BuiltinThemeEntry {
        name: "volkov-commander",
        content: THEME_VOLKOV_COMMANDER_TOML,
        storage: &THEME_VOLKOV_COMMANDER,
    },
    BuiltinThemeEntry {
        name: "windows-95",
        content: THEME_WINDOWS_95_TOML,
        storage: &THEME_WINDOWS_95,
    },
    BuiltinThemeEntry {
        name: "windows-98",
        content: THEME_WINDOWS_98_TOML,
        storage: &THEME_WINDOWS_98,
    },
    BuiltinThemeEntry {
        name: "windows-xp",
        content: THEME_WINDOWS_XP_TOML,
        storage: &THEME_WINDOWS_XP,
    },
];

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
        is_light: None,
    }
}

/// Load theme from embedded TOML content.
fn load_theme_from_toml(content: &str, name: &'static str) -> Theme {
    match loader::load_theme_from_str(content, name) {
        Ok(theme) => theme,
        Err(e) => {
            log::warn!(
                "Failed to parse built-in theme '{}': {}. Using fallback theme.",
                name,
                e
            );
            get_hardcoded_fallback_theme(name)
        }
    }
}

/// Get a built-in theme by name.
fn get_builtin_theme(name: &str) -> Option<&'static Theme> {
    BUILTIN_THEMES
        .iter()
        .find(|entry| entry.name == name)
        .map(|entry| {
            entry
                .storage
                .get_or_init(|| load_theme_from_toml(entry.content, entry.name))
        })
}

/// Get the default theme (windows-xp).
fn get_default_theme() -> &'static Theme {
    get_builtin_theme("windows-xp").expect("windows-xp theme must exist")
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
    ///
    /// If ANSI-16 mode is enabled (via `set_ansi16_mode(true)`),
    /// the theme will be automatically adapted for limited color terminals.
    pub fn get_by_name(name: &str) -> &'static Theme {
        // Try to load user theme first
        let base_theme = if let Some(theme) = try_load_user_theme(name) {
            theme
        } else {
            // Fall back to built-in themes
            get_builtin_theme(name).unwrap_or_else(get_default_theme)
        };

        // If ANSI-16 mode is enabled, return adapted version
        if is_ansi16_mode() {
            return get_or_create_adapted_theme(name, base_theme);
        }

        base_theme
    }

    /// Get list of all theme names (built-in + user themes).
    /// Returns owned Vec since user themes are discovered at runtime.
    pub fn all_theme_names() -> Vec<String> {
        let mut names: Vec<String> = BUILTIN_THEMES.iter().map(|e| e.name.to_string()).collect();

        // Add user themes from config directory
        if let Some(themes_dir) = get_themes_dir() {
            if let Ok(entries) = std::fs::read_dir(themes_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|ext| ext == "toml") {
                        if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                            // Avoid duplicates (user theme would override built-in)
                            if !names.iter().any(|n| n == name) {
                                names.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }

        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_theme_loading() {
        let windows_xp = Theme::get_by_name("windows-xp");
        assert_eq!(windows_xp.name, "windows-xp");

        let midnight = Theme::get_by_name("midnight");
        assert_eq!(midnight.name, "midnight");

        // Test fallback for unknown theme (should return windows-xp as default)
        let unknown = Theme::get_by_name("nonexistent");
        assert_eq!(unknown.name, "windows-xp");
    }

    /// A broken built-in TOML would silently fall back to the default theme,
    /// so every registry entry must parse and carry its own name.
    #[test]
    fn test_all_builtin_themes_parse() {
        for entry in BUILTIN_THEMES {
            loader::load_theme_from_str(entry.content, entry.name)
                .unwrap_or_else(|e| panic!("built-in theme {} fails to parse: {e}", entry.name));
            let table: toml::Table = toml::from_str(entry.content).unwrap();
            assert_eq!(table["name"].as_str(), Some(entry.name));
        }
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

    #[test]
    fn test_rgb_to_ansi16_black() {
        // Pure black should map to ANSI black (index 0)
        let color = rgb_to_ansi16(0, 0, 0);
        assert_eq!(color, Color::Indexed(0));
    }

    #[test]
    fn test_rgb_to_ansi16_white() {
        // Pure white should map to bright white (index 15)
        let color = rgb_to_ansi16(255, 255, 255);
        assert_eq!(color, Color::Indexed(15));
    }

    #[test]
    fn test_rgb_to_ansi16_red() {
        // Bright red (255, 0, 0) should map to bright red (index 9)
        let color = rgb_to_ansi16(255, 0, 0);
        assert_eq!(color, Color::Indexed(9));

        // Dark red (128, 0, 0) should map to normal red (index 1)
        let color = rgb_to_ansi16(128, 0, 0);
        assert_eq!(color, Color::Indexed(1));
    }

    #[test]
    fn test_rgb_to_ansi16_gray() {
        // Mid-gray should map to either white (7) or bright black (8)
        let color = rgb_to_ansi16(128, 128, 128);
        assert_eq!(color, Color::Indexed(8)); // Bright black (gray)
    }

    #[test]
    fn test_ansi16_mode() {
        // Test that set_ansi16_mode works
        assert!(!is_ansi16_mode()); // Default is false

        set_ansi16_mode(true);
        assert!(is_ansi16_mode());

        set_ansi16_mode(false);
        assert!(!is_ansi16_mode());
    }
}
