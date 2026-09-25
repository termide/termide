//! The AI tab's connections: the rows that list them and the page that edits
//! one. The page edits the connection in place in the config, so the modal's
//! own Apply and Cancel cover it like every other field.

use termide_config::Connection;
use termide_i18n as i18n;
use unicode_width::UnicodeWidthStr;

use super::fields::{
    bool_str, empty_or, provider_label, step_value, ContentRow, EnumOptions, FieldDescriptor,
    FieldType, MODEL_TYPE_SENTINEL, PROVIDER_VALUES,
};
use super::{SettingsModal, SettingsTab};

/// The connection page's fields, as indices into [`connection_fields`].
pub(super) const NAME: usize = 0;
pub(super) const PROVIDER: usize = 1;
pub(super) const BASE_URL: usize = 2;
pub(super) const API_KEY_ENV: usize = 3;
pub(super) const MODEL: usize = 4;
pub(super) const CONTEXT_WINDOW: usize = 5;
pub(super) const DEFAULT: usize = 6;

/// The connection page, while one is open.
#[derive(Debug, Clone)]
pub(super) struct ConnectionEdit {
    /// The connection's name: its key in `config.ai.connections`.
    pub name: String,
    /// Named after its provider until the user names it, so choosing another
    /// provider renames it too.
    pub auto_named: bool,
    /// Why the name typed last was refused.
    pub error: Option<String>,
    /// The button chosen on the buttons row: [`BACK`] or [`DELETE`].
    pub button: usize,
}

/// The page's buttons, left to right.
pub(super) const BACK: usize = 0;
pub(super) const DELETE: usize = 1;

/// Blank columns between the page's buttons.
const BUTTON_GAP: usize = 2;

/// The page's buttons as they are drawn, `[ label ]`, left to right.
pub(super) fn connection_buttons() -> [String; 2] {
    let t = i18n::t();
    [
        format!("[ {} ]", t.settings_ai_connection_back()),
        format!("[ {} ]", t.settings_ai_delete_connection()),
    ]
}

/// The column span of each button on a row starting at `x`.
pub(super) fn connection_button_spans(x: usize) -> [(usize, usize); 2] {
    let [back, delete] = connection_buttons().map(|label| label.width());
    let back = (x, x + back);
    let delete_x = back.1 + BUTTON_GAP;
    [back, (delete_x, delete_x + delete)]
}

/// The fields of the connection page.
pub(super) fn connection_fields() -> Vec<FieldDescriptor> {
    let t = i18n::t();
    vec![
        FieldDescriptor {
            label: t.settings_ai_connection_name(),
            field_type: FieldType::OptionalText,
        },
        FieldDescriptor {
            label: t.settings_agent_provider(),
            field_type: FieldType::Enum,
        },
        FieldDescriptor {
            label: t.settings_agent_base_url(),
            field_type: FieldType::OptionalText,
        },
        FieldDescriptor {
            label: t.settings_agent_api_key_env(),
            field_type: FieldType::OptionalText,
        },
        FieldDescriptor {
            // A dropdown: the endpoint's models when the app has fetched
            // them (with a "type an id" escape), just the escape otherwise.
            label: t.settings_agent_model(),
            field_type: FieldType::Enum,
        },
        FieldDescriptor {
            label: t.settings_agent_context_window(),
            field_type: FieldType::OptionalNumber,
        },
        FieldDescriptor {
            label: t.settings_ai_connection_default(),
            field_type: FieldType::Bool,
        },
    ]
}

/// The name a connection gets from its provider until the user names it.
fn provider_slug(provider: &str) -> &str {
    match provider {
        "openai_compatible" => "openai",
        "anthropic_compatible" => "anthropic",
        "claude_code" => "claude-code",
        other => other,
    }
}

impl SettingsModal {
    /// Which tab's fields the content shows: the connection page's while one
    /// is open, the active tab's otherwise.
    pub(super) fn field_tab(&self) -> SettingsTab {
        if self.active_tab == SettingsTab::Ai && self.connection_edit.is_some() {
            SettingsTab::Connection
        } else {
            self.active_tab
        }
    }

