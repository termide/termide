//! Click handlers for menu-bar dropdowns and their nested submenus.
//!
//! Each top-level menu has its own entry point; the main `handle_mouse_event`
//! dispatches here based on which menu is currently open.

use anyhow::Result;
use ratatui::layout::Rect;
use std::sync::Arc;

use crate::app::App;
use termide_i18n as i18n;
use termide_theme::Theme;
use termide_ui_render::{
    dropdown_geometry, dropdown_width, get_ai_agent_choice_items, get_ai_items,
    get_bookmarks_group_items, get_bookmarks_items, get_commands_group_items, get_commands_items,
    get_menu_item_x_position, get_options_items, get_projects_items, get_shell_items,
    get_tools_items, language_dropdown_geometry, theme_dropdown_geometry, AI_MENU_INDEX,
    BOOKMARKS_MENU_INDEX, COMMANDS_MENU_INDEX, OPTIONS_MENU_INDEX, PROJECTS_MENU_INDEX,
    WINDOWS_MENU_INDEX,
};

/// Hit-test a dropdown menu and return the clicked item index (if any).
///
/// `menu_x` is the left edge of the dropdown, `dropdown_y` is the top row.
/// Returns `Some(index)` if the click is on a valid item, `None` otherwise.
pub(in crate::app) fn hit_dropdown_item(
    x: u16,
    y: u16,
    menu_x: u16,
    dropdown_y: u16,
    items: &[termide_ui_render::DropdownItem],
    selected: usize,
    screen: Rect,
) -> Option<usize> {
    dropdown_geometry(items, selected, menu_x, dropdown_y, screen).item_at(x, y)
}

impl App {
    pub(in crate::app) fn screen_rect(&self) -> Rect {
        Rect::new(0, 0, self.state.terminal.width, self.state.terminal.height)
    }

    /// Handle click on Options submenu dropdown
    /// Returns true if click was handled
    pub(in crate::app) fn handle_submenu_click(&mut self, x: u16, y: u16) -> Result<bool> {
        // Get Options dropdown position
        let menu_x = get_menu_item_x_position(OPTIONS_MENU_INDEX);
        let dropdown_y = 1_u16;

        // Calculate Options dropdown dimensions
        let options_items = get_options_items(
            self.detach_available(),
            Some(&self.state.config.general.keybindings),
        );
        let options_width = dropdown_width(&options_items);
        let screen = self.screen_rect();

        // Check if nested submenu (Themes) is open
        if self.state.ui.nested_submenu.open && self.state.ui.options_submenu.selected == 0 {
            // Theme dropdown is to the right of Options dropdown
            let nested_x = menu_x + options_width;
            let nested_y = dropdown_y + 1;

            let theme_names = Theme::all_theme_names();
            let geometry = theme_dropdown_geometry(
                &theme_names,
                self.state.ui.nested_submenu.selected,
                nested_x,
                nested_y,
                screen,
            );
            if let Some(item_index) = geometry.item_at(x, y) {
                if item_index < theme_names.len() {
                    // Clear preview state - theme is confirmed
                    self.state.ui.theme_preview_original = None;
                    // Apply selected theme
                    if let Some(name) = theme_names.get(item_index) {
                        self.apply_theme(name)?;
                    }
                    self.state.close_menu();
                    return Ok(true);
                }
            }
        }

        // Check if nested submenu (Language) is open
        if self.state.ui.nested_submenu.open && self.state.ui.options_submenu.selected == 1 {
            // Language dropdown is to the right of Options dropdown
            let nested_x = menu_x + options_width;
            let nested_y = dropdown_y + 2; // Language is at index 1

            let languages = i18n::get_language_list();
            let geometry = language_dropdown_geometry(
                self.state.ui.nested_submenu.selected,
                nested_x,
                nested_y,
                screen,
            );
            if let Some(item_index) = geometry.item_at(x, y) {
                if item_index < languages.len() {
                    // Clear preview state - language is confirmed
                    self.state.ui.language_preview_original = None;
                    // Apply selected language
                    if let Some((code, name)) = languages.get(item_index) {
                        self.apply_language(code, name)?;
                    }
                    self.state.close_menu();
                    return Ok(true);
                }
            }
        }

        // Check click on Options dropdown
        if let Some(item_index) = hit_dropdown_item(
            x,
            y,
            menu_x,
            dropdown_y,
            &options_items,
            self.state.ui.options_submenu.selected,
            screen,
        ) {
            self.state.ui.options_submenu.selected = item_index;
            // Themes and Language toggle their nested dropdown on a second
            // click, which the keyboard path has no equivalent for; every
            // other entry is dispatched by `execute_submenu_action` so that
            // clicking and pressing Enter can never disagree about what an
            // item does.
            match options_items[item_index].key.as_str() {
                "themes" => {
                    if self.state.ui.nested_submenu.open
                        && self.state.ui.options_submenu.selected == 0
                    {
                        if let Some(original_name) = self.state.ui.theme_preview_original.take() {
                            self.state.theme = Theme::get_by_name(&original_name);
                        }
                        self.state.close_nested_submenu();
                    } else {
                        let theme_names = Theme::all_theme_names();
                        let current_idx = theme_names
                            .iter()
                            .position(|n| n == self.state.theme.name)
                            .unwrap_or(0);
                        self.state.ui.theme_preview_original =
                            Some(self.state.theme.name.to_string());
                        self.state.open_nested_submenu(current_idx);
                    }
                }
                "language" => {
                    use termide_i18n as i18n;
                    use termide_ui_render::find_current_language_index;
                    if self.state.ui.nested_submenu.open
                        && self.state.ui.options_submenu.selected == 1
                    {
                        if let Some(original_lang) = self.state.ui.language_preview_original.take()
                        {
                            let _ = i18n::set_language(&original_lang);
                        }
                        self.state.close_nested_submenu();
                    } else {
                        let current_idx = find_current_language_index();
                        self.state.ui.language_preview_original = Some(i18n::current_language());
                        self.state.open_nested_submenu(current_idx);
                    }
                }
                _ => self.execute_submenu_action()?,
            }
            return Ok(true);
        }

        // Click outside dropdowns - close all menus
        self.state.close_menu();
        Ok(true)
    }

