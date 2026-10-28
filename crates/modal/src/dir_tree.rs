//! A directory tree: the rows of a directory's subdirectories, expanded and
//! collapsed in place, with a cursor, hidden directories on demand, and
//! directories created, renamed and (when empty) deleted in a row.
//!
//! The top level is either a directory's children (under a `..` row) or a
//! fixed list of roots, each a directory to expand. The tree owns no keys:
//! its host maps keys and clicks to the operations here, so a modal of its
//! own and a dropdown under a field can share it.

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Widget},
};
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;

use crate::base::{drag_field, press_field, render_text_input};
use crate::input_keys::handle_input_key;
use crate::TextInputHandler;
use termide_theme::Theme;

/// Directory entry for tree display
#[derive(Debug, Clone)]
pub(crate) struct DirEntry {
    pub(crate) name: String,
    pub(crate) full_path: PathBuf,
    pub(crate) depth: usize,
    /// `Some(true)` = expanded, `Some(false)` = collapsed, `None` = ".." entry
    pub(crate) expanded: Option<bool>,
    /// Whether there is a directory to read under it: not so for a root
    /// that does not exist (yet) or is remote.
    pub(crate) expandable: bool,
    /// A dim mark at the right end of the row, telling roots apart.
    pub(crate) note: Option<String>,
}

/// A row of the top level when it is a list of roots.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TreeRoot {
    /// The text of the row: the path as given.
    pub(crate) label: String,
    /// The directory it stands for.
    pub(crate) path: PathBuf,
    /// A dim mark at the right end of the row.
    pub(crate) note: Option<String>,
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
pub(crate) struct RowEdit {
    kind: EditKind,
    /// Index into `entries` of the edited row.
    row: usize,
    /// Directory the name is created or renamed in.
    parent: PathBuf,
    /// Cursor to return to when a create is cancelled.
    prev_cursor: usize,
    pub(crate) input: TextInputHandler,
    /// The field as last drawn in its row, for the mouse.
    pub(crate) area: Option<Rect>,
    /// A press landed in the field and the button is still down.
    dragging: bool,
}

/// A directory tree whose top level is `dir`'s children, headed by `..`
/// when `dir` has a parent, or else the `roots` given.
#[derive(Debug)]
pub(crate) struct DirTree {
    /// The directory whose children are the top level.
    dir: PathBuf,
    /// The top level instead of `dir`'s children: a label and the
    /// directory it stands for. Roots are neither renamed nor deleted.
    roots: Option<Vec<TreeRoot>>,
    /// Only `dir`'s children this matches fuzzily are listed.
    filter: Option<String>,
    pub(crate) entries: Vec<DirEntry>,
    pub(crate) visible_indices: Vec<usize>,
    tree_prefixes: Vec<String>,
    /// Cursor position (index into visible_indices)
    pub(crate) cursor: usize,
    scroll_offset: usize,
    /// Rows drawn at most; the cursor is kept within them.
    max_visible: usize,
    /// Whether directories whose name starts with `.` are listed.
    show_hidden: bool,
    pub(crate) edit: Option<RowEdit>,
    /// An error from the last operation, until the host clears it.
    pub(crate) message: Option<String>,
    last_list_area: Option<Rect>,
    /// The rows were read from disk again since the host last asked.
    reloaded: bool,
}

impl DirTree {
    /// The tree of `dir`, showing at most `max_visible` rows at once.
    pub(crate) fn new(dir: PathBuf, max_visible: usize) -> Self {
        let mut tree = Self {
            dir,
            roots: None,
            filter: None,
            entries: Vec::new(),
            visible_indices: Vec::new(),
            tree_prefixes: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            max_visible,
            show_hidden: false,
            edit: None,
            message: None,
            last_list_area: None,
            reloaded: false,
        };
        tree.load_root();
        tree
    }

    /// The directory whose children are the top level.
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Show at most `rows` rows at once, keeping the cursor among them.
    pub(crate) fn set_max_visible(&mut self, rows: usize) {
        let rows = rows.max(1);
        if rows != self.max_visible {
            self.max_visible = rows;
            self.adjust_scroll();
        }
    }

    /// Rows in the tree as it is expanded now.
    pub(crate) fn visible_len(&self) -> usize {
        self.visible_indices.len()
    }

    /// Columns the widest loaded row takes: gutter, prefix, icon and name.
    pub(crate) fn content_width(&self) -> usize {
        self.entries
            .iter()
            .map(|e| {
                let note = e.note.as_deref().map_or(0, |n| n.width() + 1);
                e.depth * 3 + e.name.width() + note + 5
            })
            .max()
            .unwrap_or(0)
    }

