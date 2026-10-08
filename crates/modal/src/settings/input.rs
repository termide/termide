//! Settings modal input handling: per-focus-area key routing, LSP server
//! edit form, inline field editing, buttons, and keybinding capture.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use termide_config::{Config, KeyBinding, LspServerSettings, ParsedKeyBinding};
use termide_i18n as i18n;

use crate::base::field_char_at;
use crate::ModalResult;

use super::fields::{
    apply_enum_value, cycle_enum_backward, cycle_enum_forward, fields_for_tab, toggle_field,
    ContentRow, FieldType,
};
use super::kb::{
    format_key_event, get_kb_binding, kb_binding_names, kb_keys_of, kb_section_key, set_kb_value,
};
use super::{
    button_labels, EnumPicker, FocusArea, KbCapture, KbMode, LspMode, SettingsModal,
    SettingsResult, SettingsTab, SidebarRow, BUTTON_APPLY, BUTTON_PROJECT_OVERRIDE, BUTTON_RESET,
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
                        ]
                        .map(termide_ui::TextInput::with_text);
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
            KeyCode::Esc | KeyCode::Backspace if self.connection_edit.is_some() => {
                self.close_connection();
            }
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
            _ => {
                termide_ui::edit_text_input(&mut self.lsp_edit_fields[self.lsp_edit_cursor], key);
            }
        }
        Ok(None)
    }

    /// Commit the LSP server edit form.
    fn commit_lsp_edit(&mut self) {
        let lang = self.lsp_edit_fields[0].text().trim().to_string();
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

        let command = self.lsp_edit_fields[1].text().trim().to_string();
        let args: Vec<String> = if self.lsp_edit_fields[2].text().trim().is_empty() {
            vec![]
        } else {
            self.lsp_edit_fields[2]
                .text()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        let root_markers: Vec<String> = if self.lsp_edit_fields[3].text().trim().is_empty() {
            vec![]
        } else {
            self.lsp_edit_fields[3]
                .text()
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
            KeyCode::Enter => self.commit_edit(),
            KeyCode::Esc => self.cancel_edit(),
            // A number field takes digits only, typed or pasted.
            KeyCode::Char(c)
                if self.editing_number()
                    && !c.is_ascii_digit()
                    && !key.modifiers.contains(KeyModifiers::CONTROL) => {}
            _ => {
                if termide_ui::edit_text_input(&mut self.edit_input, key)
                    == termide_ui::FieldEdit::Edited
                {
                    self.keep_digits();
                }
            }
        }
        Ok(None)
    }

    /// Paste into the field being edited.
    pub(super) fn paste_into_edit(&mut self, text: &str) -> bool {
        if self.active_tab == SettingsTab::Lsp && self.lsp_mode == LspMode::ServerEdit {
            self.lsp_edit_fields[self.lsp_edit_cursor].paste(text);
            return true;
        }
        if !self.editing {
            return false;
        }
        self.edit_input.paste(text);
        self.keep_digits();
        true
    }

    /// Presses and drags on a text field being edited: a press places the
    /// cursor, starting a selection that a drag extends, and a double click
    /// (its first press may be the one that opened the field) selects the
    /// whole text. Returns whether the event was the field's. A press
    /// elsewhere commits an inline edit first and goes on to whatever it
    /// landed on.
    pub(super) fn handle_field_mouse(&mut self, mouse: MouseEvent) -> bool {
        let point = (mouse.column, mouse.row).into();
        let lsp_form = self.active_tab == SettingsTab::Lsp && self.lsp_mode == LspMode::ServerEdit;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let presses = self.clicks.click(mouse.row);
                let hit = if lsp_form {
                    self.lsp_field_areas
                        .iter()
                        .position(|area| area.contains(point))
                        .map(|index| {
                            self.lsp_edit_cursor = index;
                            self.lsp_field_areas[index]
                        })
                } else if self.editing {
                    let area = self.edit_area.filter(|area| area.contains(point));
                    if area.is_none() {
                        self.commit_edit();
                    }
                    area
                } else {
                    None
                };
                let Some(area) = hit else {
                    return false;
                };
                self.focus = FocusArea::Content;
                let input = self.mouse_input();
                if presses >= 2 {
                    input.select_all();
                    self.field_drag = false;
                    return true;
                }
                let pos = field_char_at(input, area, mouse.column);
                input.set_cursor_with_selection_start(pos);
                self.field_drag = true;
                true
            }
            MouseEventKind::Drag(MouseButton::Left) if self.field_drag => {
                let area = if lsp_form {
                    self.lsp_field_areas.get(self.lsp_edit_cursor).copied()
                } else {
                    self.edit_area
                };
                if let Some(area) = area {
                    let input = self.mouse_input();
                    let pos = field_char_at(input, area, mouse.column);
                    input.extend_selection_to(pos);
                }
                true
            }
            MouseEventKind::Up(MouseButton::Left) if self.field_drag => {
                self.field_drag = false;
                true
            }
            _ => false,
        }
    }

    /// The text field a click or drag works on.
    fn mouse_input(&mut self) -> &mut termide_ui::TextInput {
        if self.active_tab == SettingsTab::Lsp && self.lsp_mode == LspMode::ServerEdit {
            &mut self.lsp_edit_fields[self.lsp_edit_cursor]
        } else {
            &mut self.edit_input
        }
    }

    /// Whether the field being edited holds a number.
    fn editing_number(&self) -> bool {
        self.current_field_idx()
            .and_then(|index| fields_for_tab(self.field_tab()).get(index).copied())
            .is_some_and(|d| matches!(d.field_type, FieldType::Number | FieldType::OptionalNumber))
    }

    /// Drop what is not a digit from a number field, as a paste can bring.
    fn keep_digits(&mut self) {
        if !self.editing_number() {
            return;
        }
        let text = self.edit_input.text();
        if text.chars().all(|c| c.is_ascii_digit()) {
            return;
        }
        let digits: String = text.chars().filter(char::is_ascii_digit).collect();
        self.edit_input.set_text(digits);
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

    /// Warn about a chord that another action already has, comparing parsed
    /// keys rather than strings.
    ///
    /// A string compare let logically-equivalent chords through as distinct:
    /// `"Alt++"` and `"Alt+Shift+="`, or `"Ctrl+Й"` and `"Ctrl+Q"`, name the
    /// same physical key yet differ as text. `parse_keybinding` canonicalizes
    /// both to one `ParsedKeyBinding`, so comparing those is both stricter
    /// and cheaper than the string it replaces.
    fn find_conflict_for_binding(
        &self,
        new_binding: &ParsedKeyBinding,
        new_section: &str,
        new_action: &str,
    ) -> Option<String> {
        for (loc, parsed, display) in termide_config::enumerate_bindings(&self.config) {
            if &parsed != new_binding {
                continue;
            }
            if loc.section == new_section && loc.action == new_action {
                continue;
            }
            return Some(i18n::t().settings_kb_conflict_fmt(&display, &loc.display()));
        }
        None
    }

    /// Whether this action already accepts `parsed`, under any of the spellings
    /// that canonicalize to it. Adding such a chord would only put a duplicate
    /// in the row, so the picker refuses it and says so.
    fn action_already_has_key(
        &self,
        section: usize,
        action: &str,
        parsed: &ParsedKeyBinding,
    ) -> bool {
        kb_keys_of(&self.config, section, action)
            .iter()
            .filter_map(|k| termide_config::parse_keybinding(k).ok())
            .any(|k| &k == parsed)
    }

    pub(super) fn handle_keybindings_key(
        &mut self,
        key: KeyEvent,
    ) -> Result<Option<ModalResult<SettingsResult>>> {
        match self.kb_mode {
            KbMode::Bindings => {
                let names = kb_binding_names(self.kb_section);
                if self.kb_cursor >= names.len() {
                    return Ok(None);
                }
                let name = names[self.kb_cursor];
                let keys = kb_keys_of(&self.config, self.kb_section, name);
                // The cursor runs over the bound keys plus one more slot: the
                // `+` at the end of the row, where a new alternative is added.
                let last = keys.len();
                match key.code {
                    KeyCode::Up => {
                        if self.kb_cursor > 0 {
                            self.kb_cursor -= 1;
                            self.kb_key_cursor = 0;
                        }
                    }
                    KeyCode::Down => {
                        if self.kb_cursor + 1 < names.len() {
                            self.kb_cursor += 1;
                            self.kb_key_cursor = 0;
                        }
                    }
                    // Left/Right walk the keys of this row. They are free here:
                    // the sidebar takes them for its own tree, and this tab has
                    // no enum or bool field to cycle.
                    KeyCode::Left => {
                        self.kb_key_cursor = self.kb_key_cursor.min(last).saturating_sub(1);
                    }
                    KeyCode::Right => {
                        self.kb_key_cursor = (self.kb_key_cursor.min(last) + 1).min(last);
                    }
                    KeyCode::Enter => {
                        self.kb_capture_message = None;
                        self.kb_mode = KbMode::Capturing(if self.kb_key_cursor >= last {
                            KbCapture::Append
                        } else {
                            KbCapture::Replace(self.kb_key_cursor)
                        });
                    }
                    // Delete takes the key the cursor is on. Shift+Delete or
                    // Backspace clears the action outright, as before.
                    KeyCode::Delete if key.modifiers.contains(KeyModifiers::SHIFT) => {
                        self.set_kb_keys(name, None);
                    }
                    KeyCode::Delete | KeyCode::Backspace => {
                        if self.kb_key_cursor < last {
                            let binding = get_kb_binding(&self.config, self.kb_section, name);
                            let next = binding.and_then(|b| b.without_key_at(self.kb_key_cursor));
                            self.set_kb_keys(name, next);
                            // Stay on a key: the one that slid into this slot is
                            // the natural next thing to act on, and when none
                            // slid in, the last remaining one. Landing on the
                            // `+` slot instead would make the next Delete do
                            // nothing, so clearing a row by hand stalled.
                            let remaining = kb_keys_of(&self.config, self.kb_section, name).len();
                            self.kb_key_cursor =
                                self.kb_key_cursor.min(remaining.saturating_sub(1));
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
            KbMode::Capturing(capture) => {
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
                        self.apply_captured_key(capture, action, &binding_str);
                    }
                }
                self.kb_mode = KbMode::Bindings;
            }
        }
        Ok(None)
    }

    /// Write a captured chord into the action under the cursor.
    ///
    /// A chord the action already accepts is refused rather than stored: a
    /// second spelling of a key it already answers to would only clutter the
    /// row. A chord another action has is stored with a warning, as before —
    /// which of the two should win is the user's call, not ours.
    fn apply_captured_key(&mut self, capture: KbCapture, action: &str, binding_str: &str) {
        let parsed = match termide_config::parse_keybinding(binding_str) {
            Ok(parsed) => parsed,
            Err(_) => return,
        };
        if self.action_already_has_key(self.kb_section, action, &parsed) {
            self.kb_capture_message = Some(i18n::t().settings_kb_key_taken_fmt(binding_str));
            return;
        }

        let current = get_kb_binding(&self.config, self.kb_section, action);
        let next = match capture {
            KbCapture::Append => current
                .map(|b| b.push_key(binding_str))
                .unwrap_or_else(|| KeyBinding::Single(binding_str.to_string())),
            KbCapture::Replace(index) => {
                let mut keys = kb_keys_of(&self.config, self.kb_section, action);
                if index >= keys.len() {
                    keys.push(binding_str.to_string());
                } else {
                    keys[index] = binding_str.to_string();
                }
                if keys.len() == 1 {
                    KeyBinding::Single(keys.remove(0))
                } else {
                    KeyBinding::Multiple(keys)
                }
            }
        };
        self.set_kb_keys(action, Some(next));

        // Keep the cursor on the key just written, so a second `Enter` edits
        // the same one instead of jumping to the end of the row.
        self.kb_key_cursor = match capture {
            KbCapture::Append => kb_keys_of(&self.config, self.kb_section, action)
                .len()
                .saturating_sub(1),
            KbCapture::Replace(index) => index,
        };

        let section_name = kb_section_key(self.kb_section);
        self.kb_capture_message = self.find_conflict_for_binding(&parsed, section_name, action);
    }

    /// Store `binding` for the row under the cursor, clearing it when `None`.
    fn set_kb_keys(&mut self, action: &str, binding: Option<KeyBinding>) {
        set_kb_value(
            &mut self.config,
            self.kb_section,
            action,
            binding.unwrap_or_else(|| KeyBinding::Single(String::new())),
        );
        self.mark_dirty();
    }
}

#[cfg(test)]
mod keybinding_picker_tests {
    use super::*;
    use crate::Modal;
    use crossterm::event::KeyEvent;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// Global section, `close_panel` row — which ships bound to three keys.
    fn kb_modal() -> SettingsModal {
        let mut config = Config::default();
        config.normalize();
        let mut modal = SettingsModal::new(config, false);
        modal.active_tab = SettingsTab::Keybindings;
        modal.focus = FocusArea::Content;
        modal.kb_section = 0;
        modal.kb_cursor = kb_binding_names(0)
            .iter()
            .position(|n| *n == "close_panel")
            .expect("close_panel is listed");
        modal
    }

    fn press(modal: &mut SettingsModal, code: KeyCode, modifiers: KeyModifiers) {
        let chord = termide_core::KeyChord::identity(KeyEvent::new(code, modifiers));
        modal.handle_key(chord).unwrap();
    }

    fn keys(modal: &SettingsModal) -> Vec<String> {
        kb_keys_of(&modal.config, 0, "close_panel")
    }

    /// The row starts with the cursor on its first key.
    #[test]
    fn arrows_walk_the_keys_of_the_row() {
        let mut modal = kb_modal();
        assert_eq!(keys(&modal), vec!["Alt+W", "Alt+X", "F10"]);
        assert_eq!(modal.kb_key_cursor, 0);

        press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(modal.kb_key_cursor, 1);
        press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(modal.kb_key_cursor, 2);
        // The next stop past the last key is the `+` slot.
        press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(modal.kb_key_cursor, 3);
        press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(modal.kb_key_cursor, 3, "the row has no further slot");

        press(&mut modal, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(modal.kb_key_cursor, 2);
    }

    /// Regression: `Enter` used to write a `Single`, so rebinding one key of
    /// `Alt+W, Alt+X, F10` silently dropped the other two.
    #[test]
    fn enter_replaces_only_the_key_under_the_cursor() {
        let mut modal = kb_modal();
        press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(modal.kb_mode, KbMode::Capturing(KbCapture::Replace(1)));

        press(&mut modal, KeyCode::F(7), KeyModifiers::CONTROL);
        assert_eq!(keys(&modal), vec!["Alt+W", "Ctrl+F7", "F10"]);
        assert_eq!(
            modal.kb_key_cursor, 1,
            "the cursor stays on what was written"
        );
    }

    /// `Enter` on the `+` slot appends instead of replacing.
    #[test]
    fn the_plus_slot_adds_another_alternative() {
        let mut modal = kb_modal();
        for _ in 0..3 {
            press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        }
        assert_eq!(modal.kb_key_cursor, 3, "the cursor is on the + slot");
        press(&mut modal, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(modal.kb_mode, KbMode::Capturing(KbCapture::Append));

        press(&mut modal, KeyCode::F(3), KeyModifiers::ALT);
        assert_eq!(keys(&modal), vec!["Alt+W", "Alt+X", "F10", "Alt+F3"]);
    }

    /// Adding a chord the action already answers to would only put a duplicate
    /// in the row, so it is refused and said out loud.
    #[test]
    fn a_key_the_action_already_has_is_refused() {
        let mut modal = kb_modal();
        for _ in 0..3 {
            press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        }
        press(&mut modal, KeyCode::Enter, KeyModifiers::NONE);
        // `Alt+W` is already the first alternative; a different spelling of the
        // same physical chord is refused just the same.
        press(&mut modal, KeyCode::Char('w'), KeyModifiers::ALT);

        assert_eq!(
            keys(&modal),
            vec!["Alt+W", "Alt+X", "F10"],
            "the duplicate was not stored"
        );
        assert!(
            modal
                .kb_capture_message
                .as_deref()
                .is_some_and(|m| m.contains("Alt+W")),
            "the user is told why: {:?}",
            modal.kb_capture_message
        );
    }

    /// `"Alt+Shift+="` and `"Alt++"` are one physical chord. A string compare
    /// let the second spelling through as if it were new.
    #[test]
    fn a_equivalent_spelling_of_an_existing_key_is_refused_too() {
        let mut modal = kb_modal();
        modal.config.general.keybindings.close_panel =
            Some(KeyBinding::Single("Alt+Shift+=".to_string()));
        for _ in 0..2 {
            press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        }
        press(&mut modal, KeyCode::Enter, KeyModifiers::NONE);
        // The picker produces `Alt++` for that chord; it canonicalizes to the
        // same parsed binding the row already holds.
        let captured = format_key_event(&KeyEvent::new(
            KeyCode::Char('+'),
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ));
        press(
            &mut modal,
            KeyCode::Char('+'),
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        );

        assert_eq!(
            kb_keys_of(&modal.config, 0, "close_panel"),
            vec!["Alt+Shift+="],
            "{captured} names the key already there"
        );
    }

    /// Delete takes the key the cursor is on, not always the last one.
    #[test]
    fn delete_removes_the_selected_key() {
        let mut modal = kb_modal();
        press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(keys(&modal), vec!["Alt+W", "F10"]);

        press(&mut modal, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(keys(&modal), vec!["Alt+W"], "the next key slid into place");

        press(&mut modal, KeyCode::Delete, KeyModifiers::NONE);
        assert!(keys(&modal).is_empty(), "the last one clears the row");
    }

    /// Shift+Delete still clears the whole action, as it did before.
    #[test]
    fn shift_delete_clears_the_action() {
        let mut modal = kb_modal();
        press(&mut modal, KeyCode::Delete, KeyModifiers::SHIFT);
        assert!(keys(&modal).is_empty());
    }

    /// `Esc` leaves the binding untouched.
    #[test]
    fn escape_abandons_the_capture() {
        let mut modal = kb_modal();
        press(&mut modal, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(modal.kb_mode, KbMode::Bindings);
        assert_eq!(keys(&modal), vec!["Alt+W", "Alt+X", "F10"]);
    }

    /// A chord another action holds is stored — which should win is the
    /// user's call — but with a warning naming where it is also bound.
    #[test]
    fn a_chord_held_elsewhere_is_stored_with_a_warning() {
        let mut modal = kb_modal();
        for _ in 0..3 {
            press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        }
        press(&mut modal, KeyCode::Enter, KeyModifiers::NONE);
        // `Alt+Q` is `quit`'s binding in the same section.
        press(&mut modal, KeyCode::Char('q'), KeyModifiers::ALT);

        assert!(keys(&modal).contains(&"Alt+Q".to_string()));
        let msg = modal.kb_capture_message.clone().unwrap_or_default();
        assert!(msg.contains("Alt+Q"), "{msg}");
        assert!(msg.contains("quit"), "{msg} names the other action");
    }

    /// Regression: rebinding a *global* action used to warn about itself. The
    /// picker compared the sidebar's `Global` against the `general` the conflict
    /// list reports, so the "skip my own binding" test never matched and the
    /// chord just written into this very action came back as a clash.
    #[test]
    fn rebinding_a_global_action_does_not_report_itself() {
        let mut modal = kb_modal();
        // `Ctrl+F4` is bound to nothing in the default config.
        for _ in 0..3 {
            press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        }
        press(&mut modal, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut modal, KeyCode::F(4), KeyModifiers::CONTROL);

        assert!(
            kb_keys_of(&modal.config, 0, "close_panel").contains(&"Ctrl+F4".to_string()),
            "the chord was stored"
        );
        assert_eq!(
            modal.kb_capture_message, None,
            "a free chord on a global action is no conflict"
        );
    }

    /// Moving between rows restarts the key cursor: a row's own first key is
    /// the thing to act on, not wherever the last row happened to be.
    #[test]
    fn changing_row_resets_the_key_cursor() {
        let mut modal = kb_modal();
        press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(modal.kb_key_cursor, 2);

        press(&mut modal, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(modal.kb_key_cursor, 0);
    }

    /// A row with no keys still takes a first binding: the cursor sits on the
    /// `+` slot, and `Enter` captures.
    #[test]
    fn an_unbound_row_binds_from_the_plus_slot() {
        let mut modal = kb_modal();
        modal.config.general.keybindings.close_panel = Some(KeyBinding::Single(String::new()));
        assert_eq!(keys(&modal), Vec::<String>::new());

        press(&mut modal, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(modal.kb_mode, KbMode::Capturing(KbCapture::Append));
        press(&mut modal, KeyCode::F(9), KeyModifiers::NONE);
        assert_eq!(keys(&modal), vec!["F9"]);
    }

    /// The row shows every alternative, with the `+` slot after them.
    #[test]
    fn the_row_draws_every_key_and_the_slot() {
        let mut modal = kb_modal();
        let area = Rect::new(0, 0, 110, 40);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &termide_theme::Theme::default());

        let row = kb_binding_names(0)
            .iter()
            .position(|n| *n == "close_panel")
            .unwrap();
        let slots = modal
            .kb_rows
            .iter()
            .find(|(i, _)| *i == row)
            .map(|(_, s)| s.clone())
            .expect("the row recorded its slots");
        assert_eq!(
            slots.iter().map(|s| s.key).collect::<Vec<_>>(),
            vec![Some(0), Some(1), Some(2), None],
            "three keys then the + slot"
        );

        // The line those slots were drawn on, read back out of the buffer:
        // found by its label rather than by recomputing the row's y.
        let (y, line) = (0..area.height)
            .map(|y| {
                let line: String = (0..area.width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect();
                (y, line)
            })
            .find(|(_, l)| l.contains("close_panel"))
            .expect("the close_panel row is drawn");
        assert!(
            line.contains("Alt+W, Alt+X, F10"),
            "all three alternatives, comma-separated: {line:?}"
        );

        // Each slot holds what its span says it holds.
        for slot in &slots {
            let text: String = (slot.span.0..slot.span.1)
                .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                .collect();
            let expected = match slot.key {
                Some(i) => ["Alt+W", "Alt+X", "F10"][i],
                None => crate::settings::render::KB_ADD_BUTTON,
            };
            assert_eq!(text, expected, "slot {slot:?}");
        }
    }

    /// A click on a key selects it, so the next Delete takes that one.
    #[test]
    fn clicking_a_key_selects_it() {
        let mut modal = kb_modal();
        let area = Rect::new(0, 0, 110, 40);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &termide_theme::Theme::default());

        let row = kb_binding_names(0)
            .iter()
            .position(|n| *n == "close_panel")
            .unwrap();
        let slots = modal
            .kb_rows
            .iter()
            .find(|(i, _)| *i == row)
            .map(|(_, s)| s.clone())
            .expect("slots recorded");
        let key = slots[1];
        modal.kb_click(row, (key.span.0 + key.span.1) / 2);
        assert_eq!(modal.kb_key_cursor, 1);

        press(&mut modal, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(keys(&modal), vec!["Alt+W", "F10"]);
    }

    /// A click on the `+` starts capturing right away.
    #[test]
    fn clicking_the_plus_starts_a_capture() {
        let mut modal = kb_modal();
        let area = Rect::new(0, 0, 110, 40);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &termide_theme::Theme::default());

        let row = kb_binding_names(0)
            .iter()
            .position(|n| *n == "close_panel")
            .unwrap();
        let add = modal
            .kb_rows
            .iter()
            .find(|(i, _)| *i == row)
            .and_then(|(_, slots)| slots.iter().find(|s| s.key.is_none()))
            .copied()
            .expect("the + slot is drawn");
        modal.kb_click(row, add.span.0);
        assert_eq!(modal.kb_mode, KbMode::Capturing(KbCapture::Append));

        press(&mut modal, KeyCode::F(2), KeyModifiers::ALT);
        assert_eq!(keys(&modal), vec!["Alt+W", "Alt+X", "F10", "Alt+F2"]);
    }

    /// Walking right past the last key lands on the `[+]` button, which is
    /// then inverted like a selected key; before that it is not.
    #[test]
    fn the_plus_button_is_inverted_under_the_cursor() {
        let mut modal = kb_modal();
        let area = Rect::new(0, 0, 110, 40);
        let row = kb_binding_names(0)
            .iter()
            .position(|n| *n == "close_panel")
            .unwrap();
        let add_modifiers = |modal: &mut SettingsModal| {
            let mut buf = Buffer::empty(area);
            modal.render(area, &mut buf, &termide_theme::Theme::default());
            let (y, add) = modal
                .kb_rows
                .iter()
                .find(|(i, _)| *i == row)
                .and_then(|(_, slots)| slots.iter().find(|s| s.key.is_none()))
                .map(|s| {
                    let y = (0..area.height)
                        .find(|&y| buf[(s.span.0, y)].symbol() == "[")
                        .expect("the [+] is drawn");
                    (y, *s)
                })
                .expect("the [+] slot is drawn");
            (add.span.0..add.span.1)
                .map(|x| buf[(x, y)].modifier)
                .collect::<Vec<_>>()
        };

        assert!(add_modifiers(&mut modal)
            .iter()
            .all(|m| !m.contains(ratatui::style::Modifier::REVERSED)));
        for _ in 0..3 {
            press(&mut modal, KeyCode::Right, KeyModifiers::NONE);
        }
        assert!(add_modifiers(&mut modal)
            .iter()
            .all(|m| m.contains(ratatui::style::Modifier::REVERSED)));
    }

    /// The `+` is drawn on the focused row only, so a click at its column on
    /// another row selects that row rather than adding to it.
    #[test]
    fn the_plus_slot_does_not_leak_across_rows() {
        let mut modal = kb_modal();
        let area = Rect::new(0, 0, 110, 40);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &termide_theme::Theme::default());

        let row = kb_binding_names(0)
            .iter()
            .position(|n| *n == "close_panel")
            .unwrap();
        let add = modal
            .kb_rows
            .iter()
            .find(|(i, _)| *i == row)
            .and_then(|(_, slots)| slots.iter().find(|s| s.key.is_none()))
            .copied()
            .expect("the + slot is drawn");

        let other = row + 1;
        modal.kb_click(other, add.span.0);
        assert_eq!(modal.kb_cursor, other);
        assert_eq!(
            modal.kb_mode,
            KbMode::Bindings,
            "a click at the + column of a different row only selects it"
        );
    }
}

#[cfg(test)]
mod keybinding_double_click_tests {
    use super::*;
    use crate::settings::{KbSlot, SettingsTab};
    use crate::Modal;
    use crossterm::event::KeyEvent;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn kb_modal() -> (SettingsModal, Vec<KbSlot>) {
        let mut config = Config::default();
        config.normalize();
        let mut modal = SettingsModal::new(config, false);
        modal.active_tab = SettingsTab::Keybindings;
        modal.focus = FocusArea::Content;
        modal.kb_section = 0;
        modal.kb_cursor = kb_binding_names(0)
            .iter()
            .position(|n| *n == "close_panel")
            .unwrap();
        let area = Rect::new(0, 0, 110, 40);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &termide_theme::Theme::default());
        let row = modal.kb_cursor;
        let slots = modal
            .kb_rows
            .iter()
            .find(|(i, _)| *i == row)
            .map(|(_, s)| s.clone())
            .expect("slots recorded");
        (modal, slots)
    }

    /// A second click on the same key captures it.
    #[test]
    fn a_double_click_on_one_key_captures_it() {
        let (mut modal, slots) = kb_modal();
        let row = modal.kb_cursor;
        let key = slots[1];
        let at = (key.span.0 + key.span.1) / 2;

        modal.kb_click(row, at);
        assert_eq!(modal.kb_mode, KbMode::Bindings, "the first click selects");
        modal.kb_click(row, at);
        assert_eq!(modal.kb_mode, KbMode::Capturing(KbCapture::Replace(1)));
    }

    /// Two quick clicks on *different* keys of one row are not a double click
    /// on either: the tracker is keyed by slot, not by row.
    #[test]
    fn quick_clicks_on_different_keys_are_not_a_double_click() {
        let (mut modal, slots) = kb_modal();
        let row = modal.kb_cursor;

        modal.kb_click(row, (slots[0].span.0 + slots[0].span.1) / 2);
        modal.kb_click(row, (slots[1].span.0 + slots[1].span.1) / 2);

        assert_eq!(
            modal.kb_mode,
            KbMode::Bindings,
            "moving across the row must not start a capture"
        );
        assert_eq!(modal.kb_key_cursor, 1);
    }

    /// The captured key lands on the key that was double-clicked.
    #[test]
    fn a_double_click_then_a_key_replaces_that_key() {
        let (mut modal, slots) = kb_modal();
        let row = modal.kb_cursor;
        let at = (slots[2].span.0 + slots[2].span.1) / 2;

        modal.kb_click(row, at);
        modal.kb_click(row, at);
        let chord =
            termide_core::KeyChord::identity(KeyEvent::new(KeyCode::F(8), KeyModifiers::CONTROL));
        modal.handle_key(chord).unwrap();

        assert_eq!(
            kb_keys_of(&modal.config, 0, "close_panel"),
            vec!["Alt+W", "Alt+X", "Ctrl+F8"]
        );
    }
}

#[cfg(test)]
mod wheel_tests {
    use super::*;
    use crate::settings::SettingsTab;
    use crate::Modal;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn rendered(tab: SettingsTab) -> (SettingsModal, Rect) {
        let mut config = Config::default();
        config.normalize();
        let mut modal = SettingsModal::new(config, false);
        modal.active_tab = tab;
        modal.field_cursor = modal.first_selectable_row();
        // Short enough that the hotkey list does not fit.
        let screen = Rect::new(0, 0, 110, 20);
        let mut buf = Buffer::empty(screen);
        modal.render(screen, &mut buf, &termide_theme::Theme::default());
        (modal, screen)
    }

    fn wheel(modal: &mut SettingsModal, screen: Rect, at: Rect, down: bool) {
        let kind = if down {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        };
        let event = MouseEvent {
            kind,
            column: at.x + 1,
            row: at.y,
            modifiers: KeyModifiers::NONE,
        };
        modal.handle_mouse(event, screen).unwrap();
        let mut buf = Buffer::empty(screen);
        modal.render(screen, &mut buf, &termide_theme::Theme::default());
    }

    #[test]
    fn the_wheel_scrolls_the_hotkey_list() {
        let (mut modal, screen) = rendered(SettingsTab::Keybindings);
        let content = modal.last_content_area.expect("content drawn");
        assert!(kb_binding_names(modal.kb_section).len() > content.height as usize);

        for _ in 0..20 {
            wheel(&mut modal, screen, content, true);
        }
        assert!(modal.kb_scroll > 0, "the list scrolled down");
        assert!(modal.kb_cursor < kb_binding_names(modal.kb_section).len());

        for _ in 0..20 {
            wheel(&mut modal, screen, content, false);
        }
        assert_eq!(modal.kb_scroll, 0, "and back up");
        assert_eq!(modal.kb_cursor, 0);
    }

    #[test]
    fn the_wheel_scrolls_a_field_list() {
        let (mut modal, screen) = rendered(SettingsTab::General);
        let content = modal.last_content_area.expect("content drawn");
        let start = modal.field_cursor;
        wheel(&mut modal, screen, content, true);
        assert!(modal.field_cursor > start, "the cursor moved down");
        assert!(modal.current_row().is_some_and(|row| row.is_selectable()));
    }

    #[test]
    fn the_wheel_over_the_sidebar_leaves_the_section_alone() {
        let (mut modal, screen) = rendered(SettingsTab::General);
        let sidebar = modal.last_sidebar_area.expect("sidebar drawn");
        wheel(&mut modal, screen, sidebar, true);
        assert_eq!(modal.active_tab, SettingsTab::General);
    }
}
