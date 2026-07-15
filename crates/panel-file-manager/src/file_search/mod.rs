//! File and content search state for file manager.
//!
//! Replaces TreeSearchModal's result display — search results are shown
//! in the file manager panel instead of a modal.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use regex::RegexBuilder;
use termide_git::{get_git_status, GitStatus};

mod navigation;
mod replace;
mod result_tree;
mod search_worker;

use result_tree::{build_tree_nodes, TreeBuildItem};
use search_worker::{search_content, search_files};

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
