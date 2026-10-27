//! A path field whose dropdown is a directory tree.
//!
//! Collapsed it is a one-line field with `▼` at its right end; Tab or a
//! press on the arrow opens the tree under it. While the field still holds
//! its first value the tree lists the quick destinations (the other panels'
//! directories) as roots; once the path is edited it lists the directory
//! the path names up to its last `/`, filtered fuzzily by what follows.
//!
//! Down moves into the tree. Moving the cursor there writes the directory
//! under it into the field, keeping the file name when the field names a
//! file. Right and Left expand and collapse; the file manager's keys create,
//! rename and delete directories and show hidden ones. Typing returns to the
//! field. Enter closes the tree keeping the path, Esc closes it restoring
//! the path it opened with.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    widgets::{Block, Borders, Widget},
};
use std::path::{Path, PathBuf};

use crate::base::{drag_field, press_field_box, render_text_input};
use crate::dir_tree::DirTree;
use crate::input_keys::{handle_input_key, InputKeyResult};
use crate::TextInputHandler;
use termide_config::FileManagerKeybindings;
use termide_core::{HotkeyTable, KeyChord};
use termide_theme::Theme;
use termide_ui::path_utils::{expand_tilde, truncate_left};

/// Rows of the tree under the field while it is open.
pub(crate) const DROPDOWN_ROWS: usize = 10;

/// What the field made of a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FieldKey {
    /// The key was used.
    Handled,
    /// The key is the host's: Enter, Down or Tab on a closed field, Esc on
    /// a closed one, and keys the field has no use for.
    NotHandled,
}

#[derive(Debug)]
pub(crate) struct PathField {
    input: TextInputHandler,
    /// The value the field opened with; while it holds it, the tree lists
    /// the quick destinations.
    initial: String,
    /// Quick destinations: directories (or remote URLs) as typed.
    roots: Vec<String>,
    /// Where a relative path is taken from.
    base_dir: PathBuf,
    /// The field names a file in a directory: picking a directory keeps the
    /// name after the last `/`.
    keeps_name: bool,
    /// The tree, while it is open.
    tree: Option<DirTree>,
    /// The cursor is in the tree rather than the field.
    in_tree: bool,
    /// The text the tree opened with, restored by Esc.
    snapshot: String,
    /// `create_dir`, `rename`, `delete` and `toggle_hidden` from the file
    /// manager.
    hotkeys: HotkeyTable,
    /// The field's text as last drawn, for the mouse.
    last_text_area: Option<Rect>,
    /// The field's box as last drawn, the arrow included.
    last_box_area: Option<Rect>,
    /// The tree's area as last drawn, inside its border.
    last_tree_area: Option<Rect>,
    /// A press landed in the text and the button is still down.
    dragging: bool,
}

impl PathField {
    pub(crate) fn new(text: impl Into<String>, roots: Vec<String>) -> Self {
        let text = text.into();
        let base_dir = Path::new(&text)
            .parent()
            .filter(|p| p.is_absolute())
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        Self {
            input: TextInputHandler::with_text(text.clone()),
            snapshot: text.clone(),
            initial: text,
            roots,
            base_dir,
            keeps_name: false,
            tree: None,
            in_tree: false,
            hotkeys: field_hotkeys(&{
                let mut kb = FileManagerKeybindings::default();
                kb.with_defaults();
                kb
            }),
            last_text_area: None,
            last_box_area: None,
            last_tree_area: None,
            dragging: false,
        }
    }

    /// Offer `roots` as the quick destinations.
    pub(crate) fn set_roots(&mut self, roots: Vec<String>) {
        self.roots = roots;
    }

    /// Take a relative path from `dir`.
    pub(crate) fn set_base_dir(&mut self, dir: PathBuf) {
        self.base_dir = dir;
    }

    /// Keep the file name after the last `/` when a directory is picked.
    pub(crate) fn set_keeps_name(&mut self, keeps: bool) {
        self.keeps_name = keeps;
    }

