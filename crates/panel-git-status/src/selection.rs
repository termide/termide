//! Selection, section navigation, cursor movement, and click hit-testing.

use std::path::PathBuf;

use unicode_width::UnicodeWidthStr;

use termide_core::PanelEvent;

use crate::tree;
use crate::types::{Section, Selection};
use crate::GitStatusPanel;

impl GitStatusPanel {
    /// Move to next section
    pub(crate) fn next_section(&mut self) {
        self.current_section = match self.current_section {
            Section::RepoSelector => Section::BranchSelector,
            Section::BranchSelector => {
                let total_files = self.unstaged_files.len() + self.staged_files.len();
                if total_files > 0 {
                    Section::Files
                } else {
                    Section::Buttons
                }
            }
            Section::Files => Section::Buttons,
            Section::Buttons => Section::RepoSelector,
        };
    }

    /// Move to previous section
    pub(crate) fn prev_section(&mut self) {
        self.current_section = match self.current_section {
            Section::RepoSelector => Section::Buttons,
            Section::BranchSelector => Section::RepoSelector,
            Section::Files => Section::BranchSelector,
            Section::Buttons => {
                let total_files = self.unstaged_files.len() + self.staged_files.len();
                if total_files > 0 {
                    Section::Files
                } else {
                    Section::BranchSelector
                }
            }
        };
    }

    /// Number of items in unstaged section (visible tree nodes)
    pub(crate) fn unstaged_item_count(&self) -> usize {
        self.unstaged.visible.len()
    }

    /// Number of items in staged section (visible tree nodes)
    pub(crate) fn staged_item_count(&self) -> usize {
        self.staged.visible.len()
    }

