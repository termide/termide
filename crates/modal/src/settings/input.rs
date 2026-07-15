//! Settings modal input handling: per-focus-area key routing, LSP server
//! edit form, inline field editing, buttons, and keybinding capture.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termide_config::{Config, KeyBinding, LspServerSettings};

use crate::ModalResult;

use super::fields::{
    cycle_enum_backward, cycle_enum_forward, fields_for_tab, toggle_field, ContentRow, FieldType,
};
use super::kb::{format_key_event, kb_binding_names, set_kb_value, KB_SECTIONS};
use super::{
    button_labels, FocusArea, KbMode, LspMode, SettingsModal, SettingsResult, SettingsTab,
    SidebarRow, BUTTON_APPLY, BUTTON_PROJECT_OVERRIDE, BUTTON_RESET,
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

    pub(super) fn handle_content_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ModalResult<SettingsResult>>> {
        // LSP server edit form mode
        if self.active_tab == SettingsTab::Lsp && self.lsp_mode == LspMode::ServerEdit {
            return self.handle_lsp_edit_key(key);
        }

        let current = self.current_row();
        let field_desc = match current {
            Some(ContentRow::Field(i)) => fields_for_tab(self.active_tab).get(i).copied(),
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
            KeyCode::BackTab | KeyCode::Esc => {
                self.focus = FocusArea::Sidebar;
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Ok(Some(ModalResult::Confirmed(SettingsResult::Apply(
                    Box::new(self.config.clone()),
                ))));
            }
            KeyCode::Enter | KeyCode::Char(' ') => match current {
                Some(ContentRow::Field(field_idx)) => {
                    if let Some(d) = field_desc {
                        match d.field_type {
                            FieldType::Bool => {
                                toggle_field(&mut self.config, self.active_tab, field_idx);
                                self.dirty = true;
                            }
                            FieldType::Enum => {
                                cycle_enum_forward(&mut self.config, self.active_tab, field_idx);
                                self.dirty = true;
                            }
                            FieldType::Number | FieldType::OptionalText => {
                                self.start_edit();
                            }
                        }
                    }
                }
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
            },
            KeyCode::Delete => {
                if let Some(ContentRow::LspServer(idx)) = current {
                    if idx < self.lsp_server_keys.len() {
                        let lang = self.lsp_server_keys[idx].clone();
                        self.config.lsp.servers.remove(&lang);
                        self.refresh_server_keys();
                        self.dirty = true;
                    }
                }
            }
            KeyCode::Left => {
                if let (Some(ContentRow::Field(field_idx)), Some(d)) = (current, field_desc) {
                    if d.field_type == FieldType::Enum {
                        cycle_enum_backward(&mut self.config, self.active_tab, field_idx);
                        self.dirty = true;
                    }
                }
            }
            KeyCode::Right => {
                if let (Some(ContentRow::Field(field_idx)), Some(d)) = (current, field_desc) {
                    if d.field_type == FieldType::Enum {
                        cycle_enum_forward(&mut self.config, self.active_tab, field_idx);
                        self.dirty = true;
                    }
                }
            }
            _ => {}
        }
        Ok(None)
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
        self.dirty = true;
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
                    let fields = fields_for_tab(self.active_tab);
                    if let Some(d) = fields.get(field_idx) {
                        if d.field_type == FieldType::Number {
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
                self.config = Config::default();
                self.dirty = true;
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
                    KeyCode::Delete | KeyCode::Backspace => {
                        if self.kb_cursor < names.len() {
                            set_kb_value(
                                &mut self.config,
                                self.kb_section,
                                names[self.kb_cursor],
                                KeyBinding::Single(String::new()),
                            );
                            self.dirty = true;
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
                            KeyBinding::Single(binding_str.clone()),
                        );
                        self.dirty = true;
                        self.kb_capture_message = conflict_msg;
                    }
                }
                self.kb_mode = KbMode::Bindings;
            }
        }
        Ok(None)
    }
}
