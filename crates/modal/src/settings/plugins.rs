//! The Plugins tab: each installed plugin and whether it runs. Stored
//! globally the row sets the plugin's mode; stored in the project, the
//! project's switch, within that mode — the same split the config loader
//! enforces, so the tab never writes what a layer may not hold.

use termide_config::{PluginMode, PluginSettings};
use termide_i18n as i18n;

use super::fields::ContentRow;
use super::SettingsModal;

impl SettingsModal {
    /// Name the plugins installed, for the Plugins tab.
    pub fn with_plugins(mut self, names: Vec<String>) -> Self {
        self.plugin_names = names;
        self
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
        let mode = match entry.mode.unwrap_or_default() {
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
        let note = if entry.enabled == Some(true) && entry.mode == Some(PluginMode::Off) {
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
        let entry = self.config.plugins.entry(name).or_default();
        if in_project {
            entry.enabled = match entry.enabled {
                None => Some(true),
                Some(true) => Some(false),
                Some(false) => None,
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
        SettingsModal::new(config.clone(), config, in_project).with_plugins(vec!["fmt".into()])
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
}
