//! Settings modal input handling: per-focus-area key routing, LSP server
//! edit form, inline field editing, buttons, and keybinding capture.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_config::{Config, KeyBinding, LspServerSettings};

use crate::ModalResult;

use super::fields::{
    apply_enum_value, cycle_enum_backward, cycle_enum_forward, fields_for_tab, toggle_field,
    ContentRow, FieldType,
};
use super::kb::{format_key_event, get_kb_binding, kb_binding_names, set_kb_value, KB_SECTIONS};
use super::{
    button_labels, EnumPicker, FocusArea, KbMode, LspMode, SettingsModal, SettingsResult,
    SettingsTab, SidebarRow, BUTTON_APPLY, BUTTON_PROJECT_OVERRIDE, BUTTON_RESET,
    ENUM_PICKER_MAX_VISIBLE,
};

impl SettingsModal {
    pub(super) fn handle_sidebar_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ModalResult<SettingsResult>>> {
        let rows = self.visible_sidebar_rows();
        if rows.is_empty() {
            return Ok(None);
        }
        // Sync cursor with current active tab on first entry.
        if self.sidebar_cursor >= rows.len() {
            self.sidebar_cursor = self.sidebar_cursor_for_active();
        }

        match key.code {
            KeyCode::Up => {
                if self.sidebar_cursor > 0 {
                    self.sidebar_cursor -= 1;
                    self.preview_sidebar_row(rows[self.sidebar_cursor]);
                }
            }
            KeyCode::Down => {
                if self.sidebar_cursor + 1 < rows.len() {
                    self.sidebar_cursor += 1;
                    self.preview_sidebar_row(rows[self.sidebar_cursor]);
                }
            }
            KeyCode::Tab => {
                // Cycle focus zones: Sidebar → Content → Buttons → Sidebar.
                self.focus = FocusArea::Content;
                self.content_scroll = 0;
                self.field_cursor = self.first_selectable_row();
            }
            KeyCode::BackTab => {
                // Reverse cycle: Sidebar → Buttons → Content → Sidebar.
                self.selected_button = 0;
                self.focus = FocusArea::Buttons;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let row = rows[self.sidebar_cursor];
                if matches!(row, SidebarRow::KbGroupHeader) {
                    // Toggle expansion; after toggling, refresh rows and stay on header.
                    self.activate_sidebar_row(row);
                    let new_rows = self.visible_sidebar_rows();
                    if self.keybindings_expanded {
                        // Move cursor to first child for convenience.
                        if self.sidebar_cursor + 1 < new_rows.len() {
                            self.sidebar_cursor += 1;
                            self.activate_sidebar_row(new_rows[self.sidebar_cursor]);
                        }
                    }
                } else {
                    // Leaf or KbChild — move focus to content.
                    self.activate_sidebar_row(row);
                    self.focus = FocusArea::Content;
                    self.content_scroll = 0;
                    self.field_cursor = self.first_selectable_row();
                }
            }
            KeyCode::Left => {
                // Tree-style: collapse expanded group or move from child to its header.
                // Does not change focus area.
                match rows[self.sidebar_cursor] {
                    SidebarRow::KbGroupHeader if self.keybindings_expanded => {
                        self.keybindings_expanded = false;
                    }
                    SidebarRow::KbChild(_) => {
                        let new_rows = self.visible_sidebar_rows();
                        if let Some(pos) = new_rows
                            .iter()
                            .position(|r| matches!(r, SidebarRow::KbGroupHeader))
                        {
                            self.sidebar_cursor = pos;
                        }
                    }
                    _ => {}
                }
            }
            KeyCode::Esc => {
                return Ok(Some(ModalResult::Cancelled));
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(Some(ModalResult::Confirmed(SettingsResult::Apply(
                    Box::new(self.config.clone()),
                ))));
            }
            _ => {}
        }
        Ok(None)
    }

