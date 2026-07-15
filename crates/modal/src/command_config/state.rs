//! Command-config modal state model: construction, sizing, focus order,
//! hotkey validation, and confirmation logic.

use crate::{ModalResult, TextInputHandler};
use termide_config::commands::{CommandMetadata, CommandMode};
use termide_config::constants::{
    MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT, MODAL_MIN_WIDTH_WIDE, MODAL_PADDING_WITH_DOUBLE_BORDER,
};
use termide_i18n as i18n;
use termide_ui::SuggestionInput;

use super::{
    sanitize_filename, CommandConfigAction, CommandConfigModal, CommandConfigMode,
    CommandConfigResult, FocusArea, ReservedHotkey,
};

impl CommandConfigModal {
    /// Create a new modal in Create mode.
    pub fn new_create(title: impl Into<String>, existing_groups: Vec<String>) -> Self {
        Self {
            title: title.into(),
            mode: CommandConfigMode::Create,
            focus: FocusArea::Group,
            command_name: String::new(),
            group_suggestion: SuggestionInput::new(existing_groups),
            command_input: TextInputHandler::new(),
            display_name_input: TextInputHandler::new(),
            command_mode: CommandMode::Terminal,
            hotkey_input: TextInputHandler::new(),
            is_project: false,
            selected_button: 0,
            hotkey_error: false,
            hotkey_conflict: None,
            reserved_hotkeys: Vec::new(),
            last_buttons_area: None,
            last_group_field_area: None,
            last_group_dropdown_area: None,
            last_command_area: None,
            last_display_name_area: None,
            last_mode_area: None,
            last_hotkey_area: None,
            last_checkbox_area: None,
        }
    }

    /// Create a modal in Edit mode, pre-populated from existing metadata.
    pub fn new_edit(
        title: impl Into<String>,
        command_name: String,
        group: Option<String>,
        is_project: bool,
        _path: Option<std::path::PathBuf>,
        existing_groups: Vec<String>,
        metadata: Option<CommandMetadata>,
    ) -> Self {
        let display_name_text = metadata
            .as_ref()
            .and_then(|m| m.display_name.clone())
            .unwrap_or_default();
        let mut display_name_input = TextInputHandler::new();
        if !display_name_text.is_empty() {
            display_name_input.set_text(&display_name_text);
        }

        let command_mode = metadata.as_ref().and_then(|m| m.mode).unwrap_or_default();

        let hotkey_text = metadata
            .as_ref()
            .and_then(|m| m.key.clone())
            .unwrap_or_default();
        let mut hotkey_input = TextInputHandler::new();
        if !hotkey_text.is_empty() {
            hotkey_input.set_text(&hotkey_text);
        }

        let command_text = metadata
            .as_ref()
            .and_then(|m| m.command.clone())
            .unwrap_or_default();
        let mut command_input = TextInputHandler::new();
        if !command_text.is_empty() {
            command_input.set_text(&command_text);
        }

        let mut group_suggestion = SuggestionInput::new(existing_groups);
        if let Some(group) = group {
            group_suggestion.input_mut().set_text(&group);
        }

        Self {
            title: title.into(),
            mode: CommandConfigMode::Edit,
            focus: FocusArea::Group,
            command_name,
            group_suggestion,
            command_input,
            display_name_input,
            command_mode,
            hotkey_input,
            is_project,
            selected_button: 0,
            hotkey_error: false,
            hotkey_conflict: None,
            reserved_hotkeys: Vec::new(),
            last_buttons_area: None,
            last_group_field_area: None,
            last_group_dropdown_area: None,
            last_command_area: None,
            last_display_name_area: None,
            last_mode_area: None,
            last_hotkey_area: None,
            last_checkbox_area: None,
        }
    }

    pub(super) fn is_create(&self) -> bool {
        self.mode == CommandConfigMode::Create
    }

    pub fn with_reserved_hotkeys(mut self, reserved_hotkeys: Vec<ReservedHotkey>) -> Self {
        self.reserved_hotkeys = reserved_hotkeys
            .into_iter()
            .filter_map(|item| termide_config::parse_keybinding(&item.binding).ok())
            .collect();
        self.validate_hotkey();
        self
    }

    pub(super) fn button_count(&self) -> usize {
        2 // Save/Create, Cancel
    }

