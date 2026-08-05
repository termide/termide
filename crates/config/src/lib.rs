//! Configuration management for termide.
//!
//! This crate provides configuration loading, saving, and validation
//! with support for TOML format and XDG directory conventions.

pub mod bookmarks;
pub mod commands;
pub mod conflicts;
pub mod constants;
pub mod diff;
pub mod keybindings;
mod settings;
mod xdg;

pub use bookmarks::{Bookmark, BookmarkType, BookmarksConfig};
pub use conflicts::{
    binding_requires_kitty, enumerate_bindings, find_conflicts, BindingLocation, ConflictKind,
    HotkeyConflict,
};
pub use diff::{diff_toml, merge_partial};
pub use keybindings::{
    is_go_end, is_go_home, is_move_down, is_move_up, parse_keybinding, DatabaseKeybindings,
    EditorKeybindings, FileManagerKeybindings, GitDiffKeybindings, GitLogKeybindings,
    GitStatusKeybindings, GlobalKeybindings, KeyBinding, ParsedKeyBinding, TerminalKeybindings,
    ViewerKeybindings,
};
pub use settings::{
    Config, CustomLanguage, DatabaseSettings, EditorSettings, FileManagerSettings, GeneralSettings,
    GitDiffSettings, GitLogSettings, GitStatusSettings, HighlightSettings, IconMode, LegacyConfig,
    LinkOpen, LoggingSettings, LspServerSettings, LspSettings, TerminalSettings, VfsSettings,
    ViewerSettings,
};
pub use xdg::{get_config_dir, get_data_dir};

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Default values as constants
pub mod defaults {
    pub const THEME_NAME: &str = "default";
    pub const LANGUAGE: &str = "auto";
    pub const AUTO_STACK_THRESHOLD: u16 = 80;
    pub const MIN_PANEL_WIDTH: u16 = 20;
    pub const SESSION_RETENTION_DAYS: u32 = 30;
    pub const TAB_SIZE: usize = 4;
    pub const SHOW_GIT_DIFF: bool = true;
    pub const WORD_WRAP: bool = true;
    pub const VIM_MODE: bool = false;
    pub const LARGE_FILE_THRESHOLD_MB: u64 = 5;
    pub const CONTENT_SEARCH_MAX_FILE_SIZE_MB: u64 = 1;
    pub const EXTENDED_VIEW_WIDTH: usize = 50;
    pub const FM_DIR_SIZE_IN_WIDE_VIEW: bool = true;
    /// Per-directory time budget (ms) for the size walk rendered in FM wide view.
    /// `0` disables the feature the same way as `FM_DIR_SIZE_IN_WIDE_VIEW = false`.
    pub const FM_DIR_SIZE_BUDGET_MS: u64 = 100;
    pub const MIN_LOG_LEVEL: &str = "info";
    pub const RESOURCE_MONITOR_INTERVAL: u64 = 2000;
    pub const BELL_ON_OPERATION_COMPLETE: bool = true;
    /// Minimum panel width (columns) to enable tree/wide view in list panels
    pub const TREE_VIEW_MIN_WIDTH: u16 = 35;
    // LSP defaults
    pub const LSP_ENABLED: bool = true;
    pub const LSP_AUTO_COMPLETION: bool = true;
    pub const LSP_COMPLETION_DELAY_MS: u64 = 150;
    pub const LSP_HOVER_DELAY_MS: u64 = 1000;
}

/// Path to the per-project config override file:
/// `<project_root>/.termide/config.toml`.
///
/// This path is the single source of truth for whether a project has an
/// active override — its existence enables overlay loading and switches
/// the Settings save target. The file is created/removed by an explicit
/// user action in the Settings modal.
pub fn project_config_path(project_root: &Path) -> PathBuf {
    project_root.join(".termide").join("config.toml")
}

impl Config {
    /// Load configuration from the global file.
    ///
    /// On first run, creates the file with `Config::default()`. Returns a
    /// fully `normalize()`-d `Config` (all `Option<KeyBinding>` slots
    /// filled). Supports legacy-flat-format migration on read.
    ///
    /// Does **not** layer in the per-project override file. For startup
    /// use [`Config::load_layered`].
    pub fn load() -> Result<Self> {
        let config_path = Self::config_file_path()?;

        if config_path.exists() {
            let original_content = std::fs::read_to_string(&config_path)?;

            // Try parsing as new structured format first
            let mut config: Self = match toml::from_str(&original_content) {
                Ok(config) => config,
                Err(_) => {
                    // Legacy flat format → migrate. Save the converted result so the
                    // user's file moves to the new shape; the file shrinks to
                    // diff-against-defaults form on the first post-migration save.
                    let legacy: LegacyConfig = toml::from_str(&original_content)?;
                    let mut config: Config = legacy.into();
                    config.normalize();
                    config.save_global()?;
                    return Ok(config);
                }
            };

            // Fill None keybinding values with defaults. The on-disk file is
            // intentionally not rewritten here — diff-form files would otherwise
            // expand back to fully-normalized content on every startup.
            config.normalize();
            Ok(config)
        } else {
            // First run: don't write a config file with the defaults — that
            // would defeat the diff-against-defaults invariant. The file is
            // created lazily, the first time the user changes a setting.
            let mut config = Self::default();
            config.normalize();
            Self::ensure_themes_dir()?;
            Ok(config)
        }
    }

