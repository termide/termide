//! Directory picker modal dialog with tree-view navigation.
//!
//! Supports expand/collapse of subdirectories (Right/Left arrows), toggling
//! hidden directories (`.`, as in the file manager), cursor rendering
//! matching the file manager panel style.
//!
//! Directories can be created, renamed and (when empty) deleted in place with
//! the file manager's `create_dir`, `rename` and `delete` keys. A name is
//! typed into the tree row itself; a new directory goes beside the entry
//! under the cursor, never inside it.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};

use crate::base::render_modal_block;
use crate::dir_tree::DirTree;
use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;

use termide_config::FileManagerKeybindings;
use termide_core::HotkeyTable;
use termide_theme::Theme;

use crate::{calculate_modal_width, centered_rect_with_size, Modal, ModalResult, ModalWidthConfig};

/// Button order in the bottom row.
const BUTTON_CONFIRM: usize = 0;
const BUTTON_NEW_FOLDER: usize = 1;
const BUTTON_CANCEL: usize = 2;

/// Directory picker modal window with tree navigation
#[derive(Debug)]
pub struct DirectoryPickerModal {
    title: String,
    tree: DirTree,
    button_focused: bool,
    selected_button: usize,
    last_buttons_area: Option<Rect>,
    create_label: String,
    new_folder_label: String,
    cancel_label: String,
    cached_width: Option<u16>,
    /// `create_dir`, `rename` and `delete`, as bound in the file manager.
    hotkeys: HotkeyTable,
}

const MAX_VISIBLE_ITEMS: usize = 15;

impl DirectoryPickerModal {
    /// Create a new directory picker modal with custom title and confirm button
    pub fn new(initial_dir: PathBuf, title: String, confirm_label: String) -> Self {
        let t = termide_i18n::t();
        Self {
            title,
            tree: DirTree::new(initial_dir, MAX_VISIBLE_ITEMS),
            button_focused: false,
            selected_button: 0,
            last_buttons_area: None,
            create_label: confirm_label,
            new_folder_label: t.directory_picker_new_folder().to_string(),
            cancel_label: t.directory_picker_cancel().to_string(),
            cached_width: None,
            hotkeys: picker_hotkeys(&{
                let mut kb = FileManagerKeybindings::default();
                kb.with_defaults();
                kb
            }),
        }
    }

    /// Use the file manager's keys as configured instead of the defaults.
    pub fn with_keybindings(mut self, keybindings: &FileManagerKeybindings) -> Self {
        self.hotkeys = picker_hotkeys(keybindings);
        self
    }

    /// Get the currently selected path
    fn selected_path(&self) -> PathBuf {
        self.tree.selected_path()
    }

    /// Open a name row for a new directory; the cursor leaves the buttons.
    fn start_create(&mut self) {
        self.button_focused = false;
        self.tree.start_create();
    }

    /// Start and width of each button, left to right, centred in `area`.
    fn button_layout(&self, area: Rect) -> [(u16, u16); 3] {
        let widths = [
            self.create_label.width() as u16 + 4,
            self.new_folder_label.width() as u16 + 4,
            self.cancel_label.width() as u16 + 4,
        ];
        let total = widths.iter().sum::<u16>() + 4;
        let mut x = area.x + area.width.saturating_sub(total) / 2;
        widths.map(|w| {
            let start = x;
            x += w + 2;
            (start, w)
        })
    }

    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        let title_width = self.title.len() as u16 + 4;
        let path_width = self.tree.dir().to_string_lossy().len() as u16 + 5;

        let buttons_width = (self.create_label.width()
            + self.new_folder_label.width()
            + self.cancel_label.width()) as u16
            + 18;

        calculate_modal_width(
            [
                title_width,
                path_width,
                self.tree.content_width() as u16 + 2,
                buttons_width,
            ]
            .into_iter(),
            screen_width,
            ModalWidthConfig::wide(),
        )
    }

    fn get_modal_width(&mut self, screen_width: u16) -> u16 {
        if self.tree.take_reloaded() {
            self.cached_width = None;
        }
        if let Some(width) = self.cached_width {
            return width;
        }
        let width = self.calculate_modal_width(screen_width);
        self.cached_width = Some(width);
        width
    }
}

