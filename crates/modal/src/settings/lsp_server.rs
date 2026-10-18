//! The LSP tab's servers: the page that edits one, laid out like the AI
//! tab's connection page — the server's fields, then a row of buttons back
//! to the list and deleting it. The page edits the server in place in the
//! config, so every change is saved as it is made.

use termide_config::LspServerSettings;
use termide_i18n as i18n;

use super::connection::{BACK, DELETE};
use super::fields::{empty_or, ContentRow, FieldDescriptor, FieldType};
use super::{SettingsModal, SettingsTab};

/// The server page's fields, as indices into [`lsp_server_fields`].
pub(super) const LANGUAGE: usize = 0;
pub(super) const COMMAND: usize = 1;
pub(super) const ARGS: usize = 2;
pub(super) const ROOT_MARKERS: usize = 3;

/// The server page, while one is open.
#[derive(Debug, Clone)]
pub(super) struct LspServerEdit {
    /// The server's language: its key in `config.lsp.servers`.
    pub language: String,
    /// Added from the list and not given a command yet: going back drops
    /// it, so an abandoned "add" leaves nothing behind.
    pub added: bool,
    /// Why the value typed last was refused, and the field it was typed in.
    pub error: Option<(usize, String)>,
    /// The button chosen on the buttons row: [`BACK`] or [`DELETE`].
    pub button: usize,
}

/// The language an added server starts under, until the user names it.
const NEW_SERVER: &str = "new";

/// The page's buttons as they are drawn, `[ label ]`, left to right.
pub(super) fn lsp_server_buttons() -> [String; 2] {
    let t = i18n::t();
    [
        format!("[ {} ]", t.settings_ai_connection_back()),
        format!("[ {} ]", t.settings_lsp_delete_server()),
    ]
}

/// The fields of the server page.
pub(super) fn lsp_server_fields() -> Vec<FieldDescriptor> {
    let t = i18n::t();
    [
        t.settings_lsp_server_language(),
        t.settings_lsp_server_command(),
        t.settings_lsp_server_args(),
        t.settings_lsp_server_root_markers(),
    ]
    .into_iter()
    .map(|label| FieldDescriptor {
        label,
        field_type: FieldType::OptionalText,
    })
    .collect()
}

/// A comma-separated list, as the args and root markers are typed.
fn split_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

impl SettingsModal {
    /// The language of the server the page has open.
    pub(super) fn open_lsp_server_name(&self) -> Option<&str> {
        self.lsp_server_edit.as_ref().map(|e| e.language.as_str())
    }

    fn edited_server(&self) -> Option<&LspServerSettings> {
        self.config.lsp.servers.get(self.open_lsp_server_name()?)
    }

    fn edited_server_mut(&mut self) -> Option<&mut LspServerSettings> {
        let language = self.lsp_server_edit.as_ref()?.language.clone();
        self.config.lsp.servers.get_mut(&language)
    }

    /// Why the value typed last on the open page — a connection's or a
    /// server's — was refused, and the field it was typed in.
    pub(super) fn page_error(&self) -> Option<&(usize, String)> {
        match self.field_tab() {
            SettingsTab::Connection => self.connection_edit.as_ref()?.error.as_ref(),
            SettingsTab::LspServer => self.lsp_server_edit.as_ref()?.error.as_ref(),
            _ => None,
        }
    }

    /// The open page's buttons as drawn, and the one chosen.
    pub(super) fn page_buttons(&self) -> ([String; 2], usize) {
        match self.field_tab() {
            SettingsTab::LspServer => (
                lsp_server_buttons(),
                self.lsp_server_edit.as_ref().map_or(BACK, |e| e.button),
            ),
            _ => (
                super::connection::connection_buttons(),
                self.connection_edit.as_ref().map_or(BACK, |e| e.button),
            ),
        }
    }

    /// Choose button `button` of the open page.
    pub(super) fn choose_page_button(&mut self, button: usize) {
        if let Some(edit) = self.connection_edit.as_mut() {
            edit.button = button;
        }
        if let Some(edit) = self.lsp_server_edit.as_mut() {
            edit.button = button;
        }
    }

    /// The rows of the server page.
    pub(super) fn lsp_server_page_rows(&self) -> Vec<ContentRow> {
        use ContentRow::{Field, Header, LspServerButtons, Spacer};
        vec![
            Header(i18n::t().settings_header_server()),
            Field(LANGUAGE),
            Field(COMMAND),
            Field(ARGS),
            Field(ROOT_MARKERS),
            Spacer,
            LspServerButtons,
        ]
    }

