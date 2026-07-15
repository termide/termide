//! File and content search state for file manager.
//!
//! Replaces TreeSearchModal's result display — search results are shown
//! in the file manager panel instead of a modal.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use regex::RegexBuilder;
use termide_core::util::is_binary_file;
use termide_git::{get_git_status, GitStatus, GitStatusCache};

/// Maximum total results to collect
const MAX_RESULTS: usize = 500;

/// Content-header hit-test columns (must match the renderer): the
/// `[▼]`/`[▶]` collapse triangle occupies columns 0..4, the `[ ]`/`[x]`
/// selection checkbox columns 4..8.
const TRIANGLE_COLS: std::ops::Range<usize> = 0..4;
const CHECKBOX_COLS: std::ops::Range<usize> = 4..8;

/// Content match info for a single line match
#[derive(Debug, Clone)]
pub(crate) struct ContentMatch {
    pub line_number: usize,
    pub matched_line: String,
    pub match_start: usize,
    pub match_end: usize,
}

/// A node in the search result tree
#[derive(Debug, Clone)]
pub(crate) struct ResultTreeNode {
    pub name: String,
    pub full_path: PathBuf,
    pub depth: usize,
    pub is_dir: bool,
    pub git_status: GitStatus,
    pub content_match: Option<ContentMatch>,
    /// Content mode only: this node is a per-file group header (path + count),
    /// not a match row. Its `match_count` matches rows follow it.
    pub is_file_header: bool,
    /// Number of matches in the file (only meaningful for `is_file_header`).
    pub match_count: usize,
    /// Content mode only: a collapsed header hides its match rows.
    pub collapsed: bool,
}

/// Search mode for file search
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileSearchMode {
    FileGlob,
    Content,
}

/// Search results from background thread
enum SearchResults {
    FileResults(Vec<FileResult>),
    ContentResults(Vec<ContentResult>),
}

#[derive(Debug, Clone)]
struct FileResult {
    full_path: PathBuf,
    relative_path: String,
    git_status: GitStatus,
    is_dir: bool,
}

#[derive(Debug, Clone)]
struct ContentResult {
    full_path: PathBuf,
    relative_path: String,
    line_number: usize,
    matched_line: String,
    match_start: usize,
    match_end: usize,
    git_status: GitStatus,
}

/// Persistent search state for file manager
pub(crate) struct FileSearchState {
    pub mode: FileSearchMode,
    pub tree_nodes: Vec<ResultTreeNode>,
    pub tree_prefixes: Vec<String>,
    pub result_count: usize,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub is_searching: bool,
    search_receiver: Option<mpsc::Receiver<SearchResults>>,
    search_cancel: Option<Arc<AtomicBool>>,
    base_path: PathBuf,
    max_file_size: u64,
    /// Content replace: text typed in the Replace field (for preview/apply).
    replace_text: Option<String>,
    /// Effective regex pattern used by the last content search (escaped when
    /// literal), kept so replace can re-match files.
    search_pattern: String,
    /// Whether the last content search treated the query as a regex.
    search_use_regex: bool,
    /// Case sensitivity of the last content search.
    search_case_sensitive: bool,
    /// Content replace mode: show per-file selection checkboxes.
    pub show_checkboxes: bool,
    /// Indices of selected file headers (content replace; default empty).
    selected_headers: std::collections::HashSet<usize>,
}

impl std::fmt::Debug for FileSearchState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSearchState")
            .field("mode", &self.mode)
            .field("result_count", &self.result_count)
            .field("cursor", &self.cursor)
            .field("is_searching", &self.is_searching)
            .finish()
    }
}

impl FileSearchState {
    /// Create new file glob search state
    pub fn new_file_glob(base_path: PathBuf) -> Self {
        Self {
            mode: FileSearchMode::FileGlob,
            tree_nodes: Vec::new(),
            tree_prefixes: Vec::new(),
            result_count: 0,
            cursor: 0,
            scroll_offset: 0,
            is_searching: false,
            search_receiver: None,
            search_cancel: None,
            base_path,
            max_file_size: 0,
            replace_text: None,
            search_pattern: String::new(),
            search_use_regex: false,
            search_case_sensitive: false,
            show_checkboxes: false,
            selected_headers: std::collections::HashSet::new(),
        }
    }