    /// Act on the focused row: toggle a switch, cycle an enum, start editing
    /// a value, or open an LSP server form.
    ///
    /// Shared by Enter/Space and by a mouse click, so that clicking a checkbox
    /// does what pressing Enter on it does — previously a click only moved the
    /// cursor, and the switch stayed put.
    pub(super) fn activate_current_row(&mut self) {
        let field_desc = match self.current_row() {
            Some(ContentRow::Field(i)) => fields_for_tab(self.field_tab()).get(i).copied(),
            _ => None,
        };

        match self.current_row() {
            Some(ContentRow::Field(field_idx)) => {
                if let Some(d) = field_desc {
                    match d.field_type {
                        FieldType::Bool => {
                            self.toggle(field_idx);
                            self.mark_dirty();
                        }
                        FieldType::Enum => self.open_enum_picker(field_idx),
                        FieldType::Number | FieldType::OptionalText | FieldType::OptionalNumber => {
                            self.start_edit();
                        }
                    }
                }
            }
            Some(ContentRow::ConnectionAdd) => self.add_connection(),
            Some(ContentRow::Connection(index)) => {
                if let Some(name) = self.connection_name(index) {
                    self.open_connection(name);
                }
            }
            Some(ContentRow::ConnectionButtons) => self.press_connection_button(),
            Some(ContentRow::LspAddServer) => {
                self.lsp_edit_fields = Default::default();
                self.lsp_edit_index = None;
                self.lsp_edit_cursor = 0;
                self.lsp_mode = LspMode::ServerEdit;
            }
            Some(ContentRow::LspServer(idx)) => {
                if idx < self.lsp_server_keys.len() {
                    let lang = self.lsp_server_keys[idx].clone();
                    if let Some(srv) = self.config.lsp.servers.get(&lang) {
                        self.lsp_edit_fields = [
                            lang,
                            srv.command.clone(),
                            srv.args.join(", "),
                            srv.root_markers.join(", "),
                        ];
                        self.lsp_edit_index = Some(idx);
                        self.lsp_edit_cursor = 0;
                        self.lsp_mode = LspMode::ServerEdit;
                    }
                }
            }
            _ => {}
        }
    }

    /// Open the dropdown for an enum field, highlighting its current value.
    pub(super) fn open_enum_picker(&mut self, field_index: usize) {
        let Some(options) = self.enum_options_for(field_index) else {
            return;
        };
        // With nothing matching, start at the top rather than nowhere.
        let cursor = options.current.unwrap_or(0);
        let scroll = cursor.saturating_sub(ENUM_PICKER_MAX_VISIBLE.saturating_sub(1));
        self.enum_picker = Some(EnumPicker {
            field_index,
            cursor,
            scroll,
            area: None,
        });
    }

    pub(super) fn close_enum_picker(&mut self) {
        self.enum_picker = None;
    }

    /// Store the highlighted choice and close.
    pub(super) fn commit_enum_picker(&mut self) {
        let Some(picker) = self.enum_picker.take() else {
            return;
        };
        let Some(options) = self.enum_options_for(picker.field_index) else {
            return;
        };
        if let Some(value) = options.values.get(picker.cursor) {
            let value = value.clone();
            // The model dropdown's last entry is the "type an id" escape: it
            // opens inline editing rather than storing a value.
            if value == crate::settings::fields::MODEL_TYPE_SENTINEL {
                self.start_edit();
                return;
            }
            self.apply_enum(picker.field_index, &value);
            self.mark_dirty();
        }
    }

    /// Scroll-wheel entry point for the dropdown highlight.
    pub(super) fn move_enum_cursor_public(&mut self, forward: bool) {
        self.move_enum_cursor(forward);
    }

    /// Move the highlight, keeping it inside the visible window.
    fn move_enum_cursor(&mut self, forward: bool) {
        let Some(picker) = self.enum_picker.as_ref() else {
            return;
        };
        let Some(options) = self.enum_options_for(picker.field_index) else {
            return;
        };
        let len = options.values.len();
        if len == 0 {
            return;
        }

        let Some(picker) = self.enum_picker.as_mut() else {
            return;
        };
        picker.cursor = if forward {
            (picker.cursor + 1) % len
        } else {
            (picker.cursor + len - 1) % len
        };

        let visible = ENUM_PICKER_MAX_VISIBLE.min(len);
        if picker.cursor < picker.scroll {
            picker.scroll = picker.cursor;
        } else if picker.cursor >= picker.scroll + visible {
            picker.scroll = picker.cursor + 1 - visible;
        }
    }

    /// Keys belonging to an open dropdown. Returns `true` when consumed.
    fn handle_enum_picker_key(&mut self, key: KeyEvent) -> bool {
        if self.enum_picker.is_none() {
            return false;
        }
        match key.code {
            KeyCode::Up => self.move_enum_cursor(false),
            KeyCode::Down => self.move_enum_cursor(true),
            KeyCode::Enter | KeyCode::Char(' ') => self.commit_enum_picker(),
            KeyCode::Esc => self.close_enum_picker(),
            // Anything else closes the list rather than falling through to the
            // form underneath, where it would edit a field the user cannot see.
            _ => self.close_enum_picker(),
        }
        true
    }

