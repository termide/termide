//! Settings modal state model: construction, sizing, sidebar/content-row
//! navigation, scroll clamping, and inline-edit commit logic.

use ratatui::layout::Rect;
use termide_config::Config;
use termide_i18n as i18n;

use super::connection;
use super::fields::{fields_for_tab, get_field_value, ContentRow, FieldType};
use super::kb::KB_SECTIONS;
use super::{
    FocusArea, KbMode, LspMode, SettingsModal, SettingsTab, SidebarRow, BUTTON_APPLY,
    TOP_LEVEL_TABS,
};

impl SettingsModal {
    /// Build the modal. `project_override_active` reflects the existence
    /// of `<project>/.termide/config.toml` at modal-open time and is used
    /// purely for the third button's label and routing — the modal
    /// itself never touches the filesystem.
    pub fn new(config: Config, project_override_active: bool) -> Self {
        let lsp_server_keys = Self::sorted_server_keys(&config);
        let mut m = Self {
            config,
            active_tab: SettingsTab::General,
            sidebar_cursor: 0,
            sidebar_scroll: 0,
            // Expanded from the start: the group holds nine sections, and
            // leaving it collapsed hides every keybinding behind a step users
            // have no reason to expect.
            keybindings_expanded: true,
            focus: FocusArea::Sidebar,
            field_cursor: 0,
            content_scroll: 0,
            editing: false,
            edit_input: termide_ui::TextInput::new(),
            edit_area: None,
            field_drag: false,
            clicks: termide_ui::ClickTracker::new(),
            reset_available: false,
            enum_picker: None,
            lsp_mode: LspMode::Fields,
            lsp_edit_index: None,
            lsp_server_keys,
            lsp_edit_fields: Default::default(),
            lsp_edit_cursor: 0,
            lsp_field_areas: Vec::new(),
            connection_edit: None,
            model_fetch_request: None,
            kb_mode: KbMode::Bindings,
            kb_section: 0,
            kb_cursor: 0,
            kb_scroll: 0,
            kb_capture_message: None,
            selected_button: BUTTON_APPLY,
            dirty: false,
            project_override_active,
            model_options: Vec::new(),
            last_modal_area: None,
            last_sidebar_area: None,
            last_content_area: None,
            last_buttons_area: None,
        };
        m.field_cursor = m.first_selectable_row();
        m.refresh_reset_available();
        m
    }

    /// Provide the open connection's model list (fetched off-thread by the
    /// app), so the model field's dropdown lists them.
    pub fn set_model_options(&mut self, models: Vec<String>) {
        self.model_options = models;
    }

    /// The dropdown options for a field of the tab the content shows.
    pub(super) fn enum_options_for(
        &self,
        field_index: usize,
    ) -> Option<crate::settings::fields::EnumOptions> {
        match self.field_tab() {
            SettingsTab::Connection => self.connection_enum_options(field_index),
            tab => crate::settings::fields::enum_options(&self.config, tab, field_index),
        }
    }

    /// A field's value, as its row shows it, in the tab the content shows.
    pub(super) fn field_value(&self, index: usize) -> String {
        match self.field_tab() {
            SettingsTab::Connection => self.connection_value(index),
            tab => get_field_value(&self.config, tab, index),
        }
    }

    fn sorted_server_keys(config: &Config) -> Vec<String> {
        let mut keys: Vec<String> = config.lsp.servers.keys().cloned().collect();
        keys.sort();
        keys
    }

    pub(super) fn refresh_server_keys(&mut self) {
        self.lsp_server_keys = Self::sorted_server_keys(&self.config);
    }

    // ---- Sizing ----

    pub(super) fn calculate_size(screen: Rect) -> Rect {
        let w = ((screen.width as usize * 90) / 100).clamp(80, 140);
        let h = ((screen.height as usize * 85) / 100).clamp(20, 50);
        let w = w.min(screen.width as usize).max(60);
        let h = h.min(screen.height as usize).max(16);
        let x = (screen.width as usize).saturating_sub(w) / 2;
        let y = (screen.height as usize).saturating_sub(h) / 2;
        Rect::new(x as u16, y as u16, w as u16, h as u16)
    }

    // ---- Sidebar helpers ----

