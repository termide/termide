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
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Widget},
};

use crate::base::{render_input_field_scrolled, render_modal_block};
use crate::input_keys::handle_input_key;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;

use termide_config::FileManagerKeybindings;
use termide_core::HotkeyTable;
use termide_theme::Theme;

use crate::{
    calculate_modal_width, centered_rect_with_size, Modal, ModalResult, ModalWidthConfig,
    TextInputHandler,
};

/// Directory entry for tree display
#[derive(Debug, Clone)]
struct DirEntry {
    name: String,
    full_path: PathBuf,
    depth: usize,
    /// `Some(true)` = expanded, `Some(false)` = collapsed, `None` = ".." entry
    expanded: Option<bool>,
}

/// What an inline row edit does on Enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    /// The row is a placeholder for a directory still to be created.
    Create,
    /// The row is an existing directory being renamed.
    Rename,
}

/// A name being typed into a tree row.
#[derive(Debug)]
struct RowEdit {
    kind: EditKind,
    /// Index into `entries` of the edited row.
    row: usize,
    /// Directory the name is created or renamed in.
    parent: PathBuf,
    /// Cursor to return to when a create is cancelled.
    prev_cursor: usize,
    input: TextInputHandler,
}

/// Button order in the bottom row.
const BUTTON_CONFIRM: usize = 0;
const BUTTON_NEW_FOLDER: usize = 1;
const BUTTON_CANCEL: usize = 2;

/// Directory picker modal window with tree navigation
#[derive(Debug)]
pub struct DirectoryPickerModal {
    title: String,
    current_dir: PathBuf,
    entries: Vec<DirEntry>,
    visible_indices: Vec<usize>,
    tree_prefixes: Vec<String>,
    /// Cursor position (index into visible_indices)
    cursor: usize,
    scroll_offset: usize,
    /// Whether directories whose name starts with `.` are listed.
    show_hidden: bool,
    button_focused: bool,
    selected_button: usize,
    last_list_area: Option<Rect>,
    last_buttons_area: Option<Rect>,
    create_label: String,
    new_folder_label: String,
    cancel_label: String,
    cached_width: Option<u16>,
    /// `create_dir`, `rename` and `delete`, as bound in the file manager.
    hotkeys: HotkeyTable,
    edit: Option<RowEdit>,
    /// An error shown in place of the path line until the next key.
    message: Option<String>,
}

const MAX_VISIBLE_ITEMS: usize = 15;

impl DirectoryPickerModal {
    /// Create a new directory picker modal with custom title and confirm button
    pub fn new(initial_dir: PathBuf, title: String, confirm_label: String) -> Self {
        let t = termide_i18n::t();
        let mut modal = Self {
            title,
            current_dir: initial_dir,
            entries: Vec::new(),
            visible_indices: Vec::new(),
            tree_prefixes: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            show_hidden: false,
            button_focused: false,
            selected_button: 0,
            last_list_area: None,
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
            edit: None,
            message: None,
        };
        modal.load_root();
        modal
    }

    /// Use the file manager's keys as configured instead of the defaults.
    pub fn with_keybindings(mut self, keybindings: &FileManagerKeybindings) -> Self {
        self.hotkeys = picker_hotkeys(keybindings);
        self
    }

    /// Get the currently selected path
    fn selected_path(&self) -> PathBuf {
        if let Some(&tree_idx) = self.visible_indices.get(self.cursor) {
            let entry = &self.entries[tree_idx];
            if entry.expanded.is_none() {
                // ".." entry — return current dir (going up = selecting parent)
                self.current_dir.clone()
            } else {
                entry.full_path.clone()
            }
        } else {
            self.current_dir.clone()
        }
    }