    /// Use the file manager's keys as configured instead of the defaults.
    pub(crate) fn set_keybindings(&mut self, keybindings: &FileManagerKeybindings) {
        self.hotkeys = field_hotkeys(keybindings);
    }

    pub(crate) fn text(&self) -> &str {
        self.input.text()
    }

    pub(crate) fn input_mut(&mut self) -> &mut TextInputHandler {
        &mut self.input
    }

    /// Replace the text, as if typed.
    pub(crate) fn set_text(&mut self, text: String) {
        self.input.set_text(text);
        self.sync_tree();
    }

    /// The text was edited through `input_mut`: the tree follows it.
    pub(crate) fn text_edited(&mut self) {
        self.in_tree = false;
        self.sync_tree();
    }

    /// The field's text as last drawn.
    #[cfg(test)]
    pub(crate) fn text_area(&self) -> Option<Rect> {
        self.last_text_area
    }

    pub(crate) fn is_open(&self) -> bool {
        self.tree.is_some()
    }

    /// Lines the field takes: its box, and the tree under it while open.
    pub(crate) fn height(&self) -> u16 {
        if self.is_open() {
            2 + DROPDOWN_ROWS as u16 + 1
        } else {
            3
        }
    }

    /// Open the tree on the path in the field.
    pub(crate) fn open(&mut self) {
        if self.tree.is_some() {
            return;
        }
        self.snapshot = self.input.text().to_string();
        self.tree = Some(DirTree::new(self.base_dir.clone(), DROPDOWN_ROWS));
        self.sync_tree();
    }

    /// Close the tree keeping the path.
    pub(crate) fn close(&mut self) {
        self.tree = None;
        self.in_tree = false;
    }

    /// Close the tree and restore the path it opened with.
    fn roll_back(&mut self) {
        let snapshot = std::mem::take(&mut self.snapshot);
        self.input.set_text(snapshot);
        self.close();
    }

    fn toggle(&mut self) {
        if self.is_open() {
            self.close();
        } else {
            self.open();
        }
    }

    /// The directory the text names up to its last `/`, made absolute, and
    /// what follows that `/`.
    fn split_text(&self) -> (PathBuf, String) {
        let text = self.input.text();
        let (dir, tail) = match text.rfind('/') {
            Some(i) => (&text[..=i], &text[i + 1..]),
            None => ("", text),
        };
        let dir = if dir.is_empty() {
            self.base_dir.clone()
        } else {
            let dir = expand_tilde(dir);
            if dir.is_absolute() {
                dir
            } else {
                self.base_dir.join(dir)
            }
        };
        (dir, tail.to_string())
    }

    /// Make the tree list what the text asks for: the quick destinations
    /// while the text is the first value, else the directory it names
    /// (its closest existing ancestor) filtered by the name being typed.
    fn sync_tree(&mut self) {
        if self.tree.is_none() {
            return;
        }
        let untouched = self.input.text() == self.initial && !self.roots.is_empty();
        let (dir, tail) = self.split_text();
        let roots = self.roots.clone();
        let keeps_name = self.keeps_name;
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        if untouched {
            if !tree.has_roots() {
                tree.set_roots(
                    roots
                        .iter()
                        .map(|r| (r.clone(), PathBuf::from(r.trim_end_matches('/'))))
                        .collect(),
                );
            }
            return;
        }
        let existing = dir
            .ancestors()
            .find(|d| d.is_dir())
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        let filter = (existing == dir && !keeps_name).then_some(tail);
        if tree.has_roots() || tree.dir() != existing {
            tree.set_filter(None);
            tree.set_dir(existing);
        }
        tree.set_filter(filter);
    }

    /// Write the directory under the tree's cursor into the field, with the
    /// file name kept when the field names a file.
    fn take_selection(&mut self) {
        let Some(tree) = self.tree.as_ref() else {
            return;
        };
        if tree.visible_len() == 0 {
            return;
        }
        let dir = tree.selected_path().display().to_string();
        let mut text = if dir.ends_with('/') {
            dir
        } else {
            format!("{dir}/")
        };
        if self.keeps_name {
            let current = self.input.text();
            let name = current.rsplit('/').next().unwrap_or(current);
            text.push_str(name);
        }
        self.input.set_text(text);
    }

