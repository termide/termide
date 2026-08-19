use std::collections::HashSet;
use std::path::PathBuf;

use super::FileManager;
use termide_git::GitStatus;
use termide_vfs::VfsPath;

/// Drag selection mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DragMode {
    Select, // Shift+drag - selection
    Toggle, // Ctrl+drag - toggle selection
}

/// Selection state for file manager (multi-select and drag selection)
#[derive(Clone, Default)]
pub(crate) struct SelectionState {
    /// Set of selected items (indices)
    pub(crate) items: HashSet<usize>,
    /// Starting index for drag selection
    pub(crate) drag_start: Option<usize>,
    /// Drag mode (Shift/Ctrl)
    drag_mode: Option<DragMode>,
    /// Set of items already processed during current drag (to avoid re-toggling)
    pub(crate) dragged: HashSet<usize>,
}

impl SelectionState {
    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    pub(crate) fn toggle(&mut self, index: usize) {
        if self.items.contains(&index) {
            self.items.remove(&index);
        } else {
            self.items.insert(index);
        }
    }

    pub(crate) fn select(&mut self, index: usize) {
        self.items.insert(index);
    }

    pub(crate) fn is_selected(&self, index: usize) -> bool {
        self.items.contains(&index)
    }

    pub(crate) fn start_shift_drag(&mut self, index: usize) {
        self.dragged.clear();
        self.select(index);
        self.dragged.insert(index);
        self.drag_start = Some(index);
        self.drag_mode = Some(DragMode::Select);
    }

    pub(crate) fn start_ctrl_drag(&mut self, index: usize) {
        self.toggle(index);
        self.drag_start = Some(index);
        self.drag_mode = Some(DragMode::Toggle);
        self.dragged.clear();
        self.dragged.insert(index);
    }

    pub(crate) fn end_drag(&mut self) {
        self.drag_start = None;
        self.drag_mode = None;
        self.dragged.clear();
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.drag_mode.is_some()
    }

    pub(crate) fn process_drag(&mut self, index: usize) -> bool {
        if !self.dragged.contains(&index) {
            match self.drag_mode {
                Some(DragMode::Select) => {
                    self.select(index);
                    self.dragged.insert(index);
                    true
                }
                Some(DragMode::Toggle) => {
                    self.toggle(index);
                    self.dragged.insert(index);
                    true
                }
                None => false,
            }
        } else {
            false
        }
    }
}

impl FileManager {
    /// Check if entry at visible index is ".." (not selectable)
    fn is_parent_entry(&self, vis_idx: usize) -> bool {
        self.entry_at(vis_idx).is_some_and(|e| e.name == "..")
    }

    /// Insert visible index into selection, skipping ".."
    fn select_index(&mut self, vis_idx: usize) {
        if !self.is_parent_entry(vis_idx) {
            self.selection.items.insert(vis_idx);
            self.sync_parent_selection(vis_idx);
        }
    }

    /// Range of visible descendants of an expanded directory at vis_idx (exclusive of dir itself).
    /// Returns empty range if not an expanded directory or has no visible children.
    fn visible_descendants_range(&self, vis_idx: usize) -> std::ops::Range<usize> {
        let tree_idx = match self.visible_indices.get(vis_idx) {
            Some(&idx) => idx,
            None => return vis_idx..vis_idx,
        };
        if self.tree_entries[tree_idx].expanded != Some(true) {
            return vis_idx..vis_idx;
        }
        let dir_depth = self.tree_entries[tree_idx].depth;
        let start = vis_idx + 1;
        let end = self.visible_indices[start..]
            .iter()
            .position(|&ti| self.tree_entries[ti].depth <= dir_depth)
            .map(|pos| start + pos)
            .unwrap_or(self.visible_indices.len());
        start..end
    }

    /// Find the visible index of the parent expanded directory for a given vis_idx.
    /// Walks backward through visible_indices looking for the first entry with a smaller depth.
    fn find_parent_dir_vis(&self, vis_idx: usize) -> Option<usize> {
        let tree_idx = *self.visible_indices.get(vis_idx)?;
        let my_depth = self.tree_entries[tree_idx].depth;
        if my_depth == 0 {
            return None;
        }
        for candidate_vis in (0..vis_idx).rev() {
            let candidate_tree = self.visible_indices[candidate_vis];
            if self.tree_entries[candidate_tree].depth < my_depth {
                return Some(candidate_vis);
            }
        }
        None
    }

