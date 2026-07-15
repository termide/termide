//! Result navigation, collapse, visual hit-testing, and scroll accounting.

use super::*;

impl FileSearchState {
    /// Navigate to the next selectable row (no wrap).
    pub fn next_result(&mut self) {
        if let Some(p) = ((self.cursor + 1)..self.tree_nodes.len()).find(|&p| self.is_selectable(p))
        {
            self.cursor = p;
            self.ensure_visible();
        }
    }

    /// Navigate to the previous selectable row (no wrap).
    pub fn prev_result(&mut self) {
        if let Some(p) = (0..self.cursor).rev().find(|&p| self.is_selectable(p)) {
            self.cursor = p;
            self.ensure_visible();
        }
    }

    /// Move the cursor down by up to `page` selectable rows (no wrap).
    pub fn page_down(&mut self, page: usize) {
        let len = self.tree_nodes.len();
        let mut pos = self.cursor;
        for _ in 0..page.max(1) {
            match ((pos + 1)..len).find(|&p| self.is_selectable(p)) {
                Some(p) => pos = p,
                None => break,
            }
        }
        if pos != self.cursor {
            self.cursor = pos;
            self.ensure_visible();
        }
    }

    /// Move the cursor up by up to `page` selectable rows (no wrap).
    pub fn page_up(&mut self, page: usize) {
        let mut pos = self.cursor;
        for _ in 0..page.max(1) {
            match (0..pos).rev().find(|&p| self.is_selectable(p)) {
                Some(p) => pos = p,
                None => break,
            }
        }
        if pos != self.cursor {
            self.cursor = pos;
            self.ensure_visible();
        }
    }

    /// Index of the collapsible node for the cursor: the file header in content
    /// mode, or the directory under the cursor in file-name mode.
    fn collapsible_index(&self) -> Option<usize> {
        match self.mode {
            FileSearchMode::Content => self.header_above(self.cursor),
            FileSearchMode::FileGlob => self
                .tree_nodes
                .get(self.cursor)
                .filter(|n| n.is_dir)
                .map(|_| self.cursor),
        }
    }

    /// Walk back from `from` to the nearest content file-header node.
    pub(super) fn header_above(&self, from: usize) -> Option<usize> {
        let mut h = from.min(self.tree_nodes.len().checked_sub(1)?);
        loop {
            if self.tree_nodes.get(h)?.is_file_header {
                return Some(h);
            }
            h = h.checked_sub(1)?;
        }
    }

    /// Collapse or expand the group at the cursor (content: file header;
    /// file-name: directory). Returns true if the state changed.
    pub fn set_collapse_at_cursor(&mut self, collapse: bool) -> bool {
        let Some(i) = self.collapsible_index() else {
            return false;
        };
        if self.tree_nodes[i].collapsed == collapse {
            return false;
        }
        self.tree_nodes[i].collapsed = collapse;
        if collapse {
            self.cursor = i; // keep the now-collapsed header/dir in focus
        }
        self.ensure_visible();
        true
    }

    /// Toggle the collapsed state of the group at the cursor.
    pub fn toggle_collapse_at_cursor(&mut self) -> bool {
        let Some(i) = self.collapsible_index() else {
            return false;
        };
        let collapsed = self.tree_nodes[i].collapsed;
        self.set_collapse_at_cursor(!collapsed)
    }

    /// Set the cursor to the row rendered `line_offset` visual lines below the
    /// current scroll position (for mouse clicks). Returns true if it landed on
    /// a selectable row.
    pub fn cursor_at_visual_line(&mut self, line_offset: usize) -> bool {
        match self.node_at_visual_line(line_offset) {
            Some(idx) => {
                self.cursor = idx;
                self.is_selectable(idx)
            }
            None => false,
        }
    }

    /// If a click at (`line_offset`, `col_offset`) — relative to the results
    /// area — lands on a collapse triangle, toggle that group and return true.
    /// Content headers carry the `[▼]` marker at column 0; file-name directories
    /// carry a `▶`/`▼` marker just after their tree prefix.
    pub fn toggle_collapse_at_visual_click(
        &mut self,
        line_offset: usize,
        col_offset: usize,
    ) -> bool {
        let Some(idx) = self.node_at_visual_line(line_offset) else {
            return false;
        };
        let node = &self.tree_nodes[idx];
        let marker = match self.mode {
            FileSearchMode::Content if node.is_file_header => {
                Some((TRIANGLE_COLS.start, TRIANGLE_COLS.end))
            }
            FileSearchMode::FileGlob if node.is_dir => {
                let p = self
                    .tree_prefixes
                    .get(idx)
                    .map(|s| s.chars().count())
                    .unwrap_or(0);
                Some((p, p + 2))
            }
            _ => None,
        };
        let Some((lo, hi)) = marker else {
            return false;
        };
        if col_offset >= lo && col_offset < hi {
            self.cursor = idx;
            self.tree_nodes[idx].collapsed = !self.tree_nodes[idx].collapsed;
            self.ensure_visible();
            return true;
        }
        false
    }

    /// The node rendered at visual line `line_offset` below the scroll offset.
    pub(super) fn node_at_visual_line(&self, line_offset: usize) -> Option<usize> {
        let mut acc = 0usize;
        let mut idx = self.scroll_offset;
        while idx < self.tree_nodes.len() {
            let h = self.node_display_lines(idx);
            if h == 0 {
                idx += 1;
                continue;
            }
            if line_offset < acc + h {
                return Some(idx);
            }
            acc += h;
            idx += 1;
        }
        None
    }

