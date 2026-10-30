//! The Plugins tab: each installed plugin and whether it runs. Stored
//! globally the row sets the plugin's mode; stored in the project, the
//! project's switch, within that mode — the same split the config loader
//! enforces, so the tab never writes what a layer may not hold.

use termide_config::{PluginMode, PluginSettings};
use termide_i18n as i18n;

use super::fields::ContentRow;
use super::SettingsModal;

impl SettingsModal {
    /// Name the plugins installed, for the Plugins tab, each with whether
    /// termide carries it itself.
    pub fn with_plugins(mut self, plugins: Vec<(String, bool)>) -> Self {
        self.builtin_plugins = plugins
            .iter()
            .filter(|(_, builtin)| *builtin)
            .map(|(name, _)| name.clone())
            .collect();
        self.plugin_names = plugins.into_iter().map(|(name, _)| name).collect();
        self
    }

    /// The mode plugin `name` runs in without one set: on for an installed
    /// plugin, off for a built-in one.
    fn default_mode(&self, name: &str) -> PluginMode {
        if self.builtin_plugins.contains(name) {
            PluginMode::Off
        } else {
            PluginMode::On
        }
    }

    /// The rows of the Plugins tab.
    pub(super) fn plugins_tab_rows(&self) -> Vec<ContentRow> {
        let t = i18n::t();
        let mut rows = vec![ContentRow::Header(if self.plugin_names.is_empty() {
            t.plugins_none()
        } else {
            t.settings_header_plugins()
        })];
        rows.extend((0..self.plugin_names.len()).map(ContentRow::Plugin));
        rows
    }

    fn plugin_entry(&self, name: &str) -> PluginSettings {
        self.config.plugins.get(name).cloned().unwrap_or_default()
    }

    /// What the row of plugin `name` says: its mode globally; in the
    /// project, the project's switch and what comes of it.
    pub(super) fn plugin_summary(&self, name: &str) -> String {
        let t = i18n::t();
        let entry = self.plugin_entry(name);
        let effective = entry.mode.unwrap_or(self.default_mode(name));
        let mode = match effective {
            PluginMode::On => t.settings_plugin_mode_on(),
            PluginMode::OptIn => t.settings_plugin_mode_opt_in(),
            PluginMode::Off => t.settings_plugin_mode_off(),
        };
        if !self.in_project {
            return mode.to_string();
        }
        let switch = match entry.enabled {
            None => t.settings_plugin_follow(),
            Some(true) => t.settings_plugin_on(),
            Some(false) => t.settings_plugin_off(),
        };
        let note = if entry.enabled == Some(true) && effective == PluginMode::Off {
            t.settings_plugin_blocked()
        } else {
            mode
        };
        format!("{switch} · {note}")
    }

    /// Step plugin `index`'s value: the mode globally (on, opt-in, off), the
    /// project's switch in the project (as the mode says, on, off).
    pub(super) fn cycle_plugin(&mut self, index: usize) {
        let Some(name) = self.plugin_names.get(index).cloned() else {
            return;
        };
        let in_project = self.in_project;
        let builtin = self.builtin_plugins.contains(&name);
        let entry = self.config.plugins.entry(name).or_default();
        if in_project {
            entry.enabled = match entry.enabled {
                None => Some(true),
                Some(true) => Some(false),
                Some(false) => None,
            };
        } else if builtin {
            // Off by default: on, opt-in, then back to no mode at all.
            entry.mode = match entry.mode {
                None | Some(PluginMode::Off) => Some(PluginMode::On),
                Some(PluginMode::On) => Some(PluginMode::OptIn),
                Some(PluginMode::OptIn) => None,
            };
        } else {
            entry.mode = match entry.mode.unwrap_or_default() {
                PluginMode::On => Some(PluginMode::OptIn),
                PluginMode::OptIn => Some(PluginMode::Off),
                PluginMode::Off => None,
            };
        }
        self.mark_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_config::Config;

    fn modal(in_project: bool) -> SettingsModal {
        let config = Config::default();
        SettingsModal::new(config.clone(), config, in_project)
            .with_plugins(vec![("fmt".into(), false), ("p4".into(), true)])
    }

    #[test]
    fn globally_the_row_sets_the_mode() {
        let mut m = modal(false);
        let t = i18n::t();
        assert_eq!(m.plugin_summary("fmt"), t.settings_plugin_mode_on());
        m.cycle_plugin(0);
        assert_eq!(m.config.plugins["fmt"].mode, Some(PluginMode::OptIn));
        m.cycle_plugin(0);
        m.cycle_plugin(0);
        assert_eq!(m.config.plugins["fmt"].mode, None, "back to the default");
        assert_eq!(m.config.plugins["fmt"].enabled, None, "never the switch");
    }

    #[test]
    fn in_the_project_the_row_sets_the_switch() {
        let mut m = modal(true);
        m.config.plugins.entry("fmt".into()).or_default().mode = Some(PluginMode::Off);
        m.cycle_plugin(0);
        let entry = &m.config.plugins["fmt"];
        assert_eq!(entry.enabled, Some(true));
        assert_eq!(entry.mode, Some(PluginMode::Off), "the mode is left alone");
        assert!(m
            .plugin_summary("fmt")
            .contains(i18n::t().settings_plugin_blocked()));
    }

    #[test]
    fn a_builtin_plugin_is_off_until_given_a_mode() {
        let mut m = modal(false);
        let t = i18n::t();
        assert_eq!(m.plugin_summary("p4"), t.settings_plugin_mode_off());
        m.cycle_plugin(1);
        assert_eq!(m.config.plugins["p4"].mode, Some(PluginMode::On));
        m.cycle_plugin(1);
        assert_eq!(m.config.plugins["p4"].mode, Some(PluginMode::OptIn));
        m.cycle_plugin(1);
        assert_eq!(m.config.plugins["p4"].mode, None, "back to off");

        let mut m = modal(true);
        m.cycle_plugin(1);
        assert!(
            m.plugin_summary("p4").contains(t.settings_plugin_blocked()),
            "a project cannot wake it alone"
        );
    }
}