    /// Load root directory entries (top-level)
    fn load_root(&mut self) {
        self.entries.clear();
        self.cursor = 0;
        self.scroll_offset = 0;
        self.cached_width = None;

        // Add ".." entry if not at filesystem root
        if self.current_dir.parent().is_some() {
            self.entries.push(DirEntry {
                name: "..".to_string(),
                full_path: self.current_dir.clone(),
                depth: 0,
                expanded: None,
            });
        }

        // Read subdirectories
        self.load_children_into(&self.current_dir.clone(), 0, self.entries.len());
        self.recompute_visible();
    }

    /// Read child directories and insert into entries at given position
    fn load_children_into(&mut self, dir: &PathBuf, depth: usize, insert_at: usize) {
        let mut dirs: Vec<DirEntry> = Vec::new();

        if let Ok(read_dir) = std::fs::read_dir(dir) {
            for entry in read_dir.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if !self.show_hidden && name.starts_with('.') {
                    continue;
                }
                dirs.push(DirEntry {
                    name,
                    full_path: path,
                    depth,
                    expanded: Some(false),
                });
            }
        }

        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        // Insert children at position (after parent)
        for (i, d) in dirs.into_iter().enumerate() {
            self.entries.insert(insert_at + i, d);
        }
    }

    /// Expand directory at visible index
    fn expand_dir(&mut self, vis_idx: usize) {
        let Some(&tree_idx) = self.visible_indices.get(vis_idx) else {
            return;
        };
        if self.entries[tree_idx].expanded != Some(false) {
            return;
        }

        let dir_path = self.entries[tree_idx].full_path.clone();
        let depth = self.entries[tree_idx].depth;

        self.entries[tree_idx].expanded = Some(true);

        // Check if children already loaded
        let has_children = self
            .entries
            .get(tree_idx + 1)
            .is_some_and(|e| e.depth > depth);

        if !has_children {
            self.load_children_into(&dir_path, depth + 1, tree_idx + 1);
        }

        self.recompute_visible();
    }

    /// Collapse directory at visible index
    fn collapse_dir(&mut self, vis_idx: usize) {
        let Some(&tree_idx) = self.visible_indices.get(vis_idx) else {
            return;
        };
        if self.entries[tree_idx].expanded != Some(true) {
            return;
        }

        self.entries[tree_idx].expanded = Some(false);
        self.recompute_visible();
    }

    /// Jump cursor to parent directory in tree (for Left on non-expanded item)
    fn jump_to_parent(&mut self) {
        let Some(&tree_idx) = self.visible_indices.get(self.cursor) else {
            return;
        };
        let depth = self.entries[tree_idx].depth;
        if depth == 0 {
            return;
        }

        // Search backwards in visible for entry with depth - 1
        for (vi, &ti) in self.visible_indices.iter().enumerate().rev() {
            if self.entries[ti].depth < depth {
                self.cursor = vi;
                self.adjust_scroll();
                return;
            }
        }
    }

    /// Show or hide the hidden directories, keeping the expanded branches
    /// and the cursor where they still exist.
    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        let expanded = self.expanded_paths();
        let under_cursor = self
            .visible_indices
            .get(self.cursor)
            .map(|&i| self.entries[i].full_path.clone());
        self.reload(&expanded, under_cursor.as_deref());
    }

    /// Paths of the expanded branches.
    fn expanded_paths(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .filter(|e| e.expanded == Some(true))
            .map(|e| e.full_path.clone())
            .collect()
    }

    /// Read the tree from disk again with the `expanded` branches open, the
    /// cursor on `select` — or else on its closest ancestor still listed, or
    /// else at the top.
    fn reload(&mut self, expanded: &[PathBuf], select: Option<&Path>) {
        self.load_root();
        // Parents precede their children, so one pass re-expands nested
        // branches in order.
        let mut i = 0;
        while i < self.entries.len() {
            if self.entries[i].expanded == Some(false)
                && expanded.contains(&self.entries[i].full_path)
            {
                if let Some(vis) = self.visible_indices.iter().position(|&t| t == i) {
                    self.expand_dir(vis);
                }
            }
            i += 1;
        }

        // The cursor stays on its entry, or else on its closest ancestor
        // still listed (a hidden directory it was inside of).
        if let Some(path) = select {
            let found = path.ancestors().find_map(|ancestor| {
                self.visible_indices.iter().position(|&t| {
                    let entry = &self.entries[t];
                    entry.expanded.is_some() && entry.full_path == ancestor
                })
            });
            self.cursor = found.unwrap_or(0);
            self.adjust_scroll();
        }
    }

    /// Index just past `tree_idx` and every entry loaded below it.
    fn subtree_end(&self, tree_idx: usize) -> usize {
        let depth = self.entries[tree_idx].depth;
        let mut end = tree_idx + 1;
        while self.entries.get(end).is_some_and(|e| e.depth > depth) {
            end += 1;
        }
        end
    }

    /// The directory under the cursor (not `..`) and its tree index.
    fn dir_under_cursor(&self) -> Option<usize> {
        let &tree_idx = self.visible_indices.get(self.cursor)?;
        self.entries[tree_idx]
            .expanded
            .is_some()
            .then_some(tree_idx)
    }

    /// Open an empty name row beside the entry under the cursor: after it
    /// and its subtree, at its depth, so the new directory becomes its
    /// sibling. On `..` or an empty list the row goes to the top level.
    fn start_create(&mut self) {
        self.button_focused = false;
        let (parent, depth, row) = match self.dir_under_cursor() {
            Some(tree_idx) => {
                let entry = &self.entries[tree_idx];
                let parent = entry
                    .full_path
                    .parent()
                    .map_or_else(|| self.current_dir.clone(), Path::to_path_buf);
                (parent, entry.depth, self.subtree_end(tree_idx))
            }
            None => {
                let row = usize::from(self.entries.first().is_some_and(|e| e.expanded.is_none()));
                (self.current_dir.clone(), 0, row)
            }
        };
        self.entries.insert(
            row,
            DirEntry {
                name: String::new(),
                full_path: parent.clone(),
                depth,
                expanded: Some(false),
            },
        );
        let prev_cursor = self.cursor;
        self.recompute_visible();
        self.cursor = self
            .visible_indices
            .iter()
            .position(|&t| t == row)
            .unwrap_or(0);
        self.adjust_scroll();
        self.edit = Some(RowEdit {
            kind: EditKind::Create,
            row,
            parent,
            prev_cursor,
            input: TextInputHandler::new(),
        });
    }

    /// Turn the row under the cursor into a field holding its name.
    fn start_rename(&mut self) {
        let Some(tree_idx) = self.dir_under_cursor() else {
            return;
        };
        let entry = &self.entries[tree_idx];
        let Some(parent) = entry.full_path.parent() else {
            return;
        };
        self.button_focused = false;
        self.edit = Some(RowEdit {
            kind: EditKind::Rename,
            row: tree_idx,
            parent: parent.to_path_buf(),
            prev_cursor: self.cursor,
            input: TextInputHandler::with_text(entry.name.clone()),
        });
    }

    /// Drop the edit; a create also takes its placeholder row away.
    fn cancel_edit(&mut self) {
        let Some(edit) = self.edit.take() else {
            return;
        };
        if edit.kind == EditKind::Create {
            self.entries.remove(edit.row);
            self.recompute_visible();
            self.cursor = edit
                .prev_cursor
                .min(self.visible_indices.len().saturating_sub(1));
            self.adjust_scroll();
        }
    }

    /// Create or rename with the typed name. On failure the field stays
    /// open with the reason shown above the tree.
    fn commit_edit(&mut self) {
        let Some(edit) = self.edit.as_ref() else {
            return;
        };
        let name = edit.input.text().to_string();
        if let Err(message) = validate_name(&name) {
            self.message = Some(message);
            return;
        }
        let target = edit.parent.join(&name);
        let t = termide_i18n::t();

        match edit.kind {
            EditKind::Create => {
                if target.symlink_metadata().is_ok() {
                    self.message = Some(t.directory_picker_exists(&name));
                    return;
                }
                if let Err(e) = std::fs::create_dir(&target) {
                    self.message = Some(e.to_string());
                    return;
                }
                let row = edit.row;
                self.edit = None;
                self.entries.remove(row);
                let expanded = self.expanded_paths();
                self.reload(&expanded, Some(&target));
            }
            EditKind::Rename => {
                let old = self.entries[edit.row].full_path.clone();
                if target == old {
                    self.edit = None;
                    return;
                }
                // A case-only rename on a case-insensitive file system finds
                // the directory itself under the new name.
                if target.symlink_metadata().is_ok() && !same_entry(&old, &target) {
                    self.message = Some(t.directory_picker_exists(&name));
                    return;
                }
                if let Err(e) = std::fs::rename(&old, &target) {
                    self.message = Some(e.to_string());
                    return;
                }
                self.edit = None;
                let expanded: Vec<PathBuf> = self
                    .expanded_paths()
                    .into_iter()
                    .map(|p| match p.strip_prefix(&old) {
                        Ok(rest) => target.join(rest),
                        Err(_) => p,
                    })
                    .collect();
                self.reload(&expanded, Some(&target));
            }
        }
    }

    /// Remove the directory under the cursor if it is empty; a directory
    /// with anything in it is left alone with a message.
    fn delete_under_cursor(&mut self) {
        let Some(tree_idx) = self.dir_under_cursor() else {
            return;
        };
        let path = self.entries[tree_idx].full_path.clone();
        match std::fs::remove_dir(&path) {
            Ok(()) => {
                let cursor = self.cursor;
                let expanded = self.expanded_paths();
                self.reload(&expanded, None);
                self.cursor = cursor.min(self.visible_indices.len().saturating_sub(1));
                self.adjust_scroll();
            }
            Err(e) if e.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                self.message = Some(termide_i18n::t().directory_picker_not_empty().to_string());
            }
            Err(e) => self.message = Some(e.to_string()),
        }
    }

    /// Keys while a name is being typed: Enter commits, Esc cancels, the
    /// rest edit the field. The raw event is used so letters arrive as typed
    /// on any layout.
    fn handle_edit_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.cancel_edit(),
            KeyCode::Enter => self.commit_edit(),
            // The row stays put while it is edited.
            KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown => {}
            _ => {
                if let Some(edit) = self.edit.as_mut() {
                    handle_input_key(&mut edit.input, key);
                }
            }
        }
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

    /// Recompute visible indices and tree prefixes
    fn recompute_visible(&mut self) {
        self.visible_indices = compute_visible(&self.entries);
        self.tree_prefixes = compute_prefixes(&self.entries, &self.visible_indices);
    }

    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        let title_width = self.title.len() as u16 + 4;

        let max_entry_display = self
            .entries
            .iter()
            .map(|e| e.depth * 3 + e.name.len() + 5) // gutter + prefix + icon + name
            .max()
            .unwrap_or(0);
        let path_width = self.current_dir.to_string_lossy().len() as u16 + 5;

        let buttons_width = (self.create_label.width()
            + self.new_folder_label.width()
            + self.cancel_label.width()) as u16
            + 18;

        calculate_modal_width(
            [
                title_width,
                path_width,
                max_entry_display as u16 + 2,
                buttons_width,
            ]
            .into_iter(),
            screen_width,
            ModalWidthConfig::wide(),
        )
    }

    fn get_modal_width(&mut self, screen_width: u16) -> u16 {
        if let Some(width) = self.cached_width {
            return width;
        }
        let width = self.calculate_modal_width(screen_width);
        self.cached_width = Some(width);
        width
    }

    fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.adjust_scroll();
        }
    }

    fn cursor_down(&mut self) {
        if self.cursor < self.visible_indices.len().saturating_sub(1) {
            self.cursor += 1;
            self.adjust_scroll();
        }
    }

    fn cursor_home(&mut self) {
        self.cursor = 0;
        self.adjust_scroll();
    }

    fn cursor_end(&mut self) {
        self.cursor = self.visible_indices.len().saturating_sub(1);
        self.adjust_scroll();
    }

    fn adjust_scroll(&mut self) {
        self.scroll_offset =
            termide_ui::ensure_offset_visible(self.scroll_offset, self.cursor, MAX_VISIBLE_ITEMS);
    }

    /// Go to parent directory (reload tree root)
    fn go_parent(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            self.current_dir = parent.to_path_buf();
            self.load_root();
        }
    }
}