    /// A key while the field has the focus of its host.
    pub(crate) fn handle_key(&mut self, chord: KeyChord) -> FieldKey {
        if let Some(tree) = self.tree.as_mut() {
            tree.message = None;
        }
        if self.in_tree {
            return self.handle_tree_key(chord);
        }
        let key = chord.raw;
        match key.code {
            KeyCode::Esc if self.is_open() => self.roll_back(),
            KeyCode::Tab => self.toggle(),
            KeyCode::Enter if self.is_open() => self.close(),
            KeyCode::Down if self.is_open() => {
                if self.tree.as_ref().is_some_and(|t| t.visible_len() > 0) {
                    self.in_tree = true;
                    self.take_selection();
                }
            }
            KeyCode::Up if self.is_open() => {}
            _ => return self.edit(key),
        }
        FieldKey::Handled
    }

    /// Edit the text with `key`; the tree follows.
    fn edit(&mut self, key: KeyEvent) -> FieldKey {
        match handle_input_key(&mut self.input, key) {
            InputKeyResult::TextModified => {
                self.sync_tree();
                FieldKey::Handled
            }
            InputKeyResult::Handled => FieldKey::Handled,
            InputKeyResult::NotHandled => FieldKey::NotHandled,
        }
    }

    fn handle_tree_key(&mut self, chord: KeyChord) -> FieldKey {
        let Some(tree) = self.tree.as_mut() else {
            self.in_tree = false;
            return FieldKey::NotHandled;
        };
        if tree.is_editing() {
            tree.handle_edit_key(chord.raw);
            if !tree.is_editing() {
                self.take_selection();
            }
            return FieldKey::Handled;
        }

        // A letter is typed into the field; the file manager's directory
        // keys act only when they are not letters, `toggle_hidden` always.
        let key = chord.canonical;
        let typed = matches!(key.code, KeyCode::Char(_))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if self.hotkeys.matches_canonical("toggle_hidden", &key) {
            tree.toggle_hidden();
            self.take_selection();
            return FieldKey::Handled;
        }
        if !typed {
            if self.hotkeys.matches_canonical("create_dir", &key) {
                tree.start_create();
                return FieldKey::Handled;
            }
            if self.hotkeys.matches_canonical("rename", &key) {
                tree.start_rename();
                return FieldKey::Handled;
            }
            if self.hotkeys.matches_canonical("delete", &key) {
                tree.delete_under_cursor();
                self.take_selection();
                return FieldKey::Handled;
            }
        }

        match key.code {
            KeyCode::Esc => self.roll_back(),
            KeyCode::Enter | KeyCode::Tab => self.close(),
            KeyCode::Up if tree.cursor == 0 => self.in_tree = false,
            KeyCode::Up => {
                tree.cursor_up();
                self.take_selection();
            }
            KeyCode::Down => {
                tree.cursor_down();
                self.take_selection();
            }
            KeyCode::PageUp | KeyCode::PageDown => {
                tree.cursor_page(key.code == KeyCode::PageUp);
                self.take_selection();
            }
            KeyCode::Home => {
                tree.cursor_home();
                self.take_selection();
            }
            KeyCode::End => {
                tree.cursor_end();
                self.take_selection();
            }
            KeyCode::Right => tree.expand(),
            KeyCode::Left => {
                tree.collapse_or_parent();
                self.take_selection();
            }
            _ => {
                // Anything else edits the field, as typing there would.
                self.in_tree = false;
                return self.edit(chord.raw);
            }
        }
        FieldKey::Handled
    }