    /// Create new content search state
    pub fn new_content(base_path: PathBuf, max_file_size: u64) -> Self {
        Self {
            mode: FileSearchMode::Content,
            tree_nodes: Vec::new(),
            tree_prefixes: Vec::new(),
            result_count: 0,
            cursor: 0,
            scroll_offset: 0,
            is_searching: false,
            search_receiver: None,
            search_cancel: None,
            base_path,
            max_file_size,
            replace_text: None,
            search_pattern: String::new(),
            search_use_regex: false,
            search_case_sensitive: false,
            show_checkboxes: false,
            selected_headers: std::collections::HashSet::new(),
        }
    }

    /// Start file search in background thread
    pub fn start_file_search(&mut self, mask: &str, use_regex: bool, case_sensitive: bool) {
        if mask.is_empty() {
            self.tree_nodes.clear();
            self.tree_prefixes.clear();
            self.result_count = 0;
            self.cursor = 0;
            self.scroll_offset = 0;
            self.is_searching = false;
            return;
        }

        // Cancel previous search
        if let Some(cancel) = self.search_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.search_cancel = Some(cancel.clone());

        let (tx, rx) = mpsc::channel();
        let base_path = self.base_path.clone();
        let mask = mask.to_string();

        self.search_receiver = Some(rx);
        self.is_searching = true;

        std::thread::spawn(move || {
            // Build the git status cache on the worker thread so the
            // search panel opens without blocking the UI on a slow
            // `git status` (large or network-mounted repos).
            let git_cache = get_git_status(&base_path);
            let results = search_files(
                &base_path,
                &mask,
                use_regex,
                case_sensitive,
                &cancel,
                git_cache.as_ref(),
            );
            if !cancel.load(Ordering::Relaxed) {
                let _ = tx.send(SearchResults::FileResults(results));
            }
        });
    }

    /// Start content search in background thread
    pub fn start_content_search(
        &mut self,
        mask: &str,
        content_pattern: &str,
        use_regex: bool,
        case_sensitive: bool,
    ) {
        if mask.is_empty() || content_pattern.is_empty() {
            self.tree_nodes.clear();
            self.tree_prefixes.clear();
            self.result_count = 0;
            self.cursor = 0;
            self.scroll_offset = 0;
            self.is_searching = false;
            return;
        }

        // Literal search escapes the query; regex uses it verbatim.
        let pattern = if use_regex {
            content_pattern.to_string()
        } else {
            regex::escape(content_pattern)
        };

        // Validate the (effective) regex with the requested case sensitivity.
        if RegexBuilder::new(&pattern)
            .case_insensitive(!case_sensitive)
            .build()
            .is_err()
        {
            return;
        }

        // Remember how this search matched, so replace can re-match files.
        self.search_pattern = pattern.clone();
        self.search_use_regex = use_regex;
        self.search_case_sensitive = case_sensitive;
        self.replace_text = None;

        // Cancel previous search
        if let Some(cancel) = self.search_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }

        let cancel = Arc::new(AtomicBool::new(false));
        self.search_cancel = Some(cancel.clone());

        let (tx, rx) = mpsc::channel();
        let base_path = self.base_path.clone();
        let mask = mask.to_string();
        let max_file_size = self.max_file_size;

        self.search_receiver = Some(rx);
        self.is_searching = true;