    /// The name of the connection the page has open.
    pub(super) fn open_connection_name(&self) -> Option<&str> {
        self.connection_edit.as_ref().map(|edit| edit.name.as_str())
    }

    fn edited(&self) -> Option<&Connection> {
        self.config.ai.connections.get(self.open_connection_name()?)
    }

    fn edited_mut(&mut self) -> Option<&mut Connection> {
        let name = self.connection_edit.as_ref()?.name.clone();
        self.config.ai.connections.get_mut(&name)
    }

    /// The name of connection row `index`: the connections by name.
    pub(super) fn connection_name(&self, index: usize) -> Option<String> {
        self.config.ai.connections.keys().nth(index).cloned()
    }

    /// The AI tab's rows for the connections: a heading, one row each, the
    /// row that adds one.
    pub(super) fn connection_list_rows(&self) -> Vec<ContentRow> {
        let mut rows = vec![ContentRow::Header("Connections")];
        rows.extend((0..self.config.ai.connections.len()).map(ContentRow::Connection));
        rows.push(ContentRow::ConnectionAdd);
        rows
    }

    /// The rows of the connection page. A CLI agent brings its endpoint,
    /// key and window, so only its model (pre-selected over ACP) applies.
    pub(super) fn connection_page_rows(&self) -> Vec<ContentRow> {
        use ContentRow::{ConnectionButtons, Field, Header, Spacer};
        let cli = self.edited().is_some_and(Connection::is_cli);
        let mut rows = vec![Header("Connection"), Field(NAME), Field(PROVIDER)];
        if !cli {
            rows.extend([Field(BASE_URL), Field(API_KEY_ENV)]);
        }
        rows.push(Field(MODEL));
        if !cli {
            rows.push(Field(CONTEXT_WINDOW));
        }
        rows.extend([Field(DEFAULT), Spacer, ConnectionButtons]);
        rows
    }

    /// Add an OpenAI-compatible connection and open it.
    pub(super) fn add_connection(&mut self) {
        let connection = Connection::default();
        let name = self.unused_name(provider_slug(&connection.provider), None);
        self.config.ai.connections.insert(name.clone(), connection);
        self.mark_dirty();
        self.show_connection(name, true);
    }

    /// Open connection `name` on its page.
    pub(super) fn open_connection(&mut self, name: String) {
        if self.config.ai.connections.contains_key(&name) {
            self.show_connection(name, false);
        }
    }

    fn show_connection(&mut self, name: String, auto_named: bool) {
        self.connection_edit = Some(ConnectionEdit {
            name,
            auto_named,
            error: None,
            button: BACK,
        });
        self.enum_picker = None;
        self.editing = false;
        self.content_scroll = 0;
        self.field_cursor = self.first_selectable_row();
        self.model_options.clear();
        self.request_models();
    }

    /// Back from the page to the list, on the row of the connection it had
    /// open.
    pub(super) fn close_connection(&mut self) {
        let Some(edit) = self.connection_edit.take() else {
            return;
        };
        self.enum_picker = None;
        self.editing = false;
        let index = self
            .config
            .ai
            .connections
            .keys()
            .position(|name| *name == edit.name);
        let rows = self.content_rows();
        self.field_cursor = index
            .and_then(|index| {
                rows.iter()
                    .position(|row| *row == ContentRow::Connection(index))
            })
            .unwrap_or_else(|| self.first_selectable_row());
    }

    /// Choose the button to the left or right on the buttons row.
    pub(super) fn step_connection_button(&mut self, forward: bool) {
        if let Some(edit) = self.connection_edit.as_mut() {
            edit.button = if forward { DELETE } else { BACK };
        }
    }