    pub(super) fn handle_content_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ModalResult<SettingsResult>>> {
        // LSP server edit form mode
        if self.active_tab == SettingsTab::Lsp && self.lsp_mode == LspMode::ServerEdit {
            return self.handle_lsp_edit_key(key);
        }
        // An open dropdown owns the keyboard until it closes.
        if self.handle_enum_picker_key(key) {
            return Ok(None);
        }

        let current = self.current_row();
        let field_desc = match current {
            Some(ContentRow::Field(i)) => fields_for_tab(self.field_tab()).get(i).copied(),
            _ => None,
        };

        match key.code {
            KeyCode::Up => {
                if !self.step_cursor(false) {
                    self.focus = FocusArea::Sidebar;
                }
            }
            KeyCode::Down => {
                if !self.step_cursor(true) {
                    self.selected_button = 0;
                    self.focus = FocusArea::Buttons;
                }
            }
            KeyCode::Tab => {
                self.focus = FocusArea::Buttons;
            }
            // The connection page goes back to the list it was opened from.
            KeyCode::Esc if self.connection_edit.is_some() => self.close_connection(),
            KeyCode::BackTab | KeyCode::Esc => {
                self.focus = FocusArea::Sidebar;
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(Some(ModalResult::Confirmed(SettingsResult::Apply(
                    Box::new(self.config.clone()),
                ))));
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.activate_current_row(),
            KeyCode::Delete if matches!(current, Some(ContentRow::Connection(_))) => {
                if let Some(ContentRow::Connection(index)) = current {
                    if let Some(name) = self.connection_name(index) {
                        self.delete_connection(&name);
                    }
                }
            }
            KeyCode::Delete => {
                if let Some(ContentRow::LspServer(idx)) = current {
                    if idx < self.lsp_server_keys.len() {
                        let lang = self.lsp_server_keys[idx].clone();
                        self.config.lsp.servers.remove(&lang);
                        self.refresh_server_keys();
                        self.mark_dirty();
                    }
                }
            }
            KeyCode::Left | KeyCode::Right if current == Some(ContentRow::ConnectionButtons) => {
                self.step_connection_button(key.code == KeyCode::Right);
            }
            KeyCode::Left => {
                if let (Some(ContentRow::Field(field_idx)), Some(d)) = (current, field_desc) {
                    if d.field_type == FieldType::Enum {
                        self.cycle(field_idx, false);
                        self.mark_dirty();
                    }
                }
            }
            KeyCode::Right => {
                if let (Some(ContentRow::Field(field_idx)), Some(d)) = (current, field_desc) {
                    if d.field_type == FieldType::Enum {
                        self.cycle(field_idx, true);
                        self.mark_dirty();
                    }
                }
            }
            _ => {}
        }
        Ok(None)
    }

    /// Toggle a switch of the tab the content shows.
    fn toggle(&mut self, index: usize) {
        match self.field_tab() {
            SettingsTab::Connection => self.toggle_connection_field(index),
            tab => toggle_field(&mut self.config, tab, index),
        }
    }

    /// Store a dropdown choice in a field of the tab the content shows.
    fn apply_enum(&mut self, index: usize, value: &str) {
        match self.field_tab() {
            SettingsTab::Connection => self.apply_connection_enum(index, value),
            tab => apply_enum_value(&mut self.config, tab, index, value),
        }
    }

    /// Step an enum field of the tab the content shows.
    fn cycle(&mut self, index: usize, forward: bool) {
        match self.field_tab() {
            SettingsTab::Connection => self.cycle_connection_field(index, forward),
            tab if forward => cycle_enum_forward(&mut self.config, tab, index),
            tab => cycle_enum_backward(&mut self.config, tab, index),
        }
    }

    /// Handle keys in LSP server edit form.
    fn handle_lsp_edit_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ModalResult<SettingsResult>>> {
        match key.code {
            KeyCode::Esc => {
                self.lsp_mode = LspMode::Fields;
            }
            KeyCode::Enter => {
                self.commit_lsp_edit();
                self.lsp_mode = LspMode::Fields;
            }
            KeyCode::Tab => {
                self.lsp_edit_cursor = (self.lsp_edit_cursor + 1) % 4;
            }
            KeyCode::BackTab => {
                self.lsp_edit_cursor = if self.lsp_edit_cursor == 0 {
                    3
                } else {
                    self.lsp_edit_cursor - 1
                };
            }
            KeyCode::Backspace => {
                self.lsp_edit_fields[self.lsp_edit_cursor].pop();
            }
            KeyCode::Char(c) => {
                self.lsp_edit_fields[self.lsp_edit_cursor].push(c);
            }
            _ => {}
        }
        Ok(None)
    }

    /// Commit the LSP server edit form.
    fn commit_lsp_edit(&mut self) {
        let lang = self.lsp_edit_fields[0].trim().to_string();
        if lang.is_empty() {
            return;
        }

        // If editing existing, remove old key if language changed
        if let Some(idx) = self.lsp_edit_index {
            if idx < self.lsp_server_keys.len() {
                let old_lang = self.lsp_server_keys[idx].clone();
                if old_lang != lang {
                    self.config.lsp.servers.remove(&old_lang);
                }
            }
        }

        let command = self.lsp_edit_fields[1].trim().to_string();
        let args: Vec<String> = if self.lsp_edit_fields[2].trim().is_empty() {
            vec![]
        } else {
            self.lsp_edit_fields[2]
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        let root_markers: Vec<String> = if self.lsp_edit_fields[3].trim().is_empty() {
            vec![]
        } else {
            self.lsp_edit_fields[3]
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };

        self.config.lsp.servers.insert(
            lang,
            LspServerSettings {
                command,
                args,
                root_markers,
            },
        );
        self.refresh_server_keys();
        self.mark_dirty();
    }

    pub(super) fn handle_edit_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ModalResult<SettingsResult>>> {
        match key.code {
            KeyCode::Enter => {
                self.commit_edit();
            }
            KeyCode::Esc => {
                self.cancel_edit();
            }
            KeyCode::Backspace => {
                self.edit_buffer.pop();
            }
            KeyCode::Char(c) => {
                if let Some(field_idx) = self.current_field_idx() {
                    let fields = fields_for_tab(self.field_tab());
                    if let Some(d) = fields.get(field_idx) {
                        if matches!(d.field_type, FieldType::Number | FieldType::OptionalNumber) {
                            if c.is_ascii_digit() {
                                self.edit_buffer.push(c);
                            }
                        } else {
                            self.edit_buffer.push(c);
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(None)
    }

    pub(super) fn handle_buttons_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ModalResult<SettingsResult>>> {
        match key.code {
            KeyCode::Left => {
                if self.selected_button > 0 {
                    self.selected_button -= 1;
                }
            }
            KeyCode::Right => {
                if self.selected_button < button_labels(self.project_override_active).len() - 1 {
                    self.selected_button += 1;
                }
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.field_cursor = self.last_selectable_row();
                self.focus = FocusArea::Content;
            }
            KeyCode::Down | KeyCode::Tab => {
                self.focus = FocusArea::Sidebar;
            }
            KeyCode::Enter => {
                return self.execute_selected_button();
            }
            KeyCode::Esc => {
                return Ok(Some(ModalResult::Cancelled));
            }
            _ => {}
        }
        Ok(None)
    }

    pub(super) fn execute_selected_button(
        &mut self,
    ) -> Result<Option<ModalResult<SettingsResult>>> {
        match self.selected_button {
            BUTTON_APPLY => Ok(Some(ModalResult::Confirmed(SettingsResult::Apply(
                Box::new(self.config.clone()),
            )))),
            BUTTON_RESET => {
                // Greyed out means inert: with nothing to reset, a click here
                // should not mark the modal dirty over an unchanged config.
                if !self.reset_available {
                    return Ok(None);
                }
                // `Config::default()` derives its keybindings, so they are
                // all unset there; without normalising, the Keybindings tab
                // would show an empty list until the modal is reopened.
                let mut defaults = Config::default();
                defaults.normalize();
                self.config = defaults;
                self.mark_dirty();
                // The connection the page had open is gone with the rest.
                self.connection_edit = None;
                self.enum_picker = None;
                self.field_cursor = 0;
                self.content_scroll = 0;
                self.editing = false;
                Ok(None)
            }
            BUTTON_PROJECT_OVERRIDE => {
                if self.project_override_active {
                    Ok(Some(ModalResult::Confirmed(
                        SettingsResult::RemoveProjectOverride,
                    )))
                } else {
                    Ok(Some(ModalResult::Confirmed(
                        SettingsResult::CreateProjectOverride(Box::new(self.config.clone())),
                    )))
                }
            }
            _ => Ok(Some(ModalResult::Cancelled)),
        }
    }

    /// Look up an existing binding string in `self.config` to warn the
    /// user about a same-section / cross-section clash before the new
    /// assignment overwrites it. The check is intentionally string-based
    /// (not parsed) so it matches what the user sees in the picker;
    /// canonicalization at parse time means logically-equivalent strings
    /// (`"Alt++"` vs `"Alt+Shift+="`) reach this function in the same
    /// canonical form because the picker always produces the canonical
    /// shape through `format_key_event`.
    fn find_conflict_for_binding(
        &self,
        new_binding: &str,
        new_section: &str,
        new_action: &str,
    ) -> Option<String> {
        for (loc, _, display) in termide_config::enumerate_bindings(&self.config) {
            if display != new_binding {
                continue;
            }
            if loc.section == new_section && loc.action == new_action {
                continue;
            }
            return Some(format!(
                "{} is also bound to {}.{}",
                new_binding, loc.section, loc.action
            ));
        }
        None
    }

    pub(super) fn handle_keybindings_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ModalResult<SettingsResult>>> {
        match self.kb_mode {
            KbMode::Bindings => {
                let names = kb_binding_names(self.kb_section);
                match key.code {
                    KeyCode::Up => {
                        if self.kb_cursor > 0 {
                            self.kb_cursor -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if self.kb_cursor < names.len().saturating_sub(1) {
                            self.kb_cursor += 1;
                        }
                    }
                    KeyCode::Enter => {
                        self.kb_mode = KbMode::Capturing;
                    }
                    // Delete peels off one alternative at a time, so a
                    // binding like `Alt+W, Alt+X, F10` can be trimmed instead
                    // of only wiped. Shift+Delete clears the action outright.
                    KeyCode::Delete | KeyCode::Backspace => {
                        if self.kb_cursor < names.len() {
                            let name = names[self.kb_cursor];
                            let clear_all = key.modifiers.contains(KeyModifiers::SHIFT)
                                || key.code == KeyCode::Backspace;
                            let current = get_kb_binding(&self.config, self.kb_section, name);
                            let next = match (clear_all, current) {
                                (false, Some(binding)) => binding.without_last_key(),
                                _ => None,
                            };
                            set_kb_value(
                                &mut self.config,
                                self.kb_section,
                                name,
                                next.unwrap_or(KeyBinding::Single(String::new())),
                            );
                            self.mark_dirty();
                        }
                    }
                    KeyCode::Esc | KeyCode::BackTab => {
                        self.focus = FocusArea::Sidebar;
                    }
                    KeyCode::Tab => {
                        self.focus = FocusArea::Buttons;
                    }
                    KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(Some(ModalResult::Confirmed(SettingsResult::Apply(
                            Box::new(self.config.clone()),
                        ))));
                    }
                    _ => {}
                }
            }
            KbMode::Capturing => {
                if key.code == KeyCode::Esc || key.code == KeyCode::Tab {
                    self.kb_mode = KbMode::Bindings;
                    self.kb_capture_message = None;
                    return Ok(None);
                }
                let binding_str = format_key_event(&key);
                if !binding_str.is_empty() {
                    let names = kb_binding_names(self.kb_section);
                    if self.kb_cursor < names.len() {
                        let action = names[self.kb_cursor];
                        let section_name = KB_SECTIONS
                            .get(self.kb_section)
                            .copied()
                            .unwrap_or("")
                            .to_lowercase();
                        // Pre-check for an existing binding so the user
                        // sees a warning and the conflict resolver gets a
                        // chance to inform them.
                        let conflict_msg =
                            self.find_conflict_for_binding(&binding_str, &section_name, action);
                        set_kb_value(
                            &mut self.config,
                            self.kb_section,
                            action,
                            KeyBinding::Single(binding_str),
                        );
                        self.mark_dirty();
                        self.kb_capture_message = conflict_msg;
                    }
                }
                self.kb_mode = KbMode::Bindings;
            }
        }
        Ok(None)
    }
}