    /// Add a server under a placeholder language and open it.
    pub(super) fn add_lsp_server(&mut self) {
        let language = (1..)
            .map(|n| match n {
                1 => NEW_SERVER.to_string(),
                n => format!("{NEW_SERVER}-{n}"),
            })
            .find(|name| !self.config.lsp.servers.contains_key(name))
            .expect("some suffix is free");
        self.config.lsp.servers.insert(
            language.clone(),
            LspServerSettings {
                command: String::new(),
                args: Vec::new(),
                root_markers: Vec::new(),
            },
        );
        self.refresh_server_keys();
        self.mark_dirty();
        self.show_lsp_server(language, true);
    }

    /// Open server `language` on its page.
    pub(super) fn open_lsp_server(&mut self, language: String) {
        if self.config.lsp.servers.contains_key(&language) {
            self.show_lsp_server(language, false);
        }
    }

    fn show_lsp_server(&mut self, language: String, added: bool) {
        self.lsp_server_edit = Some(LspServerEdit {
            language,
            added,
            error: None,
            button: BACK,
        });
        self.enum_picker = None;
        self.editing = false;
        self.content_scroll = 0;
        self.field_cursor = self.first_selectable_row();
    }

    /// Back from the page to the list, on the row of the server it had open.
    /// A server added and still without a command is dropped.
    pub(super) fn close_lsp_server(&mut self) {
        let Some(edit) = self.lsp_server_edit.take() else {
            return;
        };
        self.enum_picker = None;
        self.editing = false;
        let unused = self
            .config
            .lsp
            .servers
            .get(&edit.language)
            .is_some_and(|server| server.command.trim().is_empty());
        if edit.added && unused {
            self.config.lsp.servers.remove(&edit.language);
            self.refresh_server_keys();
            self.mark_dirty();
        }
        let rows = self.content_rows();
        self.field_cursor = self
            .lsp_server_keys
            .iter()
            .position(|name| *name == edit.language)
            .and_then(|index| {
                rows.iter()
                    .position(|row| *row == ContentRow::LspServer(index))
            })
            .or_else(|| rows.iter().position(|row| *row == ContentRow::LspAddServer))
            .unwrap_or_else(|| self.first_selectable_row());
    }

    /// Choose the button to the left or right on the buttons row.
    pub(super) fn step_lsp_server_button(&mut self, forward: bool) {
        if let Some(edit) = self.lsp_server_edit.as_mut() {
            edit.button = if forward { DELETE } else { BACK };
        }
    }

    /// Press the chosen button: back to the list, or delete the server.
    pub(super) fn press_lsp_server_button(&mut self) {
        let Some(edit) = self.lsp_server_edit.as_ref() else {
            return;
        };
        if edit.button == DELETE {
            let language = edit.language.clone();
            self.delete_lsp_server(&language);
        } else {
            self.close_lsp_server();
        }
    }

    /// Remove server `language`, leaving its page if it is open.
    pub(super) fn delete_lsp_server(&mut self, language: &str) {
        if self.config.lsp.servers.remove(language).is_none() {
            return;
        }
        let index = self.lsp_server_keys.iter().position(|k| k == language);
        self.refresh_server_keys();
        if self.open_lsp_server_name() == Some(language) {
            self.lsp_server_edit = None;
            self.enum_picker = None;
            self.editing = false;
        }
        self.mark_dirty();
        // On the server that took its place, or the last one, or the add row.
        let rows = self.content_rows();
        let last = self.lsp_server_keys.len().checked_sub(1);
        self.field_cursor = index
            .zip(last)
            .map(|(index, last)| index.min(last))
            .and_then(|index| {
                rows.iter()
                    .position(|row| *row == ContentRow::LspServer(index))
            })
            .or_else(|| rows.iter().position(|row| *row == ContentRow::LspAddServer))
            .unwrap_or_else(|| self.first_selectable_row());
    }

    /// A server page field's value as the row shows it.
    pub(super) fn lsp_server_value(&self, index: usize) -> String {
        match index {
            LANGUAGE => self.lsp_server_edit_text(index),
            _ => empty_or(&self.lsp_server_edit_text(index)),
        }
    }