    /// Handle click on Sessions submenu dropdown
    /// Returns true if click was handled
    pub(in crate::app) fn handle_sessions_submenu_click(&mut self, x: u16, y: u16) -> Result<bool> {
        let menu_x = get_menu_item_x_position(PROJECTS_MENU_INDEX);
        let items = get_projects_items(Some(&self.state.config.general.keybindings));
        if let Some(index) = hit_dropdown_item(
            x,
            y,
            menu_x,
            1,
            &items,
            self.state.ui.projects_submenu.selected,
            self.screen_rect(),
        ) {
            self.state.ui.projects_submenu.selected = index;
            self.execute_projects_submenu_action()?;
            return Ok(true);
        }
        self.state.close_menu();
        Ok(true)
    }

    /// Handle click on Tools submenu dropdown
    /// Returns true if click was handled
    pub(in crate::app) fn handle_tools_submenu_click(&mut self, x: u16, y: u16) -> Result<bool> {
        let menu_x = get_menu_item_x_position(WINDOWS_MENU_INDEX);
        let items = get_tools_items(Some(&self.state.config.general.keybindings));

        // If shell picker nested submenu is open, check clicks on it first
        if self.state.ui.tools_nested.open {
            let shell_items = get_shell_items(
                &self.state.cache.shells,
                self.state.config.terminal.default_shell.as_deref(),
            );
            if !shell_items.is_empty() {
                // Calculate nested dropdown position (same formula as in ui.rs rendering)
                let dropdown_y = 1_u16;
                let parent_width = dropdown_width(&items);
                let nested_x = menu_x + parent_width;
                let nested_y = dropdown_y + 1 + self.state.ui.tools_submenu.selected as u16;
                if let Some(index) = hit_dropdown_item(
                    x,
                    y,
                    nested_x,
                    nested_y,
                    &shell_items,
                    self.state.ui.tools_nested.selected,
                    self.screen_rect(),
                ) {
                    if let Some(shell) = self.state.cache.shells.get(index) {
                        let shell_path = shell.path.clone();
                        // Copy-on-write: mutate in-place if single owner, else clone
                        {
                            let config = Arc::make_mut(&mut self.state.config);
                            config.terminal.default_shell = Some(shell_path.clone());
                        }
                        if let Err(e) = self.save_shell_preference(&shell_path) {
                            log::warn!("Failed to save shell preference: {}", e);
                        }
                        self.state.close_menu();
                        self.handle_new_terminal_with_shell(Some(&shell_path))?;
                        return Ok(true);
                    }
                }
            }
        }

        // Check click on Tools main dropdown
        if let Some(index) = hit_dropdown_item(
            x,
            y,
            menu_x,
            1,
            &items,
            self.state.ui.tools_submenu.selected,
            self.screen_rect(),
        ) {
            self.state.ui.tools_submenu.selected = index;
            self.execute_tools_submenu_action()?;
            return Ok(true);
        }
        self.state.close_menu();
        Ok(true)
    }