        std::thread::spawn(move || {
            let git_cache = get_git_status(&base_path);
            let results = search_content(
                &base_path,
                &mask,
                &pattern,
                case_sensitive,
                &cancel,
                git_cache.as_ref(),
                max_file_size,
            );
            if !cancel.load(Ordering::Relaxed) {
                let _ = tx.send(SearchResults::ContentResults(results));
            }
        });
    }

    /// Poll for search results (call from tick())
    pub fn poll_results(&mut self) -> bool {
        if let Some(rx) = &self.search_receiver {
            match rx.try_recv() {
                Ok(results) => {
                    match results {
                        SearchResults::FileResults(items) => {
                            self.build_file_tree(items);
                        }
                        SearchResults::ContentResults(items) => {
                            self.build_content_tree(items);
                        }
                    }
                    self.cursor = 0;
                    self.scroll_offset = 0;
                    self.is_searching = false;
                    self.search_receiver = None;
                    // Move cursor to the first selectable row.
                    if let Some(i) = (0..self.tree_nodes.len()).find(|&i| self.is_selectable(i)) {
                        self.cursor = i;
                    }
                    return true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.is_searching = false;
                    self.search_receiver = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        false
    }

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
    fn header_above(&self, from: usize) -> Option<usize> {
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
    fn node_at_visual_line(&self, line_offset: usize) -> Option<usize> {
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

    /// Whether row `idx` can hold the navigation cursor: a file header in
    /// content mode, any visible node in file-name mode. Rows hidden under a
    /// collapsed group are never selectable.
    fn is_selectable(&self, idx: usize) -> bool {
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

    fn build_file_tree(&mut self, items: Vec<FileResult>) {
        self.result_count = items.len();
        let (nodes, prefixes) = build_tree_nodes(
            items
                .iter()
                .map(|i| TreeBuildItem {
                    relative_path: &i.relative_path,
                    full_path: &i.full_path,
                    git_status: i.git_status,
                    is_dir: i.is_dir,
                    content_match: None,
                })
                .collect(),
        );
        self.tree_nodes = nodes;
        self.tree_prefixes = prefixes;
    }

    /// Build the content-search display list: one collapsible header row per
    /// file (relative path + match count) followed by one row per match
    /// (line number + matched line). `items` arrive sorted by relative path,
    /// so files are already grouped.
    fn build_content_tree(&mut self, items: Vec<ContentResult>) {
        self.result_count = items.len();

        // Count matches per file so the header can show the total.
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for it in &items {
            *counts.entry(it.relative_path.as_str()).or_default() += 1;
        }

        // Show at most this many match rows per file; the rest collapse into a
        // single "+ N more" row (the header keeps the true total).
        const MAX_SHOWN_PER_FILE: usize = 5;

        let mut nodes: Vec<ResultTreeNode> = Vec::new();
        let mut current_file: Option<String> = None;
        let mut shown = 0usize;
        let mut overflow_added = false;
        for it in &items {
            let total = counts.get(it.relative_path.as_str()).copied().unwrap_or(0);
            if current_file.as_deref() != Some(it.relative_path.as_str()) {
                current_file = Some(it.relative_path.clone());
                shown = 0;
                overflow_added = false;
                nodes.push(ResultTreeNode {
                    name: it.relative_path.clone(),
                    full_path: it.full_path.clone(),
                    depth: 0,
                    is_dir: false,
                    git_status: it.git_status,
                    content_match: None,
                    is_file_header: true,
                    match_count: total,
                    collapsed: false,
                });
            }
            if shown < MAX_SHOWN_PER_FILE {
                nodes.push(ResultTreeNode {
                    name: String::new(),
                    full_path: it.full_path.clone(),
                    depth: 1,
                    is_dir: false,
                    git_status: it.git_status,
                    content_match: Some(ContentMatch {
                        line_number: it.line_number,
                        matched_line: it.matched_line.clone(),
                        match_start: it.match_start,
                        match_end: it.match_end,
                    }),
                    is_file_header: false,
                    match_count: 0,
                    collapsed: false,
                });
                shown += 1;
            } else if !overflow_added {
                overflow_added = true;
                // A "+ N more" context row (no content_match → not selectable).
                nodes.push(ResultTreeNode {
                    name: format!("+ {} more", total - MAX_SHOWN_PER_FILE),
                    full_path: it.full_path.clone(),
                    depth: 1,
                    is_dir: false,
                    git_status: it.git_status,
                    content_match: None,
                    is_file_header: false,
                    match_count: 0,
                    collapsed: false,
                });
            }
        }

        self.tree_prefixes = vec![String::new(); nodes.len()];
        self.tree_nodes = nodes;
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

/// Result when user selects a search result
#[derive(Debug, Clone)]
pub(crate) enum SelectedSearchResult {
    NavigateToFile(PathBuf),
    OpenAtLine {
        path: PathBuf,
        line: usize,
    },
    /// Enter (cd into) a directory result.
    OpenDir(PathBuf),
}

// ─── Tree building ───────────────────────────────────────────────────────

struct TreeBuildItem<'a> {
    relative_path: &'a str,
    full_path: &'a Path,
    git_status: GitStatus,
    is_dir: bool,
    content_match: Option<ContentMatch>,
}

fn build_tree_nodes(items: Vec<TreeBuildItem<'_>>) -> (Vec<ResultTreeNode>, Vec<String>) {
    if items.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut nodes: Vec<ResultTreeNode> = Vec::new();
    let mut added_dirs: HashSet<PathBuf> = HashSet::new();

    for item in &items {
        let rel_path = Path::new(item.relative_path);
        let components: Vec<&std::ffi::OsStr> = rel_path.iter().collect();

        // Add ancestor directories
        for depth in 0..components.len().saturating_sub(1) {
            let dir_path: PathBuf = components[..=depth].iter().collect();
            if !added_dirs.contains(&dir_path) {
                added_dirs.insert(dir_path.clone());
                let dir_name = components[depth].to_string_lossy().into_owned();
                nodes.push(ResultTreeNode {
                    name: dir_name,
                    full_path: Path::new(item.full_path)
                        .ancestors()
                        .nth(components.len() - 1 - depth)
                        .unwrap_or(item.full_path)
                        .to_path_buf(),
                    depth,
                    is_dir: true,
                    git_status: GitStatus::Unmodified,
                    content_match: None,
                    is_file_header: false,
                    match_count: 0,
                    collapsed: false,
                });
            }
        }

        // Add the item itself
        let depth = components.len().saturating_sub(1);
        let name = components
            .last()
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default();

        nodes.push(ResultTreeNode {
            name,
            full_path: item.full_path.to_path_buf(),
            depth,
            is_dir: item.is_dir,
            git_status: item.git_status,
            content_match: item.content_match.clone(),
            is_file_header: false,
            match_count: 0,
            collapsed: false,
        });
    }

    // Sort by path to maintain tree structure
    nodes.sort_by(|a, b| a.full_path.cmp(&b.full_path));

    // Deduplicate dir nodes
    nodes.dedup_by(|a, b| a.is_dir && b.is_dir && a.full_path == b.full_path);

    let prefixes = compute_tree_prefixes(&nodes);
    (nodes, prefixes)
}

fn compute_tree_prefixes(nodes: &[ResultTreeNode]) -> Vec<String> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let max_depth = nodes.iter().map(|n| n.depth).max().unwrap_or(0);
    if max_depth == 0 {
        return vec![String::new(); nodes.len()];
    }

    let mut has_next_at_level = vec![false; max_depth + 1];
    let mut prefixes: Vec<String> = Vec::with_capacity(nodes.len());

    for node in nodes.iter().rev() {
        let depth = node.depth;

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
                    prefix.push_str("├─ ");
                } else {
                    prefix.push_str("└─ ");
                }
            } else if *has_next {
                prefix.push_str("│  ");
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

// ─── Background search functions ─────────────────────────────────────────

/// Matches a path's file name (or relative path) against a search mask.
///
/// Shared by the name and content walkers so both apply the same
/// path-glob / name-glob / substring rules and honor the `[Aa]` Case toggle
/// identically. `use_regex` (name search with `[.*]` on) matches the name as
/// a regular expression; content search always passes `use_regex = false`
/// because its mask is only ever a glob (the regex there matches file
/// *contents*, not names).
struct NameMatcher {
    has_path_sep: bool,
    has_wildcards: bool,
    case_sensitive: bool,
    /// Some only in name-regex mode; matched against the name / relative path.
    regex: Option<regex::Regex>,
    /// Glob/substring needle, case-folded when Case is off.
    glob: Option<glob::Pattern>,
    needle: String,
}

impl NameMatcher {
    /// Returns `None` only when a regex mask fails to compile (the caller then
    /// yields no results); a glob that fails to parse simply matches nothing.
    fn new(mask: &str, use_regex: bool, case_sensitive: bool) -> Option<Self> {
        let regex = if use_regex {
            match RegexBuilder::new(mask)
                .case_insensitive(!case_sensitive)
                .build()
            {
                Ok(r) => Some(r),
                Err(_) => return None,
            }
        } else {
            None
        };
        // Fold case into the needle when Case is off so pattern and candidate
        // are compared in the same case.
        let needle = if case_sensitive {
            mask.to_string()
        } else {
            mask.to_lowercase()
        };
        let glob = if use_regex {
            None
        } else {
            glob::Pattern::new(&needle).ok()
        };
        Some(Self {
            has_path_sep: mask.contains('/') || mask.contains('\\'),
            has_wildcards: mask.contains('*') || mask.contains('?'),
            case_sensitive,
            regex,
            glob,
            needle,
        })
    }

    /// `Some(matched)`, or `None` when the entry has no usable file name.
    fn matches(&self, path: &Path, relative_path: &str) -> Option<bool> {
        if let Some(re) = self.regex.as_ref() {
            // Match the path when the pattern spans separators, else the name.
            return Some(if self.has_path_sep {
                re.is_match(relative_path)
            } else {
                re.is_match(&path.file_name()?.to_string_lossy())
            });
        }
        if self.has_path_sep {
            let hay = if self.case_sensitive {
                relative_path.to_string()
            } else {
                relative_path.to_lowercase()
            };
            return Some(self.glob.as_ref().map(|g| g.matches(&hay)).unwrap_or(false));
        }
        let name = path.file_name()?.to_string_lossy();
        Some(if self.has_wildcards {
            let hay = if self.case_sensitive {
                name.into_owned()
            } else {
                name.to_lowercase()
            };
            self.glob.as_ref().map(|g| g.matches(&hay)).unwrap_or(false)
        } else if self.case_sensitive {
            name.contains(&self.needle)
        } else {
            name.to_lowercase().contains(&self.needle)
        })
    }
}

fn search_files(
    base_path: &Path,
    mask: &str,
    use_regex: bool,
    case_sensitive: bool,
    cancel: &AtomicBool,
    git_cache: Option<&GitStatusCache>,
) -> Vec<FileResult> {
    use ignore::WalkBuilder;

    let Some(matcher) = NameMatcher::new(mask, use_regex, case_sensitive) else {
        return Vec::new();
    };
    let mut results = Vec::new();

    let walker = WalkBuilder::new(base_path)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .build();

    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if path == base_path {
            continue;
        }

        let relative_path = path
            .strip_prefix(base_path)
            .map(|r| r.display().to_string())
            .unwrap_or_default();

        if matcher.matches(path, &relative_path) != Some(true) {
            continue;
        }

        let is_dir = path.is_dir();
        let git_status = git_cache
            .map(|cache| {
                if is_dir {
                    cache.get_directory_status(&relative_path)
                } else {
                    cache.get_status(&relative_path)
                }
            })
            .unwrap_or(GitStatus::Unmodified);

        results.push(FileResult {
            full_path: path.to_path_buf(),
            relative_path,
            git_status,
            is_dir,
        });

        if results.len() >= MAX_RESULTS {
            break;
        }
    }

    results.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    results
}

fn search_content(
    base_path: &Path,
    mask: &str,
    content_pattern: &str,
    case_sensitive: bool,
    cancel: &AtomicBool,
    git_cache: Option<&GitStatusCache>,
    max_file_size: u64,
) -> Vec<ContentResult> {
    use ignore::WalkBuilder;

    let regex = match RegexBuilder::new(content_pattern)
        .case_insensitive(!case_sensitive)
        .build()
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    // The mask filters file names with the same rules (and Case toggle) as the
    // name search; the content `regex` above is what honors case for matches.
    let matcher = match NameMatcher::new(mask, false, case_sensitive) {
        Some(m) => m,
        None => return Vec::new(),
    };

    let min_size = content_pattern.len() as u64;
    let mut results = Vec::new();

    let walker = WalkBuilder::new(base_path)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .build();

    for entry in walker {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if path.is_dir() {
            continue;
        }

        let relative_path = path
            .strip_prefix(base_path)
            .map(|r| r.display().to_string())
            .unwrap_or_default();

        if matcher.matches(path, &relative_path) != Some(true) {
            continue;
        }

        if should_skip_file(path, max_file_size, min_size) {
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let lines: Vec<&str> = content.lines().collect();
        let git_status = git_cache
            .map(|cache| cache.get_status(&relative_path))
            .unwrap_or(GitStatus::Unmodified);

        for (line_idx, line) in lines.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }

            if let Some(m) = regex.find(line) {
                results.push(ContentResult {
                    full_path: path.to_path_buf(),
                    relative_path: relative_path.clone(),
                    line_number: line_idx + 1,
                    matched_line: line.to_string(),
                    match_start: m.start(),
                    match_end: m.end(),
                    git_status,
                });

                if results.len() >= MAX_RESULTS {
                    return results;
                }
            }
        }

        if results.len() >= MAX_RESULTS {
            break;
        }
    }

    results
}

fn should_skip_file(path: &Path, max_size: u64, min_size: u64) -> bool {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return true,
    };
    let file_size = meta.len();
    if file_size < min_size {
        return true;
    }
    if max_size > 0 && file_size > max_size {
        return true;
    }
    is_binary_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(path: PathBuf) -> ResultTreeNode {
        ResultTreeNode {
            name: path.display().to_string(),
            full_path: path,
            depth: 0,
            is_dir: false,
            git_status: GitStatus::Unmodified,
            content_match: None,
            is_file_header: true,
            match_count: 1,
            collapsed: false,
        }
    }

    fn match_row(path: PathBuf, line: usize) -> ResultTreeNode {
        ResultTreeNode {
            name: String::new(),
            full_path: path,
            depth: 1,
            is_dir: false,
            git_status: GitStatus::Unmodified,
            content_match: Some(ContentMatch {
                line_number: line,
                matched_line: "hit".to_string(),
                match_start: 0,
                match_end: 3,
            }),
            is_file_header: false,
            match_count: 0,
            collapsed: false,
        }
    }

    /// [H0, m1, m2, H3, m4] — two files, three matches.
    fn grouped_state() -> FileSearchState {
        let f1 = PathBuf::from("a.txt");
        let f2 = PathBuf::from("b.txt");
        let mut s = FileSearchState::new_content(PathBuf::from("."), 1 << 20);
        s.tree_nodes = vec![
            header(f1.clone()),
            match_row(f1.clone(), 1),
            match_row(f1, 2),
            header(f2.clone()),
            match_row(f2, 3),
        ];
        s.result_count = 3;
        s.cursor = 1; // first match
        s
    }

    #[test]
    fn content_navigation_lands_on_file_headers() {
        let mut s = grouped_state();
        s.cursor = 0; // first header
        assert!(s.is_selectable(0));
        assert!(!s.is_selectable(1)); // match rows aren't selectable
        s.next_result();
        assert_eq!(s.cursor, 3, "next lands on the second file header");
        s.next_result();
        assert_eq!(s.cursor, 3, "no wrap past the last header");
    }

    #[test]
    fn collapsing_a_header_keeps_focus_and_skips_hidden_in_nav() {
        let mut s = grouped_state();
        s.cursor = 0;
        assert!(s.set_collapse_at_cursor(true));
        assert_eq!(s.cursor, 0);
        assert!(s.tree_nodes[0].collapsed);
        // Matches under the collapsed header are hidden, but the next header is
        // still reachable.
        s.next_result();
        assert_eq!(s.cursor, 3);
    }

    #[test]
    fn cursor_at_visual_line_selects_headers() {
        let mut s = grouped_state();
        // Lines: 0=H0,1=m1,2=m2,3=H3,4=m4.
        assert!(s.cursor_at_visual_line(0)); // header → selectable
        assert_eq!(s.cursor, 0);
        assert!(s.cursor_at_visual_line(3)); // H3 → selectable
        assert_eq!(s.cursor, 3);
        assert!(!s.cursor_at_visual_line(2)); // a match row → not selectable
    }

    #[test]
    fn per_file_selection_toggles_and_summarizes() {
        let mut s = grouped_state();
        s.set_replace_mode(true);
        s.cursor = 0;
        s.toggle_selected_at_cursor();
        assert!(s.is_header_selected(0));
        assert_eq!(s.selected_summary(), (1, 1)); // 1 file, 1 match

        s.set_all_selected(true);
        assert!(s.is_header_selected(0) && s.is_header_selected(3));
        assert!(s.all_selected());
        assert_eq!(s.selected_summary().0, 2);

        s.set_all_selected(false);
        assert_eq!(s.selected_summary().0, 0);

        // Leaving replace mode clears the selection.
        s.toggle_selected_at_cursor();
        assert_eq!(s.selected_summary().0, 1);
        s.set_replace_mode(false);
        assert_eq!(s.selected_summary().0, 0);
    }

    #[test]
    fn checkbox_click_toggles_selection_only_in_replace_mode() {
        let mut s = grouped_state();
        // Not in replace mode → checkbox clicks are ignored.
        assert!(!s.toggle_selection_at_visual_click(0, 5));

        s.set_replace_mode(true);
        // Header at line 0; the checkbox spans columns 4..8.
        assert!(s.toggle_selection_at_visual_click(0, 5));
        assert!(s.is_header_selected(0));
        assert!(s.toggle_selection_at_visual_click(0, 5));
        assert!(!s.is_header_selected(0));
        // A click on the triangle region (cols 0..4) is not a checkbox click.
        assert!(!s.toggle_selection_at_visual_click(0, 1));
    }

    #[test]
    fn clicking_the_triangle_toggles_collapse() {
        let mut s = grouped_state();
        // Line 0 = header H0; the [▼] marker spans columns 0..4.
        assert!(s.toggle_collapse_at_visual_click(0, 1));
        assert!(s.tree_nodes[0].collapsed);
        // Clicking again expands.
        assert!(s.toggle_collapse_at_visual_click(0, 0));
        assert!(!s.tree_nodes[0].collapsed);
        // Outside the marker, or on a match line, does nothing.
        assert!(!s.toggle_collapse_at_visual_click(0, 20));
        assert!(!s.toggle_collapse_at_visual_click(1, 1));
    }

    #[test]
    fn page_nav_stops_at_ends_without_wrapping() {
        let mut s = grouped_state();
        s.cursor = 0;
        s.page_down(10);
        assert_eq!(s.cursor, 3); // last header
        s.page_up(10);
        assert_eq!(s.cursor, 0); // first header
    }

    #[test]
    fn replace_all_literal_rewrites_matched_files() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, "foo bar foo\nbaz\n").unwrap();

        let mut state = FileSearchState::new_content(dir.path().to_path_buf(), 1 << 20);
        state.tree_nodes = vec![header(f.clone())];
        state.selected_headers.insert(0);
        state.search_pattern = regex::escape("foo");
        state.search_use_regex = false;
        state.search_case_sensitive = true;

        assert_eq!(state.replace_all("X"), (1, 2));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "X bar X\nbaz\n");
    }

    #[test]
    fn replace_all_regex_expands_capture_groups() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("b.rs");
        std::fs::write(&f, "get_user(id)\n").unwrap();

        let mut state = FileSearchState::new_content(dir.path().to_path_buf(), 1 << 20);
        state.tree_nodes = vec![header(f.clone())];
        state.selected_headers.insert(0);
        state.search_pattern = r"get_(\w+)".to_string();
        state.search_use_regex = true;
        state.search_case_sensitive = true;

        assert_eq!(state.replace_all("fetch_$1"), (1, 1));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "fetch_user(id)\n");
    }

    #[test]
    fn literal_replace_treats_dollar_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("c.txt");
        std::fs::write(&f, "a.b\n").unwrap();

        let mut state = FileSearchState::new_content(dir.path().to_path_buf(), 1 << 20);
        state.tree_nodes = vec![header(f.clone())];
        state.selected_headers.insert(0);
        state.search_pattern = regex::escape(".");
        state.search_use_regex = false;
        state.search_case_sensitive = true;

        assert_eq!(state.replace_all("$1"), (1, 1));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "a$1b\n");
    }

    #[test]
    fn name_matcher_applies_glob_substring_regex_and_case() {
        let p = Path::new("/proj/src/Main.rs");
        let rel = "src/Main.rs";

        // Case-insensitive substring (default).
        assert_eq!(
            NameMatcher::new("main", false, false)
                .unwrap()
                .matches(p, rel),
            Some(true)
        );
        // Case-sensitive substring: "main" no longer matches "Main".
        assert_eq!(
            NameMatcher::new("main", false, true)
                .unwrap()
                .matches(p, rel),
            Some(false)
        );
        // Wildcard glob over the name, case-folded when Case is off.
        assert_eq!(
            NameMatcher::new("*.RS", false, false)
                .unwrap()
                .matches(p, rel),
            Some(true)
        );
        assert_eq!(
            NameMatcher::new("*.RS", false, true)
                .unwrap()
                .matches(p, rel),
            Some(false)
        );
        // Path-glob when the mask spans separators.
        assert_eq!(
            NameMatcher::new("src/*.rs", false, true)
                .unwrap()
                .matches(p, rel),
            Some(true)
        );
        // Regex over the name; case-sensitive anchored.
        assert_eq!(
            NameMatcher::new("^Main", true, true)
                .unwrap()
                .matches(p, rel),
            Some(true)
        );
        // Invalid regex → no matcher at all.
        assert!(NameMatcher::new("(", true, false).is_none());
    }

    #[test]
    fn name_search_honors_case_and_regex_toggles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        std::fs::write(dir.path().join("readme.txt"), "").unwrap();
        std::fs::write(dir.path().join("notes.rs"), "").unwrap();

        let names = |mask: &str, regex: bool, case: bool| -> Vec<String> {
            let cancel = AtomicBool::new(false);
            let mut out: Vec<String> = search_files(dir.path(), mask, regex, case, &cancel, None)
                .into_iter()
                .map(|r| r.relative_path)
                .collect();
            out.sort();
            out
        };

        // Substring, case-insensitive (default): both readme files.
        assert_eq!(
            names("readme", false, false),
            vec!["README.md", "readme.txt"]
        );
        // Substring, case-sensitive: only the lowercase one.
        assert_eq!(names("readme", false, true), vec!["readme.txt"]);
        // Regex, case-insensitive: anchored extension match on both readmes.
        assert_eq!(
            names(r"readme\.(md|txt)$", true, false),
            vec!["README.md", "readme.txt"]
        );
        // Regex, case-sensitive: only the uppercase README.
        assert_eq!(names(r"^README", true, true), vec!["README.md"]);
    }

    #[test]
    fn preview_replacement_builds_new_line() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = FileSearchState::new_content(dir.path().to_path_buf(), 1 << 20);
        state.search_pattern = regex::escape("foo");
        state.search_use_regex = false;
        state.search_case_sensitive = true;
        state.set_replace_text(Some("bar".to_string()));
        assert_eq!(
            state.preview_replacement("foo x foo").as_deref(),
            Some("bar x bar")
        );
        // No preview when replacement is empty.
        state.set_replace_text(Some(String::new()));
        assert!(state.preview_replacement("foo").is_none());
    }
}
