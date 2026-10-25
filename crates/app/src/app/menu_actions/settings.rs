//! Settings menu actions — themes, language, preferences.

use anyhow::Result;

use super::super::App;
use termide_i18n as i18n;
use termide_theme::Theme;

impl App {
    /// Handle keyboard event in Themes nested submenu
    pub(in crate::app) fn handle_themes_nested_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        let theme_names = Theme::all_theme_names();
        let theme_count = theme_names.len();

        match key.code {
            crossterm::event::KeyCode::Esc | crossterm::event::KeyCode::Left => {
                // Restore original theme on cancel
                if let Some(original_name) = self.state.ui.theme_preview_original.take() {
                    self.state.theme = Theme::get_by_name(&original_name);
                }
                // Close nested submenu, return to parent
                self.state.close_nested_submenu();
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Down => {
                self.navigate_nested_submenu(key.code, theme_count);
                // Live preview: apply theme on cursor move
                if let Some(name) = theme_names.get(self.state.ui.nested_submenu.selected) {
                    self.state.theme = Theme::get_by_name(name);
                }
            }
            crossterm::event::KeyCode::Enter => {
                // Clear preview state - theme is confirmed
                self.state.ui.theme_preview_original = None;
                // Apply selected theme and save preference
                if let Some(name) = theme_names.get(self.state.ui.nested_submenu.selected) {
                    self.apply_theme(name)?;
                }
                // Close all menus
                self.state.close_menu();
            }
            crossterm::event::KeyCode::Right => {
                // Restore original theme and switch to next root menu
                if let Some(original_name) = self.state.ui.theme_preview_original.take() {
                    self.state.theme = Theme::get_by_name(&original_name);
                }
                self.switch_to_next_menu()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle keyboard event in Language nested submenu
    pub(in crate::app) fn handle_language_nested_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        let languages = i18n::get_language_list();
        let lang_count = languages.len();

        match key.code {
            crossterm::event::KeyCode::Esc | crossterm::event::KeyCode::Left => {
                // Restore original language on cancel
                if let Some(original_lang) = self.state.ui.language_preview_original.take() {
                    let _ = i18n::set_language(&original_lang);
                }
                // Close nested submenu, return to parent
                self.state.close_nested_submenu();
            }
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Down => {
                self.navigate_nested_submenu(key.code, lang_count);
                // Live preview: apply language on cursor move
                if let Some((code, _)) = languages.get(self.state.ui.nested_submenu.selected) {
                    let _ = i18n::set_language(code);
                }
            }
            crossterm::event::KeyCode::Enter => {
                // Clear preview state - language is confirmed
                self.state.ui.language_preview_original = None;
                // Apply selected language and save preference
                if let Some((code, name)) = languages.get(self.state.ui.nested_submenu.selected) {
                    self.apply_language(code, name)?;
                }
                // Close all menus
                self.state.close_menu();
            }
            crossterm::event::KeyCode::Right => {
                // Restore original language and switch to next root menu
                if let Some(original_lang) = self.state.ui.language_preview_original.take() {
                    let _ = i18n::set_language(&original_lang);
                }
                self.switch_to_next_menu()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Apply language by code and save preference
    pub(in crate::app) fn apply_language(
        &mut self,
        lang_code: &str,
        lang_name: &str,
    ) -> Result<()> {
        if let Err(e) = i18n::set_language(lang_code) {
            log::warn!("Failed to set language: {}", e);
            self.show_error_modal(format!("Failed to set language: {}", e));
            return Ok(());
        }

        let t = i18n::t();
        self.state.set_info(t.language_changed(lang_name));

        // Save preference to config file
        if let Err(e) = self.save_language_preference(lang_code) {
            log::warn!("Failed to save language preference: {}", e);
        }

        Ok(())
    }

    /// Save language preference to the global settings.
    fn save_language_preference(&mut self, lang_code: &str) -> Result<()> {
        self.save_global_preference(|config| config.general.language = lang_code.to_string())
    }

    /// Apply theme by name and save preference
    pub(in crate::app) fn apply_theme(&mut self, theme_name: &str) -> Result<()> {
        let new_theme = Theme::get_by_name(theme_name);
        self.state.theme = new_theme;

        let t = i18n::t();
        self.state.set_info(t.theme_changed(theme_name));

        // Save preference to config file
        if let Err(e) = self.save_theme_preference(theme_name) {
            log::warn!("Failed to save theme preference: {}", e);
        }

        Ok(())
    }

    /// Save theme preference to the global settings.
    fn save_theme_preference(&mut self, theme_name: &str) -> Result<()> {
        self.save_global_preference(|config| config.general.theme = theme_name.to_string())
    }
}