    /// Load configuration with the layered global → project overlay.
    ///
    /// Returns `(effective, global_layer)` where:
    /// - `effective` is `Config::default()` overlaid with the global file
    ///   (if any) and then with `<project_root>/.termide/config.toml`
    ///   (if any). This is what the running app sees.
    /// - `global_layer` is the same minus the project overlay — the
    ///   baseline used when computing diffs for the project file.
    ///
    /// When `custom_global` is `Some`, layering is bypassed: the file is
    /// loaded as the entire effective config, and `global_layer` is a
    /// clone of it. This preserves the historical `--config` semantics
    /// where the user-supplied file is the single source of truth.
    pub fn load_layered(custom_global: Option<&Path>, project_root: &Path) -> Result<(Self, Self)> {
        if let Some(path) = custom_global {
            let cfg = Self::load_from(path)?;
            return Ok((cfg.clone(), cfg));
        }

        // Start with built-in defaults.
        let mut value = toml::Value::try_from(Self::default())?;

        // Overlay the global file if it exists. Failure to parse is logged
        // and ignored — we'd rather start with defaults than refuse to launch.
        let global_path = Self::config_file_path()?;
        let mut global_existed = false;
        if global_path.exists() {
            global_existed = true;
            match std::fs::read_to_string(&global_path)
                .ok()
                .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
            {
                Some(global_value) => merge_partial(&mut value, &global_value),
                None => log::warn!(
                    "Failed to parse global config at {} — starting from defaults",
                    global_path.display()
                ),
            }
        }

        // Snapshot the global layer (defaults + global) before applying the
        // project overlay. This is the baseline for diff-saving the project
        // file later.
        let global_layer_value = value.clone();

        // Overlay the project file if it exists.
        let project_path = project_config_path(project_root);
        if project_path.exists() {
            match std::fs::read_to_string(&project_path)
                .ok()
                .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
            {
                Some(project_value) => merge_partial(&mut value, &project_value),
                None => log::warn!(
                    "Failed to parse project config at {} — using global only",
                    project_path.display()
                ),
            }
        }

        // A single invalid field in the merged config must not discard every
        // other user setting. Degrade layer-by-layer instead of refusing the
        // whole document: full merge → global layer (defaults + global) →
        // built-in defaults, keeping the largest valid subset.
        let mut effective: Self = value.try_into().unwrap_or_else(|e| {
            log::warn!("Merged config is invalid ({e}); falling back to the global layer");
            global_layer_value.clone().try_into().unwrap_or_else(|e2| {
                log::warn!("Global config layer is also invalid ({e2}); using defaults");
                Self::default()
            })
        });
        effective.normalize();
        let mut global_layer: Self = global_layer_value.try_into().unwrap_or_else(|e| {
            log::warn!("Global config layer is invalid ({e}); using defaults as baseline");
            Self::default()
        });
        global_layer.normalize();

        // Make sure the themes directory exists on first run.
        if !global_existed {
            Self::ensure_themes_dir()?;
        }

        Ok((effective, global_layer))
    }

    /// Load configuration from a specific file path.
    ///
    /// Unlike `load()`, this does not create the file if it doesn't exist
    /// and does not auto-save normalized content back.
    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut config: Self = toml::from_str(&content)?;
        config.normalize();
        Ok(config)
    }

    /// Write only the fields that differ from `baseline` to `path`.
    ///
    /// Both operands are serialized to `toml::Value` and `diff_toml` is
    /// applied. The result is rendered as pretty TOML and written to
    /// `path`, creating any missing parent directories. When the diff is
    /// empty the file is written as an empty document — callers that
    /// want different behaviour (e.g. delete an empty file) should
    /// inspect the diff themselves.
    pub fn save_to(&self, path: &Path, baseline: &Config) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let actual_value = toml::Value::try_from(self)?;
        let baseline_value = toml::Value::try_from(baseline)?;
        let diff = diff_toml(&actual_value, &baseline_value)
            .unwrap_or_else(|| toml::Value::Table(Default::default()));

        let content = toml::to_string_pretty(&diff)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Save the user's global config: only fields differing from the
    /// built-in `Config::default()` are written.
    pub fn save_global(&self) -> Result<()> {
        let path = Self::config_file_path()?;
        let default = Config::default();
        self.save_to(&path, &default)
    }

    /// Save a per-project override at `<project_root>/.termide/config.toml`:
    /// only fields differing from the supplied `global_layer` baseline are
    /// written. `global_layer` should be the `Config` that the global file
    /// would produce when overlaid on `Config::default()` — i.e. the
    /// `global_layer` value returned by [`Config::load_layered`].
    pub fn save_project(&self, project_root: &Path, global_layer: &Config) -> Result<()> {
        let path = project_config_path(project_root);
        self.save_to(&path, global_layer)
    }

    /// Backwards-compatible alias for [`Config::save_global`]. Older call
    /// sites used to write the full config to disk; the diff-against-
    /// defaults shrink is now applied unconditionally.
    pub fn save(&self) -> Result<()> {
        self.save_global()
    }

    /// Get path to config file.
    pub fn config_file_path() -> Result<PathBuf> {
        Ok(get_config_dir()?.join("config.toml"))
    }

    /// Get path to themes directory.
    pub fn get_themes_dir() -> Result<PathBuf> {
        Ok(get_config_dir()?.join("themes"))
    }

    /// Check if path is the config file.
    pub fn is_config_file(path: &std::path::Path) -> bool {
        Self::config_file_path().map(|p| p == path).unwrap_or(false)
    }

    /// Validate config content.
    pub fn validate_content(content: &str) -> Result<Config> {
        toml::from_str(content).map_err(|e| anyhow::anyhow!("{}", e))
    }

    /// Ensure themes directory exists.
    fn ensure_themes_dir() -> Result<()> {
        let themes_dir = Self::get_themes_dir()?;
        if !themes_dir.exists() {
            std::fs::create_dir_all(themes_dir)?;
        }
        Ok(())
    }
}