    /// The directory under the cursor; on `..` (or with nothing listed) the
    /// listed directory itself.
    pub(crate) fn selected_path(&self) -> PathBuf {
        match self.visible_indices.get(self.cursor) {
            Some(&tree_idx) if self.entries[tree_idx].expanded.is_some() => {
                self.entries[tree_idx].full_path.clone()
            }
            _ => self.dir.clone(),
        }
    }

    /// The path the row being named stands for, else the selected one.
    pub(crate) fn shown_path(&self) -> PathBuf {
        match &self.edit {
            Some(edit) => edit.parent.join(edit.input.text()),
            None => self.selected_path(),
        }
    }

    /// Whether the rows were read from disk again since the last call, so
    /// whatever the host measured from them is stale.
    pub(crate) fn take_reloaded(&mut self) -> bool {
        std::mem::take(&mut self.reloaded)
    }

    /// Whether a name is being typed into a row.
    pub(crate) fn is_editing(&self) -> bool {
        self.edit.is_some()
    }

    /// List `dir` instead, collapsed, the cursor at the top.
    pub(crate) fn set_dir(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.roots = None;
        self.load_root();
    }

    /// List `roots` as the top level, collapsed, the cursor at the top.
    pub(crate) fn set_roots(&mut self, roots: Vec<TreeRoot>) {
        self.roots = Some(roots);
        self.load_root();
    }

    /// Whether the top level is a list of roots.
    pub(crate) fn has_roots(&self) -> bool {
        self.roots.is_some()
    }

    /// List only the children of `dir` that `filter` matches fuzzily; a
    /// filter starting with `.` lists hidden directories too.
    pub(crate) fn set_filter(&mut self, filter: Option<String>) {
        let filter = filter.filter(|f| !f.is_empty());
        if filter != self.filter {
            self.filter = filter;
            self.load_root();
        }
    }

    /// Whether hidden directories are listed: switched on, or asked for by
    /// a filter starting with `.`.
    fn lists_hidden(&self) -> bool {
        self.show_hidden || self.filter.as_deref().is_some_and(|f| f.starts_with('.'))
    }

    /// List the parent of the listed directory.
    pub(crate) fn go_parent(&mut self) {
        if let Some(parent) = self.dir.parent() {
            self.dir = parent.to_path_buf();
            self.load_root();
        }
    }

    /// Enter the row under the cursor: `..` lists the parent, a directory
    /// becomes the listed one.
    pub(crate) fn enter(&mut self) {
        let Some(&tree_idx) = self.visible_indices.get(self.cursor) else {
            return;
        };
        if self.entries[tree_idx].expanded.is_none() {
            self.go_parent();
        } else {
            let dir = self.entries[tree_idx].full_path.clone();
            self.set_dir(dir);
        }
    }

    /// Load root directory entries (top-level)
    fn load_root(&mut self) {
        self.entries.clear();
        self.reloaded = true;
        self.cursor = 0;
        self.scroll_offset = 0;

        if let Some(roots) = &self.roots {
            self.entries = roots
                .iter()
                .map(|root| DirEntry {
                    name: root.label.clone(),
                    full_path: root.path.clone(),
                    depth: 0,
                    expanded: Some(false),
                    expandable: root.path.is_dir(),
                    note: root.note.clone(),
                })
                .collect();
            self.recompute_visible();
            return;
        }

        // Add ".." entry if not at filesystem root
        if self.dir.parent().is_some() {
            self.entries.push(DirEntry {
                name: "..".to_string(),
                full_path: self.dir.clone(),
                depth: 0,
                expanded: None,
                expandable: false,
                note: None,
            });
        }

        // Read subdirectories
        let first = self.entries.len();
        self.load_children_into(&self.dir.clone(), 0, first);
        if let Some(filter) = &self.filter {
            let mut query = termide_ui::fuzzy::Query::fuzzy(filter);
            let children = self.entries.split_off(first);
            let order = termide_ui::fuzzy::rank(children.iter().map(|e| query.score(&e.name)));
            self.entries
                .extend(order.into_iter().map(|i| children[i].clone()));
        }
        self.recompute_visible();
    }