    pub(super) fn button_label(&self, index: usize) -> &'static str {
        let t = i18n::t();
        if self.is_create() {
            match index {
                0 => t.command_config_button_create(),
                1 => t.command_config_button_cancel(),
                _ => "",
            }
        } else {
            match index {
                0 => t.command_config_button_save(),
                1 => t.command_config_button_cancel(),
                _ => "",
            }
        }
    }

    fn focus_order() -> &'static [FocusArea] {
        &[
            FocusArea::Group,
            FocusArea::DisplayName,
            FocusArea::Command,
            FocusArea::Mode,
            FocusArea::Hotkey,
            FocusArea::ProjectCheckbox,
            FocusArea::Buttons,
        ]
    }

    pub(super) fn calculate_modal_size(&self, screen_width: u16, screen_height: u16) -> (u16, u16) {
        let title_width = self.title.len() as u16 + 4;
        let label_width = 15u16;
        let input_width = 48u16;

        let content_width = title_width.max(label_width + input_width).max(30);
        let total_width = content_width + MODAL_PADDING_WITH_DOUBLE_BORDER;

        let max_width = (screen_width as f32 * MODAL_MAX_WIDTH_PERCENTAGE_DEFAULT) as u16;
        let width = total_width
            .max(MODAL_MIN_WIDTH_WIDE)
            .min(max_width)
            .min(screen_width);

        let suggestions = self.group_suggestion.suggestions();
        let dropdown_height =
            if self.is_create() && self.group_suggestion.is_expanded() && !suggestions.is_empty() {
                suggestions.len().min(5) as u16 + 1
            } else {
                0
            };

        // Create: Border(1) + Group(3) + [Dropdown] + Command(3) + DisplayName(3) + Mode(2) + Hotkey(3) + [HotkeyError(1)] + Checkbox(1) + Empty(1) + Buttons(1) + Border(1)
        // Edit:   Border(1) + Group(3) + DisplayName(3) + Command(3) + Mode(2) + Hotkey(3) + [HotkeyError(1)] + Checkbox(1) + Empty(1) + Buttons(1) + Border(1)
        let mut height = if self.is_create() {
            1 + 3 + dropdown_height + 3 + 3 + 2 + 3
        } else {
            1 + 3 + 3 + 3 + 2 + 3
        };
        if self.hotkey_error || self.hotkey_conflict.is_some() {
            height += 1;
        }
        height += 4;
        height = height.min(screen_height);

        (width, height)
    }

    pub(super) fn next_focus(&mut self) {
        self.group_suggestion.collapse();
        let order = Self::focus_order();
        if let Some(idx) = order.iter().position(|f| *f == self.focus) {
            self.focus = order[(idx + 1) % order.len()];
        }
    }

    pub(super) fn prev_focus(&mut self) {
        self.group_suggestion.collapse();
        let order = Self::focus_order();
        if let Some(idx) = order.iter().position(|f| *f == self.focus) {
            self.focus = order[(idx + order.len() - 1) % order.len()];
        }
    }

    pub(super) fn try_confirm(&self) -> Option<ModalResult<CommandConfigResult>> {
        if self.hotkey_error || self.hotkey_conflict.is_some() {
            return None;
        }
        if self.is_create() {
            let command = {
                let c = self.command_input.text().trim().to_string();
                if c.is_empty() {
                    return None;
                } else {
                    Some(c)
                }
            };
            // Derive name from display_name or command
            let display_name = {
                let d = self.display_name_input.text().trim().to_string();
                if d.is_empty() {
                    None
                } else {
                    Some(d)
                }
            };
            let name = sanitize_filename(
                display_name
                    .as_deref()
                    .unwrap_or_else(|| command.as_deref().unwrap_or("")),
            );
            if name.is_empty() {
                return None;
            }
            let group = {
                let g = sanitize_filename(self.group_suggestion.text().trim());
                if g.is_empty() {
                    None
                } else {
                    Some(g)
                }
            };
            Some(ModalResult::Confirmed(CommandConfigResult {
                name,
                command,
                display_name,
                group,
                mode: self.command_mode,
                hotkey: self.hotkey_value(),
                is_project: self.is_project,
                action: CommandConfigAction::Save,
                is_edit: false,
            }))
        } else {
            let command = {
                let c = self.command_input.text().trim().to_string();
                if c.is_empty() {
                    None
                } else {
                    Some(c)
                }
            };
            let display_name = {
                let d = self.display_name_input.text().trim().to_string();
                if d.is_empty() {
                    None
                } else {
                    Some(d)
                }
            };
            Some(ModalResult::Confirmed(CommandConfigResult {
                name: self.command_name.clone(),
                command,
                display_name,
                group: {
                    let g = sanitize_filename(self.group_suggestion.text().trim());
                    if g.is_empty() {
                        None
                    } else {
                        Some(g)
                    }
                },
                mode: self.command_mode,
                hotkey: self.hotkey_value(),
                is_project: self.is_project,
                action: CommandConfigAction::Save,
                is_edit: true,
            }))
        }
    }

    fn hotkey_value(&self) -> Option<String> {
        let h = self.hotkey_input.text().trim().to_string();
        if h.is_empty() {
            None
        } else {
            Some(h)
        }
    }

    pub(super) fn validate_hotkey(&mut self) {
        let text = self.hotkey_input.text().trim();
        self.hotkey_error = !text.is_empty() && !is_valid_hotkey(text);
        self.hotkey_conflict = None;
        if self.hotkey_error || text.is_empty() {
            return;
        }
        let Ok(parsed) = termide_config::parse_keybinding(text) else {
            return;
        };
        if self.reserved_hotkeys.contains(&parsed) {
            self.hotkey_conflict = Some(i18n::t().command_config_hotkey_conflict().to_string());
        }
    }

    pub(super) fn mode_label(mode: CommandMode) -> &'static str {
        let t = i18n::t();
        match mode {
            CommandMode::Terminal => t.command_config_mode_terminal(),
            CommandMode::Background => t.command_config_mode_background(),
            CommandMode::Report => t.command_config_mode_report(),
        }
    }
}