    /// Handle click on Commands submenu dropdown
    /// Returns true if click was handled
    pub(in crate::app) fn handle_commands_submenu_click(&mut self, x: u16, y: u16) -> Result<bool> {
        let registry = match self.commands_registry() {
            Some(r) => r,
            None => {
                self.state.close_menu();
                return Ok(true);
            }
        };

        // If nested submenu is open, handle clicks on it first
        if self.state.ui.commands_nested.open {
            if let Some(group_name) = self.state.ui.current_commands_group.as_ref() {
                let nested_items = get_commands_group_items(&registry, group_name);
                if !nested_items.is_empty() {
                    let menu_x = get_menu_item_x_position(COMMANDS_MENU_INDEX);
                    let parent_items = get_commands_items(&registry);
                    let parent_width = dropdown_width(&parent_items);
                    let nested_x = menu_x + parent_width;
                    let nested_y = 2 + self.state.ui.commands_submenu.selected as u16;
                    if let Some(index) = hit_dropdown_item(
                        x,
                        y,
                        nested_x,
                        nested_y,
                        &nested_items,
                        self.state.ui.commands_nested.selected,
                        self.screen_rect(),
                    ) {
                        self.state.ui.commands_nested.selected = index;
                        self.execute_commands_nested_action()?;
                        return Ok(true);
                    }
                }
            }
        }

        // Check click on Commands main dropdown
        let menu_x = get_menu_item_x_position(COMMANDS_MENU_INDEX);
        let commands_items = get_commands_items(&registry);
        if let Some(index) = hit_dropdown_item(
            x,
            y,
            menu_x,
            1,
            &commands_items,
            self.state.ui.commands_submenu.selected,
            self.screen_rect(),
        ) {
            self.state.ui.commands_submenu.selected = index;
            self.execute_commands_submenu_action()?;
            return Ok(true);
        }

        self.state.close_menu();
        Ok(true)
    }

    /// Handle click on the AI submenu (sections) and its nested section list.
    pub(in crate::app) fn handle_ai_submenu_click(&mut self, x: u16, y: u16) -> Result<bool> {
        let menu_x = get_menu_item_x_position(AI_MENU_INDEX);
        let ai_items = get_ai_items(super::super::agent_panel::web_browser_shown());

        // Nested section list first.
        if self.state.ui.ai_nested.open {
            if let Some(section) = self.state.ui.current_ai_section.clone() {
                let nested_items = self.state.ai_section_items(&section);
                if !nested_items.is_empty() {
                    let nested_x = menu_x + dropdown_width(&ai_items);
                    let nested_y = 2 + self.state.ui.ai_submenu.selected as u16;

                    // The agent file-choice (third level) sits to the right of
                    // the nested list, at the selected agent's row.
                    if self.state.ui.ai_agent_choice.open {
                        let choice_items = get_ai_agent_choice_items();
                        let choice_x = nested_x + dropdown_width(&nested_items);
                        let choice_y = nested_y + 1 + self.state.ui.ai_nested.selected as u16;
                        if let Some(index) = hit_dropdown_item(
                            x,
                            y,
                            choice_x,
                            choice_y,
                            &choice_items,
                            self.state.ui.ai_agent_choice.selected,
                            self.screen_rect(),
                        ) {
                            self.state.ui.ai_agent_choice.selected = index;
                            self.execute_ai_agent_choice_action()?;
                            return Ok(true);
                        }
                    }

                    if let Some(index) = hit_dropdown_item(
                        x,
                        y,
                        nested_x,
                        nested_y,
                        &nested_items,
                        self.state.ui.ai_nested.selected,
                        self.screen_rect(),
                    ) {
                        self.state.ui.ai_nested.selected = index;
                        self.execute_ai_nested_action(&section)?;
                        return Ok(true);
                    }
                }
            }
        }

        // The AI main dropdown (the four sections).
        if let Some(index) = hit_dropdown_item(
            x,
            y,
            menu_x,
            1,
            &ai_items,
            self.state.ui.ai_submenu.selected,
            self.screen_rect(),
        ) {
            self.state.ui.ai_submenu.selected = index;
            self.open_ai_selected_section();
            return Ok(true);
        }

        self.state.close_menu();
        Ok(true)
    }