impl Modal for DirectoryPickerModal {
    type Result = PathBuf;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let modal_width = self.get_modal_width(area.width);

        let visible_count = self.tree.visible_len().clamp(3, MAX_VISIBLE_ITEMS);
        let list_height = visible_count as u16;
        let modal_height = 6 + list_height;

        let modal_area = centered_rect_with_size(modal_width, modal_height, area);
        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // Path line: an error if there is one, else the path the edited
        // name or the cursor stands for (truncated on the left).
        if let Some(message) = &self.tree.message {
            let text = termide_ui::path_utils::truncate_left(message, inner.width as usize);
            buf.set_string(inner.x, inner.y, &text, Style::default().fg(theme.error));
        } else {
            let selected = self.tree.shown_path();
            let path_str = selected.to_string_lossy();
            let display_path =
                termide_ui::path_utils::truncate_left(&path_str, inner.width as usize);
            buf.set_string(
                inner.x,
                inner.y,
                &display_path,
                Style::default().fg(theme.fg),
            );
        }

        // Separator
        let sep_y = inner.y + 1;
        let sep_style = Style::default().fg(theme.accented_bg);
        for x in inner.x..inner.x + inner.width {
            buf[(x, sep_y)].set_char('─').set_style(sep_style);
        }

        // Tree list
        let list_area = Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: list_height,
        };
        self.tree
            .render(list_area, buf, theme, !self.button_focused);

        // Separator before buttons
        let sep2_y = list_area.y + list_area.height;
        for x in inner.x..inner.x + inner.width {
            buf[(x, sep2_y)].set_char('─').set_style(sep_style);
        }

        // Buttons
        let buttons_y = sep2_y + 1;
        let buttons_area = Rect {
            x: inner.x,
            y: buttons_y,
            width: inner.width,
            height: 1,
        };
        self.last_buttons_area = Some(buttons_area);

        let labels = [
            &self.create_label,
            &self.new_folder_label,
            &self.cancel_label,
        ];
        for (i, (label, (x, _))) in labels
            .into_iter()
            .zip(self.button_layout(buttons_area))
            .enumerate()
        {
            let style = if self.button_focused && self.selected_button == i {
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            buf.set_string(x, buttons_y, format!("[ {} ]", label), style);
        }
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        self.tree.message = None;
        if self.tree.is_editing() {
            self.tree.handle_edit_key(chord.raw);
            return Ok(None);
        }

        // Outside a name field letter keys are shortcuts, matched on the
        // layout-normalized form so they work on a Cyrillic layout too.
        let key = chord.canonical;
        if self.hotkeys.matches_canonical("create_dir", &key) {
            self.start_create();
            return Ok(None);
        }
        if self.hotkeys.matches_canonical("rename", &key) {
            self.tree.start_rename();
            if self.tree.is_editing() {
                self.button_focused = false;
            }
            return Ok(None);
        }
        if self.hotkeys.matches_canonical("delete", &key) {
            self.tree.delete_under_cursor();
            return Ok(None);
        }
        match key.code {
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            KeyCode::Char('.') => {
                self.tree.toggle_hidden();
                Ok(None)
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.button_focused = !self.button_focused;
                Ok(None)
            }
            // List navigation
            KeyCode::Up | KeyCode::Char('k') if !self.button_focused => {
                self.tree.cursor_up();
                Ok(None)
            }
            KeyCode::Down | KeyCode::Char('j') if !self.button_focused => {
                self.tree.cursor_down();
                Ok(None)
            }
            KeyCode::Home if !self.button_focused => {
                self.tree.cursor_home();
                Ok(None)
            }
            KeyCode::End if !self.button_focused => {
                self.tree.cursor_end();
                Ok(None)
            }
            // Expand directory
            KeyCode::Right | KeyCode::Char('l') if !self.button_focused => {
                self.tree.expand();
                Ok(None)
            }
            // Collapse directory or jump to parent in tree
            KeyCode::Left | KeyCode::Char('h') if !self.button_focused => {
                self.tree.collapse_or_parent();
                Ok(None)
            }
            // Button navigation
            KeyCode::Left if self.button_focused => {
                self.selected_button = self.selected_button.saturating_sub(1);
                Ok(None)
            }
            KeyCode::Right if self.button_focused => {
                self.selected_button = (self.selected_button + 1).min(BUTTON_CANCEL);
                Ok(None)
            }
            // Go to parent directory (reload tree root)
            KeyCode::Backspace if !self.button_focused => {
                self.tree.go_parent();
                Ok(None)
            }
            // Ctrl+Enter to confirm from anywhere
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Ok(Some(ModalResult::Confirmed(self.selected_path())))
            }
            KeyCode::Enter => {
                if self.button_focused {
                    match self.selected_button {
                        BUTTON_CONFIRM => Ok(Some(ModalResult::Confirmed(self.selected_path()))),
                        BUTTON_NEW_FOLDER => {
                            self.start_create();
                            Ok(None)
                        }
                        _ => Ok(Some(ModalResult::Cancelled)),
                    }
                } else {
                    // Enter = cd into directory (reload tree from selected dir)
                    self.tree.enter();
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        // A name being typed keeps its row; Enter or Esc ends it first.
        if self.tree.is_editing() {
            self.tree.handle_edit_mouse(&mouse);
            return Ok(None);
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                for _ in 0..3 {
                    self.tree.cursor_up();
                }
                return Ok(None);
            }
            MouseEventKind::ScrollDown => {
                for _ in 0..3 {
                    self.tree.cursor_down();
                }
                return Ok(None);
            }
            _ => {}
        }

        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(None);
        }

        // Check buttons
        if let Some(buttons_area) = self.last_buttons_area {
            if mouse.row == buttons_area.y
                && mouse.column >= buttons_area.x
                && mouse.column < buttons_area.x + buttons_area.width
            {
                let hit = self
                    .button_layout(buttons_area)
                    .iter()
                    .position(|&(x, w)| mouse.column >= x && mouse.column < x + w);
                match hit {
                    Some(BUTTON_CONFIRM) => {
                        return Ok(Some(ModalResult::Confirmed(self.selected_path())));
                    }
                    Some(BUTTON_NEW_FOLDER) => {
                        self.start_create();
                        return Ok(None);
                    }
                    Some(_) => return Ok(Some(ModalResult::Cancelled)),
                    None => {}
                }
            }
        }

        // Check list click
        if self.tree.click(mouse.column, mouse.row) {
            self.button_focused = false;
        }
        Ok(None)
    }

    /// A paste goes into the name being typed, on one line.
    fn handle_paste(&mut self, text: &str) -> bool {
        self.tree.paste(text)
    }
}