    /// Press the chosen button: back to the list, or delete the connection.
    pub(super) fn press_connection_button(&mut self) {
        let Some(edit) = self.connection_edit.as_ref() else {
            return;
        };
        if edit.button == DELETE {
            let name = edit.name.clone();
            self.delete_connection(&name);
        } else {
            self.close_connection();
        }
    }

    /// Remove connection `name`; new sessions then start on the first left.
    pub(super) fn delete_connection(&mut self, name: &str) {
        if self.config.ai.connections.remove(name).is_none() {
            return;
        }
        if self.config.ai.connection == name {
            self.config.ai.connection.clear();
        }
        if self.open_connection_name() == Some(name) {
            self.connection_edit = None;
            self.enum_picker = None;
            self.editing = false;
        }
        self.mark_dirty();
        let last = self.last_selectable_row();
        self.field_cursor = self.field_cursor.min(last);
        if !self
            .content_rows()
            .get(self.field_cursor)
            .is_some_and(ContentRow::is_selectable)
        {
            self.field_cursor = self.first_selectable_row();
        }
    }

    /// `base`, or `base-2`, `base-3`… — the first no other connection has.
    fn unused_name(&self, base: &str, own: Option<&str>) -> String {
        let taken = |name: &str| Some(name) != own && self.config.ai.connections.contains_key(name);
        if !taken(base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|name| !taken(name))
            .expect("some suffix is free")
    }

    /// A connection page field's value as the row shows it.
    pub(super) fn connection_value(&self, index: usize) -> String {
        let Some(connection) = self.edited() else {
            return String::new();
        };
        match index {
            NAME => self.open_connection_name().unwrap_or_default().to_string(),
            PROVIDER => provider_label(&connection.provider),
            BASE_URL => empty_or(&connection.base_url),
            API_KEY_ENV => empty_or(&connection.api_key_env),
            MODEL if connection.model.is_empty() => i18n::t().settings_ai_model_auto().to_string(),
            MODEL => connection.model.clone(),
            CONTEXT_WINDOW => connection.context_window_fallback.map_or_else(
                || {
                    format!(
                        "(default {})",
                        termide_config::DEFAULT_CONTEXT_WINDOW_FALLBACK
                    )
                },
                |n| n.to_string(),
            ),
            DEFAULT => {
                bool_str(Some(self.config.ai.connection.as_str()) == self.open_connection_name())
            }
            _ => String::new(),
        }
    }