    /// Handle click on stash dropdown or outside it (close).
    pub(in crate::app) fn handle_stash_dropdown_click(&mut self, x: u16, y: u16) -> Result<()> {
        let items = termide_ui_render::get_stash_items(
            &self.state.stash.entries,
            self.state.stash.has_changes,
        );
        if let Some(btn_area) = self.state.ui.stash_button_area {
            if let Some(index) = hit_dropdown_item(
                x,
                y,
                btn_area.x,
                btn_area.bottom(),
                &items,
                self.state.ui.stash_submenu.selected,
                self.screen_rect(),
            ) {
                self.state.ui.stash_submenu.selected = index;
                self.execute_stash_submenu_action()?;
                return Ok(());
            }
        }
        // Click outside → close dropdown
        self.state.ui.stash_submenu.close();
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Handle click on Bookmarks submenu dropdown
    /// Returns true if click was handled
    pub(in crate::app) fn handle_bookmarks_submenu_click(
        &mut self,
        x: u16,
        y: u16,
    ) -> Result<bool> {
        let bookmarks_items = get_bookmarks_items(
            &self.state.bookmarks,
            self.state.project_bookmarks.as_ref(),
            Some(&self.state.config.general.keybindings),
        );

        // If nested submenu is open, handle clicks on it first
        if self.state.ui.bookmarks_nested.open {
            if let Some(group_name) = self.state.ui.current_bookmarks_group.as_ref() {
                let nested_items = get_bookmarks_group_items(
                    &self.state.bookmarks,
                    self.state.project_bookmarks.as_ref(),
                    group_name,
                    self.state.ui.current_bookmarks_group_is_project,
                );
                if !nested_items.is_empty() {
                    let menu_x = get_menu_item_x_position(BOOKMARKS_MENU_INDEX);
                    let parent_width = dropdown_width(&bookmarks_items);
                    let nested_x = menu_x + parent_width;
                    let nested_y = 2 + self.state.ui.bookmarks_submenu.selected as u16;
                    if let Some(index) = hit_dropdown_item(
                        x,
                        y,
                        nested_x,
                        nested_y,
                        &nested_items,
                        self.state.ui.bookmarks_nested.selected,
                        self.screen_rect(),
                    ) {
                        self.state.ui.bookmarks_nested.selected = index;
                        self.execute_bookmarks_nested_action()?;
                        return Ok(true);
                    }
                }
            }
        }

        // Check click on Bookmarks main dropdown
        let menu_x = get_menu_item_x_position(BOOKMARKS_MENU_INDEX);
        if let Some(index) = hit_dropdown_item(
            x,
            y,
            menu_x,
            1,
            &bookmarks_items,
            self.state.ui.bookmarks_submenu.selected,
            self.screen_rect(),
        ) {
            self.state.ui.bookmarks_submenu.selected = index;
            self.execute_bookmarks_submenu_action()?;
            return Ok(true);
        }

        self.state.close_menu();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_ui_render::DropdownItem;

    const SCREEN: Rect = Rect::new(0, 0, 120, 40);

    // Regression: `hit_dropdown_item` sized dropdowns as `label + 4`, while
    // `Dropdown` draws them `label + shortcut column + 6` wide. Clicks on the
    // right part of a drawn dropdown fell outside it, and nested submenus
    // (placed at `menu_x + drawn width`) were hit-tested at the wrong column.

    #[test]
    fn click_on_shortcut_column_selects_the_row() {
        let items = vec![
            DropdownItem::new("Open", "open"),
            DropdownItem::new("Terminal", "terminal")
                .with_submenu()
                .with_shortcut(Some("Alt+T".into())),
            DropdownItem::new("Files", "files").with_shortcut(Some("Alt+F".into())),
        ];
        let width = dropdown_width(&items);

        // On the "Alt+F" text of the "Files" row (index 2, below the border).
        assert_eq!(
            hit_dropdown_item(width - 4, 3, 0, 0, &items, 0, SCREEN),
            Some(2)
        );
        // One column past the right border.
        assert_eq!(hit_dropdown_item(width, 3, 0, 0, &items, 0, SCREEN), None);
    }

    #[test]
    fn click_near_right_border_of_nested_submenu_selects_the_row() {
        let shells = vec![
            DropdownItem::new("Sh", "/bin/sh"),
            DropdownItem::new("Bash", "/bin/bash"),
        ];
        let (x0, y0) = (20, 3);
        let width = dropdown_width(&shells);

        // Last column inside the right border, on the "Bash" row.
        assert_eq!(
            hit_dropdown_item(x0 + width - 2, y0 + 2, x0, y0, &shells, 0, SCREEN),
            Some(1)
        );
    }

    #[test]
    fn click_on_dropdown_pushed_up_by_short_terminal_selects_the_drawn_row() {
        let items: Vec<DropdownItem> = (0..11)
            .map(|i| DropdownItem::new(format!("Item {i}"), format!("item{i}")))
            .collect();
        let screen = Rect::new(0, 0, 80, 12);

        assert_eq!(hit_dropdown_item(2, 1, 0, 1, &items, 0, screen), Some(0));
        assert_eq!(hit_dropdown_item(2, 4, 0, 1, &items, 0, screen), Some(3));
        assert_eq!(hit_dropdown_item(2, 10, 0, 1, &items, 10, screen), Some(10));
    }
}