    /// Build the visible sidebar rows (respects `keybindings_expanded`).
    pub(super) fn visible_sidebar_rows(&self) -> Vec<SidebarRow> {
        let mut rows: Vec<SidebarRow> = TOP_LEVEL_TABS
            .iter()
            .map(|&t| SidebarRow::Leaf(t))
            .collect();
        rows.push(SidebarRow::KbGroupHeader);
        if self.keybindings_expanded {
            for i in 0..KB_SECTIONS.len() {
                rows.push(SidebarRow::KbChild(i));
            }
        }
        rows
    }

    /// Find the sidebar cursor index matching the current `active_tab` / `kb_section`.
    pub(super) fn sidebar_cursor_for_active(&self) -> usize {
        let rows = self.visible_sidebar_rows();
        for (i, row) in rows.iter().enumerate() {
            match *row {
                SidebarRow::Leaf(tab) if tab == self.active_tab => return i,
                SidebarRow::KbChild(idx)
                    if self.active_tab == SettingsTab::Keybindings && idx == self.kb_section =>
                {
                    return i;
                }
                SidebarRow::KbGroupHeader
                    if self.active_tab == SettingsTab::Keybindings
                        && !self.keybindings_expanded =>
                {
                    return i;
                }
                _ => {}
            }
        }
        0
    }

    /// Update `active_tab` etc to match the row under the cursor, WITHOUT toggling group
    /// expansion (used by arrow/tab navigation).
    pub(super) fn preview_sidebar_row(&mut self, row: SidebarRow) {
        match row {
            SidebarRow::Leaf(tab) => {
                self.active_tab = tab;
                self.connection_edit = None;
                self.content_scroll = 0;
                self.editing = false;
                self.field_cursor = self.first_selectable_row();
            }
            SidebarRow::KbGroupHeader => {
                // No-op on navigation: keep whatever was active.
            }
            SidebarRow::KbChild(idx) => {
                self.active_tab = SettingsTab::Keybindings;
                self.kb_section = idx;
                self.kb_mode = KbMode::Bindings;
                self.kb_cursor = 0;
                self.kb_scroll = 0;
                self.editing = false;
            }
        }
    }

    /// Activate a row explicitly (Enter / mouse click). Toggles group header,
    /// otherwise behaves like `preview_sidebar_row`.
    pub(super) fn activate_sidebar_row(&mut self, row: SidebarRow) {
        match row {
            SidebarRow::KbGroupHeader => {
                self.keybindings_expanded = !self.keybindings_expanded;
                if self.keybindings_expanded {
                    self.active_tab = SettingsTab::Keybindings;
                    self.kb_mode = KbMode::Bindings;
                    self.kb_cursor = 0;
                    self.kb_scroll = 0;
                }
            }
            other => self.preview_sidebar_row(other),
        }
    }

    /// Recompute whether "Reset to Defaults" has anything to reset.
    ///
    /// Kept separate from `dirty`, which tracks unsaved edits: a config saved
    /// long ago still differs from the defaults, and the button greying out in
    /// that state is what made it look broken.
    pub(super) fn refresh_reset_available(&mut self) {
        self.reset_available = self.config.differs_from_defaults();
    }

    /// Record a config change: marks the modal dirty and refreshes what the
    /// reset button is allowed to do.
    pub(super) fn mark_dirty(&mut self) {
        self.dirty = true;
        self.refresh_reset_available();
    }

    /// Clamp sidebar scroll so `sidebar_cursor` is visible.
    pub(super) fn clamp_sidebar_scroll(&mut self, visible: usize) {
        if self.sidebar_cursor < self.sidebar_scroll {
            self.sidebar_scroll = self.sidebar_cursor;
        }
        if visible > 0 && self.sidebar_cursor >= self.sidebar_scroll + visible {
            self.sidebar_scroll = self.sidebar_cursor - visible + 1;
        }
    }

    /// Localized label for the Keybindings group header (same as the tab label).
    pub(super) fn kb_group_label() -> String {
        i18n::t().settings_tab_keybindings().to_string()
    }

    /// Localized label for a Keybindings subsection (index into `KB_SECTIONS`).
    /// `KB_SECTIONS` itself stays the stable identifier the config uses; this
    /// is only for display.
    pub(super) fn kb_section_label(section: usize) -> String {
        let t = i18n::t();
        match section {
            0 => t.settings_kb_global(),
            1 => t.settings_kb_editor(),
            2 => t.settings_kb_file_manager(),
            3 => t.settings_kb_git_status(),
            4 => t.settings_kb_git_diff(),
            5 => t.settings_kb_git_log(),
            6 => t.settings_kb_terminal(),
            7 => t.settings_kb_database(),
            8 => t.settings_kb_viewer(),
            _ => "",
        }
        .to_string()
    }