    /// A field's value as it is edited: the stored text, without the
    /// placeholder its row shows for an empty one.
    pub(super) fn lsp_server_edit_text(&self, index: usize) -> String {
        let Some(server) = self.edited_server() else {
            return String::new();
        };
        match index {
            LANGUAGE => self.open_lsp_server_name().unwrap_or_default().to_string(),
            COMMAND => server.command.clone(),
            ARGS => server.args.join(", "),
            ROOT_MARKERS => server.root_markers.join(", "),
            _ => String::new(),
        }
    }

    /// Store a text field typed on the page.
    pub(super) fn apply_lsp_server_text(&mut self, index: usize, text: &str) {
        let text = text.trim();
        if index == LANGUAGE {
            self.rename_lsp_server(text);
            return;
        }
        let Some(server) = self.edited_server_mut() else {
            return;
        };
        match index {
            COMMAND => server.command = text.to_string(),
            ARGS => server.args = split_list(text),
            ROOT_MARKERS => server.root_markers = split_list(text),
            _ => {}
        }
    }

    /// Move the open server to another language, unless that is empty or
    /// another server's.
    fn rename_lsp_server(&mut self, language: &str) {
        let Some(old) = self.open_lsp_server_name().map(str::to_string) else {
            return;
        };
        if language == old {
            return;
        }
        let refused = language.is_empty() || self.config.lsp.servers.contains_key(language);
        let Some(edit) = self.lsp_server_edit.as_mut() else {
            return;
        };
        if refused {
            edit.error = Some((
                LANGUAGE,
                i18n::t().settings_lsp_server_language_taken().to_string(),
            ));
            return;
        }
        edit.error = None;
        edit.language = language.to_string();
        if let Some(server) = self.config.lsp.servers.remove(&old) {
            self.config.lsp.servers.insert(language.to_string(), server);
        }
        self.refresh_server_keys();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::FocusArea;
    use crate::Modal;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{buffer::Buffer, layout::Rect};
    use termide_config::Config;

    fn lsp_modal() -> SettingsModal {
        let mut config = Config::default();
        config.lsp.servers.clear();
        config.lsp.servers.insert(
            "rust".into(),
            LspServerSettings {
                command: "rust-analyzer".into(),
                args: Vec::new(),
                root_markers: vec!["Cargo.toml".into()],
            },
        );
        let mut modal = SettingsModal::new(config.clone(), config, false);
        modal.active_tab = SettingsTab::Lsp;
        modal.focus = FocusArea::Content;
        modal
    }

    fn focus(modal: &mut SettingsModal, row: ContentRow) {
        modal.field_cursor = modal
            .content_rows()
            .iter()
            .position(|r| *r == row)
            .unwrap_or_else(|| panic!("{row:?} is not listed"));
    }

    fn press(modal: &mut SettingsModal, code: KeyCode) {
        let chord = termide_core::KeyChord::identity(KeyEvent::new(code, KeyModifiers::NONE));
        modal.handle_key(chord).unwrap();
    }

    /// Type `text` into page field `field` over what it holds.
    fn edit(modal: &mut SettingsModal, field: usize, text: &str) {
        focus(modal, ContentRow::Field(field));
        press(modal, KeyCode::Enter);
        modal.edit_input.set_text("");
        for c in text.chars() {
            press(modal, KeyCode::Char(c));
        }
        press(modal, KeyCode::Enter);
    }

    fn screen(modal: &mut SettingsModal) -> Vec<String> {
        let area = Rect::new(0, 0, 110, 40);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &termide_theme::Theme::default());
        (0..area.height)
            .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn click(modal: &mut SettingsModal, column: usize, row: usize) {
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: column as u16,
            row: row as u16,
            modifiers: KeyModifiers::NONE,
        };
        modal.handle_mouse(mouse, Rect::default()).unwrap();
    }