    /// Synchronize parent directory selection state after a child's selection changed.
    /// If all visible descendants of a parent are selected, selects the parent.
    /// If any descendant is not selected, deselects the parent.
    /// Recurses upward through the directory hierarchy.
    fn sync_parent_selection(&mut self, vis_idx: usize) {
        let Some(parent_vis) = self.find_parent_dir_vis(vis_idx) else {
            return;
        };
        let mut descendants = self.visible_descendants_range(parent_vis);
        if descendants.is_empty() {
            return;
        }
        let all_selected = descendants.all(|i| self.selection.items.contains(&i));
        if all_selected {
            self.selection.items.insert(parent_vis);
        } else {
            self.selection.items.remove(&parent_vis);
        }
        self.sync_parent_selection(parent_vis);
    }

    /// Toggle a single visible index in selection, skipping ".."
    /// No cascade — used for element-by-element selection (Shift+arrows, page toggle).
    fn toggle_index_single(&mut self, vis_idx: usize) {
        if self.is_parent_entry(vis_idx) {
            return;
        }
        if self.selection.items.contains(&vis_idx) {
            self.selection.items.remove(&vis_idx);
        } else {
            self.selection.items.insert(vis_idx);
        }
        self.sync_parent_selection(vis_idx);
    }

    /// Toggle selection of current item and advance cursor (Insert key).
    /// For expanded directories, cascades to all visible descendants and skips past subtree.
    pub(crate) fn toggle_selection(&mut self) {
        if self.is_parent_entry(self.selected) {
            self.move_down();
            return;
        }
        let descendants = self.visible_descendants_range(self.selected);
        let adding = !self.selection.items.contains(&self.selected);
        if adding {
            self.selection.items.insert(self.selected);
            for i in descendants.clone() {
                if !self.is_parent_entry(i) {
                    self.selection.items.insert(i);
                }
            }
        } else {
            self.selection.items.remove(&self.selected);
            for i in descendants.clone() {
                self.selection.items.remove(&i);
            }
        }
        self.sync_parent_selection(self.selected);
        if !descendants.is_empty() {
            // Jump past the subtree to the next sibling-level entry
            let target = descendants.end.min(self.visible_count().saturating_sub(1));
            self.selected = target;
            self.adjust_scroll_offset(self.visible_height);
        } else {
            self.move_down();
        }
    }

    /// Select all visible files
    pub(crate) fn select_all(&mut self) {
        self.selection.items.clear();
        for vis_idx in 0..self.visible_count() {
            if let Some(entry) = self.entry_at(vis_idx) {
                if entry.name != ".." {
                    self.selection.items.insert(vis_idx);
                }
            }
        }
    }

    /// Move down with selection
    pub(crate) fn move_down_with_selection(&mut self) {
        self.select_index(self.selected);
        self.move_down();
    }

    /// Move up with selection
    pub(crate) fn move_up_with_selection(&mut self) {
        self.select_index(self.selected);
        self.move_up();
    }

    /// Page down with selection
    pub(crate) fn page_down_with_selection(&mut self) {
        let start = self.selected;
        let target =
            (self.selected + self.visible_height).min(self.visible_count().saturating_sub(1));
        for i in start..=target {
            self.select_index(i);
        }
        self.selected = target;
        self.adjust_scroll_offset(self.visible_height);
    }

    /// Page up with selection
    pub(crate) fn page_up_with_selection(&mut self) {
        let start = self.selected;
        let target = self.selected.saturating_sub(self.visible_height);
        for i in target..=start {
            self.select_index(i);
        }
        self.selected = target;
        self.scroll_offset = 0;
    }

