//! Plugin switches: `[plugins.<name>]` in the config files.
//!
//! The code of a plugin lives only in the global plugins directory; config
//! decides only whether an installed plugin runs. The global file gives each
//! plugin a [`PluginMode`], and a project file may switch the plugin within
//! that mode with `enabled`. Each key is honoured from its own layer only —
//! `layer_rules` drops `mode` from a project file and `enabled` from the
//! global one — so a repository can never wake a plugin the user did not mark
//! `opt-in`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// How a plugin runs, as the global config sets it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginMode {
    /// Runs in every project; a project may turn it off.
    #[default]
    On,
    /// Sleeps until a project turns it on.
    OptIn,
    /// Off everywhere; no project can turn it on.
    Off,
}

/// One `[plugins.<name>]` table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginSettings {
    /// The mode, from the global file only. Absent means [`PluginMode::On`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<PluginMode>,
    /// The project's switch, from the project file only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Values handed to the plugin in `initialize`, the project's laid over
    /// the global ones. Data only: termide never runs anything named here.
    #[serde(default, skip_serializing_if = "toml::Table::is_empty")]
    pub settings: toml::Table,
}

impl PluginSettings {
    /// Whether the plugin runs, given its mode and the project's switch.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        match (self.mode.unwrap_or_default(), self.enabled) {
            (PluginMode::Off, _) | (_, Some(false)) | (PluginMode::OptIn, None) => false,
            (PluginMode::On, _) | (PluginMode::OptIn, Some(true)) => true,
        }
    }
}

/// Whether the installed plugin `name` runs under `plugins`, the effective
/// `[plugins]` section of a project.
#[must_use]
pub fn plugin_enabled(plugins: &BTreeMap<String, PluginSettings>, name: &str) -> bool {
    plugins.get(name).is_none_or(PluginSettings::is_enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(mode: Option<PluginMode>, enabled: Option<bool>) -> PluginSettings {
        PluginSettings {
            mode,
            enabled,
            settings: toml::Table::new(),
        }
    }

    #[test]
    fn the_mode_bounds_the_project_switch() {
        use PluginMode::{Off, On, OptIn};
        let cases = [
            (None, None, true),
            (None, Some(false), false),
            (Some(On), None, true),
            (Some(On), Some(false), false),
            (Some(On), Some(true), true),
            (Some(OptIn), None, false),
            (Some(OptIn), Some(false), false),
            (Some(OptIn), Some(true), true),
            (Some(Off), None, false),
            (Some(Off), Some(false), false),
            (Some(Off), Some(true), false),
        ];
        for (mode, enabled, expected) in cases {
            assert_eq!(
                settings(mode, enabled).is_enabled(),
                expected,
                "mode {mode:?}, enabled {enabled:?}"
            );
        }
    }

    #[test]
    fn a_plugin_without_an_entry_runs() {
        assert!(plugin_enabled(&BTreeMap::new(), "any"));
    }
}