/// The picker's share of the file manager keys.
fn picker_hotkeys(keybindings: &FileManagerKeybindings) -> HotkeyTable {
    let mut table = HotkeyTable::new();
    table.insert("create_dir", &keybindings.create_dir);
    table.insert("rename", &keybindings.rename);
    table.insert("delete", &keybindings.delete);
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn scratch(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("termide-dir-picker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in [".git/objects", "src/bin", "target"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        root
    }

    fn names(modal: &DirectoryPickerModal) -> Vec<&str> {
        modal
            .tree
            .visible_indices
            .iter()
            .map(|&i| modal.tree.entries[i].name.as_str())
            .collect()
    }

    fn press(modal: &mut DirectoryPickerModal, code: KeyCode) {
        let key = KeyEvent::new(code, KeyModifiers::NONE);
        modal
            .handle_key(termide_core::KeyChord::identity(key))
            .unwrap();
    }

    #[test]
    fn a_click_places_the_cursor_and_a_drag_selects_in_the_rename_field() {
        use crate::base::test_mouse::{drag, press as down, release};
        let root = scratch("mouse");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        modal.tree.cursor = 2; // `target`
        modal.tree.start_rename();
        let area = Rect::new(0, 0, 100, 40);
        modal.render(area, &mut Buffer::empty(area), &Theme::default());
        let field = modal
            .tree
            .edit
            .as_ref()
            .and_then(|e| e.area)
            .expect("drawn");
        modal
            .handle_mouse(down(field.x + 2, field.y), area)
            .unwrap();
        let input = &modal.tree.edit.as_ref().unwrap().input;
        assert_eq!(input.cursor_pos(), 2);
        // Down over the next rows the drag keeps selecting in the field.
        modal
            .handle_mouse(drag(field.x + 5, field.y + 2), area)
            .unwrap();
        assert_eq!(
            modal.tree.edit.as_ref().unwrap().input.selected_text(),
            Some("rge")
        );
        modal.handle_mouse(release(field.x, field.y), area).unwrap();
        assert!(modal.tree.edit.is_some(), "still renaming");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dot_toggles_hidden_directories() {
        let root = scratch("toggle");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        assert_eq!(names(&modal), ["..", "src", "target"]);

        press(&mut modal, KeyCode::Char('.'));
        assert_eq!(names(&modal), ["..", ".git", "src", "target"]);

        press(&mut modal, KeyCode::Char('.'));
        assert_eq!(names(&modal), ["..", "src", "target"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn toggling_hidden_keeps_expanded_branches_and_the_cursor() {
        let root = scratch("keep");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        // Expand `src` and step onto `bin` inside it.
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::Right);
        press(&mut modal, KeyCode::Down);
        assert_eq!(modal.selected_path(), root.join("src/bin"));

        press(&mut modal, KeyCode::Char('.'));
        assert_eq!(names(&modal), ["..", ".git", "src", "bin", "target"]);
        assert_eq!(modal.selected_path(), root.join("src/bin"));

        // Hiding `.git` with the cursor inside it falls back to the list top.
        press(&mut modal, KeyCode::Up);
        press(&mut modal, KeyCode::Up);
        press(&mut modal, KeyCode::Right);
        press(&mut modal, KeyCode::Down);
        assert_eq!(modal.selected_path(), root.join(".git/objects"));
        press(&mut modal, KeyCode::Char('.'));
        assert_eq!(modal.tree.cursor, 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn type_text(modal: &mut DirectoryPickerModal, text: &str) {
        for c in text.chars() {
            press(modal, KeyCode::Char(c));
        }
    }

    #[test]
    fn new_directory_goes_beside_the_entry_under_the_cursor() {
        let root = scratch("create-sibling");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        // Expand `src` and step onto `bin` inside it.
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::Right);
        press(&mut modal, KeyCode::Down);

        press(&mut modal, KeyCode::F(7));
        type_text(&mut modal, "lib");
        press(&mut modal, KeyCode::Enter);

        assert!(root.join("src/lib").is_dir());
        assert!(modal.tree.edit.is_none());
        assert_eq!(names(&modal), ["..", "src", "bin", "lib", "target"]);
        assert_eq!(modal.selected_path(), root.join("src/lib"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_directory_on_an_expanded_branch_lands_beside_it_not_inside() {
        let root = scratch("create-expanded");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::Right);

        press(&mut modal, KeyCode::F(7));
        type_text(&mut modal, "docs");
        press(&mut modal, KeyCode::Enter);

        assert!(root.join("docs").is_dir());
        assert!(!root.join("src/docs").exists());
        assert_eq!(names(&modal), ["..", "docs", "src", "bin", "target"]);
        assert_eq!(modal.selected_path(), root.join("docs"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_directory_on_dotdot_lands_at_the_top_level() {
        let root = scratch("create-dotdot");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        press(&mut modal, KeyCode::F(7));
        type_text(&mut modal, "app");
        press(&mut modal, KeyCode::Enter);

        assert!(root.join("app").is_dir());
        assert_eq!(names(&modal), ["..", "app", "src", "target"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn esc_takes_the_new_row_away_and_creates_nothing() {
        let root = scratch("create-cancel");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::Right);
        press(&mut modal, KeyCode::Down);

        press(&mut modal, KeyCode::F(7));
        assert_eq!(names(&modal), ["..", "src", "bin", "", "target"]);
        type_text(&mut modal, "x");
        press(&mut modal, KeyCode::Esc);

        assert!(modal.tree.edit.is_none());
        assert_eq!(names(&modal), ["..", "src", "bin", "target"]);
        assert_eq!(modal.selected_path(), root.join("src/bin"));
        assert!(!root.join("src/x").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn letters_in_the_name_field_are_typed_not_shortcuts() {
        let root = scratch("create-letters");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        press(&mut modal, KeyCode::F(7));
        // `D` creates, `j`/`h` move, `.` toggles hidden outside the field.
        type_text(&mut modal, "Dj.h");
        press(&mut modal, KeyCode::Enter);

        assert!(root.join("Dj.h").is_dir());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_bad_or_taken_name_keeps_the_field_open() {
        let root = scratch("create-invalid");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        press(&mut modal, KeyCode::F(7));

        press(&mut modal, KeyCode::Enter);
        assert!(modal.tree.message.is_some());
        assert!(modal.tree.edit.is_some());

        type_text(&mut modal, "a/b");
        press(&mut modal, KeyCode::Enter);
        assert!(modal.tree.message.is_some());
        assert!(!root.join("a").exists());

        for _ in 0..3 {
            press(&mut modal, KeyCode::Backspace);
        }
        type_text(&mut modal, "target");
        press(&mut modal, KeyCode::Enter);
        assert!(modal.tree.message.is_some());
        assert!(modal.tree.edit.is_some());

        // The message lasts until the next key.
        press(&mut modal, KeyCode::Esc);
        assert!(modal.tree.message.is_none());
        assert_eq!(names(&modal), ["..", "src", "target"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rename_keeps_the_branch_open_under_the_new_name() {
        let root = scratch("rename");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::Right);

        press(&mut modal, KeyCode::F(2));
        for _ in 0.."src".len() {
            press(&mut modal, KeyCode::Backspace);
        }
        type_text(&mut modal, "source");
        press(&mut modal, KeyCode::Enter);

        assert!(root.join("source/bin").is_dir());
        assert!(!root.join("src").exists());
        assert_eq!(names(&modal), ["..", "source", "bin", "target"]);
        assert_eq!(modal.selected_path(), root.join("source"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rename_on_dotdot_does_nothing() {
        let root = scratch("rename-dotdot");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        press(&mut modal, KeyCode::F(2));
        assert!(modal.tree.edit.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_removes_only_an_empty_directory() {
        let root = scratch("delete");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        // `src` holds `bin`: refused with a message.
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::Delete);
        assert!(root.join("src/bin").is_dir());
        assert!(modal.tree.message.is_some());

        // `target` is empty: gone, the cursor stays on the row it was on.
        press(&mut modal, KeyCode::Down);
        press(&mut modal, KeyCode::F(8));
        assert!(!root.join("target").exists());
        assert_eq!(names(&modal), ["..", "src"]);
        assert_eq!(modal.selected_path(), root.join("src"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_folder_button_opens_the_name_row() {
        let root = scratch("button");
        let mut modal = DirectoryPickerModal::new(root.clone(), String::new(), String::new());
        press(&mut modal, KeyCode::Tab);
        press(&mut modal, KeyCode::Right);
        press(&mut modal, KeyCode::Enter);
        assert!(modal.tree.edit.is_some());
        assert!(!modal.button_focused);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_name_row_and_three_buttons_are_drawn() {
        let root = scratch("render");
        let mut modal = DirectoryPickerModal::new(root.clone(), "T".into(), "Go".into());
        press(&mut modal, KeyCode::F(7));
        type_text(&mut modal, "newdir");

        let area = Rect::new(0, 0, 80, 30);
        let mut buf = Buffer::empty(area);
        modal.render(area, &mut buf, &Theme::default());
        let screen: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    + "\n"
            })
            .collect();

        assert!(screen.contains("▶ newdir"), "{screen}");
        // The path line (cut on the left when long) ends in the typed name.
        assert!(screen.contains("/newdir"), "{screen}");
        assert!(screen.contains("[ Go ]"));
        assert!(screen.contains("[ New folder ]"));
        assert!(screen.contains("[ Cancel ]"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