    // ---- Content-row helpers ----

    /// Build the list of rows rendered in the content area for the active tab.
    /// Field indices reference `fields_for_tab(self.active_tab)`.
    pub(super) fn content_rows(&self) -> Vec<ContentRow> {
        use ContentRow::*;
        match self.field_tab() {
            SettingsTab::General => vec![
                Header("Appearance"),
                Field(1), // theme
                Field(2), // language
                Field(3), // icon_mode
                Spacer,
                Header("Input"),
                Field(0), // vim_mode
                Spacer,
                Header("Layout"),
                Field(4), // auto_stack_threshold
                Field(5), // min_panel_width
                Spacer,
                Header("Notifications"),
                Field(7), // bell
                Spacer,
                Header("Performance"),
                Field(6), // session_retention
                Field(8), // resource_monitor_interval
                Spacer,
                Header("Session"),
                Field(9), // always_detachable
            ],
            SettingsTab::Editor => vec![
                Header("Typing"),
                Field(0), // tab_size
                Field(2), // auto_indent
                Field(3), // auto_close_brackets
                Spacer,
                Header("Display"),
                Field(1), // word_wrap
                Field(4), // show_git_diff
                Field(5), // show_blame
                Spacer,
                Header("Performance"),
                Field(6), // large_file_threshold
            ],
            SettingsTab::FileManager => vec![
                Header("Display"),
                Field(0), // extended_view_width
                Field(2), // dir_size_in_wide_view
                Field(3), // dir_size_budget_ms
                Spacer,
                Header("Search"),
                Field(1), // content_search_max_file_size_mb
            ],
            SettingsTab::Terminal => vec![Field(0)],
            SettingsTab::Lsp => {
                let mut rows = vec![
                    Header("General"),
                    Field(0), // enabled
                    Field(1), // auto_completion
                    Spacer,
                    Header("Timing"),
                    Field(2), // completion_delay
                    Field(3), // hover_delay
                    Spacer,
                    Header("Servers"),
                    LspAddServer,
                ];
                for i in 0..self.lsp_server_keys.len() {
                    rows.push(LspServer(i));
                }
                rows
            }
            SettingsTab::Logging => vec![Field(0), Field(1)],
            SettingsTab::Vfs => vec![Field(0)],
            SettingsTab::Ai => {
                let mut rows = self.connection_list_rows();
                rows.extend([
                    Spacer,
                    Header("Model"),
                    Field(0), // max_tokens
                    Field(1), // reasoning
                    Spacer,
                    Header("Permissions"),
                    Field(7), // permission mode for new sessions
                    Spacer,
                    Header("Transcript"),
                    Field(2), // autofold
                    Spacer,
                    Header("Web"),
                    Field(3), // web backend
                    Field(4), // search engine
                    Field(5), // browser display
                    Field(6), // browser executable
                ]);
                rows
            }
            SettingsTab::Connection => self.connection_page_rows(),
            SettingsTab::Keybindings => Vec::new(),
        }
    }

    pub(super) fn current_row(&self) -> Option<ContentRow> {
        self.content_rows().get(self.field_cursor).copied()
    }

    pub(super) fn current_field_idx(&self) -> Option<usize> {
        match self.current_row()? {
            ContentRow::Field(i) => Some(i),
            _ => None,
        }
    }

    pub(super) fn first_selectable_row(&self) -> usize {
        self.content_rows()
            .iter()
            .position(|r| r.is_selectable())
            .unwrap_or(0)
    }

    pub(super) fn last_selectable_row(&self) -> usize {
        let rows = self.content_rows();
        rows.iter()
            .enumerate()
            .rev()
            .find_map(|(i, r)| if r.is_selectable() { Some(i) } else { None })
            .unwrap_or(0)
    }

    /// Move cursor to the next selectable row in the given direction.
    /// Returns false if no further selectable row exists.
    pub(super) fn step_cursor(&mut self, forward: bool) -> bool {
        let rows = self.content_rows();
        if rows.is_empty() {
            return false;
        }
        let mut c = self.field_cursor.min(rows.len().saturating_sub(1));
        loop {
            if forward {
                if c + 1 >= rows.len() {
                    return false;
                }
                c += 1;
            } else if c == 0 {
                return false;
            } else {
                c -= 1;
            }
            if rows[c].is_selectable() {
                self.field_cursor = c;
                return true;
            }
        }
    }

    // ---- Scroll ----