    #[test]
    fn an_added_server_is_edited_on_its_page_and_saved_as_typed() {
        let mut modal = lsp_modal();
        focus(&mut modal, ContentRow::LspAddServer);
        press(&mut modal, KeyCode::Enter);
        assert_eq!(modal.open_lsp_server_name(), Some(NEW_SERVER));
        edit(&mut modal, LANGUAGE, "python");
        edit(&mut modal, COMMAND, "pyright-langserver");
        edit(&mut modal, ARGS, "--stdio, --verbose");
        edit(&mut modal, ROOT_MARKERS, "pyproject.toml,setup.py");
        let server = &modal.config.lsp.servers["python"];
        assert_eq!(server.command, "pyright-langserver");
        assert_eq!(server.args, ["--stdio", "--verbose"]);
        assert_eq!(server.root_markers, ["pyproject.toml", "setup.py"]);
        assert!(!modal.config.lsp.servers.contains_key(NEW_SERVER));
        assert!(modal.save_pending);
        // Esc goes back to the list, on the server's row.
        press(&mut modal, KeyCode::Esc);
        assert!(modal.lsp_server_edit.is_none());
        let index = modal.lsp_server_keys.iter().position(|k| k == "python");
        assert_eq!(modal.current_row(), index.map(ContentRow::LspServer));
    }

    #[test]
    fn an_added_server_left_without_a_command_is_dropped() {
        let mut modal = lsp_modal();
        focus(&mut modal, ContentRow::LspAddServer);
        press(&mut modal, KeyCode::Enter);
        press(&mut modal, KeyCode::Backspace);
        assert_eq!(modal.lsp_server_keys, ["rust"]);
        assert_eq!(modal.current_row(), Some(ContentRow::LspAddServer));
    }

    #[test]
    fn a_language_that_is_empty_or_taken_is_refused() {
        let mut modal = lsp_modal();
        focus(&mut modal, ContentRow::LspAddServer);
        press(&mut modal, KeyCode::Enter);
        edit(&mut modal, LANGUAGE, "rust");
        assert_eq!(modal.open_lsp_server_name(), Some(NEW_SERVER));
        let shown = screen(&mut modal).join("\n");
        assert!(
            shown.contains(i18n::t().settings_lsp_server_language_taken()),
            "{shown}"
        );
        edit(&mut modal, LANGUAGE, " ");
        assert_eq!(modal.open_lsp_server_name(), Some(NEW_SERVER));
    }

    /// The page is drawn as rows of the content, so a click lands on the
    /// field under it — not on the list row the page replaced.
    #[test]
    fn clicks_on_the_page_hit_its_fields_and_buttons() {
        let mut modal = lsp_modal();
        modal.open_lsp_server("rust".into());
        let shown = screen(&mut modal);
        assert!(shown.iter().any(|row| row.contains("› rust")), "{shown:?}");
        let label = i18n::t().settings_lsp_server_command();
        let y = shown.iter().position(|row| row.contains(label)).unwrap();
        let x = shown[y][..shown[y].find(label).unwrap()].chars().count();
        click(&mut modal, x, y);
        assert!(modal.editing);
        assert_eq!(modal.edit_input.text(), "rust-analyzer");
        press(&mut modal, KeyCode::Esc);

        let [back, delete] = lsp_server_buttons();
        let shown = screen(&mut modal);
        let y = shown.iter().position(|row| row.contains(&back)).unwrap();
        let column_of = |label: &str| shown[y][..shown[y].find(label).unwrap()].chars().count();
        click(&mut modal, column_of(&back) + 1, y);
        assert!(modal.lsp_server_edit.is_none());
        assert_eq!(modal.current_row(), Some(ContentRow::LspServer(0)));

        modal.open_lsp_server("rust".into());
        screen(&mut modal);
        click(&mut modal, column_of(&delete) + 1, y);
        assert!(modal.config.lsp.servers.is_empty());
        assert!(modal.lsp_server_edit.is_none());
    }

    #[test]
    fn the_page_types_what_the_keyboard_sends() {
        let mut modal = lsp_modal();
        modal.open_lsp_server("rust".into());
        focus(&mut modal, ContentRow::Field(COMMAND));
        press(&mut modal, KeyCode::Enter);
        modal.edit_input.set_text("");
        // A key as a Russian layout sends it: `raw` is what was typed,
        // `canonical` its Latin, unshifted twin.
        for (raw, canonical) in [('ф', 'a'), ('+', '=')] {
            let chord = termide_core::KeyChord {
                raw: KeyEvent::new(KeyCode::Char(raw), KeyModifiers::NONE),
                canonical: KeyEvent::new(KeyCode::Char(canonical), KeyModifiers::NONE),
            };
            modal.handle_key(chord).unwrap();
        }
        assert_eq!(modal.edit_input.text(), "ф+");
    }
}