    /// Whether row `idx` can hold the navigation cursor: a file header in
    /// content mode, any visible node in file-name mode. Rows hidden under a
    /// collapsed group are never selectable.
    pub(super) fn is_selectable(&self, idx: usize) -> bool {
        let Some(node) = self.tree_nodes.get(idx) else {
            return false;
        };
        if self.is_hidden_by_collapse(idx) {
            return false;
        }
        match self.mode {
            FileSearchMode::Content => node.is_file_header,
            FileSearchMode::FileGlob => true,
        }
    }

    /// Bar counter: (current_index, total) over the selectable rows — files in
    /// content mode, entries in file-name mode.
    pub fn get_match_info(&self) -> Option<(usize, usize)> {
        let total = (0..self.tree_nodes.len())
            .filter(|&i| self.is_selectable(i))
            .count();
        if total == 0 {
            return None;
        }
        let cur = self.cursor.min(self.tree_nodes.len().saturating_sub(1));
        let current = (0..=cur)
            .filter(|&i| self.is_selectable(i))
            .count()
            .saturating_sub(1);
        Some((current, total))
    }

    /// Get the selected result for opening, or `None` when the cursor is on a
    /// collapsible-only row (a directory in file-name mode) — the caller then
    /// toggles collapse instead of opening.
    pub fn get_selected_result(&self) -> Option<SelectedSearchResult> {
        let node = self.tree_nodes.get(self.cursor)?;
        match self.mode {
            FileSearchMode::FileGlob => {
                if node.is_dir {
                    return Some(SelectedSearchResult::OpenDir(node.full_path.clone()));
                }
                Some(SelectedSearchResult::NavigateToFile(node.full_path.clone()))
            }
            FileSearchMode::Content => {
                // The cursor sits on a file header; open at the file's first
                // match line (the matches that follow, up to the next header).
                let line = self
                    .tree_nodes
                    .get(self.cursor + 1..)
                    .unwrap_or(&[])
                    .iter()
                    .take_while(|n| !n.is_file_header)
                    .find_map(|n| n.content_match.as_ref().map(|m| m.line_number))
                    .unwrap_or(1);
                Some(SelectedSearchResult::OpenAtLine {
                    path: node.full_path.clone(),
                    line,
                })
            }
        }
    }

    /// Max visible nodes for this mode
    pub fn max_visible_nodes(&self) -> usize {
        match self.mode {
            FileSearchMode::FileGlob => 15,
            FileSearchMode::Content => 40,
        }
    }

    /// How many display lines a node takes. Every visible node is a single
    /// line now (file header, match row, or file/dir); rows hidden under a
    /// collapsed header take none.
    pub fn node_display_lines(&self, idx: usize) -> usize {
        if idx >= self.tree_nodes.len() || self.is_hidden_by_collapse(idx) {
            return 0;
        }
        // While composing a replacement, every shown match expands to a
        // two-line -old/+new preview (diff-panel style).
        if self.has_replace_preview() && self.tree_nodes[idx].content_match.is_some() {
            return 2;
        }
        1
    }

    fn ensure_visible(&mut self) {
        let max_vis = self.max_visible_nodes();
        let lines_to_cursor = self.count_lines(self.scroll_offset, self.cursor);
        if lines_to_cursor >= max_vis {
            self.scroll_offset = self.find_scroll_for_cursor(max_vis);
        } else if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        }
    }

    fn count_lines(&self, from: usize, to: usize) -> usize {
        if to < from || from >= self.tree_nodes.len() {
            return 0;
        }
        let end = to.min(self.tree_nodes.len());
        let mut lines = 0;
        for i in from..end {
            lines += self.node_display_lines(i);
        }
        lines
    }

    fn find_scroll_for_cursor(&self, max_vis: usize) -> usize {
        let mut lines = self.node_display_lines(self.cursor);
        let mut start = self.cursor;
        while start > 0 && lines < max_vis {
            start -= 1;
            lines += self.node_display_lines(start);
        }
        if lines > max_vis && start < self.cursor {
            start + 1
        } else {
            start
        }
    }

    /// Whether row `idx` is hidden under a collapsed group: in content mode a
    /// match/overflow row whose file header is collapsed; in file-name mode a
    /// node nested under a collapsed ancestor directory.
    fn is_hidden_by_collapse(&self, idx: usize) -> bool {
        let Some(node) = self.tree_nodes.get(idx) else {
            return false;
        };
        match self.mode {
            FileSearchMode::Content => {
                if node.is_file_header {
                    return false;
                }
                for j in (0..idx).rev() {
                    if self.tree_nodes[j].is_file_header {
                        return self.tree_nodes[j].collapsed;
                    }
                }
                false
            }
            FileSearchMode::FileGlob => {
                // Walk up the ancestor chain (strictly-decreasing depth); hidden
                // if any ancestor directory is collapsed.
                let mut min_depth = node.depth;
                for j in (0..idx).rev() {
                    let a = &self.tree_nodes[j];
                    if a.depth < min_depth {
                        if a.is_dir && a.collapsed {
                            return true;
                        }
                        min_depth = a.depth;
                        if min_depth == 0 {
                            break;
                        }
                    }
                }
                false
            }
        }
    }
}