    /// The mouse over the field or its tree; `true` when it was used.
    pub(crate) fn handle_mouse(&mut self, mouse: &MouseEvent) -> bool {
        if let Some(tree) = self.tree.as_mut() {
            if tree.is_editing() {
                tree.handle_edit_mouse(mouse);
                return true;
            }
        }
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                drag_field(&mut self.input, self.last_text_area, mouse);
                return true;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.dragging = false;
                return false;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let over_tree = self
                    .last_tree_area
                    .is_some_and(|a| a.contains((mouse.column, mouse.row).into()));
                let Some(tree) = self.tree.as_mut().filter(|_| over_tree) else {
                    return false;
                };
                for _ in 0..3 {
                    if mouse.kind == MouseEventKind::ScrollUp {
                        tree.cursor_up();
                    } else {
                        tree.cursor_down();
                    }
                }
                self.in_tree = true;
                self.take_selection();
                return true;
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return false,
        }

        // The box but its arrow places the cursor in the text.
        let frame = self.last_box_area.map(|area| Rect {
            width: area.width.saturating_sub(3),
            ..area
        });
        if press_field_box(&mut self.input, self.last_text_area, frame, mouse) {
            self.in_tree = false;
            self.dragging = true;
            return true;
        }
        let point = (mouse.column, mouse.row).into();
        if self.last_box_area.is_some_and(|a| a.contains(point)) {
            self.in_tree = false;
            self.toggle();
            return true;
        }
        if let Some(tree) = self.tree.as_mut() {
            if tree.click(mouse.column, mouse.row) {
                self.in_tree = true;
                self.take_selection();
                return true;
            }
        }
        self.last_tree_area.is_some_and(|a| a.contains(point))
    }

    /// A paste goes into a name being typed in the tree, else the field.
    pub(crate) fn paste(&mut self, text: &str) {
        if self.tree.as_mut().is_some_and(|t| t.paste(text)) {
            return;
        }
        self.in_tree = false;
        self.input.paste(text);
        self.sync_tree();
    }

    /// Draw the field into the first three lines of `area` and the tree,
    /// while open, below it. `focused` draws the cursor where it is.
    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, focused: bool) {
        let open = self.is_open();
        let border = Style::default().fg(theme.accented_fg);
        let box_area = Rect { height: 3, ..area };
        let block = Block::default()
            .borders(if open {
                Borders::LEFT | Borders::TOP | Borders::RIGHT
            } else {
                Borders::ALL
            })
            .border_style(border);
        let inner = block.inner(box_area);
        block.render(box_area, buf);
        self.last_box_area = Some(box_area);

        let text_area = Rect::new(inner.x, inner.y, inner.width.saturating_sub(2), 1);
        render_text_input(
            buf,
            text_area,
            &mut self.input,
            focused && !self.in_tree,
            theme,
        );
        self.last_text_area = Some(text_area);
        buf.set_string(
            inner.x + inner.width.saturating_sub(1),
            inner.y,
            if open { "▲" } else { "▼" },
            Style::default().fg(theme.disabled),
        );

        self.last_tree_area = None;
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let list_area = Rect {
            y: box_area.y + 2,
            height: area.height.saturating_sub(2),
            ..area
        };
        let block = Block::default()
            .borders(Borders::LEFT | Borders::BOTTOM | Borders::RIGHT)
            .border_style(border);
        let rows = block.inner(list_area);
        block.render(list_area, buf);
        tree.render(rows, buf, theme, focused && self.in_tree);
        self.last_tree_area = Some(rows);

        if let Some(message) = &tree.message {
            let width = list_area.width.saturating_sub(4) as usize;
            buf.set_string(
                list_area.x + 2,
                list_area.y + list_area.height.saturating_sub(1),
                truncate_left(message, width),
                Style::default().fg(theme.error),
            );
        }
    }
}

/// The field's share of the file manager keys.
fn field_hotkeys(keybindings: &FileManagerKeybindings) -> HotkeyTable {
    let mut table = HotkeyTable::new();
    table.insert("create_dir", &keybindings.create_dir);
    table.insert("rename", &keybindings.rename);
    table.insert("delete", &keybindings.delete);
    table.insert("toggle_hidden", &keybindings.toggle_hidden);
    table
}