    /// Select to beginning of list
    pub(crate) fn select_to_home(&mut self) {
        for i in 0..=self.selected {
            self.select_index(i);
        }
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Select to end of list
    pub(crate) fn select_to_end(&mut self) {
        let max_index = self.visible_count().saturating_sub(1);
        for i in self.selected..=max_index {
            self.select_index(i);
        }
        self.selected = max_index;
    }

    /// Get list of selected files/directories as absolute paths.
    /// If nothing is selected, return current item under cursor.
    ///
    /// Deduplicates: if a directory is selected, its selected descendants are omitted
    /// (the directory copy/move/delete already covers them recursively).
    pub fn get_selected_paths(&self) -> Vec<PathBuf> {
        if self.selection.items.is_empty() {
            if let Some(te) = self.tree_entry_at(self.selected) {
                if te.file_entry.name != ".." && te.file_entry.git_status != GitStatus::Deleted {
                    return vec![te.full_path.clone()];
                }
            }
            return Vec::new();
        }

        // Collect all selected paths (excluding deleted files)
        let mut paths: Vec<PathBuf> = Vec::with_capacity(self.selection.items.len());
        for &vis_idx in &self.selection.items {
            if let Some(te) = self.tree_entry_at(vis_idx) {
                if te.file_entry.name != ".." && te.file_entry.git_status != GitStatus::Deleted {
                    paths.push(te.full_path.clone());
                }
            }
        }

        // Remove paths whose parent directory is also in the selection.
        // Sort so that parent dirs come before their children.
        paths.sort();
        let mut deduped: Vec<PathBuf> = Vec::with_capacity(paths.len());
        for path in &paths {
            let dominated = deduped
                .iter()
                .any(|ancestor| path.starts_with(ancestor) && path != ancestor);
            if !dominated {
                deduped.push(path.clone());
            }
        }
        deduped
    }

    /// Get list of selected files/directories as VfsPath (for remote operations).
    /// If nothing is selected, return current item under cursor.
    ///
    /// Deduplicates: if a directory is selected, its selected descendants are omitted.
    pub fn get_selected_vfs_paths(&self) -> Vec<VfsPath> {
        let base_path = self.vfs.current_path();

        if self.selection.items.is_empty() {
            if let Some(te) = self.tree_entry_at(self.selected) {
                if te.file_entry.name != ".." && te.file_entry.git_status != GitStatus::Deleted {
                    return vec![base_path.join(&te.file_entry.name)];
                }
            }
            return Vec::new();
        }

        // Collect all selected tree entries with their full_path for dedup (excluding deleted)
        let mut entries: Vec<(PathBuf, String)> = Vec::with_capacity(self.selection.items.len());
        for &vis_idx in &self.selection.items {
            if let Some(te) = self.tree_entry_at(vis_idx) {
                if te.file_entry.name != ".." && te.file_entry.git_status != GitStatus::Deleted {
                    entries.push((te.full_path.clone(), te.file_entry.name.clone()));
                }
            }
        }

        // Sort by full_path so parents come before children
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut kept_ancestors: Vec<PathBuf> = Vec::with_capacity(entries.len());
        let mut paths = Vec::with_capacity(entries.len());
        for (full_path, name) in &entries {
            let dominated = kept_ancestors
                .iter()
                .any(|ancestor| full_path.starts_with(ancestor) && full_path != ancestor);
            if !dominated {
                kept_ancestors.push(full_path.clone());
                paths.push(base_path.join(name));
            }
        }
        paths
    }

    /// Get count of selected items
    pub fn get_selected_count(&self) -> usize {
        self.selection.items.len()
    }

    /// Select all visible descendants of an expanded directory.
    /// Used when expanding a directory that is already selected.
    pub(crate) fn select_descendants(&mut self, vis_idx: usize) {
        for i in self.visible_descendants_range(vis_idx) {
            self.select_index(i);
        }
    }

    /// Clear file selection
    pub fn clear_selection(&mut self) {
        self.selection.items.clear();
    }

    /// Move down with toggle selection
    pub(crate) fn move_down_with_toggle(&mut self) {
        self.toggle_index_single(self.selected);
        self.move_down();
    }

    /// Move up with toggle selection
    pub(crate) fn move_up_with_toggle(&mut self) {
        self.toggle_index_single(self.selected);
        self.move_up();
    }

    /// Page down with toggle selection
    pub(crate) fn page_down_with_toggle(&mut self) {
        let start = self.selected;
        let target =
            (self.selected + self.visible_height).min(self.visible_count().saturating_sub(1));

        for i in start..=target {
            self.toggle_index_single(i);
        }

        self.selected = target;
        self.adjust_scroll_offset(self.visible_height);
    }

    /// Page up with toggle selection
    pub(crate) fn page_up_with_toggle(&mut self) {
        let start = self.selected;
        let target = self.selected.saturating_sub(self.visible_height);

        for i in target..=start {
            self.toggle_index_single(i);
        }

        self.selected = target;
        self.scroll_offset = 0;
    }
}
