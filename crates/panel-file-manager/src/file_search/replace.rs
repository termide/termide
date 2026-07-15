//! Content-replace: per-file selection, preview, and apply-to-disk.

use super::*;

impl FileSearchState {
    // === Content replace: per-file selection ===

    /// Enable/disable per-file selection checkboxes (content replace mode).
    pub fn set_replace_mode(&mut self, on: bool) {
        self.show_checkboxes = on;
        if !on {
            self.selected_headers.clear();
        }
    }

    /// Whether the file header at `idx` is selected for replacement.
    pub fn is_header_selected(&self, idx: usize) -> bool {
        self.selected_headers.contains(&idx)
    }

    /// Whether every file header is selected (and there is at least one).
    pub fn all_selected(&self) -> bool {
        let mut any = false;
        for (i, n) in self.tree_nodes.iter().enumerate() {
            if n.is_file_header {
                any = true;
                if !self.selected_headers.contains(&i) {
                    return false;
                }
            }
        }
        any
    }

    /// The content file header at or above the cursor.
    fn header_index_at_cursor(&self) -> Option<usize> {
        if self.mode != FileSearchMode::Content {
            return None;
        }
        self.header_above(self.cursor)
    }

    /// Toggle selection of the file group at or above the cursor.
    pub fn toggle_selected_at_cursor(&mut self) {
        if let Some(h) = self.header_index_at_cursor() {
            if !self.selected_headers.remove(&h) {
                self.selected_headers.insert(h);
            }
        }
    }

    /// Select or deselect every file group.
    pub fn set_all_selected(&mut self, on: bool) {
        self.selected_headers.clear();
        if on {
            for (i, n) in self.tree_nodes.iter().enumerate() {
                if n.is_file_header {
                    self.selected_headers.insert(i);
                }
            }
        }
    }

    /// (files, matches) over the selected files — for the replace confirmation.
    pub fn selected_summary(&self) -> (usize, usize) {
        let mut files = 0;
        let mut matches = 0;
        for &i in &self.selected_headers {
            if let Some(n) = self.tree_nodes.get(i) {
                if n.is_file_header {
                    files += 1;
                    matches += n.match_count;
                }
            }
        }
        (files, matches)
    }

    /// If a click lands on a file's selection checkbox (just after the collapse
    /// marker, `CHECKBOX_COLS`), toggle its selection and return true.
    pub fn toggle_selection_at_visual_click(
        &mut self,
        line_offset: usize,
        col_offset: usize,
    ) -> bool {
        if !self.show_checkboxes {
            return false;
        }
        let Some(idx) = self.node_at_visual_line(line_offset) else {
            return false;
        };
        if self.mode == FileSearchMode::Content
            && self.tree_nodes[idx].is_file_header
            && CHECKBOX_COLS.contains(&col_offset)
        {
            self.cursor = idx;
            if !self.selected_headers.remove(&idx) {
                self.selected_headers.insert(idx);
            }
            return true;
        }
        false
    }

    /// Store the in-progress replacement text (Content mode), used for the
    /// preview and as the default for apply.
    pub fn set_replace_text(&mut self, text: Option<String>) {
        self.replace_text = text;
    }

    /// True when a non-empty replacement is being composed — the cursor match
    /// then shows a `-old/+new` preview.
    pub fn has_replace_preview(&self) -> bool {
        self.replace_text.as_deref().is_some_and(|t| !t.is_empty())
    }

    /// Compile the stored content pattern with the active case sensitivity.
    fn compiled_pattern(&self) -> Option<regex::Regex> {
        RegexBuilder::new(&self.search_pattern)
            .case_insensitive(!self.search_case_sensitive)
            .build()
            .ok()
    }

    /// Apply the replacement to `hay`: regex mode expands `$1` / `${name}`
    /// capture groups, literal mode inserts `rep` verbatim.
    fn apply_replacement(&self, re: &regex::Regex, hay: &str, rep: &str) -> String {
        if self.search_use_regex {
            re.replace_all(hay, rep).into_owned()
        } else {
            re.replace_all(hay, regex::NoExpand(rep)).into_owned()
        }
    }

    /// Compute the post-replace version of `matched_line` for the preview.
    /// Returns `None` when no replacement is active.
    pub fn preview_replacement(&self, matched_line: &str) -> Option<String> {
        let rep = self.replace_text.as_deref()?;
        if rep.is_empty() {
            return None;
        }
        let re = self.compiled_pattern()?;
        Some(self.apply_replacement(&re, matched_line, rep))
    }

    /// Apply `replace_with` to every matched file on disk, re-matching at
    /// apply time. Returns (files_changed, occurrences_replaced).
    pub fn replace_all(&self, replace_with: &str) -> (usize, usize) {
        if self.mode != FileSearchMode::Content || self.search_pattern.is_empty() {
            return (0, 0);
        }
        let re = match self.compiled_pattern() {
            Some(r) => r,
            None => return (0, 0),
        };

        let mut files_changed = 0;
        let mut occurrences = 0;
        for (idx, node) in self.tree_nodes.iter().enumerate() {
            if !node.is_file_header || !self.selected_headers.contains(&idx) {
                continue;
            }
            let content = match std::fs::read_to_string(&node.full_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let n = re.find_iter(&content).count();
            if n == 0 {
                continue;
            }
            let new_content = self.apply_replacement(&re, &content, replace_with);
            if new_content != content && std::fs::write(&node.full_path, new_content).is_ok() {
                files_changed += 1;
                occurrences += n;
            }
        }
        (files_changed, occurrences)
    }
}