    /// Read child directories and insert into entries at given position
    fn load_children_into(&mut self, dir: &Path, depth: usize, insert_at: usize) {
        let mut dirs: Vec<DirEntry> = Vec::new();

        if let Ok(read_dir) = std::fs::read_dir(dir) {
            for entry in read_dir.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                if !self.lists_hidden() && name.starts_with('.') {
                    continue;
                }
                dirs.push(DirEntry {
                    name,
                    full_path: path,
                    depth,
                    expanded: Some(false),
                    expandable: true,
                    note: None,
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
        if self.entries[tree_idx].expanded != Some(false) || !self.entries[tree_idx].expandable {
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

    /// Expand the directory under the cursor.
    pub(crate) fn expand(&mut self) {
        self.expand_dir(self.cursor);
    }

    /// Collapse the directory under the cursor, or else step to the
    /// directory it is in.
    pub(crate) fn collapse_or_parent(&mut self) {
        if let Some(&tree_idx) = self.visible_indices.get(self.cursor) {
            if self.entries[tree_idx].expanded == Some(true) {
                self.collapse_dir(self.cursor);
            } else if self.entries[tree_idx].depth > 0 {
                self.jump_to_parent();
            }
        }
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
    pub(crate) fn toggle_hidden(&mut self) {
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

    /// The directory under the cursor if it may be renamed or deleted: not
    /// `..` and not a root.
    fn own_dir_under_cursor(&self) -> Option<usize> {
        self.dir_under_cursor()
            .filter(|&i| self.roots.is_none() || self.entries[i].depth > 0)
    }

    /// Open an empty name row beside the entry under the cursor: after it
    /// and its subtree, at its depth, so the new directory becomes its
    /// sibling. On `..` or an empty list the row goes to the top level.
    pub(crate) fn start_create(&mut self) {
        if self.roots.is_some() && self.own_dir_under_cursor().is_none() {
            // A root's sibling would not be listed among the roots.
            return;
        }
        let (parent, depth, row) = match self.dir_under_cursor() {
            Some(tree_idx) => {
                let entry = &self.entries[tree_idx];
                let parent = entry
                    .full_path
                    .parent()
                    .map_or_else(|| self.dir.clone(), Path::to_path_buf);
                (parent, entry.depth, self.subtree_end(tree_idx))
            }
            None => {
                let row = usize::from(self.entries.first().is_some_and(|e| e.expanded.is_none()));
                (self.dir.clone(), 0, row)
            }
        };
        self.entries.insert(
            row,
            DirEntry {
                name: String::new(),
                full_path: parent.clone(),
                depth,
                expanded: Some(false),
                expandable: true,
                note: None,
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
            area: None,
            dragging: false,
        });
    }

    /// Turn the row under the cursor into a field holding its name.
    pub(crate) fn start_rename(&mut self) {
        let Some(tree_idx) = self.own_dir_under_cursor() else {
            return;
        };
        let entry = &self.entries[tree_idx];
        let Some(parent) = entry.full_path.parent() else {
            return;
        };
        self.edit = Some(RowEdit {
            kind: EditKind::Rename,
            row: tree_idx,
            parent: parent.to_path_buf(),
            prev_cursor: self.cursor,
            input: TextInputHandler::with_text(entry.name.clone()),
            area: None,
            dragging: false,
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
    /// open with the reason in `message`.
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
                // The new name may not match what the list is filtered by.
                self.filter = None;
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
                self.filter = None;
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
    pub(crate) fn delete_under_cursor(&mut self) {
        let Some(tree_idx) = self.own_dir_under_cursor() else {
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
    pub(crate) fn handle_edit_key(&mut self, key: KeyEvent) {
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

    /// The mouse while a name is being typed: a press in its field places
    /// the cursor, a drag from there selects. Anything else is ignored, so
    /// the row stays until Enter or Esc.
    pub(crate) fn handle_edit_mouse(&mut self, mouse: &MouseEvent) {
        let Some(edit) = self.edit.as_mut() else {
            return;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                edit.dragging = press_field(&mut edit.input, edit.area, mouse);
            }
            MouseEventKind::Drag(MouseButton::Left) if edit.dragging => {
                drag_field(&mut edit.input, edit.area, mouse);
            }
            MouseEventKind::Up(MouseButton::Left) => edit.dragging = false,
            _ => {}
        }
    }

    /// A paste goes into the name being typed, on one line; `false` when
    /// no name is.
    pub(crate) fn paste(&mut self, text: &str) -> bool {
        let Some(edit) = self.edit.as_mut() else {
            return false;
        };
        let line: String = text.chars().filter(|c| !matches!(c, '\n' | '\r')).collect();
        edit.input.paste(&line);
        true
    }

    /// A left press on the rows as last drawn: the first selects a row, a
    /// second on the selected one expands or collapses it (`..` lists the
    /// parent). `false` when the press is on no row.
    pub(crate) fn click(&mut self, column: u16, row: u16) -> bool {
        use crate::{check_mouse_click, MouseClickResult};
        let MouseClickResult::OnListItem(clicked) =
            check_mouse_click(column, row, None, self.last_list_area, self.scroll_offset)
        else {
            return false;
        };
        if clicked >= self.visible_indices.len() {
            return false;
        }
        if self.cursor == clicked {
            match self.entries[self.visible_indices[clicked]].expanded {
                Some(false) => self.expand_dir(clicked),
                Some(true) => self.collapse_dir(clicked),
                None => self.go_parent(),
            }
        } else {
            self.cursor = clicked;
        }
        true
    }

    /// Recompute visible indices and tree prefixes
    fn recompute_visible(&mut self) {
        self.visible_indices = compute_visible(&self.entries);
        self.tree_prefixes = compute_prefixes(&self.entries, &self.visible_indices);
    }

    pub(crate) fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.adjust_scroll();
        }
    }

    pub(crate) fn cursor_down(&mut self) {
        if self.cursor < self.visible_indices.len().saturating_sub(1) {
            self.cursor += 1;
            self.adjust_scroll();
        }
    }

    /// Move the cursor a page of rows up or down.
    pub(crate) fn cursor_page(&mut self, up: bool) {
        let last = self.visible_indices.len().saturating_sub(1);
        self.cursor = if up {
            self.cursor.saturating_sub(self.max_visible)
        } else {
            (self.cursor + self.max_visible).min(last)
        };
        self.adjust_scroll();
    }

    pub(crate) fn cursor_home(&mut self) {
        self.cursor = 0;
        self.adjust_scroll();
    }

    pub(crate) fn cursor_end(&mut self) {
        self.cursor = self.visible_indices.len().saturating_sub(1);
        self.adjust_scroll();
    }

    fn adjust_scroll(&mut self) {
        self.scroll_offset =
            termide_ui::ensure_offset_visible(self.scroll_offset, self.cursor, self.max_visible);
    }

    /// Draw the rows into `area`, the cursor row inverted when
    /// `show_cursor`, a row being named as a field.
    pub(crate) fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        show_cursor: bool,
    ) {
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
            .take(self.max_visible)
        {
            let entry = &self.entries[tree_idx];
            let is_edited = edit_row == Some(tree_idx);
            let is_selected = vis_pos == self.cursor && show_cursor && !is_edited;

            // Tree prefix (├─, └─, etc.)
            let tree_prefix = self
                .tree_prefixes
                .get(vis_pos)
                .map(|s| s.as_str())
                .unwrap_or("");

            // Icon: ".." and what cannot be expanded = none, expanded = ▼,
            // collapsed = ▶
            let icon = match entry.expanded {
                Some(true) => "▼",
                Some(false) if entry.expandable => "▶",
                _ => " ",
            };

            // Cursor style: invert fg/bg like file manager
            let (row_style, prefix_row_style) = if is_selected {
                let style = Style::default()
                    .fg(theme.bg)
                    .bg(theme.fg)
                    .add_modifier(Modifier::BOLD);
                (style, style)
            } else {
                (Style::default().fg(theme.fg), prefix_style)
            };

            // The mark goes at the right end, before the margin column.
            let note = entry
                .note
                .as_deref()
                .filter(|_| !is_edited)
                .map(|n| format!(" {n} "))
                .unwrap_or_default();
            let text = if is_edited {
                let x = 1 + tree_prefix.width() as u16 + icon.width() as u16 + 1;
                let y = area.y + (vis_pos - self.scroll_offset) as u16;
                field_at = Some((area.x + x, y, area.width.saturating_sub(x + 1)));
                format!("{} ", icon)
            } else {
                // A root is a whole path: its end tells it apart, so it is
                // cut on the left; a name is cut on the right.
                let room = (area.width as usize)
                    .saturating_sub(1 + tree_prefix.width() + icon.width() + 1 + note.width());
                let name = if self.roots.is_some() && entry.depth == 0 {
                    termide_ui::path_utils::truncate_left(&entry.name, room)
                } else {
                    termide_ui::path_utils::truncate_right(&entry.name, room)
                };
                format!("{} {}", icon, name)
            };
            let line_width = 1 + tree_prefix.width() + text.width() + note.width();
            let padding = " ".repeat((area.width as usize).saturating_sub(line_width));

            let line = Line::from(vec![
                // A one-column gutter before the tree, as in the file manager.
                Span::styled(" ", row_style),
                Span::styled(tree_prefix, prefix_row_style),
                Span::styled(text, row_style),
                Span::styled(padding, row_style),
                Span::styled(note, prefix_row_style),
            ]);

            list_items.push(ListItem::new(line));
        }

        let list = List::new(list_items).style(Style::default().bg(theme.bg));
        list.render(area, buf);
        self.last_list_area = Some(area);

        if let Some(edit) = self.edit.as_mut() {
            edit.area = field_at.map(|(x, y, width)| Rect::new(x, y, width, 1));
            if let Some(area) = edit.area {
                render_text_input(buf, area, &mut edit.input, true, theme);
            }
        }
    }
}

/// A single path component: not empty, no separator, not `.` or `..`.
fn validate_name(name: &str) -> Result<(), String> {
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