    /// A text field's value as it is edited: the stored text, without the
    /// placeholder its row shows for an empty or default one.
    pub(super) fn connection_edit_text(&self, index: usize) -> String {
        let Some(connection) = self.edited() else {
            return String::new();
        };
        match index {
            NAME => self.open_connection_name().unwrap_or_default().to_string(),
            BASE_URL => connection.base_url.clone(),
            API_KEY_ENV => connection.api_key_env.clone(),
            MODEL => connection.model.clone(),
            CONTEXT_WINDOW => connection
                .context_window_fallback
                .map(|n| n.to_string())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// Toggle the page's switch: whether new sessions start on this
    /// connection. Off everywhere, they start on the first by name.
    pub(super) fn toggle_connection_field(&mut self, index: usize) {
        if index == DEFAULT {
            if let Some(name) = self.open_connection_name().map(str::to_string) {
                if self.config.ai.connection == name {
                    self.config.ai.connection.clear();
                } else {
                    self.config.ai.connection = name;
                }
                self.mark_dirty();
            }
        }
    }

    /// The choices of the page's dropdowns: the provider, and the model.
    pub(super) fn connection_enum_options(&self, index: usize) -> Option<EnumOptions> {
        let connection = self.edited()?;
        match index {
            PROVIDER => {
                // OpenAI/Anthropic compatible are wire protocols the built-in
                // loop speaks (base_url picks the actual server); Claude Code
                // and Codex drive their CLI over ACP. Listed by label.
                let values: Vec<String> = PROVIDER_VALUES.iter().map(|s| s.to_string()).collect();
                let labels = PROVIDER_VALUES.iter().map(|v| provider_label(v)).collect();
                let current = values.iter().position(|v| *v == connection.provider);
                Some(EnumOptions {
                    values,
                    labels,
                    current,
                })
            }
            MODEL => Some(model_enum_options(&connection.model, &self.model_options)),
            _ => None,
        }
    }

    /// Store the value chosen in one of the page's dropdowns.
    pub(super) fn apply_connection_enum(&mut self, index: usize, value: &str) {
        match index {
            PROVIDER => self.set_connection_provider(value),
            MODEL => {
                if let Some(connection) = self.edited_mut() {
                    connection.model = value.to_string();
                }
            }
            _ => {}
        }
    }

    /// Step the provider with Left/Right, wrapping.
    pub(super) fn cycle_connection_field(&mut self, index: usize, forward: bool) {
        if index != PROVIDER {
            return;
        }
        let Some(mut provider) = self.edited().map(|c| c.provider.clone()) else {
            return;
        };
        let values: Vec<String> = PROVIDER_VALUES.iter().map(|s| s.to_string()).collect();
        step_value(&mut provider, &values, forward);
        self.set_connection_provider(&provider);
    }

    /// Store a text field typed on the page.
    pub(super) fn apply_connection_text(&mut self, index: usize, text: &str) {
        let text = text.trim();
        if index == NAME {
            self.rename_connection(text);
            return;
        }
        let changes_endpoint = matches!(index, BASE_URL | API_KEY_ENV);
        let Some(connection) = self.edited_mut() else {
            return;
        };
        match index {
            BASE_URL if text.is_empty() => connection.base_url = Connection::default().base_url,
            BASE_URL => connection.base_url = text.to_string(),
            API_KEY_ENV => connection.api_key_env = text.to_string(),
            MODEL => connection.model = text.to_string(),
            _ => {}
        }
        if changes_endpoint {
            self.request_models();
        }
    }

    /// Store the context window typed on the page; `None` is the default.
    pub(super) fn apply_connection_window(&mut self, window: Option<u64>) {
        if let Some(connection) = self.edited_mut() {
            connection.context_window_fallback = window;
        }
    }

    /// Rename the open connection, unless the name is empty or another's.
    fn rename_connection(&mut self, name: &str) {
        let Some(old) = self.open_connection_name().map(str::to_string) else {
            return;
        };
        if name == old {
            return;
        }
        if name.is_empty() || self.config.ai.connections.contains_key(name) {
            if let Some(edit) = self.connection_edit.as_mut() {
                edit.error = Some(i18n::t().settings_ai_connection_name_taken().to_string());
            }
            return;
        }
        self.move_connection(&old, name);
        if let Some(edit) = self.connection_edit.as_mut() {
            edit.auto_named = false;
            edit.error = None;
        }
    }

    /// Move connection `old` to key `new`, keeping it the default if it was.
    fn move_connection(&mut self, old: &str, new: &str) {
        let Some(connection) = self.config.ai.connections.remove(old) else {
            return;
        };
        self.config
            .ai
            .connections
            .insert(new.to_string(), connection);
        if self.config.ai.connection == old {
            self.config.ai.connection = new.to_string();
        }
        if let Some(edit) = self.connection_edit.as_mut() {
            edit.name = new.to_string();
        }
    }

    /// Switch the open connection's provider. A CLI agent brings its own
    /// endpoint, key and window, so those go back to their defaults (and out
    /// of the saved file); the model stays, as the one to pre-select.
    fn set_connection_provider(&mut self, provider: &str) {
        let Some(connection) = self.edited_mut() else {
            return;
        };
        connection.provider = provider.to_string();
        if connection.is_cli() {
            let defaults = Connection::default();
            connection.base_url = defaults.base_url;
            connection.api_key_env = defaults.api_key_env;
            connection.context_window_fallback = defaults.context_window_fallback;
        }
        let auto = self.connection_edit.as_ref().is_some_and(|e| e.auto_named);
        if let (true, Some(old)) = (auto, self.open_connection_name().map(str::to_string)) {
            let new = self.unused_name(provider_slug(provider), Some(&old));
            self.move_connection(&old, &new);
        }
        self.model_options.clear();
        self.request_models();
    }

    /// Ask the app for the open connection's models; a CLI agent lists its
    /// own over ACP.
    fn request_models(&mut self) {
        self.model_fetch_request = self.edited().filter(|c| !c.is_cli()).cloned();
    }

    /// The connection whose models the model dropdown waits for, once: the
    /// app fetches them off-thread and hands them back with
    /// [`SettingsModal::set_model_options`].
    pub fn take_model_fetch_request(&mut self) -> Option<Connection> {
        self.model_fetch_request.take()
    }
}

/// The dropdown for a model field: "auto" first — the model left to the
/// provider, stored as an empty id — then the fetched models (with the
/// current value kept present), then a "type an id" escape that opens inline
/// editing. With no fetched models — a CLI provider, or an endpoint that
/// cannot list — only auto, the current value and the escape show, so typing
/// still works.
pub(super) fn model_enum_options(current: &str, model_options: &[String]) -> EnumOptions {
    let mut values: Vec<String> = model_options.to_vec();
    if !current.is_empty() && !values.iter().any(|v| v == current) {
        values.insert(0, current.to_string());
    }
    values.insert(0, String::new());
    let mut labels: Vec<String> = values.clone();
    labels[0] = i18n::t().settings_ai_model_auto().to_string();
    values.push(MODEL_TYPE_SENTINEL.to_string());
    labels.push(i18n::t().agent_model_other().to_string());
    let current_index = values.iter().position(|v| v == current);
    EnumOptions {
        values,
        labels,
        current: current_index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Modal, ModalResult};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{buffer::Buffer, layout::Rect};
    use termide_config::Config;

    use crate::settings::{FocusArea, SettingsResult};

    fn ai_modal(config: Config) -> SettingsModal {
        let mut modal = SettingsModal::new(config, false);
        modal.active_tab = SettingsTab::Ai;
        modal.focus = FocusArea::Content;
        modal
    }

    fn with_local() -> Config {
        let mut config = Config::default();
        config.ai.connections.insert(
            "local".into(),
            Connection {
                model: "qwen".into(),
                ..Connection::default()
            },
        );
        config
    }

    fn focus(modal: &mut SettingsModal, row: ContentRow) {
        modal.field_cursor = modal
            .content_rows()
            .iter()
            .position(|r| *r == row)
            .unwrap_or_else(|| panic!("{row:?} is not listed"));
    }

    fn press(modal: &mut SettingsModal, code: KeyCode) -> Option<ModalResult<SettingsResult>> {
        let chord = termide_core::KeyChord::identity(KeyEvent::new(code, KeyModifiers::NONE));
        modal.handle_key(chord).unwrap()
    }

    fn type_text(modal: &mut SettingsModal, text: &str) {
        for c in text.chars() {
            press(modal, KeyCode::Char(c));
        }
    }

    /// Type `text` into page field `field` over what it holds.
    fn edit(modal: &mut SettingsModal, field: usize, text: &str) {
        focus(modal, ContentRow::Field(field));
        press(modal, KeyCode::Enter);
        modal.edit_input.set_text("");
        type_text(modal, text);
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

    #[test]
    fn an_added_connection_is_named_after_its_provider_until_named() {
        let mut modal = ai_modal(Config::default());
        focus(&mut modal, ContentRow::ConnectionAdd);
        press(&mut modal, KeyCode::Enter);
        assert_eq!(modal.open_connection_name(), Some("openai"));
        assert!(modal.dirty);
        // The only one, so new sessions start on it, but it is not marked the
        // default until chosen; the switch goes on and off again.
        assert_eq!(modal.config.ai.default_connection(), Some("openai"));
        assert_eq!(modal.connection_value(DEFAULT), "false");
        modal.toggle_connection_field(DEFAULT);
        assert_eq!(modal.connection_value(DEFAULT), "true");
        modal.toggle_connection_field(DEFAULT);
        assert_eq!(modal.connection_value(DEFAULT), "false");
        assert!(modal.config.ai.connection.is_empty());

        // A CLI agent: renamed with it, and its endpoint fields go.
        edit(&mut modal, BASE_URL, "https://example/v1");
        modal.apply_connection_enum(PROVIDER, "claude_code");
        assert_eq!(modal.open_connection_name(), Some("claude-code"));
        let connection = &modal.config.ai.connections["claude-code"];
        assert_eq!(connection.base_url, Connection::default().base_url);
        assert!(!modal.content_rows().contains(&ContentRow::Field(BASE_URL)));
        assert!(modal.content_rows().contains(&ContentRow::Field(MODEL)));

        // Once named by hand the name stays.
        edit(&mut modal, NAME, "mine");
        modal.apply_connection_enum(PROVIDER, "codex");
        assert_eq!(modal.open_connection_name(), Some("mine"));
        let names: Vec<_> = modal.config.ai.connections.keys().cloned().collect();
        assert_eq!(names, ["mine"]);
    }

    #[test]
    fn a_name_that_is_empty_or_taken_is_refused() {
        let mut config = with_local();
        config
            .ai
            .connections
            .insert("cloud".into(), Connection::default());
        let mut modal = ai_modal(config);
        modal.open_connection("cloud".into());
        edit(&mut modal, NAME, "local");
        assert_eq!(modal.open_connection_name(), Some("cloud"));
        assert!(modal.connection_edit.as_ref().unwrap().error.is_some());
        let shown = screen(&mut modal).join("\n");
        assert!(
            shown.contains(i18n::t().settings_ai_connection_name_taken()),
            "{shown}"
        );
        edit(&mut modal, NAME, "  ");
        assert_eq!(modal.open_connection_name(), Some("cloud"));
        // A free name goes through, and carries the default mark with it.
        modal.toggle_connection_field(DEFAULT);
        edit(&mut modal, NAME, "hosted");
        assert_eq!(modal.config.ai.connection, "hosted");
        assert!(modal.connection_edit.as_ref().unwrap().error.is_none());
    }

    #[test]
    fn the_page_edits_the_connection_it_has_open_with_the_mouse_too() {
        let mut modal = ai_modal(with_local());
        focus(&mut modal, ContentRow::Connection(0));
        press(&mut modal, KeyCode::Enter);
        let shown = screen(&mut modal);
        assert!(shown.iter().any(|row| row.contains("› local")), "{shown:?}");
        // A click on the provider row opens the dropdown of this connection's
        // provider, and the choice lands on it.
        let label = i18n::t().settings_agent_provider();
        let y = shown.iter().position(|row| row.contains(label)).unwrap();
        let x = shown[y].find(label).unwrap() as u16;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y as u16,
            modifiers: KeyModifiers::NONE,
        };
        modal.handle_mouse(click, Rect::default()).unwrap();
        let picker = modal.enum_picker.as_mut().expect("the provider dropdown");
        assert_eq!(picker.field_index, PROVIDER);
        let options = modal.connection_enum_options(PROVIDER).unwrap();
        modal.enum_picker.as_mut().unwrap().cursor = options
            .values
            .iter()
            .position(|v| v == "anthropic_compatible")
            .unwrap();
        modal.commit_enum_picker();
        assert_eq!(
            modal.config.ai.connections["local"].provider,
            "anthropic_compatible"
        );
        // Esc goes back to the list, on the connection's row.
        press(&mut modal, KeyCode::Esc);
        assert!(modal.connection_edit.is_none());
        assert_eq!(modal.current_row(), Some(ContentRow::Connection(0)));
    }

    #[test]
    fn the_list_marks_the_default_and_del_removes_a_connection() {
        let mut config = with_local();
        config.ai.connections.insert(
            "cloud".into(),
            Connection {
                provider: "anthropic_compatible".into(),
                model: "claude-x".into(),
                ..Connection::default()
            },
        );
        config.ai.connection = "local".into();
        let mut modal = ai_modal(config);
        let shown = screen(&mut modal).join("\n");
        assert!(shown.contains("● local"), "{shown}");
        assert!(shown.contains("○ cloud"), "{shown}");
        assert!(shown.contains("OpenAI compatible · qwen"), "{shown}");

        // The other one becomes the default from its page.
        modal.open_connection("cloud".into());
        focus(&mut modal, ContentRow::Field(DEFAULT));
        press(&mut modal, KeyCode::Enter);
        assert_eq!(modal.config.ai.connection, "cloud");
        // Deleted from the page, the default falls back to what is left.
        focus(&mut modal, ContentRow::ConnectionButtons);
        press(&mut modal, KeyCode::Right);
        press(&mut modal, KeyCode::Enter);
        assert!(modal.connection_edit.is_none());
        assert!(modal.config.ai.connection.is_empty());
        assert_eq!(modal.config.ai.default_connection(), Some("local"));
        // And from the list with Del.
        focus(&mut modal, ContentRow::Connection(0));
        press(&mut modal, KeyCode::Delete);
        assert!(modal.config.ai.connections.is_empty());
        assert_eq!(modal.current_row(), Some(ContentRow::ConnectionAdd));
    }

    #[test]
    fn the_model_dropdown_lists_the_fetched_models_of_the_open_connection() {
        let mut modal = ai_modal(with_local());
        modal.open_connection("local".into());
        // Opening it asks the app for its models, once.
        let asked = modal.take_model_fetch_request().expect("a fetch request");
        assert_eq!(asked.model, "qwen");
        assert!(modal.take_model_fetch_request().is_none());

        // Auto (left to the provider) comes first.
        let options = modal.connection_enum_options(MODEL).unwrap();
        assert_eq!(options.values, ["", "qwen", MODEL_TYPE_SENTINEL]);
        assert_eq!(options.labels[0], i18n::t().settings_ai_model_auto());
        modal.set_model_options(vec!["a".into(), "qwen".into()]);
        let options = modal.connection_enum_options(MODEL).unwrap();
        assert_eq!(options.values, ["", "a", "qwen", MODEL_TYPE_SENTINEL]);
        assert_eq!(options.current, Some(2));
        modal.apply_connection_enum(MODEL, "");
        assert!(modal.config.ai.connections["local"].model.is_empty());
        assert_eq!(
            modal.connection_value(MODEL),
            i18n::t().settings_ai_model_auto()
        );
        assert_eq!(
            modal.connection_enum_options(MODEL).unwrap().current,
            Some(0)
        );

        // The escape types an id by hand, starting from the stored id rather
        // than the auto label.
        modal.open_enum_picker(MODEL);
        modal.enum_picker.as_mut().unwrap().cursor = 3;
        focus(&mut modal, ContentRow::Field(MODEL));
        modal.commit_enum_picker();
        assert!(modal.editing);
        assert_eq!(modal.edit_input.text(), "");
        modal.edit_input.set_text("typed");
        press(&mut modal, KeyCode::Enter);
        assert_eq!(modal.config.ai.connections["local"].model, "typed");

        // Another endpoint asks again; a CLI agent lists its own models.
        edit(&mut modal, BASE_URL, "http://other/v1");
        assert!(modal.take_model_fetch_request().is_some());
        modal.apply_connection_enum(PROVIDER, "codex");
        assert!(modal.take_model_fetch_request().is_none());
    }

    #[test]
    fn the_page_buttons_go_back_or_delete_by_key_and_by_click() {
        let [back, delete] = connection_buttons();
        let mut modal = ai_modal(with_local());
        modal.open_connection("local".into());
        let shown = screen(&mut modal);
        let y = shown.iter().position(|row| row.contains(&back)).unwrap();
        assert!(shown[y].contains(&delete), "side by side: {}", shown[y]);
        let click = |column: usize| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: column as u16,
            row: y as u16,
            modifiers: KeyModifiers::NONE,
        };
        let column_of = |row: &str, label: &str| row[..row.find(label).unwrap()].chars().count();

        // The gap between them does nothing; the back button leaves the page.
        let gap = column_of(&shown[y], &delete) - 1;
        modal.handle_mouse(click(gap), Rect::default()).unwrap();
        assert!(modal.connection_edit.is_some());
        modal
            .handle_mouse(click(column_of(&shown[y], &back) + 1), Rect::default())
            .unwrap();
        assert!(modal.connection_edit.is_none());
        assert_eq!(modal.current_row(), Some(ContentRow::Connection(0)));

        // From the keyboard the row starts on the back button.
        modal.open_connection("local".into());
        focus(&mut modal, ContentRow::ConnectionButtons);
        press(&mut modal, KeyCode::Enter);
        assert!(modal.connection_edit.is_none());
        assert!(modal.config.ai.connections.contains_key("local"));

        // A click on delete removes it.
        modal.open_connection("local".into());
        let shown = screen(&mut modal);
        modal
            .handle_mouse(click(column_of(&shown[y], &delete) + 1), Rect::default())
            .unwrap();
        assert!(modal.config.ai.connections.is_empty());
    }

    #[test]
    fn a_text_field_edits_with_selection_by_key_and_mouse() {
        let mut modal = ai_modal(with_local());
        modal.open_connection("local".into());
        focus(&mut modal, ContentRow::Field(BASE_URL));
        press(&mut modal, KeyCode::Enter);
        assert!(modal.editing);
        assert_eq!(modal.edit_input.text(), Connection::default().base_url);
        // Shift+Home selects back to the start; typing replaces the selection.
        let shift =
            |code| termide_core::KeyChord::identity(KeyEvent::new(code, KeyModifiers::SHIFT));
        modal.handle_key(shift(KeyCode::Home)).unwrap();
        type_text(&mut modal, "http://x/v1");
        assert_eq!(modal.edit_input.text(), "http://x/v1");

        // A press on the field places the cursor, a drag selects.
        let shown = screen(&mut modal);
        let area = modal.edit_area.expect("drawn as an input field");
        assert!(shown[area.y as usize].contains("http://x/v1"));
        let at = |kind, column: u16| MouseEvent {
            kind,
            column,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        };
        let down = at(MouseEventKind::Down(MouseButton::Left), area.x + 7);
        modal.handle_mouse(down, Rect::default()).unwrap();
        assert_eq!(modal.edit_input.cursor_pos(), 7);
        let drag = at(MouseEventKind::Drag(MouseButton::Left), area.x + 8);
        modal.handle_mouse(drag, Rect::default()).unwrap();
        assert_eq!(modal.edit_input.selected_text(), Some("x"));
        type_text(&mut modal, "host");
        press(&mut modal, KeyCode::Enter);
        assert_eq!(
            modal.config.ai.connections["local"].base_url,
            "http://host/v1"
        );

        // A number field keeps digits, pasted ones too.
        focus(&mut modal, ContentRow::Field(CONTEXT_WINDOW));
        press(&mut modal, KeyCode::Enter);
        assert!(modal.handle_paste("64 000 tokens"));
        type_text(&mut modal, "x");
        press(&mut modal, KeyCode::Enter);
        assert_eq!(
            modal.config.ai.connections["local"].context_window_fallback,
            Some(64_000)
        );
    }

    #[test]
    fn every_page_field_is_shown_for_an_endpoint() {
        let mut modal = ai_modal(with_local());
        modal.open_connection("local".into());
        let rows = modal.content_rows();
        for index in 0..connection_fields().len() {
            assert!(rows.contains(&ContentRow::Field(index)), "field {index}");
        }
        // The sidebar leaves the page.
        modal.preview_sidebar_row(crate::settings::SidebarRow::Leaf(SettingsTab::General));
        assert!(modal.connection_edit.is_none());
    }
}
