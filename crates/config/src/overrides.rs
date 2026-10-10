//! Settings forced for one run, which must not reach the config files.
//!
//! `--log-level`, `--no-lsp` and the Linux console's theme replace values of
//! the running config. The files keep what the user chose: before a save the
//! forced values are swapped back for the ones they replaced, and after it
//! they are forced again. A value the user changes by hand while it is forced
//! is their choice: it is saved, and the override ends for the session.

use crate::Config;

/// Theme the Linux console gets in place of the default one: it shows 16
/// colours only.
pub const CONSOLE_THEME: &str = "norton-commander";

/// One forced field: the value it replaced and the value it was given.
#[derive(Debug, Clone, PartialEq)]
struct Forced<T> {
    replaced: T,
    forced: T,
}

/// What this run forces on the config, and what it replaced.
#[derive(Debug, Clone, Default)]
pub struct RuntimeOverrides {
    log_level: Option<String>,
    lsp_off: bool,
    console_theme: bool,
    forced_log_level: Option<Forced<String>>,
    forced_lsp: Option<Forced<bool>>,
    forced_theme: Option<Forced<String>>,
}

impl RuntimeOverrides {
    /// Overrides from the command line (`--log-level`, `--no-lsp`) and the
    /// terminal (`console_theme`: a Linux console, which gets
    /// [`CONSOLE_THEME`] while the theme is left at the default).
    pub fn new(log_level: Option<String>, lsp_off: bool, console_theme: bool) -> Self {
        Self {
            log_level,
            lsp_off,
            console_theme,
            ..Self::default()
        }
    }

    /// Force the overrides on `config`, a config as the files give it,
    /// remembering what they replace.
    pub fn apply(&mut self, config: &mut Config) {
        self.forced_log_level = self.log_level.as_ref().map(|level| Forced {
            replaced: std::mem::replace(&mut config.logging.min_level, level.clone()),
            forced: level.clone(),
        });
        self.forced_lsp = self.lsp_off.then(|| Forced {
            replaced: std::mem::replace(&mut config.lsp.enabled, false),
            forced: false,
        });
        self.forced_theme = (self.console_theme
            && config.general.theme == crate::defaults::THEME_NAME)
            .then(|| Forced {
                replaced: std::mem::replace(&mut config.general.theme, CONSOLE_THEME.to_string()),
                forced: CONSOLE_THEME.to_string(),
            });
    }

    /// `config` with what [`apply`](Self::apply) forced put back, ready to
    /// be written to a file. A forced field that `config` no longer holds
    /// the forced value in was changed by the user: it is kept as it is, and
    /// its override is dropped for the rest of the session.
    pub fn strip(&mut self, mut config: Config) -> Config {
        if let Some(f) = self.forced_log_level.clone() {
            if config.logging.min_level == f.forced {
                config.logging.min_level = f.replaced;
            } else {
                self.log_level = None;
            }
        }
        if let Some(f) = self.forced_lsp.clone() {
            if config.lsp.enabled == f.forced {
                config.lsp.enabled = f.replaced;
            } else {
                self.lsp_off = false;
            }
        }
        if let Some(f) = self.forced_theme.clone() {
            if config.general.theme == f.forced {
                config.general.theme = f.replaced;
            } else {
                self.console_theme = false;
            }
        }
        config
    }

    /// `config` with the overrides forced on it, for showing, without
    /// recording what they replaced: the record stays that of the running
    /// config.
    pub fn applied_to(&self, config: &Config) -> Config {
        let mut config = config.clone();
        self.clone().apply(&mut config);
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored() -> Config {
        let mut config = Config::default();
        config.normalize();
        config.logging.min_level = "warn".into();
        config
    }

    #[test]
    fn forced_values_never_reach_the_saved_config() {
        let mut overrides = RuntimeOverrides::new(Some("trace".into()), true, true);
        let mut running = stored();
        overrides.apply(&mut running);
        assert_eq!(running.logging.min_level, "trace");
        assert!(!running.lsp.enabled);
        assert_eq!(running.general.theme, CONSOLE_THEME);

        running.editor.tab_size = 8;
        let saved = overrides.strip(running);
        assert_eq!(saved.logging.min_level, "warn");
        assert!(saved.lsp.enabled);
        assert_eq!(saved.general.theme, "default");
        assert_eq!(saved.editor.tab_size, 8);
    }

    #[test]
    fn overrides_return_after_a_save() {
        let mut overrides = RuntimeOverrides::new(Some("trace".into()), true, false);
        let mut running = stored();
        overrides.apply(&mut running);
        let mut running = overrides.strip(running);
        overrides.apply(&mut running);
        assert_eq!(running.logging.min_level, "trace");
        assert!(!running.lsp.enabled);
    }

    #[test]
    fn a_value_changed_by_hand_is_saved_and_ends_its_override() {
        let mut overrides = RuntimeOverrides::new(None, true, true);
        let mut running = stored();
        overrides.apply(&mut running);
        running.lsp.enabled = true;
        running.general.theme = "dracula".into();

        let mut saved = overrides.strip(running);
        assert!(saved.lsp.enabled);
        assert_eq!(saved.general.theme, "dracula");

        // Picking the default theme later keeps it: the console no longer
        // swaps it.
        saved.general.theme = "default".into();
        overrides.apply(&mut saved);
        assert!(saved.lsp.enabled);
        assert_eq!(saved.general.theme, "default");
    }

    #[test]
    fn the_console_theme_replaces_only_the_default_theme() {
        let mut overrides = RuntimeOverrides::new(None, false, true);
        let mut running = stored();
        running.general.theme = "dracula".into();
        overrides.apply(&mut running);
        assert_eq!(running.general.theme, "dracula");
    }

    #[test]
    fn showing_a_config_leaves_the_record_alone() {
        let mut overrides = RuntimeOverrides::new(Some("trace".into()), false, false);
        let mut running = stored();
        overrides.apply(&mut running);

        let mut other = stored();
        other.logging.min_level = "error".into();
        assert_eq!(overrides.applied_to(&other).logging.min_level, "trace");

        assert_eq!(overrides.strip(running).logging.min_level, "warn");
    }
}