impl Modal for DirectoryPickerModal {
    type Result = PathBuf;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let modal_width = self.get_modal_width(area.width);

        let visible_count = self.visible_indices.len().clamp(3, MAX_VISIBLE_ITEMS);
        let list_height = visible_count as u16;
        let modal_height = 6 + list_height;

        let modal_area = centered_rect_with_size(modal_width, modal_height, area);
        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // Path line: an error if there is one, else the path the edited
        // name or the cursor stands for (truncated on the left).
        if let Some(message) = &self.message {
            let text = termide_ui::path_utils::truncate_left(message, inner.width as usize);
            buf.set_string(inner.x, inner.y, &text, Style::default().fg(theme.error));
        } else {
            let selected = match &self.edit {
                Some(edit) => edit.parent.join(edit.input.text()),
                None => self.selected_path(),
            };
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

        let prefix_style = Style::default().fg(theme.disabled);
        let mut list_items: Vec<ListItem> = Vec::new();
        let edit_row = self.edit.as_ref().map(|e| e.row);
        // Where the name field goes: (x, y, width) inside the edited row.
        let mut field_at: Option<(u16, u16, u16)> = None;

        for (vis_pos, &tree_idx) in self
            .visible_indices
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(MAX_VISIBLE_ITEMS)
        {
            let entry = &self.entries[tree_idx];
            let is_edited = edit_row == Some(tree_idx);
            let is_selected = vis_pos == self.cursor && !self.button_focused && !is_edited;

            // Tree prefix (├─, └─, etc.)
            let tree_prefix = self
                .tree_prefixes
                .get(vis_pos)
                .map(|s| s.as_str())
                .unwrap_or("");

            // Icon: ".." = special, expanded = ▼, collapsed = ▶
            let icon = if entry.expanded.is_none() {
                " "
            } else if entry.expanded == Some(true) {
                "▼"
            } else {
                "▶"
            };

            // Cursor style: invert fg/bg like file manager
            let (row_style, prefix_row_style) = if is_selected {
                (
                    Style::default()
                        .fg(theme.bg)
                        .bg(theme.fg)
                        .add_modifier(Modifier::BOLD),
                    Style::default()
                        .fg(theme.bg)
                        .bg(theme.fg)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (Style::default().fg(theme.fg), prefix_style)
            };

            let text = if is_edited {
                let x = 1 + tree_prefix.width() as u16 + icon.width() as u16 + 1;
                let y = list_area.y + (vis_pos - self.scroll_offset) as u16;
                field_at = Some((inner.x + x, y, inner.width.saturating_sub(x + 1)));
                format!("{} ", icon)
            } else {
                format!("{} {}", icon, entry.name)
            };
            let line_width = 1 + tree_prefix.width() + text.width();
            let padding = " ".repeat((inner.width as usize).saturating_sub(line_width));

            let line = Line::from(vec![
                // A one-column gutter before the tree, as in the file manager.
                Span::styled(" ", row_style),
                Span::styled(tree_prefix, prefix_row_style),
                Span::styled(text, row_style),
                Span::styled(padding, row_style),
            ]);

            list_items.push(ListItem::new(line));
        }

        let list = List::new(list_items).style(Style::default().bg(theme.bg));
        list.render(list_area, buf);
        self.last_list_area = Some(list_area);

        if let (Some((x, y, width)), Some(edit)) = (field_at, self.edit.as_mut()) {
            let scroll = edit.input.follow_scroll(width);
            render_input_field_scrolled(
                buf,
                x,
                y,
                width,
                edit.input.text(),
                edit.input.cursor_pos(),
                edit.input.selection_range(),
                true,
                theme,
                scroll,
            );
        }

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
        self.message = None;
        if self.edit.is_some() {
            self.handle_edit_key(chord.raw);
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
            self.start_rename();
            return Ok(None);
        }
        if self.hotkeys.matches_canonical("delete", &key) {
            self.delete_under_cursor();
            return Ok(None);
        }
        match key.code {
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            KeyCode::Char('.') => {
                self.toggle_hidden();
                Ok(None)
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.button_focused = !self.button_focused;
                Ok(None)
            }
            // List navigation
            KeyCode::Up | KeyCode::Char('k') if !self.button_focused => {
                self.cursor_up();
                Ok(None)
            }
            KeyCode::Down | KeyCode::Char('j') if !self.button_focused => {
                self.cursor_down();
                Ok(None)
            }
            KeyCode::Home if !self.button_focused => {
                self.cursor_home();
                Ok(None)
            }
            KeyCode::End if !self.button_focused => {
                self.cursor_end();
                Ok(None)
            }
            // Expand directory
            KeyCode::Right | KeyCode::Char('l') if !self.button_focused => {
                self.expand_dir(self.cursor);
                Ok(None)
            }
            // Collapse directory or jump to parent in tree
            KeyCode::Left | KeyCode::Char('h') if !self.button_focused => {
                if let Some(&tree_idx) = self.visible_indices.get(self.cursor) {
                    if self.entries[tree_idx].expanded == Some(true) {
                        self.collapse_dir(self.cursor);
                    } else if self.entries[tree_idx].depth > 0 {
                        self.jump_to_parent();
                    }
                }
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
                self.go_parent();
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
                    if let Some(&tree_idx) = self.visible_indices.get(self.cursor) {
                        if self.entries[tree_idx].expanded.is_none() {
                            // ".." — go to parent
                            self.go_parent();
                        } else {
                            // Navigate into: make selected dir the new root
                            self.current_dir = self.entries[tree_idx].full_path.clone();
                            self.load_root();
                        }
                    }
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
        use crate::{check_mouse_click, MouseClickResult};

        // A name being typed keeps its row; Enter or Esc ends it first.
        if self.edit.is_some() {
            return Ok(None);
        }

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                for _ in 0..3 {
                    self.cursor_up();
                }
                return Ok(None);
            }
            MouseEventKind::ScrollDown => {
                for _ in 0..3 {
                    self.cursor_down();
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
        match check_mouse_click(
            mouse.column,
            mouse.row,
            None,
            self.last_list_area,
            self.scroll_offset,
        ) {
            MouseClickResult::OnListItem(clicked_vis_idx) => {
                if clicked_vis_idx < self.visible_indices.len() {
                    self.button_focused = false;
                    if self.cursor == clicked_vis_idx {
                        // Click on already-selected item: toggle expand/collapse
                        if let Some(&tree_idx) = self.visible_indices.get(clicked_vis_idx) {
                            match self.entries[tree_idx].expanded {
                                Some(false) => self.expand_dir(clicked_vis_idx),
                                Some(true) => self.collapse_dir(clicked_vis_idx),
                                None => {
                                    // ".." — go parent
                                    self.go_parent();
                                }
                            }
                        }
                    } else {
                        self.cursor = clicked_vis_idx;
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// A paste goes into the name being typed, on one line.
    fn handle_paste(&mut self, text: &str) -> bool {
        let Some(edit) = self.edit.as_mut() else {
            return false;
        };
        let line: String = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
        edit.input.paste(&line);
        true
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

/// A single path component: not empty, no separator, not `.` or `..`.
fn validate_name(name: &str) -> std::result::Result<(), String> {
    let t = termide_i18n::t();
    if name.trim().is_empty() {
        return Err(t.directory_picker_name_empty().to_string());
    }
    if name == "."
        || name == ".."
        || name.contains(['/', '\0'])
        || name.contains(std::path::MAIN_SEPARATOR)
    {
        return Err(t.directory_picker_name_invalid().to_string());
    }
    Ok(())
}

/// Whether `a` and `b` name the same file system entry.
fn same_entry(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (a.symlink_metadata(), b.symlink_metadata()) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        // Without inode numbers, trust a case-insensitive spelling match.
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    }
}

// === Tree utilities (same algorithm as panel-file-manager/src/tree.rs) ===

/// Compute indices of visible nodes, skipping children of collapsed directories.
fn compute_visible(entries: &[DirEntry]) -> Vec<usize> {
    let mut visible = Vec::new();
    let mut skip_below_depth: Option<usize> = None;

    for (i, entry) in entries.iter().enumerate() {
        if let Some(max_depth) = skip_below_depth {
            if entry.depth > max_depth {
                continue;
            }
            skip_below_depth = None;
        }

        visible.push(i);

        if entry.expanded == Some(false) {
            skip_below_depth = Some(entry.depth);
        }
    }

    visible
}

/// Compute tree-drawing prefixes for visible nodes.
fn compute_prefixes(entries: &[DirEntry], visible: &[usize]) -> Vec<String> {
    if visible.is_empty() {
        return Vec::new();
    }

    let max_depth = visible
        .iter()
        .map(|&idx| entries[idx].depth)
        .max()
        .unwrap_or(0);

    if max_depth == 0 {
        return vec![String::new(); visible.len()];
    }

    let mut has_next_at_level = vec![false; max_depth + 1];
    let mut prefixes: Vec<String> = Vec::with_capacity(visible.len());

    for &tree_idx in visible.iter().rev() {
        let depth = entries[tree_idx].depth;

        if depth == 0 {
            has_next_at_level.fill(false);
            has_next_at_level[0] = true;
            prefixes.push(String::new());
            continue;
        }

        let mut prefix = String::with_capacity(depth * 3);
        for (lvl, has_next) in has_next_at_level[1..=depth].iter().enumerate() {
            let lvl = lvl + 1;
            if lvl == depth {
                if *has_next {
                    prefix.push_str(" ├─");
                } else {
                    prefix.push_str(" └─");
                }
            } else if *has_next {
                prefix.push_str(" │ ");
            } else {
                prefix.push_str("   ");
            }
        }
        prefixes.push(prefix);

        for val in &mut has_next_at_level[(depth + 1)..] {
            *val = false;
        }
        has_next_at_level[depth] = true;
    }

    prefixes.reverse();
    prefixes
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
            .visible_indices
            .iter()
            .map(|&i| modal.entries[i].name.as_str())
            .collect()
    }

    fn press(modal: &mut DirectoryPickerModal, code: KeyCode) {
        let key = KeyEvent::new(code, KeyModifiers::NONE);
        modal
            .handle_key(termide_core::KeyChord::identity(key))
            .unwrap();
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
        assert_eq!(modal.cursor, 0);
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
        assert!(modal.edit.is_none());
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

        assert!(modal.edit.is_none());
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
        assert!(modal.message.is_some());
        assert!(modal.edit.is_some());

        type_text(&mut modal, "a/b");
        press(&mut modal, KeyCode::Enter);
        assert!(modal.message.is_some());
        assert!(!root.join("a").exists());

        for _ in 0..3 {
            press(&mut modal, KeyCode::Backspace);
        }
        type_text(&mut modal, "target");
        press(&mut modal, KeyCode::Enter);
        assert!(modal.message.is_some());
        assert!(modal.edit.is_some());

        // The message lasts until the next key.
        press(&mut modal, KeyCode::Esc);
        assert!(modal.message.is_none());
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
        assert!(modal.edit.is_none());
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
        assert!(modal.message.is_some());

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
        assert!(modal.edit.is_some());
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