/// Basic hotkey string validation: [Ctrl+][Alt+][Shift+]Key
fn is_valid_hotkey(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    let parts: Vec<&str> = s.split('+').collect();
    if parts.is_empty() {
        return false;
    }
    let modifiers = &parts[..parts.len() - 1];
    let key = parts.last().unwrap();

    const VALID_MODS: &[&str] = &["Ctrl", "Alt", "Shift"];
    const VALID_KEYS: &[&str] = &[
        "A",
        "B",
        "C",
        "D",
        "E",
        "F",
        "G",
        "H",
        "I",
        "J",
        "K",
        "L",
        "M",
        "N",
        "O",
        "P",
        "Q",
        "R",
        "S",
        "T",
        "U",
        "V",
        "W",
        "X",
        "Y",
        "Z",
        "0",
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        "F1",
        "F2",
        "F3",
        "F4",
        "F5",
        "F6",
        "F7",
        "F8",
        "F9",
        "F10",
        "F11",
        "F12",
        "Enter",
        "Tab",
        "Esc",
        "Space",
        "Backspace",
        "Delete",
        "Insert",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "Left",
        "Right",
        "Up",
        "Down",
    ];

    modifiers.iter().all(|m| VALID_MODS.contains(m)) && VALID_KEYS.contains(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Modal;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::Once;

    static INIT_I18N: Once = Once::new();

    fn setup_i18n() {
        INIT_I18N.call_once(|| {
            let _ = i18n::init();
        });
    }

    fn edit_modal(is_project: bool) -> CommandConfigModal {
        CommandConfigModal::new_edit(
            "Edit command",
            "build".to_string(),
            Some("dev".to_string()),
            is_project,
            None,
            vec!["dev".to_string(), "qa".to_string()],
            None,
        )
    }

    #[test]
    fn edit_mode_focus_includes_project_checkbox() {
        let mut modal = edit_modal(false);
        modal.focus = FocusArea::Hotkey;
        modal.next_focus();
        assert_eq!(modal.focus, FocusArea::ProjectCheckbox);
        modal.next_focus();
        assert_eq!(modal.focus, FocusArea::Buttons);
    }

    #[test]
    fn edit_mode_space_toggles_project_checkbox() {
        let mut modal = edit_modal(false);
        modal.focus = FocusArea::ProjectCheckbox;
        let result = modal
            .handle_key(termide_core::KeyChord::identity(KeyEvent::new(
                KeyCode::Char(' '),
                KeyModifiers::empty(),
            )))
            .expect("space should be handled");
        assert!(result.is_none());
        assert!(modal.is_project);
    }

    #[test]
    fn edit_mode_confirm_returns_updated_project_flag() {
        let mut modal = edit_modal(false);
        modal.focus = FocusArea::ProjectCheckbox;
        modal.is_project = true;
        let result = modal.try_confirm().expect("edit modal should confirm");
        let ModalResult::Confirmed(result) = result else {
            panic!("expected confirmed result");
        };
        assert!(result.is_project);
        assert!(result.is_edit);
    }

    #[test]
    fn edit_mode_confirm_returns_updated_group() {
        let mut modal = edit_modal(false);
        modal.group_suggestion.input_mut().set_text("qa");
        let result = modal.try_confirm().expect("edit modal should confirm");
        let ModalResult::Confirmed(result) = result else {
            panic!("expected confirmed result");
        };
        assert_eq!(result.group.as_deref(), Some("qa"));
    }

    #[test]
    fn reserved_hotkey_conflict_is_detected() {
        setup_i18n();
        let mut modal = CommandConfigModal::new_create("Create command", vec![])
            .with_reserved_hotkeys(vec![ReservedHotkey {
                binding: "Ctrl+B".to_string(),
            }]);
        modal.hotkey_input.set_text("Ctrl+B");
        modal.validate_hotkey();
        assert_eq!(
            modal.hotkey_conflict.as_deref(),
            Some(i18n::t().command_config_hotkey_conflict())
        );
        modal.command_input.set_text("cargo build");
        assert!(modal.try_confirm().is_none());
    }
}