    /// Get current selection based on cursor position (virtual line)
    pub(crate) fn get_selection(&self) -> Option<Selection> {
        let unstaged_header = 0;
        let unstaged_start = 1;
        let unstaged_end = unstaged_start + self.unstaged_item_count();
        let staged_header = unstaged_end;
        let staged_start = staged_header + 1;

        if self.cursor == unstaged_header && !self.unstaged_files.is_empty() {
            Some(Selection::UnstagedHeader)
        } else if self.cursor >= unstaged_start && self.cursor < unstaged_end {
            let idx = self.cursor - unstaged_start;
            if let Some(&tree_idx) = self.unstaged.visible.get(idx) {
                match self.unstaged.tree[tree_idx].kind {
                    tree::TreeNodeKind::Directory { .. } => Some(Selection::UnstagedDir(tree_idx)),
                    tree::TreeNodeKind::File { file_index, .. } => {
                        Some(Selection::UnstagedFile(file_index))
                    }
                }
            } else {
                None
            }
        } else if self.cursor == staged_header && !self.staged_files.is_empty() {
            Some(Selection::StagedHeader)
        } else if self.cursor >= staged_start {
            let idx = self.cursor - staged_start;
            if let Some(&tree_idx) = self.staged.visible.get(idx) {
                match self.staged.tree[tree_idx].kind {
                    tree::TreeNodeKind::Directory { .. } => Some(Selection::StagedDir(tree_idx)),
                    tree::TreeNodeKind::File { file_index, .. } => {
                        Some(Selection::StagedFile(file_index))
                    }
                }
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Check if a virtual line is selectable (files and headers with buttons)
    pub(crate) fn is_selectable_line(&self, vline: usize) -> bool {
        let unstaged_end = 1 + self.unstaged_item_count();
        let staged_end = unstaged_end + 1 + self.staged_item_count();
        self.is_selectable_line_with_bounds(vline, unstaged_end, staged_end)
    }

    /// Check if a virtual line is selectable, using pre-calculated boundaries.
    /// Use this in loops to avoid recalculating counts every iteration.
    fn is_selectable_line_with_bounds(
        &self,
        vline: usize,
        unstaged_end: usize,
        staged_end: usize,
    ) -> bool {
        let unstaged_header = 0;
        let staged_header = unstaged_end;

        if vline == unstaged_header {
            !self.unstaged_files.is_empty()
        } else if vline == staged_header {
            !self.staged_files.is_empty()
        } else {
            vline > unstaged_header && vline < staged_end && vline != staged_header
        }
    }

    /// Check if there are any files (unstaged or staged)
    fn has_any_files(&self) -> bool {
        !self.unstaged_files.is_empty() || !self.staged_files.is_empty()
    }

    /// Get first selectable virtual line
    pub(crate) fn first_selectable_line(&self) -> usize {
        if !self.unstaged_files.is_empty() {
            0 // Unstaged header
        } else if !self.staged_files.is_empty() {
            1 // Staged header (vline = 1 when no unstaged files)
        } else {
            0
        }
    }

    /// Get last selectable virtual line
    pub(crate) fn last_selectable_line(&self) -> usize {
        let unstaged_end = 1 + self.unstaged_item_count();
        let staged_end = unstaged_end + 1 + self.staged_item_count();
        let total = self.total_virtual_lines();
        for vline in (0..total).rev() {
            if self.is_selectable_line_with_bounds(vline, unstaged_end, staged_end) {
                return vline;
            }
        }
        0
    }

    /// Find the nearest selectable line to the given position
    /// Prefers moving backward (up) when current line is not selectable
    pub(crate) fn find_nearest_selectable_line(&self, vline: usize) -> usize {
        let unstaged_end = 1 + self.unstaged_item_count();
        let staged_end = unstaged_end + 1 + self.staged_item_count();
        // Try moving backward first (more natural for cursor adjustment after refresh)
        for offset in 1..=vline {
            let target = vline - offset;
            if self.is_selectable_line_with_bounds(target, unstaged_end, staged_end) {
                return target;
            }
        }
        // If nothing found backward, try forward
        let total = self.total_virtual_lines();
        for target in (vline + 1)..total {
            if self.is_selectable_line_with_bounds(target, unstaged_end, staged_end) {
                return target;
            }
        }
        // Fallback to first selectable
        self.first_selectable_line()
    }

    /// Cursor position is the virtual line directly
    fn cursor_to_virtual_line(&self) -> usize {
        self.cursor
    }

    /// Ensure cursor is visible in viewport
    pub(crate) fn ensure_cursor_visible(&mut self) {
        if self.viewport_height == 0 {
            return;
        }
        let cursor_line = self.cursor_to_virtual_line();
        if cursor_line < self.scroll_offset {
            self.scroll_offset = cursor_line;
        } else if cursor_line >= self.scroll_offset + self.viewport_height {
            self.scroll_offset = cursor_line - self.viewport_height + 1;
        }
    }

    /// Get total virtual lines count (headers + items)
    pub(crate) fn total_virtual_lines(&self) -> usize {
        2 + self.unstaged_item_count() + self.staged_item_count()
    }

    /// Item count of the currently open selector dropdown (repo or branch).
    pub(crate) fn open_dropdown_len(&self) -> usize {
        if self.repo_dropdown_open {
            if self.show_repo_filter {
                self.filtered_repo_indices().len()
            } else {
                self.repo_manager.len()
            }
        } else if self.branch_dropdown_open {
            if self.show_branch_filter {
                self.filtered_branch_indices().len()
            } else {
                self.branches.len()
            }
        } else {
            0
        }
    }

    /// Get selected files from the given section (staged or unstaged).
    pub(crate) fn get_selected_files(&self, staged: bool) -> Vec<PathBuf> {
        match self.get_selection() {
            Some(Selection::UnstagedFile(idx)) if !staged => self
                .unstaged_files
                .get(idx)
                .map(|f| f.path.clone())
                .into_iter()
                .collect(),
            Some(Selection::UnstagedDir(idx)) if !staged => {
                tree::collect_files_under(&self.unstaged.tree, idx)
            }
            Some(Selection::StagedFile(idx)) if staged => self
                .staged_files
                .get(idx)
                .map(|f| f.path.clone())
                .into_iter()
                .collect(),
            Some(Selection::StagedDir(idx)) if staged => {
                tree::collect_files_under(&self.staged.tree, idx)
            }
            _ => vec![],
        }
    }

    // =========================================================================
    // Keyboard Navigation Helpers
    // =========================================================================

    /// Handle Up key navigation
    pub(crate) fn handle_up_key(&mut self) {
        match self.current_section {
            Section::RepoSelector => {
                if self.repo_dropdown_open && self.dropdown_cursor > 0 {
                    self.dropdown_cursor -= 1;
                }
            }
            Section::BranchSelector => {
                if self.branch_dropdown_open && self.dropdown_cursor > 0 {
                    self.dropdown_cursor -= 1;
                }
            }
            Section::Files => {
                let first = self.first_selectable_line();
                if self.cursor == first {
                    self.current_section = Section::BranchSelector;
                } else {
                    let mut new_cursor = self.cursor;
                    while new_cursor > 0 {
                        new_cursor -= 1;
                        if self.is_selectable_line(new_cursor) {
                            self.cursor = new_cursor;
                            self.ensure_cursor_visible();
                            break;
                        }
                    }
                }
            }
            Section::Buttons => {
                if self.has_any_files() {
                    self.current_section = Section::Files;
                    self.cursor = self.last_selectable_line();
                    self.ensure_cursor_visible();
                } else {
                    self.current_section = Section::BranchSelector;
                }
            }
        }
    }

    /// Handle Down key navigation
    pub(crate) fn handle_down_key(&mut self) {
        match self.current_section {
            Section::RepoSelector => {
                if self.repo_dropdown_open {
                    let len = if self.show_repo_filter {
                        self.filtered_repo_indices().len()
                    } else {
                        self.repo_manager.len()
                    };
                    if self.dropdown_cursor + 1 < len {
                        self.dropdown_cursor += 1;
                    }
                } else if self.has_any_files() {
                    self.current_section = Section::Files;
                    self.cursor = self.first_selectable_line();
                    self.ensure_cursor_visible();
                } else {
                    self.current_section = Section::Buttons;
                }
            }
            Section::BranchSelector => {
                if self.branch_dropdown_open {
                    let len = if self.show_branch_filter {
                        self.filtered_branch_indices().len()
                    } else {
                        self.branches.len()
                    };
                    if self.dropdown_cursor + 1 < len {
                        self.dropdown_cursor += 1;
                    }
                } else if self.has_any_files() {
                    self.current_section = Section::Files;
                    self.cursor = self.first_selectable_line();
                    self.ensure_cursor_visible();
                } else {
                    self.current_section = Section::Buttons;
                }
            }
            Section::Files => {
                let last = self.last_selectable_line();
                if self.cursor == last {
                    self.current_section = Section::Buttons;
                    let total = self.total_virtual_lines();
                    if total > self.viewport_height {
                        self.scroll_offset = total - self.viewport_height;
                    }
                } else {
                    let max = self.total_virtual_lines();
                    let mut new_cursor = self.cursor;
                    while new_cursor + 1 < max {
                        new_cursor += 1;
                        if self.is_selectable_line(new_cursor) {
                            self.cursor = new_cursor;
                            self.ensure_cursor_visible();
                            break;
                        }
                    }
                }
            }
            Section::Buttons => {
                // At bottom, do nothing
            }
        }
    }

    /// Handle Enter key
    pub(crate) fn handle_enter_key(&mut self) -> Vec<PanelEvent> {
        match self.current_section {
            Section::Files => {
                match self.get_selection() {
                    Some(Selection::UnstagedFile(_)) => self.do_stage(),
                    Some(Selection::StagedFile(_)) => self.do_unstage(),
                    Some(Selection::UnstagedDir(idx)) => self.toggle_dir_expand(true, idx),
                    Some(Selection::StagedDir(idx)) => self.toggle_dir_expand(false, idx),
                    _ => {}
                }
                vec![]
            }
            Section::RepoSelector => {
                if self.repo_dropdown_open {
                    // When filtering, resolve the highlighted position to a real
                    // repo index; if the filter matches nothing, close without
                    // switching instead of falling back to index 0.
                    let idx = if self.show_repo_filter {
                        self.filtered_repo_indices()
                            .get(self.dropdown_cursor)
                            .copied()
                    } else {
                        Some(self.dropdown_cursor)
                    };
                    if let Some(idx) = idx {
                        if idx != self.repo_manager.selected_index() {
                            self.repo_manager.select(idx);
                            self.refresh();
                        }
                    }
                    self.repo_dropdown_open = false;
                    self.reset_repo_filter();
                } else {
                    self.repo_dropdown_open = true;
                    self.branch_dropdown_open = false;
                    self.reset_branch_filter();
                    self.dropdown_cursor = self.repo_manager.selected_index();
                }
                vec![]
            }
            Section::BranchSelector => {
                if self.branch_dropdown_open {
                    // When filtering, resolve the highlighted position to a real
                    // branch index; if the filter matches nothing, close without
                    // checking out instead of falling back to index 0.
                    let idx = if self.show_branch_filter {
                        self.filtered_branch_indices()
                            .get(self.dropdown_cursor)
                            .copied()
                    } else {
                        Some(self.dropdown_cursor)
                    };
                    if let Some(idx) = idx {
                        self.switch_to_branch(idx);
                    }
                    self.branch_dropdown_open = false;
                    self.reset_branch_filter();
                } else {
                    self.branch_dropdown_open = true;
                    self.repo_dropdown_open = false;
                    self.reset_repo_filter();
                    self.dropdown_cursor = self
                        .branches
                        .iter()
                        .position(|b| Some(b.as_str()) == self.branch.as_deref())
                        .unwrap_or(0);
                }
                vec![]
            }
            Section::Buttons => self.execute_button(),
        }
    }

    /// Check if click column hits the expand/collapse icon of a tree directory node.
    /// Returns `Some((is_unstaged, tree_idx))` if the click is on a directory icon.
    pub(crate) fn check_dir_icon_click(
        &self,
        vline: usize,
        relative_col: usize,
    ) -> Option<(bool, usize)> {
        let unstaged_start = 1;
        let unstaged_end = unstaged_start + self.unstaged_item_count();
        let staged_start = unstaged_end + 1;

        let (is_unstaged, visible_idx) = if vline >= unstaged_start && vline < unstaged_end {
            (true, vline - unstaged_start)
        } else if vline >= staged_start {
            (false, vline - staged_start)
        } else {
            return None;
        };

        let ft = if is_unstaged {
            &self.unstaged
        } else {
            &self.staged
        };
        let (tree_nodes, visible, prefixes) = (&ft.tree, &ft.visible, &ft.prefixes);

        let &tree_idx = visible.get(visible_idx)?;
        if !matches!(
            tree_nodes[tree_idx].kind,
            tree::TreeNodeKind::Directory { .. }
        ) {
            return None;
        }

        // Rendering layout: " {prefix}{arrow} /{name}"
        // Arrow icon is at column 1 + prefix_width
        let prefix_width = prefixes.get(visible_idx).map(|p| p.width()).unwrap_or(0);
        let icon_end = 1 + prefix_width + 1; // " " + prefix + arrow char

        if relative_col <= icon_end {
            Some((is_unstaged, tree_idx))
        } else {
            None
        }
    }

    /// Check if current click is a double-click on the same item
    pub(crate) fn check_double_click(&self, now: std::time::Instant, vline: usize) -> bool {
        self.click_tracker.is_double_click_at(now, &vline)
    }

    /// Reset double-click tracking state
    pub(crate) fn reset_click_state(&mut self) {
        self.click_tracker.reset();
    }

    /// Record click for double-click detection
    pub(crate) fn record_click(&mut self, now: std::time::Instant, vline: usize) {
        self.click_tracker.record_at(now, vline);
    }
}