    pub(super) fn clamp_scroll(&mut self, visible: usize) {
        self.content_scroll =
            termide_ui::ensure_offset_visible(self.content_scroll, self.field_cursor, visible);
    }

    /// Commit the current edit buffer to the config.
    pub(super) fn commit_edit(&mut self) {
        let tab = self.field_tab();
        let Some(field_idx) = self.current_field_idx() else {
            self.editing = false;
            return;
        };
        let fields = fields_for_tab(tab);
        let Some(desc) = fields.get(field_idx) else {
            self.editing = false;
            return;
        };

        match desc.field_type {
            FieldType::Number => {
                let val = self.edit_input.text().parse::<u64>().unwrap_or(0);
                self.apply_number(tab, field_idx, val);
                self.dirty = true;
            }
            FieldType::OptionalText => {
                let text = self.edit_input.text().to_string();
                self.apply_text(tab, field_idx, &text);
                self.dirty = true;
            }
            FieldType::OptionalNumber => {
                // Empty or zero clears the field back to "(auto)".
                let val = self
                    .edit_input
                    .text()
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|n| *n > 0);
                self.apply_optional_number(tab, field_idx, val);
                self.dirty = true;
            }
            // The model field: an enum, but its typed-id escape commits text.
            FieldType::Enum if tab == SettingsTab::Connection && field_idx == connection::MODEL => {
                let text = self.edit_input.text().to_string();
                self.apply_connection_text(field_idx, &text);
                self.dirty = true;
            }
            _ => {}
        }
        self.editing = false;
    }

    /// Cancel the current inline edit.
    pub(super) fn cancel_edit(&mut self) {
        self.editing = false;
    }

    /// Start editing the current field.
    pub(super) fn start_edit(&mut self) {
        let Some(field_idx) = self.current_field_idx() else {
            return;
        };
        let fields = fields_for_tab(self.field_tab());
        let Some(desc) = fields.get(field_idx) else {
            return;
        };
        // The model field is an enum but its "type an id" escape edits it
        // inline like a text field.
        let is_model =
            self.field_tab() == SettingsTab::Connection && field_idx == connection::MODEL;
        match desc.field_type {
            FieldType::Bool => return,
            FieldType::Enum if !is_model => return,
            _ => {}
        }
        let value = if self.field_tab() == SettingsTab::Connection {
            self.connection_edit_text(field_idx)
        } else {
            let mut value = self.field_value(field_idx);
            // Strip "(auto)" / "(none)" placeholders
            if value.starts_with('(') {
                value.clear();
            }
            value
        };
        self.edit_input = termide_ui::TextInput::with_text(value);
        self.editing = true;
    }

    fn apply_number(&mut self, tab: SettingsTab, index: usize, val: u64) {
        match tab {
            SettingsTab::General => match index {
                4 => self.config.general.auto_stack_threshold = val as u16,
                5 => self.config.general.min_panel_width = val as u16,
                6 => self.config.general.project_retention_days = val as u32,
                8 => self.config.general.resource_monitor_interval = val,
                _ => {}
            },
            SettingsTab::Editor => match index {
                0 => self.config.editor.tab_size = val as usize,
                6 => self.config.editor.large_file_threshold_mb = val,
                _ => {}
            },
            SettingsTab::FileManager => match index {
                0 => self.config.file_manager.extended_view_width = val as usize,
                1 => self.config.file_manager.content_search_max_file_size_mb = val,
                3 => self.config.file_manager.dir_size_budget_ms = val,
                _ => {}
            },
            SettingsTab::Lsp => match index {
                2 => self.config.lsp.completion_delay_ms = val,
                3 => self.config.lsp.hover_delay_ms = val,
                _ => {}
            },
            SettingsTab::Vfs => {
                if index == 0 {
                    self.config.vfs.connection_timeout_secs = val;
                }
            }
            SettingsTab::Ai => {
                if index == 0 {
                    self.config.ai.max_tokens_per_turn = i64::try_from(val).unwrap_or(i64::MAX);
                }
            }
            _ => {}
        }
    }

    /// Apply an optional-number field (`None` means "(auto)").
    fn apply_optional_number(&mut self, tab: SettingsTab, index: usize, val: Option<u64>) {
        if tab == SettingsTab::Connection && index == connection::CONTEXT_WINDOW {
            self.apply_connection_window(val);
        }
    }

    fn apply_text(&mut self, tab: SettingsTab, index: usize, text: &str) {
        match tab {
            SettingsTab::Terminal => {
                if index == 0 {
                    if text.is_empty() {
                        self.config.terminal.default_shell = None;
                    } else {
                        self.config.terminal.default_shell = Some(text.to_string());
                    }
                }
            }
            SettingsTab::Logging => {
                if index == 0 {
                    if text.is_empty() {
                        self.config.logging.file_path = None;
                    } else {
                        self.config.logging.file_path = Some(text.to_string());
                    }
                }
            }
            SettingsTab::Ai => {
                if index == 6 {
                    self.config.ai.web.chrome_path = text.to_string();
                }
            }
            SettingsTab::Connection => self.apply_connection_text(index, text),
            _ => {}
        }
    }
}

#[cfg(test)]
mod content_row_tests {
    use super::*;
    use crate::settings::fields::fields_for_tab;

    /// `content_rows` lists the fields to render by hand, one `Field(i)` per
    /// entry, while `fields_for_tab` declares what exists. Nothing connects
    /// them, so a setting added to the descriptor list but not to the rows is
    /// simply invisible in the modal — configurable only by editing the TOML.
    /// This test is what notices.
    #[test]
    fn every_declared_field_is_rendered_somewhere() {
        let tabs = [
            SettingsTab::General,
            SettingsTab::Editor,
            SettingsTab::FileManager,
            SettingsTab::Terminal,
            SettingsTab::Lsp,
            SettingsTab::Logging,
            SettingsTab::Vfs,
            SettingsTab::Ai,
        ];

        for tab in tabs {
            let mut modal = SettingsModal::new(Config::default(), false);
            modal.active_tab = tab;

            let declared = fields_for_tab(tab).len();
            let rendered: Vec<usize> = modal
                .content_rows()
                .into_iter()
                .filter_map(|row| match row {
                    ContentRow::Field(i) => Some(i),
                    _ => None,
                })
                .collect();

            for index in 0..declared {
                assert!(
                    rendered.contains(&index),
                    "{tab:?}: field {index} is declared but never rendered"
                );
            }
        }
    }
}

#[cfg(test)]
mod reset_availability_tests {
    use super::*;

    /// "Reset to Defaults" used to grey out whenever the modal had no unsaved
    /// edits, which is the state it opens in — so a user who had configured
    /// termide, saved, and come back found the button dead exactly when it had
    /// the most to do.
    #[test]
    fn reset_is_offered_for_a_saved_non_default_config() {
        let mut config = Config::default();
        config.general.theme = "dracula".to_string();

        let modal = SettingsModal::new(config, false);
        assert!(!modal.dirty, "a freshly opened modal has no unsaved edits");
        assert!(
            modal.reset_available,
            "but the config differs from the defaults, so reset has work to do"
        );
    }

    #[test]
    fn reset_is_inert_for_a_default_config() {
        let modal = SettingsModal::new(Config::default(), false);
        assert!(!modal.reset_available);
    }

    /// Editing a value makes reset available; resetting takes it away again.
    #[test]
    fn availability_follows_the_config() {
        let mut modal = SettingsModal::new(Config::default(), false);
        assert!(!modal.reset_available);

        modal.config.general.vim_mode = true;
        modal.mark_dirty();
        assert!(modal.reset_available);

        modal.config = Config::default();
        modal.mark_dirty();
        assert!(!modal.reset_available);
    }
}

#[cfg(test)]
mod reset_content_tests {
    use super::*;
    use crate::settings::kb::{get_kb_value, kb_binding_names};
    use crate::settings::BUTTON_RESET;

    /// Reset used to install a raw `Config::default()`, whose keybindings are
    /// all unset — so the Keybindings tab went blank and stayed blank until
    /// the modal was reopened, making it look as though reset had wiped them.
    #[test]
    fn reset_leaves_the_keybinding_list_populated() {
        let mut modal = SettingsModal::new(Config::default(), false);
        modal.config.general.keybindings.quit =
            Some(termide_config::KeyBinding::Single("Alt+F4".to_string()));
        modal.mark_dirty();

        modal.selected_button = BUTTON_RESET;
        modal.execute_selected_button().unwrap();

        let names = kb_binding_names(0);
        let populated = names
            .iter()
            .filter(|name| !get_kb_value(&modal.config, 0, name).is_empty())
            .count();
        assert!(
            populated > 20,
            "defaults should be visible straight away, got {populated} of {}",
            names.len()
        );
        assert_eq!(get_kb_value(&modal.config, 0, "quit"), "Alt+Q");
    }
}
